//! Temporary enrollment pairings (Canvas
//! `TemporaryEnrollmentPairingsApiController`) integration tests.
//!
//! Covers the Canvas routes:
//! - `GET    /api/v1/accounts/:account_id/temporary_enrollment_pairings`
//! - `GET    /api/v1/accounts/:account_id/temporary_enrollment_pairings/:id`
//! - `GET    /api/v1/accounts/:account_id/temporary_enrollment_pairings/new`
//! - `POST   /api/v1/accounts/:account_id/temporary_enrollment_pairings`
//! - `DELETE /api/v1/accounts/:account_id/temporary_enrollment_pairings/:id`
//!
//! Fleet rule: dedicated account, account user and pairings per test, every
//! seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, AtomicU32::Ordering::Relaxed);
    format!(
        "tep-{}-{}-{}",
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
        TestServer::new(star_enrollments::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
    )
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// A root account (no parent) the pairings hang from.
async fn seed_root_account(pool: &PgPool, prefix: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id, workflow_state) \
         VALUES ($1, NULL, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-root"))
    .fetch_one(pool)
    .await
    .expect("seed root account")
}

/// A sub account of the root account.
async fn seed_sub_account(pool: &PgPool, prefix: &str, root_account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id, workflow_state) \
         VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-sub"))
    .bind(root_account_id)
    .fetch_one(pool)
    .await
    .expect("seed sub account")
}

/// An account user with a role that may manage the temporary enrollments.
async fn seed_account_user(pool: &PgPool, prefix: &str, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role) \
         VALUES ($1, $2, 'AccountAdmin') ON CONFLICT DO NOTHING",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account user");
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM temporary_enrollment_pairings WHERE root_account_id IN \
             (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE account_id IN \
             (SELECT id FROM accounts WHERE name LIKE $1)",
    )
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

/// The pairings of the root account are listed, created and deleted.
#[tokio::test]
async fn pairings_are_listed_created_and_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let root_account_id = seed_root_account(&pool, &prefix).await;
    let sub_account_id = seed_sub_account(&pool, &prefix, root_account_id).await;
    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    seed_account_user(&pool, &prefix, root_account_id, admin).await;

    let bearer = auth(admin, "admin");

    // The empty index.
    let resp = server
        .get(&format!(
            "/accounts/{sub_account_id}/temporary_enrollment_pairings"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "index; body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().map(|items| items.len()), Some(0));

    // The unsaved pairing.
    let resp = server
        .get(&format!(
            "/accounts/{sub_account_id}/temporary_enrollment_pairings/new"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "new; body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], Value::Null);
    assert_eq!(body["root_account_id"], root_account_id);
    assert_eq!(body["workflow_state"], "active");

    // The created pairing hangs from the root account of the context.
    let resp = server
        .post(&format!(
            "/accounts/{sub_account_id}/temporary_enrollment_pairings"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "ending_enrollment_state": "completed" }))
        .await;
    assert_eq!(resp.status_code(), 201, "create; body: {}", resp.text());
    let body: Value = resp.json();
    let id = body["id"].as_i64().expect("the id of the pairing");
    assert_eq!(body["root_account_id"], root_account_id);
    assert_eq!(body["ending_enrollment_state"], "completed");
    assert_eq!(body["workflow_state"], "active");

    // The index carries the pairing.
    let resp = server
        .get(&format!(
            "/accounts/{sub_account_id}/temporary_enrollment_pairings"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "index; body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().map(|items| items.len()), Some(1));

    // The pairing is shown on its own.
    let resp = server
        .get(&format!(
            "/accounts/{sub_account_id}/temporary_enrollment_pairings/{id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "show; body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], id);

    // The pairing is deleted.
    let resp = server
        .delete(&format!(
            "/accounts/{sub_account_id}/temporary_enrollment_pairings/{id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "destroy; body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/accounts/{sub_account_id}/temporary_enrollment_pairings/{id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 404, "the pairing is gone");

    cleanup(&pool, &prefix).await;
}

/// A caller without the manage right is refused, an unknown account is a 404.
#[tokio::test]
async fn the_guards_answer_403_and_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let root_account_id = seed_root_account(&pool, &prefix).await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let bearer = auth(student, "student");

    // A user without an account role may not manage the pairings.
    let resp = server
        .get(&format!(
            "/accounts/{root_account_id}/temporary_enrollment_pairings"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "index; body: {}", resp.text());

    // An unknown account is a 404.
    let resp = server
        .get("/accounts/999999999/temporary_enrollment_pairings")
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 404, "index; body: {}", resp.text());

    // An anonymous caller is a 401.
    let resp = server
        .get(&format!(
            "/accounts/{root_account_id}/temporary_enrollment_pairings"
        ))
        .await;
    assert_eq!(resp.status_code(), 401, "index; body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
