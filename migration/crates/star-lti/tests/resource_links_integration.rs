//! LTI resource links (Canvas `Lti::ResourceLinksController`) integration
//! tests.
//!
//! Covers the Canvas routes:
//! - `GET|POST /api/v1/courses/:course_id/lti_resource_links`
//! - `POST /api/v1/courses/:course_id/lti_resource_links/bulk`
//! - `GET|PUT|DELETE /api/v1/courses/:course_id/lti_resource_links/:id`
//!
//! Fleet rule: dedicated users, accounts, courses and tools per test, every
//! seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

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
        "rl-{}-{}-{}",
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

/// A tool of the course — the resource links point at a launch URL.
async fn seed_tool(pool: &PgPool, prefix: &str, course_id: i64, url: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO external_tools (name, url, domain, course_id, context_type, context_id, \
             workflow_state, tool_configuration) \
         VALUES ($1, $2, 'tool.test', $3, 'Course', $3, 'active', '{}') RETURNING id",
    )
    .bind(format!("{prefix}-tool"))
    .bind(url)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .expect("seed tool")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query(
        "DELETE FROM lti_resource_links WHERE context_id IN \
             (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
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
async fn a_resource_link_is_created_read_updated_and_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let url = "https://tool.test/launch";
    seed_tool(&pool, &prefix, course, url).await;

    let resp = server
        .post(&format!("/courses/{course}/lti_resource_links"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "url": url, "title": "A link" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["url"], url);
    assert_eq!(body["title"], "A link");
    assert_eq!(body["context_type"], "Course");
    assert_eq!(body["resource_type"], "rich_content");
    let id = body["id"].as_i64().expect("link id");

    // List them.
    let resp = server
        .get(&format!("/courses/{course}/lti_resource_links"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().expect("links").len(), 1);

    // Show one.
    let resp = server
        .get(&format!("/courses/{course}/lti_resource_links/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Update it.
    let resp = server
        .put(&format!("/courses/{course}/lti_resource_links/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "title": "Renamed" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], "Renamed");

    // Delete it.
    let resp = server
        .delete(&format!("/courses/{course}/lti_resource_links/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_unknown_url_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .post(&format!("/courses/{course}/lti_resource_links"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "url": "https://unknown.test/launch" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_missing_course_is_a_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;

    let resp = server
        .get("/courses/999999/lti_resource_links")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
