//! Integration tests for the account-scoped enrollments tail (#58).
//!
//! Canvas `EnrollmentsApiController#show` (GET
//! /api/v1/accounts/:account_id/enrollments/:id) and `#bulk_enrollment`
//! (POST /api/v1/accounts/:account_id/bulk_enrollment) against a real
//! Postgres, through the same TestServer harness as enrollments_integration.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-enrollments
//! - Unique email/course prefix per run + full teardown, including the
//!   dedicated accounts seeded here (never the root account).

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness (same shape as enrollments_integration.rs)
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "enr-acc-tests-{}-{}",
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

    let server = TestServer::new(star_enrollments_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn star_enrollments_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_enrollments::router())
        .with_state(state)
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
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code, account_id)
        VALUES ($1, $2, 1)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed course")
}

/// Seed a course hanging under a dedicated account (not the root account), so
/// sub-account visibility can be exercised.
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

/// Seed a dedicated account (optionally under a parent) — never account 1.
async fn seed_account(pool: &PgPool, prefix: &str, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

/// Make a user an account admin through `account_users` (Canvas
/// `:read_roster` approximation for non site-admins).
async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
}

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

async fn seed_student_enrollment(pool: &PgPool, user_id: i64, course_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, 'student', 'active') RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .expect("seed student enrollment")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

