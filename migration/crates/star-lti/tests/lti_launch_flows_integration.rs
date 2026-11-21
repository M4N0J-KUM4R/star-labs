//! Integration tests for issue #95 — LTI launch flows: OIDC login
//! initiation (`/api/lti/authorize_redirect`), the authentication request
//! (`/api/lti/authorize`), the LTI 2 authorization server
//! (`/api/lti/{courses,accounts}/:id/authorize`), the LTI Resource Links
//! API (`/api/v1/courses/:course_id/lti_resource_links*`) and deep
//! linking content selection (`/{courses,accounts,groups}/:id/
//! deep_linking_response`, `/deep_linking_cancel`).
//!
//! Canvas references:
//! - app/controllers/lti/ims/authentication_controller.rb
//! - app/controllers/lti/ims/authorization_controller.rb
//! - app/controllers/lti/resource_links_controller.rb
//! - app/controllers/lti/ims/deep_linking_controller.rb
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set.
//! - Unique per-run prefix + full teardown, so repeat runs against a shared
//!   database are safe.

use axum::http::header::{AUTHORIZATION, HOST};
use axum_test::TestServer;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_common::AppState;
use std::collections::HashMap;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "lti95-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

struct Fixtures {
    admin: i64,
    teacher: i64,
    student: i64,
    account: i64,
    course: i64,
    api_key: String,
    tool_id: i64,
}

