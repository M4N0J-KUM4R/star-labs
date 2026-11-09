//! Integration tests for star-submission-comments Canvas nested route aliases.
//!
//! Exercises the real router against a real Postgres:
//! - GET/POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments
//! - DELETE   /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}
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
        "sc-tests-{}-{}",
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

fn nested_path(course: i64, assignment: i64, user: i64) -> String {
    format!("/courses/{course}/assignments/{assignment}/submissions/{user}/comments")
}

fn flat_path(submission: i64) -> String {
    format!("/submissions/{submission}/comments")
}

fn sorted_ids(list: &[Value]) -> Vec<i64> {
    let mut ids: Vec<i64> = list.iter().map(|c| c["id"].as_i64().unwrap()).collect();
    ids.sort_unstable();
    ids
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
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
async fn nested_alias_list_create_delete_match_flat_routes() {
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
    let nested = nested_path(course, assignment, student);
    let flat = flat_path(submission);

    // Create via the nested alias.
    let resp = server
        .post(&nested)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "comment": "via nested" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["submission_id"].as_i64(), Some(submission));
    assert_eq!(created["author_id"].as_i64(), Some(teacher));
    assert_eq!(created["workflow_state"], "active");
    let created_id = created["id"].as_i64().unwrap();

    // Create via the existing flat route.
    let resp = server
        .post(&flat)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "comment": "via flat" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let flat_created_id = resp.json::<Value>()["id"].as_i64().unwrap();

    // Nested list and flat list expose the same active comments.
    let nested_list: Vec<Value> = server
        .get(&nested)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    let flat_list: Vec<Value> = server
        .get(&flat)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(nested_list.len(), 2, "nested list: {nested_list:?}");
    assert_eq!(sorted_ids(&nested_list), sorted_ids(&flat_list));
    assert_eq!(sorted_ids(&nested_list), vec![created_id, flat_created_id]);

    // Delete via the nested alias, observe it through the flat route.
    let resp = server
        .delete(&format!("{nested}/{created_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let flat_list: Vec<Value> = server
        .get(&flat)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await
        .json();
    assert_eq!(sorted_ids(&flat_list), vec![flat_created_id]);

    // The flat delete route keeps working unchanged.
    let resp = server
        .delete(&format!("{flat}/{flat_created_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_alias_cross_course_or_wrong_user_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let submission = seed_submission(&pool, assignment, student).await;

    let resp = server
        .post(&flat_path(submission))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "comment": "existing" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let comment_id = resp.json::<Value>()["id"].as_i64().unwrap();

    // The assignment belongs to `course`, not `other_course`.
    let wrong_course = nested_path(other_course, assignment, student);
    let resp = server
        .get(&wrong_course)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .post(&wrong_course)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "comment": "cross-course" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // `other` has no submission for this assignment.
    let wrong_user = nested_path(course, assignment, other);
    let resp = server
        .get(&wrong_user)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Nested delete with a mismatched course must not touch the comment.
    let resp = server
        .delete(&format!("{wrong_course}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let flat_list: Vec<Value> = server
        .get(&flat_path(submission))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(sorted_ids(&flat_list), vec![comment_id]);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_alias_keeps_existing_authorization() {
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
    let nested = nested_path(course, assignment, student);

    let resp = server
        .post(&flat_path(submission))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "comment": "teacher comment" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let comment_id = resp.json::<Value>()["id"].as_i64().unwrap();

    // Non-enrolled outsider is forbidden on both flat and nested routes.
    let resp = server
        .get(&flat_path(submission))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "flat list: {}", resp.text());
    let resp = server
        .get(&nested)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "nested list: {}", resp.text());
    let resp = server
        .post(&nested)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "comment": "not allowed" }))
        .await;
    assert_eq!(resp.status_code(), 403, "nested create: {}", resp.text());

    // Only the comment author (or admin) may delete — unchanged via alias.
    let resp = server
        .delete(&format!("{nested}/{comment_id}"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(classmate, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "nested delete: {}", resp.text());
    let resp = server
        .delete(&format!("{}/{}", flat_path(submission), comment_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(classmate, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "flat delete: {}", resp.text());

    // The comment survived the forbidden deletes.
    let flat_list: Vec<Value> = server
        .get(&flat_path(submission))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await
        .json();
    assert_eq!(sorted_ids(&flat_list), vec![comment_id]);

    cleanup(&pool, &prefix).await;
}
