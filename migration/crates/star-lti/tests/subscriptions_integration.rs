//! LTI webhook subscriptions (Canvas `Lti::SubscriptionsApiController`)
//! integration tests.
//!
//! Covers the Canvas routes:
//! - `POST   /api/lti/subscriptions`
//! - `GET    /api/lti/subscriptions`
//! - `GET    /api/lti/subscriptions/:id`
//! - `PUT    /api/lti/subscriptions/:id`
//! - `DELETE /api/lti/subscriptions/:id`
//!
//! The routes are authenticated with an LTI 2.0 access token that names the
//! tool proxy, exactly like the other LTI Advantage routes.
//!
//! Fleet rule: dedicated users, accounts and tool proxies per test, every
//! seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";
const SUBSCRIPTION_SCOPE: &str = "https://purl.imsglobal.org/spec/lti-ags/scope/subscription";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "ls-{}-{}-{}",
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
    let app = star_lti::lti_advantage_router().with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

/// An LTI 2.0 access token with the given scopes.
fn access_token(guid: &str, scopes: &str) -> String {
    let claims = json!({
        "sub": guid,
        "scopes": scopes,
        "exp": (chrono::Utc::now().timestamp() + 3600),
        "iss": "canvas",
    });
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("mint token")
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

/// A tool proxy installed in an account, with the webhook capabilities.
async fn seed_tool_proxy(pool: &PgPool, prefix: &str, account_id: i64) -> (i64, String) {
    let guid = format!("{prefix}-guid");
    let raw = json!({
        "name": "Plagiarism Tool",
        "description": "A tool",
        "launch_url": "https://tool.test/launch",
        "domain": "tool.test",
        "enabled_capabilities": [
            "https://purl.imsglobal.org/spec/lti-ags/scope/webhook_subscription.all"
        ],
    });
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO lti_tool_proxies (guid, context_type, context_id, raw_data, workflow_state) \
         VALUES ($1, 'Account', $2, $3, 'active') RETURNING id",
    )
    .bind(&guid)
    .bind(account_id)
    .bind(raw)
    .fetch_one(pool)
    .await
    .expect("seed tool proxy");
    (id, guid)
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query(
        "DELETE FROM lti_data_service_subscriptions \
         WHERE owner_type = 'tool_proxy' AND owner_id IN \
             (SELECT id::text FROM lti_tool_proxies WHERE guid LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query("DELETE FROM lti_tool_proxies WHERE guid LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
}

#[tokio::test]
async fn a_subscription_is_created_read_updated_and_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let (_proxy_id, guid) = seed_tool_proxy(&pool, &prefix, account).await;
    let token = access_token(&guid, SUBSCRIPTION_SCOPE);

    let resp = server
        .post("/api/lti/subscriptions")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&json!({
            "subscription": {
                "ContextId": account.to_string(),
                "ContextType": "root_account",
                "EventTypes": ["plagiarism_submission_created"],
                "Format": "live-event",
                "TransportMetadata": { "Url": "sqs.example" },
                "TransportType": "sqs"
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["ContextType"], "root_account");
    assert_eq!(body["ContextId"], account.to_string());
    assert_eq!(body["TransportType"], "sqs");
    let id = body["Id"].as_str().expect("subscription id").to_string();

    // List them.
    let resp = server
        .get("/api/lti/subscriptions")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let list = body.as_array().expect("subscriptions");
    assert_eq!(list.len(), 1, "body: {body}");

    // Show one.
    let resp = server
        .get(&format!("/api/lti/subscriptions/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["Id"], id);

    // Update it.
    let resp = server
        .put(&format!("/api/lti/subscriptions/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&json!({
            "subscription": {
                "ContextId": account.to_string(),
                "ContextType": "root_account",
                "EventTypes": ["plagiarism_submission_created"],
                "Format": "caliper",
                "TransportMetadata": { "Url": "https.example" },
                "TransportType": "https"
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["Format"], "caliper");
    assert_eq!(body["TransportType"], "https");

    // Delete it.
    let resp = server
        .delete(&format!("/api/lti/subscriptions/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_session_token_is_not_accepted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let (_proxy_id, guid) = seed_tool_proxy(&pool, &prefix, account).await;

    // A token without the subscription scope.
    let token = access_token(
        &guid,
        "https://purl.imsglobal.org/spec/lti-ags/scope/lineitem",
    );
    let resp = server
        .get("/api/lti/subscriptions")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // No token at all.
    let resp = server.get("/api/lti/subscriptions").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_unknown_context_type_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let (_proxy_id, guid) = seed_tool_proxy(&pool, &prefix, account).await;
    let token = access_token(&guid, SUBSCRIPTION_SCOPE);

    let resp = server
        .post("/api/lti/subscriptions")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .json(&json!({
            "subscription": {
                "ContextId": account.to_string(),
                "ContextType": "not_a_context",
                "EventTypes": ["plagiarism_submission_created"],
                "Format": "live-event",
                "TransportMetadata": { "Url": "sqs.example" },
                "TransportType": "sqs"
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "Invalid subscription");

    cleanup(&pool, &prefix).await;
}