async fn setup() -> Option<(TestServer, PgPool, String, Fixtures)> {
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

    // Mirror the star-http mount exactly: the course-scoped router is
    // nested under /api/v1 and the LTI Advantage router (which carries the
    // /api/lti prefix and the root-level deep-linking paths) is merged at
    // the application root.
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_lti::lti_advantage_router())
            .nest("/api/v1", star_lti::router())
            .with_state(state),
    )
    .expect("build TestServer");

    let prefix = run_prefix();

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root").await;
    let course = seed_course(&pool, &prefix, "course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Developer key — the LTI 1.3 client.
    let api_key = format!("{prefix}-client-id");
    let dev_key_id: i64 = sqlx::query_scalar(
        "INSERT INTO developer_keys (name, api_key, redirect_uris, scopes, workflow_state) \
         VALUES ($1, $2, $3, $4, 'active') RETURNING id",
    )
    .bind(format!("{prefix} dev key"))
    .bind(&api_key)
    .bind(vec!["https://tool.example.com/launch".to_string()])
    .bind(vec![
        "https://canvas.instructure.com/lti/replace_editor_contents".to_string(),
    ])
    .fetch_one(&pool)
    .await
    .expect("seed developer key");

    // An LTI 1.3 registration + deployment for deployment-id resolution.
    let registration_id: i64 = sqlx::query_scalar(
        "INSERT INTO lti_registrations (account_id, name) VALUES ($1, $2) RETURNING id",
    )
    .bind(account)
    .bind(format!("{prefix}-registration"))
    .fetch_one(&pool)
    .await
    .expect("seed registration");
    let _ = sqlx::query(
        "INSERT INTO lti_deployments (registration_id, deployment_id, context_type, context_id) \
         VALUES ($1, $2, 'Course', $3)",
    )
    .bind(registration_id)
    .bind(format!("{prefix}-deployment"))
    .bind(course)
    .execute(&pool)
    .await
    .expect("seed deployment");

    // The LTI 1.3 tool in the course context.
    let tool_id: i64 = sqlx::query_scalar(
        "INSERT INTO external_tools \
         (name, url, domain, context_type, context_id, lti_version, developer_key_id, \
          lti_registration_id, account_id) \
         VALUES ($1, $2, $3, 'Course', $4, '1.3', $5, $6, $7) RETURNING id",
    )
    .bind(format!("{prefix}-tool"))
    .bind("https://tool.example.com/launch")
    .bind("tool.example.com")
    .bind(course)
    .bind(dev_key_id)
    .bind(registration_id)
    .bind(account)
    .fetch_one(&pool)
    .await
    .expect("seed tool");

    let fixtures = Fixtures {
        admin,
        teacher,
        student,
        account,
        course,
        api_key,
        tool_id,
    };
    Some((server, pool, prefix, fixtures))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
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

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    let _ = sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await;
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str, course: i64) {
    // Canvas-parity tables without FK cascades to the seeded course.
    let _ = sqlx::query("DELETE FROM content_tags WHERE context_id = $1")
        .bind(course)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM context_modules WHERE context_id = $1")
        .bind(course)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM collaborations WHERE context_id = $1")
        .bind(course)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM lti_resource_links WHERE context_id = $1")
        .bind(course)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM lti_resource_links \
                         WHERE context_type = 'Assignment' AND context_id IN \
                         (SELECT id FROM assignments WHERE course_id = $1)",
    )
    .bind(course)
    .execute(pool)
    .await;
    // Star-specific tables.
    let _ = sqlx::query("DELETE FROM external_tools WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM developer_keys WHERE api_key LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    // registrations/deployments cascade from the accounts; courses must go
    // before accounts.
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
// JWT minting
// ---------------------------------------------------------------------------

fn sign(claims: &Value, secret: &str) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign jwt")
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A platform-signed `lti_message_hint` (Canvas `CanvasSecurity.create_jwt`).
fn hint_jwt(claims: Value, secret: &str) -> String {
    let mut all = json!({
        "verifier": "verifier-1",
        "canvas_domain": "canvas.test",
        "context_type": "Course",
        "context_id": 0,
        "exp": now() + 3600,
        "iat": now(),
    });
    if let (Some(target), Some(source)) = (all.as_object_mut(), claims.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    sign(&all, secret)
}

fn decode(token: &str, secret: &str) -> Value {
    let mut validation = jsonwebtoken::Validation::default();
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    jsonwebtoken::decode::<Value>(
        token,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .expect("decode jwt")
    .claims
}

// ---------------------------------------------------------------------------
// authorize_redirect
// -------------------------------------------------------------------

#[tokio::test]
async fn authorize_redirect_redirects_to_the_hint_domain() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let hint = hint_jwt(
        json!({"context_id": f.course, "canvas_domain": "canvas.test:8443"}),
        JWT_SECRET,
    );
    let query = format!(
        "/api/lti/authorize_redirect?client_id={}&login_hint=abc&lti_message_hint={}&nonce=n1\
         &prompt=none&redirect_uri=https%3A%2F%2Ftool.example.com%2Flaunch&response_mode=form_post\
         &response_type=id_token&scope=openid&state=st1&extra=dropped",
        f.api_key, hint
    );

    let resp = server.get(&query).await;
    assert_eq!(resp.status_code(), 302, "body: {}", resp.text());
    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .expect("location header")
        .to_string();
    assert!(
        location.starts_with("http://canvas.test:8443/api/lti/authorize?"),
        "location: {location}"
    );
    assert!(location.contains("client_id="), "location: {location}");
    assert!(location.contains("state=st1"), "location: {location}");
    assert!(
        !location.contains("extra=dropped"),
        "non-OIDC params are dropped: {location}"
    );

    // The POST form variant behaves the same.
    let mut form = HashMap::new();
    form.insert("client_id".to_string(), f.api_key.clone());
    form.insert("lti_message_hint".to_string(), hint);
    form.insert("scope".to_string(), "openid".to_string());
    let resp = server.post("/api/lti/authorize_redirect").form(&form).await;
    assert_eq!(resp.status_code(), 302, "POST body: {}", resp.text());

    // An invalid hint is Canvas' plain 400.
    let resp = server
        .get("/api/lti/authorize_redirect?lti_message_hint=not-a-jwt")
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.text(), "Invalid lti_message_hint");

    cleanup(&pool, &prefix, f.course).await;
}

// ---------------------------------------------------------------------------
// authorize (GET /api/lti/authorize)
// ---------------------------------------------------------------------------

/// The full, valid OIDC parameter set for `authorize`.
fn oidc_params(f: &Fixtures, hint: &str, login_hint: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    params.insert("client_id".to_string(), f.api_key.clone());
    params.insert("login_hint".to_string(), login_hint.to_string());
    params.insert("lti_message_hint".to_string(), hint.to_string());
    params.insert("nonce".to_string(), "nonce-1".to_string());
    params.insert("prompt".to_string(), "none".to_string());
    params.insert(
        "redirect_uri".to_string(),
        "https://tool.example.com/launch".to_string(),
    );
    params.insert("response_mode".to_string(), "form_post".to_string());
    params.insert("response_type".to_string(), "id_token".to_string());
    params.insert("scope".to_string(), "openid".to_string());
    params.insert("state".to_string(), "st-1".to_string());
    params
}

fn query_string(params: &HashMap<String, String>) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

#[tokio::test]
async fn authorize_validates_oidc_params_in_canvas_order() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let hint = hint_jwt(json!({"context_id": f.course}), JWT_SECRET);
    let teacher_auth = auth(f.teacher, "teacher");

    // Missing parameters (with a session, Canvas lists them in
    // REQUIRED_PARAMS order).
    let resp = server
        .get(&format!("/api/lti/authorize?client_id={}", f.api_key))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher_auth.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_request_object", "body: {body}");
    assert_eq!(
        body["error_description"],
        "The following parameters are missing: login_hint,lti_message_hint,nonce,prompt,\
         redirect_uri,response_mode,response_type,scope"
    );
    assert!(body["state"].is_null());

    // Wrong scope.
    let mut params = oidc_params(&f, &hint, "some-login-hint");
    params.insert("scope".to_string(), "profile".to_string());
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher_auth.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_request_object");
    assert_eq!(body["error_description"], "The 'scope' must be 'openid'");
    assert_eq!(body["state"], "st-1");

    // No session with valid params and a valid hint: the login_required
    // error screen (401).
    let params = oidc_params(&f, &hint, "some-login-hint");
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .await;
    assert_eq!(resp.status_code(), 401, "no session");
    let body: Value = resp.json();
    assert_eq!(body["error"], "login_required");
    assert_eq!(
        body["error_description"],
        "Must have an active user session"
    );

    // Without a session the hint is still decoded before the error screen:
    // an invalid hint is a plain 400 even when other params are missing.
    let resp = server
        .get("/api/lti/authorize?client_id=1&lti_message_hint=garbage")
        .add_header(HOST, "canvas.test")
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.text(), "Invalid lti_message_hint");

    // A login_hint that does not identify the user.
    let params = oidc_params(&f, &hint, "some-login-hint");
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher_auth.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["error"], "login_required", "body: {body}");

    // Unknown client_id: Canvas' 404 (the hint was already decoded).
    let mut params = oidc_params(&f, &hint, &star_lti::opaque_login_hint(f.teacher));
    params.insert("client_id".to_string(), "999999999".to_string());
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher_auth.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "unknown client");

    // Invalid hint with a session (decoded inside validate_current_user!).
    let params = oidc_params(&f, "garbage", &star_lti::opaque_login_hint(f.teacher));
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher_auth.clone())
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.text(), "Invalid lti_message_hint");

    // A hint pointing at an unknown context.
    let hint = hint_jwt(json!({"context_id": 987654321}), JWT_SECRET);
    let params = oidc_params(&f, &hint, &star_lti::opaque_login_hint(f.teacher));
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher_auth)
        .await;
    assert_eq!(resp.status_code(), 404, "unknown context");

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn authorize_returns_launch_no_longer_valid_without_a_cached_launch() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let hint = hint_jwt(json!({"context_id": f.course}), JWT_SECRET);
    let params = oidc_params(&f, &hint, &star_lti::opaque_login_hint(f.teacher));
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["error"], "launch_no_longer_valid", "body: {body}");
    assert_eq!(
        body["error_description"],
        "The launch has either expired or already been consumed"
    );

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn authorize_rejects_unregistered_redirect_uri() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let hint = hint_jwt(
        json!({"context_id": f.course, "verifier": "v-redir"}),
        JWT_SECRET,
    );
    star_lti::launch_cache::seed_launch(
        "Course",
        f.course,
        "v-redir",
        star_lti::launch_cache::CachedLaunch {
            tool_id: f.tool_id,
            message_type: "LtiResourceLinkRequest".to_string(),
            target_link_uri: "https://tool.example.com/launch".to_string(),
            extra_claims: json!({}),
        },
    );
    let mut params = oidc_params(&f, &hint, &star_lti::opaque_login_hint(f.teacher));
    params.insert(
        "redirect_uri".to_string(),
        "https://evil.example.com/steal".to_string(),
    );
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.text(), "Invalid redirect_uri");

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn authorize_returns_a_signed_id_token_with_nonce_state_and_deployment_id() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let verifier = format!("{prefix}-verifier");
    let hint = hint_jwt(
        json!({"context_id": f.course, "verifier": verifier}),
        JWT_SECRET,
    );
    star_lti::launch_cache::seed_launch(
        "Course",
        f.course,
        &verifier,
        star_lti::launch_cache::CachedLaunch {
            tool_id: f.tool_id,
            message_type: "LtiResourceLinkRequest".to_string(),
            target_link_uri: "https://tool.example.com/launch".to_string(),
            extra_claims: json!({
                "https://purl.imsglobal.org/spec/lti/claim/custom": {"hello": "world"},
                "https://purl.imsglobal.org/spec/lti/claim/resource_link": {
                    "id": "res-1", "title": "The Link"
                }
            }),
        },
    );
    let params = oidc_params(&f, &hint, &star_lti::opaque_login_hint(f.teacher));
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["state"], "st-1");

    let id_token = body["id_token"].as_str().expect("id_token").to_string();
    let claims = decode(&id_token, JWT_SECRET);
    assert_eq!(claims["iss"], "https://canvas.instructure.com");
    assert_eq!(claims["aud"], f.api_key, "aud is the client id");
    assert_eq!(claims["azp"], f.api_key);
    assert_eq!(claims["nonce"], "nonce-1", "OIDC nonce is applied");
    assert_eq!(claims["sub"], star_lti::opaque_login_hint(f.teacher));
    assert_eq!(
        claims["https://purl.imsglobal.org/spec/lti/claim/deployment_id"],
        format!("{prefix}-deployment"),
        "deployment id is resolved from the registration's course deployment"
    );
    assert_eq!(
        claims["https://purl.imsglobal.org/spec/lti/claim/message_type"],
        "LtiResourceLinkRequest"
    );
    assert_eq!(
        claims["https://purl.imsglobal.org/spec/lti/claim/version"],
        "1.3.0"
    );
    assert_eq!(
        claims["https://purl.imsglobal.org/spec/lti/claim/custom"]["hello"],
        "world"
    );
    assert!(claims["exp"].as_i64().unwrap() > now());

    // The launch is single-use: replaying is launch_no_longer_valid.
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["error"], "launch_no_longer_valid", "body: {body}");

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn authorize_forwards_the_lti_storage_target_when_requested() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let hint = hint_jwt(
        json!({"context_id": f.course, "verifier": format!("{prefix}-v2"), "include_storage_target": true}),
        JWT_SECRET,
    );
    star_lti::launch_cache::seed_launch(
        "Course",
        f.course,
        &format!("{prefix}-v2"),
        star_lti::launch_cache::CachedLaunch {
            tool_id: f.tool_id,
            message_type: "LtiResourceLinkRequest".to_string(),
            target_link_uri: "https://tool.example.com/launch".to_string(),
            extra_claims: json!({}),
        },
    );
    let params = oidc_params(&f, &hint, &star_lti::opaque_login_hint(f.teacher));
    let resp = server
        .get(&format!("/api/lti/authorize?{}", query_string(&params)))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["lti_storage_target"], "post_message_forwarding");

    cleanup(&pool, &prefix, f.course).await;
}

