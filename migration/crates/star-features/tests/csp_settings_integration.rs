//! Integration tests for star-features CSP settings (`CspSettingsController`).
//!
//! Pinned behavior (Canvas `csp_settings_controller.rb`,
//! `app/models/csp/account_helper.rb`, `app/models/csp/course_helper.rb`):
//! - An account that never set CSP is off, not inherited and not locked.
//! - `PUT status=enabled|disabled` writes the account's own
//!   `csp_inherited_data`; `status=inherited` drops it so the account falls
//!   back to its parent; any other status is 400.
//! - A sub-account without its own setting inherits the nearest ancestor
//!   that has one, and reports `inherited: true`.
//! - An inherited and locked account cannot change its setting (400) and
//!   cannot be locked or unlocked itself (400).
//! - Reads need an account admin (or a course teacher for a course); writes
//!   need `manage_account_settings`. Missing accounts and courses are 404,
//!   unassociated users are 403, anonymous calls are 401.
//! - Allowed domains are added one at a time or in batch, rejected when they
//!   are not parseable, lowercased on insert, and soft deleted on removal.
//! - A course may only enable CSP when its account has CSP enabled.
//!
//! Fleet rule: every test uses DEDICATED accounts (never account 1) and
//! cleans up every seeded row, including csp_domains and account_users.
//! Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "csp_{}_{}",
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

    let server =
        TestServer::new(star_features::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

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
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str, parent_id: Option<i64>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_account_user(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id) VALUES ($1, $2)")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account user");
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
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(course_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed enrollment");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM csp_domains WHERE account_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Account scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_settings_defaults_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Root", None).await;
    seed_account_user(&pool, account, admin).await;

    let resp = server
        .get(&format!("/accounts/{account}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["enabled"], false);
    assert_eq!(body["inherited"], false);
    assert_eq!(body["settings_locked"], false);
    assert_eq!(body["current_account_whitelist"], json!([]));
    // The effective whitelist is only present while CSP is enabled.
    assert!(body.get("effective_whitelist").is_none());

    // An account member may read; an unassociated user may not.
    let resp = server
        .get(&format!("/accounts/{account}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Anonymous calls are rejected before the handler.
    let resp = server
        .get(&format!("/accounts/{account}/csp_settings"))
        .await;
    assert_eq!(resp.status_code(), 401);

    // A missing account is 404 before the permission check.
    let resp = server
        .get("/accounts/999999999/csp_settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_setting_lifecycle() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root", None).await;
    seed_account_user(&pool, account, admin).await;

    // enabled
    let resp = server
        .put(&format!("/accounts/{account}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "enabled" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["enabled"], true);
    assert_eq!(body["inherited"], false);

    // disabled
    let resp = server
        .put(&format!("/accounts/{account}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "disabled" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["enabled"], false);

    // an unknown status is a bad request
    let resp = server
        .put(&format!("/accounts/{account}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "bogus" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // a missing status is a bad request too
    let resp = server
        .put(&format!("/accounts/{account}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400);

    // inherited drops the account's own setting
    let resp = server
        .put(&format!("/accounts/{account}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "inherited" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["enabled"], false);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn child_account_inherits_and_cannot_change_a_locked_setting() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let parent = seed_account(&pool, &prefix, "Parent", None).await;
    let child = seed_account(&pool, &prefix, "Child", Some(parent)).await;
    seed_account_user(&pool, parent, admin).await;

    // The parent enables CSP and locks it.
    let resp = server
        .put(&format!("/accounts/{parent}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "enabled" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .put(&format!("/accounts/{parent}/csp_settings/lock"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "settings_locked": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["settings_locked"], true);

    // The child inherits the parent's setting.
    let resp = server
        .get(&format!("/accounts/{child}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["enabled"], true);
    assert_eq!(body["inherited"], true);
    assert_eq!(body["settings_locked"], true);

    // The child cannot change an inherited, locked setting.
    let resp = server
        .put(&format!("/accounts/{child}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "disabled" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // The child cannot lock or unlock itself either.
    let resp = server
        .put(&format!("/accounts/{child}/csp_settings/lock"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "settings_locked": false }))
        .await;
    assert_eq!(resp.status_code(), 400);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn allowed_domains_add_list_and_remove() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root", None).await;
    seed_account_user(&pool, account, admin).await;

    // Add one domain. Canvas lowercases the domain on save.
    let resp = server
        .post(&format!("/accounts/{account}/csp_settings/domains"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "domain": "Example.COM" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["current_account_whitelist"],
        json!(["example.com"])
    );

    // A domain that cannot be parsed is rejected.
    let resp = server
        .post(&format!("/accounts/{account}/csp_settings/domains"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "domain": "not a domain" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // A missing domain is rejected.
    let resp = server
        .post(&format!("/accounts/{account}/csp_settings/domains"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400);

    // Removing the domain takes it out of the whitelist.
    let resp = server
        .delete(&format!("/accounts/{account}/csp_settings/domains"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "domain": "example.com" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["current_account_whitelist"], json!([]));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn allowed_domains_batch_create() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root", None).await;
    seed_account_user(&pool, account, admin).await;

    let resp = server
        .post(&format!(
            "/accounts/{account}/csp_settings/domains/batch_create"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "domains": ["a.example.com", "b.example.com"] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["current_account_whitelist"],
        json!(["a.example.com", "b.example.com"])
    );

    // One bad domain rejects the whole batch.
    let resp = server
        .post(&format!(
            "/accounts/{account}/csp_settings/domains/batch_create"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "domains": ["c.example.com", "not a domain"] }))
        .await;
    assert_eq!(resp.status_code(), 400);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Course scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_settings_follow_the_account() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Root", None).await;
    seed_account_user(&pool, account, admin).await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // The course starts disabled: the account has no CSP setting yet.
    let resp = server
        .get(&format!("/courses/{course}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["enabled"], false);
    assert_eq!(body["inherited"], true);
    assert_eq!(body["settings_locked"], false);

    // A course cannot enable CSP while the account has it off.
    let resp = server
        .put(&format!("/courses/{course}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "enabled" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // With the account enabled, the course follows.
    let resp = server
        .put(&format!("/accounts/{account}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "enabled" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!("/courses/{course}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["enabled"], true);
    assert_eq!(body["inherited"], true);

    // The course can disable CSP for itself.
    let resp = server
        .put(&format!("/courses/{course}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "disabled" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["enabled"], false);

    // An unknown status is a bad request.
    let resp = server
        .put(&format!("/courses/{course}/csp_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "status": "bogus" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // A missing course is 404.
    let resp = server
        .get("/courses/999999999/csp_settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}
