//! Course pacing contexts (Canvas `CoursePacing::PaceContextsApiController`)
//! integration tests.
//!
//! Covers the Canvas route `GET /api/v1/courses/:course_id/pace_contexts`:
//! the listing of the contexts of a course that can be paced, filtered by
//! the `type` parameter ("course", "section" or "student_enrollment").
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
        "pc-{}-{}-{}",
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

    // The course_paces tables arrive with their own migration. Until then
    // the slice has nothing to talk to.
    let tables: Option<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_name IN ('course_paces', 'course_pace_module_items')",
    )
    .fetch_one(&pool)
    .await
    .expect("count the course pace tables");
    if tables.0 < 2 {
        println!("SKIP pace_contexts_integration: the course_paces tables are not migrated yet");
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
    sqlx::query("DELETE FROM course_sections WHERE course_id IN (SELECT id FROM courses WHERE name LIKE $1)")
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
async fn the_course_context_is_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    enable_course_paces(&pool, course).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;

    let resp = server
        .get(&format!("/courses/{course}/pace_contexts?type=course"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let contexts = body["pace_contexts"].as_array().expect("pace contexts");
    assert_eq!(contexts.len(), 1, "body: {body}");
    assert_eq!(contexts[0]["type"], "Course");
    assert_eq!(contexts[0]["item_id"].as_i64(), Some(course));
    assert_eq!(body["total_entries"].as_i64(), Some(1));
    // A course context is never "on pace" — only student enrollments are.
    assert!(contexts[0]["on_pace"].is_null(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_sections_and_the_students_are_listed() {
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
    seed_enrollment(&pool, student, course, "student", Some(section)).await;

    let resp = server
        .get(&format!("/courses/{course}/pace_contexts?type=section"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let contexts = body["pace_contexts"].as_array().expect("pace contexts");
    assert_eq!(contexts.len(), 1, "body: {body}");
    assert_eq!(contexts[0]["type"], "CourseSection");
    assert_eq!(contexts[0]["item_id"].as_i64(), Some(section));

    let resp = server
        .get(&format!(
            "/courses/{course}/pace_contexts?type=student_enrollment"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let contexts = body["pace_contexts"].as_array().expect("pace contexts");
    assert_eq!(contexts.len(), 1, "body: {body}");
    assert_eq!(contexts[0]["type"], "StudentEnrollment");
    // A student enrollment without an overdue item is on pace.
    assert_eq!(contexts[0]["on_pace"], json!(true), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_unknown_type_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    enable_course_paces(&pool, course).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;

    let resp = server
        .get(&format!("/courses/{course}/pace_contexts?type=bogus"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_course_without_paces_is_a_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;

    // The course setting `enable_course_paces` is off.
    let resp = server
        .get(&format!("/courses/{course}/pace_contexts?type=course"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_may_not_list_the_contexts() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    enable_course_paces(&pool, course).await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, student, course, "student", None).await;

    let resp = server
        .get(&format!("/courses/{course}/pace_contexts?type=course"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
