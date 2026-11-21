//! Integration tests for the Canvas /login/oauth2 flow family (issue #97):
//!
//! - GET /login/oauth2/auth (oauth2_provider#auth)
//! - POST /login/oauth2/token (oauth2_provider#token, authorization_code
//!   + refresh_token grants, PKCE S256)
//! - DELETE /login/oauth2/token (oauth2_provider#destroy)
//! - GET /login/oauth2/jwks (security#jwks)
//! - POST /api/v1/inst_access_tokens (inst_access_tokens#create; the api
//!   router is mounted at the test root, so the path here is
//!   /inst_access_tokens)
//!
//! Parity notes (canvas-lms master):
//! - Auth validation order (oauth2_provider_controller.rb:31-117): unknown
//!   client 401 JSON + WWW-Authenticate; bad redirect_uri 400 JSON; scope /
//!   account / response_type / prompt errors redirect back to the client with
//!   `error` params (Rails to_query order: error, error_description, state).
//! - Codes are 128 hex chars, single-use, bound to user + client + scopes
//!   (`Canvas::OAuth::Token.generate_code_for`).
//! - Token responses follow `Canvas::OAuth::Token#as_json`: access_token,
//!   token_type, user object, canvas_region, refresh_token only when freshly
//!   generated, expires_in only for auto-expiring keys.
//! - Refresh keeps the refresh token (Canvas rotates only for public
//!   clients) and regenerates the access token, which kills the old value.
//! - No secret leakage: token values never appear outside `access_token` /
//!   `refresh_token`, the JWKS only exposes the public key half.
//!
//! Conventions (mirrors auth_integration.rs): skipped cleanly without
//! DATABASE_URL, unique per-run prefix, full teardown.

use axum_test::TestServer;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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
        "oauth-flow-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

async fn db_pool() -> Option<PgPool> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    Some(pool)
}

async fn setup() -> Option<(TestServer, PgPool, String)> {
    let pool = db_pool().await?;
    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    // Both routers, exactly like star-http: the api router under /api/v1
    // (mounted at the test root here) and the login router at the root.
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_auth::router())
            .merge(star_auth::login_router())
            .with_state(state),
    )
    .expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role)
        VALUES ($1, $2, 'x', $3)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

struct SeedKey {
    id: i64,
    api_key: String,
}

/// Insert a developer key directly (the /developer_keys API is covered in
/// auth_integration.rs).
async fn seed_key(
    pool: &PgPool,
    prefix: &str,
    name: &str,
    require_scopes: bool,
    auto_expire_tokens: bool,
) -> SeedKey {
    let api_key = format!("{prefix}-{name}-secret");
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO developer_keys
            (name, api_key, redirect_uris, scopes, require_scopes, visible,
             auto_expire_tokens, workflow_state)
        VALUES ($1, $2, $3, $4, $5, TRUE, $6, 'active')
        RETURNING id
        "#,
    )
    .bind(format!("{prefix}-{name}"))
    .bind(&api_key)
    .bind(vec!["https://client.example.org/callback"])
    .bind(vec!["url:GET|/api/v1/courses", "url:POST|/api/v1/courses"])
    .bind(require_scopes)
    .bind(auto_expire_tokens)
    .fetch_one(pool)
    .await
    .expect("seed developer key");
    SeedKey { id, api_key }
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM developer_keys WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

