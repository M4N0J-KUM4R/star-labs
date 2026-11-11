//! Integration tests for Canvas rubric CSV template + export routes.
//!
//! Covers `GET /rubrics/upload_template`,
//! `POST /courses/{courseId}/rubrics/download_rubrics` and
//! `POST /accounts/{accountId}/rubrics/download_rubrics` through the real
//! star-rubrics router against Postgres.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-rubrics

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "rub-export-{}-{}",
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
            .merge(star_rubrics::router())
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

async fn seed_account(pool: &PgPool, prefix: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-account"))
        .fetch_one(pool)
        .await
        .expect("seed account")
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

async fn seed_rubric(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rubrics (course_id, title, points_possible) VALUES ($1, $2, 5.0) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed rubric")
}

async fn seed_criterion(pool: &PgPool, rubric_id: i64, description: &str, position: i32) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rubric_criteria (rubric_id, description, long_description, points, position) \
         VALUES ($1, $2, $3, 3.0, $4) RETURNING id",
    )
    .bind(rubric_id)
    .bind(description)
    .bind(format!("{description} long"))
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed criterion")
}

async fn seed_rating(pool: &PgPool, criterion_id: i64, description: &str, points: f64, pos: i32) {
    sqlx::query(
        "INSERT INTO rubric_ratings (criterion_id, description, points, position) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(criterion_id)
    .bind(description)
    .bind(points)
    .bind(pos)
    .execute(pool)
    .await
    .expect("seed rating");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM rubrics WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
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
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn upload_template_returns_canvas_csv_attachment() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;

    let response = server
        .get("/rubrics/upload_template")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;

    assert_eq!(response.status_code(), 200);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/csv")
    );
    let disposition = response
        .headers()
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        disposition.contains("import_rubric_template.csv"),
        "disposition: {disposition}"
    );

    let body = response.text();
    assert!(
        body.starts_with(
            "Rubric Name,Criteria Name,Criteria Description,Criteria Enable Range,Rating Name"
        ),
        "body: {body}"
    );
    assert!(body.contains("Rubric 1,Criteria 1,Criteria 1 Description,false"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_download_rubrics_exports_csv_and_validates_ids() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let rubric = seed_rubric(&pool, course, "Essay Rubric").await;
    let first = seed_criterion(&pool, rubric, "Thesis", 1).await;
    seed_rating(&pool, first, "Strong", 3.0, 1).await;
    seed_rating(&pool, first, "Weak", 1.0, 2).await;
    let second = seed_criterion(&pool, rubric, "Evidence", 2).await;
    seed_rating(&pool, second, "Solid", 2.0, 1).await;

    let response = server
        .post(&format!("/courses/{course}/rubrics/download_rubrics"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "rubric_ids": [rubric] }))
        .await;
    let status = response.status_code();
    let body = response.text();
    assert_eq!(status, 200, "body: {body}");
    let lines: Vec<&str> = body.lines().collect();
    // 4 base columns + 3 columns per rating (max 2 ratings on a criterion).
    assert_eq!(lines[0].split(',').count(), 10, "header: {}", lines[0]);
    assert!(lines
        .iter()
        .any(|line| line.starts_with("Essay Rubric,Thesis")));
    assert!(lines
        .iter()
        .any(|line| line.starts_with("Essay Rubric,Evidence")));

    // No ids supplied is Canvas' 400 with an `error` key.
    let missing = server
        .post(&format!("/courses/{course}/rubrics/download_rubrics"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({}))
        .await;
    assert_eq!(missing.status_code(), 400);
    let missing_body: Value = missing.json();
    assert_eq!(
        missing_body["error"].as_str(),
        Some("No rubric IDs provided")
    );

    // Unmatched ids are a 404 with the Canvas message.
    let unknown = server
        .post(&format!("/courses/{course}/rubrics/download_rubrics"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "rubric_ids": [999_999_999_i64] }))
        .await;
    assert_eq!(unknown.status_code(), 404);
    let unknown_body: Value = unknown.json();
    assert_eq!(unknown_body["error"].as_str(), Some("No rubrics found"));

    // Enrolled teachers export; unrelated users are forbidden.
    let forbidden = server
        .post(&format!("/courses/{course}/rubrics/download_rubrics"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "rubric_ids": [rubric] }))
        .await;
    assert_eq!(forbidden.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_download_rubrics_is_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix).await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let rubric = seed_rubric(&pool, course, "Account Rubric").await;
    let criterion = seed_criterion(&pool, rubric, "Clarity", 1).await;
    seed_rating(&pool, criterion, "Clear", 2.0, 1).await;

    let forbidden = server
        .post(&format!("/accounts/{account}/rubrics/download_rubrics"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "rubric_ids": [rubric] }))
        .await;
    assert_eq!(forbidden.status_code(), 403);

    let response = server
        .post(&format!("/accounts/{account}/rubrics/download_rubrics"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "rubric_ids": [rubric] }))
        .await;
    let status = response.status_code();
    let body = response.text();
    assert_eq!(status, 200, "body: {body}");
    assert!(body.contains("Account Rubric,Clarity"));

    // Unknown accounts are 404 before rubric validation.
    let missing_account = server
        .post("/accounts/999999999/rubrics/download_rubrics")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "rubric_ids": [rubric] }))
        .await;
    assert_eq!(missing_account.status_code(), 404);

    cleanup(&pool, &prefix).await;
}
