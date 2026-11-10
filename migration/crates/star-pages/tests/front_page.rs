//! Front page API tests for star-pages (Canvas wiki_pages_api_controller
//! `show_front_page` / `update_front_page` parity).
//!
//! Coverage: the `{"message":"No front page has been set"}` 404 when no page is
//! designated (even when a page uses the default `front-page` url), creation and
//! update through PUT /courses/{courseId}/front_page, slug regeneration,
//! simplicity of the front-page marker (one live marker per course), revision
//! appends, and the "cannot be unpublished" 400.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-pages

use axum::http::header::AUTHORIZATION;
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
        "fpg-tests-{}-{}",
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
async fn missing_front_page_returns_canvas_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "NoFrontPage").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    // Canvas does not treat a page at the default `front-page` url as the front
    // page unless the wiki designates it.
    seed_page(&pool, course, "Front Page", "front-page", "implicit").await;

    let url = format!("/courses/{course}/front_page");
    for bearer in [auth(teacher, "teacher"), auth(student, "student")] {
        let resp = server.get(&url).add_header(AUTHORIZATION, bearer).await;
        assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
        let body: Value = resp.json();
        assert_eq!(body["message"], "No front page has been set");
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn put_creates_front_page_page_and_revision() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "CreateFront").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");
    let url = format!("/courses/{course}/front_page");

    let resp = server
        .put(&url)
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({ "title": "Welcome Aboard", "body": "<p>hi</p>" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    let page_id = created["id"].as_i64().expect("page id");
    assert_eq!(created["page_id"].as_i64(), Some(page_id));
    assert_eq!(created["front_page"], true);
    assert_eq!(created["published"], true);
    assert_eq!(created["hide_from_students"], false);
    assert_eq!(created["title"], "Welcome Aboard");
    assert_eq!(created["url"], "welcome-aboard");
    assert_eq!(created["body"], "<p>hi</p>");
    assert_eq!(created["editing_roles"], "teachers");
    assert_eq!(
        created["html_url"],
        format!("/courses/{course}/pages/welcome-aboard")
    );

    // simply_versioned: the creating save is revision 1.
    let revisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM page_revisions WHERE page_id = $1")
            .bind(page_id)
            .fetch_one(&pool)
            .await
            .expect("revision count");
    assert_eq!(revisions, 1);

    // GET returns the same page and the pages list exposes it too.
    let resp = server
        .get(&url)
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let fetched: Value = resp.json();
    assert_eq!(fetched["id"].as_i64(), Some(page_id));
    assert_eq!(fetched["body"], "<p>hi</p>");

    let resp = server
        .get(&format!("/courses/{course}/pages"))
        .add_header(AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    assert!(
        listed
            .as_array()
            .expect("page list")
            .iter()
            .any(|page| page["id"].as_i64() == Some(page_id)),
        "created front page must be listed: {listed:?}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn put_updates_the_single_front_page_marker() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UpdateFront").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");
    let url = format!("/courses/{course}/front_page");

    let resp = server
        .put(&url)
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({ "title": "Draft One", "body": "a" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let page_id = resp.json::<Value>()["id"].as_i64().expect("page id");

    // A title change regenerates the slug and appends a revision.
    let resp = server
        .put(&url)
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({ "title": "Renamed Page", "body": "b" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["id"].as_i64(), Some(page_id));
    assert_eq!(updated["url"], "renamed-page");
    assert_eq!(updated["body"], "b");
    assert_eq!(updated["front_page"], true);

    let revisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM page_revisions WHERE page_id = $1")
            .bind(page_id)
            .fetch_one(&pool)
            .await
            .expect("revision count");
    assert_eq!(revisions, 2);

    // Still exactly one page, and it is the single front page marker.
    let pages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pages WHERE course_id = $1")
        .bind(course)
        .fetch_one(&pool)
        .await
        .expect("page count");
    assert_eq!(pages, 1);
    let markers: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pages WHERE course_id = $1 AND front_page")
            .bind(course)
            .fetch_one(&pool)
            .await
            .expect("front page count");
    assert_eq!(markers, 1);

    // GET returns the updated page.
    let resp = server.get(&url).add_header(AUTHORIZATION, bearer).await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["body"], "b");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn front_page_cannot_be_unpublished() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UnpublishFront").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");
    let url = format!("/courses/{course}/front_page");

    let resp = server
        .put(&url)
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({ "title": "Keep Published" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let page_id = resp.json::<Value>()["id"].as_i64().expect("page id");

    // Canvas `cannot_have_unpublished_front_page` -> 400 for both spellings.
    for payload in [
        json!({ "published": false }),
        json!({ "workflow_state": "unpublished" }),
    ] {
        let resp = server
            .put(&url)
            .add_header(AUTHORIZATION, bearer.clone())
            .json(&payload)
            .await;
        assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    }

    let state: (String, bool) =
        sqlx::query_as("SELECT workflow_state, front_page FROM pages WHERE id = $1")
            .bind(page_id)
            .fetch_one(&pool)
            .await
            .expect("page row");
    assert_eq!(state, ("published".to_string(), true));

    cleanup(&pool, &prefix).await;
}