// ---------------------------------------------------------------------------
// LTI 2 authorization server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lti2_authorize_returns_an_access_token() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let path = format!("/api/lti/courses/{}/authorize", f.course);

    // Unknown context.
    let resp = server
        .post("/api/lti/courses/987654321/authorize")
        .form(&HashMap::<String, String>::from([(
            "grant_type".to_string(),
            "urn:ietf:params:oauth:grant-type:jwt-bearer".to_string(),
        )]))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown course");

    // Bad grant type / missing assertion.
    for (grant_type, assertion) in [
        ("password", ""),
        ("urn:ietf:params:oauth:grant-type:jwt-bearer", ""),
    ] {
        let resp = server
            .post(&path)
            .add_header(HOST, "canvas.test")
            .form(&HashMap::<String, String>::from([
                ("grant_type".to_string(), grant_type.to_string()),
                ("assertion".to_string(), assertion.to_string()),
            ]))
            .await;
        assert_eq!(resp.status_code(), 400, "grant {grant_type}");
        assert_eq!(resp.json()["error"], "invalid_grant");
    }

    // The assertion is signed with the developer key's api_key (Canvas
    // `AuthorizationValidator#jwt_secret`) and its aud is this endpoint.
    let assertion = sign(
        &json!({
            "iss": f.api_key,
            "sub": f.api_key,
            "aud": format!("http://canvas.test{path}"),
            "exp": now() + 60,
            "iat": now(),
            "jti": uuid::Uuid::new_v4().to_string(),
        }),
        &f.api_key,
    );
    let resp = server
        .post(&path)
        .add_header(HOST, "canvas.test")
        .form(&HashMap::<String, String>::from([
            (
                "grant_type".to_string(),
                "urn:ietf:params:oauth:grant-type:jwt-bearer".to_string(),
            ),
            ("assertion".to_string(), assertion.clone()),
        ]))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["token_type"], "bearer");
    assert_eq!(body["expires_in"], "3600");
    let access_token = body["access_token"].as_str().expect("access_token");
    let claims = decode(access_token, JWT_SECRET);
    assert_eq!(claims["iss"], "Canvas");
    assert_eq!(claims["sub"], f.api_key);
    assert_eq!(claims["reg_key"], f.api_key, "reg_key defaults to sub");

    // authorization_code grants pass the reg_key through.
    let resp = server
        .post(&path)
        .add_header(HOST, "canvas.test")
        .form(&HashMap::<String, String>::from([
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("assertion".to_string(), assertion),
            ("code".to_string(), "reg-key-42".to_string()),
        ]))
        .await;
    assert_eq!(resp.status_code(), 200);
    let claims = decode(resp.json()["access_token"].as_str().unwrap(), JWT_SECRET);
    assert_eq!(claims["reg_key"], "reg-key-42");

    // A wrong signature and a wrong aud are both invalid_grant.
    let bad_sig = sign(
        &json!({
            "iss": f.api_key, "sub": f.api_key,
            "aud": format!("http://canvas.test{path}"),
            "exp": now() + 60, "iat": now(), "jti": "x",
        }),
        "wrong-secret",
    );
    let resp = server
        .post(&path)
        .add_header(HOST, "canvas.test")
        .form(&HashMap::<String, String>::from([
            (
                "grant_type".to_string(),
                "urn:ietf:params:oauth:grant-type:jwt-bearer".to_string(),
            ),
            ("assertion".to_string(), bad_sig),
        ]))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.json()["error"], "invalid_grant");

    let bad_aud = sign(
        &json!({
            "iss": f.api_key, "sub": f.api_key,
            "aud": "http://elsewhere.example.com/authorize",
            "exp": now() + 60, "iat": now(), "jti": "y",
        }),
        &f.api_key,
    );
    let resp = server
        .post(&path)
        .add_header(HOST, "canvas.test")
        .form(&HashMap::<String, String>::from([
            (
                "grant_type".to_string(),
                "urn:ietf:params:oauth:grant-type:jwt-bearer".to_string(),
            ),
            ("assertion".to_string(), bad_aud),
        ]))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.json()["error"], "invalid_grant");

    // The account variant of the endpoint answers too.
    let resp = server
        .post(&format!("/api/lti/accounts/{}/authorize", f.account))
        .form(&HashMap::<String, String>::from([(
            "grant_type".to_string(),
            "urn:ietf:params:oauth:grant-type:jwt-bearer".to_string(),
        )]))
        .await;
    assert_eq!(resp.status_code(), 400, "account endpoint answers");

    cleanup(&pool, &prefix, f.course).await;
}

