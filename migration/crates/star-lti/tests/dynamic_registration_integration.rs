//! Integration tests for dynamic_registration and tool_proxy routes.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set.
//! - Unique per-run prefix + full teardown.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "lti_dr_{}_{}",
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

    let server = TestServer::new(star_lti::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) \
         RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin') \
         ON CONFLICT (account_id, user_id) DO NOTHING",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account admin");
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM lti_tool_proxies WHERE guid LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM lti_ims_registrations WHERE guid LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM lti_registration_update_requests WHERE uuid LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
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
// Helpers
// ---------------------------------------------------------------------------

/// Create an IMS registration via POST /registrations and return its id.
async fn create_ims_registration(server: &TestServer, admin_id: i64, prefix: &str) -> i64 {
    let response = server
        .post("/registrations")
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({
            "client_name": format!("{prefix}-tool"),
            "redirect_uris": "https://example.com/callback",
            "initiate_login_uri": "https://example.com/oidc/login",
            "jwks_uri": "https://example.com/.well-known/jwks.json",
            "logo_uri": "https://example.com/logo.png",
            "scopes": "openid",
            "https://purl.imsglobal.org/spec/lti-tool-configuration": {
                "title": "Test Tool",
                "target_link_uri": "https://example.com/launch"
            }
        }))
        .await;
    assert_eq!(
        response.status_code(),
        201,
        "create ims registration: {}",
        response.text()
    );
    let body: Value = response.json();
    body["id"].as_i64().expect("registration id")
}

/// Create an LTI registration in lti_registrations and return its id.
async fn create_lti_registration(
    pool: &PgPool,
    account_id: i64,
    admin_id: i64,
    prefix: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO lti_registrations
        (account_id, root_account_id, name, lti_version, workflow_state, configuration,
         dynamic_registration, created_by_id, updated_by_id)
        VALUES ($1, $1, $2, '1.3', 'active', $3, TRUE, $4, $4)
        RETURNING id
        "#,
    )
    .bind(account_id)
    .bind(format!("{prefix}-reg"))
    .bind(json!({ "title": "Test Tool" }))
    .bind(admin_id)
    .fetch_one(pool)
    .await
    .expect("create lti registration")
}

async fn create_tool_proxy(
    server: &TestServer,
    course_id: i64,
    admin_id: i64,
    prefix: &str,
) -> Value {
    let response = server
        .post(&format!("/courses/{course_id}/tool_proxy"))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({
            "@context": "http://purl.imsglobal.org/ctx/lti/v2/ToolProxy",
            "@type": "ToolProxy",
            "tool_proxy_guid": format!("{prefix}-guid"),
            "template": "https://example.com/tool-proxy"
        }))
        .await;
    assert_eq!(
        response.status_code(),
        201,
        "create tool proxy: {}",
        response.text()
    );
    response.json()
}

