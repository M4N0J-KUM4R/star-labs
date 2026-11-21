//! Integration tests for the Canvas ePortfolios routes.
//!
//! Pinned behaviour (from `canvas-src/app/controllers/eportfolios_api_controller.rb`):
//! - `GET /users/{user_id}/eportfolios` answers a **bare JSON array** of
//!   ePortfolio hashes (`id`, `user_id`, `name`, `public`, `created_at`,
//!   `updated_at`, `workflow_state`, `deleted_at`, `spam_status`), ordered by
//!   `updated_at`;
//! - the owner only ever sees the **active** portfolios, a moderator sees the
//!   deleted ones as well when `include[]=deleted` is passed;
//! - `show` / `pages` answer a 404 for an unknown id and a 403 for a
//!   portfolio the caller may not read;
//! - `delete` soft deletes (`workflow_state = "deleted"`, `deleted_at` set)
//!   and `restore` brings the portfolio back;
//! - `moderate` / `moderate_all` only accept `marked_as_safe` and
//!   `marked_as_spam`, anything else is a 400 with Canvas' error body;
//! - an anonymous caller is a 401 on every route.
//!
//! Conventions (mirrors the rest of the workspace):
//! - skipped cleanly when `DATABASE_URL` is not set (prints SKIP and returns);
//! - unique per-run prefix and a full teardown, so repeat runs against a
//!   shared database are safe.
//!
//! The routes under test are wired into `star_eportfolios::router()` by the
//! coordinator; this file exercises them through that router.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test `AppState` and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

/// The DDL of the three tables the ePortfolio routes need. They already exist
/// in `star-db` (migration `0224`); the statements keep the suite runnable
/// on its own and are a no-op afterwards.
const EPORTFOLIO_DDL: [&str; 3] = [
    r#"
    CREATE TABLE IF NOT EXISTS eportfolio_categories (
        id                BIGSERIAL PRIMARY KEY,
        eportfolio_id     bigint NOT NULL,
        name              varchar(255),
        position          integer,
        slug              varchar(255),
        created_at        timestamp NOT NULL DEFAULT now(),
        updated_at        timestamp NOT NULL DEFAULT now()
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS eportfolio_entries (
        id                     BIGSERIAL PRIMARY KEY,
        eportfolio_id          bigint NOT NULL,
        eportfolio_category_id bigint NOT NULL,
        position               integer,
        name                   varchar(255),
        allow_comments         boolean,
        show_comments          boolean,
        slug                   varchar(255),
        content                text,
        created_at             timestamp NOT NULL DEFAULT now(),
        updated_at             timestamp NOT NULL DEFAULT now()
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS eportfolios (
        id             BIGSERIAL PRIMARY KEY,
        user_id        bigint NOT NULL,
        name           varchar(255),
        public         boolean,
        created_at     timestamp NOT NULL DEFAULT now(),
        updated_at     timestamp NOT NULL DEFAULT now(),
        uuid           varchar(255),
        workflow_state varchar(255) NOT NULL DEFAULT 'active',
        deleted_at     timestamp,
        spam_status    varchar
    )
    "#,
];

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so repeated runs against a shared DB never collide.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "eportfolio-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
        n
    )
}

