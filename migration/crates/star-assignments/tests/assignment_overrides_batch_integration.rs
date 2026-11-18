//! Integration tests for the Canvas assignment overrides batch + alias
//! routes in star-assignments (issue #55).
//!
//! Covers:
//! - GET    /courses/{courseId}/assignments/overrides            (batch_retrieve)
//! - POST   /courses/{courseId}/assignments/overrides            (batch_create)
//! - PUT    /courses/{courseId}/assignments/overrides            (batch_update)
//! - GET    /groups/{groupId}/assignments/{assignmentId}/override (group_alias)
//! - GET    /sections/{sectionId}/assignments/{assignmentId}/override (section_alias)
//!
//! Skipped cleanly when DATABASE_URL is not set, mirroring
//! peer_reviews_integration.rs. Unique prefix per run + full teardown.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "ovbatch-{}-{}",
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
        jwt_secret: "test-jwt-secret".to_string(),
    };

    let server = TestServer::new(
        axum::Router::new()
            .merge(star_assignments::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, 'x', $4)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(sortable_name)
    .bind(format!("{prefix}-{name}@test.local"))
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

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

async fn seed_section(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO course_sections (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed section")
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, points_possible)
        VALUES ($1, $2, 10)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

/// Create an override directly (bypassing the API under test).
async fn seed_override(
    pool: &PgPool,
    assignment_id: i64,
    title: &str,
    due_at: Option<&str>,
    section_id: Option<i64>,
    group_id: Option<i64>,
    student_ids: &[i64],
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignment_overrides
            (assignment_id, title, due_at, course_section_id, group_id, student_ids)
        VALUES ($1, $2, $3::timestamptz, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(title)
    .bind(due_at)
    .bind(section_id)
    .bind(group_id)
    .bind(student_ids)
    .fetch_one(pool)
    .await
    .expect("seed override")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let user_ids = "SELECT id FROM users WHERE email LIKE $1";
    let course_ids = "SELECT id FROM courses WHERE course_code LIKE $1";
    let _ = sqlx::query(&format!(
        "DELETE FROM assignment_overrides WHERE assignment_id IN \
         (SELECT id FROM assignments WHERE course_id IN ({course_ids}))"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM assignments WHERE course_id IN ({course_ids})"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM course_sections WHERE course_id IN ({course_ids})"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM group_memberships WHERE group_id IN \
         (SELECT id FROM groups WHERE course_id IN ({course_ids}))"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM groups WHERE course_id IN ({course_ids})"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM enrollments WHERE user_id IN ({user_ids})"
    ))
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// batch_retrieve
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_retrieve_returns_request_order_with_nulls() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "BatchRetrieve").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let a1 = seed_assignment(&pool, course, "A1").await;
    let a2 = seed_assignment(&pool, course, "A2").await;
    let o1 = seed_override(&pool, a1, "Section A", None, None, None, &[]).await;
    let o2 = seed_override(&pool, a2, "Section B", None, None, None, &[]).await;

    let query = format!(
        "/courses/{course}/assignments/overrides?assignment_overrides[][id]={o2}\
         &assignment_overrides[][assignment_id]={a2}\
         &assignment_overrides[][id]={o1}\
         &assignment_overrides[][assignment_id]={a1}\
         &assignment_overrides[][id]=999999\
         &assignment_overrides[][assignment_id]={a1}"
    );
    let resp = server
        .get(&query)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let ids: Vec<Value> = body
        .as_array()
        .expect("array response")
        .iter()
        .map(|entry| entry.get("id").cloned().unwrap_or(Value::Null))
        .collect();
    // Request order is preserved; unknown override ids answer null.
    assert_eq!(ids, vec![json!(o2), json!(o1), Value::Null], "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn batch_retrieve_visibility_and_errors() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "BatchVisibility").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other, course, "student").await;
    let a1 = seed_assignment(&pool, course, "A1").await;
    let a2 = seed_assignment(&pool, course, "A2").await;
    // o1 targets `other` (invisible to `student`), o2 targets `student`'s
    // adhoc set, o3 belongs to an unknown assignment id.
    let o1 = seed_override(&pool, a1, "Adhoc Other", None, None, None, &[other]).await;
    let o2 = seed_override(&pool, a2, "Adhoc Student", None, None, None, &[student]).await;
    let _o3 = seed_override(&pool, a1, "Stays", None, None, None, &[]).await;

    // A student only sees overrides targeting them; unknown assignments
    // silently drop to null (request order preserved).
    let query = format!(
        "/courses/{course}/assignments/overrides?assignment_overrides[][id]={o1}\
         &assignment_overrides[][assignment_id]={a1}\
         &assignment_overrides[][id]={o2}\
         &assignment_overrides[][assignment_id]={a2}\
         &assignment_overrides[][id]={o2}\
         &assignment_overrides[][assignment_id]=424242"
    );
    let resp = server
        .get(&query)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let arr = body.as_array().expect("array response");
    assert_eq!(arr.len(), 3, "body: {body}");
    assert!(arr[0].is_null(), "invisible override must be null: {body}");
    assert_eq!(arr[1]["id"], json!(o2), "body: {body}");
    assert!(arr[2].is_null(), "unknown assignment must be null: {body}");

    // Missing entries entirely.
    let resp = server
        .get(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json(),
        json!({ "errors": ["no assignment_overrides values present"] })
    );

    // Entries without both keys.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/overrides?assignment_overrides[][id]={o1}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json(),
        json!({ "errors": ["must specify an array with entry format { id, assignment_id }"] })
    );

    // Unenrolled callers are forbidden.
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/overrides?assignment_overrides[][id]=1\
             &assignment_overrides[][assignment_id]=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unknown courses are 404.
    let resp = server
        .get(
            "/courses/424242/assignments/overrides?assignment_overrides[][id]=1\
              &assignment_overrides[][assignment_id]=1",
        )
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// batch_create / batch_update
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_create_creates_all_or_nothing() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Alice", "student").await;
    let student2 = seed_user(&pool, &prefix, "Bob", "student").await;
    let course = seed_course(&pool, &prefix, "BatchCreate").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, student2, course, "student").await;
    let a1 = seed_assignment(&pool, course, "A1").await;
    let a2 = seed_assignment(&pool, course, "A2").await;
    let section = seed_section(&pool, course, "Section 1").await;

    let resp = server
        .post(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "assignment_id": a1, "student_ids": [student, student2], "due_at": "2027-01-15T12:00:00Z" },
            { "assignment_id": a2, "course_section_id": section, "unlock_at": "2027-01-01T00:00:00Z" },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let arr = body.as_array().expect("array response");
    assert_eq!(arr.len(), 2, "body: {body}");
    assert_eq!(
        arr[0]["student_ids"],
        json!([student, student2]),
        "body: {body}"
    );
    // Adhoc title defaults to the student count; section titles to the name.
    assert_eq!(arr[0]["title"], json!("2 students"), "body: {body}");
    assert_eq!(arr[1]["course_section_id"], json!(section), "body: {body}");
    assert_eq!(arr[1]["title"], json!("Section 1"), "body: {body}");
    assert!(arr[0]["due_at"].is_string(), "body: {body}");
    assert!(arr[1]["unlock_at"].is_string(), "body: {body}");

    let created1 = arr[0]["id"].as_i64().expect("override id");
    let created2 = arr[1]["id"].as_i64().expect("override id");

    // An invalid entry aborts the whole batch (all-or-nothing).
    let resp = server
        .post(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "assignment_id": a1, "course_section_id": 999999 },
            { "assignment_id": a1, "student_ids": [student] },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"],
        json!([["unknown section id 999999"], null]),
        "body: {body}"
    );

    // Neither entry of the failed batch exists; the first batch remains.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM assignment_overrides WHERE assignment_id = ANY($1)",
    )
    .bind(vec![a1, a2])
    .fetch_one(&pool)
    .await
    .expect("count overrides");
    assert_eq!(count, 2, "failed batch must not create rows");
    let still_there: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM assignment_overrides WHERE id = ANY($1)")
            .bind(vec![created1, created2])
            .fetch_one(&pool)
            .await
            .expect("count overrides");
    assert_eq!(still_there, 2);

    // Structure errors: id on create, missing assignment_id. Canvas stops
    // before interpreting any entry once a structure error exists, so the
    // third entry reports nothing.
    let resp = server
        .post(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "id": created1, "assignment_id": a1, "student_ids": [student] },
            { "student_ids": [student] },
            { "assignment_id": a1, "student_ids": [424242] },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"],
        json!([
            ["may not specify an override id"],
            ["must specify an assignment id"],
            null
        ]),
        "body: {body}"
    );

    // Interpretation errors surface when the structure is sound.
    let resp = server
        .post(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "assignment_id": a1, "student_ids": [424242] },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"],
        json!([["unknown student ids: [\"424242\"]"]]),
        "body: {body}"
    );

    // Payload shape errors.
    let resp = server
        .post(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json(),
        json!({ "errors": ["no assignment override data present"] })
    );

    let resp = server
        .post(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": 5 }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json(),
        json!({ "errors": ["must specify an array of overrides"] })
    );

    // Students may not batch-create (Canvas manage_assignments_edit).
    let resp = server
        .post(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "assignment_id": a1, "student_ids": [student] }
        ]}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn batch_update_clears_dates_and_keeps_adhoc_set() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Alice", "student").await;
    let student2 = seed_user(&pool, &prefix, "Bob", "student").await;
    let course = seed_course(&pool, &prefix, "BatchUpdate").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, student2, course, "student").await;
    let a1 = seed_assignment(&pool, course, "A1").await;
    let adhoc = seed_override(
        &pool,
        a1,
        "Adhoc",
        Some("2027-02-01T10:00:00Z"),
        None,
        None,
        &[student],
    )
    .await;
    let section = seed_section(&pool, course, "Sec").await;
    let section_override = seed_override(
        &pool,
        a1,
        "Sec",
        Some("2027-02-01T10:00:00Z"),
        Some(section),
        None,
        &[],
    )
    .await;

    // Adhoc update without student_ids keeps the set; date keys replace.
    let resp = server
        .put(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "id": adhoc, "assignment_id": a1, "due_at": "2027-03-01T09:00:00Z" },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body[0]["student_ids"], json!([student]), "body: {body}");
    assert!(body[0]["due_at"].is_string(), "body: {body}");
    assert!(body[0].get("unlock_at").is_none(), "body: {body}");

    // Omitted dates clear the override values (Canvas clear_*_override).
    let resp = server
        .put(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "id": adhoc, "assignment_id": a1, "student_ids": [student, student2] },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body[0]["student_ids"],
        json!([student, student2]),
        "body: {body}"
    );
    assert!(body[0].get("due_at").is_none(), "cleared due_at: {body}");

    // Section overrides keep their target; the title survives a date-only
    // update.
    let resp = server
        .put(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "id": section_override, "assignment_id": a1, "lock_at": "2027-04-01T00:00:00Z" },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body[0]["course_section_id"], json!(section), "body: {body}");
    assert_eq!(body[0]["title"], json!("Sec"), "body: {body}");
    assert!(body[0]["lock_at"].is_string(), "body: {body}");
    assert!(body[0].get("due_at").is_none(), "cleared due_at: {body}");

    // Unknown override/assignment ids surface as aligned errors.
    let resp = server
        .put(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "assignment_id": 424242, "id": adhoc },
            { "assignment_id": a1, "id": 424242 },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"],
        json!([["assignment not found"], ["override not found"]]),
        "body: {body}"
    );

    // Missing id / missing assignment_id (structure errors — Canvas stops
    // before interpreting, mirroring the rspec `fails if ids not present`).
    let resp = server
        .put(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "assignment_id": a1, "title": "foo" },
            { "id": adhoc, "title": "bar", "due_at": "not-a-date" },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"],
        json!([
            ["must specify an override id"],
            ["must specify an assignment id"]
        ]),
        "body: {body}"
    );

    // Invalid dates surface when the structure is sound.
    let resp = server
        .put(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "id": adhoc, "assignment_id": a1, "due_at": "not-a-date" },
        ]}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"],
        json!([["invalid due_at \"not-a-date\""]]),
        "body: {body}"
    );

    // Students may not batch-update.
    let resp = server
        .put(&format!("/courses/{course}/assignments/overrides"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .content_type("application/json")
        .json(&json!({ "assignment_overrides": [
            { "id": adhoc, "assignment_id": a1, "due_at": "2027-05-01T00:00:00Z" }
        ]}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// group_alias / section_alias
// ---------------------------------------------------------------------------

#[tokio::test]
async fn aliases_redirect_to_canonical_override_or_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let section_student = seed_user(&pool, &prefix, "SectionKid", "student").await;
    let course = seed_course(&pool, &prefix, "Aliases").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, member, course, "student").await;
    seed_enrollment(&pool, outsider, course, "student").await;
    seed_enrollment(&pool, section_student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "A1").await;

    let group = seed_group(&pool, course, "Group 1").await;
    sqlx::query("INSERT INTO group_memberships (group_id, user_id, workflow_state) VALUES ($1, $2, 'accepted')")
        .bind(group)
        .bind(member)
        .execute(&pool)
        .await
        .expect("seed membership");

    let section = seed_section(&pool, course, "Section 1").await;
    // Enroll the section student with the section id.
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state, course_section_id) \
         VALUES ($1, $2, 'student', 'active', $3)",
    )
    .bind(section_student)
    .bind(course)
    .bind(section)
    .execute(&pool)
    .await
    .expect("seed sectioned enrollment");

    let group_override =
        seed_override(&pool, assignment, "Group 1", None, None, Some(group), &[]).await;
    let section_override = seed_override(
        &pool,
        assignment,
        "Section 1",
        None,
        Some(section),
        None,
        &[],
    )
    .await;

    // Teacher: 302 with the canonical Location.
    let resp = server
        .get(&format!(
            "/groups/{group}/assignments/{assignment}/override"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 302, "body: {}", resp.text());
    assert_eq!(
        resp.headers().get(axum::http::header::LOCATION).unwrap(),
        format!("/api/v1/courses/{course}/assignments/{assignment}/overrides/{group_override}")
    );

    // Group member sees their group's override; an unaffiliated student does
    // not (visibility), answering 404.
    let resp = server
        .get(&format!(
            "/groups/{group}/assignments/{assignment}/override"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 302, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/groups/{group}/assignments/{assignment}/override"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Section alias: sectioned student + teacher; unknown section 404.
    let resp = server
        .get(&format!(
            "/sections/{section}/assignments/{assignment}/override"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(section_student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 302, "body: {}", resp.text());
    assert_eq!(
        resp.headers().get(axum::http::header::LOCATION).unwrap(),
        format!("/api/v1/courses/{course}/assignments/{assignment}/overrides/{section_override}")
    );

    let resp = server
        .get(&format!(
            "/sections/{section}/assignments/{assignment}/override"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/sections/424242/assignments/{assignment}/override"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Assignment from another course / unknown assignment: 404.
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    let other_assignment = seed_assignment(&pool, other_course, "B1").await;
    let resp = server
        .get(&format!(
            "/groups/{group}/assignments/{other_assignment}/override"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // No override for the group at all: 404.
    let group2 = seed_group(&pool, course, "Group 2").await;
    let resp = server
        .get(&format!(
            "/groups/{group2}/assignments/{assignment}/override"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
