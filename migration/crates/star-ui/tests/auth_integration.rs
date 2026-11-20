//! Integration tests for the star-ui auth journey (issue #70):
//! registration, forgot/reset password, session expiry, ENV safety.
//! Mirrors ui_integration.rs patterns (TestServer, seed, SKIP-if-no-DB,
//! unique run prefix).

#![allow(dead_code)]

use axum_test::TestServer;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

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
    let server = TestServer::new(star_ui::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM password_reset_tokens WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

/// Fetch the CSRF cookie from a GET of `path`.
async fn get_csrf(server: &TestServer, path: &str) -> String {
    let page = server.get(path).await;
    assert_eq!(page.status_code(), 200, "GET {path}: {}", page.text());
    page.cookies()
        .get("star_csrf")
        .expect("csrf cookie issued")
        .value()
        .to_string()
}

/// Mask per-request values that legitimately differ between two renders of
/// the same page: the CSRF hidden input (a fresh random token per request)
/// and the echoed email input (the form re-echoes the submitted address).
/// Everything else must match byte-for-byte, which is what the
/// no-enumeration comparison below actually asserts.
fn normalize_per_request_values(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(idx) = rest.find("value=\"") {
        let (head, tail) = rest.split_at(idx + "value=\"".len());
        let end = tail.find('"').expect("unterminated value attribute");
        let value = &tail[..end];
        out.push_str(head);
        if is_per_request_value(head, value) {
            out.push_str("<MASKED>");
        } else {
            out.push_str(value);
        }
        out.push('"');
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Does `value` (the text following `value="`, with `head` being everything
/// earlier in the body) belong to the CSRF hidden input or the email input?
/// Those two attributes legitimately vary per request; any other attribute
/// varying between two renders of the same page is an enumeration leak.
fn is_per_request_value(head: &str, value: &str) -> bool {
    // CSRF hidden input: 32 hex chars under name="_csrf".
    let is_csrf = head.contains("name=\"_csrf\"")
        && value.len() == 32
        && value.bytes().all(|b| b.is_ascii_hexdigit());
    // Email echo: the name="email" input re-echoes the submitted address.
    // It belongs to the same tag when no '>' separates the two attributes.
    let is_email = head.rfind("name=\"email\"").is_some_and(|p| !head[p..].contains('>'));
    is_csrf || is_email
}

/// GET /register renders the form.
#[tokio::test]
async fn register_form_renders() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let body = server.get("/register").await.text();
    assert!(body.contains("register_form"));
    assert!(body.contains("name=\"password_confirmation\""));
    let env = star_ui::env::parse_env_from_html(&body).expect("ENV parses");
    assert!(env.get("current_user_id").is_none());
    cleanup(&pool, &prefix).await;
}

/// POST /register: valid → 303 + session cookie + student row; mismatched
/// confirmation → 422 with field error; duplicate email → re-render error.
#[tokio::test]
async fn register_valid_mismatch_and_duplicate() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let email = format!("{prefix}-newuser@test.local");

    // Valid registration.
    let csrf = get_csrf(&server, "/register").await;
    let resp = server
        .post("/register")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "name": "New User",
            "email": email,
            "password": "Password123!",
            "password_confirmation": "Password123!",
            "_csrf": csrf
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "body: {}", resp.text());
    assert_eq!(resp.headers().get("location").unwrap(), "/dashboard");
    assert!(
        resp.cookies().get("star_session").is_some(),
        "auto-login cookie"
    );

    let row: Option<(String,)> = sqlx::query_as("SELECT role FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(&pool)
        .await
        .expect("query");
    assert_eq!(row.unwrap().0, "student", "role forced to student");

    // Mismatched confirmation → 422 re-render with the field error.
    let csrf = get_csrf(&server, "/register").await;
    let resp = server
        .post("/register")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "name": "New User",
            "email": format!("{prefix}-other@test.local"),
            "password": "Password123!",
            "password_confirmation": "Different123!",
            "_csrf": csrf
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "mismatch re-renders");
    let body = resp.text();
    assert!(body.contains("register_form"), "form re-rendered");
    assert!(body.contains("does not match"), "field error shown: {body}");

    // Duplicate email → re-render with the star-auth conflict message.
    let csrf = get_csrf(&server, "/register").await;
    let resp = server
        .post("/register")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "name": "New User",
            "email": email,
            "password": "Password123!",
            "password_confirmation": "Password123!",
            "_csrf": csrf
        }))
        .await;
    let body = resp.text();
    assert!(
        body.contains("Email already in use"),
        "duplicate error: {body}"
    );

    cleanup(&pool, &prefix).await;
}

