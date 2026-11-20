//! Integration tests for the Canvas AI experience routes.
//!
//! Pinned behavior:
//! - the `ai_experiences` feature flag gates the whole family: without it the
//!   routes answer the Canvas 404 `{"error": "Resource Not Found"}`;
//! - the index lists the experiences of a course and needs a member;
//! - the create route needs a teacher (or a site administrator) and stores
//!   the experience unpublished;
//! - the show route answers 404 for an experience of another course.
//!
//! Fleet rule: dedicated users, accounts, courses and experiences per test,
//! every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "ai-exp-{}-{}-{}",
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
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_courses::router())
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

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// Enable the `ai_experiences` feature for the account of the course.
async fn enable_feature(pool: &PgPool, account_id: i64) {
    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state) \
         VALUES ('Account', $1, 'ai_experiences', 'ai_experiences', 'enabled') \
         ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'enabled'",
    )
    .bind(account_id)
    .execute(pool)
    .await
    .expect("enable the feature");
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM ai_conversations WHERE ai_experience_id IN \
           (SELECT id FROM ai_experiences WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM ai_experiences WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Account' AND context_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
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

#[tokio::test]
async fn the_ai_experiences_need_the_feature_flag() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // Without the feature the family answers the Canvas 404.
    let resp = server
        .get(&format!("/courses/{course}/ai_experiences"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["error"], "Resource Not Found");

    enable_feature(&pool, account).await;

    let resp = server
        .get(&format!("/courses/{course}/ai_experiences"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert!(resp.json::<Value>().as_array().expect("array").is_empty());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_teacher_creates_an_ai_experience() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let other_course = seed_course(&pool, &prefix, "Other", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    enable_feature(&pool, account).await;

    // A student may not create an experience.
    let resp = server
        .post(&format!("/courses/{course}/ai_experiences"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "title": "Student" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The teacher may.
    let resp = server
        .post(&format!("/courses/{course}/ai_experiences"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "title": "Course facts",
            "description": "The facts of the course",
            "learning_objective": "Learn the course",
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let experience: Value = resp.json();
    assert_eq!(experience["title"], "Course facts");
    assert_eq!(experience["workflow_state"], "unpublished");
    let experience_id = experience["id"].as_i64().expect("experience id");

    // The index lists it.
    let resp = server
        .get(&format!("/courses/{course}/ai_experiences"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(|items| items.len()), Some(1));

    // The show route serves it.
    let resp = server
        .get(&format!("/courses/{course}/ai_experiences/{experience_id}"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["id"], experience_id);

    // An experience of another course is a 404.
    let resp = server
        .get(&format!(
            "/courses/{other_course}/ai_experiences/{experience_id}"
        ))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
