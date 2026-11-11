//! Integration tests for the Canvas blackout-date routes.
//!
//! Exercises the real grading-standards router:
//!   GET/POST/PUT /courses/{courseId}/blackout_dates
//!   GET/POST     /accounts/{accountId}/blackout_dates
//!   DELETE       /courses/{courseId}/blackout_dates/{id}
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-grading-standards
//!
//! Test cases:
//!   1. Course CRUD: POST 201 returns the Canvas shape (`event_title`),
//!      GET lists it, DELETE is 204 and a repeat DELETE is 404.
//!   2. Bulk PUT syncs: id-less entries are created, id entries updated,
//!      rows omitted from the payload are deleted.
//!   3. Authz: an enrolled student gets 403; validation errors are 422.
//!   4. Account routes: account admin can create/list; unknown account 404;
//!      a non-admin teacher is 403.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "bd-tests-{}-{}",
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
            .merge(star_grading_standards::router())
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

async fn cleanup(pool: &PgPool, prefix: &str, course_ids: &[i64]) {
    for course_id in course_ids {
        let _ = sqlx::query("DELETE FROM blackout_dates WHERE context_id = $1")
            .bind(course_id)
            .execute(pool)
            .await;
    }
    let _ = sqlx::query("DELETE FROM blackout_dates WHERE context_type = 'Account' AND context_id IN (SELECT id FROM accounts WHERE name LIKE $1)")
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
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn course_blackout_date_crud() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    let course = seed_course(&pool, &prefix, "Course1", account).await;

    let resp = server
        .post(&format!("/courses/{course}/blackout_dates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "event_title": "Winter break",
            "description": "Campus closed",
            "start_date": "2999-12-20",
            "end_date": "3000-01-02"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let created: Value = resp.json();
    let id = created["id"].as_i64().expect("id");
    assert_eq!(created["context_type"], "Course");
    assert_eq!(created["context_id"], course);
    assert_eq!(created["event_title"], "Winter break");
    assert_eq!(created["description"], "Campus closed");
    assert!(created["parent_grading_period_id"].is_null());

    let resp = server
        .get(&format!("/courses/{course}/blackout_dates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    assert_eq!(listed.as_array().expect("array").len(), 1);
    assert_eq!(listed[0]["id"], id);
    assert_eq!(listed[0]["event_title"], "Winter break");

    let resp = server
        .delete(&format!("/courses/{course}/blackout_dates/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    let resp = server
        .delete(&format!("/courses/{course}/blackout_dates/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix, &[course]).await;
}

#[tokio::test]
async fn bulk_update_creates_updates_and_deletes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    let course = seed_course(&pool, &prefix, "Course1", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // Seed two rows: one will be updated, one dropped from the payload.
    let keep: i64 = sqlx::query_scalar(
        "INSERT INTO blackout_dates (context_id, context_type, title, start_date, end_date)
         VALUES ($1, 'Course', 'Keep', '2999-01-01', '2999-01-02') RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed keep");
    let drop: i64 = sqlx::query_scalar(
        "INSERT INTO blackout_dates (context_id, context_type, title, start_date, end_date)
         VALUES ($1, 'Course', 'Drop', '2999-02-01', '2999-02-02') RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed drop");

    let resp = server
        .put(&format!("/courses/{course}/blackout_dates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "blackout_dates": [
                {"id": keep, "event_title": "Renamed", "start_date": "2999-01-01", "end_date": "2999-01-03"},
                {"event_title": "New", "start_date": "2999-03-01", "end_date": "2999-03-02"}
            ]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let rows = body.as_array().expect("array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], keep);
    assert_eq!(rows[0]["event_title"], "Renamed");
    assert_eq!(rows[1]["event_title"], "New");

    let remaining: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM blackout_dates WHERE context_id = $1")
            .bind(course)
            .fetch_all(&pool)
            .await
            .expect("count rows");
    assert!(!remaining.contains(&drop));

    cleanup(&pool, &prefix, &[course]).await;
}

#[tokio::test]
async fn student_forbidden_and_validation_errors() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    let course = seed_course(&pool, &prefix, "Course1", account).await;
    seed_enrollment(&pool, student, course, "student").await;

    let resp = server
        .get(&format!("/courses/{course}/blackout_dates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .post(&format!("/courses/{course}/blackout_dates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({"event_title": "X", "start_date": "2999-01-01", "end_date": "2999-01-02"}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // event_title is required.
    let resp = server
        .post(&format!("/courses/{course}/blackout_dates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({"start_date": "2999-01-01", "end_date": "2999-01-02"}))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    // end_date before start_date.
    let resp = server
        .post(&format!("/courses/{course}/blackout_dates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "event_title": "Backwards",
            "start_date": "2999-01-05",
            "end_date": "2999-01-01"
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    // Unknown grading period.
    let resp = server
        .post(&format!("/courses/{course}/blackout_dates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "event_title": "GP",
            "start_date": "2999-01-01",
            "end_date": "2999-01-02",
            "parent_grading_period_id": 999_999_999
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix, &[course]).await;
}

#[tokio::test]
async fn account_blackout_dates_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_admin = seed_user(&pool, &prefix, "AcctAdmin", "user").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    seed_account_admin(&pool, account, account_admin).await;

    let resp = server
        .post(&format!("/accounts/{account}/blackout_dates"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "user"),
        )
        .json(&json!({
            "event_title": "Account holiday",
            "start_date": "2999-05-01",
            "end_date": "2999-05-02"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["context_type"], "Account");
    assert_eq!(created["context_id"], account);
    assert_eq!(created["event_title"], "Account holiday");

    let resp = server
        .get(&format!("/accounts/{account}/blackout_dates"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "user"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    assert_eq!(listed.as_array().expect("array").len(), 1);

    // Non-admin, non-enrolled teacher is forbidden.
    let resp = server
        .get(&format!("/accounts/{account}/blackout_dates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unknown account is 404.
    let resp = server
        .get("/accounts/999999999/blackout_dates")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "user"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix, &[]).await;
}
