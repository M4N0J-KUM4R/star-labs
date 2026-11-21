//! Access tokens (Canvas `TokensController`) integration tests.
//!
//! Covers the Canvas routes:
//! - `GET    /api/v1/users/:user_id/user_generated_tokens`
//! - `GET|PUT|DELETE /api/v1/users/:user_id/tokens/:id`
//! - `POST   /api/v1/users/:user_id/tokens`
//!
//! Fleet rule: dedicated users and tokens per test, every seeded row is
//! cleaned up. Skipped cleanly when DATABASE_URL is unset.

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
        "tk-{}-{}-{}",
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
    let server = TestServer::new(star_users::router().with_state(state)).expect("build TestServer");
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

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query("DELETE FROM access_tokens WHERE token LIKE $1")
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
async fn a_token_is_created_read_updated_and_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;

    let resp = server
        .post(&format!("/users/{user}/tokens"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "token": { "purpose": "A purpose", "scopes": ["a_scope"] } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["purpose"], "A purpose");
    assert_eq!(body["user_id"].as_i64(), Some(user));
    assert_eq!(body["workflow_state"], "active");
    // The token value is only rendered when the token is created.
    assert!(body["token"].is_string(), "body: {body}");
    let id = body["id"].as_i64().expect("token id");
    let hint = body["token_hint"].as_str().expect("token hint").to_string();
    assert_eq!(hint.len(), 5, "body: {body}");

    // Read it back by its id.
    let resp = server
        .get(&format!("/users/{user}/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Read it back by its hint.
    let resp = server
        .get(&format!("/users/{user}/tokens/{hint}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Update it.
    let resp = server
        .put(&format!("/users/{user}/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "token": { "purpose": "Another purpose" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["purpose"], "Another purpose");

    // List them.
    let resp = server
        .get(&format!("/users/{user}/user_generated_tokens"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().expect("tokens").len(), 1);

    // Delete it.
    let resp = server
        .delete(&format!("/users/{user}/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_token_without_a_purpose_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;

    let resp = server
        .post(&format!("/users/{user}/tokens"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "token": {} }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn another_user_may_not_read_a_token() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;

    let resp = server
        .post(&format!("/users/{owner}/tokens"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "token": { "purpose": "A purpose" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let id = body["id"].as_i64().expect("token id");

    let resp = server
        .get(&format!("/users/{owner}/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
