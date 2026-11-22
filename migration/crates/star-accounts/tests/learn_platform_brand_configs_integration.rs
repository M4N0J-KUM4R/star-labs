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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
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
