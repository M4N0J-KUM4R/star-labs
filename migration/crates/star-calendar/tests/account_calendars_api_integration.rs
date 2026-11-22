//! Integration tests for the Canvas account calendar routes
//! (`AccountCalendarsApiController`).
//!
//! Pinned behaviour (the Ruby source of truth is
//! `canvas-src/app/controllers/account_calendars_api_controller.rb`):
//!
//! - `GET /account_calendars` answers `{"account_calendars": [...],
//!   "total_results": n}` — the wrapper belongs to the controller and
//!   `total_results` counts the whole (searched) set;
//! - `GET /account_calendars/{account_id}` answers the calendar of the account
//!   with the `Api::V1::AccountCalendar` fields, a missing account is a 404
//!   and a caller without the right is a 403;
//! - `PUT /account_calendars/{account_id}` reads `visible` and
//!   `auto_subscribe` from the request parameters (query string, form body or
//!   JSON body) and answers the updated calendar;
//! - `PUT /accounts/{account_id}/account_calendars` takes the JSON array of
//!   the accounts, answers `{"message": "Updated 2 accounts"}` and refuses the
//!   accounts outside of the subtree with a 403;
//! - `GET /accounts/{account_id}/account_calendars` answers a **bare array**
//!   with the `sub_account_count` of every entry;
//! - `GET /accounts/{account_id}/visible_calendars_count` answers
//!   `{"count": n}`.
//!
//! Fleet rule: dedicated users, accounts and memberships per test, every
//! seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.
//!
//! The test drives the crate router, so the crate has to mount
//! `account_calendars_api::router()` (the module of this port) — the routes
//! are registered below `/api/v1` by `star-http`.

use axum::body::Bytes;
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
        "acapi-{}-{}-{}",
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
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(account_id)
    .bind(user_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed account user");
}

async fn set_calendar(pool: &PgPool, account_id: i64, visible: bool, subscription: &str) {
    sqlx::query(
        "UPDATE accounts SET account_calendar_visible = $2, \
             account_calendar_subscription_type = $3 WHERE id = $1",
    )
    .bind(account_id)
    .bind(visible)
    .bind(subscription)
    .execute(pool)
    .await
    .expect("set the calendar");
}

async fn calendar_of(pool: &PgPool, account_id: i64) -> (bool, String) {
    let row: (bool, String) = sqlx::query_as(
        "SELECT account_calendar_visible, account_calendar_subscription_type \
         FROM accounts WHERE id = $1",
    )
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("read the calendar");
    row
}

