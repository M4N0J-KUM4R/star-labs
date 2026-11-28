//! Integration tests for the file-bytes slice: upload, download and the
//! attachment JWT.
//!
//! Pins ACTUAL handler behavior:
//! - POST /courses/{courseId}/files with a multipart body stores the bytes
//!   under the key Canvas derives from the attachment id
//!   (`0000/<id>/<filename>`) and records the digest, the size and the state.
//! - GET /files/{id}/download serves the bytes with the content type, the
//!   length and the disposition Canvas sends.
//! - The attachment JWT stands in for the API token: a request that carries
//!   only the JWT is served the file.
//! - POST /files/update_word_count accepts the attachment JWT and records the
//!   word count.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-files

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "bytesint-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// The directory the store writes into. The store is a process-wide
/// singleton, so every test in this binary uses the same one.
fn storage_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("star-files-bytes-{}", std::process::id()))
}

async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    // The store reads its root out of the environment when it is first used.
    std::env::set_var("STAR_STORAGE_DIR", storage_dir());

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

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// A multipart body with one file part.
fn multipart_body(boundary: &str, filename: &str, content_type: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// The key Canvas derives from an attachment id
/// (`("%08d" % attachment_path_id).scan(/..../)`,
/// `gems/attachment_fu/lib/attachment_fu/backends/file_system_backend.rb:57`).
fn canvas_key(attachment_id: i64, filename: &str) -> String {
    let id = format!("{attachment_id:08}");
    let chunks: Vec<&str> = id
        .as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or("0000"))
        .collect();
    format!("{}/{filename}", chunks.join("/"))
}

async fn cleanup(pool: &PgPool, prefix: &str, files: &[(i64, &str)]) {
    let _ = sqlx::query(
        "DELETE FROM files WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM usage_rights WHERE context_type = 'Course' \
         AND context_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM folders WHERE context_type = 'Course' \
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
    // Only this test's objects: the store is shared by the whole binary.
    for (file, filename) in files {
        let _ = std::fs::remove_file(storage_dir().join(canvas_key(*file, filename)));
    }
}

#[tokio::test]
async fn uploaded_bytes_come_back_on_download() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "BytesTeacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "BytesCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let contents = b"hello canvas";
    let body = multipart_body("starboundary", "report.txt", "text/plain", contents);

    let resp = server
        .post(&format!("/courses/{course}/files"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("multipart/form-data; boundary=starboundary")
        .bytes(axum::body::Bytes::from(body))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let json: Value = resp.json();
    let file = json["id"].as_i64().expect("the upload answers an id");
    assert_eq!(json["filename"], "report.txt");

    // The row records where the bytes are and what they are.
    let row: (Option<String>, Option<String>, i64, String, String) = sqlx::query_as(
        "SELECT storage_key, md5, size, content_type, file_state FROM files WHERE id = $1",
    )
    .bind(file)
    .fetch_one(&pool)
    .await
    .expect("read the file row");

    // Canvas keys the bytes by the attachment id
    // (`gems/attachment_fu/lib/attachment_fu/backends/file_system_backend.rb:38-58`).
    assert_eq!(
        row.0.as_deref(),
        Some(canvas_key(file, "report.txt").as_str())
    );
    // The digest is the MD5 of the bytes, like Canvas' `attachments.md5`.
    assert_eq!(
        row.1.as_deref(),
        Some(star_storage::md5_hex(contents).as_str())
    );
    assert_eq!(row.2, contents.len() as i64);
    assert_eq!(row.3, "text/plain");
    assert_eq!(row.4, "available");

    // The download serves the bytes with the headers Canvas sends.
    let resp = server
        .get(&format!("/files/{file}/download"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/plain; charset=UTF-8")
    );
    assert_eq!(
        resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()),
        Some(contents.len().to_string().as_str())
    );
    let disposition = resp
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        disposition.contains("filename=\"report.txt\""),
        "{disposition}"
    );
    assert_eq!(resp.text(), String::from_utf8_lossy(contents));

    cleanup(&pool, &prefix, &[(file, "report.txt")]).await;
}

#[tokio::test]
async fn the_attachment_jwt_stands_in_for_the_api_token() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "JwtTeacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "JwtCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let contents = b"jwt bytes";
    let body = multipart_body("starboundary", "notes.txt", "text/plain", contents);
    let resp = server
        .post(&format!("/courses/{course}/files"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .content_type("multipart/form-data; boundary=starboundary")
        .bytes(axum::body::Bytes::from(body))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let file = resp.json::<Value>()["id"].as_i64().expect("an id");

    // The token is minted for the file.
    let resp = server
        .post(&format!("/files/{file}/attachment_jwt"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let token = resp.json::<Value>()["attachment_jwt"]
        .as_str()
        .expect("a token")
        .to_string();

    // A request that carries only the token is served the file.
    let resp = server
        .get(&format!("/files/{file}/download"))
        .add_query_param("attachment_jwt", token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.text(), String::from_utf8_lossy(contents));

    // The token also carries the word count back to Canvas.
    let resp = server
        .post("/files/update_word_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "attachment_jwt": token, "word_count": 42 }))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    let word_count: Option<i32> = sqlx::query_scalar("SELECT word_count FROM files WHERE id = $1")
        .bind(file)
        .fetch_one(&pool)
        .await
        .expect("read the word count");
    assert_eq!(word_count, Some(42));

    cleanup(&pool, &prefix, &[(file, "notes.txt")]).await;
}
