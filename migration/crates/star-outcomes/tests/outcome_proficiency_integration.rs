//! Integration tests for the Canvas outcome-proficiency routes.
//!
//!   GET/POST /accounts/{accountId}/outcome_proficiency
//!   GET/POST /courses/{courseId}/outcome_proficiency
//!
//! Skipped cleanly when DATABASE_URL is not set.
//!
//! Cases:
//!   1. Account show 404 → POST upserts → show resolves own row; re-POST
//!      replaces the ratings atomically.
//!   2. Course show falls back to the account chain (Course#
//!      resolved_outcome_proficiency); a course-own row overrides.
//!   3. Authz: anonymous 401; unknown account/course 404; student cannot
//!      POST; teacher can POST on the course.
//!   4. Validation: empty ratings / blank description → 422.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "prof-tests-{}-{}",
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
            .merge(star_outcomes::router())
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
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
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM outcome_proficiency_ratings WHERE outcome_proficiency_id IN \
         (SELECT id FROM outcome_proficiencies WHERE context_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1) OR context_id IN \
           (SELECT id FROM courses WHERE name LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM outcome_proficiencies WHERE context_id IN \
         (SELECT id FROM accounts WHERE name LIKE $1) OR context_id IN \
         (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM account_users WHERE account_id IN (SELECT id FROM accounts WHERE name LIKE $1)")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
}

fn ratings() -> Value {
    json!([
    {"description": "Exceeds", "points": 4.0, "mastery": true, "color": "00FF00"},
    {"description": "Meets", "points": 3.0, "mastery": true, "color": "0000FF"},
    {"description": "Below", "points": 1.0, "mastery": false, "color": "FF0000"}
    ])
}

#[tokio::test]
async fn account_upsert_and_resolution() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let root = seed_account(&pool, &prefix, "root", None).await;
    let sub = seed_account(&pool, &prefix, "sub", Some(root)).await;
    seed_account_admin(&pool, root, admin).await;
    let hdr = auth(admin, "admin");

    // Show on a chain with nothing → 404.
    let resp = server
        .get(&format!("/accounts/{sub}/outcome_proficiency"))
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // POST on the ROOT account (sub inherits it).
    let resp = server
        .post(&format!("/accounts/{root}/outcome_proficiency"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"ratings": ratings()}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["context_type"], "Account");
    assert_eq!(body["context_id"], root);
    assert_eq!(body["ratings"].as_array().unwrap().len(), 3);
    assert_eq!(body["ratings"][0]["description"], "Exceeds");
    assert_eq!(body["ratings"][1]["mastery"], true);
    assert_eq!(body["ratings"][2]["color"], "FF0000");

    // Sub-account show resolves through the chain.
    let resp = server
        .get(&format!("/accounts/{sub}/outcome_proficiency"))
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["context_id"], root, "inherited from root");

    // Re-POST replaces ratings atomically.
    let resp = server
        .post(&format!("/accounts/{root}/outcome_proficiency"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"ratings": [
            {"description": "Single", "points": 2.0, "mastery": true, "color": "000000"}
        ]}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["ratings"].as_array().unwrap().len(), 1);
    assert_eq!(body["ratings"][0]["description"], "Single");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_fallback_and_override() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "acct", None).await;
    seed_account_admin(&pool, account, admin).await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Account-level proficiency for the fallback.
    let resp = server
        .post(&format!("/accounts/{account}/outcome_proficiency"))
        .add_header("Authorization", auth(admin, "admin"))
        .content_type("application/json")
        .json(&json!({"ratings": ratings()}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Course show resolves through the account.
    let resp = server
        .get(&format!("/courses/{course}/outcome_proficiency"))
        .add_header("Authorization", auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["context_type"], "Account", "falls back to account");

    // Teacher POSTs a course-own scale; it now overrides.
    let resp = server
        .post(&format!("/courses/{course}/outcome_proficiency"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({"ratings": [
            {"description": "Course Level", "points": 5.0, "mastery": true, "color": "123456"}
        ]}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let resp = server
        .get(&format!("/courses/{course}/outcome_proficiency"))
        .add_header("Authorization", auth(student, "student"))
        .await;
    let body: Value = resp.json();
    assert_eq!(body["context_type"], "Course", "own row overrides");
    assert_eq!(body["ratings"][0]["description"], "Course Level");

    // A course with neither own nor account proficiency → 404 (a fresh
    // account — `account` itself carries the fallback row above).
    let bare_account = seed_account(&pool, &prefix, "bare-acct", None).await;
    let other = seed_course(&pool, &prefix, "Bare", bare_account).await;
    let resp = server
        .get(&format!("/courses/{other}/outcome_proficiency"))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn authorization_and_validation() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "acct", None).await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Anonymous → 401 on both contexts.
    let resp = server
        .get(&format!("/accounts/{account}/outcome_proficiency"))
        .await;
    assert_eq!(resp.status_code(), 401);
    let resp = server
        .get(&format!("/courses/{course}/outcome_proficiency"))
        .await;
    assert_eq!(resp.status_code(), 401);

    // Unknown contexts → 404.
    let resp = server
        .get("/accounts/999999999/outcome_proficiency")
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .get("/courses/999999999/outcome_proficiency")
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Non-admin cannot POST on the account (manage_proficiency_scales).
    let resp = server
        .post(&format!("/accounts/{account}/outcome_proficiency"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({"ratings": ratings()}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Student cannot POST on the course.
    let resp = server
        .post(&format!("/courses/{course}/outcome_proficiency"))
        .add_header("Authorization", auth(student, "student"))
        .content_type("application/json")
        .json(&json!({"ratings": ratings()}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Validation: empty ratings → 422; blank description → 422.
    let resp = server
        .post(&format!("/courses/{course}/outcome_proficiency"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({"ratings": []}))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let resp = server
        .post(&format!("/courses/{course}/outcome_proficiency"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({"ratings": [{"description": "  ", "points": 1.0}]}))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
