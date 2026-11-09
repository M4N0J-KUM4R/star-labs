//! Integration tests for star-content.
//!
//! star-content is a thin aggregator: it merges the pages, announcements and
//! modules routers into one `Router<AppState>` for inclusion in star-http.
//! These tests build that merged router over a real Postgres (migrations run,
//! state seeded directly in the DB) and smoke-test the merge:
//! - one route from each sub-crate responds through the merged router,
//! - every tested route rejects anonymous callers with 401.
//!
//! This is deliberately a routing/merge smoke suite — deep per-endpoint
//! behavior is covered by the sub-crates' own parity/integration tests.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
//!   cargo test -p star-content
//! - Uses a unique email prefix per run + full teardown so it is safe to run
//!   repeatedly against a shared/dev database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test AppState and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "content-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the merged content
/// router exactly the way star-http assembles it. Returns None when
/// DATABASE_URL is unset so callers can skip cleanly.
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

    let server =
        TestServer::new(star_content::router().with_state(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Create a user directly in the DB with the given role. Returns user id.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Create a course directly in the DB. Returns course id.
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

/// Enroll a user in a course (pages/announcements handlers authorize on the
/// enrollments table, not on the users.role column).
async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(course_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed enrollment");
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every course/user row created by this run. Content rows
/// (pages/announcements/modules) cascade from their course.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Pages routes through the merged router (star_pages::router())
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pages_route_responds_through_merged_router() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PagesCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // POST /courses/{courseId}/pages — create (teacher or admin).
    let resp = server
        .post(&format!("/courses/{course}/pages"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "title": "Merged Router Page", "body": "hello" }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "POST /courses/{{id}}/pages through merged router; body: {}",
        resp.text()
    );
    let page: Value = resp.json();
    assert_eq!(page["title"], "Merged Router Page");

    // GET /courses/{courseId}/pages — list (enrolled or admin).
    let resp = server
        .get(&format!("/courses/{course}/pages"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "GET /courses/{{id}}/pages through merged router; body: {}",
        resp.text()
    );
    let list: Value = resp.json();
    let found = list
        .as_array()
        .expect("page list is a JSON array")
        .iter()
        .any(|p| p["id"] == page["id"]);
    assert!(found, "created page must appear in the list: {list}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Announcements routes through the merged router (star_announcements::router())
// ---------------------------------------------------------------------------

#[tokio::test]
async fn announcements_route_responds_through_merged_router() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "AnnouncementsCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // POST /courses/{courseId}/announcements — create (teacher or admin).
    let resp = server
        .post(&format!("/courses/{course}/announcements"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "title": "Merged Router Announcement",
            "message": "hello from the merged router"
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "POST /courses/{{id}}/announcements through merged router; body: {}",
        resp.text()
    );
    let announcement: Value = resp.json();
    assert_eq!(announcement["title"], "Merged Router Announcement");

    // GET /announcements/{id} — fetch one through the merged router.
    let id = announcement["id"].as_i64().expect("announcement id");
    let resp = server
        .get(&format!("/announcements/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "GET /announcements/{{id}} through merged router; body: {}",
        resp.text()
    );
    let detail: Value = resp.json();
    assert_eq!(detail["id"], announcement["id"]);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Modules routes through the merged router (star_modules::router())
// ---------------------------------------------------------------------------

#[tokio::test]
async fn modules_route_responds_through_merged_router() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "ModulesCourse").await;

    // POST /courses/{courseId}/modules — create (teacher or admin).
    let resp = server
        .post(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Merged Router Module", "position": 1 }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "POST /courses/{{id}}/modules through merged router; body: {}",
        resp.text()
    );
    let module: Value = resp.json();
    assert_eq!(module["name"], "Merged Router Module");

    // GET /modules/{id} — fetch one (with items) through the merged router.
    let id = module["id"].as_i64().expect("module id");
    let resp = server
        .get(&format!("/modules/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "GET /modules/{{id}} through merged router; body: {}",
        resp.text()
    );
    let detail: Value = resp.json();
    assert_eq!(detail["id"], module["id"]);
    assert_eq!(detail["items"].as_array().map(Vec::len), Some(0));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// 401 without a token on each merged route
// ---------------------------------------------------------------------------

#[tokio::test]
async fn all_merged_routes_reject_missing_token_with_401() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "AuthCourse").await;

    // Pages: list (GET) and create (POST).
    let resp = server.get(&format!("/courses/{course}/pages")).await;
    assert_eq!(
        resp.status_code(),
        401,
        "anonymous GET pages must be 401, got {}: {}",
        resp.status_code(),
        resp.text()
    );
    let resp = server
        .post(&format!("/courses/{course}/pages"))
        .json(&json!({ "title": "Nope" }))
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "anonymous POST pages must be 401, got {}",
        resp.status_code()
    );

    // Announcements: list (GET) and create (POST).
    let resp = server
        .get(&format!("/courses/{course}/announcements"))
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "anonymous GET announcements must be 401, got {}",
        resp.status_code()
    );
    let resp = server
        .post(&format!("/courses/{course}/announcements"))
        .json(&json!({ "title": "Nope", "message": "Nope" }))
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "anonymous POST announcements must be 401, got {}",
        resp.status_code()
    );

    // Modules: list (GET) and create (POST). A huge course id is fine —
    // the AuthUser extractor must reject before any handler/DB work.
    let resp = server.get("/courses/999999/modules").await;
    assert_eq!(
        resp.status_code(),
        401,
        "anonymous GET modules must be 401, got {}",
        resp.status_code()
    );
    let resp = server
        .post("/courses/999999/modules")
        .json(&json!({ "name": "Nope" }))
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "anonymous POST modules must be 401, got {}",
        resp.status_code()
    );

    cleanup(&pool, &prefix).await;
}
