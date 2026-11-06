//! Integration tests for star-enrollments.
//!
//! Exercises the real enrollments router (POST /enrollments, GET /enrollments,
//! DELETE /enrollments/{id}) through axum-test against a real Postgres.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-enrollments
//! - Uses a unique email prefix per run + full teardown so it is safe to run
//!   repeatedly against a shared/dev database.

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
        "enr-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the enrollments
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

    let server = TestServer::new(star_enrollments_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same merge pattern as star-http::build_api_router, restricted to the
/// crates needed by the enrollments endpoints.
fn star_enrollments_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_enrollments::router())
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

/// Give a user active teacher access to a course. Course-scoped teacher
/// listing/deletion requires an actual teacher enrollment, not only a global
/// users.role value.
async fn seed_course_teacher(pool: &PgPool, user_id: i64, course_id: i64) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, 'teacher', 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .execute(pool)
    .await
    .expect("seed teacher course enrollment");
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run (users cascade to enrollments).
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM enrollments WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

/// GET /enrollments with the given bearer token and query string.
async fn list_enrollments(server: &TestServer, token: &str, query: &str) -> Value {
    let mut path = "/enrollments".to_string();
    if !query.is_empty() {
        path.push('?');
        path.push_str(query);
    }
    let resp = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "list should succeed; body: {}",
        resp.text()
    );
    resp.json()
}

