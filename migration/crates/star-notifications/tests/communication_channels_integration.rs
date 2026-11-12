//! Integration tests for star-notifications communication-channels routes.
//!
//! Canvas API v1 parity contract:
//! - GET    /users/{userId}/communication_channels          — list channels (self or admin)
//! - POST   /users/{userId}/communication_channels          — create channel (self only)
//! - DELETE /users/{userId}/communication_channels/{id}     — retire channel (self only)
//! - GET    /accounts/{accountId}/bounced_communication_channels — admin-only bounce report
//!
//! These tests pin actual handler behavior without relying on implementation
//! internals. Tests are skipped cleanly when DATABASE_URL is not set.

#![allow(dead_code)]

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "notifcc_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer. Returns None when
/// DATABASE_URL is unset so callers can skip cleanly.
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
        TestServer::new(star_notifications::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

/// Create a user directly in the DB. Returns user id.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed_user")
}

/// Create a communication channel directly in the DB. Returns channel id.
async fn seed_channel(
    pool: &PgPool,
    user_id: i64,
    address: &str,
    channel_type: &str,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO communication_channels (user_id, path, path_type, workflow_state)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(address)
    .bind(channel_type)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed_channel")
}

/// Mint a valid bearer token for the given user.
fn auth_header(_pool: &PgPool, user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

// ---------------------------------------------------------------------------
// Tests — GET /users/{userId}/communication_channels
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_user_channels_requires_auth() {
    let Some((server, _, _)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.get("/api/v1/users/1/communication_channels").await;
    assert_eq!(resp.status_code(), 401, "unauthenticated request");
}

#[tokio::test]
async fn list_user_channels_self_returns_own_channels() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "list-self-user", "student").await;
    seed_channel(&pool, user_id, "self-test@example.edu", "email", "active").await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "self returns 200: {:#?}",
        resp.json::<Value>()
    );
    let body: Value = resp.json::<Value>();
    assert!(
        body.is_array(),
        "response must be an array of CommunicationChannel"
    );
    // Channel must appear in list
    let found = body
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["address"].as_str() == Some("self-test@example.edu"));
    assert!(found, "created channel must be in list response");

    // Clean up
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
}

#[tokio::test]
async fn list_user_channels_admin_can_read_other_user() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let admin_id = seed_user(&pool, &prefix, "list-admin", "admin").await;
    let target_id = seed_user(&pool, &prefix, "list-target", "student").await;
    seed_channel(
        &pool,
        target_id,
        "other-channel@example.edu",
        "email",
        "active",
    )
    .await;

    let admin_token = auth_header(&pool, admin_id, "admin");
    let resp = server
        .get(&format!("/api/v1/users/{target_id}/communication_channels"))
        .add_header(axum::http::header::AUTHORIZATION, admin_token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "admin reading other user: {:#?}",
        resp.json::<Value>()
    );
    let body: Value = resp.json::<Value>();
    assert!(body.is_array(), "response must be an array");
    let found = body
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["address"].as_str() == Some("other-channel@example.edu"));
    assert!(found, "target user's channel must be visible to admin");

    // Clean up
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1 OR user_id = $2")
        .bind(admin_id)
        .bind(target_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1 OR id = $2")
        .bind(admin_id)
        .bind(target_id)
        .execute(&pool)
        .await
        .expect("teardown");
}

#[tokio::test]
async fn list_user_channels_student_cannot_read_other_user() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let student_a = seed_user(&pool, &prefix, "list-student-a", "student").await;
    let student_b = seed_user(&pool, &prefix, "list-student-b", "student").await;

    let token_a = auth_header(&pool, student_a, "student");
    let resp = server
        .get(&format!("/api/v1/users/{student_b}/communication_channels"))
        .add_header(axum::http::header::AUTHORIZATION, token_a.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        403,
        "student reading other user must be 403: {:#?}",
        resp.json::<Value>()
    );

    // Clean up
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1 OR user_id = $2")
        .bind(student_a)
        .bind(student_b)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1 OR id = $2")
        .bind(student_a)
        .bind(student_b)
        .execute(&pool)
        .await
        .expect("teardown");
}

