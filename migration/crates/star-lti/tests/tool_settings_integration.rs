//! Integration tests for the Canvas LTI tool settings routes.
//!
//! Pinned behavior:
//! - a request without an `Authorization` header is a 401;
//! - a token without the `ToolSettings` scope is a 403;
//! - the tool settings of a course answer the `@graph` form with the tool
//!   settings media type and the flat `custom` object with the simple one;
//! - an update replaces the custom settings and answers 200 with an empty
//!   body;
//! - a `bubble` value that is neither blank, `distinct` nor `all` is a 400.
//!
//! Fleet rule: dedicated tool proxies, courses and settings per test, every
//! seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum::body::Bytes;
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test AppState and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

/// The media type of the tool settings graph.
const TOOL_SETTINGS_JSON: &str = "application/vnd.ims.lti.v2.toolsettings+json";

/// The media type of the flat tool settings.
const TOOL_SETTINGS_SIMPLE_JSON: &str = "application/vnd.ims.lti.v2.toolsettings.simple+json";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "tool-settings-{}-{}-{}",
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

    // The LTI Advantage router — the routes carry the `/api/lti` prefix.
    let app = star_lti::lti_advantage_router().with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

/// An LTI Advantage access token with the given scopes.
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

/// A Canvas session token — it carries no scopes, so it is not an LTI
/// Advantage access token.
fn session_token(user_id: i64) -> String {
    let claims = json!({
        "sub": user_id,
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

async fn seed_tool_proxy(pool: &PgPool, prefix: &str) -> (i64, String) {
    let guid = format!("{prefix}-guid");
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO lti_tool_proxies (guid, context_type, context_id, workflow_state) \
         VALUES ($1, 'Account', 1, 'active') RETURNING id",
    )
    .bind(&guid)
    .fetch_one(pool)
    .await
    .expect("seed tool proxy");
    (id, guid)
}

async fn seed_course(pool: &PgPool, prefix: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-Course"))
        .bind(format!("{prefix}-Course"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_setting(
    pool: &PgPool,
    tool_proxy_id: i64,
    context_type: &str,
    context_id: i64,
    custom: Value,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO tool_settings (tool_proxy_id, context_type, context_id, custom) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(tool_proxy_id)
    .bind(context_type)
    .bind(context_id)
    .bind(custom)
    .fetch_one(pool)
    .await
    .expect("seed tool setting")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM tool_settings WHERE tool_proxy_id IN \
           (SELECT id FROM lti_tool_proxies WHERE guid LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM lti_tool_proxies WHERE guid LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn the_tool_settings_need_an_lti_advantage_token() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let (tool_proxy_id, guid) = seed_tool_proxy(&pool, &prefix).await;
    let course = seed_course(&pool, &prefix).await;
    seed_setting(&pool, tool_proxy_id, "Course", course, json!({ "a": 1 })).await;

    // Without a token the route is a 401.
    let resp = server
        .get(&format!(
            "/api/lti/tool_proxy/{guid}/courses/{course}/tool_setting"
        ))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // A token that is not an LTI Advantage token is a 401.
    let resp = server
        .get(&format!(
            "/api/lti/tool_proxy/{guid}/courses/{course}/tool_setting"
        ))
        .add_header(AUTHORIZATION, format!("Bearer {}", session_token(42)))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // A token without the scope is a 403.
    let resp = server
        .get(&format!(
            "/api/lti/tool_proxy/{guid}/courses/{course}/tool_setting"
        ))
        .add_header(AUTHORIZATION, format!("Bearer {}", access_token(&guid, "")))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_tool_settings_answer_the_graph_and_the_flat_form() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let (tool_proxy_id, guid) = seed_tool_proxy(&pool, &prefix).await;
    let course = seed_course(&pool, &prefix).await;
    let setting_id = seed_setting(
        &pool,
        tool_proxy_id,
        "Course",
        course,
        json!({ "deep_link": true }),
    )
    .await;

    let path = format!("/api/lti/tool_proxy/{guid}/courses/{course}/tool_setting");

    // The graph form.
    let resp = server
        .get(&path)
        .add_header(
            AUTHORIZATION,
            format!("Bearer {}", access_token(&guid, "ToolSettings")),
        )
        .add_header(ACCEPT, TOOL_SETTINGS_JSON)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let graph = body["@graph"].as_array().expect("graph");
    assert_eq!(graph.len(), 1);
    assert_eq!(graph[0]["@type"], "ToolProxyBinding");
    assert_eq!(graph[0]["custom"]["deep_link"], true);

    // The flat form.
    let resp = server
        .get(&path)
        .add_header(
            AUTHORIZATION,
            format!("Bearer {}", access_token(&guid, "ToolSettings")),
        )
        .add_header(ACCEPT, TOOL_SETTINGS_SIMPLE_JSON)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["deep_link"], true);

    // An unknown bubble value is a 400.
    let resp = server
        .get(&format!("{path}?bubble=nonsense"))
        .add_header(
            AUTHORIZATION,
            format!("Bearer {}", access_token(&guid, "ToolSettings")),
        )
        .add_header(ACCEPT, TOOL_SETTINGS_JSON)
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // The update replaces the custom settings.
    let resp = server
        .put(&path)
        .add_header(
            AUTHORIZATION,
            format!("Bearer {}", access_token(&guid, "ToolSettings")),
        )
        .add_header(CONTENT_TYPE, TOOL_SETTINGS_SIMPLE_JSON)
        .bytes(Bytes::from(json!({ "deep_link": false }).to_string()))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.text(), "");

    let custom: Value = sqlx::query_scalar("SELECT custom FROM tool_settings WHERE id = $1")
        .bind(setting_id)
        .fetch_one(&pool)
        .await
        .expect("read the setting");
    assert_eq!(custom["deep_link"], false);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_setting_that_does_not_exist_is_a_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let (_tool_proxy_id, guid) = seed_tool_proxy(&pool, &prefix).await;
    let course = seed_course(&pool, &prefix).await;

    let resp = server
        .get(&format!(
            "/api/lti/tool_proxy/{guid}/courses/{course}/tool_setting"
        ))
        .add_header(
            AUTHORIZATION,
            format!("Bearer {}", access_token(&guid, "ToolSettings")),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
