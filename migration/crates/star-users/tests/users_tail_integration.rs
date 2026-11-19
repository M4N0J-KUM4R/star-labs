//! Integration tests for the users-controller tail (issue #57, Group A):
//! - GET    /users/activity_stream                    (deprecated alias)
//! - POST   /accounts/{id}/self_registration          (create_self_registered_user)
//! - DELETE /users/mobile_sessions                    (expire all)
//! - DELETE /users/{id}/mobile_sessions               (expire for user)
//! - POST   /users/self/pandata_events_token          (access-token guard)
//! - GET    /show_k5_dashboard                        (k5 account association)
//! - POST   /users/{id}/clear_cache                   (manage_site_settings)
//!
//! Conventions (mirrors users_integration.rs):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-users
//! - Unique per-run prefix + full teardown keep repeated runs safe.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

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
        "usr-tail-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the users router
/// exactly the way star-http assembles it.
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
        jwt_secret: "test-jwt-secret".to_string(),
    };

    let server = TestServer::new(star_users_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn star_users_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_users::router())
        .with_state(state)
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(sortable_name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// A fresh root account (never account 1) plus a course in it.
async fn seed_account_with_course(pool: &PgPool, prefix: &str) -> (i64, i64) {
    let account_id: i64 =
        sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
            .bind(format!("{prefix} account"))
            .fetch_one(pool)
            .await
            .expect("seed account");
    let course_id: i64 = sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix} course"))
    .bind(format!("{prefix}-course"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course");
    (account_id, course_id)
}

async fn enroll(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active') ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// Delete everything this run created. The dedicated account cascades its
/// courses/enrollments/account_users; users cascade the rest.
async fn cleanup(pool: &PgPool, prefix: &str) {
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
// GET /users/activity_stream (deprecated alias)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deprecated_activity_stream_alias_returns_same_items_as_self_route() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "StreamUser", "student").await;
    sqlx::query(
        "INSERT INTO stream_items (user_id, asset_string, item_type, context_code, title) \
         VALUES ($1, 'assignment_1', 'Submission', 'course_2', 'stream tail item')",
    )
    .bind(user)
    .execute(&pool)
    .await
    .expect("seed stream item");

    let alias = server
        .get("/users/activity_stream")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(
        alias.status_code(),
        200,
        "deprecated alias should work; body: {}",
        alias.text()
    );
    let body: Vec<Value> = alias.json();
    assert_eq!(body.len(), 1, "one visible stream item via alias");
    assert_eq!(body[0]["asset_string"], "assignment_1");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn deprecated_activity_stream_alias_requires_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.get("/users/activity_stream").await;
    assert_eq!(resp.status_code(), 401, "no token -> 401 before handler");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /accounts/{id}/self_registration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn self_registration_creates_user_when_account_allows_it() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Dedicated account with a canvas auth provider allowing self
    // registration for everyone (Canvas jit_provisioning + auth_filter=all).
    let (account_id, _course_id) = seed_account_with_course(&pool, &prefix).await;
    sqlx::query(
        "INSERT INTO authentication_providers (account_id, auth_type, settings) \
         VALUES ($1, 'canvas', $2::jsonb)",
    )
    .bind(account_id)
    .bind(json!({"jit_provisioning": true, "auth_filter": "all"}).to_string())
    .execute(&pool)
    .await
    .expect("seed auth provider");

    let email = format!("{prefix}-selfreg@test.local");
    let resp = server
        .post(&format!("/accounts/{account_id}/self_registration"))
        .json(&json!({
            "user": { "name": "Self Reg" },
            "pseudonym": { "unique_id": email },
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "self registration should create the user; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["email"], email.as_str());
    assert_eq!(body["name"], "Self Reg");
    assert!(body["id"].as_i64().is_some(), "id present");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn self_registration_is_403_when_account_has_no_canvas_provider() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Dedicated account, no auth provider -> Canvas' default is disabled.
    let (account_id, _course_id) = seed_account_with_course(&pool, &prefix).await;

    let resp = server
        .post(&format!("/accounts/{account_id}/self_registration"))
        .json(&json!({
            "user": { "name": "No Provider" },
            "pseudonym": { "unique_id": format!("{prefix}-np@test.local") },
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "self registration disabled without a canvas provider; body: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn self_registration_is_403_when_provider_disables_it() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (account_id, _course_id) = seed_account_with_course(&pool, &prefix).await;
    sqlx::query(
        "INSERT INTO authentication_providers (account_id, auth_type, settings) \
         VALUES ($1, 'canvas', $2::jsonb)",
    )
    .bind(account_id)
    .bind(json!({"jit_provisioning": false}).to_string())
    .execute(&pool)
    .await
    .expect("seed auth provider");

    let resp = server
        .post(&format!("/accounts/{account_id}/self_registration"))
        .json(&json!({
            "user": { "name": "Disabled" },
            "pseudonym": { "unique_id": format!("{prefix}-off@test.local") },
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "jit_provisioning off -> 403");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn self_registration_missing_unique_id_is_400() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (account_id, _course_id) = seed_account_with_course(&pool, &prefix).await;
    sqlx::query(
        "INSERT INTO authentication_providers (account_id, auth_type, settings) \
         VALUES ($1, 'canvas', $2::jsonb)",
    )
    .bind(account_id)
    .bind(json!({"jit_provisioning": true, "auth_filter": "all"}).to_string())
    .execute(&pool)
    .await
    .expect("seed auth provider");

    let missing = server
        .post(&format!("/accounts/{account_id}/self_registration"))
        .json(&json!({ "user": { "name": "No Email" } }))
        .await;
    assert_eq!(missing.status_code(), 400, "unique_id required");

    // Unknown account: Canvas' require_self_registration substitutes the
    // domain root account for the gate, and the root account has no
    // self-registration provider in this environment, so the gate rejects
    // with 403 before any 404 could apply.
    let unknown = server
        .post("/accounts/999999999/self_registration")
        .json(&json!({
            "user": { "name": "Ghost" },
            "pseudonym": { "unique_id": format!("{prefix}-ghost@test.local") },
        }))
        .await;
    assert_eq!(
        unknown.status_code(),
        403,
        "gate runs against the domain root account first"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /users/mobile_sessions + /users/{id}/mobile_sessions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn expire_mobile_sessions_requires_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "PlainStudent", "student").await;
    let resp = server
        .delete("/users/mobile_sessions")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "manage_user_logins required");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn expire_all_mobile_sessions_ok_for_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let resp = server
        .delete("/users/mobile_sessions")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "admin allowed; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["message"], "All mobile sessions terminated");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn expire_user_mobile_sessions_unknown_user_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin2", "admin").await;
    let resp = server
        .delete("/users/999999999/mobile_sessions")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "api_find fails before the permission"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn expire_user_mobile_sessions_ok_for_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin3", "admin").await;
    let resp = server
        .delete(&format!("/users/{admin}/mobile_sessions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["message"], "All mobile sessions terminated");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /users/self/pandata_events_token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pandata_events_token_always_requires_access_token() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "PandataUser", "teacher").await;
    let resp = server
        .post("/users/self/pandata_events_token")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "teacher"))
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "Canvas' first guard: access token required"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /show_k5_dashboard
// ---------------------------------------------------------------------------

#[tokio::test]
async fn show_k5_dashboard_false_without_k5_association() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "K5Plain", "student").await;
    let resp = server
        .get("/show_k5_dashboard")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["show_k5_dashboard"], false);
    assert_eq!(body["use_classic_font"], false);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_k5_dashboard_true_for_enrollment_in_k5_account() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (account_id, course_id) = seed_account_with_course(&pool, &prefix).await;
    // Mark the dedicated account as a k5 account (Canvas accounts.settings
    // k5_accounts list, here on its own settings row).
    sqlx::query("UPDATE accounts SET settings = $2::jsonb WHERE id = $1")
        .bind(account_id)
        .bind(json!({"k5_accounts": [account_id]}).to_string())
        .execute(&pool)
        .await
        .expect("mark k5 account");

    let student = seed_user(&pool, &prefix, "K5Student", "student").await;
    enroll(&pool, student, course_id, "student").await;

    let resp = server
        .get("/show_k5_dashboard")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(
        body["show_k5_dashboard"], true,
        "enrolled in k5 account course"
    );
    assert_eq!(body["use_classic_font"], false, "no classic font entry");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_k5_dashboard_teacher_can_opt_out_via_preference() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (account_id, course_id) = seed_account_with_course(&pool, &prefix).await;
    sqlx::query("UPDATE accounts SET settings = $2::jsonb WHERE id = $1")
        .bind(account_id)
        .bind(json!({"k5_accounts": [account_id]}).to_string())
        .execute(&pool)
        .await
        .expect("mark k5 account");

    let teacher = seed_user(&pool, &prefix, "K5Teacher", "teacher").await;
    enroll(&pool, teacher, course_id, "teacher").await;
    // Opt out through the settings endpoint itself (elementary_dashboard_disabled).
    let put = server
        .put(&format!("/users/{teacher}/settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "elementary_dashboard_disabled": true }))
        .await;
    assert_eq!(
        put.status_code(),
        200,
        "settings update; body: {}",
        put.text()
    );

    let resp = server
        .get("/show_k5_dashboard")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(
        body["show_k5_dashboard"], false,
        "teachers may disable the elementary dashboard"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_k5_dashboard_requires_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.get("/show_k5_dashboard").await;
    assert_eq!(resp.status_code(), 401, "load_user runs before the action");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /users/{id}/clear_cache
// ---------------------------------------------------------------------------

#[tokio::test]
async fn clear_cache_requires_admin_and_unknown_user_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "CacheStudent", "student").await;
    let resp = server
        .post(&format!("/users/{student}/clear_cache"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "manage_site_settings required");

    // A non-admin on a missing user still gets 404 first (api_find order).
    let missing = server
        .post("/users/999999999/clear_cache")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(
        missing.status_code(),
        404,
        "user resolution precedes permission"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn clear_cache_ok_for_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "CacheAdmin", "admin").await;
    let target = seed_user(&pool, &prefix, "CacheTarget", "student").await;
    let resp = server
        .post(&format!("/users/{target}/clear_cache"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "ok");

    cleanup(&pool, &prefix).await;
}