#[tokio::test]
async fn list_user_channels_unknown_user_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "list-unk-user", "student").await;
    let token = auth_header(&pool, user_id, "student");

    // 99999999 is extremely unlikely to exist
    let resp = server
        .get("/api/v1/users/99999999/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(resp.status_code(), 404, "unknown user must be 404");

    // Clean up
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
}

#[tokio::test]
async fn list_user_channels_excludes_retired() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "list-retired", "student").await;
    seed_channel(&pool, user_id, "active@example.edu", "email", "active").await;
    seed_channel(&pool, user_id, "retired@example.edu", "email", "retired").await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json::<Value>();
    let addresses: Vec<_> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["address"].as_str().unwrap())
        .collect();
    assert!(
        addresses.contains(&"active@example.edu"),
        "active channel must be present"
    );
    assert!(
        !addresses.contains(&"retired@example.edu"),
        "retired channel must NOT be in list response"
    );

    // Clean up
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
}

// ---------------------------------------------------------------------------
// Tests — POST /users/{userId}/communication_channels
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_channel_email_ok() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "create-email", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let payload = json!({
        "address": "newemail@example.edu",
        "type": "email"
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "create email channel: {:#?}",
        resp.json::<Value>()
    );
    let body: Value = resp.json::<Value>();
    assert_eq!(body["address"], "newemail@example.edu");
    assert_eq!(body["type"], "email");
    assert_eq!(
        body["workflow_state"], "unconfirmed",
        "non-admin must start unconfirmed"
    );
    assert!(body["id"].is_number(), "id must be returned");

    // Clean up
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
}

