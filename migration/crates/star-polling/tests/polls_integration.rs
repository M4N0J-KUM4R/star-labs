//! Polls and poll choices (Canvas `Polling::PollsController` and
//! `Polling::PollChoicesController`) integration tests.
//!
//! Covers the Canvas routes:
//! - `GET|POST /api/v1/polls`
//! - `GET|PUT|DELETE /api/v1/polls/:id`
//! - `GET|POST /api/v1/polls/:poll_id/poll_choices`
//! - `GET|PUT|DELETE /api/v1/polls/:poll_id/poll_choices/:id`
//!
//! Fleet rule: dedicated users and polls per test, every seeded row is
//! cleaned up. Skipped cleanly when DATABASE_URL is unset.

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
        "po-{}-{}-{}",
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
        TestServer::new(star_polling::router().with_state(state)).expect("build TestServer");
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

async fn seed_poll(pool: &PgPool, prefix: &str, user_id: i64, question: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO polls (user_id, question) VALUES ($1, $2) RETURNING id")
        .bind(user_id)
        .bind(format!("{prefix}-{question}"))
        .fetch_one(pool)
        .await
        .expect("seed poll")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query(
        "DELETE FROM poll_choices WHERE poll_id IN \
             (SELECT id FROM polls WHERE question LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query("DELETE FROM polls WHERE question LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
}

#[tokio::test]
async fn a_poll_is_created_read_updated_and_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;

    let resp = server
        .post("/polls")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "polls": [{ "question": "A poll question" }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let polls = body["polls"].as_array().expect("polls");
    assert_eq!(polls.len(), 1, "body: {body}");
    assert_eq!(polls[0]["question"], "A poll question");
    // The owner sees the teacher keys.
    assert!(polls[0]["user_id"].is_string(), "body: {body}");
    assert!(polls[0].get("total_results").is_some(), "body: {body}");
    let id = polls[0]["id"].as_str().expect("poll id").to_string();

    // Read it back.
    let resp = server
        .get(&format!("/polls/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Update it.
    let resp = server
        .put(&format!("/polls/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "polls": [{ "question": "Another question" }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["polls"][0]["question"], "Another question");

    // Delete it.
    let resp = server
        .delete(&format!("/polls/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!("/polls/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_poll_of_another_user_is_forbidden() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let owner = seed_user(&pool, &prefix, "Owner", "teacher").await;
    let other = seed_user(&pool, &prefix, "Other", "teacher").await;
    let poll = seed_poll(&pool, &prefix, owner, "A poll question").await;

    let resp = server
        .get(&format!("/polls/{poll}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_choices_of_a_poll_are_managed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let poll = seed_poll(&pool, &prefix, teacher, "A poll question").await;

    let resp = server
        .post(&format!("/polls/{poll}/poll_choices"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "poll_choices": [{ "text": "A Poll Choice", "position": 1, "is_correct": true }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let choices = body["poll_choices"].as_array().expect("poll choices");
    assert_eq!(choices.len(), 1, "body: {body}");
    assert_eq!(choices[0]["text"], "A Poll Choice");
    assert_eq!(choices[0]["position"], 1);
    assert_eq!(choices[0]["is_correct"], json!(true));
    let id = choices[0]["id"].as_str().expect("choice id").to_string();

    // List them.
    let resp = server
        .get(&format!("/polls/{poll}/poll_choices"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["poll_choices"].as_array().expect("choices").len(), 1);

    // Show one.
    let resp = server
        .get(&format!("/polls/{poll}/poll_choices/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Update it.
    let resp = server
        .put(&format!("/polls/{poll}/poll_choices/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "poll_choices": [{ "text": "Renamed" }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["poll_choices"][0]["text"], "Renamed");

    // Delete it.
    let resp = server
        .delete(&format!("/polls/{poll}/poll_choices/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_poll_without_a_question_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;

    let resp = server
        .post("/polls")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "polls": [{}] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
