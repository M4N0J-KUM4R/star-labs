//! Integration tests for the Canvas enrollment "deep" endpoints:
//! temporary enrollment status and the last attended date.
//!
//! Conventions (mirrors `enrollments_integration.rs`):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Unique email/course-code prefix per run + full teardown, safe to repeat
//!   against a shared database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "enr-deep-{}-{}",
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

    let server = TestServer::new(star_enrollments_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn star_enrollments_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_enrollments::router())
        .with_state(state)
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
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_enrollment(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    role: &str,
    temporary_source_user_id: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO enrollments
            (user_id, course_id, role, workflow_state, temporary_enrollment_source_user_id)
        VALUES ($1, $2, $3, 'active', $4)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .bind(temporary_source_user_id)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// PUT /courses/{courseId}/users/{userId}/last_attended
// ---------------------------------------------------------------------------

#[tokio::test]
async fn last_attended_updates_enrollment_and_returns_date() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "LastAttended").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;
    let enrollment = seed_enrollment(&pool, student, course, "student", None).await;

    let resp = server
        .put(&format!(
            "/courses/{course}/users/{student}/last_attended?date=2024-03-04T05:06:07Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;

    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["date"], "2024-03-04T05:06:07Z");

    let (last_attended,): (Option<chrono::DateTime<chrono::Utc>>,) =
        sqlx::query_as("SELECT last_attended_at FROM enrollments WHERE id = $1")
            .bind(enrollment)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(last_attended.is_some(), "last_attended_at was not stored");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn last_attended_invalid_date_is_400_with_message() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "BadDate").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;

    let resp = server
        .put(&format!(
            "/courses/{course}/users/{student}/last_attended?date=not-a-date"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;

    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "Invalid date time input");

    // Missing `date` is also 400 in Canvas.
    let resp = server
        .put(&format!("/courses/{course}/users/{student}/last_attended"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn last_attended_requires_course_teacher() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "Forbidden").await;
    seed_enrollment(&pool, student, course, "student", None).await;

    let resp = server
        .put(&format!(
            "/courses/{course}/users/{student}/last_attended?date=2024-03-04T05:06:07Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;

    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unknown course is a 404 before the permission check passes.
    let resp = server
        .put(&format!(
            "/courses/999999999/users/{student}/last_attended?date=2024-03-04T05:06:07Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /users/{userId}/temporary_enrollment_status + bulk variant
// ---------------------------------------------------------------------------

#[tokio::test]
async fn temporary_enrollment_status_flags_and_bulk_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let provider = seed_user(&pool, &prefix, "Provider", "teacher").await;
    let recipient = seed_user(&pool, &prefix, "Recipient", "student").await;
    let course = seed_course(&pool, &prefix, "TempStatus").await;
    // Provider teaches; recipient holds a temporary enrollment sourced from
    // the provider, which makes the provider a provider and the recipient a
    // recipient.
    seed_enrollment(&pool, provider, course, "teacher", None).await;
    seed_enrollment(&pool, recipient, course, "student", Some(provider)).await;

    // Self-service: the provider reads their own status.
    let resp = server
        .get(&format!("/users/{provider}/temporary_enrollment_status"))
        .add_header(axum::http::header::AUTHORIZATION, auth(provider, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["is_provider"], true, "body: {body}");
    assert_eq!(body["is_recipient"], false, "body: {body}");
    assert_eq!(body["can_provide"], true, "body: {body}");

    // A non-admin cannot read somebody else's status.
    let resp = server
        .get(&format!("/users/{recipient}/temporary_enrollment_status"))
        .add_header(axum::http::header::AUTHORIZATION, auth(provider, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Admin bulk call returns the Canvas object keyed by user id.
    let resp = server
        .get(&format!(
            "/temporary_enrollment_status?user_ids={provider},{recipient}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let provider_key = provider.to_string();
    let recipient_key = recipient.to_string();
    assert_eq!(
        body.get(&provider_key).map(|v| &v["is_provider"]),
        Some(&json!(true)),
        "body: {body}"
    );
    assert_eq!(
        body.get(&recipient_key).map(|v| &v["is_recipient"]),
        Some(&json!(true)),
        "body: {body}"
    );

    // Missing user_ids is Canvas's 400 with an empty body object.
    let resp = server
        .get("/temporary_enrollment_status")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({}));

    cleanup(&pool, &prefix).await;
}
