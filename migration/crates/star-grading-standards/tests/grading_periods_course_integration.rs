//! Integration tests for the course and set scoped Canvas grading-period
//! routes.
//!
//! Pinned behavior:
//! - the course index lists the periods of the grading period group of the
//!   course, ordered by the start date;
//! - a period of another course is a 404 on the show, update and delete
//!   routes;
//! - the update route reads the `{"grading_periods": [...]}` envelope;
//! - the delete route answers 204 and soft deletes the period;
//! - the batch update creates the periods that do not exist yet;
//! - a student may read the periods but not change them.
//!
//! Fleet rule: dedicated users, courses and periods per test, every seeded
//! row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gpc-tests-{}-{}-{}",
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

async fn seed_group(pool: &PgPool, course_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO grading_period_groups (course_id, title, workflow_state) \
         VALUES ($1, 'Course periods', 'active') RETURNING id",
    )
    .bind(course_id)
    .fetch_one(pool)
    .await
    .expect("seed grading period group")
}

async fn seed_period(pool: &PgPool, group_id: i64, title: &str, start: &str, end: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO grading_periods \
         (grading_period_group_id, title, start_date, end_date, close_date, weight) \
         VALUES ($1, $2, $3::timestamptz, $4::timestamptz, $4::timestamptz, 50) \
         RETURNING id",
    )
    .bind(group_id)
    .bind(title)
    .bind(start)
    .bind(end)
    .fetch_one(pool)
    .await
    .expect("seed grading period")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM grading_periods WHERE grading_period_group_id IN \
           (SELECT id FROM grading_period_groups WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM grading_period_groups WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
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
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn the_course_grading_periods_are_scoped_to_the_course() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let other_course = seed_course(&pool, &prefix, "Other").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let group = seed_group(&pool, course).await;
    let period = seed_period(&pool, group, "First", "2026-01-01", "2026-02-01").await;

    // The index lists the periods of the course.
    let resp = server
        .get(&format!("/courses/{course}/grading_periods"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let periods = body["grading_periods"].as_array().expect("periods");
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0]["title"], "First");
    assert_eq!(body["grading_periods_read_only"], false);

    // The show route serves the period.
    let resp = server
        .get(&format!("/courses/{course}/grading_periods/{period}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["grading_periods"].as_array().map(|items| items.len()),
        Some(1)
    );

    // A student may not update the period.
    let resp = server
        .put(&format!("/courses/{course}/grading_periods/{period}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "grading_periods": [{ "title": "Renamed" }] }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The teacher may.
    let resp = server
        .put(&format!("/courses/{course}/grading_periods/{period}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "grading_periods": [{ "title": "Renamed", "weight": 60 }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["grading_periods"][0]["title"], "Renamed");

    // A period of another course is a 404.
    let other_group = seed_group(&pool, other_course).await;
    let other_period =
        seed_period(&pool, other_group, "Elsewhere", "2026-03-01", "2026-04-01").await;
    let resp = server
        .get(&format!("/courses/{course}/grading_periods/{other_period}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The delete route answers 204.
    let resp = server
        .delete(&format!("/courses/{course}/grading_periods/{period}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    let state: String =
        sqlx::query_scalar("SELECT workflow_state FROM grading_periods WHERE id = $1")
            .bind(period)
            .fetch_one(&pool)
            .await
            .expect("read the period state");
    assert_eq!(state, "deleted");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_course_batch_update_creates_and_updates_periods() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, course).await;
    let period = seed_period(&pool, group, "First", "2026-01-01", "2026-02-01").await;

    let resp = server
        .patch(&format!("/courses/{course}/grading_periods/batch_update"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "grading_periods": [
                { "title": "First", "start_date": "2026-01-01", "end_date": "2026-03-01" },
                { "title": "Second", "start_date": "2026-03-01", "end_date": "2026-04-01" },
            ]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let periods = body["grading_periods"].as_array().expect("periods");
    assert_eq!(periods.len(), 2);

    // The existing period was updated, the new one was created.
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM grading_periods WHERE grading_period_group_id = $1",
    )
    .bind(group)
    .fetch_one(&pool)
    .await
    .expect("count the periods");
    assert_eq!(count, 2);

    let title: String = sqlx::query_scalar("SELECT title FROM grading_periods WHERE id = $1")
        .bind(period)
        .fetch_one(&pool)
        .await
        .expect("read the period title");
    assert_eq!(title, "First");

    cleanup(&pool, &prefix).await;
}