/// Connect, migrate, and build a TestServer over the ePortfolio router.
async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    for statement in EPORTFOLIO_DDL {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("create the ePortfolio tables");
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server =
        TestServer::new(star_eportfolios::router().with_state(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Seed an account below the site root account.
async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    let root: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL \
           AND workflow_state <> 'deleted' ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .expect("read the site root account");
    let root_id = root.expect("the site root account exists").0;
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(root_id)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

/// Add a user to an account. Canvas grants the account rights through the
/// `account_users` role.
async fn seed_account_member(pool: &PgPool, account_id: i64, user_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (account_id, user_id) DO NOTHING",
    )
    .bind(account_id)
    .bind(user_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed account member");
}

/// Seed an ePortfolio in the given workflow state.
async fn seed_portfolio(
    pool: &PgPool,
    user_id: i64,
    name: &str,
    public: bool,
    workflow_state: &str,
) -> i64 {
    let deleted_at = if workflow_state == "deleted" {
        Some(chrono::Utc::now().naive_utc())
    } else {
        None
    };
    sqlx::query_scalar(
        "INSERT INTO eportfolios (user_id, name, public, workflow_state, deleted_at, \
                created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, now(), now()) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(public)
    .bind(workflow_state)
    .bind(deleted_at)
    .fetch_one(pool)
    .await
    .expect("seed portfolio")
}

async fn seed_category(pool: &PgPool, eportfolio_id: i64, name: &str, position: i32) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO eportfolio_categories (eportfolio_id, name, position, created_at, updated_at) \
         VALUES ($1, $2, $3, now(), now()) RETURNING id",
    )
    .bind(eportfolio_id)
    .bind(name)
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed category")
}

async fn seed_entry(
    pool: &PgPool,
    eportfolio_id: i64,
    category_id: i64,
    name: &str,
    position: i32,
    content: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO eportfolio_entries (eportfolio_id, eportfolio_category_id, name, position, \
                content, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, now(), now()) RETURNING id",
    )
    .bind(eportfolio_id)
    .bind(category_id)
    .bind(name)
    .bind(position)
    .bind(content)
    .fetch_one(pool)
    .await
    .expect("seed entry")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Remove every row this file seeded.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM eportfolio_entries WHERE eportfolio_id IN \
             (SELECT id FROM eportfolios WHERE user_id IN \
                (SELECT id FROM users WHERE email LIKE $1))",
    )
    .bind(format!("{prefix}-%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM eportfolio_categories WHERE eportfolio_id IN \
             (SELECT id FROM eportfolios WHERE user_id IN \
                (SELECT id FROM users WHERE email LIKE $1))",
    )
    .bind(format!("{prefix}-%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM eportfolios WHERE user_id IN \
             (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}-%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE user_id IN \
             (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}-%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}-%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// index
// ---------------------------------------------------------------------------

#[tokio::test]
async fn owner_lists_their_active_portfolios() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let active = seed_portfolio(&pool, owner, "Active Portfolio", false, "active").await;
    seed_portfolio(&pool, owner, "Deleted Portfolio", false, "deleted").await;

    let response = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());

    let body: Value = response.json();
    let list = body.as_array().expect("a bare JSON array");
    assert_eq!(list.len(), 1, "the owner only sees the active portfolio");
    assert_eq!(list[0]["id"], active);
    assert_eq!(list[0]["user_id"], owner);
    assert_eq!(list[0]["name"], "Active Portfolio");
    assert_eq!(list[0]["public"], false);
    assert_eq!(list[0]["workflow_state"], "active");
    assert_eq!(list[0]["deleted_at"], Value::Null);
    assert_eq!(list[0]["spam_status"], Value::Null);
    assert!(
        list[0]["created_at"]
            .as_str()
            .is_some_and(|v| v.ends_with('Z')),
        "created_at is an ISO8601 timestamp: {}",
        list[0]["created_at"]
    );

    // `self` resolves to the current user.
    let response = server
        .get("/users/self/eportfolios")
        .add_header(AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body.as_array().map(|list| list.len()), Some(1));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn index_rejects_another_student() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let other = seed_user(&pool, &prefix, "other", "student").await;
    seed_portfolio(&pool, owner, "Private", false, "active").await;

    let response = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(response.status_code(), 403, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["status"], "unauthorized");

    // An unknown user is a 404, the lookup runs before the permission check.
    let response = server
        .get("/users/99999999/eportfolios")
        .add_header(AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn moderator_sees_the_deleted_portfolios_on_demand() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let moderator = seed_user(&pool, &prefix, "moderator", "student").await;
    let account = seed_account(&pool, &prefix, "acct").await;
    seed_account_member(&pool, account, moderator, "admin").await;
    seed_account_member(&pool, account, owner, "member").await;

    let active = seed_portfolio(&pool, owner, "Active", false, "active").await;
    let deleted = seed_portfolio(&pool, owner, "Deleted", false, "deleted").await;

    // Without `include[]=deleted` only the active portfolio is listed.
    let response = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(AUTHORIZATION, auth(moderator, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let ids: Vec<i64> = body
        .as_array()
        .expect("a bare JSON array")
        .iter()
        .map(|entry| entry["id"].as_i64().expect("an id"))
        .collect();
    assert_eq!(ids, vec![active]);

    // With `include[]=deleted` the deleted portfolio shows up as well.
    let response = server
        .get(&format!("/users/{owner}/eportfolios?include[]=deleted"))
        .add_header(AUTHORIZATION, auth(moderator, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let mut ids: Vec<i64> = body
        .as_array()
        .expect("a bare JSON array")
        .iter()
        .map(|entry| entry["id"].as_i64().expect("an id"))
        .collect();
    ids.sort_unstable();
    let mut expected = vec![active, deleted];
    expected.sort_unstable();
    assert_eq!(ids, expected);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// show / pages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn show_answers_the_portfolio() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let portfolio = seed_portfolio(&pool, owner, "My Portfolio", true, "active").await;

    let response = server
        .get(&format!("/eportfolios/{portfolio}"))
        .add_header(AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"], portfolio);
    assert_eq!(body["user_id"], owner);
    assert_eq!(body["name"], "My Portfolio");
    assert_eq!(body["public"], true);
    assert_eq!(body["workflow_state"], "active");

    // An unknown id is a 404.
    let response = server
        .get("/eportfolios/99999999")
        .add_header(AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_follows_the_portfolio_policy() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let other = seed_user(&pool, &prefix, "other", "student").await;
    let private = seed_portfolio(&pool, owner, "Private", false, "active").await;
    let public = seed_portfolio(&pool, owner, "Public", true, "active").await;

    // A private portfolio is not readable by another student.
    let response = server
        .get(&format!("/eportfolios/{private}"))
        .add_header(AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(response.status_code(), 403, "body: {}", response.text());

    // A public portfolio is.
    let response = server
        .get(&format!("/eportfolios/{public}"))
        .add_header(AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn pages_lists_the_entries_of_the_portfolio() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let portfolio = seed_portfolio(&pool, owner, "With Pages", false, "active").await;
    let category = seed_category(&pool, portfolio, "Home", 1).await;
    let entry = seed_entry(
        &pool,
        portfolio,
        category,
        "Welcome",
        1,
        "Nothing entered yet",
    )
    .await;

    let response = server
        .get(&format!("/eportfolios/{portfolio}/pages"))
        .add_header(AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let list = body.as_array().expect("a bare JSON array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], entry);
    assert_eq!(list[0]["eportfolio_id"], portfolio);
    assert_eq!(list[0]["position"], 1);
    assert_eq!(list[0]["name"], "Welcome");
    assert_eq!(list[0]["content"], "Nothing entered yet");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// delete / restore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_soft_deletes_the_portfolio() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let portfolio = seed_portfolio(&pool, owner, "Doomed", false, "active").await;

    let response = server
        .delete(&format!("/eportfolios/{portfolio}"))
        .add_header(AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["workflow_state"], "deleted");
    assert!(
        body["deleted_at"].as_str().is_some(),
        "deleted_at is set: {}",
        body["deleted_at"]
    );

    // The portfolio is gone from the owner's index.
    let response = server
        .get(&format!("/users/{owner}/eportfolios"))
        .add_header(AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body.as_array().map(|list| list.len()), Some(0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn restore_brings_the_portfolio_back() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let moderator = seed_user(&pool, &prefix, "moderator", "student").await;
    let account = seed_account(&pool, &prefix, "acct").await;
    seed_account_member(&pool, account, moderator, "admin").await;
    seed_account_member(&pool, account, owner, "member").await;

    let portfolio = seed_portfolio(&pool, owner, "Restorable", false, "deleted").await;

    // The owner has no `:restore` right on their own portfolio.
    let response = server
        .put(&format!("/eportfolios/{portfolio}/restore"))
        .add_header(AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(response.status_code(), 403, "body: {}", response.text());

    let response = server
        .put(&format!("/eportfolios/{portfolio}/restore"))
        .add_header(AUTHORIZATION, auth(moderator, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["workflow_state"], "active");
    assert_eq!(body["deleted_at"], Value::Null);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// moderate / moderate_all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn moderate_sets_the_spam_status() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let moderator = seed_user(&pool, &prefix, "moderator", "student").await;
    let account = seed_account(&pool, &prefix, "acct").await;
    seed_account_member(&pool, account, moderator, "admin").await;
    seed_account_member(&pool, account, owner, "member").await;

    let portfolio = seed_portfolio(&pool, owner, "Moderated", false, "active").await;

    // Another student may not moderate.
    let response = server
        .put(&format!("/eportfolios/{portfolio}/moderate"))
        .add_header(AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(response.status_code(), 403, "body: {}", response.text());

    let response = server
        .put(&format!("/eportfolios/{portfolio}/moderate"))
        .add_header(AUTHORIZATION, auth(moderator, "student"))
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["spam_status"], "marked_as_spam");

    // An unknown spam status is Canvas' 400.
    let response = server
        .put(&format!("/eportfolios/{portfolio}/moderate"))
        .add_header(AUTHORIZATION, auth(moderator, "student"))
        .json(&json!({ "spam_status": "bogus" }))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["error"],
        "spam_status must be one of [\"marked_as_safe\", \"marked_as_spam\"]"
    );

    // A missing spam status is the same 400.
    let response = server
        .put(&format!("/eportfolios/{portfolio}/moderate"))
        .add_header(AUTHORIZATION, auth(moderator, "student"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn moderate_all_updates_the_active_portfolios() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let moderator = seed_user(&pool, &prefix, "moderator", "student").await;
    let account = seed_account(&pool, &prefix, "acct").await;
    seed_account_member(&pool, account, moderator, "admin").await;
    seed_account_member(&pool, account, owner, "member").await;

    let active = seed_portfolio(&pool, owner, "Active", false, "active").await;
    let deleted = seed_portfolio(&pool, owner, "Deleted", false, "deleted").await;

    let response = server
        .put(&format!("/users/{owner}/eportfolios"))
        .add_header(AUTHORIZATION, auth(moderator, "student"))
        .json(&json!({ "spam_status": "marked_as_safe" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["success"], true);

    let status: (Option<String>,) =
        sqlx::query_as("SELECT spam_status FROM eportfolios WHERE id = $1")
            .bind(active)
            .fetch_one(&pool)
            .await
            .expect("read the active portfolio");
    assert_eq!(status.0.as_deref(), Some("marked_as_safe"));

    // Only the active portfolios are touched.
    let status: (Option<String>,) =
        sqlx::query_as("SELECT spam_status FROM eportfolios WHERE id = $1")
            .bind(deleted)
            .fetch_one(&pool)
            .await
            .expect("read the deleted portfolio");
    assert_eq!(status.0, None);

    // An unknown spam status is Canvas' 400.
    let response = server
        .put(&format!("/users/{owner}/eportfolios"))
        .add_header(AUTHORIZATION, auth(moderator, "student"))
        .json(&json!({ "spam_status": "bogus" }))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Anonymous
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anonymous_requests_are_unauthorized() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let portfolio = seed_portfolio(&pool, owner, "Portfolio", false, "active").await;

    let paths = [
        format!("/users/{owner}/eportfolios"),
        format!("/eportfolios/{portfolio}"),
        format!("/eportfolios/{portfolio}/pages"),
    ];
    for path in &paths {
        let response = server.get(path).await;
        assert_eq!(
            response.status_code(),
            401,
            "{path}; body: {}",
            response.text()
        );
    }

    let response = server.delete(&format!("/eportfolios/{portfolio}")).await;
    assert_eq!(response.status_code(), 401, "body: {}", response.text());

    let response = server
        .put(&format!("/eportfolios/{portfolio}/moderate"))
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(response.status_code(), 401, "body: {}", response.text());

    let response = server
        .put(&format!("/eportfolios/{portfolio}/restore"))
        .await;
    assert_eq!(response.status_code(), 401, "body: {}", response.text());

    let response = server
        .put(&format!("/users/{owner}/eportfolios"))
        .json(&json!({ "spam_status": "marked_as_spam" }))
        .await;
    assert_eq!(response.status_code(), 401, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}
