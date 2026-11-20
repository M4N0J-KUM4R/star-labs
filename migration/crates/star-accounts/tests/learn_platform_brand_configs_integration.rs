//! Integration tests for the Canvas learn platform and shared brand config
//! routes.
//!
//! Pinned behavior:
//! - the learn platform routes are public and answer the empty body of the
//!   unconfigured Learn Platform service;
//! - a shared brand config needs an account and the account admin right, and
//!   the body carries the name and the md5 of the brand config;
//! - the update route reads the `shared_brand_config` envelope;
//! - the destroy route answers the model attributes of the share.
//!
//! Fleet rule: dedicated users and accounts per test, every seeded row is
//! cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "lp-sbc-{}-{}-{}",
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
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_accounts::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
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
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role, workflow_state) \
         VALUES ($1, $2, 'admin', 'active') \
         ON CONFLICT (account_id, user_id) DO NOTHING",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account admin");
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM shared_brand_configs WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
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

#[tokio::test]
async fn the_learn_platform_routes_answer_the_unconfigured_service() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;

    // The routes are public.
    for path in [
        format!("/accounts/{account}/learn_platform/products"),
        format!("/accounts/{account}/learn_platform/products_categories"),
        format!("/accounts/{account}/learn_platform/products/7"),
        format!("/accounts/{account}/learn_platform/filters"),
        format!("/accounts/{account}/learn_platform/organizations/9/products"),
        format!("/accounts/{account}/learn_platform/custom_filters"),
    ] {
        let resp = server.get(&path).await;
        assert_eq!(resp.status_code(), 200, "path {path}");
        assert_eq!(resp.json::<Value>(), json!({}), "path {path}");
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_shared_brand_config_is_created_updated_and_removed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;

    // A caller without the account admin right may not share a theme.
    let resp = server
        .post(&format!("/accounts/{account}/shared_brand_configs"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "shared_brand_config": { "name": "Theme", "brand_config_md5": "abc" }
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The admin may.
    let resp = server
        .post(&format!("/accounts/{account}/shared_brand_configs"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "shared_brand_config": { "name": "Theme", "brand_config_md5": "abc" }
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let share: Value = resp.json();
    assert_eq!(share["name"], "Theme");
    assert_eq!(share["brand_config_md5"], "abc");
    assert_eq!(share["account_id"], account);
    let share_id = share["id"].as_i64().expect("share id");

    // The update route reads the same envelope.
    let resp = server
        .put(&format!(
            "/accounts/{account}/shared_brand_configs/{share_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "shared_brand_config": { "name": "New name", "brand_config_md5": "def" }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let share: Value = resp.json();
    assert_eq!(share["name"], "New name");
    assert_eq!(share["brand_config_md5"], "def");

    // The destroy route answers the model attributes.
    let resp = server
        .delete(&format!("/shared_brand_configs/{share_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let share: Value = resp.json();
    assert_eq!(share["id"], share_id);

    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM shared_brand_configs WHERE id = $1")
        .bind(share_id)
        .fetch_one(&pool)
        .await
        .expect("count the shares");
    assert_eq!(left, 0);

    cleanup(&pool, &prefix).await;
}
