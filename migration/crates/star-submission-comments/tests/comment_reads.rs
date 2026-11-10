//! Integration tests for submission comment read state.
//!
//! Exercises the real router against a real Postgres:
//! - PUT /submissions/{submissionId}/comments/read
//! - GET /submissions/{submissionId}/comments/unread_count
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-submission-comments

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
        "scr-tests-{}-{}",
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
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_submission_comments::router())
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

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(course_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed enrollment");
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO assignments (course_id, title) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(title)
        .fetch_one(pool)
        .await
        .expect("seed assignment")
}

async fn seed_submission(pool: &PgPool, assignment_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO submissions (assignment_id, user_id, body) VALUES ($1, $2, 'seed') RETURNING id",
    )
    .bind(assignment_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

fn read_path(submission: i64) -> String {
    format!("/submissions/{submission}/comments/read")
}

fn unread_path(submission: i64) -> String {
    format!("/submissions/{submission}/comments/unread_count")
}

fn flat_path(submission: i64) -> String {
    format!("/submissions/{submission}/comments")
}

async fn unread(server: &TestServer, submission: i64, user: i64, role: &str) -> Value {
    let resp = server
        .get(&unread_path(submission))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    resp.json()
}

async fn post_comment(
    server: &TestServer,
    submission: i64,
    user: i64,
    role: &str,
    body: &str,
) -> i64 {
    let resp = server
        .post(&flat_path(submission))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
        .json(&json!({ "comment": body }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    resp.json::<Value>()["id"].as_i64().unwrap()
}

async fn mark_read(server: &TestServer, submission: i64, user: i64, role: &str) {
    let resp = server
        .put(&read_path(submission))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
    let _ = sqlx::query(
        "DELETE FROM submission_comment_reads WHERE submission_id IN (
             SELECT s.id FROM submissions s
             JOIN assignments a ON a.id = s.assignment_id
             JOIN courses c ON c.id = a.course_id
             WHERE c.course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM submission_comments WHERE submission_id IN (
             SELECT s.id FROM submissions s
             JOIN assignments a ON a.id = s.assignment_id
             JOIN courses c ON c.id = a.course_id
             WHERE c.course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM submissions WHERE assignment_id IN (
             SELECT a.id FROM assignments a
             JOIN courses c ON c.id = a.course_id
             WHERE c.course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    for query in [
        "DELETE FROM assignments WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM enrollments WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM courses WHERE course_code LIKE $1",
    ] {
        let _ = sqlx::query(query).bind(&pattern).execute(pool).await;
    }
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn unread_count_tracks_comments_since_mark_read() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let submission = seed_submission(&pool, assignment, student).await;

    // No comments yet.
    assert_eq!(
        unread(&server, submission, teacher, "teacher").await["unread_count"],
        json!(0)
    );

    // A student comment is unread for the teacher, but never for its author.
    post_comment(&server, submission, student, "student", "first").await;
    assert_eq!(
        unread(&server, submission, teacher, "teacher").await["unread_count"],
        json!(1)
    );
    assert_eq!(
        unread(&server, submission, student, "student").await["unread_count"],
        json!(0)
    );

    // Marking read clears the teacher's count and is idempotent.
    mark_read(&server, submission, teacher, "teacher").await;
    assert_eq!(
        unread(&server, submission, teacher, "teacher").await["unread_count"],
        json!(0)
    );
    mark_read(&server, submission, teacher, "teacher").await;
    assert_eq!(
        unread(&server, submission, teacher, "teacher").await["unread_count"],
        json!(0)
    );

    // A later comment becomes unread again.
    post_comment(&server, submission, student, "student", "second").await;
    assert_eq!(
        unread(&server, submission, teacher, "teacher").await["unread_count"],
        json!(1)
    );
    // The teacher's own reply does not add to their unread count.
    post_comment(&server, submission, teacher, "teacher", "reply").await;
    assert_eq!(
        unread(&server, submission, teacher, "teacher").await["unread_count"],
        json!(1)
    );
    // ...but it is unread for the student.
    assert_eq!(
        unread(&server, submission, student, "student").await["unread_count"],
        json!(1)
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn soft_deleted_comments_are_not_unread() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let submission = seed_submission(&pool, assignment, student).await;

    let first = post_comment(&server, submission, student, "student", "first").await;
    mark_read(&server, submission, teacher, "teacher").await;
    let second = post_comment(&server, submission, student, "student", "second").await;
    assert_eq!(
        unread(&server, submission, teacher, "teacher").await["unread_count"],
        json!(1)
    );

    // Soft-deleting the new comment removes it from the unread count.
    let resp = server
        .delete(&format!("{}/{}", flat_path(submission), second))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    assert_eq!(
        unread(&server, submission, teacher, "teacher").await["unread_count"],
        json!(0)
    );

    // The older read comment is still listed but not unread.
    let list: Vec<Value> = server
        .get(&flat_path(submission))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"].as_i64(), Some(first));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn read_state_requires_comment_access() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let classmate = seed_user(&pool, &prefix, "Classmate", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, classmate, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let submission = seed_submission(&pool, assignment, student).await;

    post_comment(&server, submission, student, "student", "only").await;

    for (user, role) in [(classmate, "student"), (outsider, "student")] {
        let resp = server
            .get(&unread_path(submission))
            .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
        let resp = server
            .put(&read_path(submission))
            .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    }

    // Unknown submission is 404 for both routes.
    let missing = 9_000_000_001_i64;
    let resp = server
        .get(&unread_path(missing))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .put(&read_path(missing))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A forbidden mark-read never created a marker: the teacher still sees 1.
    assert_eq!(
        unread(&server, submission, teacher, "teacher").await["unread_count"],
        json!(1)
    );

    cleanup(&pool, &prefix).await;
}
