//! LTI apps (Canvas `Lti::LtiAppsController`) integration tests.
//!
//! Covers the Canvas routes:
//! - `GET /api/v1/courses/:course_id/lti_apps`
//! - `GET /api/v1/accounts/:account_id/lti_apps`
//! - `GET /api/v1/courses/:course_id/lti_apps/launch_definitions`
//! - `GET /api/v1/accounts/:account_id/lti_apps/launch_definitions`
//!
//! Fleet rule: dedicated users, accounts, courses and tools per test, every
//! seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "la-{}-{}-{}",
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

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = TestServer::new(star_lti::router().with_state(state)).expect("build TestServer");
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

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

async fn seed_account_user(pool: &PgPool, account_id: i64, user_id: i64, role: &str) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(account_id)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed account user");
}

async fn seed_tool(
    pool: &PgPool,
    prefix: &str,
    name: &str,
    context_type: &str,
    context_id: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO external_tools (name, description, url, domain, context_type, context_id, \
             workflow_state, tool_configuration) \
         VALUES ($1, 'A tool', 'https://tool.test/launch', 'tool.test', $2, $3, 'active', \
             '{\"placements\": {\"course_navigation\": {\"text\": \"Go\"}}}') \
         RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(context_type)
    .bind(context_id)
    .fetch_one(pool)
    .await
    .expect("seed tool")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query("DELETE FROM external_tools WHERE name LIKE $1")
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
        "DELETE FROM account_users WHERE account_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
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
async fn the_app_definitions_of_a_course_are_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let tool = seed_tool(&pool, &prefix, "Tool", "Course", course).await;

    let resp = server
        .get(&format!("/courses/{course}/lti_apps"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let definitions = body.as_array().expect("app definitions");
    assert_eq!(definitions.len(), 1, "body: {body}");
    assert_eq!(definitions[0]["app_type"], "ContextExternalTool");
    assert_eq!(definitions[0]["app_id"].as_i64(), Some(tool));
    assert_eq!(definitions[0]["context"], "Course");
    assert_eq!(definitions[0]["context_id"].as_i64(), Some(course));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_may_not_read_the_app_definitions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, student, course, "student").await;

    let resp = server
        .get(&format!("/courses/{course}/lti_apps"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_launch_definitions_of_a_course_are_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_tool(&pool, &prefix, "Tool", "Course", course).await;

    // The placements array is percent-encoded: a raw `[` / `"` is an
    // invalid URI character for the test server's request builder.
    let resp = server
        .get(&format!(
            "/courses/{course}/lti_apps/launch_definitions?placements=%5B%22course_navigation%22%5D"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let definitions = body.as_array().expect("launch definitions");
    assert_eq!(definitions.len(), 1, "body: {body}");
    assert_eq!(definitions[0]["definition_type"], "ContextExternalTool");
    assert_eq!(definitions[0]["url"], "https://tool.test/launch");
    assert_eq!(definitions[0]["domain"], "tool.test");
    assert!(definitions[0]["placements"]["course_navigation"].is_object());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_launch_definitions_without_placements_are_empty() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_tool(&pool, &prefix, "Tool", "Course", course).await;

    let resp = server
        .get(&format!("/courses/{course}/lti_apps/launch_definitions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body.as_array().expect("array").is_empty(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_app_definitions_of_an_account_are_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    seed_account_user(&pool, account, admin, "admin").await;
    seed_tool(&pool, &prefix, "Tool", "Account", account).await;

    let resp = server
        .get(&format!("/accounts/{account}/lti_apps"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let definitions = body.as_array().expect("app definitions");
    assert_eq!(definitions.len(), 1, "body: {body}");
    assert_eq!(definitions[0]["context"], "Account");

    cleanup(&pool, &prefix).await;
}
