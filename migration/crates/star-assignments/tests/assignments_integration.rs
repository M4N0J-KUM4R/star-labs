//! Integration tests for star-assignments.
//!
//! Exercises the real assignments router (assignment CRUD, student
//! submissions, grading, and the course gradebook) through axum-test
//! against a real Postgres.
//!
//! Conventions (mirrors star-enrollments/tests/enrollments_integration.rs):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-assignments
//! - Uses a unique email/course prefix per run + full teardown so it is safe
//!   to run repeatedly against a shared/dev database.
//!
//! Grading semantics under test (as actually implemented in the crate):
//! - Points-based only: current_grade = sum(scores) / sum(points_possible of
//!   graded submissions) * 100. Ungraded and unsubmitted assignments are
//!   excluded from the denominator. There is no weighted assignment-group
//!   and no letter-grade computation here (star-assignment-groups and
//!   star-grading-standards own those concepts as separate crates).
//! - Resubmitting upserts the existing (assignment_id, user_id) row: same
//!   submission id, new body, and the previous score is retained.
//! - The handlers implement no late/missing logic: submissions after due_at
//!   are accepted with no flag or penalty, and unsubmitted work simply has
//!   no score.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "asg-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the assignments
/// router exactly the way star-http assembles it. Returns None when
/// DATABASE_URL is unset so callers can skip cleanly.
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

    let server = TestServer::new(assignments_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same merge pattern as star-http::build_api_router, restricted to the
/// assignments crate under test.
fn assignments_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_assignments::router())
        .with_state(state)
}

/// Create a user directly in the DB (bypassing the register endpoint) with a
/// deterministic password hash and the given role. Returns user id.
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
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Create a course directly in the DB. Returns course id.
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

/// Give a user an active enrollment in a course with the given role. The
/// assignments handlers authorize off enrollments rows, not users.role.
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

/// Mint a real bearer token the same way star-auth does at login time.
/// The AuthUser extractor re-reads the role from the DB, so the token role
/// claim is advisory only.
fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

/// Standard fixture: one teacher and one student enrolled in a course, plus
/// one 10-point assignment created through the API.
struct Fixture {
    teacher: i64,
    student: i64,
    course: i64,
    assignment: i64,
}

async fn seed_fixture(
    server: &TestServer,
    pool: &PgPool,
    prefix: &str,
    course_name: &str,
) -> Fixture {
    let teacher = seed_user(pool, prefix, "Teacher", "teacher").await;
    let student = seed_user(pool, prefix, "Student", "student").await;
    let course = seed_course(pool, prefix, course_name).await;
    seed_enrollment(pool, teacher, course, "teacher").await;
    seed_enrollment(pool, student, course, "student").await;
    let assignment = api_create_assignment(
        server,
        &auth(teacher, "teacher"),
        course,
        "Fixture Assignment",
        10.0,
    )
    .await;
    Fixture {
        teacher,
        student,
        course,
        assignment: assignment["id"].as_i64().expect("assignment id"),
    }
}

/// Delete every row created by this run. Submissions and assignments are
/// removed by prefix-marked user/course ids before their parents; users and
/// courses then go last. assignment_groups is cleared defensively although
/// this crate never writes it.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let user_ids = "SELECT id FROM users WHERE email LIKE $1";
    let course_ids = "SELECT id FROM courses WHERE course_code LIKE $1";
    let _ = sqlx::query(&format!(
        "DELETE FROM submissions WHERE user_id IN ({user_ids}) \
         OR assignment_id IN (SELECT id FROM assignments WHERE course_id IN ({course_ids}))",
    ))
    .bind(format!("{prefix}%@test.local"))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM assignments WHERE course_id IN ({course_ids})",
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM assignment_groups WHERE course_id IN ({course_ids})",
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM enrollments WHERE user_id IN ({user_ids})",
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
// Request / DB helpers
// ---------------------------------------------------------------------------

/// GET a path with a bearer token, asserting 200 and returning the JSON.
async fn get_ok(server: &TestServer, token: &str, path: &str) -> Value {
    let resp = server
        .get(path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}

/// Create an assignment through the API as the given token; asserts 201.
async fn api_create_assignment(
    server: &TestServer,
    token: &str,
    course_id: i64,
    title: &str,
    points_possible: f64,
) -> Value {
    let resp = server
        .post(&format!("/courses/{course_id}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "title": title, "points_possible": points_possible }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "create assignment: {}",
        resp.text()
    );
    resp.json()
}

/// Submit to an assignment through the API; asserts 201.
async fn api_submit(server: &TestServer, token: &str, assignment_id: i64, body: &str) -> Value {
    let resp = server
        .post(&format!("/assignments/{assignment_id}/submissions"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "body": body }))
        .await;
    assert_eq!(resp.status_code(), 201, "submit: {}", resp.text());
    resp.json()
}

/// Grade a submission through the API; asserts 200.
async fn api_grade(server: &TestServer, token: &str, submission_id: i64, score: f64) -> Value {
    let resp = server
        .put(&format!("/submissions/{submission_id}/grade"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "score": score }))
        .await;
    assert_eq!(resp.status_code(), 200, "grade: {}", resp.text());
    resp.json()
}