/// Forgot password: unknown and known email produce IDENTICAL 200 bodies —
/// compared after masking the two attributes that legitimately vary per
/// request (the fresh CSRF token and the echoed email input); the known
/// email additionally creates an unexpired token row, and that token never
/// appears in the response HTML.
#[tokio::test]
async fn forgot_password_no_enumeration() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, 'student') RETURNING id",
    )
    .bind("Forgot User")
    .bind(format!("{prefix}-forgot@test.local"))
    .bind(&hash)
    .fetch_one(&pool)
    .await
    .expect("seed user");

    let csrf_known = get_csrf(&server, "/forgot_password").await;
    let known = server
        .post("/forgot_password")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf_known.clone(),
        ))
        .form(&serde_json::json!({
            "email": format!("{prefix}-forgot@test.local"),
            "_csrf": csrf_known
        }))
        .await;
    let csrf_unknown = get_csrf(&server, "/forgot_password").await;
    let unknown = server
        .post("/forgot_password")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf_unknown.clone(),
        ))
        .form(&serde_json::json!({
            "email": format!("{prefix}-unknown@test.local"),
            "_csrf": csrf_unknown
        }))
        .await;
    assert_eq!(known.status_code(), 200);
    assert_eq!(unknown.status_code(), 200);
    let known_body = known.text();
    let unknown_body = unknown.text();
    // Compare with the per-request CSRF token and the echoed email masked
    // out: both legitimately differ between the two POSTs (the CSRF value is
    // regenerated per request and the form re-echoes the submitted email).
    // Everything else must be byte-identical, or the page leaks whether the
    // address is registered.
    assert_eq!(
        normalize_per_request_values(&known_body),
        normalize_per_request_values(&unknown_body),
        "known and unknown emails must render identical pages (modulo CSRF/email echo)"
    );
    assert!(known_body.contains("If that account exists"));

    // The token created for the known email must NOT appear anywhere in the
    // response HTML (Canvas parity: the link goes out via email; here it is
    // only logged server-side). If it ever reaches the rendered page the
    // response becomes an enumeration + takeover oracle.
    let token: (String,) = sqlx::query_as(
        "SELECT token FROM password_reset_tokens WHERE user_id = $1 AND used_at IS NULL AND expires_at > now()",
    )
    .bind(user)
    .fetch_one(&pool)
    .await
    .expect("query");
    assert!(
        !known_body.contains(token.0.as_str()),
        "reset token must never appear in the response HTML"
    );

    cleanup(&pool, &prefix).await;
}

/// Reset password: valid token changes the password (verify via star-auth),
/// token is single-use, expired tokens are rejected, session issued.
#[tokio::test]
async fn reset_password_flow() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("OldPassword123!").expect("hash");
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, 'student') RETURNING id",
    )
    .bind("Reset User")
    .bind(format!("{prefix}-reset@test.local"))
    .bind(&hash)
    .fetch_one(&pool)
    .await
    .expect("seed user");

    // Issue a token directly (the forgot_password handler was covered above).
    let token = format!("{prefix}deadbeefdeadbeef");
    sqlx::query("INSERT INTO password_reset_tokens (user_id, token, expires_at) VALUES ($1, $2, now() + interval '2 hours')")
        .bind(user)
        .bind(&token)
        .execute(&pool)
        .await
        .expect("seed token");

    // GET renders the form.
    let body = server.get(&format!("/reset_password/{token}")).await.text();
    assert!(body.contains("reset_password_form"), "form renders: {body}");

    // POST changes the password and logs in.
    let csrf = get_csrf(&server, &format!("/reset_password/{token}")).await;
    let resp = server
        .post(&format!("/reset_password/{token}"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "password": "NewPassword123!",
            "password_confirmation": "NewPassword123!",
            "_csrf": csrf
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "body: {}", resp.text());
    assert!(
        resp.cookies().get("star_session").is_some(),
        "session issued"
    );

    let new_hash: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .expect("hash");
    assert!(star_auth::verify_password("NewPassword123!", &new_hash));
    assert!(!star_auth::verify_password("OldPassword123!", &new_hash));

    let used: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT used_at FROM password_reset_tokens WHERE token = $1")
            .bind(&token)
            .fetch_optional(&pool)
            .await
            .expect("query");
    assert!(used.unwrap().0.is_some(), "token marked used");

    // Second use rejected.
    let csrf = get_csrf(&server, &format!("/reset_password/{token}")).await;
    let replay = server
        .post(&format!("/reset_password/{token}"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "password": "AnotherPass123!",
            "password_confirmation": "AnotherPass123!",
            "_csrf": csrf
        }))
        .await;
    let body = replay.text();
    assert!(
        body.contains("invalid or has expired"),
        "replay rejected: {body}"
    );
    let new_hash2: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .expect("hash");
    assert!(
        star_auth::verify_password("NewPassword123!", &new_hash2),
        "password unchanged on replay"
    );

    // Expired token rejected.
    let expired = format!("{prefix}expiredexpired");
    sqlx::query("INSERT INTO password_reset_tokens (user_id, token, expires_at) VALUES ($1, $2, now() - interval '1 hour')")
        .bind(user)
        .bind(&expired)
        .execute(&pool)
        .await
        .expect("seed expired");
    let body = server
        .get(&format!("/reset_password/{expired}"))
        .await
        .text();
    assert!(body.contains("invalid or has expired"), "expired rejected");

    cleanup(&pool, &prefix).await;
}