// ---------------------------------------------------------------------------
// POST /enrollments
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_enrollment_as_teacher_returns_201_and_body() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "CreateCourse").await;

    let resp = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "user_id": student, "course_id": course, "role": "student" }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["user_id"], student);
    assert_eq!(body["course_id"], course);
    assert_eq!(body["role"], "student");
    assert_eq!(body["name"], "Student", "unexpected name: {body}");
    assert_eq!(body["email"], format!("{prefix}-Student@test.local"));

    // Row actually persisted.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'student'")
        .bind(student)
        .bind(course)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_enrollment_defaults_role_to_student() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "DefaultRole").await;

    let resp = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "user_id": student, "course_id": course }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["role"], "student");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_enrollment_as_admin_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "AdminCreate").await;

    let resp = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .json(&json!({ "user_id": student, "course_id": course, "role": "teacher" }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["role"], "teacher");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_enrollment_duplicate_is_409() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "DupCourse").await;

    let payload = json!({ "user_id": student, "course_id": course, "role": "student" });
    let first = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&payload)
        .await;
    assert_eq!(first.status_code(), 201, "first insert: {}", first.text());

    let second = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&payload)
        .await;
    assert_eq!(second.status_code(), 409, "body: {}", second.text());

    // Same user+course but different role is a different row — allowed.
    let other_role = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "user_id": student, "course_id": course, "role": "teacher" }))
        .await;
    assert_eq!(other_role.status_code(), 201, "body: {}", other_role.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_enrollment_missing_course_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;

    let resp = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "user_id": student, "course_id": 999_999_999, "role": "student" }))
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_enrollment_missing_user_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "NoUser").await;

    let resp = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "user_id": 999_999_999, "course_id": course, "role": "student" }))
        .await;

    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_enrollment_invalid_role_is_422() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "BadRole").await;

    let resp = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "user_id": student, "course_id": course, "role": "guardian" }))
        .await;

    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_enrollment_as_student_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "ForbiddenCreate").await;

    let resp = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .json(&json!({ "user_id": other, "course_id": course, "role": "student" }))
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Nothing was created.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM enrollments WHERE course_id = $1")
        .bind(course)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_enrollment_without_token_is_401() {
    let Some((server, _pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server
        .post("/enrollments")
        .json(&json!({ "user_id": 1, "course_id": 1, "role": "student" }))
        .await;

    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
}

// ---------------------------------------------------------------------------
// GET /enrollments
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_as_admin_sees_all_and_filters() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let t1 = seed_user(&pool, &prefix, "T1", "teacher").await;
    let s1 = seed_user(&pool, &prefix, "S1", "student").await;
    let s2 = seed_user(&pool, &prefix, "S2", "student").await;
    let c1 = seed_course(&pool, &prefix, "ListCourseA").await;
    let c2 = seed_course(&pool, &prefix, "ListCourseB").await;

    for (u, c, r) in [(t1, c1, "teacher"), (s1, c1, "student"), (s2, c2, "student")] {
        let resp = server
            .post("/enrollments")
            .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
            .json(&json!({ "user_id": u, "course_id": c, "role": r }))
            .await;
        assert_eq!(resp.status_code(), 201, "seed enroll: {}", resp.text());
    }

    // Unfiltered: all three.
    let all = list_enrollments(&server, &auth(admin, "admin"), "").await;
    let arr = all.as_array().expect("array response");
    let ours: Vec<&Value> = arr
        .iter()
        .filter(|e| e["email"].as_str().map(|e| e.starts_with(&prefix)).unwrap_or(false))
        .collect();
    assert_eq!(ours.len(), 3, "ours: {ours:?}");

    // Filter by course.
    let by_course = list_enrollments(&server, &auth(admin, "admin"), &format!("course_id={c1}")).await;
    let arr = by_course.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert!(arr.iter().all(|e| e["course_id"] == c1));

    // Filter by user.
    let by_user = list_enrollments(&server, &auth(admin, "admin"), &format!("user_id={s1}")).await;
    let arr = by_user.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["user_id"], s1);
    assert_eq!(arr[0]["course_id"], c1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_as_teacher_scoped_to_own_courses() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let mine = seed_user(&pool, &prefix, "TeacherMine", "teacher").await;
    let other = seed_user(&pool, &prefix, "TeacherOther", "teacher").await;
    let s1 = seed_user(&pool, &prefix, "S1", "student").await;
    let s2 = seed_user(&pool, &prefix, "S2", "student").await;
    let my_course = seed_course(&pool, &prefix, "MyCourse").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;

    for (u, c, r) in [
        (mine, my_course, "teacher"),
        (other, other_course, "teacher"),
        (s1, my_course, "student"),
        (s2, other_course, "student"),
    ] {
        let resp = server
            .post("/enrollments")
            .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
            .json(&json!({ "user_id": u, "course_id": c, "role": r }))
            .await;
        assert_eq!(resp.status_code(), 201, "seed enroll: {}", resp.text());
    }

    // Teacher sees only enrollments in courses they teach.
    let all = list_enrollments(&server, &auth(mine, "teacher"), "").await;
    let arr = all.as_array().expect("array response");
    assert_eq!(arr.len(), 2, "teacher 'mine' should see 2 rows: {arr:?}");
    assert!(arr.iter().all(|e| e["course_id"] == my_course));

    // course_id filter within scope works.
    let scoped = list_enrollments(&server, &auth(mine, "teacher"), &format!("course_id={my_course}")).await;
    assert_eq!(scoped.as_array().unwrap().len(), 2);

    // course_id filter outside scope is Forbidden.
    let out_of_scope = server
        .get(&format!("/enrollments?course_id={other_course}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(mine, "teacher"))
        .await;
    assert_eq!(out_of_scope.status_code(), 403, "body: {}", out_of_scope.text());

    // user_id filter still constrained to own courses.
    let by_user = list_enrollments(&server, &auth(mine, "teacher"), &format!("user_id={s2}")).await;
    assert_eq!(by_user.as_array().unwrap().len(), 0, "s2 is in other teacher's course");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_as_student_gets_empty_array() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other_student = seed_user(&pool, &prefix, "S2", "student").await;
    let course = seed_course(&pool, &prefix, "StudentList").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    // Populate the DB so a broken scoping check (e.g. returning everything)
    // would fail the test.
    for (u, c, r) in [(student, course, "student"), (other_student, course, "student")] {
        let resp = server
            .post("/enrollments")
            .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
            .json(&json!({ "user_id": u, "course_id": c, "role": r }))
            .await;
        assert_eq!(resp.status_code(), 201, "seed enroll: {}", resp.text());
    }

    let resp = list_enrollments(&server, &auth(student, "student"), "").await;
    assert_eq!(resp.as_array().expect("array").len(), 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_without_token_is_401() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.get("/enrollments").await;
    assert_eq!(resp.status_code(), 401);
}

// ---------------------------------------------------------------------------
// DELETE /enrollments/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_by_course_teacher_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "DeleteCourse").await;
    seed_course_teacher(&pool, teacher, course).await;

    let created: Value = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "user_id": student, "course_id": course, "role": "student" }))
        .await
        .json();

    let enrollment_id = created["id"].as_i64().expect("enrollment id");

    let resp = server
        .delete(&format!("/enrollments/{enrollment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // Soft-deleted enrollments remain in the database for audit/history, but
    // are no longer active.
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM enrollments WHERE id = $1 AND workflow_state != 'deleted'",
    )
        .bind(enrollment_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_by_admin_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "AdminDelete").await;

    let created: Value = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "user_id": student, "course_id": course, "role": "student" }))
        .await
        .json();
    let enrollment_id = created["id"].as_i64().expect("enrollment id");

    let resp = server
        .delete(&format!("/enrollments/{enrollment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_by_other_teacher_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher_a = seed_user(&pool, &prefix, "TeacherA", "teacher").await;
    let teacher_b = seed_user(&pool, &prefix, "TeacherB", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course_a = seed_course(&pool, &prefix, "CourseA").await;

    let created: Value = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .json(&json!({ "user_id": student, "course_id": course_a, "role": "student" }))
        .await
        .json();
    let enrollment_id = created["id"].as_i64().expect("enrollment id");

    // Teacher B does not teach course A.
    let resp = server
        .delete(&format!("/enrollments/{enrollment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher_b, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Row still exists.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM enrollments WHERE id = $1")
        .bind(enrollment_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_by_student_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "StudentDelete").await;

    let created: Value = server
        .post("/enrollments")
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .json(&json!({ "user_id": student, "course_id": course, "role": "student" }))
        .await
        .json();
    let enrollment_id = created["id"].as_i64().expect("enrollment id");

    let resp = server
        .delete(&format!("/enrollments/{enrollment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_missing_enrollment_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .delete("/enrollments/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_without_token_is_401() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.delete("/enrollments/1").await;
    assert_eq!(resp.status_code(), 401);
}

// ---------------------------------------------------------------------------
// Cross-cutting / lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_lifecycle_create_list_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let s1 = seed_user(&pool, &prefix, "S1", "student").await;
    let s2 = seed_user(&pool, &prefix, "S2", "student").await;
    let course = seed_course(&pool, &prefix, "Lifecycle").await;
    seed_course_teacher(&pool, teacher, course).await;

    // Create two enrollments as the teacher.
    for s in [s1, s2] {
        let resp = server
            .post("/enrollments")
            .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
            .json(&json!({ "user_id": s, "course_id": course, "role": "student" }))
            .await;
        assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    }

    // Teacher lists and sees both.
    let listed = list_enrollments(&server, &auth(teacher, "teacher"), &format!("course_id={course}")).await;
    let arr = listed.as_array().unwrap();
    assert_eq!(arr.len(), 3, "teacher plus two student enrollments");
    assert_eq!(
        arr.iter().filter(|e| e["role"] == "student").count(),
        2
    );
    assert!(arr.iter().all(|e| e["course_id"] == course));

    // Admin deletes one of them.
    let enrollment_id = arr
        .iter()
        .find(|e| e["role"] == "student")
        .and_then(|e| e["id"].as_i64())
        .expect("student enrollment id");
    let resp = server
        .delete(&format!("/enrollments/{enrollment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 204);

    // Listing reflects the deletion.
    let after = list_enrollments(&server, &auth(teacher, "teacher"), &format!("course_id={course}")).await;
    let arr = after.as_array().unwrap();
    assert_eq!(arr.len(), 2, "teacher plus one remaining student");
    assert_eq!(arr.iter().filter(|e| e["role"] == "student").count(), 1);
    assert!(!arr.iter().any(|e| e["id"].as_i64() == Some(enrollment_id)));

    cleanup(&pool, &prefix).await;
}
