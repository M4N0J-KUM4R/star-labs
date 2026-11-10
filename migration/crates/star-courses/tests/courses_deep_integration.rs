//! Integration tests for the Phase 4 courses-deep routes:
//! - GET  /courses/{courseId}/content_share_users
//! - GET  /courses/{courseId}/activity_stream/summary
//! - GET  /users/{userId}/courses
//! - POST /courses/{courseId}/ping
//!
//! Same harness conventions as tests/courses_integration.rs: skip cleanly
//! when DATABASE_URL is unset, unique per-run prefix, full teardown.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "crs-deep-{}-{}",
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
            .merge(star_courses::router())
            .with_state(state),
    )
    .expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id"#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id"#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(r#"INSERT INTO accounts (name) VALUES ($1) RETURNING id"#)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/content_share_users
// ---------------------------------------------------------------------------

#[tokio::test]
async fn content_share_users_requires_admin_and_search_term() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "share-acct").await;
    let course_id = seed_course(&pool, &prefix, "share-course", account_id).await;
    let admin = seed_user(&pool, &prefix, "share-admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "share-teacher", "teacher").await;
    sqlx::query("INSERT INTO account_users (account_id, user_id) VALUES ($1, $2)")
        .bind(account_id)
        .bind(admin)
        .execute(&pool)
        .await
        .expect("seed account admin");
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'teacher')")
        .bind(teacher)
        .bind(course_id)
        .execute(&pool)
        .await
        .expect("seed teacher enrollment");

    // Non-admin caller -> 403 (Canvas :read_as_admin)
    let forbidden = server
        .get(&format!(
            "/courses/{course_id}/content_share_users?search_term=share"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    // Missing search_term -> 400 (Canvas reject!("Search term required"))
    let missing = server
        .get(&format!("/courses/{course_id}/content_share_users"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing.status_code(), 400, "{}", missing.text());

    // Admin + term -> 200 with the teacher as a candidate
    let resp = server
        .get(&format!(
            "/courses/{course_id}/content_share_users?search_term=share-teacher"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let users: Value = resp.json();
    let ids: Vec<i64> = users
        .as_array()
        .expect("bare array")
        .iter()
        .map(|u| u["id"].as_i64().expect("id"))
        .collect();
    assert!(ids.contains(&teacher), "teacher is a share candidate");
    assert!(users[0].get("short_name").is_some());
    assert!(users[0].get("sortable_name").is_some());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn content_share_users_unknown_course_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "share-missing", "admin").await;
    let resp = server
        .get("/courses/999999999/content_share_users?search_term=x")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);
    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /users/{userId}/courses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_courses_returns_enrolled_courses_for_self_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "uc-acct").await;
    let course_id = seed_course(&pool, &prefix, "uc-course", account_id).await;
    let student = seed_user(&pool, &prefix, "uc-student", "student").await;
    let other = seed_user(&pool, &prefix, "uc-other", "student").await;
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student')")
        .bind(student)
        .bind(course_id)
        .execute(&pool)
        .await
        .expect("seed enrollment");

    let resp = server
        .get(&format!("/users/{student}/courses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let courses: Value = resp.json();
    let ids: Vec<i64> = courses
        .as_array()
        .expect("bare array")
        .iter()
        .map(|c| c["id"].as_i64().expect("id"))
        .collect();
    assert_eq!(ids, vec![course_id]);

    // Another student cannot read this user's courses.
    let forbidden = server
        .get(&format!("/users/{student}/courses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /courses/{courseId}/ping  (+ empty activity stream summary)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ping_returns_success_and_activity_summary_is_empty_array() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "ping-acct").await;
    let course_id = seed_course(&pool, &prefix, "ping-course", account_id).await;
    let student = seed_user(&pool, &prefix, "ping-student", "student").await;
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student')")
        .bind(student)
        .bind(course_id)
        .execute(&pool)
        .await
        .expect("seed enrollment");

    let ping = server
        .post(&format!("/courses/{course_id}/ping"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(ping.status_code(), 200, "{}", ping.text());
    let body: Value = ping.json();
    assert_eq!(body, serde_json::json!({ "success": true }));

    let summary = server
        .get(&format!("/courses/{course_id}/activity_stream/summary"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(summary.status_code(), 200, "{}", summary.text());
    let items: Value = summary.json();
    assert_eq!(items, serde_json::json!([]));

    cleanup(&pool, &prefix).await;
}