// ---------------------------------------------------------------------------
// Resource links API
// ---------------------------------------------------------------------------

struct LinkFixtures {
    /// (id, lookup_uuid) of the seeded links.
    rich: (i64, String),
    module_item: (i64, String),
    collaboration: (i64, String),
    assignment: (i64, String),
    deleted: (i64, String),
    assignment_id: i64,
    /// The module item content tag id for the module_item link.
    module_item_tag: i64,
}

async fn seed_link_row(
    pool: &PgPool,
    f: &Fixtures,
    context_type: &str,
    context_id: i64,
    state: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO lti_resource_links
         (context_external_tool_id, workflow_state, root_account_id, context_id,
          context_type, custom, lookup_uuid, resource_link_uuid, url, title, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, '{"k":"v"}'::jsonb, gen_random_uuid(), gen_random_uuid(),
          'https://tool.example.com/launch', 'seeded', now(), now()) RETURNING id"#,
    )
    .bind(f.tool_id)
    .bind(state)
    .bind(f.account)
    .bind(context_id)
    .bind(context_type)
    .fetch_one(pool)
    .await
    .expect("seed link")
}

async fn lookup_uuid_of(pool: &PgPool, id: i64) -> String {
    let (uuid,): (String,) =
        sqlx::query_as("SELECT lookup_uuid FROM lti_resource_links WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .expect("lookup uuid");
    uuid
}

async fn seed_links(pool: &PgPool, f: &Fixtures, prefix: &str) -> LinkFixtures {
    let assignment_id: i64 = sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title) VALUES ($1, $2) RETURNING id",
    )
    .bind(f.course)
    .bind(format!("{prefix}-assignment"))
    .fetch_one(pool)
    .await
    .expect("seed assignment");

    let rich = seed_link_row(pool, f, "Course", f.course, "active").await;
    let module_item = seed_link_row(pool, f, "Course", f.course, "active").await;
    let collaboration = seed_link_row(pool, f, "Course", f.course, "active").await;
    let assignment = seed_link_row(pool, f, "Assignment", assignment_id, "active").await;
    let deleted = seed_link_row(pool, f, "Course", f.course, "deleted").await;

    // A module and its module item for the module_item link.
    let module_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO context_modules
         (context_id, context_type, name, workflow_state, root_account_id, created_at, updated_at)
         VALUES ($1, 'Course', $2, 'active', $3, now(), now()) RETURNING id"#,
    )
    .bind(f.course)
    .bind(format!("{prefix}-module"))
    .bind(f.account)
    .fetch_one(pool)
    .await
    .expect("seed module");
    let module_item_tag: i64 = sqlx::query_scalar(
        r#"INSERT INTO content_tags
         (content_id, content_type, context_id, context_type, tag_type, context_module_id,
          associated_asset_type, associated_asset_id, workflow_state, root_account_id,
          created_at, updated_at)
         VALUES ($1, 'ContextExternalTool', $2, 'Course', 'context_module', $3,
          'Lti::ResourceLink', $4, 'active', $5, now(), now()) RETURNING id"#,
    )
    .bind(f.tool_id)
    .bind(f.course)
    .bind(module_id)
    .bind(module_item)
    .bind(f.account)
    .fetch_one(pool)
    .await
    .expect("seed content tag");

    let collab_lookup = lookup_uuid_of(pool, collaboration).await;
    let _ = sqlx::query(
        r#"INSERT INTO collaborations
         (collaboration_type, user_id, context_id, context_type, title,
          resource_link_lookup_uuid, created_at, updated_at)
         VALUES ('external_tool', $1, $2, 'Course', $3, $4, now(), now())"#,
    )
    .bind(f.teacher)
    .bind(f.course)
    .bind(format!("{prefix}-collab"))
    .bind(collab_lookup)
    .execute(pool)
    .await
    .expect("seed collaboration");

    LinkFixtures {
        rich: (rich, lookup_uuid_of(pool, rich).await),
        module_item: (module_item, lookup_uuid_of(pool, module_item).await),
        collaboration: (collaboration, lookup_uuid_of(pool, collaboration).await),
        assignment: (assignment, lookup_uuid_of(pool, assignment).await),
        deleted: (deleted, lookup_uuid_of(pool, deleted).await),
        assignment_id,
        module_item_tag,
    }
}

