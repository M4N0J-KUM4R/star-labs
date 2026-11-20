//! Integration tests for the planner overrides (Canvas
//! `PlannerOverridesController`).
//!
//! Pinned behavior:
//! - the overrides are the current user's; another user's override is 404;
//! - `marked_complete` and `dismissed` follow Canvas' `value_to_boolean`;
//! - creating an override for a plannable that does not exist is 404, and an
//!   unknown plannable type is 400;
//! - deleting soft deletes the override and still renders it.
//!
//! Fleet rule: dedicated users per test, every seeded row is cleaned up.
//! Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "plov-{}-{}",
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
            .merge(star_calendar::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, 'student') RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_assignment(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    let course: i64 =
        sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix}-course"))
            .bind(format!("{prefix}-course"))
            .fetch_one(pool)
            .await
            .expect("seed course");
    sqlx::query_scalar("INSERT INTO assignments (course_id, title) VALUES ($1, $2) RETURNING id")
        .bind(course)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed assignment")
}

fn auth(user_id: i64) -> String {
    let token = star_auth::create_token(user_id, "student", "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM planner_overrides WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM assignments WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn planner_override_lifecycle() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User").await;
    let assignment = seed_assignment(&pool, &prefix, "Assignment").await;

    // Create.
    let resp = server
        .post("/planner/overrides")
        .add_header(axum::http::header::AUTHORIZATION, auth(user))
        .json(&serde_json::json!({
            "plannable_type": "assignment",
            "plannable_id": assignment,
            "marked_complete": "true"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let id = body["id"].as_i64().expect("id");
    assert_eq!(body["plannable_type"], "assignment");
    assert_eq!(body["marked_complete"], true);
    assert_eq!(body["dismissed"], false);
    assert_eq!(body["assignment_id"], assignment);

    // List.
    let resp = server
        .get("/planner/overrides")
        .add_header(axum::http::header::AUTHORIZATION, auth(user))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>().as_array().map(Vec::len), Some(1));

    // Update.
    let resp = server
        .put(&format!("/planner/overrides/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user))
        .json(&serde_json::json!({ "dismissed": "true" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["dismissed"], true);
    assert_eq!(body["marked_complete"], false);

    // Delete soft deletes but still renders the override.
    let resp = server
        .delete(&format!("/planner/overrides/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "deleted");

    // The deleted override is gone from the list and from show.
    let resp = server
        .get("/planner/overrides")
        .add_header(axum::http::header::AUTHORIZATION, auth(user))
        .await;
    assert_eq!(resp.json::<Value>().as_array().map(Vec::len), Some(0));
    let resp = server
        .get(&format!("/planner/overrides/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn planner_overrides_are_scoped_to_their_owner() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let owner = seed_user(&pool, &prefix, "Owner").await;
    let other = seed_user(&pool, &prefix, "Other").await;
    let assignment = seed_assignment(&pool, &prefix, "Assignment").await;

    let resp = server
        .post("/planner/overrides")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner))
        .json(&serde_json::json!({
            "plannable_type": "assignment",
            "plannable_id": assignment
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let id = resp.json::<Value>()["id"].as_i64().expect("id");

    // Another user does not see it and cannot change it.
    let resp = server
        .get(&format!("/planner/overrides/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .put(&format!("/planner/overrides/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other))
        .json(&serde_json::json!({ "marked_complete": "true" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn planner_override_rejects_unknown_plannables() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User").await;

    // An unknown plannable type is a bad request.
    let resp = server
        .post("/planner/overrides")
        .add_header(axum::http::header::AUTHORIZATION, auth(user))
        .json(&serde_json::json!({
            "plannable_type": "unicorn",
            "plannable_id": 1
        }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // A plannable that does not exist is a 404.
    let resp = server
        .post("/planner/overrides")
        .add_header(axum::http::header::AUTHORIZATION, auth(user))
        .json(&serde_json::json!({
            "plannable_type": "assignment",
            "plannable_id": 999999999
        }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // A missing plannable type is a bad request.
    let resp = server
        .post("/planner/overrides")
        .add_header(axum::http::header::AUTHORIZATION, auth(user))
        .json(&serde_json::json!({ "plannable_id": 1 }))
        .await;
    assert_eq!(resp.status_code(), 400);

    cleanup(&pool, &prefix).await;
}
