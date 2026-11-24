//! The bundle, the shell and the cookie bridge, over the real router.
//!
//! Two things are asserted here that no unit test can see: that `build.rs` actually
//! embedded a front end (an empty `ASSETS` table compiles fine and serves nothing but
//! 404s), and that `session_bridge::bridge_session_to_bearer` authenticates a cookie
//! request against `AuthUser` in a mounted `/api/v1` — the wiring star-http does, which
//! the pure `decide()` tests cannot cover.
//!
//! Asset tests use `PgPool::connect_lazy`, so they run without a database: nothing in
//! the embedded path issues a query. The shell and bridge tests need a real user row
//! (`AuthUser` reloads the role from the DB) and skip when `DATABASE_URL` is unset,
//! matching `ui_integration.rs`.

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware;
use axum::routing::get;
use axum::{Json, Router};
use axum_test::TestServer;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

/// A router whose pool is never queried: `connect_lazy` defers connecting, and neither
/// the embedded assets nor a logged-out page (the extractor rejects before the DB) issue
/// a statement. So these tests run without a database at all.
fn lazy_server() -> TestServer {
    let state = AppState {
        db: PgPool::connect_lazy("postgres://invalid.invalid/invalid").expect("lazy pool"),
        jwt_secret: JWT_SECRET.to_string(),
    };
    TestServer::new(star_ui::app(state)).expect("build TestServer")
}

/// The DB-backed router, or `None` where Postgres is not available.
async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = std::env::var("DATABASE_URL").ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");
    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let prefix = format!(
        "ui-shell-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + N.fetch_add(1, Ordering::Relaxed) as i64
    );

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    Some((
        TestServer::new(star_ui::app(state)).expect("build TestServer"),
        pool,
        prefix,
    ))
}

async fn seed_user(pool: &PgPool, prefix: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind("Shell Student")
    .bind(format!("{prefix}@test.local"))
    .bind(star_auth::hash_password("Password123!").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

fn session_cookie(user_id: i64, role: &str) -> axum_extra::extract::cookie::Cookie<'static> {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint");
    axum_extra::extract::cookie::Cookie::new("star_session", token)
}

// ---------------------------------------------------------------------------
// The embedded bundle
// ---------------------------------------------------------------------------

/// The whole point of `build.rs`: the JS the boot script imports is in the binary, and
/// it is served with the cache policy that makes a repeat visit cost nothing.
#[tokio::test]
async fn the_boot_entry_is_embedded_and_immutable() {
    let server = lazy_server();
    let entry = star_ui::embed::ASSETS
        .iter()
        .find(|asset| asset.path.contains("/entry/start.") && asset.path.ends_with(".js"))
        .expect("a start entry embedded from build/_app");

    let resp = server.get(entry.path).await;
    assert_eq!(resp.status_code(), StatusCode::OK, "GET {}", entry.path);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        "text/javascript; charset=utf-8"
    );
    assert_eq!(resp.as_bytes().len(), entry.bytes.len());
    let cache = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(cache.contains("max-age=31536000") && cache.contains("immutable"));

    // A conditional request must not resend the body.
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let again = server
        .get(entry.path)
        .add_header("if-none-match", etag.clone())
        .await;
    assert_eq!(again.status_code(), StatusCode::NOT_MODIFIED, "etag {etag}");
}

