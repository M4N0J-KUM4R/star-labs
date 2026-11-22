//! Integration tests for the PORT-B files gaps:
//! - POST /files/capture                      (files#api_capture)
//! - POST /files/update_word_count            (files#update_word_count)
//! - POST /rce_linked_file_instfs_ids         (files#rce_linked_file_instfs_ids)
//! - PUT  /groups/{groupId}/usage_rights      (set_usage_rights, Group ctx)
//! - PUT  /users/{userId}/usage_rights        (set_usage_rights, User ctx)
//! - GET  /groups/{groupId}/content_licenses  (licenses, Group ctx)
//! - GET  /users/{userId}/content_licenses    (licenses, User ctx)
//!
//! Tables: files (0005 + word_count in 0255), user_files (0056 + usage_rights
//! id in 0255), group_files (0064 + usage_rights_id in 0255), usage_rights
//! (0084), folders (0036), groups (0018).
//!
//! Conventions (mirrors files_integration.rs): skipped cleanly when
//! DATABASE_URL is not set; unique per-run prefix + full teardown.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

/// An id far beyond any seeded row, for 404 probes.
const MISSING_ID: i64 = 999_999_999;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "filgaps-{}-{}",
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
    let server = TestServer::new(star_files::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
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

async fn seed_group(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

/// Seed a course-scoped files row (update_word_count target).
async fn seed_course_file(pool: &PgPool, course_id: i64, uploader_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO files (course_id, uploader_id, filename, display_name,
                           content_type, size, folder)
        VALUES ($1, $2, $3, $3, 'text/plain', 10, 'unfiled')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(uploader_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed course file")
}

/// Seed a metadata-only user_files row (capture / user usage rights target).
async fn seed_user_file(pool: &PgPool, user_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO user_files (user_id, filename, display_name, content_type, size, folder)
        VALUES ($1, $2, $2, 'text/plain', 10, 'unfiled')
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed user file")
}

/// Seed a metadata-only group_files row (group usage rights target).
async fn seed_group_file(pool: &PgPool, group_id: i64, uploader_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO group_files (group_id, uploader_id, folder, filename, display_name)
        VALUES ($1, $2, 'group files', $3, $3)
        RETURNING id
        "#,
    )
    .bind(group_id)
    .bind(uploader_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed group file")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Delete everything this run created. usage_rights rows key on
/// (context_type, context_id) with no FK, so they go first.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM usage_rights WHERE context_type = 'Group' \
         AND context_id IN (SELECT g.id FROM groups g \
                            JOIN courses c ON c.id = g.course_id \
                            WHERE c.course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM usage_rights WHERE context_type = 'User' \
         AND context_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM usage_rights WHERE context_type = 'Course' \
         AND context_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
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
}

// ---------------------------------------------------------------------------
// POST /files/capture
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_capture_records_user_attachment_metadata() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "CapStudent", "student").await;

    // Anonymous -> 401.
    let resp = server.post("/files/capture").await;
    assert_eq!(resp.status_code(), 401, "anonymous capture");

    // No file parameter -> 400.
    let resp = server
        .post("/files/capture")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert!(resp.text().contains("file parameter"));

    // Blank filename -> 400.
    let resp = server
        .post("/files/capture")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "file": { "name": "   " } }))
        .await;
    assert_eq!(resp.status_code(), 400, "blank filename");

    // Documented `file` shape.
    let resp = server
        .post("/files/capture")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "file": {
                "name": "capture.webm",
                "content_type": "video/webm",
                "size": 2048
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let captured: Value = resp.json();
    let file_id = captured["id"].as_i64().expect("captured id");
    assert_eq!(captured["user_id"], student);
    assert_eq!(captured["filename"], "capture.webm");
    assert_eq!(captured["display_name"], "capture.webm");
    assert_eq!(captured["content_type"], "video/webm");
    assert_eq!(captured["size"], 2048);
    assert_eq!(captured["folder"], "unfiled");

    // Legacy `attachment` alias with an explicit display_name.
    let resp = server
        .post("/files/capture")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "attachment": {
                "filename": "notes.txt",
                "display_name": "Lecture notes"
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let aliased: Value = resp.json();
    assert_eq!(aliased["filename"], "notes.txt");
    assert_eq!(aliased["display_name"], "Lecture notes");
    assert_eq!(aliased["content_type"], "application/octet-stream");

    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM user_files WHERE user_id = $1 AND id = ANY($2)")
            .bind(student)
            .bind(vec![file_id, aliased["id"].as_i64().expect("aliased id")])
            .fetch_one(&pool)
            .await
            .expect("count captured rows");
    assert_eq!(rows, 2, "both captures stored in user_files");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /files/update_word_count
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_word_count_sets_the_column_and_gates_by_owner() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "WordCountCourse").await;
    let teacher = seed_user(&pool, &prefix, "WcTeacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "WcStudent", "student").await;
    let stranger = seed_user(&pool, &prefix, "WcStranger", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let teacher_file = seed_course_file(&pool, course, teacher, "essay.txt").await;
    let student_file = seed_course_file(&pool, course, student, "draft.txt").await;

    // Missing params -> 400.
    let resp = server
        .post("/files/update_word_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "file_id": teacher_file }))
        .await;
    assert_eq!(resp.status_code(), 400, "missing word_count");

    // A stranger can neither see nor touch another user's file.
    let resp = server
        .post("/files/update_word_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .json(&json!({ "file_id": student_file, "word_count": 5 }))
        .await;
    assert_eq!(resp.status_code(), 403, "stranger denied");

    // Unknown file -> 404.
    let resp = server
        .post("/files/update_word_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "file_id": MISSING_ID, "word_count": 5 }))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown file");

    // Teacher (course manager) -> 204 and the column is set.
    let resp = server
        .post("/files/update_word_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "file_id": teacher_file, "word_count": 4242 }))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let count: i32 = sqlx::query_scalar("SELECT word_count FROM files WHERE id = $1")
        .bind(teacher_file)
        .fetch_one(&pool)
        .await
        .expect("read word_count");
    assert_eq!(count, 4242);

    // The owner may update their own file too (bare `id` alias).
    let resp = server
        .post("/files/update_word_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "id": student_file, "word_count": 17 }))
        .await;
    assert_eq!(resp.status_code(), 204, "owner may count words");
    let count: i32 = sqlx::query_scalar("SELECT word_count FROM files WHERE id = $1")
        .bind(student_file)
        .fetch_one(&pool)
        .await
        .expect("read word_count");
    assert_eq!(count, 17);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /rce_linked_file_instfs_ids
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rce_linked_file_instfs_ids_answers_the_feature_off_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = pool; // silence unused when the test body only hits the server

    let user = seed_user(&pool, &prefix, "RceUser", "student").await;

    let resp = server
        .post("/rce_linked_file_instfs_ids")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "file_ids": [1, 2, 3] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({ "ids": [] }), "no InstFS linkage exists");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /groups/{groupId}/usage_rights + /users/{userId}/usage_rights
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_usage_rights_for_group_and_user_contexts() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "UrCourse").await;
    let teacher = seed_user(&pool, &prefix, "UrTeacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "UrStudent", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, course, "usage rights group").await;
    let group_file = seed_group_file(&pool, group, teacher, "group-doc.pdf").await;
    let user_file = seed_user_file(&pool, student, "my-doc.pdf").await;

    // --- Group context ----------------------------------------------------
    // Unknown group -> 404.
    let resp = server
        .put(&format!("/groups/{MISSING_ID}/usage_rights"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "file_ids": [group_file],
            "usage_rights": { "use_justification": "own_copyright" }
        }))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown group");

    // A plain member cannot manage the group's usage rights.
    let resp = server
        .put(&format!("/groups/{group}/usage_rights"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "file_ids": [group_file],
            "usage_rights": { "use_justification": "own_copyright" }
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "member denied");

    // Missing file_ids -> Canvas' 400 error shape.
    let resp = server
        .put(&format!("/groups/{group}/usage_rights"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "usage_rights": { "use_justification": "own_copyright" }
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Invalid justification -> 400 Canvas error shape.
    let resp = server
        .put(&format!("/groups/{group}/usage_rights"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "file_ids": [group_file],
            "usage_rights": { "use_justification": "made_up" }
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "invalid justification");

    // Unknown file id -> Canvas' 404 error shape.
    let resp = server
        .put(&format!("/groups/{group}/usage_rights"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "file_ids": [group_file, MISSING_ID],
            "usage_rights": { "use_justification": "own_copyright" }
        }))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown group file");

    // Teacher of the parent course sets the group's usage rights.
    let resp = server
        .put(&format!("/groups/{group}/usage_rights"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "file_ids": [group_file],
            "usage_rights": {
                "use_justification": "used_by_permission",
                "legal_copyright": "(c) test",
                "license": "cc_by"
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["use_justification"], "used_by_permission");
    assert_eq!(body["legal_copyright"], "(c) test");
    assert_eq!(body["license"], "cc_by");
    assert_eq!(body["license_name"], "CC Attribution");
    assert_eq!(body["message"], "1 file updated");
    assert_eq!(body["file_ids"], json!([group_file]));

    let linked: Option<i64> =
        sqlx::query_scalar("SELECT usage_rights_id FROM group_files WHERE id = $1")
            .bind(group_file)
            .fetch_one(&pool)
            .await
            .expect("read group file link");
    assert!(linked.is_some(), "group file links the usage_rights row");

    // --- User context -----------------------------------------------------
    // Only the user themself (or an admin).
    let resp = server
        .put(&format!("/users/{student}/usage_rights"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "file_ids": [user_file],
            "usage_rights": { "use_justification": "own_copyright" }
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "others denied");

    let resp = server
        .put(&format!("/users/{MISSING_ID}/usage_rights"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "file_ids": [user_file],
            "usage_rights": { "use_justification": "own_copyright" }
        }))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown user");

    let resp = server
        .put(&format!("/users/{student}/usage_rights"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "file_ids": [user_file],
            "usage_rights": { "use_justification": "fair_use" }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["use_justification"], "fair_use");
    assert_eq!(body["file_ids"], json!([user_file]));

    let linked: Option<i64> =
        sqlx::query_scalar("SELECT usage_rights_id FROM user_files WHERE id = $1")
            .bind(user_file)
            .fetch_one(&pool)
            .await
            .expect("read user file link");
    assert!(linked.is_some(), "user file links the usage_rights row");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /groups/{groupId}/content_licenses + /users/{userId}/content_licenses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn content_licenses_for_group_and_user_contexts() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "LicCourse").await;
    let teacher = seed_user(&pool, &prefix, "LicTeacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "LicStudent", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, course, "licenses group").await;

    // Unknown contexts -> 404.
    let resp = server
        .get(&format!("/groups/{MISSING_ID}/content_licenses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown group");

    let resp = server
        .get(&format!("/users/{MISSING_ID}/content_licenses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown user");

    // Group licenses: the parent course's teacher may read them.
    let resp = server
        .get(&format!("/groups/{group}/content_licenses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let licenses: Vec<Value> = resp.json();
    assert_eq!(licenses.len(), 8, "UsageRights.licenses has 8 entries");
    assert!(licenses.iter().all(|entry| entry["id"].is_string()));

    // User licenses: self only (or admin).
    let resp = server
        .get(&format!("/users/{student}/content_licenses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "others denied");

    let resp = server
        .get(&format!("/users/{student}/content_licenses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let licenses: Vec<Value> = resp.json();
    assert_eq!(licenses.len(), 8, "UsageRights.licenses has 8 entries");

    cleanup(&pool, &prefix).await;
}
