//! Integration tests for star-gradebook (issue #85).
//!
//! Pinned behavior:
//! - Columns: index scope (active only; `include_hidden=true` widens to
//!   not_deleted), create defaults + 400 validation (bad_request, not 422),
//!   partial update incl. the `hidden` workflow_state virtual boolean,
//!   soft-delete destroy that renders the column JSON, reorder dedupe +
//!   positions 1..n + course scoping (and: no 403 gate, deviation 4).
//! - Column data: visible-students index scope, datum update with the blank
//!   content → delete-but-still-render quirk, 404 for non-visible students,
//!   bulk_update (400 empty, 404 unknown column via unscoped lookup, 403
//!   non-visible user, blank deletes) returning a completed
//!   `custom_columns_submissions_update` Progress.
//! - Late policy: show 404 when absent + no-timestamps shape, create
//!   (defaults, duplicate 400, validation 422), update (partial merge, 204,
//!   422 on invalid merged values).
//! - Permission chain 401 → 404 → 403 exercised on all 11 routes; an admin
//!   without an enrollment passes.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix plus
//! full teardown keeps shared-DB runs safe.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gbc85_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };

    let server =
        TestServer::new(star_gradebook::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $1) RETURNING id")
        .bind(format!("{prefix}-course"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active') RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

async fn seed_column(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    position: i32,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO custom_gradebook_columns \
         (title, position, workflow_state, course_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, now(), now()) RETURNING id",
    )
    .bind(title)
    .bind(position)
    .bind(workflow_state)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .expect("seed column")
}

async fn seed_datum(pool: &PgPool, column_id: i64, user_id: i64, content: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO custom_gradebook_column_data (content, user_id, custom_gradebook_column_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(content)
    .bind(user_id)
    .bind(column_id)
    .fetch_one(pool)
    .await
    .expect("seed datum")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // columns cascade to data (0227 FK); courses cascade to enrollments.
    let _ = sqlx::query(
        "DELETE FROM custom_gradebook_column_data WHERE custom_gradebook_column_id IN \
         (SELECT id FROM custom_gradebook_columns WHERE title LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM custom_gradebook_columns WHERE title LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM late_policies WHERE course_id IN \
         (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}-%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

#[tokio::test]
async fn column_index_scope_and_serializer_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let active = seed_column(&pool, course, &format!("{prefix}-A"), 1, "active").await;
    let hidden = seed_column(&pool, course, &format!("{prefix}-H"), 2, "hidden").await;
    let deleted = seed_column(&pool, course, &format!("{prefix}-D"), 3, "deleted").await;
    let _ = (active, hidden, deleted);

    let path = format!("/courses/{course}/custom_gradebook_columns");

    // Default scope: active only.
    let resp = server
        .get(&path)
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let list = body.as_array().expect("list");
    assert_eq!(list.len(), 1, "default scope hides hidden+deleted: {body}");
    assert_eq!(list[0]["title"], format!("{prefix}-A"));
    // serializer: exactly id, title, position, teacher_notes, read_only, hidden
    let mut keys: Vec<&str> = list[0]
        .as_object()
        .expect("obj")
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "hidden",
            "id",
            "position",
            "read_only",
            "teacher_notes",
            "title"
        ]
    );

    // include_hidden=true → not_deleted (hidden included, deleted still gone).
    let resp = server
        .get(&format!("{path}?include_hidden=true"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let list = body.as_array().expect("list");
    assert_eq!(
        list.len(),
        2,
        "include_hidden widens to not_deleted: {body}"
    );
    let hidden_col = list
        .iter()
        .find(|c| c["title"] == format!("{prefix}-H"))
        .expect("hidden col");
    assert_eq!(
        hidden_col["hidden"], true,
        "hidden state exposed as virtual boolean"
    );

    // Position ordering.
    let positions: Vec<i64> = list.iter().filter_map(|c| c["position"].as_i64()).collect();
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(positions, sorted, "ordered by position ASC");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn column_create_defaults_and_bad_request_validation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let path = format!("/courses/{course}/custom_gradebook_columns");
    let hdr = auth(teacher, "teacher");

    // Missing title → 400 (bad_request, NOT 422).
    let resp = server
        .post(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column": {"title": ""}}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // No column wrapper at all → 400.
    let resp = server
        .post(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Create with defaults → 201, position bottom, hidden=false.
    let resp = server
        .post(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column": {"title": format!("{prefix}-New")}}))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let col: Value = resp.json();
    assert_eq!(col["title"], format!("{prefix}-New"));
    assert_eq!(col["position"], 1, "first column lands at bottom = 1");
    assert_eq!(col["hidden"], false);
    assert_eq!(col["teacher_notes"], false);
    assert_eq!(col["read_only"], false);

    // Create with hidden=true + explicit position + flags.
    let resp = server
        .post(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(
            &json!({"column": {"title": format!("{prefix}-Notes"), "position": 5,
                                  "teacher_notes": true, "hidden": true}}),
        )
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let col: Value = resp.json();
    assert_eq!(
        col["hidden"], true,
        "hidden=true seeds workflow_state=hidden"
    );
    assert_eq!(col["teacher_notes"], true);
    assert_eq!(col["position"], 5);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn column_update_merge_and_destroy_soft_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let col = seed_column(&pool, course, &format!("{prefix}-U"), 2, "active").await;
    let path = format!("/courses/{course}/custom_gradebook_columns/{col}");
    let hdr = auth(teacher, "teacher");

    // Partial update: title only; other fields preserved.
    let resp = server
        .put(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column": {"title": format!("{prefix}-U2")}}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], format!("{prefix}-U2"));
    assert_eq!(body["position"], 2, "position preserved when omitted");
    assert_eq!(body["hidden"], false);

    // hidden=true → workflow_state hidden, exposed as the virtual boolean.
    let resp = server
        .put(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column": {"hidden": true}}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["hidden"], true);

    // Blank title on update → 400.
    let resp = server
        .put(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column": {"title": ""}}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Destroy: soft-delete, renders the column JSON.
    let resp = server
        .delete(&path)
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], col);
    assert_eq!(body["hidden"], false, "deleted column is not hidden");

    // Gone from every scope, second destroy → 404.
    let resp = server
        .get(&format!(
            "/courses/{course}/custom_gradebook_columns?include_hidden=true"
        ))
        .add_header("Authorization", hdr.clone())
        .await;
    let list: Value = resp.json();
    assert!(!list
        .as_array()
        .expect("list")
        .iter()
        .any(|c| c["id"] == col));
    let resp = server.delete(&path).add_header("Authorization", hdr).await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn column_reorder_dedupes_and_assigns_contiguous_positions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    seed_enrollment(&pool, student, course, "student").await;
    let c3 = seed_column(&pool, course, &format!("{prefix}-c3"), 3, "active").await;
    let c1 = seed_column(&pool, course, &format!("{prefix}-c1"), 1, "active").await;
    let c2 = seed_column(&pool, course, &format!("{prefix}-c2"), 2, "active").await;

    // A student (not a teacher) may reorder: no authorized_action upstream
    // (deviation 4). order[] has a duplicate: c2 appears twice.
    let resp = server
        .post(&format!(
            "/courses/{course}/custom_gradebook_columns/reorder"
        ))
        .add_header("Authorization", auth(student, "student"))
        .content_type("application/json")
        .json(&json!({"order": [c2, c2, c3, c1]}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.text(), "{}", "reorder answers 200 {{}}");

    let positions: Vec<(i64, i32)> = sqlx::query_as(
        "SELECT id, position FROM custom_gradebook_columns \
         WHERE course_id = $1 AND workflow_state <> 'deleted' ORDER BY position",
    )
    .bind(course)
    .fetch_all(&pool)
    .await
    .expect("positions");
    assert_eq!(
        positions,
        vec![(c2, 1), (c3, 2), (c1, 3)],
        "dedupe keeps first occurrence; positions are 1..n: {positions:?}"
    );

    // Empty order is a no-op 200.
    let resp = server
        .post(&format!(
            "/courses/{course}/custom_gradebook_columns/reorder"
        ))
        .add_header("Authorization", auth(student, "student"))
        .content_type("application/json")
        .json(&json!({"order": []}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Unknown course → 404.
    let resp = server
        .post("/courses/999999999/custom_gradebook_columns/reorder")
        .add_header("Authorization", auth(student, "student"))
        .content_type("application/json")
        .json(&json!({"order": [c1]}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A column of another course is not moved (course-scoped update).
    let other_course = seed_course(&pool, &format!("{prefix}-x")).await;
    let other_col = seed_column(&pool, other_course, &format!("{prefix}-other"), 7, "active").await;
    let resp = server
        .post(&format!(
            "/courses/{course}/custom_gradebook_columns/reorder"
        ))
        .add_header("Authorization", auth(student, "student"))
        .content_type("application/json")
        .json(&json!({"order": [other_col]}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let (pos,): (i32,) =
        sqlx::query_as("SELECT position FROM custom_gradebook_columns WHERE id = $1")
            .bind(other_col)
            .fetch_one(&pool)
            .await
            .expect("other col position");
    assert_eq!(pos, 7, "foreign-course column untouched");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Column data
// ---------------------------------------------------------------------------

#[tokio::test]
async fn datum_index_lists_visible_students_and_update_blank_deletes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let s1 = seed_user(&pool, &prefix, "s1", "student").await;
    let s2 = seed_user(&pool, &prefix, "s2", "student").await;
    let rejected = seed_user(&pool, &prefix, "rej", "student").await;
    let outsider = seed_user(&pool, &prefix, "out", "student").await;
    seed_enrollment(&pool, s1, course, "student").await;
    seed_enrollment(&pool, s2, course, "student").await;
    // rejected enrollment: not visible; outsider: enrolled in no course.
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
                 VALUES ($1, $2, 'student', 'rejected')",
    )
    .bind(rejected)
    .bind(course)
    .execute(&pool)
    .await
    .expect("seed rejected enrollment");

    let col = seed_column(&pool, course, &format!("{prefix}-data"), 1, "active").await;
    let hidden_col = seed_column(&pool, course, &format!("{prefix}-hdata"), 2, "hidden").await;
    seed_datum(&pool, col, s1, "12").await;
    seed_datum(&pool, col, rejected, "77").await;
    seed_datum(&pool, col, outsider, "88").await;
    seed_datum(&pool, hidden_col, s1, "secret").await;

    let hdr = auth(teacher, "teacher");
    let path = format!("/courses/{course}/custom_gradebook_columns/{col}/data");

    // Index: only data of visible students (s1 yes; rejected/outsider no).
    let resp = server
        .get(&path)
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let arr = list.as_array().expect("list");
    assert_eq!(arr.len(), 1, "only the visible student's datum: {list}");
    assert_eq!(arr[0]["user_id"], s1);
    assert_eq!(arr[0]["content"], "12");

    // Hidden column's data: 404 in default scope...
    let resp = server
        .get(&format!(
            "/courses/{course}/custom_gradebook_columns/{hidden_col}/data"
        ))
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    // ...200 under include_hidden.
    let resp = server
        .get(&format!(
            "/courses/{course}/custom_gradebook_columns/{hidden_col}/data?include_hidden=true"
        ))
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Update: set content.
    let resp = server
        .put(&format!("{path}/{s2}"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column_data": {"content": "A+"}}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["user_id"], s2);
    assert_eq!(body["content"], "A+");

    // Blank content deletes the datum but still renders its JSON.
    let resp = server
        .put(&format!("{path}/{s1}"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column_data": {"content": ""}}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["user_id"], s1, "deleted datum still renders");
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM custom_gradebook_column_data \
         WHERE custom_gradebook_column_id = $1 AND user_id = $2",
    )
    .bind(col)
    .bind(s1)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(count, 0, "blank content deleted the row");

    // Non-visible student → 404.
    let resp = server
        .put(&format!("{path}/{rejected}"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column_data": {"content": "x"}}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .put(&format!("{path}/{outsider}"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column_data": {"content": "x"}}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unknown column → 404.
    let resp = server
        .put(&format!(
            "/courses/{course}/custom_gradebook_columns/999999999/data/{s1}"
        ))
        .add_header("Authorization", hdr)
        .content_type("application/json")
        .json(&json!({"column_data": {"content": "x"}}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_update_validates_and_returns_completed_progress() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let s1 = seed_user(&pool, &prefix, "s1", "student").await;
    let s2 = seed_user(&pool, &prefix, "s2", "student").await;
    seed_enrollment(&pool, s1, course, "student").await;
    seed_enrollment(&pool, s2, course, "student").await;
    let col = seed_column(&pool, course, &format!("{prefix}-bulk"), 1, "active").await;
    // A column of ANOTHER course: the unscoped lookup finds it (no 404).
    let other_course = seed_course(&pool, &format!("{prefix}-x")).await;
    let other_col = seed_column(&pool, other_course, &format!("{prefix}-other"), 1, "active").await;

    let hdr = auth(teacher, "teacher");
    let path = format!("/courses/{course}/custom_gradebook_column_data");

    // Empty array → 400.
    let resp = server
        .put(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column_data": []}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Unknown column → 404 (unscoped lookup misses it entirely).
    let resp = server
        .put(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column_data": [{"column_id": 999999999, "user_id": s1, "content": "x"}]}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Non-visible user → 403.
    let ghost = seed_user(&pool, &prefix, "ghost", "student").await;
    let resp = server
        .put(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column_data": [{"column_id": col, "user_id": ghost, "content": "x"}]}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Mixed valid batch (upsert + blank-deletes + cross-course column).
    seed_datum(&pool, col, s1, "old").await;
    let resp = server
        .put(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column_data": [
            {"column_id": col, "user_id": s1, "content": ""},
            {"column_id": col, "user_id": s2, "content": "present"},
            {"column_id": other_col, "user_id": s1, "content": "cross"}
        ]}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let progress: Value = resp.json();
    assert_eq!(progress["tag"], "custom_columns_submissions_update");
    assert_eq!(
        progress["workflow_state"], "completed",
        "star runs it synchronously"
    );
    assert_eq!(progress["completion"], 100);
    assert_eq!(progress["context_type"], "Course");
    assert_eq!(progress["context_id"], course);
    assert!(
        progress["id"].as_i64().is_some(),
        "progress row id present: {progress}"
    );
    assert!(progress["url"]
        .as_str()
        .unwrap()
        .contains("/api/v1/progress/"));

    let (c1,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM custom_gradebook_column_data \
         WHERE custom_gradebook_column_id = $1 AND user_id = $2",
    )
    .bind(col)
    .bind(s1)
    .fetch_one(&pool)
    .await
    .expect("count s1");
    assert_eq!(c1, 0, "blank content deleted s1's datum");
    let (c2,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM custom_gradebook_column_data \
         WHERE custom_gradebook_column_id = $1 AND user_id = $2",
    )
    .bind(col)
    .bind(s2)
    .fetch_one(&pool)
    .await
    .expect("count s2");
    assert_eq!(c2, 1, "s2's datum upserted");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Late policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn late_policy_show_create_and_duplicate() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let path = format!("/courses/{course}/late_policy");
    let hdr = auth(teacher, "teacher");

    // Absent → 404.
    let resp = server
        .get(&path)
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Create with defaults → 201, wrapped, no timestamps in the payload.
    let resp = server
        .post(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"late_policy": {"missing_submission_deduction_enabled": true}}))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let lp = &body["late_policy"];
    assert!(lp["id"].as_i64().is_some());
    assert_eq!(lp["missing_submission_deduction_enabled"], true);
    assert_eq!(
        lp["missing_submission_deduction"].as_f64(),
        Some(100.0),
        "table default"
    );
    assert_eq!(lp["late_submission_deduction_enabled"], false);
    assert_eq!(lp["late_submission_deduction"].as_f64(), Some(0.0));
    assert_eq!(lp["late_submission_interval"], "day", "table default");
    assert_eq!(lp["late_submission_minimum_percent_enabled"], false);
    assert_eq!(lp["late_submission_minimum_percent"].as_f64(), Some(0.0));
    assert!(
        lp.get("created_at").is_none(),
        "serializer has no timestamps"
    );
    assert!(
        lp.get("updated_at").is_none(),
        "serializer has no timestamps"
    );

    // Show now returns the policy under the same wrapper.
    let resp = server
        .get(&path)
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["late_policy"]["late_submission_interval"], "day");

    // Duplicate → 400 with the Canvas message.
    let resp = server
        .post(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"late_policy": {"late_submission_deduction_enabled": true}}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert!(
        resp.text()
            .contains("only one late policy per course is allowed"),
        "Canvas duplicate message: {}",
        resp.text()
    );

    // Validation failure → 422 (percent out of 0..100).
    let other = seed_course(&pool, &format!("{prefix}-v")).await;
    let resp = server
        .post(&format!("/courses/{other}/late_policy"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(
            &json!({"late_policy": {"missing_submission_deduction_enabled": true,
                                       "missing_submission_deduction": 101}}),
        )
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    // Bad interval → 422.
    let other2 = seed_course(&pool, &format!("{prefix}-v2")).await;
    let resp = server
        .post(&format!("/courses/{other2}/late_policy"))
        .add_header("Authorization", hdr)
        .content_type("application/json")
        .json(&json!({"late_policy": {"late_submission_interval": "week"}}))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn late_policy_update_partial_merge_and_204() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let hdr = auth(teacher, "teacher");
    let path = format!("/courses/{course}/late_policy");

    // Absent → 404 on update too.
    let resp = server
        .patch(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"late_policy": {"late_submission_deduction_enabled": true}}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Seed a policy.
    sqlx::query(
        "INSERT INTO late_policies (course_id, missing_submission_deduction_enabled, \
         missing_submission_deduction, late_submission_interval, created_at, updated_at) \
         VALUES ($1, true, 100, 'day', now(), now())",
    )
    .bind(course)
    .execute(&pool)
    .await
    .expect("seed late policy");

    // Partial update: one field; 204 no content.
    let resp = server
        .patch(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(
            &json!({"late_policy": {"late_submission_deduction_enabled": true,
                                       "late_submission_deduction": 10,
                                       "late_submission_interval": "hour"}}),
        )
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    let body: Value = server
        .get(&path)
        .add_header("Authorization", hdr.clone())
        .await
        .json();
    let lp = &body["late_policy"];
    assert_eq!(
        lp["missing_submission_deduction_enabled"], true,
        "untouched field preserved"
    );
    assert_eq!(
        lp["missing_submission_deduction"].as_f64(),
        Some(100.0),
        "untouched field preserved"
    );
    assert_eq!(lp["late_submission_deduction_enabled"], true);
    assert_eq!(lp["late_submission_deduction"].as_f64(), Some(10.0));
    assert_eq!(lp["late_submission_interval"], "hour");

    // Invalid merged values → 422 (enabled flag from the store, bad value now).
    let resp = server
        .patch(&path)
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"late_policy": {"late_submission_deduction": 200}}))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Permission chain: 401 → 404 → 403 on all 11 routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn permission_chain_on_all_routes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    seed_enrollment(&pool, student, course, "student").await;
    let col = seed_column(&pool, course, &format!("{prefix}-perm"), 1, "active").await;

    let teacher_hdr = auth(teacher, "teacher");
    let student_hdr = auth(student, "student");

    // (method, path, needs body) for all 11 routes.
    let gets: Vec<String> = vec![
        format!("/courses/{course}/custom_gradebook_columns"),
        format!("/courses/{course}/custom_gradebook_columns/{col}/data"),
        format!("/courses/{course}/late_policy"),
    ];
    let with_bodies: Vec<(&str, String, Value)> = vec![
        (
            "POST",
            format!("/courses/{course}/custom_gradebook_columns"),
            json!({"column": {"title": "t"}}),
        ),
        (
            "POST",
            format!("/courses/{course}/custom_gradebook_columns/reorder"),
            json!({"order": [col]}),
        ),
        (
            "PUT",
            format!("/courses/{course}/custom_gradebook_columns/{col}"),
            json!({"column": {"title": "t"}}),
        ),
        (
            "DELETE",
            format!("/courses/{course}/custom_gradebook_columns/{col}"),
            json!({}),
        ),
        (
            "PUT",
            format!("/courses/{course}/custom_gradebook_columns/{col}/data/{student}"),
            json!({"column_data": {"content": "x"}}),
        ),
        (
            "PUT",
            format!("/courses/{course}/custom_gradebook_column_data"),
            json!({"column_data": [{"column_id": col, "user_id": student, "content": "x"}]}),
        ),
        (
            "POST",
            format!("/courses/{course}/late_policy"),
            json!({"late_policy": {"late_submission_deduction_enabled": true}}),
        ),
        (
            "PATCH",
            format!("/courses/{course}/late_policy"),
            json!({"late_policy": {"late_submission_deduction_enabled": true}}),
        ),
    ];

    // 401 without a token — every route.
    for path in &gets {
        let resp = server.get(path).await;
        assert_eq!(resp.status_code(), 401, "GET {path}: {}", resp.text());
    }
    for (method, path, body) in &with_bodies {
        let resp = match *method {
            "POST" => {
                server
                    .post(path)
                    .content_type("application/json")
                    .json(body)
                    .await
            }
            "PUT" => {
                server
                    .put(path)
                    .content_type("application/json")
                    .json(body)
                    .await
            }
            "PATCH" => {
                server
                    .patch(path)
                    .content_type("application/json")
                    .json(body)
                    .await
            }
            _ => server.delete(path).await,
        };
        assert_eq!(resp.status_code(), 401, "{method} {path}: {}", resp.text());
    }

    // 404 for an unknown course (auth first) — every route.
    let unknown = 999999999i64;
    let unknown_gets: Vec<String> = vec![
        format!("/courses/{unknown}/custom_gradebook_columns"),
        format!("/courses/{unknown}/custom_gradebook_columns/{col}/data"),
        format!("/courses/{unknown}/late_policy"),
    ];
    for path in &unknown_gets {
        let resp = server
            .get(path)
            .add_header("Authorization", teacher_hdr.clone())
            .await;
        assert_eq!(resp.status_code(), 404, "GET {path}: {}", resp.text());
    }
    let unknown_bodies: Vec<(&str, String, Value)> = vec![
        (
            "POST",
            format!("/courses/{unknown}/custom_gradebook_columns"),
            json!({"column": {"title": "t"}}),
        ),
        (
            "POST",
            format!("/courses/{unknown}/custom_gradebook_columns/reorder"),
            json!({"order": [col]}),
        ),
        (
            "PUT",
            format!("/courses/{unknown}/custom_gradebook_columns/{col}"),
            json!({"column": {"title": "t"}}),
        ),
        (
            "DELETE",
            format!("/courses/{unknown}/custom_gradebook_columns/{col}"),
            json!({}),
        ),
        (
            "PUT",
            format!("/courses/{unknown}/custom_gradebook_columns/{col}/data/{student}"),
            json!({"column_data": {"content": "x"}}),
        ),
        (
            "PUT",
            format!("/courses/{unknown}/custom_gradebook_column_data"),
            json!({"column_data": [{"column_id": col, "user_id": student, "content": "x"}]}),
        ),
        (
            "POST",
            format!("/courses/{unknown}/late_policy"),
            json!({"late_policy": {}}),
        ),
        (
            "PATCH",
            format!("/courses/{unknown}/late_policy"),
            json!({"late_policy": {}}),
        ),
    ];
    for (method, path, body) in &unknown_bodies {
        let resp = match *method {
            "POST" => {
                server
                    .post(path)
                    .add_header("Authorization", teacher_hdr.clone())
                    .content_type("application/json")
                    .json(body)
                    .await
            }
            "PUT" => {
                server
                    .put(path)
                    .add_header("Authorization", teacher_hdr.clone())
                    .content_type("application/json")
                    .json(body)
                    .await
            }
            "PATCH" => {
                server
                    .patch(path)
                    .add_header("Authorization", teacher_hdr.clone())
                    .content_type("application/json")
                    .json(body)
                    .await
            }
            _ => {
                server
                    .delete(path)
                    .add_header("Authorization", teacher_hdr.clone())
                    .await
            }
        };
        assert_eq!(resp.status_code(), 404, "{method} {path}: {}", resp.text());
    }

    // 403 for a plain student on the 10 gated routes (reorder is ungated,
    // deviation 4). The two GETs need existing records to reach the gate:
    // the column exists, and the late policy is absent — so create one
    // first via the API to test the late_policy GET gate.
    let resp = server
        .post(&format!("/courses/{course}/late_policy"))
        .add_header("Authorization", teacher_hdr.clone())
        .content_type("application/json")
        .json(&json!({"late_policy": {"missing_submission_deduction_enabled": true}}))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    for path in &gets {
        let resp = server
            .get(path)
            .add_header("Authorization", student_hdr.clone())
            .await;
        assert_eq!(resp.status_code(), 403, "GET {path}: {}", resp.text());
    }
    for (method, path, body) in &with_bodies {
        if path.ends_with("/reorder") {
            continue; // no 403 gate on reorder (deviation 4)
        }
        let resp = match *method {
            "POST" => {
                server
                    .post(path)
                    .add_header("Authorization", student_hdr.clone())
                    .content_type("application/json")
                    .json(body)
                    .await
            }
            "PUT" => {
                server
                    .put(path)
                    .add_header("Authorization", student_hdr.clone())
                    .content_type("application/json")
                    .json(body)
                    .await
            }
            "PATCH" => {
                server
                    .patch(path)
                    .add_header("Authorization", student_hdr.clone())
                    .content_type("application/json")
                    .json(body)
                    .await
            }
            _ => {
                server
                    .delete(path)
                    .add_header("Authorization", student_hdr.clone())
                    .await
            }
        };
        assert_eq!(resp.status_code(), 403, "{method} {path}: {}", resp.text());
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn admin_without_enrollment_passes_every_gate() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course = seed_course(&pool, &prefix).await;
    let admin = seed_user(&pool, &prefix, "root", "admin").await;
    // No enrollment for the admin — the global role carries the rights.
    let col = seed_column(&pool, course, &format!("{prefix}-adm"), 1, "active").await;
    let hdr = auth(admin, "admin");

    let resp = server
        .get(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .post(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column": {"title": format!("{prefix}-adm2")}}))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    let resp = server
        .put(&format!("/courses/{course}/custom_gradebook_columns/{col}"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"column": {"title": format!("{prefix}-adm3")}}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/courses/{course}/custom_gradebook_columns/{col}/data"
        ))
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .post(&format!("/courses/{course}/late_policy"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"late_policy": {"late_submission_deduction_enabled": true}}))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    let resp = server
        .patch(&format!("/courses/{course}/late_policy"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"late_policy": {"late_submission_deduction": 5}}))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