#[tokio::test]
async fn resource_links_index_classifies_and_paginates() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let links = seed_links(&pool, &f, &prefix).await;
    let base = format!("/api/v1/courses/{}/lti_resource_links", f.course);

    // Anonymous → 401; student → 403 (permission checks run first).
    let resp = server.get(&base).await;
    assert_eq!(resp.status_code(), 401);
    let resp = server
        .get(&base)
        .add_header(AUTHORIZATION, auth(f.student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Teacher (course management) → 200.
    let resp = server
        .get(&base)
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    let items = body.as_array().expect("array");
    assert_eq!(items.len(), 4, "deleted link is invisible: {body}");

    let by_id = |items: &Vec<Value>, id: i64| {
        items
            .iter()
            .find(|item| item["id"].as_i64() == Some(id))
            .unwrap_or_else(|| panic!("link {id} missing"))
            .clone()
    };

    let assignment_link = by_id(items, links.assignment.0);
    assert_eq!(assignment_link["resource_type"], "assignment");
    assert_eq!(
        assignment_link["canvas_launch_url"],
        format!(
            "http://canvas.test/courses/{}/assignments/{}",
            f.course, links.assignment_id
        )
    );

    let module_link = by_id(items, links.module_item.0);
    assert_eq!(module_link["resource_type"], "module_item");
    assert_eq!(module_link["associated_content_type"], "ModuleItem");
    assert_eq!(
        module_link["associated_content_id"].as_i64(),
        Some(links.module_item_tag)
    );
    assert_eq!(
        module_link["canvas_launch_url"],
        format!(
            "http://canvas.test/courses/{}/external_tools/retrieve?resource_link_lookup_uuid={}",
            f.course, links.module_item.1
        )
    );

    assert_eq!(
        by_id(items, links.collaboration.0)["resource_type"],
        "collaboration"
    );
    assert_eq!(by_id(items, links.rich.0)["resource_type"], "rich_content");

    // Common JSON shape.
    let rich = by_id(items, links.rich.0);
    assert_eq!(rich["context_id"].as_i64(), Some(f.course));
    assert_eq!(rich["context_type"], "Course");
    assert_eq!(rich["context_external_tool_id"].as_i64(), Some(f.tool_id));
    assert_eq!(rich["custom"]["k"], "v");
    assert_eq!(rich["workflow_state"], "active");
    assert!(rich["lookup_uuid"].is_string());
    assert!(rich["resource_link_uuid"].is_string());
    assert!(rich["created_at"].is_string());
    assert!(rich["title"].is_string());
    assert!(rich["url"].is_string());
    assert!(rich.get("associated_content_type").is_none());

    // include_deleted surfaces the soft-deleted link.
    let resp = server
        .get(&format!("{base}?include_deleted=true"))
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .await;
    let body: Value = resp.json();
    assert_eq!(body.as_array().unwrap().len(), 5, "include_deleted");

    // Pagination.
    let resp = server
        .get(&format!("{base}?per_page=2&page=2"))
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .await;
    let body: Value = resp.json();
    let page2 = body.as_array().expect("page array");
    assert_eq!(page2.len(), 2, "second page has the remaining links");
    let all: Vec<i64> = page2.iter().filter_map(|i| i["id"].as_i64()).collect();
    assert!(
        all.iter()
            .all(|id| *id > links.rich.0 && *id != links.rich.0),
        "page 2 skips the first page: {all:?}"
    );

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn resource_links_show_supports_the_canvas_id_formats() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let links = seed_links(&pool, &f, &prefix).await;
    let base = format!("/api/v1/courses/{}/lti_resource_links", f.course);

    let get = |path: String| {
        let server = &server;
        async move {
            server
                .get(&path)
                .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
                .await
        }
    };

    let (resource_link_uuid,): (String,) =
        sqlx::query_as("SELECT resource_link_uuid FROM lti_resource_links WHERE id = $1")
            .bind(links.rich.0)
            .fetch_one(&pool)
            .await
            .expect("resource link uuid");
    for id in [
        links.rich.0.to_string(),
        format!("lookup_uuid:{}", links.rich.1),
        format!("resource_link_uuid:{resource_link_uuid}"),
    ] {
        let resp = get(format!("{base}/{id}")).await;
        assert_eq!(resp.status_code(), 200, "id {id}: {}", resp.text());
        let body: Value = resp.json();
        assert_eq!(body["id"].as_i64(), Some(links.rich.0));
    }

    // Deleted links are 404 unless include_deleted.
    let resp = get(format!("{base}/{}", links.deleted.0)).await;
    assert_eq!(resp.status_code(), 404, "deleted link is invisible");
    let resp = get(format!("{base}/{}?include_deleted=true", links.deleted.0)).await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json()["workflow_state"], "deleted");

    // Unknown ids are 404 — including malformed uuid formats.
    let resp = get(format!("{base}/99999999")).await;
    assert_eq!(resp.status_code(), 404);
    let resp = get(format!("{base}/lookup_uuid:not-a-uuid")).await;
    assert_eq!(resp.status_code(), 404);

    // Unknown course.
    let resp = server
        .get("/api/v1/courses/987654321/lti_resource_links/1")
        .add_header(AUTHORIZATION, auth(f.admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn resource_links_create_validates_in_canvas_order() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let base = format!("/api/v1/courses/{}/lti_resource_links", f.course);
    let teacher = auth(f.teacher, "teacher");

    // Custom with a non-string value is invalid, and checked before the url.
    let resp = server
        .post(&base)
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({"url": "https://tool.example.com/launch", "custom": {"count": 3}}))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["errors"][0]["code"], "invalid_custom");
    assert_eq!(
        body["errors"][0]["message"],
        "'custom' param must contain key/value pairs"
    );

    // An unparseable URL.
    let resp = server
        .post(&base)
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({"url": "not a url at all"}))
        .await;
    assert_eq!(resp.status_code(), 422);
    assert_eq!(resp.json()["errors"][0]["code"], "invalid_url");
    assert_eq!(
        resp.json()["errors"][0]["message"],
        "'url' param must be a valid URL"
    );

    // A valid URL with no matching LTI 1.3 tool.
    let resp = server
        .post(&base)
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({"url": "https://unknown.example.com/launch"}))
        .await;
    assert_eq!(resp.status_code(), 422);
    assert_eq!(resp.json()["errors"][0]["code"], "invalid_url");
    assert_eq!(
        resp.json()["errors"][0]["message"],
        "No tool found for the provided URL"
    );

    // Happy path: the URL matches the seeded 1.3 tool.
    let resp = server
        .post(&base)
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({
            "url": "https://tool.example.com/launch",
            "title": "A New Link",
            "custom": {"hello": "world"}
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["resource_type"], "rich_content");
    assert_eq!(body["title"], "A New Link");
    assert_eq!(body["url"], "https://tool.example.com/launch");
    assert_eq!(body["custom"]["hello"], "world");
    assert_eq!(body["workflow_state"], "active");
    assert_eq!(body["context_external_tool_id"].as_i64(), Some(f.tool_id));
    let lookup_uuid = body["lookup_uuid"]
        .as_str()
        .expect("lookup uuid")
        .to_string();
    assert_eq!(
        body["canvas_launch_url"],
        format!(
            "http://canvas.test/courses/{}/external_tools/retrieve?resource_link_lookup_uuid={lookup_uuid}",
            f.course
        )
    );
    // The link exists and is visible.
    let resp = server
        .get(&format!("{base}/lookup_uuid:{lookup_uuid}"))
        .add_header(AUTHORIZATION, teacher)
        .await;
    assert_eq!(resp.status_code(), 200);

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn resource_links_bulk_create_is_all_or_nothing() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let base = format!("/api/v1/courses/{}/lti_resource_links/bulk", f.course);
    let teacher = auth(f.teacher, "teacher");

    // Over the Canvas cap of 100.
    let oversized: Vec<Value> = (0..101)
        .map(|i| json!({"url": format!("https://tool.example.com/launch/{i}")}))
        .collect();
    let resp = server
        .post(&base)
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&oversized)
        .await;
    assert_eq!(resp.status_code(), 422);
    let body: Value = resp.json();
    assert_eq!(body["errors"][0]["code"], "invalid_request");
    assert_eq!(
        body["errors"][0]["message"],
        "Cannot create more than 100 resource links at once"
    );

    // One bad item → no links at all, errors carry value/error/message.
    let resp = server
        .post(&base)
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!([
            {"url": "https://tool.example.com/launch", "title": "Good"},
            {"url": "https://unknown.example.com/x", "title": "Bad"},
            {"url": "also bad", "title": "Worse"}
        ]))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let body: Value = resp.json();
    let errors = body["errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 2, "body: {body}");
    assert_eq!(errors[0]["value"], "https://unknown.example.com/x");
    assert_eq!(errors[0]["error"], "invalid_url");
    assert_eq!(errors[0]["message"], "No tool found for the provided URL");
    assert_eq!(errors[1]["error"], "invalid_url");
    assert_eq!(errors[1]["message"], "'url' param must be a valid URL");

    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM lti_resource_links WHERE context_type = 'Course' AND context_id = $1",
    )
    .bind(f.course)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(count.0, 0, "nothing created on error");

    // Happy path.
    let resp = server
        .post(&base)
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!([
            {"url": "https://tool.example.com/launch", "title": "One", "custom": {"a": "b"}},
            {"url": "https://tool.example.com/launch/deep", "title": "Two"}
        ]))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["title"], "One");
    assert_eq!(items[0]["custom"]["a"], "b");
    assert_eq!(items[1]["resource_type"], "rich_content");
    assert!(items[0]["lookup_uuid"].as_str().is_some());

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn resource_links_update_validates_tools_and_urls() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let links = seed_links(&pool, &f, &prefix).await;
    let base = format!("/api/v1/courses/{}/lti_resource_links", f.course);
    let teacher = auth(f.teacher, "teacher");

    // Invalid custom.
    let resp = server
        .put(&format!("{}/{}", base, links.rich.0))
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({"custom": {"n": 1}}))
        .await;
    assert_eq!(resp.status_code(), 422);
    assert_eq!(resp.json()["errors"][0]["code"], "invalid_custom");

    // Unknown replacement tool.
    let resp = server
        .put(&format!("{}/{}", base, links.rich.0))
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({"context_external_tool_id": 99999999}))
        .await;
    assert_eq!(resp.status_code(), 422);
    let body: Value = resp.json();
    assert_eq!(body["errors"][0]["code"], "context_external_tool_id");
    assert_eq!(
        body["errors"][0]["message"],
        "'context_external_tool_id' param must be a valid ContextExternalTool ID"
    );

    // A URL that does not match the tool.
    let resp = server
        .put(&format!("{}/{}", base, links.rich.0))
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({"url": "https://evil.example.com/launch"}))
        .await;
    assert_eq!(resp.status_code(), 422);
    let body: Value = resp.json();
    assert_eq!(body["errors"][0]["code"], "url_tool_mismatch");
    assert_eq!(
        body["errors"][0]["message"],
        "'url' param must match the tool associated with 'context_external_tool_id'"
    );

    // Happy path: same-domain URL updates fine.
    let resp = server
        .put(&format!("{}/{}", base, links.rich.0))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({
            "url": "https://tool.example.com/launch/updated",
            "title_keep": true,
            "custom": {"x": "y"}
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["url"], "https://tool.example.com/launch/updated");
    assert_eq!(body["custom"]["x"], "y");
    assert_eq!(body["resource_type"], "rich_content");

    // Deleted links: 404 by default, updatable with include_deleted.
    let resp = server
        .put(&format!("{}/{}", base, links.deleted.0))
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({"url": "https://tool.example.com/launch"}))
        .await;
    assert_eq!(resp.status_code(), 404, "deleted link is invisible");
    let resp = server
        .put(&format!(
            "{}/{}?include_deleted=true",
            base, links.deleted.0
        ))
        .add_header(AUTHORIZATION, teacher.clone())
        .json(&json!({"url": "https://tool.example.com/launch"}))
        .await;
    assert_eq!(resp.status_code(), 200, "include_deleted update");

    // Unknown link id.
    let resp = server
        .put(&format!("{}/99999999", base))
        .add_header(AUTHORIZATION, teacher)
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn resource_links_destroy_only_allows_rich_content_links() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let links = seed_links(&pool, &f, &prefix).await;
    let base = format!("/api/v1/courses/{}/lti_resource_links", f.course);
    let teacher = auth(f.teacher, "teacher");

    // Module items cannot be deleted via this API.
    let resp = server
        .delete(&format!("{}/{}", base, links.module_item.0))
        .add_header(AUTHORIZATION, teacher.clone())
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["errors"][0]["code"], "invalid_resource_link");
    assert_eq!(
        body["errors"][0]["message"],
        "Cannot delete resource links associated with Assignments, Module Items, or Collaborations"
    );

    // Assignments cannot either.
    let resp = server
        .delete(&format!("{}/lookup_uuid:{}", base, links.assignment.1))
        .add_header(AUTHORIZATION, teacher.clone())
        .await;
    assert_eq!(resp.status_code(), 422);

    // Rich content is soft-deleted and returned.
    let resp = server
        .delete(&format!("{}/{}", base, links.rich.0))
        .add_header(HOST, "canvas.test")
        .add_header(AUTHORIZATION, teacher.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "deleted");
    assert_eq!(body["id"].as_i64(), Some(links.rich.0));

    // The deleted link is gone from the default scope (soft-delete
    // invisibility) and not destroyable again.
    let resp = server
        .delete(&format!("{}/{}", base, links.rich.0))
        .add_header(AUTHORIZATION, teacher)
        .await;
    assert_eq!(resp.status_code(), 404, "second destroy");

    cleanup(&pool, &prefix, f.course).await;
}

