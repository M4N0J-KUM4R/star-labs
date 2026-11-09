//! Integration tests for star-groups.
//!
//! Exercises the real groups router (POST /courses/{courseId}/groups, POST
//! /groups/{groupId}/memberships) through axum-test against a real Postgres.
//!
//! Regression scope: every `SELECT 1 ...` existence probe in
//! star-groups/src/lib.rs must decode as (i32,), because Postgres types the
//! literal 1 as INT4. Decoding into (i64,) only fails when the probe finds a
//! row, so each test below uses an existing course/group/user: before the fix
//! these requests returned 500 instead of 201.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-groups

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "grp-tests-{}-{}",
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
            .merge(star_groups::router())
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

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_teacher(pool: &PgPool, user_id: i64, course_id: i64) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'teacher')")
        .bind(user_id)
        .bind(course_id)
        .execute(pool)
        .await
        .expect("seed teacher enrollment");
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM groups WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
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
async fn create_group_for_existing_course_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;

    let resp = server
        .post(&format!("/courses/{course}/groups"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Study Group" }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["name"], "Study Group");
    assert_eq!(body["course_id"].as_i64(), Some(course));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_membership_for_existing_group_and_user_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;
    let group = seed_group(&pool, course, &format!("{prefix}-Group")).await;

    let resp = server
        .post(&format!("/groups/{group}/memberships"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "user_id": student }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["user_id"].as_i64(), Some(student));
    assert_eq!(body["workflow_state"], "invited");

    cleanup(&pool, &prefix).await;
}
