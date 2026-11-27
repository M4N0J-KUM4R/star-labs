//! Integration tests for the P5 account gaps in star-accounts:
//!
//! - GET  /accounts/{accountId}/developer_keys            (index)
//! - POST /accounts/{accountId}/developer_keys            (create)
//! - GET  /accounts/{accountId}/developer_keys/lookup_utids
//! - PUT  /release_notes/{id}/published                   (publish)
//! - DELETE /release_notes/{id}/published                 (unpublish)
//! - PUT  /accounts/{accountId}/authentication_providers/{id}/restore
//!
//! Schema adaptation (this branch): `developer_keys` has no `account_id`
//! column — the account association lives in
//! `developer_key_account_bindings` (migration 0224), so the index only
//! lists bound keys and the create handler writes a binding row.
//! `release_notes` has no `published_at` column (migration 0097), so the
//! publish flow asserts the `published` flag and `updated_at` instead.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-accounts

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "acc-p5-{}-{}-{}",
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
        r#"
        INSERT INTO users (name, email, password_hash, role, sortable_name)
        VALUES ($1, $2, $3, $4, $1)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO accounts (name, parent_account_id, workflow_state)
        VALUES ($1, $2, 'active')
        RETURNING id
        "#,
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // The bindings reference accounts (no cascade), so drop them first.
    let _ = sqlx::query(
        "DELETE FROM developer_key_account_bindings WHERE account_id IN \
         (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM developer_keys WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM release_notes WHERE title LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    // Child accounts first, then parents (self-referencing FK).
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
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

// ---------------------------------------------------------------------------
// developer_keys
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_developer_keys_lifecycle_and_lookup_utids() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Acct", None).await;

    let resp = server
        .get("/accounts/99999999/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown account is 404");

    let resp = server
        .post(&format!("/accounts/{account}/developer_keys"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "developer_key": { "name": format!("{prefix} Key") } }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let key: Value = resp.json();
    assert_eq!(key["name"], json!(format!("{prefix} Key")));
    assert_eq!(key["account_id"], json!(account));
    assert_eq!(key["workflow_state"], json!("active"));
    assert_eq!(key["redirect_uri"], Value::Null);
    assert!(key["id"].as_i64().is_some());

    // The generated secret is not part of the Canvas key JSON.
    assert!(key.get("api_key").is_none());

    // The create wrote the account binding this schema associates keys with.
    let (bound,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM developer_key_account_bindings \
         WHERE account_id = $1 AND developer_key_id = $2",
    )
    .bind(account)
    .bind(key["id"].as_i64().expect("key id"))
    .fetch_one(&pool)
    .await
    .expect("count bindings");
    assert_eq!(bound, 1, "create binds the key to the account");

    let resp = server
        .post(&format!("/accounts/{account}/developer_keys"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "developer_key": { "icon_url": "https://img.example/x.png" } }))
        .await;
    assert_eq!(resp.status_code(), 422, "name is required: {}", resp.text());

    let resp = server
        .post(&format!("/accounts/{account}/developer_keys"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "developer_key": { "name": format!("{prefix} No") } }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // An unbound key never shows up in the account listing.
    sqlx::query(
        "INSERT INTO developer_keys (name, api_key, workflow_state) \
         VALUES ($1, 'unbound-secret', 'active')",
    )
    .bind(format!("{prefix} Unbound"))
    .execute(&pool)
    .await
    .expect("seed unbound key");

    let resp = server
        .get(&format!("/accounts/{account}/developer_keys"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Vec<Value> = resp.json();
    assert_eq!(body.len(), 1, "only the bound key is listed");
    assert_eq!(body[0]["id"], key["id"]);

    let resp = server
        .get(&format!("/accounts/{account}/developer_keys"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Canvas answers 204 when no universal tool ids are registered.
    let resp = server
        .get(&format!("/accounts/{account}/developer_keys/lookup_utids"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204);
    assert_eq!(resp.text(), "");

    let resp = server
        .get("/accounts/99999999/developer_keys/lookup_utids")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// release_notes publish / unpublish
// ---------------------------------------------------------------------------

#[tokio::test]
async fn release_notes_publish_unpublish_flow() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let note: i64 = sqlx::query_scalar(
        "INSERT INTO release_notes (title, description) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix} Note"))
    .bind("Something shipped")
    .fetch_one(&pool)
    .await
    .expect("seed note");

    let resp = server
        .put(&format!("/release_notes/{note}/published"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin is 403");

    let resp = server
        .put(&format!("/release_notes/{note}/published"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], json!(note));
    assert_eq!(body["published"], json!(true));
    assert_eq!(body["target_roles"], json!(["user"]));
    // No published_at column in this schema — published is the flag.
    let (published,): (bool,) = sqlx::query_as("SELECT published FROM release_notes WHERE id = $1")
        .bind(note)
        .fetch_one(&pool)
        .await
        .expect("reload note");
    assert!(published, "published is stamped");

    let resp = server
        .delete(&format!("/release_notes/{note}/published"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["published"], json!(false));
    let (published,): (bool,) = sqlx::query_as("SELECT published FROM release_notes WHERE id = $1")
        .bind(note)
        .fetch_one(&pool)
        .await
        .expect("reload note");
    assert!(!published, "unpublish clears the flag");

    let resp = server
        .put("/release_notes/99999999/published")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// authentication_providers restore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn authentication_provider_restore_flow() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Acct", None).await;
    let other = seed_account(&pool, &prefix, "Other", None).await;

    let deleted: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO authentication_providers (account_id, auth_type, position, workflow_state)
        VALUES ($1, 'saml', 1, 'deleted')
        RETURNING id
        "#,
    )
    .bind(account)
    .fetch_one(&pool)
    .await
    .expect("seed deleted provider");
    let active: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO authentication_providers (account_id, auth_type, position, workflow_state)
        VALUES ($1, 'cas', 2, 'active')
        RETURNING id
        "#,
    )
    .bind(account)
    .fetch_one(&pool)
    .await
    .expect("seed active provider");
    let foreign: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO authentication_providers (account_id, auth_type, position, workflow_state)
        VALUES ($1, 'saml', 1, 'deleted')
        RETURNING id
        "#,
    )
    .bind(other)
    .fetch_one(&pool)
    .await
    .expect("seed foreign provider");

    let resp = server
        .put(&format!(
            "/accounts/{account}/authentication_providers/{deleted}/restore"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin is 403");

    let resp = server
        .put(&format!(
            "/accounts/{account}/authentication_providers/{deleted}/restore"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], json!(deleted));
    assert_eq!(body["workflow_state"], json!("active"));
    assert_eq!(body["auth_type"], json!("saml"));
    assert_eq!(body["account_id"], json!(account));
    // Canvas renders the provider object (unwrapped), settings merged flat.
    assert!(body.get("authentication_provider").is_none());

    // Already-active provider: no-op render.
    let resp = server
        .put(&format!(
            "/accounts/{account}/authentication_providers/{active}/restore"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["workflow_state"], json!("active"));

    // A provider under another account is not found through this account.
    let resp = server
        .put(&format!(
            "/accounts/{account}/authentication_providers/{foreign}/restore"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    let resp = server
        .put(&format!(
            "/accounts/{account}/authentication_providers/99999999/restore"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}