#[tokio::test]
async fn create_channel_push_requires_token() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "create-push", "student").await;
    let token = auth_header(&pool, user_id, "student");

    // Push without token is a validation error
    let payload = json!({
        "address": "ignored",
        "type": "push"
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(
        resp.status_code(),
        400,
        "push without token: {:#?}",
        resp.json::<Value>()
    );
}

#[tokio::test]
async fn create_channel_push_with_token_ok() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "create-push-token", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let payload = json!({
        "type": "push",
        "token": "device-token-abc123"
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "push with token: {:#?}",
        resp.json::<Value>()
    );
    let body: Value = resp.json::<Value>();
    assert_eq!(body["type"], "push");
    assert_eq!(body["workflow_state"], "unconfirmed");
}

#[tokio::test]
async fn create_channel_duplicate_returns_400() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "create-dup", "student").await;
    seed_channel(&pool, user_id, "dup@example.edu", "email", "active").await;
    let token = auth_header(&pool, user_id, "student");

    let payload = json!({
        "address": "dup@example.edu",
        "type": "email"
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(
        resp.status_code(),
        400,
        "duplicate channel must be 400: {:#?}",
        resp.json::<Value>()
    );
}

#[tokio::test]
async fn create_channel_invalid_email_returns_400() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "create-bad-email", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let payload = json!({
        "address": "not-an-email",
        "type": "email"
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(
        resp.status_code(),
        400,
        "invalid email: {:#?}",
        resp.json::<Value>()
    );
}

#[tokio::test]
async fn create_channel_missing_type_returns_400() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "create-no-type", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let payload = json!({
        "address": "addr@example.edu"
        // no "type"
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(
        resp.status_code(),
        400,
        "missing type: {:#?}",
        resp.json::<Value>()
    );
}

#[tokio::test]
async fn create_channel_invalid_type_returns_400() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "create-bad-type", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let payload = json!({
        "address": "addr@example.edu",
        "type": "carrier_pigeon"
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(
        resp.status_code(),
        400,
        "invalid type: {:#?}",
        resp.json::<Value>()
    );
}

#[tokio::test]
async fn create_channel_student_cannot_skip_confirmation() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "create-no-skip", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let payload = json!({
        "address": "skip@example.edu",
        "type": "email",
        "skip_confirmation": true
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json::<Value>();
    assert_eq!(
        body["workflow_state"], "unconfirmed",
        "student with skip_confirmation=true must still be unconfirmed"
    );

    // Clean up
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
}

#[tokio::test]
async fn create_channel_admin_can_skip_confirmation() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let admin_id = seed_user(&pool, &prefix, "create-admin-skip", "admin").await;
    let token = auth_header(&pool, admin_id, "admin");

    let payload = json!({
        "address": "admin-skip@example.edu",
        "type": "email",
        "skip_confirmation": true
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json::<Value>();
    assert_eq!(
        body["workflow_state"], "active",
        "admin with skip_confirmation=true must be active"
    );

    // Clean up
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1")
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("teardown");
}

#[tokio::test]
async fn create_channel_nested_communication_channel_payload() {
    // Canvas sends the payload nested: communication_channel: { address, type }
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "create-nested", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let payload = json!({
        "communication_channel": {
            "address": "nested@example.edu",
            "type": "email"
        }
    });
    let resp = server
        .post("/api/v1/users/self/communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload)
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "nested payload: {:#?}",
        resp.json::<Value>()
    );
    let body: Value = resp.json::<Value>();
    assert_eq!(body["address"], "nested@example.edu");
    assert_eq!(body["type"], "email");

    // Clean up
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown");
}

// ---------------------------------------------------------------------------
// Tests — DELETE /users/{userId}/communication_channels/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_channel_owner_ok() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "delete-owner", "student").await;
    let channel_id = seed_channel(&pool, user_id, "delete@example.edu", "email", "active").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .delete(&format!(
            "/api/v1/users/self/communication_channels/{channel_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "owner delete must return 200 with channel JSON: {:#?}",
        resp.json::<Value>()
    );
    let body: Value = resp.json::<Value>();
    assert_eq!(
        body["workflow_state"], "retired",
        "deleted channel must be retired"
    );
}

#[tokio::test]
async fn delete_channel_nonexistent_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "delete-404", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .delete("/api/v1/users/self/communication_channels/999999")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        404,
        "delete nonexistent: {:#?}",
        resp.json::<Value>()
    );
}

#[tokio::test]
async fn delete_channel_other_user_returns_403() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_a = seed_user(&pool, &prefix, "delete-other-a", "student").await;
    let user_b = seed_user(&pool, &prefix, "delete-other-b", "student").await;
    let channel_id =
        seed_channel(&pool, user_b, "other-users@example.edu", "email", "active").await;

    let token_a = auth_header(&pool, user_a, "student");
    let resp = server
        .delete(&format!(
            "/api/v1/users/{user_b}/communication_channels/{channel_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token_a.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        403,
        "deleting another user's channel must be 403: {:#?}",
        resp.json::<Value>()
    );
}

#[tokio::test]
async fn delete_channel_already_retired_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let user_id = seed_user(&pool, &prefix, "delete-retired", "student").await;
    let channel_id = seed_channel(&pool, user_id, "retired@example.edu", "email", "retired").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .delete(&format!(
            "/api/v1/users/self/communication_channels/{channel_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        404,
        "deleting already-retired channel must be 404: {:#?}",
        resp.json::<Value>()
    );
}

// ---------------------------------------------------------------------------
// Tests — GET /accounts/{accountId}/bounced_communication_channels
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bounced_channels_requires_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let student_id = seed_user(&pool, &prefix, "bounce-student", "student").await;
    let token = auth_header(&pool, student_id, "student");

    let resp = server
        .get("/api/v1/accounts/1/bounced_communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        403,
        "non-admin must get 403: {:#?}",
        resp.json::<Value>()
    );
}

#[tokio::test]
async fn bounced_channels_unknown_account_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let admin_id = seed_user(&pool, &prefix, "bounce-unk-acct", "admin").await;
    let token = auth_header(&pool, admin_id, "admin");

    // Create the admin user with a real account association first
    // (seed_user doesn't set account_id, so the account FK won't match — we just need to
    // verify a non-existent account returns 404)
    let resp = server
        .get("/api/v1/accounts/999999/bounced_communication_channels")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        404,
        "unknown account must be 404: {:#?}",
        resp.json::<Value>()
    );
}

#[tokio::test]
async fn bounced_channels_returns_envelope_with_channels_key() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    // Create an account, admin user associated with it, and a bounced channel
    let account_id: i64 =
        sqlx::query_scalar("INSERT INTO accounts (name, domain) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix}-bounce-acct"))
            .bind(format!("{prefix}-bounce.test.local"))
            .fetch_one(&pool)
            .await
            .expect("seed account");

    let admin_id = seed_user(&pool, &prefix, "bounce-acct-admin", "admin").await;
    sqlx::query("UPDATE users SET account_id = $1 WHERE id = $2")
        .bind(account_id)
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("link user to account");

    // Bounced channel for a user in this account
    let user_id = seed_user(&pool, &prefix, "bounce-channel-user", "student").await;
    sqlx::query("UPDATE users SET account_id = $1 WHERE id = $2")
        .bind(account_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("link user to account");

    let channel_id = seed_channel(&pool, user_id, "bounced@example.edu", "email", "active").await;
    sqlx::query("UPDATE communication_channels SET bounce_count = 3 WHERE id = $1")
        .bind(channel_id)
        .execute(&pool)
        .await
        .expect("set bounce_count");

    let token = auth_header(&pool, admin_id, "admin");
    let resp = server
        .get(&format!(
            "/api/v1/accounts/{account_id}/bounced_communication_channels"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "bounced channels: {:#?}",
        resp.json::<Value>()
    );
    let body: Value = resp.json::<Value>();
    assert!(
        body.as_object()
            .unwrap()
            .contains_key("communication_channels"),
        "response must have 'communication_channels' key (Canvas envelope)"
    );
    let channels = &body["communication_channels"];
    assert!(
        channels.is_array(),
        "communication_channels must be an array"
    );
    assert_eq!(
        channels.as_array().unwrap().len(),
        1,
        "must return the bounced channel"
    );
    assert_eq!(
        channels[0]["address"].as_str(),
        Some("bounced@example.edu"),
        "bounced channel address must match"
    );
    assert_eq!(
        channels[0]["bounce_count"].as_i64(),
        Some(3),
        "bounce_count must be surfaced"
    );

    // Teardown
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1 OR user_id = $2")
        .bind(user_id)
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1 OR id = $2")
        .bind(user_id)
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(account_id)
        .execute(&pool)
        .await
        .expect("teardown");
}

#[tokio::test]
async fn bounced_channels_excludes_retired_channels() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = prefix;

    let account_id: i64 =
        sqlx::query_scalar("INSERT INTO accounts (name, domain) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix}-bounce-retired-acct"))
            .bind(format!("{prefix}-bounce-retired.test.local"))
            .fetch_one(&pool)
            .await
            .expect("seed account");

    let admin_id = seed_user(&pool, &prefix, "bounce-retired-admin", "admin").await;
    sqlx::query("UPDATE users SET account_id = $1 WHERE id = $2")
        .bind(account_id)
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("link user to account");

    let user_id = seed_user(&pool, &prefix, "bounce-retired-user", "student").await;
    sqlx::query("UPDATE users SET account_id = $1 WHERE id = $2")
        .bind(account_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("link user to account");

    // Active bounced channel
    let active_id = seed_channel(
        &pool,
        user_id,
        "active-bounce@example.edu",
        "email",
        "active",
    )
    .await;
    sqlx::query("UPDATE communication_channels SET bounce_count = 2 WHERE id = $1")
        .bind(active_id)
        .execute(&pool)
        .await
        .expect("set bounce_count");

    // Retired bounced channel (should be excluded)
    seed_channel(
        &pool,
        user_id,
        "retired-bounce@example.edu",
        "email",
        "retired",
    )
    .await;

    let token = auth_header(&pool, admin_id, "admin");
    let resp = server
        .get(&format!(
            "/api/v1/accounts/{account_id}/bounced_communication_channels"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json::<Value>();
    let channels = &body["communication_channels"];
    let addresses: Vec<_> = channels
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["address"].as_str().unwrap())
        .collect();
    assert!(
        addresses.contains(&"active-bounce@example.edu"),
        "active bounced channel must be present"
    );
    assert!(
        !addresses.contains(&"retired-bounce@example.edu"),
        "retired bounced channel must NOT appear"
    );

    // Teardown
    sqlx::query("DELETE FROM communication_channels WHERE user_id = $1 OR user_id = $2")
        .bind(user_id)
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM users WHERE id = $1 OR id = $2")
        .bind(user_id)
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("teardown");
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(account_id)
        .execute(&pool)
        .await
        .expect("teardown");
}
