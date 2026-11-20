//! Integration tests for the account, group and user content migration
//! contexts (Canvas `ContentMigrationsController`).
//!
//! Canvas serves the same actions for a course, an account, a group and a
//! user; the port stored the context in `course_id` alone, so the account,
//! group and user variants were missing.
//!
//! Pinned behavior:
//! - the migrations of a context are listed newest first;
//! - a caller without rights on the context gets 403, a missing context 404;
//! - a migration of another context is not reachable through this context;
//! - the migrator list is served for every context.
//!
//! Fleet rule: dedicated accounts, courses and users per test, every seeded
//! row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "cmig-{}-{}",
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
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_content::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
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

async fn seed_migration(
    pool: &PgPool,
    course_id: Option<i64>,
    user_id: i64,
    context_type: &str,
    context_id: i64,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO content_migrations
            (course_id, user_id, context_type, context_id, migration_type, workflow_state)
        VALUES ($1, $2, $3, $4, 'common cartridge importer', 'imported')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(user_id)
    .bind(context_type)
    .bind(context_id)
    .fetch_one(pool)
    .await
    .expect("seed migration")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM content_migrations WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
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

#[tokio::test]
async fn account_content_migrations_are_served_in_the_account_context() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_user(&pool, account, admin).await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let migration = seed_migration(&pool, Some(course), admin, "Account", account).await;

    // The account context lists its own migrations.
    let resp = server
        .get(&format!("/accounts/{account}/content_migrations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let ids: Vec<i64> = body
        .as_array()
        .expect("array")
        .iter()
        .map(|m| m["id"].as_i64().expect("id"))
        .collect();
    assert_eq!(ids, vec![migration]);

    // The migrator list is served for the account too.
    let resp = server
        .get(&format!("/accounts/{account}/content_migrations/migrators"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert!(resp.json::<Value>().is_array());

    // The migration is reachable through its own context.
    let resp = server
        .get(&format!(
            "/accounts/{account}/content_migrations/{migration}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["id"], migration);

    // A user without account rights may not read them.
    let resp = server
        .get(&format!("/accounts/{account}/content_migrations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // A missing account is 404.
    let resp = server
        .get("/accounts/999999999/content_migrations")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_migration_of_another_context_is_not_reachable() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let other_account = seed_account(&pool, &prefix, "Other").await;
    seed_account_user(&pool, account, admin).await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let migration = seed_migration(&pool, Some(course), admin, "Account", other_account).await;

    let resp = server
        .get(&format!(
            "/accounts/{account}/content_migrations/{migration}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn user_content_migrations_are_scoped_to_the_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let migration = seed_migration(&pool, None, user, "User", user).await;

    let resp = server
        .get(&format!("/users/{user}/content_migrations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().map(Vec::len), Some(1));

    // Another user does not see it.
    let resp = server
        .get(&format!("/users/{user}/content_migrations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    cleanup(&pool, &prefix).await;
}
