//! Integration tests for submission comment attachments and comment editing.
//!
//! Exercises the real router against a real Postgres:
//! - POST   /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/files
//! - PUT    .../comments/{commentId}
//! - DELETE .../comments/{commentId} (existing) and the detail 404 afterwards
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
        "scf-tests-{}-{}",
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

fn files_path(course: i64, assignment: i64, user: i64) -> String {
    format!("{}/files", nested_path(course, assignment, user))
}

/// Seed a comment the way Canvas writes the thread: through the submission
/// update (`PUT /courses/:course_id/assignments/:assignment_id/submissions/:user_id`,
/// canvas-src/config/routes.rb:1578). There is no comments collection route.
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

/// `(workflow_state, comment, edited_at is set)` of a stored comment — the
/// shape the removed flat list used to return, read from its source table.
async fn comment_state(pool: &PgPool, comment_id: i64) -> (String, String, bool) {
    let row: (String, String, bool) = sqlx::query_as(
        "SELECT workflow_state, comment, edited_at IS NOT NULL FROM submission_comments WHERE id = $1",
    )
    .bind(comment_id)
    .fetch_one(pool)
    .await
    .expect("comment row");
    row
}

/// Attachment ids linked to a comment, oldest first.
async fn attached_ids(pool: &PgPool, comment_id: i64) -> Vec<i64> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM submission_comment_attachments WHERE comment_id = $1 ORDER BY id",
    )
    .bind(comment_id)
    .fetch_all(pool)
    .await
    .expect("attachment rows");
    rows.into_iter().map(|(id,)| id).collect()
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
    let _ = sqlx::query(
        "DELETE FROM submission_comment_attachments WHERE submission_id IN (
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
async fn comment_file_upload_records_metadata_and_is_scoped() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let submission = seed_submission(&pool, assignment, student).await;
    let files = files_path(course, assignment, student);

    // Teacher uploads metadata only: no binary, no URL.
    let resp = server
        .post(&files)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "filename": "draft.pdf",
            "content_type": "application/pdf",
            "size": 1234
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["submission_id"].as_i64(), Some(submission));
    assert_eq!(created["filename"].as_str(), Some("draft.pdf"));
    assert_eq!(
        created["display_name"].as_str(),
        Some("draft.pdf"),
        "display_name defaults to filename"
    );
    assert_eq!(created["content_type"].as_str(), Some("application/pdf"));
    assert_eq!(created["size"].as_i64(), Some(1234));
    assert!(created["comment_id"].is_null(), "upload starts unattached");
    assert!(created["created_at"].is_string());

    // The submission's own student may upload too; size is clamped >= 0 and
    // display_name is honored when provided.
    let resp = server
        .post(&files)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "filename": "x.png", "display_name": "Image.png", "size": -5 }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["display_name"].as_str(), Some("Image.png"));
    assert_eq!(created["size"].as_i64(), Some(0));
    assert_eq!(
        created["content_type"].as_str(),
        Some("application/octet-stream"),
        "content_type defaults to application/octet-stream"
    );

    // Outsiders and wrong course/assignment/user contexts are rejected.
    let resp = server
        .post(&files)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "filename": "nope.txt" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let wrong_course = files_path(other_course, assignment, student);
    let resp = server
        .post(&wrong_course)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "filename": "nope.txt" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Empty filename is a validation error.
    let resp = server
        .post(&files)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "filename": "   " }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn comment_files_link_to_comments_and_appear_in_detail() {
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

    // Canvas declares no comments create route, so the thread row is seeded
    // the way the submission write makes it and the upload links to it through
    // its own `comment_id` field.
    let comment_id = seed_comment(&pool, submission, student, "see attached").await;

    let resp = server
        .post(&files_path(course, assignment, student))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "filename": "rubric.pdf",
            "content_type": "application/pdf",
            "comment_id": comment_id
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let file_id = resp.json::<Value>()["id"].as_i64().unwrap();

    // Canvas declares no single-comment GET (routes.rb:1582-1583 is PUT +
    // DELETE only), so the comment shape is read from its table.
    let (state, text, was_edited) = comment_state(&pool, comment_id).await;
    assert_eq!(state, "active");
    assert_eq!(text, "see attached");
    assert!(!was_edited, "new comment is not edited");

    let attached: Vec<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, comment_id FROM submission_comment_attachments \
         WHERE submission_id = $1 ORDER BY id",
    )
    .bind(submission)
    .fetch_all(&pool)
    .await
    .expect("attachment rows");
    assert_eq!(attached, vec![(file_id, Some(comment_id))]);

    // Uploading straight onto an existing comment links immediately.
    let resp = server
        .post(&files_path(course, assignment, student))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "filename": "late.pdf", "comment_id": comment_id }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let late_id = resp.json::<Value>()["id"].as_i64().unwrap();
    let ids: Vec<i64> = attached_ids(&pool, comment_id).await;
    assert_eq!(ids, vec![file_id, late_id]);

    // Without `comment_id` the record stays unattached — nothing else attaches
    // it, because Canvas' link happens on the submission write in
    // star-assignments, not on a comments collection route.
    let resp = server
        .post(&files_path(course, assignment, student))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "filename": "loose.pdf" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    assert!(resp.json::<Value>()["comment_id"].is_null());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn comment_update_edits_text_and_sets_edited_at() {
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

    let comment_id = seed_comment(&pool, submission, student, "first draft").await;

    // The author edits the text; edited_at is stamped.
    let resp = server
        .put(&format!("{nested}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "comment": "final draft" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["comment"].as_str(), Some("final draft"));
    assert!(updated["edited_at"].is_string(), "edited_at must be set");

    // Same text again keeps the original edited_at.
    let edited_at = updated["edited_at"].as_str().unwrap().to_string();
    let resp = server
        .put(&format!("{nested}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "comment": "final draft" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["edited_at"].as_str(),
        Some(edited_at.as_str()),
        "unchanged text must not restamp edited_at"
    );

    // The stored thread reflects the edit; only the author (or admin) may edit.
    let (state, text, _) = comment_state(&pool, comment_id).await;
    assert_eq!(state, "active");
    assert_eq!(text, "final draft");

    let resp = server
        .put(&format!("{nested}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "comment": "teacher rewrite" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Empty comments are rejected.
    let resp = server
        .put(&format!("{nested}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "comment": "  " }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn comment_delete_soft_deletes_and_detail_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let classmate = seed_user(&pool, &prefix, "Classmate", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, classmate, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let submission = seed_submission(&pool, assignment, student).await;
    let nested = nested_path(course, assignment, student);

    let comment_id = seed_comment(&pool, submission, teacher, "to be deleted").await;

    // The comment is active before the delete (Canvas reads the thread through
    // the submission, there is no single-comment GET route).
    assert_eq!(comment_state(&pool, comment_id).await.0, "active");

    // Only the author (or admin) may delete; unchanged via the alias.
    let resp = server
        .delete(&format!("{nested}/{comment_id}"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(classmate, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .delete(&format!("{nested}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // Soft-deleted comments leave the active feed and are not editable.
    assert_eq!(
        comment_state(&pool, comment_id).await.0,
        "deleted",
        "deleted comment must leave the active feed"
    );
    let resp = server
        .put(&format!("{nested}/{comment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "comment": "resurrect" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