// ---------------------------------------------------------------------------
// Deep linking
// ---------------------------------------------------------------------------

fn deep_linking_jwt(f: &Fixtures, content_items: Value, extra: Value) -> String {
    let mut claims = json!({
        "iss": f.api_key,
        "sub": f.api_key,
        "aud": "https://canvas.instructure.com",
        "exp": now() + 300,
        "iat": now(),
        "jti": uuid::Uuid::new_v4().to_string(),
        "nonce": uuid::Uuid::new_v4().to_string(),
        "https://purl.imsglobal.org/spec/lti-dl/claim/content_items": content_items,
    });
    if let (Some(target), Some(source)) = (claims.as_object_mut(), extra.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    sign(&claims, JWT_SECRET)
}

fn data_jwt(claims: Value) -> String {
    let mut all = json!({"nonce": uuid::Uuid::new_v4().to_string()});
    if let (Some(target), Some(source)) = (all.as_object_mut(), claims.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    sign(&all, JWT_SECRET)
}

#[tokio::test]
async fn deep_linking_response_validates_the_jwt_in_canvas_order() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let path = format!("/courses/{}/deep_linking_response", f.course);
    let data = data_jwt(json!({"placement": "link_selection"}));

    // No JWT at all.
    let resp = server
        .post(&path)
        .form(&HashMap::<String, String>::from([(
            "data".to_string(),
            data.clone(),
        )]))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.json()["jwt"][0], "JWT format is invalid");

    // Unknown client (iss does not match a developer key).
    let unknown = sign(
        &json!({
            "iss": "who-knows", "sub": "who-knows",
            "aud": "https://canvas.instructure.com",
            "exp": now() + 300, "iat": now(), "jti": "j", "nonce": "n"
        }),
        JWT_SECRET,
    );
    let resp = server
        .post(&path)
        .form(&HashMap::<String, String>::from([
            ("JWT".to_string(), unknown),
            ("data".to_string(), data.clone()),
        ]))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.json()["jwt"][0], "Client not found");

    // Wrong signature.
    let bad_sig = sign(
        &json!({
            "iss": f.api_key, "sub": f.api_key,
            "aud": "https://canvas.instructure.com",
            "exp": now() + 300, "iat": now(), "jti": "j", "nonce": "n"
        }),
        "wrong-secret",
    );
    let resp = server
        .post(&path)
        .form(&HashMap::<String, String>::from([
            ("JWT".to_string(), bad_sig),
            ("data".to_string(), data.clone()),
        ]))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.json()["jwt"][0], "JWT verification failure");

    // Wrong audience.
    let bad_aud = sign(
        &json!({
            "iss": f.api_key, "sub": f.api_key,
            "aud": "https://someone-else.example.com",
            "exp": now() + 300, "iat": now(), "jti": "j", "nonce": "n"
        }),
        JWT_SECRET,
    );
    let resp = server
        .post(&path)
        .form(&HashMap::<String, String>::from([
            ("JWT".to_string(), bad_aud),
            ("data".to_string(), data.clone()),
        ]))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.json()["base"][0], "the 'aud' is invalid");

    // Missing data JWT.
    let jwt = deep_linking_jwt(
        &f,
        json!([{"type": "ltiResourceLink", "url": "https://tool.example.com/launch", "title": "L"}]),
        json!({}),
    );
    let resp = server
        .post(&path)
        .form(&HashMap::<String, String>::from([("JWT".to_string(), jwt)]))
        .await;
    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    let err = &body["data_jwt"][0];
    assert_eq!(err["attribute"], "?data=JWT");
    assert_eq!(err["type"], "presence_required");

    // Unknown context.
    let resp = server
        .post("/courses/987654321/deep_linking_response")
        .form(&HashMap::<String, String>::new())
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn deep_linking_response_creates_resource_links_for_link_selection() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let path = format!("/courses/{}/deep_linking_response", f.course);
    let data = data_jwt(json!({"placement": "link_selection"}));
    let jwt = deep_linking_jwt(
        &f,
        json!([{
            "type": "ltiResourceLink",
            "url": "https://tool.example.com/launch/content/42",
            "title": "Content Item",
            "custom": {"hello": "world"}
        }]),
        json!({
            "https://purl.imsglobal.org/spec/lti-dl/claim/msg": "picked!",
            "https://purl.imsglobal.org/spec/lti-dl/claim/log": "log-line",
        }),
    );

    let resp = server
        .post(&path)
        .add_header(HOST, "canvas.test")
        .form(&HashMap::<String, String>::from([
            ("JWT".to_string(), jwt),
            ("data".to_string(), data.clone()),
        ]))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let dl = &body["deep_link_response"];
    assert_eq!(dl["placement"], "link_selection");
    assert_eq!(dl["msg"], "Message from external tool: picked!");
    assert_eq!(dl["log"], "log-line");
    assert_eq!(dl["reloadpage"], false);
    assert_eq!(dl["moduleCreated"], false);
    assert_eq!(dl["replaceEditorContents"], false);
    assert_eq!(
        dl["ltiEndpoint"],
        format!(
            "http://canvas.test/courses/{}/external_tools/retrieve",
            f.course
        )
    );
    let item = &dl["content_items"][0];
    assert_eq!(item["title"], "Content Item");
    let lookup_uuid = item["lookup_uuid"]
        .as_str()
        .expect("lookup_uuid")
        .to_string();

    // The resource link exists with the item's data.
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT resource_link_uuid, title FROM lti_resource_links \
         WHERE context_type = 'Course' AND context_id = $1 AND lookup_uuid = $2",
    )
    .bind(f.course)
    .bind(&lookup_uuid)
    .fetch_optional(&pool)
    .await
    .expect("fetch link");
    assert!(row.is_some(), "resource link created");
    assert_eq!(row.unwrap().1.as_deref(), Some("Content Item"));

    // The data JWT is single-use (Canvas: "Do not attempt to submit content
    // items multiple times.").
    let jwt2 = deep_linking_jwt(
        &f,
        json!([{"type": "ltiResourceLink", "url": "https://tool.example.com/launch", "title": "L"}]),
        json!({}),
    );
    let resp = server
        .post(&path)
        .form(&HashMap::<String, String>::from([
            ("JWT".to_string(), jwt2),
            ("data".to_string(), data),
        ]))
        .await;
    assert_eq!(resp.status_code(), 400, "replay");
    let body: Value = resp.json();
    assert_eq!(body["data_jwt"][0]["type"], "already_used");

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn deep_linking_response_module_flow_creates_module_and_items() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let path = format!("/courses/{}/deep_linking_response", f.course);

    // No session for a resource-creating placement → 401.
    let data = data_jwt(json!({"placement": "module_index_menu_modal"}));
    let jwt = deep_linking_jwt(
        &f,
        json!([{
            "type": "ltiResourceLink",
            "url": "https://tool.example.com/launch/m1",
            "title": "Module Item"
        }]),
        json!({"https://canvas.instructure.com/lti/module_name": "My Custom Module"}),
    );
    let resp = server
        .post(&path)
        .form(&HashMap::<String, String>::from([
            ("JWT".to_string(), jwt.clone()),
            ("data".to_string(), data.clone()),
        ]))
        .await;
    assert_eq!(resp.status_code(), 401, "no session");

    // A student cannot create course content → 403.
    let resp = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(f.student, "student"))
        .form(&HashMap::<String, String>::from([
            ("JWT".to_string(), jwt.clone()),
            ("data".to_string(), data.clone()),
        ]))
        .await;
    assert_eq!(resp.status_code(), 403, "student");

    // Teacher: the module is created with the claimed name, the item is
    // added and the response reports the module.
    let resp = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .form(&HashMap::<String, String>::from([
            ("JWT".to_string(), jwt),
            ("data".to_string(), data),
        ]))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let dl = &body["deep_link_response"];
    assert_eq!(dl["moduleCreated"], true);
    assert_eq!(dl["reloadpage"], true);

    let module: (i64,) = sqlx::query_as(
        "SELECT id FROM context_modules WHERE context_id = $1 AND name = 'My Custom Module'",
    )
    .bind(f.course)
    .fetch_one(&pool)
    .await
    .expect("module created");
    let tag: (i64,) = sqlx::query_as(
        "SELECT id FROM content_tags WHERE context_module_id = $1 \
         AND associated_asset_type = 'Lti::ResourceLink'",
    )
    .bind(module.0)
    .fetch_one(&pool)
    .await
    .expect("module item added");
    assert!(tag.0 > 0);

    // Items can also be added to an existing module via context_module_id.
    let existing_module: (i64,) = sqlx::query_as(
        "INSERT INTO context_modules (context_id, context_type, name, workflow_state, root_account_id) \
         VALUES ($1, 'Course', 'existing', 'active', $2) RETURNING id",
    )
    .bind(f.course)
    .bind(f.account)
    .fetch_one(&pool)
    .await
    .expect("seed existing module");
    let data = data_jwt(json!({
        "placement": "editor_button", "context_module_id": existing_module.0
    }));
    let jwt = deep_linking_jwt(
        &f,
        json!([{
            "type": "ltiResourceLink",
            "url": "https://tool.example.com/launch/m2",
            "title": "Second Item"
        }]),
        json!({}),
    );
    let resp = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(f.teacher, "teacher"))
        .form(&HashMap::<String, String>::from([
            ("JWT".to_string(), jwt),
            ("data".to_string(), data),
        ]))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["deep_link_response"]["moduleCreated"], false);
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM content_tags WHERE context_module_id = $1")
            .bind(existing_module.0)
            .fetch_one(&pool)
            .await
            .expect("count tags");
    assert_eq!(count.0, 1, "item added to the existing module");

    cleanup(&pool, &prefix, f.course).await;
}