async fn cleanup(pool: &PgPool, prefix: &str) {
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
async fn the_available_calendars_are_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let visible = seed_account(&pool, &prefix, "Visible").await;
    let hidden = seed_account(&pool, &prefix, "Hidden").await;
    let user = seed_user(&pool, &prefix, "User", "student").await;
    seed_account_user(&pool, visible, user, "student").await;
    seed_account_user(&pool, hidden, user, "student").await;
    set_calendar(&pool, visible, true, "manual").await;

    let resp = server
        .get("/account_calendars")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();

    // Canvas wraps the listing and counts the whole set.
    assert_eq!(body["total_results"].as_i64(), Some(1), "body: {body}");
    let calendars = body["account_calendars"].as_array().expect("calendars");
    assert_eq!(calendars.len(), 1, "body: {body}");
    let calendar = &calendars[0];
    assert_eq!(calendar["id"].as_i64(), Some(visible));
    assert_eq!(calendar["visible"], json!(true));
    assert_eq!(calendar["auto_subscribe"], json!(false));
    assert_eq!(calendar["type"], json!("account"));
    assert_eq!(
        calendar["asset_string"],
        json!(format!("account_{visible}"))
    );
    assert_eq!(
        calendar["calendar_event_url"],
        json!(format!("/accounts/{visible}/calendar_events/{{{{ id }}}}"))
    );
    assert_eq!(
        calendar["create_calendar_event_url"],
        json!(format!("/accounts/{visible}/calendar_events"))
    );
    assert_eq!(
        calendar["new_calendar_event_url"],
        json!(format!("/accounts/{visible}/calendar_events/new"))
    );
    // The student is no account admin, so it may not create the events.
    assert_eq!(calendar["can_create_calendar_events"], json!(false));
    // `sub_account_count` belongs to `all_calendars` only.
    assert!(calendar.get("sub_account_count").is_none(), "{calendar}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_visible_calendar_is_read() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    seed_account_user(&pool, account, admin, "admin").await;
    set_calendar(&pool, account, true, "auto").await;

    let resp = server
        .get(&format!("/account_calendars/{account}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(account));
    assert_eq!(body["visible"], json!(true));
    assert_eq!(body["auto_subscribe"], json!(true));
    assert_eq!(body["root_account_id"].as_i64(), Some(account));
    assert_eq!(body["parent_account_id"], Value::Null);

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
        .get("/account_calendars/999999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"][0]["message"],
        json!("The specified resource does not exist.")
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_caller_without_the_right_is_forbidden() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    set_calendar(&pool, account, true, "manual").await;

    let resp = server
        .get(&format!("/account_calendars/{account}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], json!("unauthorized"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_anonymous_caller_is_unauthorized() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let resp = server.get("/account_calendars").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_calendar_is_updated() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_user(&pool, account, admin, "admin").await;

    let resp = server
        .put(&format!("/account_calendars/{account}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .json(&json!({ "visible": true, "auto_subscribe": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["visible"], json!(true));
    assert_eq!(body["auto_subscribe"], json!(true));
    assert_eq!(
        calendar_of(&pool, account).await,
        (true, "auto".to_string())
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_form_encoded_update_is_accepted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_user(&pool, account, admin, "admin").await;
    set_calendar(&pool, account, true, "auto").await;

    // Canvas reads the parameters of the request, so `visible=false` hides the
    // calendar and `auto_subscribe=0` unsubscribes it.
    let resp = server
        .put(&format!("/account_calendars/{account}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .text("visible=false&auto_subscribe=0")
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["visible"], json!(false));
    assert_eq!(body["auto_subscribe"], json!(false));
    assert_eq!(
        calendar_of(&pool, account).await,
        (false, "manual".to_string())
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_bulk_update_changes_the_calendars() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let first = seed_sub_account(&pool, &prefix, "First", account).await;
    let second = seed_sub_account(&pool, &prefix, "Second", account).await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_user(&pool, account, admin, "admin").await;

    let payload = json!([
        { "id": first, "visible": true, "auto_subscribe": false },
        { "id": second, "visible": false, "auto_subscribe": true },
    ]);
    let resp = server
        .put(&format!("/accounts/{account}/account_calendars"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .bytes(Bytes::from(payload.to_string()))
        .content_type("application/json")
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], json!("Updated 2 accounts"));
    assert_eq!(
        calendar_of(&pool, first).await,
        (true, "manual".to_string())
    );
    assert_eq!(
        calendar_of(&pool, second).await,
        (false, "auto".to_string())
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_bulk_update_rejects_a_broken_body() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_user(&pool, account, admin, "admin").await;

    // Not an array of objects.
    let resp = server
        .put(&format!("/accounts/{account}/account_calendars"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .bytes(Bytes::from(json!({ "id": account }).to_string()))
        .content_type("application/json")
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["errors"], json!("Expected array of objects"));

    // An entry without the id.
    let resp = server
        .put(&format!("/accounts/{account}/account_calendars"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .bytes(Bytes::from(json!([{ "visible": true }]).to_string()))
        .content_type("application/json")
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["errors"], json!("Missing key(s)"));

    // The same id twice.
    let resp = server
        .put(&format!("/accounts/{account}/account_calendars"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .bytes(Bytes::from(
            json!([{ "id": account }, { "id": account }]).to_string(),
        ))
        .content_type("application/json")
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["errors"], json!("Duplicate IDs"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_bulk_update_refuses_a_foreign_account() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let foreign = seed_account(&pool, &prefix, "Foreign").await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_user(&pool, account, admin, "admin").await;

    let resp = server
        .put(&format!("/accounts/{account}/account_calendars"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .bytes(Bytes::from(
            json!([{ "id": foreign, "visible": true }]).to_string(),
        ))
        .content_type("application/json")
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn all_calendars_answers_a_bare_array() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let first = seed_sub_account(&pool, &prefix, "First", account).await;
    let second = seed_sub_account(&pool, &prefix, "Second", account).await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_user(&pool, account, admin, "admin").await;

    let resp = server
        .get(&format!("/accounts/{account}/account_calendars"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let calendars = body.as_array().expect("a bare array");
    assert_eq!(calendars.len(), 3, "body: {body}");
    // The account itself comes first, the sub-accounts follow by name.
    assert_eq!(calendars[0]["id"].as_i64(), Some(account));
    assert_eq!(calendars[1]["id"].as_i64(), Some(first));
    assert_eq!(calendars[2]["id"].as_i64(), Some(second));
    // `all_calendars` adds the number of the direct sub-accounts.
    assert_eq!(calendars[0]["sub_account_count"].as_i64(), Some(2));
    assert_eq!(calendars[1]["sub_account_count"].as_i64(), Some(0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn all_calendars_filters_and_searches() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let hidden = seed_sub_account(&pool, &prefix, "Hidden", account).await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_user(&pool, account, admin, "admin").await;
    set_calendar(&pool, account, true, "manual").await;
    set_calendar(&pool, hidden, false, "manual").await;

    // Only the visible calendars.
    let resp = server
        .get(&format!(
            "/accounts/{account}/account_calendars?filter=visible"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let calendars = body.as_array().expect("a bare array");
    assert_eq!(calendars.len(), 1, "body: {body}");
    assert_eq!(calendars[0]["id"].as_i64(), Some(account));

    // Only the hidden ones.
    let resp = server
        .get(&format!(
            "/accounts/{account}/account_calendars?filter=hidden"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let calendars = body.as_array().expect("a bare array");
    assert_eq!(calendars.len(), 1, "body: {body}");
    assert_eq!(calendars[0]["id"].as_i64(), Some(hidden));

    // An unknown filter.
    let resp = server
        .get(&format!(
            "/accounts/{account}/account_calendars?filter=maybe"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"],
        json!("Expected filter param to be one of: visible, hidden")
    );

    // A search term of one character is too short.
    let resp = server
        .get(&format!(
            "/accounts/{account}/account_calendars?search_term=H"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["errors"][0]["field"], json!("search_term"));
    assert_eq!(body["errors"][0]["code"], json!("invalid"));
    assert_eq!(
        body["errors"][0]["message"],
        json!("2 or more characters is required")
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_visible_calendars_are_counted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let visible = seed_sub_account(&pool, &prefix, "Visible", account).await;
    let hidden = seed_sub_account(&pool, &prefix, "Hidden", account).await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_user(&pool, account, admin, "admin").await;
    set_calendar(&pool, account, true, "manual").await;
    set_calendar(&pool, visible, true, "manual").await;
    set_calendar(&pool, hidden, false, "manual").await;

    let resp = server
        .get(&format!("/accounts/{account}/visible_calendars_count"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["count"].as_i64(), Some(2), "body: {body}");

    cleanup(&pool, &prefix).await;
}
