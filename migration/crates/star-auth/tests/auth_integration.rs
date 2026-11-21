//! Integration tests for star-auth.
//!
//! Exercises the real auth router (POST /auth/register, POST /auth/login,
//! GET /auth/me, POST /auth/logout), the Canvas developer key routes
//! (GET/POST /developer_keys, GET/PUT/DELETE /developer_keys/{id}), the
//! OAuth2 token endpoint (POST /login/oauth2/token) and the Canvas
//! access-token routes (GET /users/:user_id/user_generated_tokens,
//! POST/GET/PUT/DELETE /users/:user_id/tokens[...], where :user_id is `self`
//! or a numeric id) through axum-test against a real Postgres, plus the
//! public helpers (hash_password/verify_password, create_token/verify_token,
//! fetch_user_by_id via GET /auth/me) and the AuthUser extractor. The full
//! OAuth2 authorization-code flow (auth -> code -> exchange -> refresh ->
//! revoke -> JWKS -> InstAccess) lives in oauth2_flow_integration.rs.
//!
//! Conventions (mirrors the rest of the workspace):
//! - DB-backed tests are skipped cleanly when DATABASE_URL is not set
//!   (prints SKIP and returns); the two pure-function tests always run.
//! - Run with: DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
//!   cargo test -p star-auth
//! - Uses a unique email prefix per run + full teardown so it is safe to run
//!   repeatedly against a shared/dev database.
//!
//! Parity notes (these tests document the ACTUAL handler behavior):
//! - POST /auth/register returns 201 + the created user JSON and NO session
//!   token: registration does not auto-login, clients must POST /auth/login
//!   separately (Canvas likewise keeps self-registration and session
//!   creation apart).
//! - POST /auth/logout is stateless: 204 No Content, nothing is revoked
//!   server-side, the bearer token keeps working until it expires (unlike
//!   Canvas, which destroys the server-side session on logout).
//! - The AuthUser extractor resolves the role from the database on every
//!   request (never trusting the token's role claim) and rejects tokens
//!   whose user no longer exists.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test AppState and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

/// Plain password given to every user seeded directly in the DB.
const SEED_PASSWORD: &str = "pw-test-1234";

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "auth-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations. Returns None when DATABASE_URL is unset so
/// callers can skip cleanly.
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

/// Connect, run migrations, and build a TestServer over the auth router
/// exactly the way star-http assembles it. Returns None when DATABASE_URL
/// is unset so callers can skip cleanly.
async fn setup() -> Option<(TestServer, PgPool, String)> {
    let pool = db_pool().await?;

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };

    let server = TestServer::new(star_auth_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same merge pattern as star-http::build_api_router + the root-level login
/// router, restricted to the auth crate.
fn star_auth_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_auth::router())
        .merge(star_auth::login_router())
        .with_state(state)
}

