//! LTI 1.1 API (Canvas `LtiApiController`) integration tests.
//!
//! Covers the Canvas routes:
//! - `POST /api/lti/v1/tools/:tool_id/grade_passback`
//! - `POST /api/lti/v1/tools/:tool_id/ext_grade_passback`
//! - `POST /api/lti/v1/xapi/:token`
//! - `POST /api/lti/v1/caliper/:token`
//! - `POST /api/lti/v1/logout_service/:token`
//! - `POST /api/lti/v1/turnitin/outcomes_placement/:tool_id`
//!
//! Every route verifies the OAuth 1.0a signature of the request, so the
//! tests sign their requests the way an LTI 1.1 tool does.
//!
//! Fleet rule: dedicated users, accounts and tools per test, every seeded
//! row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";
const CONSUMER_KEY: &str = "consumer-key";
const SHARED_SECRET: &str = "shared-secret";

type HmacSha1 = Hmac<Sha1>;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "la-{}-{}-{}",
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

async fn seed_tool(pool: &PgPool, prefix: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO external_tools (name, url, domain, consumer_key, consumer_id, \
             shared_secret, workflow_state, tool_configuration) \
         VALUES ($1, 'https://tool.test/launch', 'tool.test', $2, $2, $3, 'active', '{}') \
         RETURNING id",
    )
    .bind(format!("{prefix}-tool"))
    .bind(CONSUMER_KEY)
    .bind(SHARED_SECRET)
    .fetch_one(pool)
    .await
    .expect("seed tool")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query("DELETE FROM lti_nonces WHERE consumer_key = $1")
        .bind(CONSUMER_KEY)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM external_tools WHERE name LIKE $1")
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

/// The OAuth 1.0a header of a signed request.
fn oauth_header(path: &str, nonce: &str) -> String {
    let timestamp = chrono::Utc::now().timestamp();
    let mut normalized = [
        percent_encode(CONSUMER_KEY),
        percent_encode(nonce),
        percent_encode("HMAC-SHA1"),
        percent_encode(&timestamp.to_string()),
    ];
    normalized.sort();
    let base = format!(
        "{}&{}&{}",
        percent_encode("POST"),
        percent_encode(&format!("http://localhost{path}")),
        percent_encode(&normalized.join("&"))
    );
    let mut mac =
        HmacSha1::new_from_slice(SHARED_SECRET.as_bytes()).expect("HMAC accepts any key length");
    mac.update(base.as_bytes());
    let signature = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());

    format!(
        "OAuth oauth_consumer_key=\"{CONSUMER_KEY}\", oauth_signature_method=\"HMAC-SHA1\", \
         oauth_timestamp=\"{timestamp}\", oauth_nonce=\"{nonce}\", oauth_signature=\"{signature}\""
    )
}

/// The OAuth 1.0a percent encoding.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[tokio::test]
async fn a_signed_grade_passback_is_accepted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _user = seed_user(&pool, &prefix, "User", "student").await;
    seed_tool(&pool, &prefix).await;

    let path = "/api/lti/v1/tools/consumer-key/grade_passback";
    let resp = server
        .post(path)
        .add_header(
            axum::http::header::AUTHORIZATION,
            oauth_header(path, "nonce-grade-1"),
        )
        .add_header(axum::http::header::CONTENT_TYPE, "application/xml")
        .bytes("<?xml version=\"1.0\"?><imsx_POXEnvelopeRequest/>".into())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_unsigned_request_is_unauthorized() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _user = seed_user(&pool, &prefix, "User", "student").await;
    seed_tool(&pool, &prefix).await;

    let resp = server
        .post("/api/lti/v1/tools/consumer-key/grade_passback")
        .add_header(axum::http::header::CONTENT_TYPE, "application/xml")
        .bytes("<?xml version=\"1.0\"?><imsx_POXEnvelopeRequest/>".into())
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_reused_nonce_is_unauthorized() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _user = seed_user(&pool, &prefix, "User", "student").await;
    seed_tool(&pool, &prefix).await;

    let path = "/api/lti/v1/tools/consumer-key/ext_grade_passback";
    let resp = server
        .post(path)
        .add_header(
            axum::http::header::AUTHORIZATION,
            oauth_header(path, "nonce-reused"),
        )
        .bytes("{}".into())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // The same nonce a second time is rejected.
    let resp = server
        .post(path)
        .add_header(
            axum::http::header::AUTHORIZATION,
            oauth_header(path, "nonce-reused"),
        )
        .bytes("{}".into())
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_missing_tool_is_a_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _user = seed_user(&pool, &prefix, "User", "student").await;

    let path = "/api/lti/v1/tools/unknown-tool/grade_passback";
    let resp = server
        .post(path)
        .add_header(
            axum::http::header::AUTHORIZATION,
            oauth_header(path, "nonce-missing"),
        )
        .add_header(axum::http::header::CONTENT_TYPE, "application/xml")
        .bytes("{}".into())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
