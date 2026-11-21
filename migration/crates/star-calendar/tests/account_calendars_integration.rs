//! Account calendars (Canvas `AccountCalendarsApiController`) integration
//! tests.
//!
//! Covers the Canvas routes:
//! - `GET /api/v1/account_calendars`
//! - `GET|PUT /api/v1/account_calendars/:account_id`
//! - `GET|PUT /api/v1/accounts/:account_id/account_calendars`
//! - `GET /api/v1/accounts/:account_id/visible_calendars_count`
//!
//! Fleet rule: dedicated users, accounts and calendars per test, every
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
        "ac-{}-{}-{}",
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
    let server =
        TestServer::new(star_calendar::router().with_state(state)).expect("build TestServer");
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

async fn seed_sub_account(pool: &PgPool, prefix: &str, name: &str, parent_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id, workflow_state) \
         VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .expect("seed sub account")
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

/// Make the calendar of an account visible.
async fn make_visible(pool: &PgPool, account_id: i64) {
    sqlx::query("UPDATE accounts SET account_calendar_visible = true WHERE id = $1")
        .bind(account_id)
        .execute(pool)
        .await
        .expect("make the calendar visible");
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query(
        "DELETE FROM account_users WHERE account_id IN \
             (SELECT id FROM accounts WHERE name LIKE $1)",
    )
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
async fn the_available_calendars_are_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let hidden = seed_sub_account(&pool, &prefix, "Hidden", account).await;
    let visible = seed_sub_account(&pool, &prefix, "Visible", account).await;
    let user = seed_user(&pool, &prefix, "User", "student").await;
    seed_account_user(&pool, hidden, user, "student").await;
    seed_account_user(&pool, visible, user, "student").await;
    make_visible(&pool, visible).await;

    let resp = server
        .get("/account_calendars")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let calendars = body["account_calendars"].as_array().expect("calendars");
    assert_eq!(calendars.len(), 1, "body: {body}");
    assert_eq!(calendars[0]["id"].as_i64(), Some(visible));
    assert_eq!(calendars[0]["visible"], json!(true));
    assert_eq!(calendars[0]["auto_subscribe"], json!(false));
    assert_eq!(calendars[0]["type"], "account");
    assert_eq!(body["total_results"].as_i64(), Some(1));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_calendar_is_read_and_updated() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    seed_account_user(&pool, account, admin, "admin").await;

    let resp = server
        .get(&format!("/account_calendars/{account}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(account));

    let resp = server
        .put(&format!("/account_calendars/{account}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "visible": true, "auto_subscribe": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["visible"], json!(true));
    assert_eq!(body["auto_subscribe"], json!(true));

    // The visible-calendar count of the account.
    let resp = server
        .get(&format!("/accounts/{account}/visible_calendars_count"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["visible_calendars_count"].as_i64(), Some(1));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_may_not_change_a_calendar() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_account_user(&pool, account, student, "student").await;

    let resp = server
        .put(&format!("/account_calendars/{account}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "visible": true }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_missing_account_is_a_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .get("/account_calendars/999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