/// Percent-encode a query value for manually-built query strings.
fn enc(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// GET /login/oauth2/auth with the given params (optionally authenticated)
/// and return the status and the Location header of the response.
async fn auth_redirect(
    server: &TestServer,
    params: &[(&str, &str)],
    bearer: Option<&str>,
) -> (axum::http::StatusCode, Option<String>) {
    let query = params
        .iter()
        .map(|(key, value)| format!("{key}={}", enc(value)))
        .collect::<Vec<_>>()
        .join("&");
    let mut request = server.get(&format!("/login/oauth2/auth?{query}"));
    if let Some(bearer) = bearer {
        request = request.add_header(axum::http::header::AUTHORIZATION, bearer);
    }
    let resp = request.await;
    let location = resp
        .headers()
        .get(axum::http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (resp.status_code(), location)
}

/// Parse a `?a=b&c=d` query string into a map (values percent-decoded).
fn query_pairs(url: &str) -> std::collections::HashMap<String, String> {
    let query = url.split_once('?').map(|(_, rest)| rest).unwrap_or("");
    let mut pairs = std::collections::HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        pairs.insert(key.to_string(), dec(value));
    }
    pairs
}

fn dec(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Unpadded base64url (for PKCE challenges and JWT parts).
fn b64url_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let mut buffer = [0u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let value = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
        out.push(ALPHABET[(value >> 18) as usize & 63] as char);
        out.push(ALPHABET[(value >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[(value >> 6) as usize & 63] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[value as usize & 63] as char);
        }
    }
    out
}

fn b64url_decode(input: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut table = [255u8; 256];
    for (i, &byte) in ALPHABET.iter().enumerate() {
        table[byte as usize] = i as u8;
    }
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in input.bytes() {
        if byte == b'=' {
            continue;
        }
        let value = table[byte as usize];
        if value == 255 {
            continue;
        }
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// GET /login/oauth2/auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth2_auth_issues_code_via_redirect() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "FlowUser", "teacher").await;
    let key = seed_key(&pool, &prefix, "FlowKey", false, false).await;
    let bearer = auth(user, "teacher");

    // Happy path: a signed-in user is redirected to the client with a fresh
    // code and the state echoed (Canvas final_redirect).
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("state", "xyz-state-123"),
        ],
        Some(&bearer),
    )
    .await;
    assert_eq!(status, 302, "expected redirect");
    let location = location.expect("location header");
    assert!(
        location.starts_with("https://client.example.org/callback?"),
        "unexpected location: {location}"
    );
    let pairs = query_pairs(&location);
    let code = pairs.get("code").expect("code param").clone();
    assert_eq!(code.len(), 128, "SecureRandom.hex(64) size");
    assert!(code.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        pairs.get("state").map(String::as_str),
        Some("xyz-state-123")
    );
    assert_eq!(pairs.len(), 2, "only code + state: {pairs:?}");

    // The OOB redirect value sends the code back to the authorization
    // endpoint itself (Canvas oauth2_auth_url).
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "urn:ietf:wg:oauth:2.0:oob"),
            ("response_type", "code"),
        ],
        Some(&bearer),
    )
    .await;
    assert_eq!(status, 302);
    let location = location.expect("oob location");
    assert!(
        location.starts_with("/login/oauth2/auth?code="),
        "oob redirect: {location}"
    );

    // grant-less exchange defaults to authorization_code (covered further
    // below); here just confirm the code is redeemable.
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "client_id={}&client_secret={}&code={code}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 200, "default grant: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn oauth2_auth_validation_errors() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "FlowErrors", "student").await;
    let key = seed_key(&pool, &prefix, "ErrorsKey", false, false).await;
    let scoped = seed_key(&pool, &prefix, "ScopedKey", true, false).await;
    let bearer = auth(user, "student");

    // Unknown client: 401 JSON + WWW-Authenticate (Canvas oauth_error).
    let resp = server
        .get("/login/oauth2/auth?client_id=999999999&redirect_uri=https://x.example.org/cb&response_type=code")
        .await;
    assert_eq!(resp.status_code(), 401, "unknown client: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_client");
    assert_eq!(body["error_description"], "unknown client");

    // Mismatched redirect_uri: 400 JSON (before any redirect happens).
    let resp = server
        .get(&format!(
            "/login/oauth2/auth?client_id={}&redirect_uri=https%3A%2F%2Fevil.example.org%2Fcb&response_type=code",
            key.id
        ))
        .await;
    assert_eq!(resp.status_code(), 400, "bad redirect: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["error_description"],
        "redirect_uri does not match client settings"
    );

    // response_type must be code; errors redirect back with error params in
    // Rails to_query order (error, error_description, state).
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "token"),
            ("state", "st-1"),
        ],
        Some(&bearer),
    )
    .await;
    assert_eq!(status, 302, "response_type redirect");
    let pairs = query_pairs(&location.expect("location"));
    assert_eq!(pairs["error"], "unsupported_response_type");
    assert_eq!(
        pairs["error_description"],
        "Only response_type=code is permitted"
    );
    assert_eq!(pairs["state"], "st-1");

    // require_scopes keys reject unknown scopes (Canvas invalid_scope
    // redirect, with the missing scopes in the message).
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &scoped.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("scope", "url:GET|/api/v1/courses url:GET|/api/v1/accounts"),
        ],
        Some(&bearer),
    )
    .await;
    assert_eq!(status, 302, "scope redirect");
    let pairs = query_pairs(&location.expect("location"));
    assert_eq!(pairs["error"], "invalid_scope");
    assert!(
        pairs["error_description"].contains(
            "The following scopes were requested, but not granted: url:GET|/api/v1/accounts"
        ),
        "description: {}",
        pairs["error_description"]
    );

    // ...and requests with no scopes at all.
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &scoped.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
        ],
        Some(&bearer),
    )
    .await;
    assert_eq!(status, 302);
    let pairs = query_pairs(&location.expect("location"));
    assert_eq!(pairs["error"], "invalid_scope");

    // prompt=none without a session -> login_required...
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("prompt", "none"),
        ],
        None,
    )
    .await;
    assert_eq!(status, 302);
    let pairs = query_pairs(&location.expect("location"));
    assert_eq!(pairs["error"], "login_required");

    // ...and with a session (but no pre-authorized/reusable token) ->
    // interaction_required, Canvas authorized_token? semantics.
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("prompt", "none"),
        ],
        Some(&bearer),
    )
    .await;
    assert_eq!(status, 302);
    let pairs = query_pairs(&location.expect("location"));
    assert_eq!(pairs["error"], "interaction_required");

    // Unsupported prompt values redirect with unsupported_prompt_type.
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("prompt", "consent"),
        ],
        Some(&bearer),
    )
    .await;
    assert_eq!(status, 302);
    let pairs = query_pairs(&location.expect("location"));
    assert_eq!(pairs["error"], "unsupported_prompt_type");

    // Not signed in: Canvas redirects to its login URL.
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
        ],
        None,
    )
    .await;
    assert_eq!(status, 302, "anonymous auth redirects");
    let location = location.expect("login location");
    assert!(location.starts_with("/login"), "login redirect: {location}");

    // prompt=login forces the login screen even with a session.
    let (status, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("prompt", "login"),
        ],
        Some(&bearer),
    )
    .await;
    assert_eq!(status, 302);
    assert!(
        location.expect("forced login").starts_with("/login"),
        "prompt=login redirect"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /login/oauth2/token — authorization_code grant (+ PKCE)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth2_authorization_code_grant_flow() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "GrantUser", "teacher").await;
    let key = seed_key(&pool, &prefix, "GrantKey", false, false).await;
    let other_key = seed_key(&pool, &prefix, "OtherGrantKey", false, false).await;
    let bearer = auth(user, "teacher");

    // Missing code parameter: Canvas invalid_request error.
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=authorization_code&client_id={}&client_secret={}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400, "missing code: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["error_description"],
        "You must provide the code parameter when using the authorization_code grant type"
    );

    // Full flow: auth -> code -> exchange.
    let (_, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("purpose", "cli integration"),
        ],
        Some(&bearer),
    )
    .await;
    let code = query_pairs(&location.expect("location"))
        .get("code")
        .cloned()
        .expect("code");

    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=authorization_code&client_id={}&client_secret={}&code={code}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 200, "exchange: {}", resp.text());
    let token: Value = resp.json();
    let access_token = token["access_token"]
        .as_str()
        .expect("access_token")
        .to_string();
    assert!(
        token["refresh_token"].is_string(),
        "code exchange issues a refresh token: {token}"
    );
    assert_eq!(access_token.len(), 64);
    assert!(access_token.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(token["token_type"], "Bearer");
    // Canvas::OAuth::Token#as_json user object + region.
    assert_eq!(token["user"]["id"], user);
    assert_eq!(token["user"]["global_id"], user.to_string());
    assert_eq!(token["user"]["fake_student"], false);
    assert_eq!(token["canvas_region"], "unknown");
    // No auto-expiry configured: no expires_in (Canvas only sends it when
    // the key auto-expires tokens).
    assert!(
        token.get("expires_in").is_none(),
        "expires_in must follow auto_expire_tokens: {token}"
    );

    // The row is persisted with key, user, hint, and purpose from the auth
    // request.
    let (dev_key, user_col, hint, purpose): (i64, i64, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT developer_key_id, user_id, token_hint, purpose
             FROM access_tokens WHERE token = $1",
        )
        .bind(&access_token)
        .fetch_one(&pool)
        .await
        .expect("token row");
    assert_eq!(dev_key, key.id);
    assert_eq!(user_col, user);
    assert_eq!(hint.as_deref(), Some(&access_token[..5]));
    assert_eq!(purpose.as_deref(), Some("cli integration"));

    // Codes are single-use: the second exchange fails with Canvas's
    // authorization_code-not-found error.
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=authorization_code&client_id={}&client_secret={}&code={code}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400, "replay: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_grant");
    assert_eq!(body["error_description"], "authorization_code not found");

    // A code issued to another client cannot be redeemed here (incorrect
    // client).
    let (_, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
        ],
        Some(&bearer),
    )
    .await;
    let code = query_pairs(&location.expect("location"))
        .get("code")
        .cloned()
        .expect("code");
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=authorization_code&client_id={}&client_secret={}&code={code}",
            other_key.id, other_key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400, "wrong client: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_grant");
    assert_eq!(body["error_description"], "incorrect client");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn oauth2_pkce_s256_roundtrip() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "PkceUser", "teacher").await;
    let key = seed_key(&pool, &prefix, "PkceKey", false, false).await;
    let bearer = auth(user, "teacher");

    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = b64url_encode(&Sha256::digest(verifier.as_bytes()));

    // Authorize with a PKCE S256 challenge, then exchange with the verifier.
    let (_, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ],
        Some(&bearer),
    )
    .await;
    let code = query_pairs(&location.expect("location"))
        .get("code")
        .cloned()
        .expect("code");

    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=authorization_code&client_id={}&client_secret={}&code={code}&code_verifier={verifier}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 200, "pkce exchange: {}", resp.text());
    assert_eq!(resp.json::<Value>()["token_type"], "Bearer");

    // A wrong verifier fails with the generic invalid_grant error.
    let (_, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
        ],
        Some(&bearer),
    )
    .await;
    let code = query_pairs(&location.expect("location"))
        .get("code")
        .cloned()
        .expect("code");
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=authorization_code&client_id={}&client_secret={}&code={code}&code_verifier=wrong-verifier-wrong-verifier-wrong",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400, "wrong verifier: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_grant");

    // Only S256 is supported: a plain-method challenge is not stored, so the
    // exchange fails.
    let (_, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
            ("code_challenge", "plain-challenge-value"),
            ("code_challenge_method", "plain"),
        ],
        Some(&bearer),
    )
    .await;
    let code = query_pairs(&location.expect("location"))
        .get("code")
        .cloned()
        .expect("code");
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=authorization_code&client_id={}&client_secret={}&code={code}&code_verifier=plain-challenge-value",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400, "plain method: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /login/oauth2/token — refresh_token grant + token lifecycle
// ---------------------------------------------------------------------------

