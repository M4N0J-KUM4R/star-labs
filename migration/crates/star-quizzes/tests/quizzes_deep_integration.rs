//! Integration tests for quizzes-deep routes: reorder, validate_access_code, statistics.
//!
//! Skipped cleanly when DATABASE_URL is not set.

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
        "quiz-deep-{}-{}",
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
            .merge(star_quizzes::router())
            .with_state(state),
    )
    .expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO users (name, email, password_hash, role)
           VALUES ($1, $2, $3, $4) RETURNING id"#,
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
        r#"INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id"#,
    )
    .bind(name)
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

async fn seed_quiz(pool: &PgPool, course_id: i64, title: &str, access_code: Option<&str>) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO quizzes (course_id, title, quiz_type, access_code, published, points_possible)
           VALUES ($1, $2, 'assignment', $3, true, 10) RETURNING id"#,
    )
    .bind(course_id)
    .bind(title)
    .bind(access_code)
    .fetch_one(pool)
    .await
    .expect("seed quiz")
}

async fn seed_question(pool: &PgPool, quiz_id: i64, position: i32) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO quiz_questions (quiz_id, position, question_name, question_type, question_text, points_possible, answers)
           VALUES ($1, $2, 'Q1', 'multiple_choice_question', 'What is 2+2?', 5, '[]') RETURNING id"#,
    )
    .bind(quiz_id)
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed question")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/quizzes/{quizId}/reorder
/// - Teacher gets 204
/// - Student gets 403
/// - Missing order key gets 400
#[tokio::test]
async fn reorder_quiz() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course = seed_course(&pool, &prefix, "Test Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let quiz = seed_quiz(&pool, course, "Reorder Quiz", None).await;
    let q1 = seed_question(&pool, quiz, 1).await;
    let q2 = seed_question(&pool, quiz, 2).await;

    let teacher_auth = auth(teacher, "teacher");
    let student_auth = auth(student, "student");

    // Student gets 403
    let resp = server
        .post(&format!("/courses/{}/quizzes/{}/reorder", course, quiz))
        .add_header(axum::http::header::AUTHORIZATION, &student_auth)
        .json(&json!({ "order": [{ "type": "question", "id": q1 }] }))
        .await;
    assert_eq!(resp.status_code(), 403, "student reorder: {}", resp.text());

    // Teacher gets 204
    let resp = server
        .post(&format!("/courses/{}/quizzes/{}/reorder", course, quiz))
        .add_header(axum::http::header::AUTHORIZATION, &teacher_auth)
        .json(&json!({ "order": [{ "type": "question", "id": q2 }, { "type": "question", "id": q1 }] }))
        .await;
    assert_eq!(resp.status_code(), 204, "teacher reorder: {}", resp.text());

    // Missing order key gets 400
    let resp = server
        .post(&format!("/courses/{}/quizzes/{}/reorder", course, quiz))
        .add_header(axum::http::header::AUTHORIZATION, &teacher_auth)
        .json(&json!({ "not_order": [] }))
        .await;
    assert_eq!(resp.status_code(), 400, "missing order: {}", resp.text());

    // Unknown type gets 400
    let resp = server
        .post(&format!("/courses/{}/quizzes/{}/reorder", course, quiz))
        .add_header(axum::http::header::AUTHORIZATION, &teacher_auth)
        .json(&json!({ "order": [{ "type": "invalid", "id": q1 }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "invalid type: {}", resp.text());

    // Teardown
    sqlx::query("DELETE FROM quiz_questions WHERE quiz_id = $1").bind(quiz).execute(&pool).await.expect("teardown questions");
    sqlx::query("DELETE FROM quizzes WHERE id = $1").bind(quiz).execute(&pool).await.expect("teardown quiz");
    sqlx::query("DELETE FROM enrollments WHERE course_id = $1").bind(course).execute(&pool).await.expect("teardown enrollments");
    sqlx::query("DELETE FROM courses WHERE id = $1").bind(course).execute(&pool).await.expect("teardown course");
    sqlx::query("DELETE FROM users WHERE id = ANY($1)").bind(vec![teacher, student]).execute(&pool).await.expect("teardown users");
}

/// POST /courses/{courseId}/quizzes/{quizId}/validate_access_code
/// - Correct code returns true, wrong returns false
/// - Any enrolled user (student or teacher) can call it
#[tokio::test]
async fn validate_access_code() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "teacher2", "teacher").await;
    let student = seed_user(&pool, &prefix, "student2", "student").await;
    let course = seed_course(&pool, &prefix, "Access Code Test").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let quiz = seed_quiz(&pool, course, "Code Quiz", Some("secret123")).await;

    let teacher_auth = auth(teacher, "teacher");
    let student_auth = auth(student, "student");

    // Correct code → true
    let resp = server
        .post(&format!("/courses/{}/quizzes/{}/validate_access_code", course, quiz))
        .add_header(axum::http::header::AUTHORIZATION, &teacher_auth)
        .json(&json!({ "access_code": "secret123" }))
        .await;
    assert_eq!(resp.status_code(), 200, "correct code: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!(true), "correct code body");

    // Wrong code → false
    let resp = server
        .post(&format!("/courses/{}/quizzes/{}/validate_access_code", course, quiz))
        .add_header(axum::http::header::AUTHORIZATION, &student_auth)
        .json(&json!({ "access_code": "wrong" }))
        .await;
    assert_eq!(resp.status_code(), 200, "wrong code: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!(false), "wrong code body");

    // Teardown
    sqlx::query("DELETE FROM quizzes WHERE id = $1").bind(quiz).execute(&pool).await.expect("teardown quiz");
    sqlx::query("DELETE FROM enrollments WHERE course_id = $1").bind(course).execute(&pool).await.expect("teardown enrollments");
    sqlx::query("DELETE FROM courses WHERE id = $1").bind(course).execute(&pool).await.expect("teardown course");
    sqlx::query("DELETE FROM users WHERE id = ANY($1)").bind(vec![teacher, student]).execute(&pool).await.expect("teardown users");
}

/// GET /courses/{courseId}/quizzes/{quizId}/statistics
/// - Teacher/admin gets 200 with quiz_statistics array
/// - Student gets 403
#[tokio::test]
async fn quiz_statistics() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "teacher3", "teacher").await;
    let student = seed_user(&pool, &prefix, "student3", "student").await;
    let course = seed_course(&pool, &prefix, "Stats Test").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let quiz = seed_quiz(&pool, course, "Stats Quiz", None).await;

    let teacher_auth = auth(teacher, "teacher");
    let student_auth = auth(student, "student");

    // Student gets 403
    let resp = server
        .get(&format!("/courses/{}/quizzes/{}/statistics", course, quiz))
        .add_header(axum::http::header::AUTHORIZATION, &student_auth)
        .await;
    assert_eq!(resp.status_code(), 403, "student stats: {}", resp.text());

    // Teacher gets 200 with quiz_statistics envelope
    let resp = server
        .get(&format!("/courses/{}/quizzes/{}/statistics", course, quiz))
        .add_header(axum::http::header::AUTHORIZATION, &teacher_auth)
        .await;
    assert_eq!(resp.status_code(), 200, "teacher stats: {}", resp.text());
    let body: Value = resp.json();
    assert!(body.get("quiz_statistics").is_some(), "missing quiz_statistics key: {}", body);
    let stats = &body["quiz_statistics"];
    assert!(stats.is_array(), "quiz_statistics should be array: {}", body);
    assert_eq!(stats.as_array().unwrap().len(), 1, "one stats entry");
    let entry = &stats[0];
    assert_eq!(entry["quiz_id"], json!(quiz), "quiz_id matches");
    assert_eq!(entry["submission_statistics"]["unique_count"], json!(0), "no submissions yet");

    // Teardown
    sqlx::query("DELETE FROM quiz_questions WHERE quiz_id = $1").bind(quiz).execute(&pool).await.expect("teardown questions");
    sqlx::query("DELETE FROM quizzes WHERE id = $1").bind(quiz).execute(&pool).await.expect("teardown quiz");
    sqlx::query("DELETE FROM enrollments WHERE course_id = $1").bind(course).execute(&pool).await.expect("teardown enrollments");
    sqlx::query("DELETE FROM courses WHERE id = $1").bind(course).execute(&pool).await.expect("teardown course");
    sqlx::query("DELETE FROM users WHERE id = ANY($1)").bind(vec![teacher, student]).execute(&pool).await.expect("teardown users");
}