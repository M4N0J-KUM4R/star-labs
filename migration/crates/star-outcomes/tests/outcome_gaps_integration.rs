//! Integration tests for the P5 outcome gaps in star-outcomes:
//!
//! - GET  /accounts/{accountId}/root_outcome_group (redirect)
//! - POST /accounts/{accountId}/outcome_groups/{id}/subgroups (create)
//! - GET  /global/outcomes_import/available
//! - POST /global/outcomes_import
//! - GET  /global/outcomes_import/migration_status/{migrationId}
//!
//! The account root redirect lazily creates the account's root group (like
//! the course and global redirects) and 302s to its canonical URL; the
//! subgroup create mirrors the course-side create for account admins; the
//! Academic Benchmarks import family records completed
//! `outcome_ab_migrations` rows (migration 0254).
//!
//! Skipped cleanly when DATABASE_URL is not set.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    // The counter makes the prefix unique inside the process: two tests
    // that start in the same millisecond would otherwise seed the same
    // email and collide on the users' unique key.
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "outgaps-tests-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
        n
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
        TestServer::new(star_outcomes::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
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
    sqlx::query_scalar(
        "INSERT INTO accounts (name, workflow_state) VALUES ($1, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM outcome_groups WHERE title LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE user_id IN \
         (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
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
// GET /accounts/{accountId}/root_outcome_group
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_root_outcome_group_redirects_to_the_root_group() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Acct").await;

    // Unknown account: 404 before any rights check.
    let resp = server
        .get("/accounts/99999999/root_outcome_group")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Non-admin of the account: 403.
    let resp = server
        .get(&format!("/accounts/{account}/root_outcome_group"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // An account admin gets the 302 redirect to the lazily created root
    // group's canonical URL, with the group JSON in the body.
    let admin = seed_user(&pool, &prefix, "AcctAdmin", "teacher").await;
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account)
        .bind(admin)
        .execute(&pool)
        .await
        .expect("seed account admin");

    let resp = server
        .get(&format!("/accounts/{account}/root_outcome_group"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 302, "body: {}", resp.text());
    let location = resp
        .headers()
        .get("location")
        .and_then(|value| value.to_str().ok())
        .expect("location header")
        .to_string();
    assert!(location.starts_with(&format!("/api/v1/accounts/{account}/outcome_groups/")));
    let body: Value = resp.json();
    assert_eq!(body["context_type"], json!("Account"));
    assert_eq!(body["context_id"], json!(account));
    let group_id = body["id"].as_i64().expect("group id");
    assert!(location.ends_with(&group_id.to_string()));

    // The root group is created once — the second redirect lands on it too.
    let resp = server
        .get(&format!("/accounts/{account}/root_outcome_group"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 302);
    assert_eq!(
        resp.json::<Value>()["id"].as_i64(),
        Some(group_id),
        "root group is stable"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /accounts/{accountId}/outcome_groups/{id}/subgroups
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_subgroup_create_mirrors_the_course_create() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Acct").await;
    let admin = seed_user(&pool, &prefix, "AcctAdmin", "teacher").await;
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account)
        .bind(admin)
        .execute(&pool)
        .await
        .expect("seed account admin");

    let root: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO outcome_groups (context_type, context_id, title, root_group)
        VALUES ('Account', $1, $2, TRUE)
        RETURNING id
        "#,
    )
    .bind(account)
    .bind(format!("{prefix} Root"))
    .fetch_one(&pool)
    .await
    .expect("seed root group");

    // Non-admin: 403 (after the account check).
    let resp = server
        .post(&format!(
            "/accounts/{account}/outcome_groups/{root}/subgroups"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "title": format!("{prefix} Child") }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Blank title: Canvas validation error.
    let resp = server
        .post(&format!(
            "/accounts/{account}/outcome_groups/{root}/subgroups"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .json(&json!({ "title": "   " }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    let resp = server
        .post(&format!(
            "/accounts/{account}/outcome_groups/{root}/subgroups"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .json(&json!({
            "title": format!("{prefix} Child"),
            "description": "child group"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], json!(format!("{prefix} Child")));
    assert_eq!(body["context_type"], json!("Account"));
    assert_eq!(body["context_id"], json!(account));
    assert_eq!(body["parent_outcome_group_id"], json!(root));
    assert_eq!(body["can_edit"], json!(true));
    assert_eq!(
        body["subgroups_url"],
        json!(format!(
            "/api/v1/accounts/{account}/outcome_groups/{}/subgroups",
            body["id"].as_i64().expect("child id")
        ))
    );

    // Unknown parent group: 404.
    let resp = server
        .post(&format!(
            "/accounts/{account}/outcome_groups/99999999/subgroups"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .json(&json!({ "title": format!("{prefix} Orphan") }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Unknown account: 404.
    let resp = server
        .post("/accounts/99999999/outcome_groups/1/subgroups")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .json(&json!({ "title": format!("{prefix} No") }))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// global outcomes_import family
// ---------------------------------------------------------------------------

#[tokio::test]
async fn global_outcomes_import_family_sync_stance() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;

    // available: admin-only Canvas flag.
    let resp = server
        .get("/global/outcomes_import/available")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin is 403");
    let resp = server
        .get("/global/outcomes_import/available")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({ "available": true }));

    // create: records a completed migration and answers with its id.
    let resp = server
        .post("/global/outcomes_import")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);
    let resp = server
        .post("/global/outcomes_import")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let migration_id = resp.json::<Value>()["migration_id"]
        .as_i64()
        .expect("migration id");

    let (state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM outcome_ab_migrations WHERE id = $1")
            .bind(migration_id)
            .fetch_one(&pool)
            .await
            .expect("reload migration");
    assert_eq!(state, "completed");

    // migration_status: the recorded migration, 404 for anything else.
    let resp = server
        .get(&format!(
            "/global/outcomes_import/migration_status/{migration_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["migration_status"]["id"], json!(migration_id));
    assert_eq!(
        body["migration_status"]["workflow_state"],
        json!("completed")
    );
    assert!(body["migration_status"]["created_at"].is_string());

    let resp = server
        .get("/global/outcomes_import/migration_status/99999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    let resp = server
        .get("/global/outcomes_import/migration_status/1")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin is 403");

    cleanup(&pool, &prefix).await;
}
