//! Integration tests for the Canvas submissions API in star-assignments.
//!
//! Covers the Phase 3 routes:
//! - GET  /courses/{courseId}/students/submissions
//! - GET/PUT /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}
//! - PUT  /courses/{courseId}/submissions/bulk_mark_read
//! - POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/files
//!
//! Skipped cleanly when DATABASE_URL is not set, mirroring
//! assignments_integration.rs. Uses a unique email/course prefix per run and
//! full teardown so it is safe to run repeatedly against a shared database.

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
        "subapi-tests-{}-{}",
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

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

/// GET a path with a bearer token, asserting 200 and returning the JSON.
async fn get_json(server: &TestServer, token: &str, path: &str) -> Value {
    let resp = server
        .get(path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str, points: f64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, points_possible)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(points)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_submission(pool: &PgPool, assignment_id: i64, user_id: i64, body: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO submissions (assignment_id, user_id, body)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(body)
    .fetch_one(pool)
    .await
    .expect("seed submission")
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
// GET /courses/{courseId}/students/submissions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn students_submissions_scopes_students_and_groups() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let carol = seed_user(&pool, &prefix, "Carol", "student").await;
    let course = seed_course(&pool, &prefix, "SubList").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    seed_enrollment(&pool, carol, course, "student").await;

    let a1 = seed_assignment(&pool, course, "Essay", 10.0).await;
    let a2 = seed_assignment(&pool, course, "Quiz", 20.0).await;
    let alice_sub = seed_submission(&pool, a1, alice, "alice answer").await;
    let bob_sub = seed_submission(&pool, a2, bob, "bob answer").await;

    let teacher_token = auth(teacher, "teacher");

    // Flat list for two students.
    let listed = get_json(
        &server,
        &teacher_token,
        &format!("/courses/{course}/students/submissions?student_ids={alice},{bob}"),
    )
    .await;
    let entries = listed.as_array().expect("flat array");
    assert_eq!(entries.len(), 2, "two submissions: {listed}");
    assert_eq!(entries[0]["user_id"], alice);
    assert_eq!(entries[0]["assignment_id"], a1);
    assert_eq!(entries[0]["body"], "alice answer");
    assert_eq!(entries[0]["workflow_state"], "submitted");
    assert_eq!(entries[0]["missing"], false);
    assert_eq!(entries[0]["attempt"], 1);
    assert_eq!(
        entries[0]["url"],
        format!("/courses/{course}/assignments/{a1}/submissions/{alice}")
    );
    assert_eq!(entries[1]["user_id"], bob);

    // assignment_ids[] narrows the list.
    let filtered = get_json(
        &server,
        &teacher_token,
        &format!(
            "/courses/{course}/students/submissions?student_ids={alice},{bob}&assignment_ids={a1}"
        ),
    )
    .await;
    assert_eq!(filtered.as_array().unwrap().len(), 1, "{filtered}");

    // Students cannot read other students, but default to themselves.
    let forbidden = server
        .get(&format!(
            "/courses/{course}/students/submissions?student_ids={bob}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    let own = get_json(
        &server,
        &auth(alice, "student"),
        &format!("/courses/{course}/students/submissions"),
    )
    .await;
    let own_entries = own.as_array().expect("own array");
    assert_eq!(own_entries.len(), 1);
    assert_eq!(own_entries[0]["id"], alice_sub);

    // Grouped mode returns one entry per enrolled student, including Carol.
    let grouped = get_json(
        &server,
        &teacher_token,
        &format!("/courses/{course}/students/submissions?grouped=true&student_ids=all"),
    )
    .await;
    let groups = grouped.as_array().expect("grouped array");
    assert_eq!(groups.len(), 3, "three students: {grouped}");
    let carol_group = groups
        .iter()
        .find(|group| group["user_id"] == carol)
        .expect("carol group");
    assert_eq!(carol_group["submissions"].as_array().unwrap().len(), 0);
    let bob_group = groups
        .iter()
        .find(|group| group["user_id"] == bob)
        .expect("bob group");
    assert_eq!(bob_group["submissions"].as_array().unwrap().len(), 1);
    assert_eq!(bob_group["submissions"][0]["id"], bob_sub);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET/PUT /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_submission_synthesizes_unsubmitted_and_canvas_grade_payload() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "SingleSub").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Homework", 10.0).await;

    let path = format!("/courses/{course}/assignments/{assignment}/submissions/{student}");
    let teacher_token = auth(teacher, "teacher");

    // No submission yet: Canvas returns a synthesized unsubmitted record.
    let unsubmitted = get_json(&server, &teacher_token, &path).await;
    assert!(unsubmitted["id"].is_null(), "{unsubmitted}");
    assert_eq!(unsubmitted["workflow_state"], "unsubmitted");
    assert_eq!(unsubmitted["missing"], true);
    assert_eq!(unsubmitted["user_id"], student);
    assert_eq!(unsubmitted["assignment_id"], assignment);

    // Canvas' submission[posted_grade] payload creates and grades the row.
    let graded = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "submission": { "posted_grade": "8.5" } }))
        .await;
    assert_eq!(graded.status_code(), 200, "{}", graded.text());
    assert_eq!(graded.json::<Value>()["score"].as_f64(), Some(8.5));

    let fetched = get_json(&server, &teacher_token, &path).await;
    assert!(fetched["id"].is_i64());
    assert_eq!(fetched["score"].as_f64(), Some(8.5));
    assert_eq!(fetched["grade"], "8.5");
    assert_eq!(fetched["workflow_state"], "graded");
    assert_eq!(fetched["grader_id"], teacher);
    assert!(fetched["graded_at"].is_string());
    assert_eq!(fetched["missing"], false);

    // The legacy flat {score} payload still works on the nested route.
    let legacy = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "score": 6.0 }))
        .await;
    assert_eq!(legacy.status_code(), 200, "{}", legacy.text());
    assert_eq!(legacy.json::<Value>()["score"].as_f64(), Some(6.0));

    // A blank posted_grade clears the grade (Canvas posted_grade=).
    let cleared = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token)
        .json(&json!({ "submission": { "posted_grade": "" } }))
        .await;
    assert_eq!(cleared.status_code(), 200, "{}", cleared.text());
    let cleared: Value = cleared.json();
    assert!(cleared["score"].is_null(), "{cleared}");

    // Students cannot grade, and outsiders cannot read.
    let student_grade = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "submission": { "posted_grade": 10.0 } }))
        .await;
    assert_eq!(student_grade.status_code(), 403, "{}", student_grade.text());

    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let outsider_read = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(outsider_read.status_code(), 403, "{}", outsider_read.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /courses/{courseId}/submissions/bulk_mark_read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bulk_mark_read_marks_only_callers_submissions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let course = seed_course(&pool, &prefix, "BulkRead").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Reading", 5.0).await;
    let alice_sub = seed_submission(&pool, assignment, alice, "a").await;
    let bob_sub = seed_submission(&pool, assignment, bob, "b").await;

    // JSON body: Alice asks to mark both her own and Bob's submission read.
    let resp = server
        .put(&format!("/courses/{course}/submissions/bulk_mark_read"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "submissionIds": [alice_sub, bob_sub] }))
        .await;
    assert_eq!(resp.status_code(), 204, "{}", resp.text());

    let read_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM submission_reads WHERE user_id = $1 AND submission_id = $2",
    )
    .bind(alice)
    .bind(alice_sub)
    .fetch_one(&pool)
    .await
    .expect("count alice read");
    assert_eq!(read_rows, 1);

    let bob_read_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM submission_reads WHERE user_id = $1 AND submission_id = $2",
    )
    .bind(alice)
    .bind(bob_sub)
    .fetch_one(&pool)
    .await
    .expect("count bob read as alice");
    assert_eq!(bob_read_rows, 0, "cannot mark another user's submission");

    // Form-encoded body is accepted too and upserts the same marker.
    let resp = server
        .put(&format!("/courses/{course}/submissions/bulk_mark_read"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .text(&format!("submissionIds[]={alice_sub}"))
        .await;
    assert_eq!(resp.status_code(), 204, "{}", resp.text());

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM submission_reads WHERE user_id = $1")
        .bind(alice)
        .fetch_one(&pool)
        .await
        .expect("count alice markers");
    assert_eq!(total, 1, "upsert, not duplicate");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST .../submissions/{userId}/files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submission_file_metadata_is_recorded_and_returned() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "SubFiles").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Upload", 10.0).await;

    let files_path =
        format!("/courses/{course}/assignments/{assignment}/submissions/{student}/files");

    // Student uploads their own file: record created, submission lazily made.
    let created = server
        .post(&files_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "filename": "essay.pdf",
            "content_type": "application/pdf",
            "size": 1234
        }))
        .await;
    assert_eq!(created.status_code(), 201, "{}", created.text());
    let created: Value = created.json();
    assert_eq!(created["filename"], "essay.pdf");
    assert_eq!(created["display_name"], "essay.pdf");
    assert_eq!(created["content_type"], "application/pdf");
    assert_eq!(created["size"], 1234);

    let submission = get_json(
        &server,
        &auth(teacher, "teacher"),
        &format!("/courses/{course}/assignments/{assignment}/submissions/{student}"),
    )
    .await;
    assert_eq!(submission["submission_type"], "online_upload");
    assert_eq!(submission["workflow_state"], "submitted");
    let attachments = submission["attachments"]
        .as_array()
        .expect("attachments array");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0]["filename"], "essay.pdf");

    // Another student cannot upload on the student's behalf.
    let forbidden = server
        .post(&files_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "filename": "sneaky.pdf" }))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    // Teachers may upload on behalf; attachments accumulate.
    let teacher_upload = server
        .post(&files_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "notes.txt", "size": 42 }))
        .await;
    assert_eq!(
        teacher_upload.status_code(),
        201,
        "{}",
        teacher_upload.text()
    );
    assert_eq!(
        teacher_upload.json::<Value>()["content_type"],
        "application/octet-stream"
    );

    let submission = get_json(
        &server,
        &auth(teacher, "teacher"),
        &format!("/courses/{course}/assignments/{assignment}/submissions/{student}"),
    )
    .await;
    assert_eq!(submission["attachments"].as_array().unwrap().len(), 2);

    cleanup(&pool, &prefix).await;
}
