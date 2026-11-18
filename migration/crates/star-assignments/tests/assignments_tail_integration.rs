//! Integration tests for the Canvas assignments API tail routes in
//! star-assignments (issue #55).
//!
//! Covers:
//! - GET  /users/{userId}/courses/{courseId}/assignments                       (user_index)
//! - GET  /courses/{courseId}/assignments/{assignmentId}/users/{userId}/group_members
//! - POST /courses/{courseId}/assignments/{assignmentId}/duplicate
//! - PUT  /courses/{courseId}/assignments/bulk_update
//! - POST /courses/{courseId}/assignments/{assignmentId}/retry_alignment_clone
//! - POST /courses/{courseId}/assignments/{assignmentId}/accessibility/scan
//! - POST /courses/{courseId}/assignments/{assignmentId}/accessibility/queue_scan
//!
//! Skipped cleanly when DATABASE_URL is not set. The accessibility tests use
//! a dedicated account per run (never the root account) for feature flags.

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
        "asmtail-{}-{}",
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

async fn seed_course_in_account(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code, account_id)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
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

async fn seed_assignment_in_group(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    group_id: i64,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, points_possible, assignment_group_id)
        VALUES ($1, $2, 10, $3)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(group_id)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignment_groups (course_id, name) VALUES ($1, $2) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed assignment group")
}

