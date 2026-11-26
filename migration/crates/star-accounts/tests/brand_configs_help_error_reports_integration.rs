//! Integration tests for the issue #96 routes the behavior pass pinned:
//! the brand variables redirects, the shared brand config mutations, the
//! error report rescue path and the help links authentication gate.
//!
//! Pinned behavior:
//! - `GET /brand_variables` is public, answers 302 with CORS `*` and points
//!   at the brandable css json of the domain root account;
//! - the account/course `brand_variables` routes still require a user
//!   (Canvas skips `require_user` only for the global route): anonymous
//!   callers answer 401 — even for unknown contexts — while a signed-in
//!   caller answers 404 for an unknown context, and the redirect carries the
//!   newest shared brand config md5 of the account (the active brand), else
//!   the BrandableCSS default;
//! - shared brand configs are admin-only mutations reading the Canvas
//!   `shared_brand_config` envelope, and an md5 without a brand config fails
//!   the save with 422;
//! - the error report route answers `{logged: true, id}` and rescues a
//!   failed save as `{logged: true, id: null}` without storing the report;
//! - the help links route answers 401 for an anonymous caller.
//!
//! Fleet rule: dedicated users, accounts, courses and brand configs per
//! test, every seeded row is cleaned up. Skipped cleanly when DATABASE_URL
//! is unset.

use axum::http::header::{AUTHORIZATION, LOCATION};
use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "i96-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
        n
    )
}

/// A brand config md5 of this run — the column is `varchar(32)`, so the tag
/// is folded into the prefix and the value truncated to fit.
fn brand_md5(prefix: &str, tag: &str) -> String {
    let raw = format!("{prefix}{tag}");
    raw[..raw.len().min(32)].to_string()
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
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_accounts::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role, sortable_name) \
         VALUES ($1, $2, $3, $4, $1) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id, workflow_state) \
         VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(name)
    .bind(parent)
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

async fn seed_brand_config(pool: &PgPool, md5: &str) {
    sqlx::query("INSERT INTO brand_configs (md5, name, variables) VALUES ($1, $2, '{}')")
        .bind(md5)
        .bind("Seeded theme")
        .execute(pool)
        .await
        .expect("seed brand config");
}

/// The domain root account the port resolves once per installation.
async fn root_account(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL \
         AND workflow_state <> 'deleted' ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(pool)
    .await
    .expect("seeded root account")
}

