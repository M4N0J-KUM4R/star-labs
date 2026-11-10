//! Integration tests for the Canvas parity additions in star-users:
//! GET/PUT /users/{userId}/settings (including the `self` alias),
//! GET /users/self/todo, and POST /users/{userId}/files.
//!
//! Conventions (mirrors users_integration.rs):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-users
//! - Unique per-run prefixes + full teardown keep repeated runs safe.

use axum_test::TestServer;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix. Deliberately avoids the literal "-tests-" because
/// users_integration.rs sweeps leaked rows matching '%-tests-%' and must never
/// delete rows these tests are still using.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "p3uset{}{}",
        std::process::id(),
        Utc::now().timestamp_millis() + n as i64
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

    let server = TestServer::new(star_users_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn star_users_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_users::router())
        .with_state(state)
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(sortable_name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, label: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix} {label}"))
        .bind(format!("{prefix}-{label}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn enroll(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

async fn seed_assignment(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    due_at: Option<DateTime<Utc>>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible, due_at) \
         VALUES ($1, $2, 10, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(due_at)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_submission(pool: &PgPool, assignment_id: i64, user_id: i64, score: Option<f64>) {
    sqlx::query(
        "INSERT INTO submissions (assignment_id, user_id, body, score) VALUES ($1, $2, 'work', $3)",
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(score)
    .execute(pool)
    .await
    .expect("seed submission");
}

fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete everything this run created (users cascade to settings/files/
/// enrollments/submissions; courses cascade to assignments).
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

const SETTINGS_KEYS: [&str; 10] = [
    "manual_mark_as_read",
    "collapse_global_nav",
    "collapse_course_nav",
    "hide_dashcard_color_overlays",
    "release_notes_badge_disabled",
    "comment_library_suggestions_enabled",
    "elementary_dashboard_disabled",
    "default_to_block_editor",
    "widget_dashboard_user_preference",
    "widget_dashboard_dark_mode",
];

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn settings_defaults_put_roundtrip_and_mobile_include() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Set One", "student").await;

    // A user without a settings row reads as Canvas' all-false hash.
    let resp = server
        .get("/users/self/settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    for key in SETTINGS_KEYS {
        assert_eq!(body[key], false, "default {key} must be false: {body}");
    }
    assert!(
        body.get("pendo_mobile_teacher_api_key").is_none(),
        "mobile settings are opt-in: {body}"
    );

    // include[]=mobile_settings adds the Canvas mobile keys.
    let resp = server
        .get("/users/self/settings?include%5B%5D=mobile_settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body["pendo_mobile_teacher_api_key"].is_null());
    assert!(body["pendo_mobile_parent_api_key"].is_null());
    assert!(body["pendo_mobile_student_api_key"].is_null());
    assert_eq!(body["usage_metrics"], false);

    // Canvas value_to_boolean coercion: booleans, "1"/"0" and numbers.
    let resp = server
        .put(&format!("/users/{student}/settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "manual_mark_as_read": true,
            "collapse_global_nav": "1",
            "widget_dashboard_dark_mode": "false",
            "default_to_block_editor": 1
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["manual_mark_as_read"], true);
    assert_eq!(body["collapse_global_nav"], true);
    assert_eq!(body["widget_dashboard_dark_mode"], false);
    assert_eq!(body["default_to_block_editor"], true);
    // Unspecified preferences keep the all-false default.
    assert_eq!(body["collapse_course_nav"], false);

    // The merged state is persisted and returned on the next read.
    let resp = server
        .get(&format!("/users/{student}/settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["manual_mark_as_read"], true);
    assert_eq!(body["collapse_global_nav"], true);

    let stored: (bool, bool, bool) = sqlx::query_as(
        "SELECT manual_mark_as_read, collapse_global_nav, default_to_block_editor \
         FROM user_settings WHERE user_id = $1",
    )
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("stored settings row");
    assert_eq!(stored, (true, true, true));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn settings_are_self_or_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Setter", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;

    // Owner and admin can read; everyone else is forbidden.
    for (actor, role) in [(student, "student"), (admin, "admin")] {
        let resp = server
            .get(&format!("/users/{student}/settings"))
            .add_header(axum::http::header::AUTHORIZATION, auth(actor, role))
            .await;
        assert_eq!(resp.status_code(), 200, "{role}: {}", resp.text());
    }
    for (actor, role) in [(other, "student"), (teacher, "teacher")] {
        let resp = server
            .get(&format!("/users/{student}/settings"))
            .add_header(axum::http::header::AUTHORIZATION, auth(actor, role))
            .await;
        assert_eq!(
            resp.status_code(),
            403,
            "{role} must not read another user's settings: {}",
            resp.text()
        );
    }

    // Writes follow the same rule; an admin can change another user's settings.
    let resp = server
        .put(&format!("/users/{student}/settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "release_notes_badge_disabled": true }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .put(&format!("/users/{student}/settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "release_notes_badge_disabled": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["release_notes_badge_disabled"], true);

    // The self alias works for writes too.
    let resp = server
        .put("/users/self/settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "collapse_course_nav": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["collapse_course_nav"], true);
    assert_eq!(body["release_notes_badge_disabled"], true);

    // Missing users are 404 for admins; unauthenticated calls are 401.
    let resp = server
        .get("/users/999999999/settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(server.get("/users/self/settings").await.status_code(), 401);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// User files
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_user_file_records_metadata_for_self_or_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Uploader", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;

    // Canvas preflight names the upload `name`; the response is the attachment
    // JSON with 200 (not 201).
    let resp = server
        .post(&format!("/users/{student}/files"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "name": "essay.pdf",
            "content_type": "application/pdf",
            "size": 1234
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["user_id"], student);
    assert_eq!(body["filename"], "essay.pdf");
    assert_eq!(body["display_name"], "essay.pdf");
    assert_eq!(body["content_type"], "application/pdf");
    assert_eq!(body["size"], 1234);
    assert_eq!(body["folder"], "unfiled");
    assert!(body["folder_id"].is_null());
    assert!(body["id"].as_i64().unwrap() > 0);
    assert!(body.get("created_at").is_some());

    let stored: (String, String, i64) =
        sqlx::query_as("SELECT filename, display_name, size FROM user_files WHERE user_id = $1")
            .bind(student)
            .fetch_one(&pool)
            .await
            .expect("stored user file");
    assert_eq!(
        stored,
        ("essay.pdf".to_string(), "essay.pdf".to_string(), 1234)
    );

    // An admin may add a file to another user's context; `filename` is an
    // accepted alias and a custom folder path is stored as-is.
    let resp = server
        .post(&format!("/users/{student}/files"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "filename": "notes.txt",
            "display_name": "Notes",
            "folder": "my files"
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["filename"], "notes.txt");
    assert_eq!(body["display_name"], "Notes");
    assert_eq!(body["folder"], "my files");
    assert_eq!(body["size"], 0);
    assert_eq!(body["content_type"], "application/octet-stream");

    // Other users and teachers cannot add files to someone else's context.
    for (actor, role) in [(other, "student"), (teacher, "teacher")] {
        let resp = server
            .post(&format!("/users/{student}/files"))
            .add_header(axum::http::header::AUTHORIZATION, auth(actor, role))
            .json(&json!({ "name": "sneaky.txt" }))
            .await;
        assert_eq!(
            resp.status_code(),
            403,
            "{role} must not create files for another user: {}",
            resp.text()
        );
    }

    // A blank filename is rejected, and missing users are 404.
    let resp = server
        .post(&format!("/users/{student}/files"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "name": "   " }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    let resp = server
        .post("/users/999999999/files")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "name": "ghost.txt" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Todo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn todo_lists_grading_and_submitting_assignments() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Grader", "teacher").await;
    let student = seed_user(&pool, &prefix, "Submitter", "student").await;
    let classmate = seed_user(&pool, &prefix, "Classmate", "student").await;

    let taught = seed_course(&pool, &prefix, "Taught").await;
    let taken = seed_course(&pool, &prefix, "Taken").await;
    enroll(&pool, teacher, taught, "teacher").await;
    enroll(&pool, classmate, taught, "student").await;
    enroll(&pool, student, taken, "student").await;

    let now = Utc::now();
    let due_soon = now + Duration::days(1);
    let due_far = now + Duration::days(21);

    // Teacher todo: only the assignment with an unscored submission.
    let needs_grading = seed_assignment(&pool, taught, "Needs Grading", Some(now)).await;
    seed_submission(&pool, needs_grading, classmate, None).await;
    let graded = seed_assignment(&pool, taught, "Already Graded", Some(now)).await;
    seed_submission(&pool, graded, classmate, Some(9.0)).await;

    // Student todo: due within a week and not submitted by the caller.
    let to_submit = seed_assignment(&pool, taken, "Submit Soon", Some(due_soon)).await;
    let far_future = seed_assignment(&pool, taken, "Far Future", Some(due_far)).await;
    let submitted = seed_assignment(&pool, taken, "Already Submitted", Some(due_soon)).await;
    seed_submission(&pool, submitted, student, None).await;

    let resp = server
        .get("/users/self/todo")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("todo must be an array");
    assert_eq!(
        items.len(),
        1,
        "teacher sees only the ungraded assignment: {body}"
    );
    let item = &items[0];
    assert_eq!(item["type"], "grading");
    assert_eq!(item["assignment"]["id"], needs_grading);
    assert_eq!(item["assignment"]["title"], "Needs Grading");
    assert_eq!(item["needs_grading_count"], 1);
    assert_eq!(item["context_type"], "Course");
    assert_eq!(item["course_id"], taught);
    assert_eq!(item["context_name"], format!("{prefix} Taught"));
    assert_eq!(item["context_short_name"], format!("{prefix}-Taught"));
    assert!(item["html_url"].as_str().unwrap().contains("speed_grader"));

    let resp = server
        .get("/users/self/todo")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("todo must be an array");
    assert_eq!(
        items.len(),
        1,
        "student sees only the upcoming unsubmitted assignment: {body}"
    );
    let item = &items[0];
    assert_eq!(item["type"], "submitting");
    assert_eq!(item["assignment"]["id"], to_submit);
    assert_eq!(item["context_type"], "Course");
    assert_eq!(item["course_id"], taken);
    assert!(item.get("needs_grading_count").is_none());
    assert!(item["html_url"].as_str().unwrap().ends_with("#submit"));

    // The assignments excluded above really were seeded.
    assert_ne!(far_future, to_submit);
    assert_ne!(submitted, to_submit);

    assert_eq!(server.get("/users/self/todo").await.status_code(), 401);

    cleanup(&pool, &prefix).await;
}
