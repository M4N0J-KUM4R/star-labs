//! Integration tests for star-features (Canvas feature flags parity).
//!
//! Pinned behavior:
//! - `GET .../features` returns the feature catalog merged with the context's
//!   stored overrides; absent overrides read `disabled`.
//! - Account endpoints are admin-only (403 for anyone else); course list is
//!   enrolled-or-admin; course updates are teacher-or-admin; user list is
//!   self-or-admin.
//! - A missing account/course/user is a 404; an unknown feature or state on
//!   `PUT` is a 400; `on`/`off` are stored as `enabled`/`disabled`.
//!
//! Skipped cleanly when DATABASE_URL is unset; unique per-run prefix plus full
//! teardown.

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
        "ff_{}_{}",
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(format!("{prefix}-{name}"))
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
    let _ = sqlx::query("DELETE FROM feature_flags WHERE context_id IN (SELECT id FROM accounts WHERE name LIKE $1)")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM feature_flags WHERE context_id IN (SELECT id FROM courses WHERE course_code LIKE $1)")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
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
async fn account_features_are_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Root").await;

    let resp = server
        .get(&format!("/accounts/{account}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let features = body.as_array().expect("array of features");
    assert!(!features.is_empty());
    assert_eq!(features[0]["state"], "disabled");
    assert_eq!(features[0]["locking_account_id"], Value::Null);
    assert_eq!(features[0]["transitions"]["locked"], false);

    let resp = server
        .get(&format!("/accounts/{account}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    let resp = server
        .get("/accounts/999999999/features")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_feature_put_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    let bearer = auth(admin, "admin");
    let url = format!("/accounts/{account}/features/fancy_wickets");

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["feature"], "fancy_wickets");
    assert_eq!(body["state"], "enabled");
    assert_eq!(body["display_name"], "Fancy Wickets");

    let resp = server
        .get(&format!("/accounts/{account}/features"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    let listed: Value = resp.json();
    let flag = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["feature"] == "fancy_wickets")
        .expect("listed feature");
    assert_eq!(flag["state"], "enabled");

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "off" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["state"], "disabled");

    let resp = server
        .put(&format!("/accounts/{account}/features/not_a_feature"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .json(&json!({ "state": "bogus" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Course scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_feature_put_requires_teacher_or_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Flags").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let url = format!("/courses/{course}/features/automatic_essay_grading");

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "state": "enabled" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "state": "enabled" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["state"], "enabled");

    // An enrolled student can read the course feature list.
    let resp = server
        .get(&format!("/courses/{course}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    assert_eq!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["feature"] == "automatic_essay_grading")
            .unwrap()["state"],
        "enabled"
    );

    // Course updates are scoped by course: a missing course is a 404.
    let resp = server
        .put("/courses/999999999/features/fancy_wickets")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// User scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_features_are_self_or_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;

    let resp = server
        .get(&format!("/users/{alice}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body.as_array().unwrap().len() >= 2);

    let resp = server
        .get(&format!("/users/{alice}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    let resp = server
        .get(&format!("/users/{alice}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200);

    let resp = server
        .get("/users/999999999/features")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}