async fn seed_override(
    pool: &PgPool,
    assignment_id: i64,
    title: &str,
    due_at: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignment_overrides (assignment_id, title, due_at)
        VALUES ($1, $2, $3::timestamptz)
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(title)
    .bind(due_at)
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
        "DELETE FROM feature_flags WHERE (context_type = 'Course' AND context_id IN ({course_ids})) \
         OR (context_type = 'Account' AND context_id IN (SELECT id FROM accounts WHERE name LIKE $1))"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM assignment_overrides WHERE assignment_id IN \
         (SELECT id FROM assignments WHERE course_id IN ({course_ids}))"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM assignment_groups WHERE course_id IN ({course_ids})"
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
        "DELETE FROM enrollments WHERE user_id IN ({user_ids})"
    ))
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// user_index
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_index_visibility_matrix() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other_student = seed_user(&pool, &prefix, "Other", "student").await;
    let other_teacher = seed_user(&pool, &prefix, "OtherTeacher", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "UserIndex").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, other_teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other_student, course, "student").await;
    let a1 = seed_assignment(&pool, course, "A1").await;
    let a2 = seed_assignment(&pool, course, "A2").await;

    // Self: the "self" spelling.
    let resp = server
        .get(&format!("/users/self/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let titles: Vec<&str> = body
        .as_array()
        .expect("array")
        .iter()
        .map(|a| a["title"].as_str().expect("title"))
        .collect();
    assert_eq!(titles, vec!["A1", "A2"], "body: {body}");

    // Self by id.
    let resp = server
        .get(&format!("/users/{student}/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Teacher viewing a student of the course.
    let resp = server
        .get(&format!("/users/{student}/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().map(Vec::len), Some(2), "body: {body}");

    // Global admin viewing a student.
    let resp = server
        .get(&format!("/users/{student}/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Student viewing another student: forbidden.
    let resp = server
        .get(&format!(
            "/users/{other_student}/courses/{course}/assignments"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Teacher viewing a teacher (not a student): forbidden.
    let resp = server
        .get(&format!(
            "/users/{other_teacher}/courses/{course}/assignments"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Teacher viewing an unenrolled user: the target cannot read the course.
    let resp = server
        .get(&format!("/users/{outsider}/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unenrolled caller asking for themselves: target-read gate.
    let resp = server
        .get(&format!("/users/self/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unknown user / unknown course.
    let resp = server
        .get(&format!("/users/424242/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .get(&format!("/users/{student}/courses/424242/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The seeded ids are real ids (sanity for the assertions above).
    assert!(a1 > 0 && a2 > 0);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// student_group_members
// ---------------------------------------------------------------------------

#[tokio::test]
async fn student_group_members_returns_student_entry() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let teacher_no_course = seed_user(&pool, &prefix, "OtherTeacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "GroupMembers").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "A1").await;

    // Happy path: the student's own entry (Canvas' non-group branch), with
    // the id serialized as a string.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/users/{student}/group_members"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body,
        json!([{ "id": student.to_string(), "name": "Student" }]),
        "body: {body}"
    );

    // The student themselves may call it.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/users/{student}/group_members"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // A teacher of another course: forbidden (Canvas rspec parity).
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/users/{student}/group_members"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(teacher_no_course, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A non-student of the course: 404.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/users/{teacher}/group_members"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Bogus assignment id: 404.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/424242/users/{student}/group_members"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// duplicate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_copies_assignment_with_canvas_title_rules() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Duplicate").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let group = seed_group(&pool, course, "Assignments").await;
    let essay = seed_assignment_in_group(&pool, course, "Essay", group).await;

    // First duplicate: "Essay Copy".
    let resp = server
        .post(&format!("/courses/{course}/assignments/{essay}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], json!("Essay Copy"), "body: {body}");
    assert_eq!(body["course_id"], json!(course), "body: {body}");
    assert_eq!(body["points_possible"], json!(10), "body: {body}");
    let copy_id = body["id"].as_i64().expect("copy id");
    // new_positions maps every assignment of the group (1-based, id order).
    let mut expected_positions = serde_json::Map::new();
    expected_positions.insert(essay.to_string(), json!(1));
    expected_positions.insert(copy_id.to_string(), json!(2));
    assert_eq!(
        body["new_positions"],
        Value::Object(expected_positions),
        "body: {body}"
    );

    // Second duplicate of the same source: "Essay Copy 2".
    let resp = server
        .post(&format!("/courses/{course}/assignments/{essay}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], json!("Essay Copy 2"), "body: {body}");

    // Duplicating the copy: its title ends in digits, so the number
    // increments past the existing conflicts ("Essay Copy 2" is taken).
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{copy_id}/duplicate"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], json!("Essay Copy 3"), "body: {body}");

    // Missing source: Canvas' 400 body.
    let resp = server
        .post(&format!("/courses/{course}/assignments/424242/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.json(), json!({ "error": "assignment does not exist" }));

    // Students may not duplicate.
    let resp = server
        .post(&format!("/courses/{course}/assignments/{essay}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Cross-course duplicate without rights on the target: forbidden.
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{essay}/duplicate?target_course_id={other_course}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unknown course: 404.
    let resp = server
        .post(&format!("/courses/424242/assignments/{essay}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// bulk_update
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bulk_updates_dates_and_answers_progress() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "BulkUpdate").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let a1 = seed_assignment(&pool, course, "A1").await;
    let a2 = seed_assignment(&pool, course, "A2").await;
    let o1 = seed_override(&pool, a1, "Adhoc", Some("2027-01-01T00:00:00Z")).await;

    // Happy path: base due_at + override dates in one request.
    let resp = server
        .put(&format!("/courses/{course}/assignments/bulk_update"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!([
            { "id": a1, "all_dates": [
                { "base": true, "due_at": "2027-06-01T12:00:00Z" },
                { "id": o1, "due_at": "2027-07-01T12:00:00Z", "unlock_at": "2027-06-15T00:00:00Z" },
            ]},
            { "id": a2, "all_dates": [ { "base": true, "due_at": null } ]},
        ]))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["tag"], json!("assignment_bulk_update"), "body: {body}");
    assert_eq!(body["workflow_state"], json!("completed"), "body: {body}");
    assert_eq!(body["completion"], json!(100), "body: {body}");
    assert_eq!(body["context_id"], json!(course), "body: {body}");
    assert_eq!(
        body["results"],
        json!({ "updated_count": 2 }),
        "body: {body}"
    );

    let (due,): (Option<String>,) =
        sqlx::query_as("SELECT due_at::text FROM assignments WHERE id = $1")
            .bind(a1)
            .fetch_one(&pool)
            .await
            .expect("fetch a1");
    assert!(
        due.is_some_and(|value| value.starts_with("2027-06-01")),
        "due: {due:?}"
    );

    let (due, unlock, lock): (Option<String>, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT due_at::text, unlock_at::text, lock_at::text FROM assignment_overrides WHERE id = $1",
    )
    .bind(o1)
    .fetch_one(&pool)
    .await
    .expect("fetch override");
    assert!(
        due.is_some_and(|v| v.starts_with("2027-07-01")),
        "due: {due:?}"
    );
    assert!(
        unlock.is_some_and(|v| v.starts_with("2027-06-15")),
        "unlock: {unlock:?}"
    );
    assert!(
        lock.is_none(),
        "lock must clear when the key is absent: {lock:?}"
    );

    let (due2,): (Option<String>,) =
        sqlx::query_as("SELECT due_at::text FROM assignments WHERE id = $1")
            .bind(a2)
            .fetch_one(&pool)
            .await
            .expect("fetch a2");
    assert!(due2.is_none(), "null due_at clears the base date: {due2:?}");

    // Students are forbidden.
    let resp = server
        .put(&format!("/courses/{course}/assignments/bulk_update"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .content_type("application/json")
        .json(&json!([{ "id": a1 }]))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Not an array / missing id.
    let resp = server
        .put(&format!("/courses/{course}/assignments/bulk_update"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({ "id": a1 }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    let resp = server
        .put(&format!("/courses/{course}/assignments/bulk_update"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!([{ "all_dates": [] }]))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Unknown assignment id: Canvas raises RecordNotFound.
    let resp = server
        .put(&format!("/courses/{course}/assignments/bulk_update"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!([{ "id": 424242 }]))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unknown course.
    let resp = server
        .put("/courses/424242/assignments/bulk_update")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!([{ "id": 1 }]))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_update_fails_progress_on_foreign_override_id() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "BulkFail").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let a1 = seed_assignment(&pool, course, "A1").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    let foreign = seed_assignment(&pool, other_course, "Foreign").await;
    let foreign_override = seed_override(&pool, foreign, "Foreign Ov", None).await;

    let resp = server
        .put(&format!("/courses/{course}/assignments/bulk_update"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!([
            { "id": a1, "all_dates": [ { "base": true, "due_at": "2027-06-01T00:00:00Z" } ]},
            { "id": a1, "all_dates": [ { "id": foreign_override, "due_at": "2027-08-01T00:00:00Z" } ]},
        ]))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], json!("failed"), "body: {body}");
    assert_eq!(
        body["results"]["message"],
        json!(format!(
            "invalid assignment override id {foreign_override} for assignment {a1}"
        )),
        "body: {body}"
    );

    // Nothing was applied (Canvas fails the job before saving).
    let (due,): (Option<String>,) =
        sqlx::query_as("SELECT due_at::text FROM assignments WHERE id = $1")
            .bind(a1)
            .fetch_one(&pool)
            .await
            .expect("fetch a1");
    assert!(due.is_none(), "failed batch must not touch rows: {due:?}");
    let (foreign_due,): (Option<String>,) =
        sqlx::query_as("SELECT due_at::text FROM assignment_overrides WHERE id = $1")
            .bind(foreign_override)
            .fetch_one(&pool)
            .await
            .expect("fetch foreign override");
    assert!(
        foreign_due.is_none(),
        "foreign override untouched: {foreign_due:?}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// retry_alignment_clone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_alignment_clone_rejects_every_representable_state() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "RetryClone").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, "A1").await;

    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/retry_alignment_clone"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.json(), json!({ "error": "invalid status" }));

    // Unknown assignment / course.
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/424242/retry_alignment_clone"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .post(&format!(
            "/courses/424242/assignments/{assignment}/retry_alignment_clone"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// accessibility scans
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accessibility_scans_gate_on_rights_and_flags() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Dedicated account per run (feature flags must not leak into the root
    // account used by parallel tests).
    let account = seed_account(&pool, &prefix, "A11yAccount").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course_in_account(&pool, &prefix, "A11yCourse", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "A1").await;

    let scan_path = format!("/courses/{course}/assignments/{assignment}/accessibility/scan");
    let queue_path = format!("/courses/{course}/assignments/{assignment}/accessibility/queue_scan");

    // Flags off: teacher is forbidden.
    let resp = server
        .post(&scan_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Enable the GA1 flag on the dedicated account only.
    sqlx::query(
        r#"
        INSERT INTO feature_flags (context_type, context_id, feature, display_name, state)
        VALUES ('Account', $1, 'a11y_checker_ga1', 'Accessibility Checker', 'enabled')
        ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'enabled'
        "#,
    )
    .bind(account)
    .execute(&pool)
    .await
    .expect("enable a11y flag");

    // Students are still forbidden (manage-course-content rights).
    let resp = server
        .post(&scan_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Teacher with the flag: completed scan shape.
    for path in [&scan_path, &queue_path] {
        let resp = server
            .post(path)
            .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
            .await;
        assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
        let body: Value = resp.json();
        assert_eq!(body["resource_id"], json!(assignment), "body: {body}");
        assert_eq!(body["resource_type"], json!("Assignment"), "body: {body}");
        assert_eq!(body["resource_name"], json!("A1"), "body: {body}");
        assert_eq!(body["workflow_state"], json!("completed"), "body: {body}");
        assert_eq!(body["issue_count"], json!(0), "body: {body}");
        assert_eq!(body["issues"], json!([]), "body: {body}");
        assert_eq!(body["error_message"], json!(""), "body: {body}");
        assert_eq!(
            body["resource_url"],
            json!(format!("/courses/{course}/assignments/{assignment}")),
            "body: {body}"
        );
    }

    // Unknown assignment: 404 after the gates.
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/424242/accessibility/scan"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unknown course: 404.
    let resp = server
        .post("/courses/424242/assignments/1/accessibility/scan")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
