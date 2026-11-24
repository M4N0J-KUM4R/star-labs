//! Integration tests for star-submission-comments Canvas nested routes.
//!
//! Exercises the real router against a real Postgres:
//! - DELETE   /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}
//! - PUT      same nested member path
//!
//! Canvas declares no comments index or create route in the API route set
//! (routes.rb:1580-1583): the thread is read through
//! `include[]=submission_comments` and written through the submission update
//! (both star-assignments), so the read/create sides are seeded straight into
//! the `submission_comments` table here.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-submission-comments

use axum_test::TestServer;
use serde_json::json;
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

/// Seed a comment directly in the DB. Canvas has no comments collection route:
/// the thread is written through the submission update
/// (`PUT /courses/:course_id/assignments/:assignment_id/submissions/:user_id`,
/// canvas-src/config/routes.rb:1578), so tests create rows the same way the
/// graded submission write does. Returns the comment id.
async fn seed_comment(pool: &PgPool, submission_id: i64, author_id: i64, text: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO submission_comments (submission_id, author_id, comment) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(submission_id)
    .bind(author_id)
    .bind(text)
    .fetch_one(pool)
    .await
    .expect("seed comment")
}

/// Active comment ids of a submission, oldest first (the ordering the removed
/// flat `GET /submissions/{submissionId}/comments` used).
async fn active_comment_ids(pool: &PgPool, submission_id: i64) -> Vec<i64> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM submission_comments WHERE submission_id = $1 AND workflow_state = 'active' ORDER BY created_at ASC",
    )
    .bind(submission_id)
    .fetch_all(pool)
    .await
    .expect("active comment ids");
    rows.into_iter().map(|(id,)| id).collect()
}

/// Stored text of one comment.
async fn comment_text(pool: &PgPool, comment_id: i64) -> String {
    sqlx::query_scalar("SELECT comment FROM submission_comments WHERE id = $1")
        .bind(comment_id)
        .fetch_one(pool)
        .await
        .expect("comment text")
}

/// Whether the nested edit stamped `edited_at`.
async fn edited_at_set(pool: &PgPool, comment_id: i64) -> bool {
    sqlx::query_scalar("SELECT edited_at IS NOT NULL FROM submission_comments WHERE id = $1")
        .bind(comment_id)
        .fetch_one(pool)
        .await
        .expect("edited_at flag")
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
async fn nested_member_edit_delete_matches_stored_thread() {
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

    // The thread is written outside this crate (Canvas writes comments on the
    // submission update), so the comment row is seeded the same way.
    let created_id = seed_comment(&pool, submission, teacher, "seeded").await;

    // Editing through the nested member route updates the stored thread.
    let resp = server
        .put(&format!("{nested}/{created_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "comment": "via nested" }))
        .await;
    assert_eq!(resp.status_code(), 200, "nested edit: {}", resp.text());
    assert_eq!(
        active_comment_ids(&pool, submission).await,
        vec![created_id]
    );
    assert_eq!(comment_text(&pool, created_id).await, "via nested");
    assert!(
        edited_at_set(&pool, created_id).await,
        "nested edit stamps edited_at on comment {created_id}"
    );

    // Delete via the nested member route soft-deletes the row, so it drops out
    // of the active thread.
    let resp = server
        .delete(&format!("{nested}/{created_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    assert!(
        active_comment_ids(&pool, submission).await.is_empty(),
        "soft-deleted: {created_id}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_member_cross_course_or_wrong_user_is_404() {
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

    let comment_id = seed_comment(&pool, submission, teacher, "existing").await;

    // The assignment belongs to `course`, not `other_course`.
    let wrong_course = nested_path(other_course, assignment, student);
    for (label, resp) in [
        (
            "put",
            server
                .put(&format!("{wrong_course}/{comment_id}"))
                .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
                .json(&json!({ "comment": "cross-course" }))
                .await,
        ),
        (
            "delete",
            server
                .delete(&format!("{wrong_course}/{comment_id}"))
                .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
                .await,
        ),
    ] {
        assert_eq!(
            resp.status_code(),
            404,
            "wrong course {label}: {}",
            resp.text()
        );
    }

    // `other` has no submission for this assignment.
    let wrong_user = nested_path(course, assignment, other);
    let resp = server
        .delete(&format!("{wrong_user}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "wrong user: {}", resp.text());

    // The comment survived every mismatched request.
    assert_eq!(
        active_comment_ids(&pool, submission).await,
        vec![comment_id]
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_member_keeps_existing_authorization() {
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

    let comment_id = seed_comment(&pool, submission, teacher, "teacher comment").await;

    // A non-enrolled outsider cannot reach the thread.  (The thread read is the
    // submission payload in Canvas, so the write routes carry the check here.)
    let resp = server
        .put(&format!("{nested}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "comment": "not allowed" }))
        .await;
    assert_eq!(resp.status_code(), 403, "nested edit: {}", resp.text());
    let resp = server
        .delete(&format!("{nested}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "nested delete: {}", resp.text());

    // Only the comment author (or admin) may edit or delete.
    for (label, resp) in [
        (
            "classmate edit",
            server
                .put(&format!("{nested}/{comment_id}"))
                .add_header(
                    axum::http::header::AUTHORIZATION,
                    auth(classmate, "student"),
                )
                .json(&json!({ "comment": "not yours" }))
                .await,
        ),
        (
            "classmate delete",
            server
                .delete(&format!("{nested}/{comment_id}"))
                .add_header(
                    axum::http::header::AUTHORIZATION,
                    auth(classmate, "student"),
                )
                .await,
        ),
    ] {
        assert_eq!(resp.status_code(), 403, "{label}: {}", resp.text());
    }

    // The comment survived the forbidden writes.
    assert_eq!(
        active_comment_ids(&pool, submission).await,
        vec![comment_id]
    );
    assert_eq!(
        comment_text(&pool, comment_id).await,
        "teacher comment",
        "forbidden writes left the stored comment untouched"
    );

    cleanup(&pool, &prefix).await;
}
