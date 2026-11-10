//! Integration tests for the Canvas account grading schemes API.
//!
//! Exercises the real grading-standards router against a real Postgres:
//!   GET/POST /accounts/{accountId}/grading_schemes
//!   GET/PUT/DELETE /grading_schemes/{id}
//!
//! Spec: Canvas `GradingSchemesJsonController` parity — `data` is a
//! `[{name, value}]` array, ids serialize as strings, permissions expose only
//! `manage`, create/update/delete answer 200, validation failures are 400 and
//! deletes are soft (`workflow_state='deleted'`, subsequent reads 404).
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-grading-standards
//!
//! Test cases:
//!   1. Create an account scheme and read it back through detail_list and show.
//!   2. Update a scheme's title/data, then soft-delete it (200 `{}`).
//!   3. Scheme validation (missing 0% floor, duplicate values) returns 400,
//!      non-account-admins get 403, missing accounts 404.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gsch-tests-{}-{}",
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
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_grading_standards::router())
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
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

fn scheme_data() -> Value {
    json!([
        { "name": "A", "value": 0.90 },
        { "name": "B", "value": 0.80 },
        { "name": "C", "value": 0.70 },
        { "name": "D", "value": 0.60 },
        { "name": "F", "value": 0.0 }
    ])
}

#[tokio::test]
async fn create_list_and_show_account_grading_scheme() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_admin(&pool, account, admin).await;

    let payload = json!({
        "title": "My Scheme",
        "data": scheme_data(),
        "points_based": false,
        "scaling_factor": 1.0,
    });
    let resp = server
        .post(&format!("/accounts/{account}/grading_schemes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .json(&payload)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let body: Value = resp.json();
    let scheme_id = body["id"]
        .as_str()
        .expect("Canvas ids serialize as strings")
        .to_string();
    assert!(scheme_id.parse::<i64>().is_ok());
    assert_eq!(body["title"], "My Scheme");
    assert_eq!(body["context_type"], "Account");
    assert_eq!(body["context_id"], account);
    assert_eq!(
        body["context_name"],
        format!("{prefix}-Account"),
        "context_name comes from the owning account"
    );
    assert_eq!(body["data"], scheme_data(), "data round-trips in order");
    assert_eq!(body["permissions"], json!({ "manage": true }));
    assert_eq!(body["points_based"], false);
    assert_eq!(body["scaling_factor"], 1.0);
    assert_eq!(body["workflow_state"], "active");

    // detail_list returns the scheme inside the account collection.
    let resp = server
        .get(&format!("/accounts/{account}/grading_schemes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Vec<Value> = resp.json();
    assert!(list.iter().any(|scheme| scheme["id"] == scheme_id.as_str()));

    // show returns the same scheme.
    let resp = server
        .get(&format!("/grading_schemes/{scheme_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"], scheme_id.as_str());
    assert_eq!(shown["title"], "My Scheme");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_then_soft_delete_grading_scheme() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    seed_account_admin(&pool, account, admin).await;

    let resp = server
        .post(&format!("/accounts/{account}/grading_schemes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .json(&json!({
            "title": "Original",
            "data": [
                { "name": "A", "value": 0.90 },
                { "name": "F", "value": 0.0 }
            ],
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    let scheme_id = created["id"].as_str().expect("string id").to_string();

    let updated_data = json!([
        { "name": "Pass", "value": 0.75 },
        { "name": "Fail", "value": 0.0 }
    ]);
    let resp = server
        .put(&format!("/grading_schemes/{scheme_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .json(&json!({ "title": "Renamed", "data": updated_data }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["title"], "Renamed");
    assert_eq!(updated["data"], updated_data);

    // Canvas destroy is a soft delete that renders `{}` with 200.
    let resp = server
        .delete(&format!("/grading_schemes/{scheme_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({}));

    let resp = server
        .get(&format!("/grading_schemes/{scheme_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let (state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM grading_standards WHERE id = $1")
            .bind(scheme_id.parse::<i64>().expect("numeric id"))
            .fetch_one(&pool)
            .await
            .expect("fetch workflow_state");
    assert_eq!(state, "deleted");

    let resp = server
        .get(&format!("/accounts/{account}/grading_schemes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Vec<Value> = resp.json();
    assert!(list.iter().all(|scheme| scheme["id"] != scheme_id.as_str()));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grading_scheme_validation_and_authz() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;

    // Only account admins / global admins may create schemes.
    let resp = server
        .post(&format!("/accounts/{account}/grading_schemes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "title": "Nope", "data": scheme_data() }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Canvas `full_range_scheme`: the lowest grade must be 0%.
    let resp = server
        .post(&format!("/accounts/{account}/grading_schemes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "title": "No Floor",
            "data": [
                { "name": "A", "value": 0.90 },
                { "name": "B", "value": 0.60 }
            ],
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Canvas rejects duplicate lower bounds.
    let resp = server
        .post(&format!("/accounts/{account}/grading_schemes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "title": "Duplicates",
            "data": [
                { "name": "A", "value": 0.90 },
                { "name": "B", "value": 0.90 },
                { "name": "F", "value": 0.0 }
            ],
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Unknown account ids are 404.
    let resp = server
        .get(&format!(
            "/accounts/{}/grading_schemes",
            account + 1_000_000
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
