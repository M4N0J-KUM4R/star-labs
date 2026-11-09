//! Integration tests for star-pages.
//!
//! Exercises the real pages router against a real Postgres: the Canvas v1
//! nested aliases (GET/PUT/DELETE /courses/{courseId}/pages/{id}) must match
//! the flat /pages/{id} behavior, 404 when the page belongs to another
//! course, and keep the flat authorization rules.
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
        "pag-tests-{}-{}",
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
async fn alias_get_put_delete_match_flat_behavior() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "AliasCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let page = seed_page(
        &pool,
        course,
        &format!("{prefix}-Alias"),
        &format!("{prefix}-alias"),
        "v1",
    )
    .await;
    let bearer = auth(teacher, "teacher");
    let flat = format!("/pages/{page}");
    let alias = format!("/courses/{course}/pages/{page}");

    // GET alias returns exactly the flat route's payload.
    let resp = server
        .get(&flat)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let flat_body: Value = resp.json();
    let resp = server
        .get(&alias)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let alias_body: Value = resp.json();
    assert_eq!(alias_body, flat_body);
    assert_eq!(alias_body["id"].as_i64(), Some(page));

    // Canvas url_or_id: the alias also resolves the page's url slug.
    let resp = server
        .get(&format!("/courses/{course}/pages/{prefix}-alias"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["id"].as_i64(), Some(page));

    // PUT alias updates the same row with flat-route merge semantics.
    let resp = server
        .put(&alias)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "title": "Alias Renamed" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["id"].as_i64(), Some(page));
    assert_eq!(updated["title"], "Alias Renamed");
    assert_eq!(updated["body"], "v1", "unset fields must be preserved");

    // DELETE alias soft-deletes, and both routes hide the page afterwards.
    let resp = server
        .delete(&alias)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let state: (String,) = sqlx::query_as("SELECT workflow_state FROM pages WHERE id = $1")
        .bind(page)
        .fetch_one(&pool)
        .await
        .expect("page row survives soft delete");
    assert_eq!(state.0, "deleted");

    for url in [&flat, &alias] {
        let resp = server
            .get(url)
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 404, "{url}; body: {}", resp.text());
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn alias_of_page_in_another_course_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    // The caller teaches BOTH courses, so only the course scoping can 404.
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course_a = seed_course(&pool, &prefix, "CourseA").await;
    let course_b = seed_course(&pool, &prefix, "CourseB").await;
    seed_enrollment(&pool, teacher, course_b, "teacher").await;
    seed_enrollment(&pool, teacher, course_a, "teacher").await;
    let title = format!("{prefix}-InA");
    let page_a = seed_page(&pool, course_a, &title, &format!("{prefix}-in-a"), "b").await;
    let bearer = auth(teacher, "teacher");
    let wrong_door = format!("/courses/{course_b}/pages/{page_a}");

    let resp = server
        .get(&wrong_door)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.text(), "not found");

    let resp = server
        .put(&wrong_door)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "title": "hijack" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .delete(&wrong_door)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The cross-course calls changed nothing.
    let row: (String, String) =
        sqlx::query_as("SELECT title, workflow_state FROM pages WHERE id = $1")
            .bind(page_a)
            .fetch_one(&pool)
            .await
            .expect("page row");
    assert_eq!(row, (title, "published".to_string()));

    // A page that does not exist in the addressed course is 404 too.
    let resp = server
        .get(&format!("/courses/{course_a}/pages/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn alias_authz_matches_flat_routes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "AuthzCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let page = seed_page(&pool, course, &format!("{prefix}-Guarded"), "guarded", "b").await;
    let alias = format!("/courses/{course}/pages/{page}");
    let flat = format!("/pages/{page}");

    // Enrolled students may read through either route.
    let student_bearer = auth(student, "student");
    for url in [&alias, &flat] {
        let resp = server
            .get(url)
            .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 200, "{url}; body: {}", resp.text());
    }

    // ...but may not mutate through either route (teacher-only, as before).
    for url in [&alias, &flat] {
        let resp = server
            .put(url)
            .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
            .json(&json!({ "title": "hijack" }))
            .await;
        assert_eq!(resp.status_code(), 403, "{url}; body: {}", resp.text());
        assert_eq!(resp.text(), "forbidden");

        let resp = server
            .delete(url)
            .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 403, "{url}; body: {}", resp.text());
    }

    // Unenrolled users are still rejected on reads.
    let resp = server
        .get(&alias)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Anonymous callers get 401 on every alias verb.
    let resp = server.get(&alias).await;
    assert_eq!(resp.status_code(), 401);
    let resp = server.put(&alias).json(&json!({ "title": "x" })).await;
    assert_eq!(resp.status_code(), 401);
    let resp = server.delete(&alias).await;
    assert_eq!(resp.status_code(), 401);

    cleanup(&pool, &prefix).await;
}