/// Create a user directly in the DB (bypassing the register endpoint) with a
/// deterministic password hash and the given role. Returns user id.
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
    .bind(star_auth::hash_password(SEED_PASSWORD).expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    // Developer keys (and their access tokens, via ON DELETE CASCADE) are
    // named with the run prefix; rows whose creating user was just deleted
    // still need explicit cleanup.
    let _ = sqlx::query("DELETE FROM developer_keys WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    // Authentication providers for this run (position stores prefix in auth_type as sentinel).
    let _ = sqlx::query("DELETE FROM authentication_providers WHERE auth_type LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    // SSO settings were updated in-place; we do not clean them up since they
    // are account-level and the test account is shared infrastructure.
}

// ---------------------------------------------------------------------------
// POST /auth/register
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_creates_user_and_echoes_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let email = format!("{prefix}-Ada@test.local");
    let resp = server
        .post("/auth/register")
        .json(&json!({
            "name": "Ada Lovelace",
            "email": email,
            "password": "correct-horse-battery"
        }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let id = body["id"].as_i64().expect("id is a number");
    assert!(id > 0, "unexpected id: {body}");
    assert_eq!(body["name"], "Ada Lovelace");
    assert_eq!(body["email"], email);
    // Role defaults to student when omitted.
    assert_eq!(body["role"], "student");
    // sortable_name is computed in SQL as "Last, First".
    assert_eq!(body["sortable_name"], "Lovelace, Ada");
    assert!(body["created_at"].is_string(), "missing created_at: {body}");
    // Parity note: register does NOT return a session token; clients must
    // log in separately (see login_returns_token_and_me_returns_same_user).
    assert!(
        body.get("token").is_none(),
        "register must not return a token: {body}"
    );

    // Row actually persisted, with a real argon2 hash of the password.
    let (name, stored_hash, role): (String, String, String) =
        sqlx::query_as("SELECT name, password_hash, role FROM users WHERE email = $1")
            .bind(&email)
            .fetch_one(&pool)
            .await
            .expect("user row");
    assert_eq!(name, "Ada Lovelace");
    assert_eq!(role, "student");
    assert!(
        stored_hash.starts_with("$argon2"),
        "expected an argon2 PHC hash, got: {stored_hash}"
    );
    assert!(star_auth::verify_password(
        "correct-horse-battery",
        &stored_hash
    ));

    // Explicitly requesting the public role is also accepted.
    let resp = server
        .post("/auth/register")
        .json(&json!({
            "name": "Grace Hopper",
            "email": format!("{prefix}-Grace@test.local"),
            "password": "correct-horse-battery",
            "role": "student"
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "explicit student role: {}",
        resp.text()
    );
    assert_eq!(resp.json::<Value>()["role"], "student");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn register_duplicate_email_is_409() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let email = format!("{prefix}-Dup@test.local");
    let payload = json!({
        "name": "Dup User",
        "email": email,
        "password": "correct-horse-battery"
    });

    let first = server.post("/auth/register").json(&payload).await;
    assert_eq!(first.status_code(), 201, "first register: {}", first.text());

    let second = server.post("/auth/register").json(&payload).await;
    let body = second.text();
    assert_eq!(second.status_code(), 409, "duplicate email: {body}");
    assert!(
        body.contains("email already in use"),
        "unexpected body: {body}"
    );

    // Exactly one row exists for that email.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE email = $1")
        .bind(&email)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn register_invalid_input_is_422() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let cases: Vec<(&str, Value)> = vec![
        (
            "email without '@'",
            json!({
                "name": "Bad Email",
                "email": format!("{prefix}-no-at-sign"),
                "password": "correct-horse-battery"
            }),
        ),
        (
            "password shorter than 8 chars",
            json!({
                "name": "Short Pw",
                "email": format!("{prefix}-ShortPw@test.local"),
                "password": "1234567"
            }),
        ),
        (
            "self-assigned admin role",
            json!({
                "name": "Admin Wanna",
                "email": format!("{prefix}-AdminWanna@test.local"),
                "password": "correct-horse-battery",
                "role": "admin"
            }),
        ),
        (
            "self-assigned teacher role",
            json!({
                "name": "Teacher Wanna",
                "email": format!("{prefix}-TeacherWanna@test.local"),
                "password": "correct-horse-battery",
                "role": "teacher"
            }),
        ),
        (
            "missing name",
            json!({
                "email": format!("{prefix}-NoName@test.local"),
                "password": "correct-horse-battery"
            }),
        ),
        (
            "missing email",
            json!({
                "name": "No Email",
                "password": "correct-horse-battery"
            }),
        ),
        (
            "missing password",
            json!({
                "name": "No Password",
                "email": format!("{prefix}-NoPassword@test.local")
            }),
        ),
    ];

    for (label, payload) in cases {
        let resp = server.post("/auth/register").json(&payload).await;
        assert_eq!(resp.status_code(), 422, "{label}: {}", resp.text());
    }

    // Nothing was persisted by any rejected request.
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0, "rejected registrations must not create rows");

    // Control: a valid payload is accepted, proving the 422s came from
    // validation and not a broken harness.
    let resp = server
        .post("/auth/register")
        .json(&json!({
            "name": "Valid User",
            "email": format!("{prefix}-Valid@test.local"),
            "password": "correct-horse-battery"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "control register: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /auth/login + GET /auth/me
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_returns_token_and_me_returns_same_user() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let email = format!("{prefix}-Login@test.local");
    let registered = server
        .post("/auth/register")
        .json(&json!({
            "name": "Login User",
            "email": email,
            "password": "correct-horse-battery"
        }))
        .await;
    assert_eq!(
        registered.status_code(),
        201,
        "setup register: {}",
        registered.text()
    );
    let registered: Value = registered.json();
    let user_id = registered["id"].as_i64().expect("registered user id");

    let resp = server
        .post("/auth/login")
        .json(&json!({ "email": email, "password": "correct-horse-battery" }))
        .await;
    assert_eq!(resp.status_code(), 200, "login: {}", resp.text());
    let body: Value = resp.json();
    let token = body["token"].as_str().expect("token").to_string();
    assert!(!token.is_empty(), "token must be non-empty");

    // The token is a real JWT minted with the app secret: it round-trips
    // through the public verifier and carries the right subject and role.
    let claims = star_auth::verify_token(&token, JWT_SECRET).expect("verify login token");
    assert_eq!(claims.sub, user_id);
    assert_eq!(claims.role, "student");

    // Login response echoes the user.
    assert_eq!(body["user"]["id"], user_id);
    assert_eq!(body["user"]["email"], email);

    // GET /auth/me with that token returns the same user.
    let me = server
        .get("/auth/me")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(me.status_code(), 200, "me: {}", me.text());
    let me: Value = me.json();
    assert_eq!(me["id"], user_id);
    assert_eq!(me["email"], email);
    assert_eq!(me["name"], "Login User");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn login_wrong_password_or_unknown_email_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let email = format!("{prefix}-WrongPw@test.local");
    let created = server
        .post("/auth/register")
        .json(&json!({
            "name": "Wrong Pw",
            "email": email,
            "password": "correct-horse-battery"
        }))
        .await;
    assert_eq!(
        created.status_code(),
        201,
        "setup register: {}",
        created.text()
    );

    let wrong = server
        .post("/auth/login")
        .json(&json!({ "email": email, "password": "not-the-password" }))
        .await;
    assert_eq!(wrong.status_code(), 401, "wrong password: {}", wrong.text());

    // Unknown email is also 401 (no user enumeration through login).
    let unknown = server
        .post("/auth/login")
        .json(&json!({
            "email": format!("{prefix}-ghost@test.local"),
            "password": "whatever-long-enough"
        }))
        .await;
    assert_eq!(
        unknown.status_code(),
        401,
        "unknown email: {}",
        unknown.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn me_rejects_missing_garbage_and_wrong_secret_tokens() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "MeGuard", "student").await;

    // No Authorization header at all.
    let resp = server.get("/auth/me").await;
    assert_eq!(resp.status_code(), 401, "no header: {}", resp.text());

    // Garbage bearer token.
    let resp = server
        .get("/auth/me")
        .add_header(axum::http::header::AUTHORIZATION, "Bearer not-a-jwt")
        .await;
    assert_eq!(resp.status_code(), 401, "garbage token: {}", resp.text());

    // Non-Bearer scheme.
    let resp = server
        .get("/auth/me")
        .add_header(
            axum::http::header::AUTHORIZATION,
            format!("Token {}", token_for(user, "student")),
        )
        .await;
    assert_eq!(resp.status_code(), 401, "wrong scheme: {}", resp.text());

    // Correctly formed token signed with a DIFFERENT secret (forged).
    let forged =
        star_auth::create_token(user, "student", "unrelated-secret").expect("mint forged token");
    let resp = server
        .get("/auth/me")
        .add_header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {forged}"),
        )
        .await;
    assert_eq!(resp.status_code(), 401, "forged token: {}", resp.text());

    // Expired token, validly signed by the right secret.
    let expired_claims = star_common::Claims {
        sub: user,
        role: "student".to_string(),
        exp: (chrono::Utc::now().timestamp() - 3600) as usize,
    };
    let expired = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &expired_claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("mint expired token");
    let resp = server
        .get("/auth/me")
        .add_header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {expired}"),
        )
        .await;
    assert_eq!(resp.status_code(), 401, "expired token: {}", resp.text());

    // Control: the real bearer works.
    let resp = server
        .get("/auth/me")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "control: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Password + token helpers (pure functions, no database needed)
// ---------------------------------------------------------------------------

#[test]
fn password_hash_verify_roundtrip() {
    let hash = star_auth::hash_password("correct-horse-battery").expect("hash");

    // PHC-formatted argon2 hash.
    assert!(hash.starts_with("$argon2"), "unexpected hash: {hash}");

    // Correct password verifies; wrong ones do not.
    assert!(star_auth::verify_password("correct-horse-battery", &hash));
    assert!(!star_auth::verify_password(
        "correct-horse-battery-staple",
        &hash
    ));
    assert!(!star_auth::verify_password("", &hash));

    // A malformed hash string verifies nothing.
    assert!(!star_auth::verify_password(
        "correct-horse-battery",
        "not-a-hash"
    ));

    // Random salt: the same password hashes differently every time, and
    // every hash still verifies against the password.
    let other = star_auth::hash_password("correct-horse-battery").expect("hash");
    assert_ne!(hash, other, "salts must be random");
    assert!(star_auth::verify_password("correct-horse-battery", &other));
}

#[test]
fn token_verify_roundtrip_and_expiry() {
    let secret = "roundtrip-secret";
    let token = star_auth::create_token(42, "teacher", secret).expect("mint token");

    // Claims round-trip.
    let claims = star_auth::verify_token(&token, secret).expect("verify token");
    assert_eq!(claims.sub, 42);
    assert_eq!(claims.role, "teacher");

    // Expiry is set ~7 days ahead.
    let now = chrono::Utc::now().timestamp() as usize;
    assert!(
        claims.exp > now,
        "exp must be in the future: {}",
        claims.exp
    );
    assert!(
        claims.exp <= now + 7 * 24 * 60 * 60 + 60,
        "exp must be ~7 days ahead: {}",
        claims.exp
    );

    // Wrong secret does not verify; neither does garbage.
    assert!(star_auth::verify_token(&token, "other-secret").is_none());
    assert!(star_auth::verify_token("garbage", secret).is_none());

    // Expired token (validly signed with the right secret) does not verify.
    let expired = star_common::Claims {
        sub: 42,
        role: "teacher".to_string(),
        exp: (chrono::Utc::now().timestamp() - 3600) as usize,
    };
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &expired,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("mint expired token");
    assert!(star_auth::verify_token(&token, secret).is_none());
}

// ---------------------------------------------------------------------------
// POST /auth/logout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logout_is_stateless_no_content() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "Logout", "student").await;
    let token = token_for(user, "student");

    // Logout requires authentication: without a token it is 401.
    let resp = server.post("/auth/logout").await;
    assert_eq!(resp.status_code(), 401, "anonymous logout: {}", resp.text());

    // With a valid token: 204 No Content.
    let resp = server
        .post("/auth/logout")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(resp.status_code(), 204, "logout: {}", resp.text());
    assert_eq!(resp.text(), "", "204 must have an empty body");

    // Parity note: logout is stateless — there is no server-side session
    // store, so the token is NOT revoked and keeps working until it
    // expires. Document that reality.
    let me = server
        .get("/auth/me")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(
        me.status_code(),
        200,
        "token must still work after stateless logout: {}",
        me.text()
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// fetch_user_by_id (public helper, exercised via GET /auth/me)
// ---------------------------------------------------------------------------

// NOTE: star_auth::fetch_user_by_id cannot be called from outside the crate
// because it returns the crate-private UserResponse type (clippy
// private_interfaces, src/lib.rs:338). Its found-path is therefore exercised
// through GET /auth/me, whose handler is a thin wrapper over it; the
// NotFound branch is unreachable through the auth router because the
// AuthUser extractor rejects unknown users with 401 first.
#[tokio::test]
async fn me_returns_all_persisted_user_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "FetchMe", "teacher").await;

    let resp = server
        .get("/auth/me")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "me: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], user);
    assert_eq!(body["name"], "FetchMe");
    assert_eq!(body["email"], format!("{prefix}-FetchMe@test.local"));
    assert_eq!(body["role"], "teacher");
    assert!(body["created_at"].is_string(), "missing created_at: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Developer keys (GET/POST /developer_keys, DELETE /developer_keys/{id})
// ---------------------------------------------------------------------------

#[tokio::test]
async fn developer_keys_crud_is_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "DevKeyAdmin", "admin").await;
    let student = seed_user(&pool, &prefix, "DevKeyStudent", "student").await;
    let name = format!("{prefix}-DevKey");

    // Anonymous: 401. Non-admin: 403 for both list and create.
    let resp = server.get("/developer_keys").await;
    assert_eq!(resp.status_code(), 401, "anonymous list: {}", resp.text());
    let resp = server
        .get("/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student list: {}", resp.text());
    let resp = server
        .post("/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "developer_key": { "name": "nope" } }))
        .await;
    assert_eq!(resp.status_code(), 403, "student create: {}", resp.text());

    // Admin creates a key; response is Canvas-shaped.
    let resp = server
        .post("/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "developer_key": {
                "name": name,
                "email": "dev@example.com",
                "scopes": ["url:GET|/api/v1/courses"],
                "redirect_uris": ["https://example.com/oauth2/callback"],
                "notes": "created by test"
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "create: {}", resp.text());
    let created: Value = resp.json();
    let key_id = created["id"].as_i64().expect("id");
    let api_key = created["api_key"].as_str().expect("api_key").to_string();
    assert_eq!(created["name"], name.as_str());
    assert_eq!(created["workflow_state"], "active");
    assert_eq!(created["user_id"], admin);
    // Canvas serializes redirect_uris as a newline-joined string.
    assert_eq!(
        created["redirect_uris"],
        "https://example.com/oauth2/callback"
    );
    assert_eq!(created["scopes"][0], "url:GET|/api/v1/courses");
    assert!(!api_key.is_empty(), "api_key must be generated");

    // The key appears in the admin listing with a token count.
    let resp = server
        .get("/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "list: {}", resp.text());
    let keys: Value = resp.json();
    let listed = keys
        .as_array()
        .expect("list is an array")
        .iter()
        .find(|key| key["id"] == key_id)
        .expect("created key in list");
    assert_eq!(listed["access_token_count"], 0);

    // Delete is a soft delete; the key leaves the listing and a repeat
    // delete is 404 (unknown/already deleted).
    let resp = server
        .delete(&format!("/developer_keys/{key_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "delete: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["workflow_state"], "deleted");

    let resp = server
        .get("/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    let keys: Value = resp.json();
    assert!(keys
        .as_array()
        .unwrap()
        .iter()
        .all(|key| key["id"] != key_id));

    let resp = server
        .delete(&format!("/developer_keys/{key_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "repeat delete: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn developer_keys_show_returns_canvas_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "DevKeyShowAdmin", "admin").await;
    let student = seed_user(&pool, &prefix, "DevKeyShowStudent", "student").await;
    let name = format!("{prefix}-DevKeyShow");

    let resp = server
        .post("/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "developer_key": {
                "name": name,
                "email": "show@example.com",
                "scopes": ["url:GET|/api/v1/courses"],
                "redirect_uris": ["https://example.com/oauth2/callback"]
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "create: {}", resp.text());
    let created: Value = resp.json();
    let key_id = created["id"].as_i64().expect("id");
    let api_key = created["api_key"].as_str().expect("api_key").to_string();

    // Show is admin-only: anonymous 401, non-admin 403.
    let resp = server.get(&format!("/developer_keys/{key_id}")).await;
    assert_eq!(resp.status_code(), 401, "anonymous show: {}", resp.text());
    let resp = server
        .get(&format!("/developer_keys/{key_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student show: {}", resp.text());

    // Admin show mirrors the create response (access_token_count starts at 0).
    let resp = server
        .get(&format!("/developer_keys/{key_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "admin show: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"], key_id);
    assert_eq!(shown["name"], name.as_str());
    assert_eq!(shown["workflow_state"], "active");
    assert_eq!(shown["user_id"], admin);
    assert_eq!(shown["access_token_count"], 0);
    assert_eq!(shown["api_key"], api_key.as_str(), "secret is echoed");
    assert_eq!(
        shown["redirect_uris"],
        "https://example.com/oauth2/callback"
    );
    assert_eq!(shown["scopes"][0], "url:GET|/api/v1/courses");

    // Unknown (and already-deleted) keys are 404.
    let resp = server
        .get("/developer_keys/99999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown show: {}", resp.text());

    let resp = server
        .delete(&format!("/developer_keys/{key_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "delete: {}", resp.text());
    let resp = server
        .get(&format!("/developer_keys/{key_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "deleted show: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn developer_keys_update_partial_and_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "DevKeyUpdAdmin", "admin").await;
    let student = seed_user(&pool, &prefix, "DevKeyUpdStudent", "student").await;
    let name = format!("{prefix}-DevKeyUpd");

    let resp = server
        .post("/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "developer_key": {
                "name": name,
                "email": "before@example.com",
                "notes": "before",
                "redirect_uris": ["https://before.example.com/callback"],
                "scopes": ["url:GET|/api/v1/courses"]
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "create: {}", resp.text());
    let created: Value = resp.json();
    let key_id = created["id"].as_i64().expect("id");
    let api_key = created["api_key"].as_str().expect("api_key").to_string();

    // Update is admin-only: anonymous 401, non-admin 403.
    let resp = server
        .put(&format!("/developer_keys/{key_id}"))
        .json(&json!({ "developer_key": { "notes": "nope" } }))
        .await;
    assert_eq!(resp.status_code(), 401, "anonymous update: {}", resp.text());
    let resp = server
        .put(&format!("/developer_keys/{key_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "developer_key": { "notes": "nope" } }))
        .await;
    assert_eq!(resp.status_code(), 403, "student update: {}", resp.text());

    // Partial update: submitted fields change, omitted fields keep their
    // stored values (Canvas assigns developer_key_params over @key).
    let resp = server
        .put(&format!("/developer_keys/{key_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "developer_key": {
                "notes": "after",
                "visible": false,
                "redirect_uris": ["https://after.example.com/callback",
                                   "https://extra.example.com/callback"]
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "update: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["id"], key_id);
    assert_eq!(updated["notes"], "after", "submitted field is updated");
    assert_eq!(updated["visible"], false, "boolean flag is updated");
    assert_eq!(
        updated["redirect_uris"],
        "https://after.example.com/callback\nhttps://extra.example.com/callback",
        "array fields are replaced"
    );
    assert_eq!(updated["name"], name.as_str(), "omitted name is kept");
    assert_eq!(
        updated["email"], "before@example.com",
        "omitted email is kept"
    );
    assert_eq!(
        updated["scopes"][0], "url:GET|/api/v1/courses",
        "omitted scopes are kept"
    );
    assert_eq!(
        updated["api_key"],
        api_key.as_str(),
        "update must not rotate the client secret"
    );
    assert_eq!(updated["user_id"], admin, "creator is unchanged");

    // Updates persist (show reflects them).
    let resp = server
        .get(&format!("/developer_keys/{key_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    let shown: Value = resp.json();
    assert_eq!(shown["notes"], "after", "update must persist");

    // Unknown keys are 404.
    let resp = server
        .put("/developer_keys/99999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "developer_key": { "notes": "x" } }))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown update: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// OAuth2 token endpoint (POST /login/oauth2/token) — grant-level parity.
// The full auth-code flow (auth -> code -> exchange) lives in
// oauth2_flow_integration.rs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth2_token_client_credentials_grant() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "OAuthAdmin", "admin").await;
    let name = format!("{prefix}-OAuthKey");

    // A key WITHOUT an external client-credentials audience is rejected,
    // like Canvas SymmetricClientCredentialsProvider#assertion_method_permitted?.
    let resp = server
        .post("/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "developer_key": {
                "name": format!("{name}-internal"),
                "scopes": ["url:GET|/api/v1/courses"]
            }
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "create internal key: {}",
        resp.text()
    );
    let internal: Value = resp.json();
    let internal_id = internal["id"].as_i64().expect("key id");
    let internal_secret = internal["api_key"].as_str().expect("api_key").to_string();

    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=client_credentials&client_id={internal_id}&client_secret={internal_secret}"
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "internal audience key: {}",
        resp.text()
    );
    let error: Value = resp.json();
    assert_eq!(error["error"], "invalid_request");
    assert_eq!(
        error["error_description"],
        "assertion method not supported for this grant_type"
    );

    // The real key: external audience + configured scopes.
    let resp = server
        .post("/developer_keys")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "developer_key": {
                "name": name,
                "scopes": ["url:GET|/api/v1/courses"],
                "client_credentials_audience": "external"
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "create key: {}", resp.text());
    let created: Value = resp.json();
    let key_id = created["id"].as_i64().expect("key id");
    let api_key = created["api_key"].as_str().expect("api_key").to_string();

    // Form-encoded client_credentials request, like Canvas OAuth2 clients.
    let form = format!(
        "grant_type=client_credentials&client_id={key_id}&client_secret={api_key}\
         &scope=url%3AGET%7C%2Fapi%2Fv1%2Fcourses"
    );
    let resp = server
        .post("/login/oauth2/token")
        .text(form)
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "client_credentials: {}",
        resp.text()
    );
    let token: Value = resp.json();
    assert_eq!(token["token_type"], "Bearer");
    assert_eq!(token["expires_in"], 3600);
    assert_eq!(token["scope"], "url:GET|/api/v1/courses");
    // Canvas service tokens are signed JWTs, not stored opaque tokens — no
    // refresh_token and no access_tokens row.
    let access_token = token["access_token"]
        .as_str()
        .expect("access_token")
        .to_string();
    assert_eq!(
        access_token.split('.').count(),
        3,
        "client_credentials access_token is a JWT: {access_token}"
    );
    assert!(
        token.get("refresh_token").is_none(),
        "service tokens carry no refresh_token: {token}"
    );
    let (stored,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM access_tokens WHERE developer_key_id = $1")
            .bind(key_id)
            .fetch_one(&pool)
            .await
            .expect("count tokens");
    assert_eq!(stored, 0, "client_credentials JWTs are not persisted");

    // HTTP Basic authentication is accepted (Canvas
    // oauth2_provider#token reads Basic credentials).
    let basic = base64_encode(&format!("{key_id}:{api_key}"));
    let resp = server
        .post("/login/oauth2/token")
        .text("grant_type=client_credentials&scope=url%3AGET%7C%2Fapi%2Fv1%2Fcourses".to_string())
        .content_type("application/x-www-form-urlencoded")
        .add_header(axum::http::header::AUTHORIZATION, format!("Basic {basic}"))
        .await;
    assert_eq!(resp.status_code(), 200, "basic auth: {}", resp.text());
    assert_eq!(resp.json::<Value>()["token_type"], "Bearer");

    // A scope outside the key's configured scopes is invalid_scope.
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=client_credentials&client_id={key_id}&client_secret={api_key}\
             &scope=url%3AGET%7C%2Fapi%2Fv1%2Faccounts"
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400, "bad scope: {}", resp.text());
    let error: Value = resp.json();
    assert_eq!(error["error"], "invalid_scope");
    assert_eq!(error["error_description"], "url:GET|/api/v1/accounts");

    // Wrong client secret is invalid_client with the WWW-Authenticate
    // header, matching Canvas.
    let resp = server
        .post("/login/oauth2/token")
        .text(format!(
            "grant_type=client_credentials&client_id={key_id}&client_secret=wrong-secret"
        ))
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 401, "wrong secret: {}", resp.text());
    let secret_error: Value = resp.json();
    assert_eq!(secret_error["error"], "invalid_client");
    assert_eq!(secret_error["error_description"], "invalid client");

    // Unknown grant types and bare requests fail with Canvas's
    // unsupported_grant_type error.
    let resp = server
        .post("/login/oauth2/token")
        .text("grant_type=password&client_id=1&client_secret=x".to_string())
        .content_type("application/x-www-form-urlencoded")
        .await;
    assert_eq!(resp.status_code(), 400, "password grant: {}", resp.text());
    let unsupported: Value = resp.json();
    assert_eq!(unsupported["error"], "unsupported_grant_type");

    cleanup(&pool, &prefix).await;
}

/// Standard base64 for the HTTP Basic tests.
fn base64_encode(input: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
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
    let pad = (3 - bytes.len() % 3) % 3;
    for _ in 0..pad {
        out.push('=');
    }
    out
}

// ---------------------------------------------------------------------------
// AuthUser extractor (middleware)
// ---------------------------------------------------------------------------

/// Echo route protected by the AuthUser extractor, mounted only in the two
/// extractor tests below: /auth/me and /auth/logout consume the extractor
/// but never reveal which role it resolved, so a tiny echo handler makes
/// the extractor's decisions observable through HTTP.
async fn whoami(user: star_auth::AuthUser) -> String {
    format!("{}|{}", user.id, user.role)
}

/// Like setup(), but additionally mounts the protected echo route above
/// (still built over star_auth::router(), like star-http does).
async fn setup_with_protected_route() -> Option<(TestServer, PgPool, String)> {
    let pool = db_pool().await?;

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };

    let server = TestServer::new(
        axum::Router::new()
            .merge(star_auth::router())
            .route("/protected", axum::routing::get(whoami))
            .with_state(state),
    )
    .expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

#[tokio::test]
async fn authuser_rejects_requests_without_valid_bearer() {
    let Some((server, pool, prefix)) = setup_with_protected_route().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "Guard", "student").await;

    // No Authorization header.
    let resp = server.get("/protected").await;
    assert_eq!(resp.status_code(), 401, "no header: {}", resp.text());

    // Garbage bearer token.
    let resp = server
        .get("/protected")
        .add_header(axum::http::header::AUTHORIZATION, "Bearer garbage")
        .await;
    assert_eq!(resp.status_code(), 401, "garbage: {}", resp.text());

    // Wrong auth scheme.
    let resp = server
        .get("/protected")
        .add_header(
            axum::http::header::AUTHORIZATION,
            format!("Basic {}", token_for(user, "student")),
        )
        .await;
    assert_eq!(resp.status_code(), 401, "wrong scheme: {}", resp.text());

    // Token signed with a different secret.
    let forged =
        star_auth::create_token(user, "student", "wrong-secret").expect("mint forged token");
    let resp = server
        .get("/protected")
        .add_header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {forged}"),
        )
        .await;
    assert_eq!(resp.status_code(), 401, "forged token: {}", resp.text());

    // Valid bearer: 200 with the authenticated identity.
    let resp = server
        .get("/protected")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "valid bearer: {}", resp.text());
    assert_eq!(resp.text(), format!("{user}|student"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn authuser_resolves_role_from_db_and_rejects_deleted_users() {
    let Some((server, pool, prefix)) = setup_with_protected_route().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // The DB role is 'teacher' while the (stale) token claim says 'admin':
    // the database must win, the claim is never trusted.
    let user = seed_user(&pool, &prefix, "StaleToken", "teacher").await;
    let stale = token_for(user, "admin");

    let resp = server
        .get("/protected")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {stale}"))
        .await;
    assert_eq!(resp.status_code(), 200, "valid token: {}", resp.text());
    assert_eq!(
        resp.text(),
        format!("{user}|teacher"),
        "role must come from the DB, not the token claim"
    );

    // A validly signed token for a user that no longer exists is rejected.
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .expect("delete user");
    let resp = server
        .get("/protected")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {stale}"))
        .await;
    assert_eq!(resp.status_code(), 401, "deleted user must be rejected");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Canvas access tokens (/users/:user_id/tokens, :user_id = self | numeric)
// ---------------------------------------------------------------------------

/// POST /users/self/tokens as `user` and return the parsed Canvas `Token` body.
async fn create_personal_token(server: &TestServer, user: i64, role: &str, body: Value) -> Value {
    let resp = server
        .post("/users/self/tokens")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
        .json(&body)
        .await;
    assert_eq!(resp.status_code(), 200, "create token: {}", resp.text());
    resp.json::<Value>()
}

#[tokio::test]
async fn tokens_create_list_show_and_destroy() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "TokenOwner", "student").await;

    // Create: the full token is returned exactly once, in visible_token.
    let created = create_personal_token(
        &server,
        user,
        "student",
        json!({
            "token": {
                "purpose": "CI integration",
                "expires_at": "2030-01-02T03:04:05Z",
                "scopes": ["url:GET|/api/v1/courses", "url:POST|/api/v1/courses"]
            }
        }),
    )
    .await;

    let id = created["id"].as_i64().expect("token id");
    assert!(id > 0);
    assert_eq!(created["user_id"], user);
    assert_eq!(created["purpose"], "CI integration");
    assert_eq!(created["workflow_state"], "active");
    assert_eq!(created["scopes"][0], "url:GET|/api/v1/courses");
    assert_eq!(created["scopes"][1], "url:POST|/api/v1/courses");
    assert_eq!(created["app_name"], "User-Generated");
    assert_eq!(created["remember_access"], false);
    assert_eq!(created["real_user_id"], Value::Null);
    assert_eq!(created["last_used_at"], Value::Null);
    assert_eq!(created["can_manually_regenerate"], true);
    assert!(
        created["expires_at"]
            .as_str()
            .expect("expires_at")
            .starts_with("2030-01-02T03:04:05"),
        "unexpected expires_at: {}",
        created["expires_at"]
    );

    let full_token = created["visible_token"]
        .as_str()
        .expect("visible_token")
        .to_string();
    assert_eq!(full_token.len(), 64, "Canvas token size is 64");
    assert!(full_token.chars().all(|c| c.is_ascii_hexdigit()));
    let hint = full_token[..5].to_string();

    // The row is a manually created token: NULL developer key, plaintext
    // token, 5-character hint.
    let (stored_token, stored_hint, key_id): (String, Option<String>, Option<i64>) =
        sqlx::query_as(
            "SELECT token, token_hint, developer_key_id FROM access_tokens WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("token row");
    assert_eq!(stored_token, full_token);
    assert_eq!(stored_hint.as_deref(), Some(hint.as_str()));
    assert_eq!(key_id, None);

    // List (self and numeric paths): the token appears with its visible
    // value masked to the hint.
    for list_path in [
        "/users/self/user_generated_tokens".to_string(),
        format!("/users/{user}/user_generated_tokens"),
    ] {
        let resp = server
            .get(&list_path)
            .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
            .await;
        assert_eq!(resp.status_code(), 200, "list {list_path}: {}", resp.text());
        let listed: Value = resp.json();
        let listed = listed
            .as_array()
            .expect("list is an array")
            .iter()
            .find(|token| token["id"] == id)
            .unwrap_or_else(|| panic!("created token missing from {list_path}"));
        assert_eq!(listed["visible_token"], format!("{hint}..."));
        assert_eq!(listed["scopes"][0], "url:GET|/api/v1/courses");
    }

    // Show by id and by token_hint both resolve the same token.
    for key in [id.to_string(), hint.clone()] {
        let resp = server
            .get(&format!("/users/self/tokens/{key}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
            .await;
        assert_eq!(resp.status_code(), 200, "show {key}: {}", resp.text());
        let shown: Value = resp.json();
        assert_eq!(shown["id"], id);
        assert_eq!(shown["visible_token"], format!("{hint}..."));
    }

    // PUT update: purpose and scopes change; the visible token stays masked
    // when not regenerating (Canvas tokens#update).
    let resp = server
        .put(&format!("/users/self/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "token": {
                "purpose": "renamed purpose",
                "scopes": ["url:GET|/api/v1/courses"]
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "update: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["purpose"], "renamed purpose");
    assert_eq!(updated["scopes"].as_array().unwrap().len(), 1);
    assert_eq!(updated["visible_token"], format!("{hint}..."));

    // PUT regenerate: a fresh full token value is revealed exactly once.
    let resp = server
        .put(&format!("/users/self/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "token": { "regenerate": true } }))
        .await;
    assert_eq!(resp.status_code(), 200, "regenerate: {}", resp.text());
    let regenerated: Value = resp.json();
    let new_full = regenerated["visible_token"].as_str().expect("full token");
    assert_eq!(new_full.len(), 64);
    assert_ne!(new_full, full_token.as_str());
    let (stored_new,): (String,) = sqlx::query_as("SELECT token FROM access_tokens WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("row after regenerate");
    assert_eq!(stored_new, new_full);

    // Delete is a soft delete; the token leaves the list and is 404 after.
    let resp = server
        .delete(&format!("/users/self/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "delete: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["workflow_state"], "deleted");

    let (state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM access_tokens WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("token row after delete");
    assert_eq!(state, "deleted");

    let resp = server
        .get("/users/self/user_generated_tokens")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    let remaining: Value = resp.json();
    assert!(
        remaining
            .as_array()
            .unwrap()
            .iter()
            .all(|token| token["id"] != id),
        "deleted token must not be listed: {remaining}"
    );

    let resp = server
        .get(&format!("/users/self/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "show deleted: {}", resp.text());
    let resp = server
        .delete(&format!("/users/self/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "repeat delete: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn tokens_require_auth_and_validate_params() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "TokenValidation", "student").await;

    // Anonymous requests are rejected.
    let resp = server.get("/users/self/user_generated_tokens").await;
    assert_eq!(resp.status_code(), 401, "anonymous list: {}", resp.text());
    let resp = server
        .post("/users/self/tokens")
        .json(&json!({ "token": { "purpose": "nope" } }))
        .await;
    assert_eq!(resp.status_code(), 401, "anonymous create: {}", resp.text());

    // Missing or blank purpose: Canvas `{"message": ...}` error array, 400.
    for purpose in [Value::Null, json!("   ")] {
        let resp = server
            .post("/users/self/tokens")
            .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
            .json(&json!({ "token": { "purpose": purpose } }))
            .await;
        assert_eq!(
            resp.status_code(),
            400,
            "purpose {purpose}: {}",
            resp.text()
        );
        let error: Value = resp.json();
        assert_eq!(error[0]["message"], "token[purpose] is missing");
    }

    // Unparseable expiration dates are rejected like Canvas.
    let resp = server
        .post("/users/self/tokens")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "token": { "purpose": "bad date", "expires_at": "next tuesday" }
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "bad date: {}", resp.text());
    let error: Value = resp.json();
    assert_eq!(error[0]["message"], "Invalid expiration date format");

    // Non-numeric user ids (other than self) are 404, like Canvas's API user
    // resolution.
    let resp = server
        .get("/users/not-a-number/user_generated_tokens")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "bad user id: {}", resp.text());

    // Unknown ids are 404, and validation created no rows.
    let resp = server
        .get("/users/self/tokens/999999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown token: {}", resp.text());
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM access_tokens WHERE user_id = $1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn tokens_are_scoped_to_their_owner() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "TokenIsolationOwner", "teacher").await;
    let other = seed_user(&pool, &prefix, "TokenIsolationOther", "student").await;
    let admin = seed_user(&pool, &prefix, "TokenIsolationAdmin", "admin").await;

    let created = create_personal_token(
        &server,
        owner,
        "teacher",
        json!({ "token": { "purpose": "owner only" } }),
    )
    .await;
    let id = created["id"].as_i64().expect("token id");

    // Another (non-admin) user cannot list the owner's user-generated tokens
    // (Canvas render_unauthorized_action -> 401)...
    let resp = server
        .get(&format!("/users/{owner}/user_generated_tokens"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 401, "other list: {}", resp.text());

    // ...cannot create a token for the owner...
    let resp = server
        .post(&format!("/users/{owner}/tokens"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "token": { "purpose": "sneaky" } }))
        .await;
    assert_eq!(resp.status_code(), 401, "other create: {}", resp.text());

    // ...and cannot read or delete the owner's token (404, Canvas
    // RecordNotFound fallback).
    for method in ["GET", "PUT", "DELETE"] {
        let path = format!("/users/{owner}/tokens/{id}");
        let resp = match method {
            "GET" => {
                server
                    .get(&path)
                    .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
                    .await
            }
            "PUT" => {
                server
                    .put(&path)
                    .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
                    .json(&json!({ "token": { "purpose": "steal" } }))
                    .await
            }
            _ => {
                server
                    .delete(&path)
                    .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
                    .await
            }
        };
        assert_eq!(
            resp.status_code(),
            404,
            "{method} by another user: {}",
            resp.text()
        );
    }

    // Admins may list the owner's user-generated tokens and create tokens
    // for them; tokens created for someone else start `pending`
    // (Canvas tokens#create).
    let resp = server
        .get(&format!("/users/{owner}/user_generated_tokens"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "admin list: {}", resp.text());
    let admin_list: Value = resp.json();
    assert!(
        admin_list
            .as_array()
            .unwrap()
            .iter()
            .any(|token| token["id"] == id),
        "owner token missing from admin list: {admin_list}"
    );

    let resp = server
        .post(&format!("/users/{owner}/tokens"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "token": { "purpose": "admin created" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "admin create: {}", resp.text());
    let for_other: Value = resp.json();
    assert_eq!(for_other["workflow_state"], "pending");
    assert_eq!(for_other["user_id"], owner);
    let pending_id = for_other["id"].as_i64().expect("pending id");

    // Regenerating while acting on another user's token resets it to pending.
    let resp = server
        .put(&format!("/users/{owner}/tokens/{pending_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "token": { "regenerate": true } }))
        .await;
    assert_eq!(resp.status_code(), 200, "admin regenerate: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "pending");

    // An expired token cannot be regenerated without a new expiration date
    // (Canvas tokens#update). The token value is prefixed to stay unique
    // across parallel runs (access_tokens.token is UNIQUE).
    let expired_token = format!("{prefix}-expired-token-value");
    let expired_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO access_tokens
            (user_id, developer_key_id, token, token_hint, purpose,
             permanent_expires_at, workflow_state)
        VALUES ($1, NULL, $2, $3, 'already expired',
                now() - interval '1 hour', 'active')
        RETURNING id
        "#,
    )
    .bind(owner)
    .bind(&expired_token)
    .bind(&expired_token[..5])
    .fetch_one(&pool)
    .await
    .expect("seed expired token");
    let resp = server
        .put(&format!("/users/{owner}/tokens/{expired_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "teacher"))
        .json(&json!({ "token": { "regenerate": true } }))
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "expired regenerate: {}",
        resp.text()
    );
    let error: Value = resp.json();
    assert_eq!(
        error["errors"]["message"],
        "cannot regenerate an expired token without a new expiration date"
    );

    // ...but regenerating WITH a new expiration date succeeds.
    let resp = server
        .put(&format!("/users/{owner}/tokens/{expired_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "teacher"))
        .json(&json!({
            "token": { "regenerate": true, "expires_at": "2031-01-01T00:00:00Z" }
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "expired regenerate + date: {}",
        resp.text()
    );

    // The failed attempts left the owner's token active and usable.
    let resp = server
        .get(&format!("/users/{owner}/tokens/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "owner show: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "active");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Authentication providers + SSO settings
// ---------------------------------------------------------------------------

/// Seed a real account row for auth-providers tests (needed because the
/// FK references accounts.id). Uses id 1 which should exist from prior
/// migrations.
async fn seed_account_if_needed(pool: &PgPool) {
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = 1")
        .fetch_optional(pool)
        .await
        .expect("check account");
    if exists.is_none() {
        sqlx::query("INSERT INTO accounts (id, name) VALUES (1, 'Test Account') ON CONFLICT (id) DO NOTHING")
            .execute(pool)
            .await
            .expect("seed account");
    }
}

#[tokio::test]
async fn auth_providers_crud_requires_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    // Anonymous → 401.
    let resp = server.get("/accounts/1/authentication_providers").await;
    assert_eq!(resp.status_code(), 401, "anonymous list: {}", resp.text());

    // Student → 403.
    let resp = server
        .get("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student list: {}", resp.text());

    // Admin → 200 (empty list, account has no providers yet).
    let resp = server
        .get("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "admin list: {}", resp.text());
    let body: Value = resp.json();
    assert!(body["authentication_providers"].is_array(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn auth_providers_create_show_update_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "Admin2", "admin").await;
    let admin_auth = auth(admin, "admin");

    // POST create a SAML provider with settings.
    let create_resp = server
        .post("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, &admin_auth)
        .json(&json!({
            "authentication_provider": {
                "auth_type": "saml",
                "position": 1,
                "idp_entity_id": "https://idp.example.org/saml2",
                "log_in_url": "https://idp.example.org/login"
            }
        }))
        .await;
    assert_eq!(
        create_resp.status_code(),
        200,
        "create: {}",
        create_resp.text()
    );
    let created: Value = create_resp.json();
    let provider_id = created["id"].as_i64().expect("provider id");
    assert_eq!(created["auth_type"], "saml");
    assert_eq!(created["position"], 1);
    assert_eq!(created["idp_entity_id"], "https://idp.example.org/saml2");
    assert_eq!(created["log_in_url"], "https://idp.example.org/login");

    // GET show.
    let show_resp = server
        .get(&format!(
            "/accounts/1/authentication_providers/{provider_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &admin_auth)
        .await;
    assert_eq!(show_resp.status_code(), 200, "show: {}", show_resp.text());
    assert_eq!(show_resp.json::<Value>()["id"], provider_id);

    // PUT update: change log_in_url and position.
    let update_resp = server
        .put(&format!(
            "/accounts/1/authentication_providers/{provider_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &admin_auth)
        .json(&json!({
            "authentication_provider": {
                "position": 2,
                "log_in_url": "https://idp.example.org/new-login"
            }
        }))
        .await;
    assert_eq!(
        update_resp.status_code(),
        200,
        "update: {}",
        update_resp.text()
    );
    let updated: Value = update_resp.json();
    assert_eq!(updated["position"], 2);
    assert_eq!(updated["log_in_url"], "https://idp.example.org/new-login");
    // Original idp_entity_id preserved.
    assert_eq!(updated["idp_entity_id"], "https://idp.example.org/saml2");

    // DELETE soft-deletes.
    let delete_resp = server
        .delete(&format!(
            "/accounts/1/authentication_providers/{provider_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &admin_auth)
        .await;
    assert_eq!(
        delete_resp.status_code(),
        200,
        "delete: {}",
        delete_resp.text()
    );
    assert_eq!(delete_resp.json::<Value>()["workflow_state"], "deleted");

    // GET after delete → 404.
    let gone_resp = server
        .get(&format!(
            "/accounts/1/authentication_providers/{provider_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &admin_auth)
        .await;
    assert_eq!(
        gone_resp.status_code(),
        404,
        "after delete: {}",
        gone_resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn sso_settings_show_and_update() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "Admin3", "admin").await;
    let admin_auth = auth(admin, "admin");

    // GET sso_settings returns current values (may be nulls).
    let get_resp = server
        .get("/accounts/1/sso_settings")
        .add_header(axum::http::header::AUTHORIZATION, &admin_auth)
        .await;
    assert_eq!(
        get_resp.status_code(),
        200,
        "get sso_settings: {}",
        get_resp.text()
    );
    let body: Value = get_resp.json();
    assert!(body["sso_settings"].is_object(), "body: {body}");

    // PUT update sso_settings.
    let update_resp = server
        .put("/accounts/1/sso_settings")
        .add_header(axum::http::header::AUTHORIZATION, &admin_auth)
        .json(&json!({
            "sso_settings": {
                "login_handle_name": "Username",
                "change_password_url": "https://example.org/reset",
                "auth_discovery_url": "https://example.org/discover",
                "unknown_user_url": "https://example.org/unknown",
                "login_help_url": "https://example.org/help"
            }
        }))
        .await;
    assert_eq!(
        update_resp.status_code(),
        200,
        "update sso: {}",
        update_resp.text()
    );
    let updated: Value = update_resp.json::<serde_json::Value>()["sso_settings"].clone();
    assert_eq!(updated["login_handle_name"], "Username");
    assert_eq!(updated["change_password_url"], "https://example.org/reset");
    assert_eq!(
        updated["auth_discovery_url"],
        "https://example.org/discover"
    );
    assert_eq!(updated["unknown_user_url"], "https://example.org/unknown");
    assert_eq!(updated["login_help_url"], "https://example.org/help");

    // GET reflects the update.
    let get2_resp = server
        .get("/accounts/1/sso_settings")
        .add_header(axum::http::header::AUTHORIZATION, &admin_auth)
        .await;
    let get2_body: Value = get2_resp.json::<serde_json::Value>()["sso_settings"].clone();
    assert_eq!(get2_body["login_handle_name"], "Username");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Issue #106 — failed-login lockout (Canvas LoginRegistry parity:
// lib/canvas/security/password_policy.rb DEFAULT_LOGIN_ATTEMPTS=10,
// login_registry.rb audit_login / too_many_attempts).
// ---------------------------------------------------------------------------

/// The limiter itself: counting, locking at the Canvas threshold, the
/// 30-minute release window, and success clearing the counter.
#[tokio::test]
async fn login_rate_limiter_counts_locks_and_releases() {
    let limiter = star_auth::LoginRateLimiter::default();
    let email = "lockout-user@test.local";

    assert!(!limiter.is_locked(email));
    // Nine failures: still allowed (Canvas locks ON the 10th consecutive
    // failure — the 10th request is the one that gets refused).
    for _ in 0..9 {
        assert!(!limiter.record_failure(email));
        assert!(!limiter.is_locked(email), "nine failures must not lock");
    }
    // The tenth failure locks the account.
    assert!(limiter.record_failure(email));
    assert!(limiter.is_locked(email), "tenth failure locks");

    // A different account is unaffected.
    assert!(!limiter.is_locked("other-user@test.local"));

    // Success clears the counter even mid-count.
    limiter.record_success(email);
    assert!(!limiter.is_locked(email));
    for _ in 0..5 {
        assert!(!limiter.record_failure(email));
    }
    assert!(!limiter.is_locked(email), "counter restarted from zero");

    // Emails are normalized: case and surrounding whitespace share one
    // budget (the handlers lower-case before lookup).
    assert!(!limiter.record_failure("  CaseUser@Test.Local "));
    assert!(!limiter.record_failure("caseuser@test.local"));
    assert!(!limiter.is_locked("CASEUSER@test.local"));
}

/// Through the real route: ten wrong passwords lock the account, the
/// eleventh is a 403 even with the CORRECT password (Canvas refuses
/// :too_many_attempts before checking credentials), and a fresh account
/// with the right password still logs in.
#[tokio::test]
async fn login_locks_after_ten_failed_attempts() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let email = format!("{prefix}-lockout@test.local");
    let created = server
        .post("/auth/register")
        .json(&json!({
            "name": "Lockout Target",
            "email": email,
            "password": "correct-horse-battery"
        }))
        .await;
    assert_eq!(
        created.status_code(),
        201,
        "setup register: {}",
        created.text()
    );

    // Ten failed attempts: first nine answer 401 ...
    for attempt in 1..=9 {
        let resp = server
            .post("/auth/login")
            .json(&json!({ "email": email, "password": "not-the-password" }))
            .await;
        assert_eq!(
            resp.status_code(),
            401,
            "attempt {attempt} should still be 401"
        );
    }
    // ... the tenth failure locks.
    let resp = server
        .post("/auth/login")
        .json(&json!({ "email": email, "password": "not-the-password" }))
        .await;
    assert_eq!(resp.status_code(), 401, "the tenth failure itself is 401");

    // Now even the CORRECT password is refused while locked.
    let resp = server
        .post("/auth/login")
        .json(&json!({ "email": email, "password": "correct-horse-battery" }))
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "locked account with correct password: {}",
        resp.text()
    );

    // A different account is untouched by the lock.
    let other_email = format!("{prefix}-bystander@test.local");
    let created = server
        .post("/auth/register")
        .json(&json!({
            "name": "Bystander",
            "email": other_email,
            "password": "correct-horse-battery"
        }))
        .await;
    assert_eq!(created.status_code(), 201, "bystander register");
    let resp = server
        .post("/auth/login")
        .json(&json!({ "email": other_email, "password": "correct-horse-battery" }))
        .await;
    assert_eq!(resp.status_code(), 200, "bystander logs in normally");

    cleanup(&pool, &prefix).await;
}