/// Delete every row created by this run: users cascade to enrollments and
/// account_users, accounts and courses are removed by their prefix, and the
/// feature_flags table is never touched by these tests.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
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
    // child accounts first, then parents (self-referencing FK)
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{accountId}/enrollments/{id}  (Canvas #show)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn show_own_enrollment_via_root_account_returns_200() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;
    let enrollment = seed_student_enrollment(&pool, student, course).await;

    let resp = server
        .get(&format!("/accounts/1/enrollments/{enrollment}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "own enrollment should render; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(enrollment));
    assert_eq!(body["user_id"].as_i64(), Some(student));
    assert_eq!(body["course_id"].as_i64(), Some(course));
    assert_eq!(body["enrollment_state"], json!("active"));
    assert_eq!(body["role"], json!("student"));
    assert!(
        body.get("workflow_state").is_none(),
        "Canvas renames workflow_state to enrollment_state"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_enrollment_as_account_admin_returns_200() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // dedicated account under the root, with the course hanging under it
    let account = seed_account(&pool, &prefix, "acct", Some(1)).await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let admin = seed_user(&pool, &prefix, "admin", "student").await;
    seed_account_admin(&pool, account, admin).await;
    let course = seed_course_in_account(&pool, &prefix, "course", account).await;
    let enrollment = seed_student_enrollment(&pool, student, course).await;

    let resp = server
        .get(&format!("/accounts/{account}/enrollments/{enrollment}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "account admin should read the roster; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(enrollment));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_enrollment_as_site_admin_via_root_account_returns_200() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "student", "student").await;
    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let course = seed_course(&pool, &prefix, "course").await;
    let enrollment = seed_student_enrollment(&pool, student, course).await;

    // Root-account context (account 1) sees enrollments of any course under
    // it; the site admin passes `:read_roster`.
    let resp = server
        .get(&format!("/accounts/1/enrollments/{enrollment}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_enrollment_of_other_user_without_roster_right_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "student", "student").await;
    let stranger = seed_user(&pool, &prefix, "stranger", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;
    let enrollment = seed_student_enrollment(&pool, student, course).await;

    let resp = server
        .get(&format!("/accounts/1/enrollments/{enrollment}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "Canvas render_unauthorized_action; body: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_unknown_enrollment_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    let resp = server
        .get("/accounts/1/enrollments/999999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_unknown_account_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    let resp = server
        .get("/accounts/999999999/enrollments/1")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "Canvas require_context raises RecordNotFound; body: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_from_unrelated_sub_account_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // course hangs directly under the root account; the request context is an
    // unrelated sub-account whose chain does not contain the course account
    let account = seed_account(&pool, &prefix, "other-acct", Some(1)).await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    seed_account_admin(&pool, account, admin).await;
    let course = seed_course(&pool, &prefix, "course").await;
    let enrollment = seed_student_enrollment(&pool, student, course).await;

    let resp = server
        .get(&format!("/accounts/{account}/enrollments/{enrollment}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "sub-account outside the course's account chain must 404; body: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_deleted_enrollment_still_renders() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Canvas `all_enrollments` does not filter workflow_state, so a deleted
    // enrollment is still found by #show.
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;
    let enrollment: i64 = sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, 'student', 'deleted') RETURNING id",
    )
    .bind(student)
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed deleted enrollment");

    let resp = server
        .get(&format!("/accounts/1/enrollments/{enrollment}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["enrollment_state"], json!("deleted"));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /accounts/{accountId}/bulk_enrollment  (Canvas #bulk_enrollment)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bulk_enrollment_creates_enrollments_and_returns_progress() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let s1 = seed_user(&pool, &prefix, "s1", "student").await;
    let s2 = seed_user(&pool, &prefix, "s2", "student").await;
    let c1 = seed_course(&pool, &prefix, "c1").await;
    let c2 = seed_course(&pool, &prefix, "c2").await;

    let resp = server
        .post("/accounts/1/bulk_enrollment")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": [s1, s2],
            "course_ids": [c1, c2],
            "enrollment_type": "StudentEnrollment"
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "bulk enrollment should succeed; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["context_type"], json!("Account"));
    assert_eq!(body["context_id"].as_i64(), Some(1));
    assert_eq!(body["tag"], json!("bulk_enrollment"));
    assert_eq!(body["workflow_state"], json!("completed"));
    assert_eq!(body["completion"], json!(100));
    assert_eq!(body["user_id"].as_i64(), Some(admin));

    // four enrollments created, one per (user, course) pair
    for (user, course) in [(s1, c1), (s1, c2), (s2, c1), (s2, c2)] {
        let found: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'student'",
        )
        .bind(user)
        .bind(course)
        .fetch_one(&pool)
        .await
        .expect("count enrollments");
        assert_eq!(
            found.0, 1,
            "expected exactly one enrollment for ({user}, {course})"
        );
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_with_default_type_enrolls_students() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;

    // no enrollment_type -> Canvas defaults to StudentEnrollment
    let resp = server
        .post("/accounts/1/bulk_enrollment")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": [student],
            "course_ids": [course]
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    let found: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'student'",
    )
    .bind(student)
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("count enrollments");
    assert_eq!(found.0, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_invalid_type_is_400() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;

    let resp = server
        .post("/accounts/1/bulk_enrollment")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": [student],
            "course_ids": [course],
            "enrollment_type": "NonsenseEnrollment"
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "Canvas: Invalid enrollment type.; body: {}",
        resp.text()
    );
    assert!(resp.text().contains("Invalid enrollment type."));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_over_500_pairs_is_400() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    // 501 pairs without seeding that many rows: user_ids 1..=501 x one course
    // id. The pair-count check runs before any course lookup in Canvas, so
    // the ids do not need to exist.
    let user_ids: Vec<i64> = (1..=501).collect();
    let resp = server
        .post("/accounts/1/bulk_enrollment")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": user_ids,
            "course_ids": [1]
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "Canvas: Too many users to enroll at once.; body: {}",
        resp.text()
    );
    assert!(resp.text().contains("Too many users to enroll at once."));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_by_plain_student_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "student", "student").await;
    let other = seed_user(&pool, &prefix, "other", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;

    let resp = server
        .post("/accounts/1/bulk_enrollment")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "user_ids": [other],
            "course_ids": [course]
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "Canvas render_unauthorized_action for can_create_enrollment_for?; body: {}",
        resp.text()
    );

    let found: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM enrollments WHERE course_id = $1")
        .bind(course)
        .fetch_one(&pool)
        .await
        .expect("count enrollments");
    assert_eq!(found.0, 0, "no enrollment should be created");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_by_course_teacher_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // teachers pass can_create_enrollment_for? on the courses they teach
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;
    seed_course_teacher(&pool, teacher, course).await;

    let resp = server
        .post("/accounts/1/bulk_enrollment")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "user_ids": [student],
            "course_ids": [course]
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    let found: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'student'",
    )
    .bind(student)
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("count enrollments");
    assert_eq!(found.0, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_unknown_course_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;

    let resp = server
        .post("/accounts/1/bulk_enrollment")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": [student],
            "course_ids": [999999999]
        }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_unknown_account_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    let resp = server
        .post("/accounts/999999999/bulk_enrollment")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": [1],
            "course_ids": [1]
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "Canvas require_context raises RecordNotFound; body: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_is_idempotent_for_existing_pairs() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;

    let body = json!({
        "user_ids": [student],
        "course_ids": [course]
    });
    for i in 0..2 {
        let resp = server
            .post("/accounts/1/bulk_enrollment")
            .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
            .json(&body)
            .await;
        assert_eq!(resp.status_code(), 201, "round {i}: {}", resp.text());
    }

    // The UNIQUE (user_id, course_id, role) constraint makes the second run a
    // no-op rather than an error, matching Canvas's
    // allow_multiple_enrollments handling in Enrollment::BulkUpdate.
    let found: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM enrollments WHERE user_id = $1 AND course_id = $2")
            .bind(student)
            .bind(course)
            .fetch_one(&pool)
            .await
            .expect("count enrollments");
    assert_eq!(found.0, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_teacher_enrollment_type_sets_teacher_role() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "teacher", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;

    let resp = server
        .post("/accounts/1/bulk_enrollment")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": [teacher],
            "course_ids": [course],
            "enrollment_type": "TeacherEnrollment"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    let found: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'",
    )
    .bind(teacher)
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("count enrollments");
    assert_eq!(found.0, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_enrollment_without_token_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server
        .post("/accounts/1/bulk_enrollment")
        .json(&json!({
            "user_ids": [1],
            "course_ids": [1]
        }))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_without_token_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.get("/accounts/1/enrollments/1").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