fn location(resp: &axum_test::TestResponse) -> String {
    resp.headers()
        .get(LOCATION)
        .expect("Location header")
        .to_str()
        .expect("ascii location")
        .to_string()
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM error_reports WHERE subject LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM shared_brand_configs WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM brand_configs WHERE md5 LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    // feature_flags rows for the dedicated accounts (no FK; delete
    // explicitly), then the accounts themselves (issue #75).
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Account' \
           AND context_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
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
// GET /brand_variables, /accounts/{id}/brand_variables, /courses/{id}/brand_variables
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brand_variables_redirects_publicly_and_contexts_need_a_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let token = auth(user, "student");
    let root = root_account(&pool).await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), Some(root)).await;
    let course = seed_course(&pool, &prefix, "Course", account).await;

    // Canvas `show` is public: 302 with CORS `*` to the root account's
    // brandable css json.
    let resp = server.get("/brand_variables").await;
    assert_eq!(resp.status_code(), 302, "body: {}", resp.text());
    let acao = resp
        .headers()
        .get("access-control-allow-origin")
        .expect("CORS header");
    assert_eq!(acao, "*");
    let target = location(&resp);
    assert!(
        target.starts_with("/dist/brand-css/") && target.ends_with(".json"),
        "{target}"
    );

    // A dedicated account without a brand answers the BrandableCSS default.
    let resp = server
        .get(&format!("/accounts/{account}/brand_variables"))
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 302, "body: {}", resp.text());
    assert_eq!(location(&resp), "/dist/brand-css/default.json");

    // The course answers its account's brand.
    let resp = server
        .get(&format!("/courses/{course}/brand_variables"))
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 302, "body: {}", resp.text());
    assert_eq!(location(&resp), "/dist/brand-css/default.json");

    // The active brand wins: the newest shared brand config of the account
    // is the one the redirect points at.
    let md5_old = brand_md5(&prefix, "o");
    let md5_new = brand_md5(&prefix, "n");
    seed_brand_config(&pool, &md5_old).await;
    seed_brand_config(&pool, &md5_new).await;
    // `updated_at` is NOT NULL without a default (Rails sets both
    // timestamps on create), so the seed sets them explicitly.
    sqlx::query(
        "INSERT INTO shared_brand_configs \
         (account_id, name, brand_config_md5, created_at, updated_at) \
         VALUES ($1, 'old', $2, now(), now()), ($1, 'new', $3, now(), now())",
    )
    .bind(account)
    .bind(&md5_old)
    .bind(&md5_new)
    .execute(&pool)
    .await
    .expect("seed shares");
    let resp = server
        .get(&format!("/accounts/{account}/brand_variables"))
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 302, "body: {}", resp.text());
    assert_eq!(location(&resp), format!("/dist/brand-css/{md5_new}.json"));

    // `show_context` keeps `require_user`: anonymous callers answer 401 —
    // also on unknown contexts — while a signed-in caller answers 404.
    for path in [
        format!("/accounts/{account}/brand_variables"),
        format!("/courses/{course}/brand_variables"),
        "/accounts/999999999/brand_variables".to_string(),
    ] {
        let resp = server.get(&path).await;
        assert_eq!(resp.status_code(), 401, "path {path}: {}", resp.text());
    }
    let resp = server
        .get("/accounts/999999999/brand_variables")
        .add_header(AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST/PUT/DELETE shared_brand_configs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn shared_brand_configs_need_an_account_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let root = root_account(&pool).await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), Some(root)).await;
    let md5 = brand_md5(&prefix, "c");
    seed_brand_config(&pool, &md5).await;

    // Anonymous callers answer 401.
    let resp = server
        .post(&format!("/accounts/{account}/shared_brand_configs"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // A signed-in caller without the account admin right answers 403.
    let resp = server
        .post(&format!("/accounts/{account}/shared_brand_configs"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "shared_brand_config": { "name": "Mine", "brand_config_md5": md5 }
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The admin shares a theme through the Canvas envelope and the model
    // attributes come back with `created` (201).
    let resp = server
        .post(&format!("/accounts/{account}/shared_brand_configs"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "shared_brand_config": { "name": "Crimson", "brand_config_md5": md5 }
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let share: Value = resp.json();
    let share_id = share["id"].as_i64().expect("share id");
    assert_eq!(share["account_id"], account, "{share}");
    assert_eq!(share["name"], "Crimson");
    assert_eq!(share["brand_config_md5"], md5);
    assert!(share["created_at"].is_string(), "{share}");
    assert!(share["updated_at"].is_string(), "{share}");

    // An md5 without a brand config fails the save (Canvas answers the
    // validation errors with unprocessable_content).
    let resp = server
        .post(&format!("/accounts/{account}/shared_brand_configs"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "shared_brand_config":
                { "name": "Broken", "brand_config_md5": format!("{prefix}missing") }
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    // The update route only touches the attributes the caller sent.
    let resp = server
        .put(&format!(
            "/accounts/{account}/shared_brand_configs/{share_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "shared_brand_config": { "name": "Gold" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["id"], share_id);
    assert_eq!(updated["name"], "Gold");
    assert_eq!(updated["brand_config_md5"], md5, "{updated}");

    // The destroy route answers the removed share and then 404s.
    let resp = server
        .delete(&format!("/shared_brand_configs/{share_id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let removed: Value = resp.json();
    assert_eq!(removed["id"], share_id);
    assert_eq!(removed["name"], "Gold");

    let resp = server
        .delete("/shared_brand_configs/999999999")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /error_reports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_error_report_rescues_a_failed_save() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let root = root_account(&pool).await;

    // Canvas rescues a failed save and still answers `logged: true` — a
    // report without the required subject is not stored.
    let resp = server
        .post("/error_reports")
        .json(&json!({ "error": { "url": "http://example.test/x" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({ "logged": true, "id": null }), "{body}");
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM error_reports WHERE subject LIKE $1")
            .bind(format!("{prefix}%"))
            .fetch_one(&pool)
            .await
            .expect("count the reports");
    assert_eq!(stored, 0);

    // A full anonymous report is stored on the root account and its id is
    // the one in the answer.
    let subject = format!("{prefix} things are broken");
    let resp = server
        .post("/error_reports")
        .json(&json!({
            "error": {
                "subject": subject,
                "url": "http://example.test/courses/1",
                "email": "student@example.test",
                "comments": "it went boom",
                "http_env": { "browser": "test" },
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["logged"], true, "{body}");
    let id = body["id"].as_i64().expect("report id");

    let row: (
        Option<i64>,
        Option<i64>,
        String,
        String,
        String,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT user_id, account_id, subject, url, email, comments \
         FROM error_reports WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("the report row");
    assert_eq!(row.0, None, "the report is anonymous");
    assert_eq!(row.1, Some(root), "the report is filed on the root account");
    assert_eq!(row.2, subject);
    assert_eq!(row.3, "http://example.test/courses/1");
    assert_eq!(row.4, "student@example.test");
    assert_eq!(row.5.as_deref(), Some("it went boom"));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{id}/help_links
// ---------------------------------------------------------------------------

#[tokio::test]
async fn help_links_needs_authentication() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let root = root_account(&pool).await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), Some(root)).await;

    // An anonymous caller answers 401 before the context lookup — the
    // sentinel account behaves the same way (no account leak).
    let resp = server.get(&format!("/accounts/{account}/help_links")).await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server.get("/accounts/999999999/help_links").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
