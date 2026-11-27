//! PORT-B parity-gap tests (migration 0255 batch) for the nine module_gaps
//! routes:
//! - GET  /courses/{courseId}/module_item_redirect/{itemId}
//! - GET  /courses/{courseId}/module_item_sequence
//! - POST /courses/{courseId}/modules/{moduleId}/items/{itemId}/mark_read
//! - POST /courses/{courseId}/modules/{moduleId}/items/{itemId}/select_mastery_path
//! - PUT  /courses/{courseId}/modules (batch_update)
//! - POST /courses/{courseId}/modules/{moduleId}/duplicate
//! - PUT  /courses/{courseId}/modules/{moduleId}/relock
//! - GET  /courses/{courseId}/mastery_paths/stats/students_per_range
//! - GET  /courses/{courseId}/mastery_paths/stats/student_details
//!
//! Conventions mirror the rest of the workspace: tests skip cleanly when
//! DATABASE_URL is unset and clean up every row they create.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

/// An id far beyond any seeded row, for 404 probes.
const MISSING_ID: i64 = 999_999_999;

// ---------------------------------------------------------------------------
// Test harness (mirrors tests/module_items_api.rs)
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "mod-gaps-{}-{}",
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
        TestServer::new(star_modules::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_module(pool: &PgPool, course_id: i64, name: &str, position: i32) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO modules (course_id, name, position, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(name)
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed module")
}

async fn seed_module_item(
    pool: &PgPool,
    module_id: i64,
    item_type: &str,
    item_id: i64,
    position: i32,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO module_items (module_id, item_type, item_id, position)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(module_id)
    .bind(item_type)
    .bind(item_id)
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed module item")
}

/// ExternalUrl ContentTag — Canvas keeps the target in `url`.
async fn seed_external_url_item(
    pool: &PgPool,
    module_id: i64,
    item_id: i64,
    position: i32,
    url: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO module_items (module_id, item_type, item_id, position, url)
        VALUES ($1, 'external_url', $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(module_id)
    .bind(item_id)
    .bind(position)
    .bind(url)
    .fetch_one(pool)
    .await
    .expect("seed external url item")
}

async fn seed_page(pool: &PgPool, course_id: i64, title: &str, url: Option<&str>) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO pages (course_id, title, body, url)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(format!("{title} body"))
    .bind(url)
    .fetch_one(pool)
    .await
    .expect("seed page")
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, description, points_possible)
        VALUES ($1, $2, 'assignment description', 10)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str, state: &str) {
    sqlx::query(
        r#"
        INSERT INTO enrollments (user_id, course_id, role, workflow_state)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .bind(state)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

async fn seed_completion(pool: &PgPool, module_id: i64, item_id: i64, user_id: i64, state: &str) {
    sqlx::query(
        r#"
        INSERT INTO module_item_completions (module_id, item_id, user_id, state)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(module_id)
    .bind(item_id)
    .bind(user_id)
    .bind(state)
    .execute(pool)
    .await
    .expect("seed completion");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/module_item_redirect/{itemId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_item_redirect_targets_external_url_or_html_path() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "RedirectCourse").await;
    let module = seed_module(&pool, course, "Redirect Module", 1).await;
    let page = seed_page(&pool, course, "Home", Some("home")).await;
    let page_item = seed_module_item(&pool, module, "page", page, 1).await;
    let ext_item = seed_external_url_item(
        &pool,
        module,
        77001,
        2,
        Some("https://example.com/external"),
    )
    .await;
    let no_url_item = seed_external_url_item(&pool, module, 77002, 3, None).await;

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Anonymous -> 401 from the AuthUser extractor.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_redirect/{ext_item}"
        ))
        .await;
    assert_eq!(resp.status_code(), 401, "anonymous redirect");

    // No enrollment -> 403 (Canvas authorizes :read before resolving).
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_redirect/{ext_item}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "unenrolled redirect");

    // ExternalUrl items redirect to the stored url.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_redirect/{ext_item}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 302, "external url redirect");
    assert_eq!(
        resp.headers().get("location").unwrap(),
        "https://example.com/external"
    );

    // Other item types redirect to the item's html path.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_redirect/{page_item}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 302, "page redirect");
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/modules/items/{page_item}").as_str()
    );

    // ExternalUrl without a usable url falls back to the html path.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_redirect/{no_url_item}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 302, "url-less external redirect");
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/modules/items/{no_url_item}").as_str()
    );

    // Unknown item -> 404.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_redirect/{MISSING_ID}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "missing item redirect");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/module_item_sequence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_item_sequence_walks_course_items_across_modules() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "SequenceCourse").await;
    let other_course = seed_course(&pool, &prefix, "SequenceOther").await;
    let module_a = seed_module(&pool, course, "Sequence A", 1).await;
    let module_b = seed_module(&pool, course, "Sequence B", 2).await;

    let page_one = seed_page(&pool, course, "One", Some("one")).await;
    let page_two = seed_page(&pool, course, "Two", Some("two")).await;
    let quiz = seed_assignment(&pool, course, "Quiz").await;
    let essay = seed_assignment(&pool, course, "Essay").await;

    let item_one = seed_module_item(&pool, module_a, "page", page_one, 1).await;
    let item_two = seed_module_item(&pool, module_a, "page", page_two, 2).await;
    let item_quiz = seed_module_item(&pool, module_b, "assignment", quiz, 1).await;
    let item_essay = seed_module_item(&pool, module_b, "assignment", essay, 2).await;

    // An item of another course stays out of the walk.
    let foreign_module = seed_module(&pool, other_course, "Foreign", 1).await;
    let foreign_page = seed_page(&pool, other_course, "ForeignPage", Some("foreign")).await;
    let foreign_item = seed_module_item(&pool, foreign_module, "page", foreign_page, 1).await;

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // No enrollment -> 403.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_id={item_two}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "unenrolled sequence");

    // Middle of the course: prev/next cross the module boundary.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_id={item_two}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["items"]["current"]["id"], item_two);
    assert_eq!(body["items"]["current"]["type"], "Page");
    assert_eq!(body["items"]["current"]["page_url"], "two");
    assert_eq!(
        body["items"]["prev"]["id"], item_one,
        "prev is module A item 1"
    );
    assert_eq!(
        body["items"]["next"]["id"], item_quiz,
        "next crosses into module B"
    );
    assert_eq!(body["items"]["next"]["type"], "Assignment");

    // First item of the course: prev is null.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_id={item_one}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["items"]["current"]["id"], item_one);
    assert!(
        body["items"]["prev"].is_null(),
        "first item has no prev: {}",
        body["items"]["prev"]
    );
    assert_eq!(body["items"]["next"]["id"], item_two);

    // Last item of the course: next is null.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_id={item_essay}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["items"]["current"]["id"], item_essay);
    assert_eq!(body["items"]["prev"]["id"], item_quiz);
    assert!(
        body["items"]["next"].is_null(),
        "last item has no next: {}",
        body["items"]["next"]
    );

    // Unknown or foreign asset -> 404; missing asset_id -> 404.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_id={MISSING_ID}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown asset");

    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_id={foreign_item}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "foreign course asset");

    let resp = server
        .get(&format!("/courses/{course}/module_item_sequence"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "missing asset_id");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST .../items/{itemId}/mark_read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mark_item_read_upserts_completion_and_keeps_done_state() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "ReadCourse").await;
    let module = seed_module(&pool, course, "Read Module", 1).await;
    let page = seed_page(&pool, course, "Reading", Some("reading")).await;
    let item = seed_module_item(&pool, module, "page", page, 1).await;

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    async fn completion_state(pool: &PgPool, user_id: i64, item_id: i64) -> String {
        sqlx::query_scalar::<_, String>(
            "SELECT state FROM module_item_completions WHERE user_id = $1 AND item_id = $2",
        )
        .bind(user_id)
        .bind(item_id)
        .fetch_one(pool)
        .await
        .expect("read completion state")
    }

    // Mark read -> 204, one row in state 'read'.
    let resp = server
        .post(&format!(
            "/courses/{course}/modules/{module}/items/{item}/mark_read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "mark read");
    assert_eq!(completion_state(&pool, student, item).await, "read");

    // Idempotent: repeated reads keep a single row.
    let resp = server
        .post(&format!(
            "/courses/{course}/modules/{module}/items/{item}/mark_read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "repeat mark read");
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM module_item_completions WHERE item_id = $1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .expect("count completions");
    assert_eq!(rows, 1, "read upserts the same row");

    // Done dominates read: after PUT .../done flips the row to 'completed',
    // a later mark_read must not downgrade it.
    let resp = server
        .put(&format!(
            "/courses/{course}/modules/{module}/items/{item}/done"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "mark done");
    assert_eq!(completion_state(&pool, student, item).await, "completed");

    let resp = server
        .post(&format!(
            "/courses/{course}/modules/{module}/items/{item}/mark_read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "mark read after done");
    assert_eq!(
        completion_state(&pool, student, item).await,
        "completed",
        "a completed item must not be downgraded to 'read'"
    );

    // Unknown item -> 404.
    let resp = server
        .post(&format!(
            "/courses/{course}/modules/{module}/items/{MISSING_ID}/mark_read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "missing item mark read");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST .../items/{itemId}/select_mastery_path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn select_mastery_path_reports_the_disabled_feature() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "CyoeCourse").await;
    let module = seed_module(&pool, course, "CYOE Module", 1).await;
    let page = seed_page(&pool, course, "Choose", Some("choose")).await;
    let item = seed_module_item(&pool, module, "page", page, 1).await;

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Conditional release is off -> Canvas' 400 feature-disabled answer.
    let resp = server
        .post(&format!(
            "/courses/{course}/modules/{module}/items/{item}/select_mastery_path"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.text(),
        r#"{"errors":[{"message":"mastery paths not enabled"}]}"#
    );

    // Unknown item -> 404 (Canvas resolves the asset first).
    let resp = server
        .post(&format!(
            "/courses/{course}/modules/{module}/items/{MISSING_ID}/select_mastery_path"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "missing item select path");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /courses/{courseId}/modules (batch_update)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_update_modules_reorders_positions_in_given_order() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "BatchCourse").await;
    let other_course = seed_course(&pool, &prefix, "BatchOther").await;
    let module_a = seed_module(&pool, course, "Batch A", 1).await;
    let module_b = seed_module(&pool, course, "Batch B", 2).await;
    let module_c = seed_module(&pool, course, "Batch C", 3).await;
    let foreign = seed_module(&pool, other_course, "Foreign", 1).await;

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Students cannot reorder modules.
    let resp = server
        .put(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "module_ids": [module_c, module_a, module_b] }))
        .await;
    assert_eq!(resp.status_code(), 403, "student batch update");

    // Documented course[module_ids][] order: positions 1..n follow the array.
    let resp = server
        .put(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "module_ids": [module_c, module_a, module_b] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let updated = body.as_array().expect("modules array");
    assert_eq!(updated.len(), 3, "all listed modules returned");
    assert_eq!(updated[0]["id"], module_c);
    assert_eq!(updated[0]["position"], 1);
    assert_eq!(updated[1]["id"], module_a);
    assert_eq!(updated[1]["position"], 2);
    assert_eq!(updated[2]["id"], module_b);
    assert_eq!(updated[2]["position"], 3);

    // course[modules][] envelope carries the same ids.
    let resp = server
        .put(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "course": {
                "modules": [
                    { "id": module_a },
                    { "id": module_b },
                    { "id": module_c }
                ]
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let updated = body.as_array().expect("modules array");
    assert_eq!(updated[0]["id"], module_a);
    assert_eq!(updated[0]["position"], 1);
    assert_eq!(updated[1]["id"], module_b);
    assert_eq!(updated[1]["position"], 2);
    assert_eq!(updated[2]["id"], module_c);
    assert_eq!(updated[2]["position"], 3);

    // No ids at all -> 400.
    let resp = server
        .put(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "missing module_ids");
    assert_eq!(
        resp.text(),
        r#"{"errors":[{"message":"missing module_ids parameter"}]}"#
    );

    // A module from another course aborts the batch -> 404, nothing moves.
    let resp = server
        .put(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "module_ids": [module_a, foreign] }))
        .await;
    assert_eq!(resp.status_code(), 404, "foreign module id");
    let (position_a,): (i32,) = sqlx::query_as("SELECT position FROM modules WHERE id = $1")
        .bind(module_a)
        .fetch_one(&pool)
        .await
        .expect("module a position");
    assert_eq!(position_a, 1, "aborted batch keeps positions");

    // Modules of another course stay untouched (course-scoped update).
    let resp = server
        .put(&format!("/courses/{other_course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "module_ids": [module_a] }))
        .await;
    assert_eq!(resp.status_code(), 404, "cross-course module id");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /courses/{courseId}/modules/{moduleId}/duplicate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_module_copies_items_and_appends_at_the_end() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "DupCourse").await;
    let other_course = seed_course(&pool, &prefix, "DupOther").await;
    let source = seed_module(&pool, course, "Source Module", 1).await;
    let _second = seed_module(&pool, course, "Second Module", 2).await;

    let page_one = seed_page(&pool, course, "Alpha", Some("alpha")).await;
    let page_two = seed_page(&pool, course, "Beta", Some("beta")).await;
    seed_module_item(&pool, source, "page", page_one, 1).await;
    seed_module_item(&pool, source, "page", page_two, 2).await;

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Students cannot duplicate modules.
    let resp = server
        .post(&format!("/courses/{course}/modules/{source}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student duplicate");

    // Cross-course path -> 404.
    let resp = server
        .post(&format!(
            "/courses/{other_course}/modules/{source}/duplicate"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "cross-course duplicate");

    // Teacher duplicate -> appended copy with the same name and items.
    let resp = server
        .post(&format!("/courses/{course}/modules/{source}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let copy: Value = resp.json();
    let copy_id = copy["id"].as_i64().expect("copy id");
    assert_ne!(copy_id, source, "duplicate is a new module");
    assert_eq!(copy["name"], "Source Module", "Canvas keeps the same name");
    assert_eq!(copy["course_id"], course);
    assert_eq!(copy["position"], 3, "appended after the last module");
    assert_eq!(copy["workflow_state"], "active");

    // The copy shares the linked content (Canvas duplicates the ContentTags).
    let copied_items: Vec<(i64, i32)> = sqlx::query_as(
        "SELECT item_id, position FROM module_items WHERE module_id = $1 ORDER BY position",
    )
    .bind(copy_id)
    .fetch_all(&pool)
    .await
    .expect("read copied items");
    assert_eq!(
        copied_items,
        vec![(page_one, 1), (page_two, 2)],
        "items copied in order, pointing at the same content"
    );

    // The source module is untouched.
    let source_items: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM module_items WHERE module_id = $1")
            .bind(source)
            .fetch_one(&pool)
            .await
            .expect("count source items");
    assert_eq!(source_items, 2, "source keeps its items");

    // Unknown module -> 404.
    let resp = server
        .post(&format!("/courses/{course}/modules/{MISSING_ID}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "missing module duplicate");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /courses/{courseId}/modules/{moduleId}/relock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn relock_module_clears_its_completions_and_returns_the_module() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "RelockCourse").await;
    let module = seed_module(&pool, course, "Relock Module", 1).await;
    let other_module = seed_module(&pool, course, "Other Module", 2).await;

    let page = seed_page(&pool, course, "Locked", Some("locked")).await;
    let other_page = seed_page(&pool, course, "Other", Some("other")).await;
    let item = seed_module_item(&pool, module, "page", page, 1).await;
    let other_item = seed_module_item(&pool, other_module, "page", other_page, 1).await;

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    seed_completion(&pool, module, item, student, "completed").await;
    seed_completion(&pool, other_module, other_item, student, "completed").await;

    // Students cannot relock.
    let resp = server
        .put(&format!("/courses/{course}/modules/{module}/relock"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student relock");

    // Teacher relock -> module JSON, completions reset for that module only.
    let resp = server
        .put(&format!("/courses/{course}/modules/{module}/relock"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], module);
    assert_eq!(body["name"], "Relock Module");

    let reset: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM module_item_completions WHERE module_id = $1")
            .bind(module)
            .fetch_one(&pool)
            .await
            .expect("count reset completions");
    assert_eq!(reset, 0, "module progress is reset");

    let kept: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM module_item_completions WHERE module_id = $1")
            .bind(other_module)
            .fetch_one(&pool)
            .await
            .expect("count kept completions");
    assert_eq!(kept, 1, "other modules keep their progress");

    // Unknown module -> 404.
    let resp = server
        .put(&format!("/courses/{course}/modules/{MISSING_ID}/relock"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "missing module relock");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// /courses/{courseId}/mastery_paths/stats/{students_per_range,student_details}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mastery_paths_stats_return_the_feature_off_shapes() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "StatsCourse").await;

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Conditional release is disabled -> the same feature-off shapes the
    // /mastery_paths/stats/{stat} wildcard in star-courses returns.
    let resp = server
        .get(&format!(
            "/courses/{course}/mastery_paths/stats/students_per_range"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({ "ranges": [] }));

    let resp = server
        .get(&format!(
            "/courses/{course}/mastery_paths/stats/student_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({ "students": [] }));

    // Students and unknown courses follow the stats gating chain.
    let resp = server
        .get(&format!(
            "/courses/{course}/mastery_paths/stats/students_per_range"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student stats");

    let resp = server
        .get(&format!(
            "/courses/{MISSING_ID}/mastery_paths/stats/student_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown course stats");

    cleanup(&pool, &prefix).await;
}