#[tokio::test]
async fn deep_linking_cancel_returns_an_empty_selection() {
    let Some((server, pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let resp = server
        .get("/deep_linking_cancel?placement=link_selection&lti_msg=nevermind&lti_log=logged")
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let dl = &body["deep_link_response"];
    assert_eq!(dl["placement"], "link_selection");
    assert_eq!(dl["content_items"], json!([]));
    assert_eq!(dl["msg"], "Message from external tool: nevermind");
    assert_eq!(dl["log"], "logged");
    assert_eq!(dl["reloadpage"], false);
    assert_eq!(dl["moduleCreated"], false);
    assert_eq!(dl["replaceEditorContents"], false);

    cleanup(&pool, &prefix, f.course).await;
}

// ---------------------------------------------------------------------------
// Mount smoke: every route answers on the merged server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn launch_flow_routes_are_routed() {
    let Some((server, _pool, prefix, f)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    // (path, method) pairs that must answer with something other than the
    // routing 404 (which has an empty body).
    let paths: [(&str, &str); 7] = [
        ("/api/lti/authorize_redirect", "GET"),
        ("/api/lti/authorize", "GET"),
        ("/api/lti/courses/1/authorize", "POST"),
        ("/api/lti/accounts/1/authorize", "POST"),
        (
            &format!("/api/v1/courses/{}/lti_resource_links", f.course),
            "GET",
        ),
        ("/courses/1/deep_linking_response", "POST"),
        ("/deep_linking_cancel", "GET"),
    ];
    for (path, method) in paths {
        let resp = match method {
            "GET" => server.get(path).await,
            "POST" => {
                server
                    .post(path)
                    .form(&HashMap::<String, String>::new())
                    .await
            }
            _ => unreachable!(),
        };
        assert!(
            resp.status_code() != 404 || !resp.text().is_empty(),
            "{method} {path} is not routed"
        );
    }
    let _ = prefix;
}