/// The Canvas sheets are served both ways: the versioned URL the shell emits (immutable)
/// and the plain URL Canvas' own `url()` references resolve against (revalidated).
#[tokio::test]
async fn a_canvas_sheet_answers_under_both_urls() {
    let server = lazy_server();
    let sheet = star_ui::embed::ASSETS
        .iter()
        .find(|asset| asset.path == "/canvas/normal_contrast/common.css")
        .expect("common.css embedded");

    let versioned = server
        .get(&format!("/canvas/{}/normal_contrast/common.css", sheet.rev))
        .await;
    assert_eq!(versioned.status_code(), StatusCode::OK);
    assert_eq!(
        versioned.headers()["cache-control"]
            .to_str()
            .unwrap_or_default(),
        "public, max-age=31536000, immutable"
    );

    let plain = server.get("/canvas/normal_contrast/common.css").await;
    assert_eq!(plain.status_code(), StatusCode::OK);
    assert!(
        plain.headers()["cache-control"]
            .to_str()
            .unwrap_or_default()
            .contains("must-revalidate"),
        "an unversioned Canvas URL is fixed forever, so it must revalidate"
    );

    // The revision is the file's own hash, so a wrong one is a miss, not a hit.
    let stale = server
        .get("/canvas/00000000/normal_contrast/common.css")
        .await;
    assert_eq!(stale.status_code(), StatusCode::NOT_FOUND);
}

/// A font and an image must resolve at the literal paths Canvas' sheets name, because
/// rewriting those `url()` references is not an option.
#[tokio::test]
async fn fonts_and_images_resolve_at_their_canvas_names() {
    let server = lazy_server();
    for path in [
        "/fonts/lato/extended/Lato-Regular.woff2",
        "/images/login/canvas-logo.svg",
    ] {
        let resp = server.get(path).await;
        assert_eq!(
            resp.status_code(),
            StatusCode::OK,
            "{path} must be reachable at the name Canvas' CSS asks for"
        );
    }
}

/// The asset handler is the router's fallback, so it must not swallow an unknown
/// `/api/v1` path: star-http mounts it as the root fallback, and a rewritten 404 would
/// change every mismatched API response the goldens compare.
#[tokio::test]
async fn an_unknown_api_path_keeps_the_default_404() {
    let server = lazy_server();
    let resp = server.get("/api/v1/definitely-not-a-route").await;
    assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
    assert_eq!(
        resp.text().trim(),
        "Not Found",
        "the fallback must answer as axum's default does"
    );

    // ... and a write never fetches an asset, even at a real one's path.
    let write = server.post("/fonts/lato/extended/Lato-Regular.woff2").await;
    assert_eq!(write.status_code(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// The shell document
// ---------------------------------------------------------------------------

#[tokio::test]
async fn profile_is_the_svelte_shell_with_canvas_styles() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "student").await;

    let resp = server
        .get("/profile")
        .add_cookie(session_cookie(user, "student"))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let body = resp.text();

    // The boot document, unharmed.
    assert!(
        body.contains("kit.start(app, element)"),
        "boot script missing"
    );
    assert!(
        body.contains("/_app/immutable/entry/start."),
        "entry link missing"
    );

    // The three per-request injections.
    assert!(body.contains("<title>Profile</title>"));
    let env = star_ui::env::parse_env_from_html(&body).expect("ENV parses");
    assert_eq!(env["current_user_id"], serde_json::json!(user));

    // Canvas' own sheets, versioned, in the order the cascade needs.
    assert!(body.contains("data-canvas-bundle=\"tokens\""));
    assert!(body.contains("data-canvas-bundle=\"common\""));
    assert!(
        body.contains("<body class=\"primary-nav-expanded\""),
        "the rail and layout margins key off this class"
    );

    // One user's document must never be cached.
    assert!(resp.headers()["cache-control"]
        .to_str()
        .unwrap_or_default()
        .contains("no-store"));

    cleanup(&pool, &prefix).await;
}

/// AC1 of the shell: an anonymous `/profile` is a login redirect, not a shell with an
/// empty ENV. Rendering the bundle and letting the client discover 401s would leak the
/// page structure and show a blank dashboard for a frame.
#[tokio::test]
async fn logged_out_profile_never_renders_a_shell() {
    let server = lazy_server();
    let resp = server.get("/profile").await;
    assert_eq!(resp.status_code(), StatusCode::SEE_OTHER);
    assert!(resp.text().is_empty(), "a redirect carries no document");
    let location = resp.headers()["location"].to_str().unwrap_or_default();
    assert!(
        location.starts_with("/login?next=") && location.contains("profile"),
        "unexpected location: {location}"
    );
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// The cookie bridge, mounted the way star-http mounts it
// ---------------------------------------------------------------------------

async fn whoami(
    State(_app): State<AppState>,
    user: star_auth::AuthUser,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "id": user.id, "role": user.role }))
}

