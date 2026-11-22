//! Integration tests for the Canvas SIS import errors routes
//! (`SisImportsErrorsApiController` — spec `specs/p5-rest-sis.md` routes
//! 8–9).
//!
//! Pinned behavior (ported from
//! `canvas-src/app/controllers/sis_import_errors_api_controller.rb`):
//! - `GET /accounts/{id}/sis_import_errors` lists the account's errors,
//!   newest first, wrapped in `{"sis_import_errors": [...]}`;
//! - `GET /accounts/{id}/sis_imports/{id}/errors` narrows the listing to
//!   one import; an unknown import is a 404;
//! - `failure=true` keeps only the failed rows;
//! - a caller without `import_sis`/`manage_sis` is a 403 and an anonymous
//!   caller a 401; an unknown account a 404;
//! - the error JSON carries `sis_import_id`, `file`, `message`, `row_info`
//!   and `row` (`Api::V1::SisImportError`).
//!
//! Fleet rule: dedicated users, accounts, batches and errors per test,
//! every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is
//! unset.

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
        "siserrors-{}-{}-{}",
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
    // The crate root router mounts both the imports and the errors routes.
    let server = TestServer::new(star_sis::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str) -> i64 {
    // A root account with SIS imports enabled (Canvas enables them on
    // every root account — Account#enable_sis_imports).
    sqlx::query_scalar(
        "INSERT INTO accounts (name, allow_sis_import) \
         VALUES ($1, true) RETURNING id",
    )
    .bind(format!("{prefix}-account"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id) VALUES ($1, $2)")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
}

async fn seed_batch(pool: &PgPool, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO sis_batches (account_id, workflow_state, created_at) \
         VALUES ($1, 'imported', now()) RETURNING id",
    )
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed sis batch")
}

#[allow(clippy::too_many_arguments)]
async fn seed_error(
    pool: &PgPool,
    account_id: i64,
    batch_id: i64,
    file: &str,
    message: &str,
    row: i32,
    failure: bool,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO sis_import_errors \
             (account_id, sis_batch_id, file, message, row_info, row, failure) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(account_id)
    .bind(batch_id)
    .bind(file)
    .bind(message)
    .bind(format!("row {row} content"))
    .bind(row)
    .bind(failure)
    .fetch_one(pool)
    .await
    .expect("seed sis import error")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
    let _ = sqlx::query(
        "DELETE FROM sis_import_errors WHERE account_id IN \
         (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM sis_batches WHERE account_id IN \
         (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(&pattern)
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
        .bind(&pattern)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn the_errors_of_an_account_and_of_one_import_are_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix).await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    seed_account_admin(&pool, account, admin).await;
    let first = seed_batch(&pool, account).await;
    let second = seed_batch(&pool, account).await;

    seed_error(&pool, account, first, "courses.csv", "bad course", 3, true).await;
    seed_error(&pool, account, second, "users.csv", "bad user", 7, true).await;
    seed_error(&pool, account, second, "users.csv", "warning row", 9, false).await;

    // The account listing carries every error, newest first.
    let resp = server
        .get(&format!("/accounts/{account}/sis_import_errors"))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let errors = body["sis_import_errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 3, "body: {body}");
    assert_eq!(errors[0]["row"], json!(9));
    assert_eq!(errors[0]["sis_import_id"], json!(second));
    assert_eq!(errors[0]["file"], json!("users.csv"));
    assert_eq!(errors[0]["message"], json!("warning row"));
    assert!(errors[0]["row_info"].is_string());

    // The per-import listing narrows to the batch.
    let resp = server
        .get(&format!("/accounts/{account}/sis_imports/{first}/errors"))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let errors = body["sis_import_errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["sis_import_id"], json!(first));
    assert_eq!(errors[0]["message"], json!("bad course"));

    // An unknown import is a 404.
    let resp = server
        .get(&format!("/accounts/{account}/sis_imports/99999999/errors"))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_failure_filter_guards_and_account_rules_hold() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix).await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let nobody = seed_user(&pool, &prefix, "Nobody", "student").await;
    seed_account_admin(&pool, account, admin).await;
    let batch = seed_batch(&pool, account).await;
    seed_error(&pool, account, batch, "courses.csv", "failed row", 1, true).await;
    seed_error(
        &pool,
        account,
        batch,
        "courses.csv",
        "soft warning",
        2,
        false,
    )
    .await;

    // `failure=true` keeps only the failed rows.
    let resp = server
        .get(&format!(
            "/accounts/{account}/sis_import_errors?failure=true"
        ))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let errors = body["sis_import_errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 1, "body: {body}");
    assert_eq!(errors[0]["message"], json!("failed row"));

    // A caller without the SIS rights is a 403; an anonymous caller a 401;
    // an unknown account a 404.
    let resp = server
        .get(&format!("/accounts/{account}/sis_import_errors"))
        .add_header("Authorization", auth(nobody, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .get(&format!("/accounts/{account}/sis_import_errors"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .get("/accounts/999999999/sis_import_errors")
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
