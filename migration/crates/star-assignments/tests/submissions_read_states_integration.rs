//! Integration tests for Phase 4 submission parity in star-assignments.
//!
//! Covers:
//! - PUT /courses/{courseId}/submissions/{id}/clear_unread
//! - GET /courses/{courseId}/assignments/{assignmentId}/gradeable_students
//! - GET/PUT /courses/{courseId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}
//!
//! Skipped cleanly when DATABASE_URL is not set, mirroring
//! submissions_api_integration.rs. Unique prefix per run, full teardown.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "subread-tests-{}-{}",
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

    let server = TestServer::new(submissions_router(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

fn submissions_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_assignments::router())
        .with_state(state)
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role)
        VALUES ($1, $2, 'x', $3)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
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

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, $3, 'active')",
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
        r#"
        INSERT INTO assignments (course_id, title, points_possible)
        VALUES ($1, $2, 10)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_submission(
    pool: &PgPool,
    assignment_id: i64,
    user_id: i64,
    anonymous_id: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO submissions (assignment_id, user_id, body, anonymous_id)
        VALUES ($1, $2, 'answer', $3)
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(anonymous_id)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let user_ids = "SELECT id FROM users WHERE email LIKE $1";
    let course_ids = "SELECT id FROM courses WHERE course_code LIKE $1";
    let _ = sqlx::query(&format!(
        "DELETE FROM submissions WHERE user_id IN ({user_ids}) \
         OR assignment_id IN (SELECT id FROM assignments WHERE course_id IN ({course_ids}))",
    ))
    .bind(format!("{prefix}%@test.local"))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM assignments WHERE course_id IN ({course_ids})",
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM enrollments WHERE user_id IN ({user_ids})",
    ))
    .bind(format!("{prefix}%@test.local"))
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

// ---------------------------------------------------------------------------
// PUT /courses/{courseId}/submissions/{id}/clear_unread
// ---------------------------------------------------------------------------

#[tokio::test]
async fn clear_unread_marks_submission_read_and_always_returns_204() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ClearRead").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Reading").await;
    let submission = seed_submission(&pool, assignment, student, None).await;

    let path = format!("/courses/{course}/submissions/{submission}/clear_unread");
    let resp = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "{}", resp.text());

    let (read_state, source): (String, String) = sqlx::query_as(
        "SELECT read_state, source FROM submission_reads WHERE submission_id = $1 AND user_id = $2",
    )
    .bind(submission)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("read marker exists");
    assert_eq!(read_state, "read");
    assert_eq!(source, "clear_unread");

    // Repeating the call upserts instead of duplicating.
    let resp = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "{}", resp.text());
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM submission_reads WHERE submission_id = $1 AND user_id = $2",
    )
    .bind(submission)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("count markers");
    assert_eq!(rows, 1);

    // Unknown submission ids are ignored, still 204 (Canvas semantics).
    let resp = server
        .put(&format!("/courses/{course}/submissions/0/clear_unread"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "{}", resp.text());

    // Anonymous callers are rejected by the auth extractor.
    let resp = server.put(&path).await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/assignments/{assignmentId}/gradeable_students
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gradeable_students_requires_grades_permission_and_hides_names_when_anonymous() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alpha = seed_user(&pool, &prefix, "Alpha", "student").await;
    let beta = seed_user(&pool, &prefix, "Beta", "student").await;
    let course = seed_course(&pool, &prefix, "Gradeable").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alpha, course, "student").await;
    seed_enrollment(&pool, beta, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Exam").await;
    seed_submission(&pool, assignment, alpha, Some("anon-alpha")).await;
    seed_submission(&pool, assignment, beta, Some("anon-beta")).await;

    let path = format!("/courses/{course}/assignments/{assignment}/gradeable_students");

    let forbidden = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(alpha, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    let listed = server
        .get(&format!("{path}?sort=name&order=desc"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(listed.status_code(), 200, "{}", listed.text());
    let students: Vec<Value> = listed.json();
    let names: Vec<&str> = students
        .iter()
        .map(|s| s["display_name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, vec!["Beta", "Alpha"], "{students:?}");
    assert_eq!(students[0]["id"], beta);

    // Anonymous grading only applies when enabled and the flag is passed.
    let flagged = server
        .get(&format!("{path}?allow_new_anonymous_id=true"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let flagged: Vec<Value> = flagged.json();
    assert_eq!(
        flagged[0]["id"], alpha,
        "anonymous_grading false: {flagged:?}"
    );

    sqlx::query("UPDATE assignments SET anonymous_grading = TRUE WHERE id = $1")
        .bind(assignment)
        .execute(&pool)
        .await
        .expect("enable anonymous grading");

    let anonymous = server
        .get(&format!("{path}?allow_new_anonymous_id=true"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let anonymous: Vec<Value> = anonymous.json();
    let ids: Vec<&str> = anonymous
        .iter()
        .map(|s| s["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids, vec!["anon-alpha", "anon-beta"], "{anonymous:?}");
    assert_eq!(anonymous[0]["display_name"], "Student");

    let missing = server
        .get(&format!(
            "/courses/{course}/assignments/0/gradeable_students"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(missing.status_code(), 404, "{}", missing.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET/PUT .../anonymous_submissions/{anonymousId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anonymous_submission_is_fetched_and_graded_by_anonymous_id() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "AnonSub").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Anon").await;
    let submission = seed_submission(&pool, assignment, student, Some("anon-123")).await;

    let path = format!("/courses/{course}/assignments/{assignment}/anonymous_submissions/anon-123");
    let teacher_token = auth(teacher, "teacher");

    let shown = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .await;
    assert_eq!(shown.status_code(), 200, "{}", shown.text());
    let shown: Value = shown.json();
    assert_eq!(shown["id"], submission);
    assert_eq!(shown["anonymous_id"], "anon-123");
    assert_eq!(shown["assignment_id"], assignment);
    assert_eq!(shown["workflow_state"], "submitted");
    assert!(shown["score"].is_null(), "{shown}");
    assert_eq!(
        shown["url"],
        format!("/courses/{course}/assignments/{assignment}/anonymous_submissions/anon-123")
    );

    let forbidden = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    let unknown = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/anonymous_submissions/nope"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .await;
    assert_eq!(unknown.status_code(), 404, "{}", unknown.text());

    let graded = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "score": 9.5 }))
        .await;
    assert_eq!(graded.status_code(), 200, "{}", graded.text());
    let graded: Value = graded.json();
    assert_eq!(graded["score"].as_f64(), Some(9.5));
    assert_eq!(graded["grade"], "9.5");
    assert_eq!(graded["workflow_state"], "graded");

    let student_grade = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "score": 1.0 }))
        .await;
    assert_eq!(student_grade.status_code(), 403, "{}", student_grade.text());

    cleanup(&pool, &prefix).await;
}