/// Current title of an assignment (None once deleted).
async fn db_assignment_title(pool: &PgPool, assignment_id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT title FROM assignments WHERE id = $1")
        .bind(assignment_id)
        .fetch_optional(pool)
        .await
        .expect("fetch assignment title")
}

/// Current points_possible of an assignment (None once deleted).
async fn db_assignment_points(pool: &PgPool, assignment_id: i64) -> Option<f64> {
    sqlx::query_scalar("SELECT points_possible::float8 FROM assignments WHERE id = $1")
        .bind(assignment_id)
        .fetch_optional(pool)
        .await
        .expect("fetch assignment points")
}

/// Current score of a submission (None when ungraded or deleted).
async fn db_submission_score(pool: &PgPool, submission_id: i64) -> Option<f64> {
    sqlx::query_scalar("SELECT score::float8 FROM submissions WHERE id = $1")
        .bind(submission_id)
        .fetch_one(pool)
        .await
        .expect("fetch submission score")
}

/// Number of assignments in a course.
async fn db_count_assignments_in_course(pool: &PgPool, course_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM assignments WHERE course_id = $1")
        .bind(course_id)
        .fetch_one(pool)
        .await
        .expect("count assignments")
}

/// Number of submission rows for one (assignment, user) pair — the UNIQUE
/// constraint should keep this at 0 or 1.
async fn db_count_submissions(pool: &PgPool, assignment_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM submissions WHERE assignment_id = $1 AND user_id = $2")
        .bind(assignment_id)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("count submissions")
}

