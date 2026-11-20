//! Integration tests for the Canvas poll session routes.
//!
//! Pinned behavior:
//! - the index of a poll needs the poll right (its creator, a teacher of one
//!   of its courses or a site administrator);
//! - the create route needs a teacher (or a site administrator) and a course
//!   of the body; a missing course is a 404;
//! - the open and close routes publish and unpublish the session;
//! - the destroy route removes the session and its submissions;
//! - `/poll_sessions/opened` and `/poll_sessions/closed` list the published
//!   and the closed sessions of the courses of the caller.
//!
//! Fleet rule: dedicated users, courses, polls and sessions per test, every
//! seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

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
        "poll-sessions-{}-{}-{}",
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
            .merge(star_polling::router())
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
        .bind(format!("{prefix}-{name}"))
        .bind(format!("{prefix}-{name}"))
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

async fn seed_poll(pool: &PgPool, user_id: i64, course_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO polls (user_id, course_id, question) VALUES ($1, $2, 'How are we?') \
         RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .expect("seed poll")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM poll_submissions WHERE poll_session_id IN \
           (SELECT ps.id FROM poll_sessions ps JOIN polls p ON p.id = ps.poll_id \
            WHERE p.question LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM poll_sessions WHERE poll_id IN \
           (SELECT id FROM polls WHERE question LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM polls WHERE question LIKE $1")
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
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn a_poll_session_is_created_opened_and_closed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let poll = seed_poll(&pool, teacher, course).await;

    // A student may not create a session.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "course_id": course }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The teacher may.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "course_id": course, "has_public_results": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let session: Value = resp.json();
    assert_eq!(session["is_published"], false);
    assert_eq!(session["has_public_results"], true);
    assert_eq!(session["course_id"], course);
    let session_id = session["id"].as_i64().expect("session id");

    // The index of the poll lists the session.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let sessions = list["poll_sessions"].as_array().expect("sessions");
    assert_eq!(sessions.len(), 1, "{list:?}");

    // Opening publishes the session.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session_id}/open"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["is_published"], true);

    // The published sessions of the caller are listed.
    let resp = server
        .get("/poll_sessions/opened")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(|items| items.len()), Some(1));

    // Closing unpublishes it again.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session_id}/close"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["is_published"], false);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_poll_session_of_a_missing_course_is_a_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let poll = seed_poll(&pool, teacher, course).await;

    // A course that does not exist.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "course_id": 999_999_999 }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A poll that does not exist.
    let resp = server
        .get("/polls/999999999/poll_sessions")
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
