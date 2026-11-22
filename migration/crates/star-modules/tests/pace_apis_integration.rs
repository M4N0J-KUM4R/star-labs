//! Section paces and student-enrollment paces (Canvas
//! `CoursePacing::SectionPacesApiController` and
//! `CoursePacing::StudentEnrollmentPacesApiController`) integration tests.
//!
//! Covers the Canvas routes:
//! - `GET    /api/v1/courses/:course_id/sections/:course_section_id/pace`
//! - `POST   /api/v1/courses/:course_id/sections/:course_section_id/paces`
//! - `PATCH  /api/v1/courses/:course_id/sections/:course_section_id/pace`
//! - `DELETE /api/v1/courses/:course_id/sections/:course_section_id/pace`
//! - `GET    /api/v1/courses/:course_id/student_enrollments/:student_enrollment_id/pace`
//! - `POST   /api/v1/courses/:course_id/student_enrollments/:student_enrollment_id/paces`
//! - `PATCH  /api/v1/courses/:course_id/student_enrollments/:student_enrollment_id/pace`
//! - `DELETE /api/v1/courses/:course_id/student_enrollments/:student_enrollment_id/pace`
//!
//! Fleet rule: dedicated users, accounts, courses and sections per test,
//! every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is
//! unset, and skipped cleanly while the `course_paces` tables are not
//! migrated yet.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "pa-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
        n
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

    let tables: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_name IN ('course_paces', 'course_pace_module_items')",
    )
    .fetch_one(&pool)
    .await
    .expect("count the course pace tables");
    if tables.0 < 2 {
        println!("SKIP pace_apis_integration: the course_paces tables are not migrated yet");
        return None;
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server =
        TestServer::new(star_modules::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
    )
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id, workflow_state) \
         VALUES ($1, NULL, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id, workflow_state) \
         VALUES ($1, $2, $3, 'available') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_section(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO course_sections (course_id, name, workflow_state) \
         VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed section")
}

async fn seed_enrollment(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    role: &str,
    section_id: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, course_section_id, workflow_state) \
         VALUES ($1, $2, $3, $4, 'active') RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .bind(section_id)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

/// Canvas `Course#enable_course_paces` is a course setting.
async fn enable_course_paces(pool: &PgPool, course_id: i64) {
    sqlx::query(
        "INSERT INTO course_settings (course_id, settings) \
         VALUES ($1, '{\"enable_course_paces\": true}') \
         ON CONFLICT (course_id) DO UPDATE SET settings = EXCLUDED.settings",
    )
    .bind(course_id)
    .execute(pool)
    .await
    .expect("enable the course paces setting");
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query(
        "DELETE FROM course_pace_module_items WHERE course_pace_id IN \
             (SELECT id FROM course_paces WHERE course_id IN \
                 (SELECT id FROM courses WHERE name LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query(
        "DELETE FROM course_paces WHERE course_id IN (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query(
        "DELETE FROM course_settings WHERE course_id IN (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query(
        "DELETE FROM course_sections WHERE course_id IN (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query("DELETE FROM courses WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
}

#[tokio::test]
async fn a_section_pace_is_created_read_updated_and_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    enable_course_paces(&pool, course).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;
    let section = seed_section(&pool, course, "Section One").await;

    // No pace yet: 404.
    let resp = server
        .get(&format!("/courses/{course}/sections/{section}/pace"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Create it.
    let resp = server
        .post(&format!("/courses/{course}/sections/{section}/paces"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["pace"]["context_id"].as_i64(), Some(section));
    assert_eq!(body["pace"]["context_type"], "Section");
    assert_eq!(body["pace"]["workflow_state"], "unpublished");
    assert_eq!(body["pace"]["section"]["name"], "Section One");

    // Read it back.
    let resp = server
        .get(&format!("/courses/{course}/sections/{section}/pace"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["pace"]["context_id"].as_i64(), Some(section));

    // Update it.
    let resp = server
        .patch(&format!("/courses/{course}/sections/{section}/pace"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "pace": { "hard_end_dates": true, "workflow_state": "active" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["pace"]["hard_end_dates"], json!(true));
    assert_eq!(body["pace"]["workflow_state"], "active");

    // Delete it.
    let resp = server
        .delete(&format!("/courses/{course}/sections/{section}/pace"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    let resp = server
        .get(&format!("/courses/{course}/sections/{section}/pace"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_enrollment_pace_is_created_read_and_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    enable_course_paces(&pool, course).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;
    let section = seed_section(&pool, course, "Section One").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let enrollment = seed_enrollment(&pool, student, course, "student", Some(section)).await;

    let resp = server
        .get(&format!(
            "/courses/{course}/student_enrollments/{enrollment}/pace"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .post(&format!(
            "/courses/{course}/student_enrollments/{enrollment}/paces"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["pace"]["context_id"].as_i64(), Some(enrollment));
    assert_eq!(body["pace"]["context_type"], "StudentEnrollment");
    assert_eq!(body["pace"]["student"]["name"], "Student");

    let resp = server
        .get(&format!(
            "/courses/{course}/student_enrollments/{enrollment}/pace"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .delete(&format!(
            "/courses/{course}/student_enrollments/{enrollment}/pace"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_section_of_another_course_is_a_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let other = seed_course(&pool, &prefix, "Other", account).await;
    enable_course_paces(&pool, course).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;
    let section = seed_section(&pool, other, "Other Section").await;

    let resp = server
        .get(&format!("/courses/{course}/sections/{section}/pace"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_may_not_manage_a_pace() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    enable_course_paces(&pool, course).await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let section = seed_section(&pool, course, "Section One").await;
    seed_enrollment(&pool, student, course, "student", Some(section)).await;

    let resp = server
        .get(&format!("/courses/{course}/sections/{section}/pace"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