// ---------------------------------------------------------------------------
// POST /courses/{courseId}/assignments
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_assignment_as_teacher_returns_201_and_persists() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "CreateOk").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({
            "title": "Essay One",
            "description": "Write 500 words",
            "points_possible": 25.5,
            "due_at": "2026-12-01T23:59:00Z"
        }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let assignment_id = body["id"].as_i64().expect("assignment id");
    assert_eq!(body["course_id"], course);
    assert_eq!(body["title"], "Essay One");
    assert_eq!(body["description"], "Write 500 words");
    assert_eq!(body["points_possible"].as_f64(), Some(25.5));

    // due_at round-trips as the same instant (RFC 3339).
    let echoed =
        chrono::DateTime::parse_from_rfc3339(body["due_at"].as_str().expect("due_at echoed"))
            .expect("parse due_at");
    let expected = chrono::DateTime::parse_from_rfc3339("2026-12-01T23:59:00Z").unwrap();
    assert_eq!(echoed.timestamp(), expected.timestamp());
    assert!(body["created_at"].is_string());
    assert!(body["updated_at"].is_string());

    // Row actually persisted with the NUMERIC points value.
    let (title, points): (String, f64) =
        sqlx::query_as("SELECT title, points_possible::float8 FROM assignments WHERE id = $1")
            .bind(assignment_id)
            .fetch_one(&pool)
            .await
            .expect("fetch persisted assignment");
    assert_eq!(title, "Essay One");
    assert!((points - 25.5).abs() < 1e-9);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_assignment_as_admin_without_enrollment_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "AdminCreate").await;

    // Admins bypass the enrollment check entirely.
    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .json(&json!({ "title": "Admin Made", "points_possible": 5.0 }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["course_id"], course);
    assert_eq!(body["title"], "Admin Made");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_assignment_as_enrolled_student_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "StudentCreate").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .json(&json!({ "title": "Sneaky", "points_possible": 1.0 }))
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_count_assignments_in_course(&pool, course).await,
        0,
        "nothing persisted"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_assignment_as_unenrolled_teacher_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "NoTeacherEnroll").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Teacher exists globally but has no enrollment in this course.
    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "title": "Orphan", "points_possible": 1.0 }))
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_count_assignments_in_course(&pool, course).await,
        0,
        "nothing persisted"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_assignment_blank_title_is_422() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "BlankTitle").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    for title in ["   ", ""] {
        let resp = server
            .post(&format!("/courses/{course}/assignments"))
            .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
            .json(&json!({ "title": title, "points_possible": 1.0 }))
            .await;
        assert_eq!(resp.status_code(), 422, "title {title:?}: {}", resp.text());
        assert!(resp.text().contains("title"), "body: {}", resp.text());
    }
    assert_eq!(db_count_assignments_in_course(&pool, course).await, 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_assignment_missing_title_field_is_422() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "NoTitleField").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // The Json extractor rejects the payload before the handler runs.
    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "points_possible": 10.0 }))
        .await;

    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert_eq!(db_count_assignments_in_course(&pool, course).await, 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_assignment_negative_points_is_422() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "NegPoints").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "title": "Negative", "points_possible": -0.5 }))
        .await;

    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert!(
        resp.text().contains("points_possible"),
        "body: {}",
        resp.text()
    );
    assert_eq!(db_count_assignments_in_course(&pool, course).await, 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_assignment_defaults_points_to_zero_and_omits_nulls() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Defaults").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "title": "Bare" }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let assignment_id = body["id"].as_i64().expect("assignment id");
    assert_eq!(body["points_possible"].as_f64(), Some(0.0));
    // Optional fields are skipped, not echoed as null.
    assert!(body.get("description").is_none());
    assert!(body.get("due_at").is_none());

    assert_eq!(
        db_assignment_points(&pool, assignment_id).await,
        Some(0.0),
        "COALESCE(NULL, 0) default persisted"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/assignments
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_assignments_scoped_to_course_in_creation_order() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let c1 = seed_course(&pool, &prefix, "ListOne").await;
    let c2 = seed_course(&pool, &prefix, "ListTwo").await;
    seed_enrollment(&pool, teacher, c1, "teacher").await;
    seed_enrollment(&pool, teacher, c2, "teacher").await;
    seed_enrollment(&pool, student, c1, "student").await;

    let a1 = api_create_assignment(&server, &auth(teacher, "teacher"), c1, "Alpha", 10.0).await;
    let a2 = api_create_assignment(&server, &auth(teacher, "teacher"), c1, "Beta", 20.0).await;
    let _other = api_create_assignment(
        &server,
        &auth(teacher, "teacher"),
        c2,
        "OtherCourseItem",
        5.0,
    )
    .await;

    // Enrolled student sees only this course's assignments, oldest first.
    let listed = get_ok(
        &server,
        &auth(student, "student"),
        &format!("/courses/{c1}/assignments"),
    )
    .await;
    let arr = listed.as_array().expect("array response");
    assert_eq!(arr.len(), 2, "only course 1 assignments: {arr:?}");
    assert_eq!(arr[0]["title"], "Alpha");
    assert_eq!(arr[0]["id"], a1["id"]);
    assert_eq!(arr[1]["title"], "Beta");
    assert_eq!(arr[1]["id"], a2["id"]);
    assert!(arr.iter().all(|a| a["course_id"] == c1));

    // Admin can list without being enrolled.
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let admin_listed = get_ok(
        &server,
        &auth(admin, "admin"),
        &format!("/courses/{c1}/assignments"),
    )
    .await;
    assert_eq!(admin_listed.as_array().unwrap().len(), 2);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_assignments_as_unenrolled_student_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "ListForbidden").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;

    let resp = server
        .get(&format!("/courses/{}/assignments", f.course))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(outsider, "student"),
        )
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_missing_course_is_403_for_student_and_empty_for_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    // No 404 for a missing course: non-enrolled users are simply Forbidden.
    let resp = server
        .get("/courses/999999999/assignments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Admins bypass the check and get an empty list.
    let resp = server
        .get("/courses/999999999/assignments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>().as_array().unwrap().len(), 0);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /assignments/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_assignment_returns_fields_for_enrolled_student() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "GetOk").await;

    let got = get_ok(
        &server,
        &auth(f.student, "student"),
        &format!("/assignments/{}", f.assignment),
    )
    .await;

    assert_eq!(got["id"], f.assignment);
    assert_eq!(got["course_id"], f.course);
    assert_eq!(got["title"], "Fixture Assignment");
    assert_eq!(got["points_possible"].as_f64(), Some(10.0));
    assert!(got["created_at"].is_string());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_assignment_missing_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .get("/assignments/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_assignment_as_unenrolled_student_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "GetForbidden").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;

    let resp = server
        .get(&format!("/assignments/{}", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(outsider, "student"),
        )
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /assignments/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_assignment_merges_partial_payload() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UpdateMerge").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let created: Value = server
        .post(&format!("/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({
            "title": "Original Title",
            "description": "Original desc",
            "points_possible": 25.0,
            "due_at": "2026-12-01T23:59:00Z"
        }))
        .await
        .json();
    let assignment_id = created["id"].as_i64().expect("assignment id");

    // Update only the title: everything else must be preserved.
    let resp = server
        .put(&format!("/assignments/{assignment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "title": "Renamed" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], "Renamed");
    assert_eq!(body["description"], "Original desc");
    assert_eq!(body["points_possible"].as_f64(), Some(25.0));
    assert!(body["due_at"].is_string(), "due_at preserved");

    // Update points and description: title and due_at must be preserved.
    let resp = server
        .put(&format!("/assignments/{assignment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "points_possible": 40.5, "description": "Updated desc" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], "Renamed");
    assert_eq!(body["description"], "Updated desc");
    assert_eq!(body["points_possible"].as_f64(), Some(40.5));
    assert!(body["due_at"].is_string(), "due_at preserved");

    // An explicit null is indistinguishable from an omitted field: the
    // description cannot be cleared through this endpoint.
    let resp = server
        .put(&format!("/assignments/{assignment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "description": null }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["description"], "Updated desc",
        "null cannot clear a field"
    );

    // Final state persisted.
    assert_eq!(
        db_assignment_title(&pool, assignment_id).await,
        Some("Renamed".to_string())
    );
    assert_eq!(db_assignment_points(&pool, assignment_id).await, Some(40.5));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_assignment_missing_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;

    let resp = server
        .put("/assignments/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "title": "Ghost" }))
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_assignment_as_student_is_403_and_unchanged() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "UpdateForbidden").await;

    let resp = server
        .put(&format!("/assignments/{}", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.student, "student"),
        )
        .json(&json!({ "title": "Hacked", "points_possible": 100.0 }))
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_assignment_title(&pool, f.assignment).await,
        Some("Fixture Assignment".to_string()),
        "title unchanged"
    );
    assert_eq!(
        db_assignment_points(&pool, f.assignment).await,
        Some(10.0),
        "points unchanged"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_assignment_as_other_course_teacher_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "UpdateOtherCourse").await;
    let other_teacher = seed_user(&pool, &prefix, "OtherTeacher", "teacher").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, other_teacher, other_course, "teacher").await;

    let resp = server
        .put(&format!("/assignments/{}", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(other_teacher, "teacher"),
        )
        .json(&json!({ "title": "Not Mine" }))
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_assignment_title(&pool, f.assignment).await,
        Some("Fixture Assignment".to_string()),
        "title unchanged"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /assignments/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_assignment_as_teacher_is_204_and_cascades_submissions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "DeleteOk").await;
    let sub = api_submit(
        &server,
        &auth(f.student, "student"),
        f.assignment,
        "my answer",
    )
    .await;
    let submission_id = sub["id"].as_i64().expect("submission id");

    let resp = server
        .delete(&format!("/assignments/{}", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // Hard delete: the assignment row is gone...
    assert!(db_assignment_title(&pool, f.assignment).await.is_none());
    // ...and its submissions went with it (ON DELETE CASCADE).
    let orphaned: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM submissions WHERE id = $1")
        .bind(submission_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(orphaned.0, 0, "submission cascaded");

    // Subsequent GET is 404.
    let resp = server
        .get(&format!("/assignments/{}", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_assignment_missing_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;

    let resp = server
        .delete("/assignments/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_assignment_as_student_is_403_and_row_remains() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "DeleteForbidden").await;

    let resp = server
        .delete(&format!("/assignments/{}", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.student, "student"),
        )
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_assignment_title(&pool, f.assignment).await,
        Some("Fixture Assignment".to_string()),
        "row still present"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /assignments/{id}/submissions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn student_submit_returns_201_and_persists() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "SubmitOk").await;

    let resp = server
        .post(&format!("/assignments/{}/submissions", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.student, "student"),
        )
        .json(&json!({ "body": "My answer" }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let submission_id = body["id"].as_i64().expect("submission id");
    assert_eq!(body["assignment_id"], f.assignment);
    assert_eq!(body["user_id"], f.student);
    assert_eq!(body["body"], "My answer");
    // Fresh submissions are ungraded: the score field is omitted entirely.
    assert!(body.get("score").is_none(), "score omitted: {body}");
    assert!(body["submitted_at"].is_string());

    // Row actually persisted.
    assert_eq!(
        db_count_submissions(&pool, f.assignment, f.student).await,
        1
    );
    assert_eq!(db_submission_score(&pool, submission_id).await, None);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn resubmit_upserts_same_row_and_retains_score() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "Resubmit").await;

    // Submit, get graded, then resubmit.
    let first = api_submit(
        &server,
        &auth(f.student, "student"),
        f.assignment,
        "first attempt",
    )
    .await;
    let submission_id = first["id"].as_i64().expect("submission id");
    api_grade(&server, &auth(f.teacher, "teacher"), submission_id, 6.0).await;

    let second = api_submit(
        &server,
        &auth(f.student, "student"),
        f.assignment,
        "second attempt",
    )
    .await;

    // The UNIQUE (assignment_id, user_id) constraint turns the second POST
    // into an upsert of the same row, not a new submission.
    assert_eq!(second["id"], submission_id, "same row id: {second}");
    assert_eq!(second["body"], "second attempt");
    // The upsert only touches body/submitted_at: the score survives.
    assert_eq!(
        second["score"].as_f64(),
        Some(6.0),
        "score retained: {second}"
    );

    // submitted_at did not move backwards.
    let t1 = chrono::DateTime::parse_from_rfc3339(
        first["submitted_at"].as_str().expect("first submitted_at"),
    )
    .expect("parse first submitted_at");
    let t2 = chrono::DateTime::parse_from_rfc3339(
        second["submitted_at"]
            .as_str()
            .expect("second submitted_at"),
    )
    .expect("parse second submitted_at");
    assert!(t2 >= t1, "submitted_at advanced: {t1} -> {t2}");

    // Still exactly one row, still graded 6.0.
    assert_eq!(
        db_count_submissions(&pool, f.assignment, f.student).await,
        1,
        "UNIQUE(assignment_id, user_id) holds"
    );
    assert_eq!(db_submission_score(&pool, submission_id).await, Some(6.0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn submit_to_missing_assignment_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;

    let resp = server
        .post("/assignments/999999999/submissions")
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .json(&json!({ "body": "answer" }))
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn submit_as_unenrolled_student_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "SubmitForbidden").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;

    let resp = server
        .post(&format!("/assignments/{}/submissions", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(outsider, "student"),
        )
        .json(&json!({ "body": "sneaky answer" }))
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_count_submissions(&pool, f.assignment, outsider).await,
        0,
        "nothing persisted"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn submit_as_enrolled_teacher_is_allowed() {
    // Documents actual behavior: the handler's role gate accepts the
    // student, teacher and admin roles, so an enrolled teacher may submit
    // (the "Only students can submit" comment in the source is not enforced).
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "TeacherSubmits").await;

    let resp = server
        .post(&format!("/assignments/{}/submissions", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.teacher, "teacher"),
        )
        .json(&json!({ "body": "teacher demo submission" }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["user_id"], f.teacher);
    assert_eq!(
        db_count_submissions(&pool, f.assignment, f.teacher).await,
        1
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn submit_after_due_date_is_accepted() {
    // Documents actual behavior: the handler has no late logic — no
    // rejection, no late flag, no penalty for submissions after due_at.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "LateSubmit").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({
            "title": "Past Due",
            "points_possible": 10.0,
            "due_at": "2020-01-01T00:00:00Z"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let assignment: Value = resp.json();
    let assignment_id = assignment["id"].as_i64().expect("assignment id");

    let resp = server
        .post(&format!("/assignments/{assignment_id}/submissions"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .json(&json!({ "body": "better late than never" }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /assignments/{id}/submissions/me
// ---------------------------------------------------------------------------

#[tokio::test]
async fn my_submission_returns_own_submission_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "MeOk").await;
    let second = seed_user(&pool, &prefix, "SecondStudent", "student").await;
    seed_enrollment(&pool, second, f.course, "student").await;

    let sub1 = api_submit(
        &server,
        &auth(f.student, "student"),
        f.assignment,
        "first student answer",
    )
    .await;
    let sub2 = api_submit(
        &server,
        &auth(second, "student"),
        f.assignment,
        "second student answer",
    )
    .await;

    let mine1 = get_ok(
        &server,
        &auth(f.student, "student"),
        &format!("/assignments/{}/submissions/me", f.assignment),
    )
    .await;
    assert_eq!(mine1["id"], sub1["id"]);
    assert_eq!(mine1["user_id"], f.student);
    assert_eq!(mine1["body"], "first student answer");

    // The other enrolled student sees their own submission, not the first.
    let mine2 = get_ok(
        &server,
        &auth(second, "student"),
        &format!("/assignments/{}/submissions/me", f.assignment),
    )
    .await;
    assert_eq!(mine2["id"], sub2["id"]);
    assert_eq!(mine2["user_id"], second);
    assert_eq!(mine2["body"], "second student answer");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn my_submission_without_submission_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "MeNone").await;

    let resp = server
        .get(&format!("/assignments/{}/submissions/me", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.student, "student"),
        )
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn my_submission_as_unenrolled_student_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "MeForbidden").await;
    api_submit(&server, &auth(f.student, "student"), f.assignment, "answer").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;

    let resp = server
        .get(&format!("/assignments/{}/submissions/me", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(outsider, "student"),
        )
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn my_submission_missing_assignment_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;

    let resp = server
        .get("/assignments/999999999/submissions/me")
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /assignments/{id}/submissions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_submissions_as_teacher_shows_user_info_newest_first() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "ListSubs").await;
    let second = seed_user(&pool, &prefix, "SecondStudent", "student").await;
    seed_enrollment(&pool, second, f.course, "student").await;

    api_submit(
        &server,
        &auth(f.student, "student"),
        f.assignment,
        "first student answer",
    )
    .await;
    api_submit(
        &server,
        &auth(second, "student"),
        f.assignment,
        "second student answer",
    )
    .await;

    let listed = get_ok(
        &server,
        &auth(f.teacher, "teacher"),
        &format!("/assignments/{}/submissions", f.assignment),
    )
    .await;

    let arr = listed.as_array().expect("array response");
    assert_eq!(arr.len(), 2, "both submissions: {arr:?}");
    // Ordered by submitted_at DESC: the later submission comes first.
    assert_eq!(arr[0]["user_id"], second);
    assert_eq!(arr[0]["user_name"], "SecondStudent");
    assert_eq!(
        arr[0]["user_email"],
        format!("{prefix}-SecondStudent@test.local")
    );
    assert_eq!(arr[1]["user_id"], f.student);
    assert_eq!(arr[1]["user_name"], "Student");
    assert_eq!(arr[1]["user_email"], format!("{prefix}-Student@test.local"));
    assert_eq!(arr[1]["body"], "first student answer");
    // Ungraded submissions carry no score.
    assert!(arr.iter().all(|s| s.get("score").is_none()));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_submissions_as_student_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "ListSubsForbidden").await;
    api_submit(&server, &auth(f.student, "student"), f.assignment, "answer").await;

    let resp = server
        .get(&format!("/assignments/{}/submissions", f.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.student, "student"),
        )
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_submissions_missing_assignment_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .get("/assignments/999999999/submissions")
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /submissions/{id}/grade
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grade_submission_persists_echoes_and_is_visible_to_student() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "GradeOk").await;
    let sub = api_submit(
        &server,
        &auth(f.student, "student"),
        f.assignment,
        "my answer",
    )
    .await;
    let submission_id = sub["id"].as_i64().expect("submission id");

    let graded = api_grade(&server, &auth(f.teacher, "teacher"), submission_id, 8.5).await;

    // The response echoes the graded submission.
    assert_eq!(graded["id"], submission_id);
    assert_eq!(graded["assignment_id"], f.assignment);
    assert_eq!(graded["user_id"], f.student);
    assert_eq!(graded["body"], "my answer");
    assert_eq!(graded["score"].as_f64(), Some(8.5));

    // The score is persisted.
    assert_eq!(db_submission_score(&pool, submission_id).await, Some(8.5));

    // The student sees the grade on their own submission.
    let mine = get_ok(
        &server,
        &auth(f.student, "student"),
        &format!("/assignments/{}/submissions/me", f.assignment),
    )
    .await;
    assert_eq!(mine["score"].as_f64(), Some(8.5));

    // The teacher's per-assignment listing shows it too.
    let listed = get_ok(
        &server,
        &auth(f.teacher, "teacher"),
        &format!("/assignments/{}/submissions", f.assignment),
    )
    .await;
    let arr = listed.as_array().expect("array response");
    let row = arr
        .iter()
        .find(|s| s["user_id"] == f.student)
        .expect("student submission listed");
    assert_eq!(row["score"].as_f64(), Some(8.5));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn regrade_overwrites_previous_score() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "Regrade").await;
    let sub = api_submit(&server, &auth(f.student, "student"), f.assignment, "answer").await;
    let submission_id = sub["id"].as_i64().expect("submission id");

    api_grade(&server, &auth(f.teacher, "teacher"), submission_id, 5.0).await;
    assert_eq!(db_submission_score(&pool, submission_id).await, Some(5.0));

    // Grading again overwrites; the NUMERIC(10,2) column rounds to 2
    // decimals, and the response echoes the stored value.
    let regraded = api_grade(&server, &auth(f.teacher, "teacher"), submission_id, 9.123).await;
    assert_eq!(regraded["score"].as_f64(), Some(9.12));
    assert_eq!(db_submission_score(&pool, submission_id).await, Some(9.12));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grade_as_student_is_403_and_score_unchanged() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "GradeForbidden").await;
    let sub = api_submit(&server, &auth(f.student, "student"), f.assignment, "answer").await;
    let submission_id = sub["id"].as_i64().expect("submission id");

    // Even the submission's owner cannot grade it.
    let resp = server
        .put(&format!("/submissions/{submission_id}/grade"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.student, "student"),
        )
        .json(&json!({ "score": 100.0 }))
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_submission_score(&pool, submission_id).await,
        None,
        "score still ungraded"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grade_as_other_course_teacher_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "GradeOtherCourse").await;
    let sub = api_submit(&server, &auth(f.student, "student"), f.assignment, "answer").await;
    let submission_id = sub["id"].as_i64().expect("submission id");

    let other_teacher = seed_user(&pool, &prefix, "OtherTeacher", "teacher").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, other_teacher, other_course, "teacher").await;

    let resp = server
        .put(&format!("/submissions/{submission_id}/grade"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(other_teacher, "teacher"),
        )
        .json(&json!({ "score": 100.0 }))
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_submission_score(&pool, submission_id).await,
        None,
        "score still ungraded"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grade_as_admin_without_enrollment_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "GradeAdmin").await;
    let sub = api_submit(&server, &auth(f.student, "student"), f.assignment, "answer").await;
    let submission_id = sub["id"].as_i64().expect("submission id");

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let graded = api_grade(&server, &auth(admin, "admin"), submission_id, 7.0).await;

    assert_eq!(graded["score"].as_f64(), Some(7.0));
    assert_eq!(db_submission_score(&pool, submission_id).await, Some(7.0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grade_missing_submission_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;

    let resp = server
        .put("/submissions/999999999/grade")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "score": 1.0 }))
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/gradebook
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gradebook_points_based_current_grade() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let carol = seed_user(&pool, &prefix, "Carol", "student").await;
    let outsider = seed_user(&pool, &prefix, "Dana", "student").await;
    let course = seed_course(&pool, &prefix, "Gradebook").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    for s in [alice, bob, carol] {
        seed_enrollment(&pool, s, course, "student").await;
    }
    seed_enrollment(&pool, outsider, other_course, "student").await;

    let a1 =
        api_create_assignment(&server, &auth(teacher, "teacher"), course, "Quiz One", 10.0).await;
    let a2 = api_create_assignment(
        &server,
        &auth(teacher, "teacher"),
        course,
        "Essay One",
        40.0,
    )
    .await;
    let a1_id = a1["id"].as_i64().expect("a1 id");
    let a2_id = a2["id"].as_i64().expect("a2 id");

    // Alice submits to both and is graded on both.
    let alice_a1 = api_submit(&server, &auth(alice, "student"), a1_id, "alice quiz").await;
    let alice_a2 = api_submit(&server, &auth(alice, "student"), a2_id, "alice essay").await;
    // Bob submits to both but is only graded on the quiz.
    let bob_a1 = api_submit(&server, &auth(bob, "student"), a1_id, "bob quiz").await;
    api_submit(&server, &auth(bob, "student"), a2_id, "bob essay").await;
    // Carol never submits.
    // The teacher also submits (the role gate allows it) but must not get a
    // gradebook row.
    api_submit(&server, &auth(teacher, "teacher"), a1_id, "teacher demo").await;

    api_grade(
        &server,
        &auth(teacher, "teacher"),
        alice_a1["id"].as_i64().unwrap(),
        8.5,
    )
    .await;
    api_grade(
        &server,
        &auth(teacher, "teacher"),
        alice_a2["id"].as_i64().unwrap(),
        30.0,
    )
    .await;
    api_grade(
        &server,
        &auth(teacher, "teacher"),
        bob_a1["id"].as_i64().unwrap(),
        9.0,
    )
    .await;

    let book = get_ok(
        &server,
        &auth(teacher, "teacher"),
        &format!("/courses/{course}/gradebook"),
    )
    .await;

    // Both assignments, in creation order.
    let ids: Vec<i64> = book["assignment_ids"]
        .as_array()
        .expect("assignment_ids array")
        .iter()
        .map(|v| v.as_i64().expect("assignment id"))
        .collect();
    assert_eq!(ids, vec![a1_id, a2_id], "creation order");

    // One row per enrolled student, ordered by name; the teacher and the
    // other-course student are excluded.
    let rows = book["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3, "students only: {rows:?}");
    let names: Vec<&str> = rows
        .iter()
        .map(|r| r["user"]["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["Alice", "Bob", "Carol"]);
    assert!(rows.iter().all(|r| r["user"]["id"] != teacher));
    assert!(rows.iter().all(|r| r["user"]["id"] != outsider));

    // Alice: graded on both -> (8.5 + 30) / (10 + 40) * 100.
    let alice_row = &rows[0];
    assert_eq!(
        alice_row["user"]["email"],
        format!("{prefix}-Alice@test.local")
    );
    let alice_subs = alice_row["submissions"]
        .as_array()
        .expect("alice submissions");
    assert_eq!(alice_subs.len(), 2);
    assert_eq!(alice_subs[0]["assignment_id"], a1_id);
    assert_eq!(alice_subs[0]["score"].as_f64(), Some(8.5));
    assert_eq!(alice_subs[0]["points_possible"].as_f64(), Some(10.0));
    assert_eq!(alice_subs[1]["assignment_id"], a2_id);
    assert_eq!(alice_subs[1]["score"].as_f64(), Some(30.0));
    assert_eq!(alice_subs[1]["points_possible"].as_f64(), Some(40.0));
    let expected_alice = (8.5 + 30.0) / (10.0 + 40.0) * 100.0;
    let actual_alice = alice_row["current_grade"].as_f64().expect("alice grade");
    assert!(
        (actual_alice - expected_alice).abs() < 0.01,
        "alice: {actual_alice} vs {expected_alice}"
    );

    // Bob: graded only on the quiz; the ungraded essay is excluded from the
    // denominator -> 9 / 10 * 100.
    let bob_row = &rows[1];
    let bob_subs = bob_row["submissions"].as_array().expect("bob submissions");
    assert_eq!(bob_subs[0]["score"].as_f64(), Some(9.0));
    assert!(bob_subs[1].get("score").is_none(), "ungraded: no score key");
    assert_eq!(bob_subs[1]["points_possible"].as_f64(), Some(40.0));
    let expected_bob = 9.0 / 10.0 * 100.0;
    let actual_bob = bob_row["current_grade"].as_f64().expect("bob grade");
    assert!(
        (actual_bob - expected_bob).abs() < 0.01,
        "bob: {actual_bob} vs {expected_bob}"
    );

    // Carol: no submissions at all -> null current_grade, one slot per
    // assignment with the points but no score.
    let carol_row = &rows[2];
    assert!(carol_row["current_grade"].is_null());
    let carol_subs = carol_row["submissions"]
        .as_array()
        .expect("carol submissions");
    assert_eq!(carol_subs.len(), 2);
    assert!(carol_subs.iter().all(|s| s.get("score").is_none()));
    assert_eq!(carol_subs[0]["points_possible"].as_f64(), Some(10.0));
    assert_eq!(carol_subs[1]["points_possible"].as_f64(), Some(40.0));

    // Admins can read the gradebook without an enrollment.
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let admin_view = server
        .get(&format!("/courses/{course}/gradebook"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(admin_view.status_code(), 200, "body: {}", admin_view.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn gradebook_requires_teacher_or_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "GradebookForbidden").await;

    // Enrolled students cannot see the gradebook.
    let resp = server
        .get(&format!("/courses/{}/gradebook", f.course))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(f.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Teachers without an enrollment in the course cannot either.
    let other_teacher = seed_user(&pool, &prefix, "OtherTeacher", "teacher").await;
    let resp = server
        .get(&format!("/courses/{}/gradebook", f.course))
        .add_header(
            axum::http::header::AUTHORIZATION,
            &auth(other_teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn gradebook_empty_course_returns_empty_structure() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "EmptyGradebook").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // No assignments: the handler short-circuits, so even enrolled students
    // produce no rows.
    let book = get_ok(
        &server,
        &auth(teacher, "teacher"),
        &format!("/courses/{course}/gradebook"),
    )
    .await;

    assert_eq!(book["assignment_ids"].as_array().unwrap().len(), 0);
    assert_eq!(book["rows"].as_array().unwrap().len(), 0);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unauthenticated_requests_are_401() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Representative endpoints, no token at all.
    let resp = server.get("/courses/1/assignments").await;
    assert_eq!(resp.status_code(), 401, "list: {}", resp.text());

    let resp = server
        .post("/courses/1/assignments")
        .json(&json!({ "title": "X" }))
        .await;
    assert_eq!(resp.status_code(), 401, "create: {}", resp.text());

    let resp = server.get("/assignments/1").await;
    assert_eq!(resp.status_code(), 401, "get: {}", resp.text());

    let resp = server
        .post("/assignments/1/submissions")
        .json(&json!({ "body": "x" }))
        .await;
    assert_eq!(resp.status_code(), 401, "submit: {}", resp.text());

    let resp = server
        .put("/submissions/1/grade")
        .json(&json!({ "score": 1.0 }))
        .await;
    assert_eq!(resp.status_code(), 401, "grade: {}", resp.text());

    let resp = server.get("/courses/1/gradebook").await;
    assert_eq!(resp.status_code(), 401, "gradebook: {}", resp.text());

    // A malformed bearer token is rejected the same way.
    let resp = server
        .get("/courses/1/assignments")
        .add_header(axum::http::header::AUTHORIZATION, "Bearer not-a-token")
        .await;
    assert_eq!(resp.status_code(), 401, "garbage token: {}", resp.text());
}
