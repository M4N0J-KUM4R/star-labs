//! Peer reviews (Canvas `PeerReviewsApiController`) integration tests.
//!
//! Covers the Canvas routes:
//! - `GET    /api/v1/sections/:section_id/assignments/:assignment_id/peer_reviews`
//! - `GET    /api/v1/sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews`
//! - `POST   /api/v1/sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews`
//! - `DELETE /api/v1/sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews`
//! - `POST   /api/v1/courses/:course_id/assignments/:assignment_id/allocate`
//!
//! Fleet rule: dedicated users, accounts, courses, sections, assignments and
//! submissions per test, every seeded row is cleaned up. Skipped cleanly
//! when DATABASE_URL is unset.

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
        "pr-{}-{}-{}",
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
        TestServer::new(star_rubrics::router().with_state(state)).expect("build TestServer");
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id, workflow_state) \
         VALUES ($1, NULL, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id, workflow_state) \
         VALUES ($1, $2, $3, 'available') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_section(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO course_sections (course_id, name, workflow_state) \
         VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed section")
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

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, workflow_state) \
         VALUES ($1, $2, 'published') RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_submission(pool: &PgPool, assignment_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO submissions (assignment_id, user_id, submitted_at) \
         VALUES ($1, $2, now()) RETURNING id",
    )
    .bind(assignment_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query(
        "DELETE FROM assessment_requests WHERE asset_id IN \
             (SELECT s.id FROM submissions s \
              JOIN assignments a ON a.id = s.assignment_id \
              JOIN courses c ON c.id = a.course_id WHERE c.name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query(
        "DELETE FROM submissions WHERE assignment_id IN \
             (SELECT id FROM assignments WHERE course_id IN \
                 (SELECT id FROM courses WHERE name LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query(
        "DELETE FROM assignments WHERE course_id IN \
             (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN \
             (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query(
        "DELETE FROM course_sections WHERE course_id IN \
             (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query("DELETE FROM courses WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
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
async fn the_peer_reviews_of_an_assignment_are_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let section = seed_section(&pool, course, "Section One").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "An assignment").await;
    let submission = seed_submission(&pool, assignment, student).await;

    // The teacher reviews the student.
    let resp = server
        .post(&format!(
            "/sections/{section}/assignments/{assignment}/submissions/{submission}/peer_reviews"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "reviewer_id": teacher, "user_id": student }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["asset_id"].as_i64(), Some(submission));
    assert_eq!(body["workflow_state"], "assigned");

    // List them.
    let resp = server
        .get(&format!(
            "/sections/{section}/assignments/{assignment}/peer_reviews"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().expect("reviews").len(), 1, "body: {body}");

    // Delete them.
    let resp = server
        .delete(&format!(
            "/sections/{section}/assignments/{assignment}/submissions/{submission}/peer_reviews"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_review_of_oneself_is_rejected() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let section = seed_section(&pool, course, "Section One").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "An assignment").await;
    let submission = seed_submission(&pool, assignment, student).await;

    let resp = server
        .post(&format!(
            "/sections/{section}/assignments/{assignment}/submissions/{submission}/peer_reviews"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "reviewer_id": student, "user_id": student }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_allocation_answers_the_new_reviews() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "An assignment").await;
    let submission = seed_submission(&pool, assignment, student).await;

    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/allocate"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let reviews = body.as_array().expect("reviews");
    assert_eq!(reviews.len(), 1, "body: {body}");
    assert_eq!(reviews[0]["asset_id"].as_i64(), Some(submission));

    cleanup(&pool, &prefix).await;
}
