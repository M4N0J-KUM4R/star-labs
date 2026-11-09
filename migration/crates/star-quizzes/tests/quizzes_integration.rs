//! Integration tests for star-quizzes.
//!
//! Exercises the real quizzes router (quiz CRUD, the POST /quizzes/{id}/start
//! submission upsert, per-user "my submission", and the course-level
//! submissions listing) through axum-test against a real Postgres.
//!
//! Conventions (mirrors star-assignments/tests/assignments_integration.rs):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-quizzes
//! - Uses a unique email/course prefix per run + full teardown so it is safe
//!   to run repeatedly against a shared/dev database.
//!
//! Behavior pinned as actually implemented:
//! - Canvas parity quirks: quiz_type is restricted to Canvas' four values
//!   (assignment | practice | survey | graded_survey); `published` is the
//!   boolean workflow equivalent; DELETE is a HARD delete (Canvas soft-deletes
//!   via workflow_state='deleted') so quiz_submissions cascade away.
//! - POST /quizzes/{id}/start is students-only: the caller needs an enrollment
//!   row with role='student' in the quiz's course (teachers and admins without
//!   a student enrollment get 403). It returns 409 Conflict when the user
//!   already has a (quiz_id, user_id) submission row (UNIQUE constraint backs
//!   this up).
//! - update (PUT) validates quiz_type and requires a non-blank title when one
//!   is supplied, matching create (the title is trimmed). An explicit JSON
//!   null cannot clear an optional field (omitted and null are
//!   indistinguishable).
//! - GET /courses/{courseId}/quizzes/{id}/submissions loads the quiz's own
//!   course_id first: a missing quiz, or a quiz that is not in the path
//!   course, is 404; authorization is then checked on the quiz's course.

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
        "quiz-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the quizzes router
/// exactly the way star-http assembles it. Returns None when DATABASE_URL is
/// unset so callers can skip cleanly.
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

    let server = TestServer::new(quizzes_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same merge pattern as star-http::build_api_router, restricted to the
/// quizzes crate under test.
fn quizzes_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_quizzes::router())
        .with_state(state)
}

/// Create a user directly in the DB (bypassing the register endpoint) with a
/// real password hash and the given role. Returns user id.
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

/// Give a user an enrollment in a course with the given role. The quizzes
/// handlers authorize off enrollments rows, not users.role.
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
/// one 10-point unpublished quiz created through the API.
struct Fixture {
    teacher: i64,
    student: i64,
    course: i64,
    quiz: i64,
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
    let quiz = api_create_quiz(server, &auth(teacher, "teacher"), course, "Fixture Quiz").await;
    Fixture {
        teacher,
        student,
        course,
        quiz: quiz["id"].as_i64().expect("quiz id"),
    }
}

/// Delete every row created by this run. quiz_submissions cascade from their
/// quiz, quizzes cascade from their course; the explicit deletes mirror the
/// other suites and keep the teardown order obvious.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let user_ids = "SELECT id FROM users WHERE email LIKE $1";
    let course_ids = "SELECT id FROM courses WHERE course_code LIKE $1";
    let _ = sqlx::query(&format!(
        "DELETE FROM quiz_submissions WHERE quiz_id IN (SELECT id FROM quizzes \
         WHERE course_id IN ({course_ids})) OR user_id IN ({user_ids})"
    ))
    .bind(format!("{prefix}%@test.local"))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM quizzes WHERE course_id IN ({course_ids})"
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

