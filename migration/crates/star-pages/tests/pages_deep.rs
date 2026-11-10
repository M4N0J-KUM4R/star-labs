//! Integration tests for the pages-deep slice: page duplication and the
//! pages_ai alt-text stub.
//!
//! Pins ACTUAL handler behavior:
//! - POST /courses/{courseId}/pages/{urlOrId}/duplicate answers 200 with the
//!   new page json (title "<source> Copy", workflow_state 'unpublished',
//!   duplicated_from_page_id pointing at the source, revision 1 stored).
//! - teachers/admins may duplicate; students get 403; unknown pages 404;
//!   anonymous callers 401.
//! - POST /courses/{courseId}/pages_ai/alt_text answers the Canvas
//!   `{ image: { altText } }` shape, 400 without attachment_id, 403 for
//!   students.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-pages

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
        "pagdeep-{}-{}",
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
    let server = TestServer::new(star_pages::router().with_state(state)).expect("build TestServer");
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

async fn seed_page(pool: &PgPool, course_id: i64, title: &str, url: &str, body: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO pages (course_id, title, body, url) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(body)
    .bind(url)
    .fetch_one(pool)
    .await
    .expect("seed page")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn duplicate_copies_page_as_unpublished() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "DupTeacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "DupCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let page = seed_page(
        &pool,
        course,
        "Original",
        &format!("{prefix}-original"),
        "<p>source</p>",
    )
    .await;
    let bearer = auth(teacher, "teacher");

    // Canvas answers 200 (not 201) with the new page json.
    let resp = server
        .post(&format!("/courses/{course}/pages/{page}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let copy: Value = resp.json();
    assert_ne!(copy["id"].as_i64(), Some(page));
    assert_eq!(copy["title"], "Original Copy");
    assert_eq!(copy["workflow_state"], "unpublished");
    assert_eq!(copy["body"], "<p>source</p>");
    assert_eq!(copy["url"], "original-copy");

    // Provenance column + version 1 for the copy.
    let copied_from: (Option<i64>,) =
        sqlx::query_as("SELECT duplicated_from_page_id FROM pages WHERE id = $1")
            .bind(copy["id"].as_i64().expect("copy id"))
            .fetch_one(&pool)
            .await
            .expect("copy row");
    assert_eq!(copied_from.0, Some(page));
    let revisions: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM page_revisions WHERE page_id = $1")
            .bind(copy["id"].as_i64().expect("copy id"))
            .fetch_one(&pool)
            .await
            .expect("copy revisions");
    assert_eq!(revisions.0, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn duplicate_authorization_and_missing_pages() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "AuthTeacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "AuthStudent", "student").await;
    let course = seed_course(&pool, &prefix, "AuthCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let page = seed_page(
        &pool,
        course,
        "Guarded",
        &format!("{prefix}-guarded"),
        "body",
    )
    .await;

    // Students cannot duplicate (Canvas :create on the page).
    let resp = server
        .post(&format!("/courses/{course}/pages/{page}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unknown page ref answers 404, cross-course refs included.
    let resp = server
        .post(&format!("/courses/{course}/pages/does-not-exist/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Anonymous callers are rejected by the AuthUser extractor.
    let resp = server
        .post(&format!("/courses/{course}/pages/{page}/duplicate"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn alt_text_stub_matches_canvas_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "AltTeacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "AltStudent", "student").await;
    let course = seed_course(&pool, &prefix, "AltCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let resp = server
        .post(&format!("/courses/{course}/pages_ai/alt_text"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "attachment_id": 4242 }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["image"]["altText"], "Image of attachment 4242");
    assert_eq!(body.as_object().map(|o| o.len()), Some(1), "{body}");

    // Blank attachment_id is a 400 (Canvas ai_alt_text_params_valid?).
    let resp = server
        .post(&format!("/courses/{course}/pages_ai/alt_text"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Students lack content-manage rights.
    let resp = server
        .post(&format!("/courses/{course}/pages_ai/alt_text"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "attachment_id": 4242 }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