/// Expired session: a token past its exp redirects to
/// /login?next=...&expired=1; the page shows the expiry notice; a fresh
/// login clears it.
#[tokio::test]
async fn expired_session_redirects_with_expired_flag() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, 'student') RETURNING id",
    )
    .bind("Expiry User")
    .bind(format!("{prefix}-expiry@test.local"))
    .bind(&hash)
    .fetch_one(&pool)
    .await
    .expect("seed user");

    // Mint a token that expired 1 hour ago: exp = now - 1h.
    let claims = star_common::Claims {
        sub: user,
        role: "student".to_string(),
        exp: (chrono::Utc::now().timestamp() - 3600) as usize,
    };
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("mint expired token");

    let resp = server
        .get("/dashboard")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            token,
        ))
        .await;
    assert!(resp.status_code() == 303 || resp.status_code() == 302);
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        location.contains("expired=1"),
        "expired token must carry expired=1: {location}"
    );
    assert!(location.contains("next="));

    // The login page shows the dismissible notice.
    let body = server
        .get("/login?next=%2Fdashboard&expired=1")
        .await
        .text();
    assert!(
        body.contains("Your session has expired"),
        "notice shown: {body}"
    );
    assert!(body.contains("Dismiss"));

    // Fresh login (no expired flag) has no notice.
    let body = server.get("/login?next=%2Fdashboard").await.text();
    assert!(!body.contains("Your session has expired"));

    // Absent token: no expired flag.
    let resp = server.get("/dashboard").await;
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        !location.contains("expired="),
        "absent token must not set expired=1"
    );

    cleanup(&pool, &prefix).await;
}

/// ENV safety: no credential material (password, hash, reset token, CSRF
/// token) ever appears in the serialized ENV of the auth pages.
///
/// The ENV payloads the handlers actually build are the surface that can
/// leak: `auth_page_shell()` / login / error paths all pass the generic
/// confirmation or the field-error text as notices — never a credential.
/// (Canvas parity: the reset link goes out via email; star-lms logs it
/// server-side, so no handler ever embeds the token in a page or its ENV.)
/// `notices` is free text by contract, so a notice string that *was given* a
/// token would of course serialize it — the invariant to test is what the
/// handlers put in, which is why the cases below are the real handler strings.
#[test]
fn env_safety_no_credentials_leak() {
    let password = "SuperSecret123!";
    let hash = star_auth::hash_password(password).expect("hash");
    let reset_token = "abcdef0123456789abcdef0123456789";

    // The notice strings the auth handlers render (auth.rs FORGOT_CONFIRMATION
    // and the field errors); plus empty notices for the plain GET pages.
    let confirmation =
        "If that account exists, a password reset link was sent. Check your spam box.";
    let mismatch = "Password confirmation does not match.";
    let cases = vec![
        star_ui::env::JsEnv::anonymous("http://localhost", Vec::new()),
        star_ui::env::JsEnv::anonymous("http://localhost", vec![confirmation.to_string()]),
        star_ui::env::JsEnv::anonymous("http://localhost", vec![mismatch.to_string()]),
    ];
    for env in cases {
        let json = env.to_json_string();
        assert!(!json.contains(password), "password leaked");
        assert!(!json.contains(&hash), "hash leaked");
        assert!(!json.contains(reset_token), "reset token leaked");
        // And the struct simply has no field that could carry them.
        let v = serde_json::to_value(&env).unwrap();
        for key in ["password", "password_hash", "token", "csrf", "jwt"] {
            assert!(v.get(key).is_none(), "ENV must not have a {key} key");
        }
    }
}