/// Create a quiz through the API as the given token; asserts 201.
async fn api_create_quiz(server: &TestServer, token: &str, course_id: i64, title: &str) -> Value {
    let resp = server
        .post(&format!("/courses/{course_id}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "title": title, "points_possible": 10.0 }))
        .await;
    assert_eq!(resp.status_code(), 201, "create quiz: {}", resp.text());
    resp.json()
}

/// Start a quiz through the API as the given token; asserts 201.
async fn api_start_quiz(server: &TestServer, token: &str, quiz_id: i64) -> Value {
    let resp = server
        .post(&format!("/quizzes/{quiz_id}/start"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 201, "start quiz: {}", resp.text());
    resp.json()
}

/// Current title of a quiz (None once deleted).
async fn db_quiz_title(pool: &PgPool, quiz_id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT title FROM quizzes WHERE id = $1")
        .bind(quiz_id)
        .fetch_optional(pool)
        .await
        .expect("fetch quiz title")
}

/// Current published flag of a quiz (None once deleted).
async fn db_quiz_published(pool: &PgPool, quiz_id: i64) -> Option<bool> {
    sqlx::query_scalar("SELECT published FROM quizzes WHERE id = $1")
        .bind(quiz_id)
        .fetch_optional(pool)
        .await
        .expect("fetch quiz published")
}

/// Number of quizzes in a course.
async fn db_count_quizzes_in_course(pool: &PgPool, course_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM quizzes WHERE course_id = $1")
        .bind(course_id)
        .fetch_one(pool)
        .await
        .expect("count quizzes")
}

/// Number of quiz_submission rows for one (quiz, user) pair — the UNIQUE
/// constraint should keep this at 0 or 1.
async fn db_count_quiz_submissions(pool: &PgPool, quiz_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM quiz_submissions WHERE quiz_id = $1 AND user_id = $2")
        .bind(quiz_id)
        .bind(user_id)
        .fetch_one(pool)
        .await
        .expect("count quiz submissions")
}

// ---------------------------------------------------------------------------
// POST /courses/{courseId}/quizzes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_quiz_as_teacher_returns_201_and_persists() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "CreateOk").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "title": "Midterm Review",
            "description": "<p>good luck</p>",
            "quiz_type": "practice",
            "time_limit": 30,
            "shuffle_answers": true,
            "allowed_attempts": 2,
            "published": false,
            "available_from": "2026-01-01T00:00:00Z",
            "available_until": "2026-12-31T23:59:59Z",
            "points_possible": 12.5
        }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let quiz_id = body["id"].as_i64().expect("quiz id");
    assert_eq!(body["course_id"], course);
    assert_eq!(body["title"], "Midterm Review");
    assert_eq!(body["description"], "<p>good luck</p>");
    assert_eq!(body["quiz_type"], "practice");
    assert_eq!(body["time_limit"].as_i64(), Some(30));
    assert_eq!(body["shuffle_answers"].as_bool(), Some(true));
    assert_eq!(body["allowed_attempts"].as_i64(), Some(2));
    assert_eq!(body["published"].as_bool(), Some(false));
    assert_eq!(body["points_possible"].as_f64(), Some(12.5));

    // available_from/until round-trip as the same instant (RFC 3339).
    for (field, expected) in [
        ("available_from", "2026-01-01T00:00:00Z"),
        ("available_until", "2026-12-31T23:59:59Z"),
    ] {
        let echoed = chrono::DateTime::parse_from_rfc3339(
            body[field]
                .as_str()
                .unwrap_or_else(|| panic!("{field} echoed, body: {body}")),
        )
        .unwrap_or_else(|e| panic!("parse {field}: {e}"));
        let want = chrono::DateTime::parse_from_rfc3339(expected).unwrap();
        assert_eq!(echoed.timestamp(), want.timestamp(), "{field} instant");
    }
    assert!(body["created_at"].is_string());
    assert!(body["updated_at"].is_string());

    // Row actually persisted with the NUMERIC points value.
    let (title, points): (String, f64) =
        sqlx::query_as("SELECT title, points_possible::float8 FROM quizzes WHERE id = $1")
            .bind(quiz_id)
            .fetch_one(&pool)
            .await
            .expect("fetch persisted quiz");
    assert_eq!(title, "Midterm Review");
    assert!((points - 12.5).abs() < 1e-9);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_quiz_applies_canvas_defaults_and_omits_null_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Defaults").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "title": "Bare" }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let quiz_id = body["id"].as_i64().expect("quiz id");
    // Canvas-parity defaults: quiz_type assignment, shuffle_answers false,
    // allowed_attempts 1, published false, points_possible 0.
    assert_eq!(body["quiz_type"], "assignment");
    assert_eq!(body["shuffle_answers"].as_bool(), Some(false));
    assert_eq!(body["allowed_attempts"].as_i64(), Some(1));
    assert_eq!(body["published"].as_bool(), Some(false));
    assert_eq!(body["points_possible"].as_f64(), Some(0.0));
    // Optional fields are skipped, not echoed as null.
    assert!(body.get("description").is_none());
    assert!(body.get("time_limit").is_none());
    assert!(body.get("available_from").is_none());
    assert!(body.get("available_until").is_none());

    // Defaults persisted.
    let (quiz_type, attempts, published): (String, i32, bool) =
        sqlx::query_as("SELECT quiz_type, allowed_attempts, published FROM quizzes WHERE id = $1")
            .bind(quiz_id)
            .fetch_one(&pool)
            .await
            .expect("fetch persisted defaults");
    assert_eq!(quiz_type, "assignment");
    assert_eq!(attempts, 1);
    assert!(!published);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_quiz_validates_title_and_quiz_type() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Validation").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let token = &auth(teacher, "teacher");

    // Blank titles (the handler trims) are 422 with a title error.
    for title in ["", "   "] {
        let resp = server
            .post(&format!("/courses/{course}/quizzes"))
            .add_header(axum::http::header::AUTHORIZATION, token)
            .json(&json!({ "title": title }))
            .await;
        assert_eq!(resp.status_code(), 422, "title {title:?}: {}", resp.text());
        assert!(resp.text().contains("title"), "body mentions title");
    }

    // A missing title field is rejected by the Json extractor, also 422.
    let resp = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "points_possible": 5.0 }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    // quiz_type is restricted to Canvas' four values.
    let resp = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "title": "Bogus Type", "quiz_type": "bogus" }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert!(resp.text().contains("quiz_type"), "body mentions quiz_type");

    // All four Canvas types are accepted.
    for quiz_type in ["assignment", "practice", "survey", "graded_survey"] {
        let resp = server
            .post(&format!("/courses/{course}/quizzes"))
            .add_header(axum::http::header::AUTHORIZATION, token)
            .json(&json!({ "title": format!("Type {quiz_type}"), "quiz_type": quiz_type }))
            .await;
        assert_eq!(
            resp.status_code(),
            201,
            "quiz_type {quiz_type}: {}",
            resp.text()
        );
        let body: Value = resp.json();
        assert_eq!(body["quiz_type"], quiz_type);
    }

    // Only the four valid creations persisted.
    assert_eq!(db_count_quizzes_in_course(&pool, course).await, 4);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_quiz_authz_student_403_unenrolled_teacher_403_admin_ok() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let unaffiliated = seed_user(&pool, &prefix, "Unaffiliated", "teacher").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "CreateAuthz").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Enrolled students cannot create quizzes.
    let resp = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "title": "Sneaky" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A teacher with no enrollment in this course cannot either.
    let resp = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(unaffiliated, "teacher"),
        )
        .json(&json!({ "title": "Orphan" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Admins bypass the enrollment check entirely.
    let resp = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "title": "Admin Made" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["course_id"], course);
    assert_eq!(body["title"], "Admin Made");

    // Exactly the admin's quiz persisted.
    assert_eq!(db_count_quizzes_in_course(&pool, course).await, 1);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/quizzes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_quizzes_scoped_to_course_in_creation_order() {
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

    let q1 = api_create_quiz(&server, &auth(teacher, "teacher"), c1, "Alpha").await;
    let q2 = api_create_quiz(&server, &auth(teacher, "teacher"), c1, "Beta").await;
    let _other = api_create_quiz(&server, &auth(teacher, "teacher"), c2, "OtherCourseQuiz").await;

    // Enrolled student sees only this course's quizzes, oldest first.
    let listed = get_ok(
        &server,
        &auth(student, "student"),
        &format!("/courses/{c1}/quizzes"),
    )
    .await;
    let arr = listed.as_array().expect("array response");
    assert_eq!(arr.len(), 2, "only course 1 quizzes: {arr:?}");
    assert_eq!(arr[0]["title"], "Alpha");
    assert_eq!(arr[0]["id"], q1["id"]);
    assert_eq!(arr[1]["title"], "Beta");
    assert_eq!(arr[1]["id"], q2["id"]);
    assert!(arr.iter().all(|q| q["course_id"] == c1));

    // Admin can list without being enrolled.
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let admin_listed = get_ok(
        &server,
        &auth(admin, "admin"),
        &format!("/courses/{c1}/quizzes"),
    )
    .await;
    assert_eq!(admin_listed.as_array().unwrap().len(), 2);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_quizzes_missing_course_403_for_student_and_empty_for_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "ListAuthz").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    seed_enrollment(&pool, student, course, "student").await;

    // No 404 for a missing course: non-enrolled users are simply Forbidden.
    let resp = server
        .get("/courses/999999999/quizzes")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Admins bypass the check and get an empty list.
    let resp = server
        .get("/courses/999999999/quizzes")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>().as_array().unwrap().len(), 0);

    // A student enrolled elsewhere is Forbidden on this course too.
    let resp = server
        .get(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /quizzes/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_quiz_fields_for_enrolled_student_missing_404_unenrolled_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "GetOk").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;

    // Enrolled student sees the fields.
    let got = get_ok(
        &server,
        &auth(f.student, "student"),
        &format!("/quizzes/{}", f.quiz),
    )
    .await;
    assert_eq!(got["id"], f.quiz);
    assert_eq!(got["course_id"], f.course);
    assert_eq!(got["title"], "Fixture Quiz");
    assert_eq!(got["points_possible"].as_f64(), Some(10.0));
    assert!(got["created_at"].is_string());

    // Missing quiz is 404 (checked before any authorization).
    let resp = server
        .get("/quizzes/999999999")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // An existing quiz is 403 for a user not enrolled in its course.
    let resp = server
        .get(&format!("/quizzes/{}", f.quiz))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /quizzes/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_quiz_merges_partial_payload_and_publishes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UpdateMerge").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let token = &auth(teacher, "teacher");

    let created: Value = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({
            "title": "Original Title",
            "description": "Original desc",
            "time_limit": 45,
            "points_possible": 10.0
        }))
        .await
        .json();
    let quiz_id = created["id"].as_i64().expect("quiz id");

    // Update only the title: everything else must be preserved.
    let resp = server
        .put(&format!("/quizzes/{quiz_id}"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "title": "Renamed" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], "Renamed");
    assert_eq!(body["description"], "Original desc");
    assert_eq!(body["time_limit"].as_i64(), Some(45));
    assert_eq!(body["published"].as_bool(), Some(false));
    assert_eq!(body["points_possible"].as_f64(), Some(10.0));

    // Publish: only the published flag changes.
    let resp = server
        .put(&format!("/quizzes/{quiz_id}"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "published": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["published"].as_bool(), Some(true));
    assert_eq!(body["title"], "Renamed", "title preserved");

    // An explicit null is indistinguishable from an omitted field: the
    // description cannot be cleared through this endpoint.
    let resp = server
        .put(&format!("/quizzes/{quiz_id}"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "description": null }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["description"], "Original desc",
        "null cannot clear a field"
    );

    // Points update persisted.
    let resp = server
        .put(&format!("/quizzes/{quiz_id}"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "points_possible": 40.5 }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    assert_eq!(
        db_quiz_title(&pool, quiz_id).await,
        Some("Renamed".to_string())
    );
    assert_eq!(db_quiz_published(&pool, quiz_id).await, Some(true));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_quiz_validates_quiz_type_and_rejects_blank_title() {
    // PUT validates quiz_type against Canvas' four values and, like POST,
    // requires a non-blank title when a title is supplied.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "UpdateValidation").await;
    let token = &auth(f.teacher, "teacher");

    // Bogus quiz_type is rejected with 422.
    let resp = server
        .put(&format!("/quizzes/{}", f.quiz))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "quiz_type": "bogus" }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert!(resp.text().contains("quiz_type"), "body mentions quiz_type");

    // A valid Canvas type change is accepted.
    let resp = server
        .put(&format!("/quizzes/{}", f.quiz))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "quiz_type": "graded_survey" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["quiz_type"], "graded_survey");

    // Blank or whitespace-only titles are rejected on update.
    for title in ["", "   "] {
        let resp = server
            .put(&format!("/quizzes/{}", f.quiz))
            .add_header(axum::http::header::AUTHORIZATION, token)
            .json(&json!({ "title": title }))
            .await;
        assert_eq!(resp.status_code(), 422, "title {title:?}: {}", resp.text());
        assert!(resp.text().contains("title"), "body mentions title");
    }
    assert_eq!(
        db_quiz_title(&pool, f.quiz).await,
        Some("Fixture Quiz".to_string()),
        "title unchanged"
    );

    // A valid title update is trimmed and accepted.
    let resp = server
        .put(&format!("/quizzes/{}", f.quiz))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "title": "  Renamed Quiz  " }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], "Renamed Quiz");
    assert_eq!(
        db_quiz_title(&pool, f.quiz).await,
        Some("Renamed Quiz".to_string())
    );

    // Other partial updates still succeed.
    let resp = server
        .put(&format!("/quizzes/{}", f.quiz))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "points_possible": 25.0 }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["points_possible"].as_f64(), Some(25.0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_quiz_missing_404_and_authz_403_unchanged() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "UpdateAuthz").await;
    let other_teacher = seed_user(&pool, &prefix, "OtherTeacher", "teacher").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, other_teacher, other_course, "teacher").await;

    // Missing quiz is 404.
    let resp = server
        .put("/quizzes/999999999")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.teacher, "teacher"),
        )
        .json(&json!({ "title": "Ghost" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The enrolled student cannot update, and nothing changed.
    let resp = server
        .put(&format!("/quizzes/{}", f.quiz))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.student, "student"),
        )
        .json(&json!({ "title": "Hacked", "points_possible": 100.0 }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_quiz_title(&pool, f.quiz).await,
        Some("Fixture Quiz".to_string()),
        "title unchanged"
    );

    // A teacher enrolled in a DIFFERENT course cannot update either.
    let resp = server
        .put(&format!("/quizzes/{}", f.quiz))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(other_teacher, "teacher"),
        )
        .json(&json!({ "title": "Not Mine" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_quiz_title(&pool, f.quiz).await,
        Some("Fixture Quiz".to_string()),
        "title still unchanged"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /quizzes/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_quiz_as_teacher_is_204_and_cascades_submissions() {
    // Canvas parity note: Canvas soft-deletes quizzes (workflow_state=
    // 'deleted') and keeps submissions; this crate HARD deletes and the
    // quiz_submissions cascade away. Intentional documented difference.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "DeleteOk").await;
    api_start_quiz(&server, &auth(f.student, "student"), f.quiz).await;
    assert_eq!(db_count_quiz_submissions(&pool, f.quiz, f.student).await, 1);

    let resp = server
        .delete(&format!("/quizzes/{}", f.quiz))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // Hard delete: the quiz row is gone...
    assert!(db_quiz_title(&pool, f.quiz).await.is_none());
    // ...and its submissions went with it (ON DELETE CASCADE).
    let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM quiz_submissions WHERE quiz_id = $1")
        .bind(f.quiz)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows.0, 0, "quiz_submissions cascaded");

    // Subsequent GET is 404 for the owner too.
    let resp = server
        .get(&format!("/quizzes/{}", f.quiz))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_quiz_missing_404_and_student_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "DeleteAuthz").await;

    // Missing quiz is 404.
    let resp = server
        .delete("/quizzes/999999999")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The enrolled student cannot delete; the row remains.
    let resp = server
        .delete(&format!("/quizzes/{}", f.quiz))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        db_quiz_title(&pool, f.quiz).await,
        Some("Fixture Quiz".to_string()),
        "row still present"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /quizzes/{id}/start  (quiz_submission upsert)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn start_quiz_returns_201_and_persists() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "StartOk").await;

    let resp = server
        .post(&format!("/quizzes/{}/start", f.quiz))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.student, "student"),
        )
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let submission_id = body["id"].as_i64().expect("submission id");
    assert_eq!(body["quiz_id"], f.quiz);
    assert_eq!(body["user_id"], f.student);
    assert!(body["started_at"].is_string());
    // Fresh submissions are ungraded and incomplete: both optional fields
    // are omitted entirely, not echoed as null.
    assert!(body.get("score").is_none(), "score omitted: {body}");
    assert!(body.get("completed_at").is_none(), "completed_at omitted");

    // Row actually persisted.
    assert_eq!(db_count_quiz_submissions(&pool, f.quiz, f.student).await, 1);
    let (score,): (Option<f64>,) =
        sqlx::query_as("SELECT score::float8 FROM quiz_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("fetch submission score");
    assert_eq!(score, None, "fresh submission is ungraded");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn start_quiz_twice_is_409_conflict() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "StartTwice").await;

    api_start_quiz(&server, &auth(f.student, "student"), f.quiz).await;

    // Starting again is a 409 Conflict, not an upsert.
    let resp = server
        .post(&format!("/quizzes/{}/start", f.quiz))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());
    assert!(
        resp.text().contains("quiz already started"),
        "conflict body names the reason"
    );

    // Still exactly one row for that (quiz, user).
    assert_eq!(db_count_quiz_submissions(&pool, f.quiz, f.student).await, 1);

    // A different enrolled student gets their own submission.
    let second = seed_user(&pool, &prefix, "SecondStudent", "student").await;
    seed_enrollment(&pool, second, f.course, "student").await;
    let sub = api_start_quiz(&server, &auth(second, "student"), f.quiz).await;
    assert_eq!(sub["user_id"], second);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn start_quiz_missing_404_teacher_and_admin_403_student_201() {
    // Start is students-only: the caller needs an enrollment row with
    // role='student' in the quiz's course. Teachers and admins without a
    // student enrollment are forbidden.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "StartAuthz").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    // Missing quiz is 404 (checked before authorization).
    let resp = server
        .post("/quizzes/999999999/start")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // An unenrolled student is 403 and nothing is persisted.
    let resp = server
        .post(&format!("/quizzes/{}/start", f.quiz))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(db_count_quiz_submissions(&pool, f.quiz, outsider).await, 0);

    // An enrolled teacher is forbidden: only students take quizzes.
    let resp = server
        .post(&format!("/quizzes/{}/start", f.quiz))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(db_count_quiz_submissions(&pool, f.quiz, f.teacher).await, 0);

    // Admins get no bypass: without a student enrollment they are 403 too.
    let resp = server
        .post(&format!("/quizzes/{}/start", f.quiz))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(db_count_quiz_submissions(&pool, f.quiz, admin).await, 0);

    // An enrolled student still starts the quiz.
    let sub = api_start_quiz(&server, &auth(f.student, "student"), f.quiz).await;
    assert_eq!(sub["user_id"], f.student);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /quizzes/{id}/submissions/me
// ---------------------------------------------------------------------------

#[tokio::test]
async fn my_submission_returns_own_row_and_404_when_absent() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "MeOk").await;
    let second = seed_user(&pool, &prefix, "SecondStudent", "student").await;
    seed_enrollment(&pool, second, f.course, "student").await;
    let no_start = seed_user(&pool, &prefix, "NoStart", "student").await;
    seed_enrollment(&pool, no_start, f.course, "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;

    let sub1 = api_start_quiz(&server, &auth(f.student, "student"), f.quiz).await;
    let sub2 = api_start_quiz(&server, &auth(second, "student"), f.quiz).await;

    // Each enrolled user sees exactly their own submission.
    let mine1 = get_ok(
        &server,
        &auth(f.student, "student"),
        &format!("/quizzes/{}/submissions/me", f.quiz),
    )
    .await;
    assert_eq!(mine1["id"], sub1["id"]);
    assert_eq!(mine1["user_id"], f.student);
    assert_eq!(mine1["quiz_id"], f.quiz);

    let mine2 = get_ok(
        &server,
        &auth(second, "student"),
        &format!("/quizzes/{}/submissions/me", f.quiz),
    )
    .await;
    assert_eq!(mine2["id"], sub2["id"]);
    assert_eq!(mine2["user_id"], second);

    // Enrolled but never started: 404.
    let resp = server
        .get(&format!("/quizzes/{}/submissions/me", f.quiz))
        .add_header(axum::http::header::AUTHORIZATION, auth(no_start, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unenrolled: 403 even though submissions exist.
    let resp = server
        .get(&format!("/quizzes/{}/submissions/me", f.quiz))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/quizzes/{id}/submissions  (teacher/admin listing)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_quiz_submissions_teacher_view_newest_first_student_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "ListSubs").await;
    let second = seed_user(&pool, &prefix, "SecondStudent", "student").await;
    seed_enrollment(&pool, second, f.course, "student").await;

    api_start_quiz(&server, &auth(f.student, "student"), f.quiz).await;
    api_start_quiz(&server, &auth(second, "student"), f.quiz).await;

    let listed = get_ok(
        &server,
        &auth(f.teacher, "teacher"),
        &format!("/courses/{}/quizzes/{}/submissions", f.course, f.quiz),
    )
    .await;

    let arr = listed.as_array().expect("array response");
    assert_eq!(arr.len(), 2, "both submissions: {arr:?}");
    // Ordered by started_at DESC: the later start comes first.
    assert_eq!(arr[0]["user_id"], second);
    assert_eq!(arr[0]["user_name"], "SecondStudent");
    assert_eq!(
        arr[0]["user_email"],
        format!("{prefix}-SecondStudent@test.local")
    );
    assert_eq!(arr[1]["user_id"], f.student);
    assert_eq!(arr[1]["user_name"], "Student");
    assert_eq!(arr[1]["user_email"], format!("{prefix}-Student@test.local"));
    // Ungraded submissions carry no score.
    assert!(arr.iter().all(|s| s.get("score").is_none()));

    // Students cannot list all submissions, even their own course's.
    let resp = server
        .get(&format!(
            "/courses/{}/quizzes/{}/submissions",
            f.course, f.quiz
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(f.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_quiz_submissions_404_for_missing_or_wrong_course_quiz() {
    // The handler loads the quiz's own course_id first: a missing quiz, or a
    // quiz that is not in the PATH course, is 404. Authorization is then
    // checked on the quiz's course.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let f = seed_fixture(&server, &pool, &prefix, "PathCourseOnly").await;
    api_start_quiz(&server, &auth(f.student, "student"), f.quiz).await;

    let other_teacher = seed_user(&pool, &prefix, "OtherTeacher", "teacher").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, other_teacher, other_course, "teacher").await;

    // The other course's teacher cannot read the quiz's submissions through
    // their own course path: the quiz is not in that course.
    let resp = server
        .get(&format!(
            "/courses/{other_course}/quizzes/{}/submissions",
            f.quiz
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(other_teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Even with the quiz's real course in the path, a teacher from another
    // course is forbidden by the quiz-course authorization check.
    let resp = server
        .get(&format!(
            "/courses/{}/quizzes/{}/submissions",
            f.course, f.quiz
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(other_teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A missing quiz is 404, even for an admin who bypasses authorization.
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let resp = server
        .get(&format!(
            "/courses/{other_course}/quizzes/999999999/submissions"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The correct course's teacher still sees the submissions.
    let listed = get_ok(
        &server,
        &auth(f.teacher, "teacher"),
        &format!("/courses/{}/quizzes/{}/submissions", f.course, f.quiz),
    )
    .await;
    let arr = listed.as_array().expect("array response");
    assert_eq!(arr.len(), 1, "one submission: {arr:?}");
    assert_eq!(arr[0]["quiz_id"], f.quiz);
    assert_eq!(arr[0]["user_id"], f.student);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Full lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_quiz_lifecycle_create_start_list_update_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "LifeTeacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "LifeStudent", "student").await;
    let course = seed_course(&pool, &prefix, "Lifecycle").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let teacher_auth = &auth(teacher, "teacher");
    let student_auth = &auth(student, "student");

    // 1) Teacher creates an unpublished practice quiz.
    let created: Value = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_auth)
        .json(&json!({
            "title": "Lifecycle Quiz",
            "quiz_type": "practice",
            "time_limit": 20,
            "points_possible": 10.0
        }))
        .await
        .json();
    assert_eq!(created["published"].as_bool(), Some(false));
    let quiz_id = created["id"].as_i64().expect("quiz id");

    // 2) Student lists the course quizzes and finds it.
    let listed = get_ok(&server, student_auth, &format!("/courses/{course}/quizzes")).await;
    let arr = listed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], quiz_id);
    assert_eq!(arr[0]["title"], "Lifecycle Quiz");

    // 3) Student opens it and starts; the second start is rejected.
    let started = api_start_quiz(&server, student_auth, quiz_id).await;
    let submission_id = started["id"].as_i64().expect("submission id");
    let resp = server
        .post(&format!("/quizzes/{quiz_id}/start"))
        .add_header(axum::http::header::AUTHORIZATION, student_auth)
        .await;
    assert_eq!(resp.status_code(), 409, "second start: {}", resp.text());

    // 4) Student reads their own submission.
    let mine = get_ok(
        &server,
        student_auth,
        &format!("/quizzes/{quiz_id}/submissions/me"),
    )
    .await;
    assert_eq!(mine["id"], submission_id);
    assert_eq!(mine["user_id"], student);

    // 5) Teacher lists the course-level submissions and sees the student.
    let listed = get_ok(
        &server,
        teacher_auth,
        &format!("/courses/{course}/quizzes/{quiz_id}/submissions"),
    )
    .await;
    let arr = listed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["user_id"], student);
    assert_eq!(arr[0]["user_name"], "LifeStudent");

    // 6) Teacher publishes the quiz; the student sees the new state.
    let resp = server
        .put(&format!("/quizzes/{quiz_id}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_auth)
        .json(&json!({ "published": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "publish: {}", resp.text());
    let got = get_ok(&server, student_auth, &format!("/quizzes/{quiz_id}")).await;
    assert_eq!(got["published"].as_bool(), Some(true));

    // 7) Teacher deletes; the quiz and its submissions are gone.
    let resp = server
        .delete(&format!("/quizzes/{quiz_id}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_auth)
        .await;
    assert_eq!(resp.status_code(), 204, "delete: {}", resp.text());

    let resp = server
        .get(&format!("/quizzes/{quiz_id}"))
        .add_header(axum::http::header::AUTHORIZATION, student_auth)
        .await;
    assert_eq!(resp.status_code(), 404, "quiz gone");

    let resp = server
        .get(&format!("/quizzes/{quiz_id}/submissions/me"))
        .add_header(axum::http::header::AUTHORIZATION, student_auth)
        .await;
    assert_eq!(resp.status_code(), 404, "submission gone with the quiz");

    let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM quiz_submissions WHERE id = $1")
        .bind(submission_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows.0, 0, "submission row cascaded away");
    assert_eq!(db_count_quizzes_in_course(&pool, course).await, 0);

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
    let resp = server.get("/courses/1/quizzes").await;
    assert_eq!(resp.status_code(), 401, "list: {}", resp.text());

    let resp = server
        .post("/courses/1/quizzes")
        .json(&json!({ "title": "X" }))
        .await;
    assert_eq!(resp.status_code(), 401, "create: {}", resp.text());

    let resp = server.get("/quizzes/1").await;
    assert_eq!(resp.status_code(), 401, "get: {}", resp.text());

    let resp = server
        .put("/quizzes/1")
        .json(&json!({ "title": "X" }))
        .await;
    assert_eq!(resp.status_code(), 401, "update: {}", resp.text());

    let resp = server.delete("/quizzes/1").await;
    assert_eq!(resp.status_code(), 401, "delete: {}", resp.text());

    let resp = server.post("/quizzes/1/start").await;
    assert_eq!(resp.status_code(), 401, "start: {}", resp.text());

    let resp = server.get("/quizzes/1/submissions/me").await;
    assert_eq!(resp.status_code(), 401, "my submission: {}", resp.text());

    let resp = server.get("/courses/1/quizzes/1/submissions").await;
    assert_eq!(
        resp.status_code(),
        401,
        "course submissions listing: {}",
        resp.text()
    );

    // A malformed bearer token is rejected the same way.
    let resp = server
        .get("/courses/1/quizzes")
        .add_header(axum::http::header::AUTHORIZATION, "Bearer not-a-token")
        .await;
    assert_eq!(resp.status_code(), 401, "garbage token: {}", resp.text());
}