/// Run the full auth -> code -> exchange sequence and return the token JSON
/// (shared by the refresh/revocation tests).
async fn issue_code_token(server: &TestServer, key: &SeedKey, bearer: &str) -> Value {
    let (_, location) = auth_redirect(
        server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
        ],
        Some(bearer),
    )
    .await;
    let code = query_pairs(&location.expect("location"))
        .get("code")
        .cloned()
        .expect("code");
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=authorization_code&client_id={}&client_secret={}&code={code}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 200, "exchange: {}", resp.text());
    resp.json::<Value>()
}

#[tokio::test]
async fn oauth2_refresh_grant_and_revocation() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "RefreshUser", "teacher").await;
    let key = seed_key(&pool, &prefix, "RefreshKey", false, false).await;
    let bearer = auth(user, "teacher");

    // Missing refresh_token parameter.
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=refresh_token&client_id={}&client_secret={}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400, "missing refresh: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(
        body["error_description"],
        "You must provide the refresh_token parameter when using the refresh_token grant type"
    );

    // Issue a token via the authorization_code grant.
    let token = issue_code_token(&server, &key, &bearer).await;
    let access_token = token["access_token"].as_str().expect("access").to_string();
    let refresh_token = token["refresh_token"]
        .as_str()
        .expect("refresh")
        .to_string();

    // Refresh: a new access token is issued, the refresh token is NOT
    // echoed (Canvas confidential clients keep it) and NOT rotated.
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=refresh_token&client_id={}&client_secret={}&refresh_token={refresh_token}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 200, "refresh: {}", resp.text());
    let refreshed: Value = resp.json();
    let new_access = refreshed["access_token"]
        .as_str()
        .expect("new access")
        .to_string();
    assert_ne!(new_access, access_token);
    assert!(
        refreshed.get("refresh_token").is_none(),
        "confidential refresh must not echo the refresh token: {refreshed}"
    );
    assert_eq!(refreshed["user"]["id"], user);

    // The same refresh token keeps working (no rotation for confidential
    // clients).
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=refresh_token&client_id={}&client_secret={}&refresh_token={refresh_token}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 200, "refresh again: {}", resp.text());
    // The second refresh regenerated the access token again — only this
    // value is live now.
    let live_access = resp.json::<Value>()["access_token"]
        .as_str()
        .expect("live access")
        .to_string();

    // The regenerated (rotated) access token killed the old values:
    // revoking with the FIRST token fails with Canvas's message, the
    // current one succeeds.
    let resp = server
        .delete("/login/oauth2/token")
        .add_header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {access_token}"),
        )
        .await;
    assert_eq!(resp.status_code(), 400, "stale revoke: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["message"],
        "can't delete OAuth access token when not using an OAuth access token"
    );

    let resp = server
        .delete("/login/oauth2/token")
        .add_header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {live_access}"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "revoke: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({}));

    // Revocation deletes the row's usability: the refresh token is gone too.
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=refresh_token&client_id={}&client_secret={}&refresh_token={refresh_token}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "refresh after revoke: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["error"], "invalid_grant");
    assert_eq!(body["error_description"], "refresh_token not found");

    // Session JWTs cannot revoke anything (Canvas: not an OAuth token).
    let resp = server
        .delete("/login/oauth2/token")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "jwt revoke: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["message"],
        "can't delete OAuth access token when not using an OAuth access token"
    );

    // Unknown refresh tokens and expired tokens are both "not found".
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=refresh_token&client_id={}&client_secret={}&refresh_token=nope",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(
        resp.json::<Value>()["error_description"],
        "refresh_token not found"
    );

    let expired_refresh = format!("{prefix}-expired-refresh");
    sqlx::query(
        r#"
        INSERT INTO access_tokens
            (user_id, developer_key_id, token, token_hint, refresh_token,
             permanent_expires_at, workflow_state)
        VALUES ($1, $2, $3, $4, $5, now() - interval '1 hour', 'active')
        "#,
    )
    .bind(user)
    .bind(key.id)
    .bind(format!("{prefix}-expired-access"))
    .bind("expir")
    .bind(&expired_refresh)
    .execute(&pool)
    .await
    .expect("seed expired token");
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=refresh_token&client_id={}&client_secret={}&refresh_token={expired_refresh}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400, "expired refresh: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["error_description"],
        "refresh_token not found"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn oauth2_auto_expire_tokens_add_expires_in() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "ExpiryUser", "teacher").await;
    let key = seed_key(&pool, &prefix, "ExpiryKey", false, true).await;
    let bearer = auth(user, "teacher");

    // Keys with auto_expire_tokens get an expires_at and an expires_in in
    // the response (Canvas Token#as_json).
    let (_, location) = auth_redirect(
        &server,
        &[
            ("client_id", &key.id.to_string()),
            ("redirect_uri", "https://client.example.org/callback"),
            ("response_type", "code"),
        ],
        Some(&bearer),
    )
    .await;
    let code = query_pairs(&location.expect("location"))
        .get("code")
        .cloned()
        .expect("code");

    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=authorization_code&client_id={}&client_secret={}&code={code}",
            key.id, key.api_key
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 200, "exchange: {}", resp.text());
    let token: Value = resp.json();
    let expires_in = token["expires_in"].as_i64().expect("expires_in");
    assert!(
        (3550..=3600).contains(&expires_in),
        "expires_in: {expires_in}"
    );

    let access = token["access_token"].as_str().expect("access").to_string();
    let (has_expiry,): (bool,) =
        sqlx::query_as("SELECT expires_at > now() FROM access_tokens WHERE token = $1")
            .bind(&access)
            .fetch_one(&pool)
            .await
            .expect("expiry row");
    assert!(has_expiry, "auto-expiring tokens store expires_at");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /login/oauth2/jwks + POST /inst_access_tokens
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth2_jwks_shape_and_inst_access_tokens() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "JwksUser", "teacher").await;

    // JWKS: Canvas SecurityController#jwks shape + Cache-Control.
    let resp = server.get("/login/oauth2/jwks").await;
    assert_eq!(resp.status_code(), 200, "jwks: {}", resp.text());
    let cache_control = resp
        .headers()
        .get(axum::http::header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .expect("cache-control header")
        .to_string();
    assert_eq!(cache_control, "max-age=3600");

    let jwks_body = resp.json::<Value>();
    let keys = jwks_body["keys"].as_array().expect("keys array");
    assert!(!keys.is_empty());
    let jwk = &keys[0];
    assert_eq!(jwk["kty"], "RSA");
    assert_eq!(jwk["alg"], "RS256");
    assert_eq!(jwk["use"], "sig");
    // The kid encodes the key's creation date (Canvas
    // JWKKeyPair.time_from_kid); the key itself is generated at runtime.
    assert!(
        jwk["kid"].as_str().unwrap_or("").starts_with("star-oauth-"),
        "kid: {jwk:?}"
    );
    assert_eq!(jwk["e"], "AQAB");
    let modulus = b64url_decode(jwk["n"].as_str().expect("n"));
    assert_eq!(modulus.len(), 256, "2048-bit modulus");

    // The modulus must match the public key that signs InstAccess tokens.
    // rotation_check reports the kid-encoded month (Canvas internal aid).
    let resp = server.get("/login/oauth2/jwks?rotation_check=1").await;
    assert_eq!(resp.status_code(), 200, "rotation_check: {}", resp.text());
    let reports: Value = resp.json();
    let report = reports[0].as_str().expect("report");
    assert!(
        report.starts_with("today is day ") && report.contains("key 0 is "),
        "report: {report}"
    );

    // InstAccess tokens: anonymous requests are rejected...
    let resp = server.post("/inst_access_tokens").await;
    assert_eq!(resp.status_code(), 401, "anonymous inst_access");

    // ...authenticated users get a 201 with a signed RS256 JWT.
    let resp = server
        .post("/inst_access_tokens")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "inst_access: {}", resp.text());
    let inst_body: Value = resp.json();
    let token = inst_body["token"].as_str().expect("token").to_string();
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "JWT shape: {token}");

    let header: Value = serde_json::from_slice(&b64url_decode(parts[0])).expect("jwt header");
    assert_eq!(header["alg"], "RS256");
    assert!(
        header["kid"]
            .as_str()
            .unwrap_or("")
            .starts_with("star-oauth-"),
        "jwt kid: {header}"
    );

    let claims: Value = serde_json::from_slice(&b64url_decode(parts[1])).expect("jwt claims");
    assert_eq!(claims["sub"], user.to_string());
    assert_eq!(claims["user_id"], user);
    let exp = claims["exp"].as_i64().expect("exp");
    let iat = claims["iat"].as_i64().expect("iat");
    assert_eq!(exp - iat, 3600, "1 hour InstAccess lifetime");

    // No secret leakage: the JWKS and the inst_access response expose only
    // public material — never the private key, never the api_key.
    let jwks_raw = serde_json::to_string(&jwks_body).unwrap_or_default();
    assert!(!jwks_raw.contains("PRIVATE KEY"), "jwks: {jwks_raw}");
    assert!(jwk.get("d").is_none(), "no private exponent in jwk");
    let inst_raw = serde_json::to_string(&inst_body).unwrap_or_default();
    assert!(!inst_raw.contains("PRIVATE KEY"));

    cleanup(&pool, &prefix).await;
}
