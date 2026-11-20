//! Integration tests for content_shares — Canvas `content_shares_controller` parity.
//!
//! Routes covered:
//! - POST /users/{userId}/content_shares                — create
//! - GET  /users/{userId}/content_shares/sent           — list sent
//! - GET  /users/{userId}/content_shares/received       — list received
//! - GET  /users/{userId}/content_shares/unread_count   — unread count
//! - GET  /users/{userId}/content_shares/{id}            — show
//! - DELETE /users/{userId}/content_shares/{id}         — destroy
//! - POST /users/{userId}/content_shares/{id}/add_users  — add_users
//! - PUT  /users/{userId}/content_shares/{id}            — update (read_state)
//!
//! Conventions (mirrors content_integration.rs):
//! - Skipped cleanly when DATABASE_URL is not set.
//! - Run with: DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test cargo test -p star-content

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
        "content-shares-tests-{}-{}",
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
        jwt_secret: JWT_SECRET.to_string(),
    };

    let server =
        TestServer::new(star_content::router().with_state(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id"#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM content_shares WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1) OR sender_id IN (SELECT id FROM users WHERE email LIKE $1)")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM content_exports WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// -----------------------------------------------------------------------------
// CS-01: POST /users/{userId}/content_shares — create
// -----------------------------------------------------------------------------

#[tokio::test]
async fn content_shares_create_happy() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "student").await;

    let resp = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let share: Value = resp.json();
    assert_eq!(share["content_type"], "assignment");
    assert_eq!(share["read_state"], "read");
    assert_eq!(share["user_id"], alice);
    assert!(share["id"].is_number());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_create_requires_self() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceShare", "student").await;
    let bob = seed_user(&pool, &prefix, "BobShare", "student").await;

    // Users can only create shares for themselves.
    let resp = server
        .post(&format!("/users/{bob}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_create_validates_content_type() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceVT", "student").await;

    let resp = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "invalid_type",
            "content_id": 1
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body["message"].as_str().unwrap().contains("Content type not allowed"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_create_requires_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceRF", "student").await;

    let resp = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "Content type and id required");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_create_rejects_missing_content_id() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceRCI", "student").await;

    // Only content_type given, no content_id.
    let resp = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "content_type": "assignment" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------------
// CS-02: GET /users/{userId}/content_shares/sent — list sent
// CS-03: GET /users/{userId}/content_shares/received — list received
// -----------------------------------------------------------------------------

#[tokio::test]
async fn content_shares_sent_list() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceSent", "student").await;

    // Create a share so sent list is non-empty.
    let _ = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;

    let resp = server
        .get(&format!("/users/{alice}/content_shares/sent"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert!(list.is_array(), "sent list must be array: {}", list);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_received_list() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceRec", "student").await;

    let resp = server
        .get(&format!("/users/{alice}/content_shares/received"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert!(list.is_array(), "received list must be array: {}", list);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_sent_requires_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceSentAuth", "student").await;

    let resp = server.get(&format!("/users/{alice}/content_shares/sent")).await;
    assert_eq!(resp.status_code(), 401, "anonymous must be 401");

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------------
// CS-04: GET /users/{userId}/content_shares/unread_count
// -----------------------------------------------------------------------------

#[tokio::test]
async fn content_shares_unread_count() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceUC", "student").await;

    let resp = server
        .get(&format!("/users/{alice}/content_shares/unread_count"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body.get("unread_count").is_some(), "unread_count field: {}", body);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_unread_count_requires_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceUCAuth", "student").await;

    let resp = server.get(&format!("/users/{alice}/content_shares/unread_count")).await;
    assert_eq!(resp.status_code(), 401, "anonymous must be 401");

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------------
// CS-05: GET /users/{userId}/content_shares/{id} — show
// -----------------------------------------------------------------------------

#[tokio::test]
async fn content_shares_show() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceShow", "student").await;

    // Create a share first.
    let create = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // GET /users/{userId}/content_shares/{id}
    let resp = server
        .get(&format!("/users/{alice}/content_shares/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let detail: Value = resp.json();
    assert_eq!(detail["id"], id);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_show_404_unknown() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceShow404", "student").await;

    let resp = server
        .get(&format!("/users/{alice}/content_shares/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_show_requires_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceShowAuth", "student").await;

    let resp = server.get(&format!("/users/{alice}/content_shares/1")).await;
    assert_eq!(resp.status_code(), 401, "anonymous must be 401");

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------------
// CS-06: DELETE /users/{userId}/content_shares/{id} — destroy
// -----------------------------------------------------------------------------

#[tokio::test]
async fn content_shares_destroy() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceDel", "student").await;

    // Create a share.
    let create = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Delete it.
    let resp = server
        .delete(&format!("/users/{alice}/content_shares/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "content share deleted");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_destroy_requires_owner() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceDRO", "student").await;
    let bob = seed_user(&pool, &prefix, "BobDRO", "student").await;

    // Bob creates a share for himself.
    let create = server
        .post(&format!("/users/{bob}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Alice (not the owner) cannot delete Bob's share.
    let resp = server
        .delete(&format!("/users/{bob}/content_shares/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------------
// CS-07: POST /users/{userId}/content_shares/{id}/add_users
// -----------------------------------------------------------------------------

#[tokio::test]
async fn content_shares_add_users() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceAdd", "student").await;

    // Create a share.
    let create = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Add more receivers.
    let resp = server
        .post(&format!("/users/{alice}/content_shares/{id}/add_users"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "receiver_ids": [alice] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["id"], id);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_add_users_requires_owner() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceAddRO", "student").await;
    let bob = seed_user(&pool, &prefix, "BobAddRO", "student").await;

    // Bob creates a share.
    let create = server
        .post(&format!("/users/{bob}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Alice cannot add users to Bob's share.
    let resp = server
        .post(&format!("/users/{bob}/content_shares/{id}/add_users"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "receiver_ids": [alice] }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_add_users_empty_receiver_ids() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceAddEmpty", "student").await;

    // Create a share.
    let create = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Empty receiver_ids — 400.
    let resp = server
        .post(&format!("/users/{alice}/content_shares/{id}/add_users"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "receiver_ids": [] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "No valid receiving users found");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_add_users_nonexistent_receiver() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceAddNR", "student").await;

    // Create a share.
    let create = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Non-existent receiver id — 400.
    let resp = server
        .post(&format!("/users/{alice}/content_shares/{id}/add_users"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "receiver_ids": [999999999] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "No valid receiving users found");

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------------
// CS-08: PUT /users/{userId}/content_shares/{id} — update (read_state)
// -----------------------------------------------------------------------------

#[tokio::test]
async fn content_shares_update_read_state() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceUpd", "student").await;

    // Create a share.
    let create = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Update read_state to unread.
    let resp = server
        .put(&format!("/users/{alice}/content_shares/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "read_state": "unread" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["read_state"], "unread");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_update_read_state_valid() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceUpdValid", "student").await;

    // Create a share.
    let create = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Valid read_state values.
    for state in ["read", "unread"] {
        let resp = server
            .put(&format!("/users/{alice}/content_shares/{id}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
            .json(&json!({ "read_state": state }))
            .await;
        assert_eq!(resp.status_code(), 200, "state={state}; body: {}", resp.text());
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_update_invalid_read_state() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceUpdInv", "student").await;

    // Create a share.
    let create = server
        .post(&format!("/users/{alice}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Invalid read_state — 400.
    let resp = server
        .put(&format!("/users/{alice}/content_shares/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "read_state": "invalid" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_shares_update_requires_owner() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "AliceUpdRO", "student").await;
    let bob = seed_user(&pool, &prefix, "BobUpdRO", "student").await;

    // Bob creates a share.
    let create = server
        .post(&format!("/users/{bob}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": 1
        }))
        .await;
    assert_eq!(create.status_code(), 201);
    let share: Value = create.json();
    let id = share["id"].as_i64().expect("share id");

    // Alice cannot update Bob's share.
    let resp = server
        .put(&format!("/users/{bob}/content_shares/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "read_state": "read" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