// ---------------------------------------------------------------------------
// Tests: dynamic_registration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registration_token_requires_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;

    // Anonymous gets 401.
    let resp = server
        .get(&format!("/accounts/{account}/registration_token"))
        .await;
    assert_eq!(resp.status_code(), 401);

    // Student gets 403.
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let resp = server
        .get(&format!("/accounts/{account}/registration_token"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Admin gets 200 with token.
    let resp = server
        .get(&format!("/accounts/{account}/registration_token"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body["token"].is_string(), "token is a JWT string");
    assert!(body["uuid"].is_string());
    assert!(body["oidc_configuration_url"].is_string());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_show_update_ims_registration() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    seed_account_admin(&pool, account, admin).await;

    // Create via POST /registrations.
    let response = server
        .post("/registrations")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "client_name": format!("{prefix}-tool"),
            "redirect_uris": "https://example.com/callback",
            "initiate_login_uri": "https://example.com/oidc/login",
            "jwks_uri": "https://example.com/.well-known/jwks.json",
            "scopes": "openid",
            "https://purl.imsglobal.org/spec/lti-tool-configuration": {
                "title": "Integration Tool"
            }
        }))
        .await;
    assert_eq!(response.status_code(), 201, "body: {}", response.text());
    let created: Value = response.json();
    let id = created["id"].as_i64().expect("id");
    assert_eq!(created["client_name"], format!("{prefix}-tool"));
    assert_eq!(created["name"], format!("{prefix}-tool"));

    // Show via GET /registrations/:registration_id.
    let response = server
        .get(&format!("/registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let shown: Value = response.json();
    assert_eq!(shown["id"], id);
    assert_eq!(shown["client_name"], format!("{prefix}-tool"));

    // Update via PUT /registrations/:registration_id.
    let response = server
        .put(&format!("/registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "client_name": format!("{prefix}-updated-tool"),
            "redirect_uris": "https://example.com/new-callback",
            "initiate_login_uri": "https://example.com/oidc/login",
            "jwks_uri": "https://example.com/.well-known/jwks.json"
        }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let updated: Value = response.json();
    assert_eq!(updated["client_name"], format!("{prefix}-updated-tool"));

    // Non-admin cannot create.
    let response = server
        .post("/registrations")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "client_name": "Should Fail",
            "redirect_uris": "https://fail.com/callback",
            "initiate_login_uri": "https://fail.com/login",
            "jwks_uri": "https://fail.com/jwks"
        }))
        .await;
    assert_eq!(response.status_code(), 403, "non-admin create");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_registration_validates_required_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    for (missing_field, expected_error) in [
        ("client_name", "client_name is required"),
        ("redirect_uris", "redirect_uris is required"),
        ("initiate_login_uri", "initiate_login_uri is required"),
        ("jwks_uri", "jwks_uri is required"),
    ] {
        let mut payload = json!({
            "client_name": "Test Tool",
            "redirect_uris": "https://example.com/callback",
            "initiate_login_uri": "https://example.com/login",
            "jwks_uri": "https://example.com/jwks"
        });
        payload.as_object_mut().unwrap().remove(missing_field);

        let response = server
            .post("/registrations")
            .add_header(AUTHORIZATION, auth(admin, "admin"))
            .json(&payload)
            .await;
        assert_eq!(response.status_code(), 422, "missing {}", missing_field);
        let body: Value = response.json();
        assert!(
            body["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| { e.as_str().unwrap().contains(expected_error) }),
            "error '{}' not found in {:?}",
            expected_error,
            body["errors"]
        );
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn dr_iframe_requires_valid_token() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;

    // First get a valid token.
    let resp = server
        .get(&format!("/accounts/{account}/registration_token"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    let token_body: Value = resp.json();
    let token = token_body["token"].as_str().unwrap();

    // Valid token + url returns 200.
    let url = format!(
        "https://example.com/dynamic-registration?registration_token={}",
        token
    );
    let resp = server
        .get(&format!("/accounts/{account}/dr_iframe"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .add_query_params([("url", &url)])
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Missing token is 400.
    let resp = server
        .get(&format!("/accounts/{account}/dr_iframe"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .add_query_params([("url", "https://example.com/no-token")])
        .await;
    assert_eq!(resp.status_code(), 400, "missing token");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn registration_view_returns_redirect() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let reg_id = create_lti_registration(&pool, account, admin, &prefix).await;

    let resp = server
        .get(&format!("/accounts/{account}/registrations/{reg_id}/view"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body["url"].as_str().unwrap().contains(&reg_id.to_string()));
}

#[tokio::test]
async fn overlay_update_and_account_scoped_shows() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let reg_id = create_lti_registration(&pool, account, admin, &prefix).await;

    // Update overlay.
    let resp = server
        .put(&format!(
            "/accounts/{account}/registrations/{reg_id}/overlay"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "disabledScopes": ["https://example.com/scope1"]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "overlay update: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], reg_id);
    assert_eq!(
        body["overlay"]["disabledScopes"][0],
        "https://example.com/scope1"
    );

    // Show via account-scoped path.
    let resp = server
        .get(&format!("/accounts/{account}/registrations/{reg_id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Tests: tool_proxy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_tool_proxy_course_and_account() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let course = seed_course(&pool, &prefix, "course", account).await;

    // Course tool proxy.
    let resp = server
        .post(&format!("/courses/{course}/tool_proxy"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "tool_proxy_guid": format!("{prefix}-course-guid") }))
        .await;
    assert_eq!(resp.status_code(), 201, "course create: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["@type"], "ToolProxy");
    assert!(body["tool_proxy_guid"].is_string());

    // Account tool proxy.
    let resp = server
        .post(&format!("/accounts/{account}/tool_proxy"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "tool_proxy_guid": format!("{prefix}-account-guid") }))
        .await;
    assert_eq!(resp.status_code(), 201, "account create: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_and_delete_tool_proxy() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let course = seed_course(&pool, &prefix, "course", account).await;

    // Create.
    let proxy = create_tool_proxy(&server, course, admin, &prefix).await;
    let proxy_id = proxy["id"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_default();

    // Update.
    let resp = server
        .put(&format!("/courses/{course}/tool_proxies/{proxy_id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "update: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "active");

    // Delete.
    let resp = server
        .delete(&format!("/courses/{course}/tool_proxies/{proxy_id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "delete: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "deleted");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn accept_and_dismiss_tool_proxy_update() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let course = seed_course(&pool, &prefix, "course", account).await;

    // Create a tool proxy with update_payload.
    let proxy = create_tool_proxy(&server, course, admin, &prefix).await;
    let proxy_id = proxy["id"]
        .as_str()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_default();

    // Seed an update_payload.
    sqlx::query(
        "UPDATE lti_tool_proxies SET update_payload = $2, updated_at = now() \
         WHERE id = $1",
    )
    .bind(proxy_id)
    .bind(json!({ "changes": ["new_feature"] }))
    .execute(&pool)
    .await
    .expect("seed update_payload");

    // Accept update.
    let resp = server
        .put(&format!("/courses/{course}/tool_proxies/{proxy_id}/update"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "accept: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "Success");

    // Seed again and dismiss.
    sqlx::query(
        "UPDATE lti_tool_proxies SET update_payload = $2, updated_at = now() \
         WHERE id = $1",
    )
    .bind(proxy_id)
    .bind(json!({ "changes": ["other"] }))
    .execute(&pool)
    .await
    .expect("seed update_payload 2");

    let resp = server
        .delete(&format!("/courses/{course}/tool_proxies/{proxy_id}/update"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "dismiss: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "success");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn tool_proxy_account_routes() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;

    // Create.
    let resp = server
        .post(&format!("/accounts/{account}/tool_proxy"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "tool_proxy_guid": format!("{prefix}-acct-guid") }))
        .await;
    assert_eq!(resp.status_code(), 201, "account create: {}", resp.text());

    // Read tool proxies via GUID lookup (needs a real guid from DB).
    let row: Option<(String,)> =
        sqlx::query_as("SELECT guid FROM lti_tool_proxies WHERE context_type = 'Account' LIMIT 1")
            .fetch_optional(&pool)
            .await
            .expect("lookup guid");
    if let Some((guid,)) = row {
        let resp = server
            .get(&format!("/tool_proxy/{guid}"))
            .add_header(AUTHORIZATION, auth(admin, "admin"))
            .await;
        assert_eq!(resp.status_code(), 200, "guid show: {}", resp.text());
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn tool_proxy_student_forbidden() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let course = seed_course(&pool, &prefix, "course", account).await;
    seed_account_admin(&pool, account, admin).await;

    // Create as admin succeeds.
    let resp = server
        .post(&format!("/courses/{course}/tool_proxy"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "tool_proxy_guid": format!("{prefix}-s-guid") }))
        .await;
    assert_eq!(resp.status_code(), 201);

    // Student gets 403.
    let resp = server
        .post(&format!("/courses/{course}/tool_proxy"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "tool_proxy_guid": "should-fail" }))
        .await;
    assert_eq!(resp.status_code(), 403, "student create");

    cleanup(&pool, &prefix).await;
}