fn bridged(state: AppState) -> TestServer {
    let api = Router::new()
        .route("/whoami", get(whoami).post(whoami))
        .layer(middleware::from_fn(
            star_ui::session_bridge::bridge_session_to_bearer,
        ));
    let app = Router::new().nest("/api/v1", api).with_state(state);
    TestServer::new(app).expect("build bridged TestServer")
}

/// The claim the whole frontend plan rests on: a page with no token can call the API
/// because the session cookie is turned into the credential `AuthUser` verifies.
#[tokio::test]
async fn a_cookie_get_reaches_the_bearer_only_handler() {
    let db_url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("SKIP: DATABASE_URL not set");
            return;
        }
    };
    let pool = PgPool::connect(&db_url).await.expect("connect");
    star_db::MIGRATOR.run(&pool).await.expect("migrate");
    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = bridged(state);
    let user = seed_user(&pool, "bridge-get", "teacher").await;

    let resp = server
        .get("/api/v1/whoami")
        .add_cookie(session_cookie(user, "teacher"))
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK, "{}", resp.text());
    assert_eq!(
        resp.json::<serde_json::Value>()["id"],
        serde_json::json!(user)
    );

    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind("bridge-get@test.local")
        .execute(&pool)
        .await;
}

/// A write from the browser carries the double-submit pair; without it the same request
/// is a 403 rather than an authenticated mutation.
#[tokio::test]
async fn a_cookie_post_needs_the_csrf_header() {
    let db_url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("SKIP: DATABASE_URL not set");
            return;
        }
    };
    let pool = PgPool::connect(&db_url).await.expect("connect");
    star_db::MIGRATOR.run(&pool).await.expect("migrate");
    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = bridged(state);
    let user = seed_user(&pool, "bridge-post", "teacher").await;
    let session = session_cookie(user, "teacher");

    let csrf = star_ui::session::generate_csrf_token();
    let without_header = server
        .post("/api/v1/whoami")
        .add_cookie(session.clone())
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .await;
    assert_eq!(
        without_header.status_code(),
        StatusCode::FORBIDDEN,
        "a session POST with no CSRF header must not reach the handler"
    );

    let with_header = server
        .post("/api/v1/whoami")
        .add_cookie(session)
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .add_header("x-csrf-token", csrf)
        .await;
    assert_eq!(
        with_header.status_code(),
        StatusCode::OK,
        "{}",
        with_header.text()
    );

    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind("bridge-post@test.local")
        .execute(&pool)
        .await;
}

/// Cross-origin requests are refused, and an explicit `Authorization` header still
/// behaves exactly as it did before the bridge existed.
#[tokio::test]
async fn cross_site_is_refused_and_bearer_is_untouched() {
    let db_url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("SKIP: DATABASE_URL not set");
            return;
        }
    };
    let pool = PgPool::connect(&db_url).await.expect("connect");
    star_db::MIGRATOR.run(&pool).await.expect("migrate");
    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = bridged(state);
    let user = seed_user(&pool, "bridge-origin", "student").await;
    let session = session_cookie(user, "student");

    let foreign = server
        .get("/api/v1/whoami")
        .add_cookie(session.clone())
        .add_header("origin", "https://evil.example")
        .await;
    assert_eq!(foreign.status_code(), StatusCode::FORBIDDEN);

    let token = session.value().to_string();
    let bearer = server
        .get("/api/v1/whoami")
        .add_cookie(session)
        .add_header("authorization", format!("Bearer {token}"))
        .await;
    assert_eq!(bearer.status_code(), StatusCode::OK, "{}", bearer.text());

    let anonymous = server.get("/api/v1/whoami").await;
    assert_eq!(anonymous.status_code(), StatusCode::UNAUTHORIZED);

    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind("bridge-origin@test.local")
        .execute(&pool)
        .await;
}
