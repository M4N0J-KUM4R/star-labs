//! Observer alert thresholds (Canvas `ObserverAlertThresholdsApiController`)
//! integration tests.
//!
//! Covers the Canvas routes:
//! - `GET|POST /api/v1/users/:user_id/observer_alert_thresholds`
//! - `GET|PUT|DELETE /api/v1/users/:user_id/observer_alert_thresholds/:observer_alert_threshold_id`
//!
//! Fleet rule: dedicated users, observers and thresholds per test, every
//! seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

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
        "oa-{}-{}-{}",
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
        TestServer::new(star_enrollments::router().with_state(state)).expect("build TestServer");
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

/// Link an observer to the student it watches.
async fn link_observer(pool: &PgPool, observer_id: i64, student_id: i64) {
    sqlx::query(
        "INSERT INTO observer_enrollments (observer_id, user_id, course_id) \
         VALUES ($1, $2, NULL) ON CONFLICT DO NOTHING",
    )
    .bind(observer_id)
    .bind(student_id)
    .execute(pool)
    .await
    .expect("link the observer");
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query(
        "DELETE FROM observer_alert_thresholds WHERE observer_id IN \
             (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query(
        "DELETE FROM observer_enrollments WHERE observer_id IN \
             (SELECT id FROM users WHERE email LIKE $1)",
    )
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
async fn a_threshold_is_created_read_updated_and_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    link_observer(&pool, observer, student).await;

    let resp = server
        .post(&format!("/users/{observer}/observer_alert_thresholds"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({
            "observer_alert_threshold": {
                "alert_type": "assignment_grade_low",
                "threshold": "50",
                "user_id": student
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["alert_type"], "assignment_grade_low");
    assert_eq!(body["threshold"], "50");
    assert_eq!(body["user_id"].as_i64(), Some(student));
    assert_eq!(body["observer_id"].as_i64(), Some(observer));
    let id = body["id"].as_i64().expect("threshold id");

    // List them.
    let resp = server
        .get(&format!("/users/{observer}/observer_alert_thresholds"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().expect("thresholds").len(), 1);

    // Show one.
    let resp = server
        .get(&format!("/users/{observer}/observer_alert_thresholds/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Update it.
    let resp = server
        .put(&format!("/users/{observer}/observer_alert_thresholds/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "observer_alert_threshold": { "threshold": "75" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["threshold"], "75");

    // Delete it.
    let resp = server
        .delete(&format!("/users/{observer}/observer_alert_thresholds/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_unknown_alert_type_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    link_observer(&pool, observer, student).await;

    let resp = server
        .post(&format!("/users/{observer}/observer_alert_thresholds"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({
            "observer_alert_threshold": { "alert_type": "not_an_alert", "user_id": student }
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn another_observer_may_not_read_a_threshold() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    link_observer(&pool, observer, student).await;

    let resp = server
        .post(&format!("/users/{observer}/observer_alert_thresholds"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({
            "observer_alert_threshold": {
                "alert_type": "course_grade_low",
                "threshold": "20",
                "user_id": student
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let id = body["id"].as_i64().expect("threshold id");

    let resp = server
        .get(&format!("/users/{observer}/observer_alert_thresholds/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
