//! Integration tests for star-pages.
//!
//! Exercises the real pages router against a real Postgres: the Canvas v1
//! nested aliases (GET/PUT/DELETE /courses/{courseId}/pages/{id}) must match
//! the flat /pages/{id} behavior, 404 when the page belongs to another
//! course, and keep the flat authorization rules. Revision coverage:
//! GET /courses/{courseId}/pages/{url}/revisions[/latest|/{revisionId}],
//! revisions appended on create/update, course scoping, and the legacy
//! no-history fallback for `latest`.
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

// ---------------------------------------------------------------------------
// Page revisions (Canvas wiki_pages_api_controller#revisions / #show_revision)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revisions_are_recorded_on_create_and_update() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "RevisionCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let teacher_bearer = auth(teacher, "teacher");

    // POST creates revision 1, PUTs append revisions 2 and 3.
    let resp = server
        .post(&format!("/courses/{course}/pages"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "title": "History Page", "body": "v1" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let page: Value = resp.json();
    let page_id = page["id"].as_i64().expect("page id");
    let slug = page["url"].as_str().expect("page url").to_string();

    for body in ["v2", "v3"] {
        let resp = server
            .put(&format!("/pages/{page_id}"))
            .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
            .json(&json!({ "body": body }))
            .await;
        assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    }

    // History is newest first, summaries only, latest flagged.
    let history_url = format!("/courses/{course}/pages/{slug}/revisions");
    let resp = server
        .get(&history_url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let history: Value = resp.json();
    let items = history.as_array().expect("history array");
    assert_eq!(items.len(), 3, "create + two updates; body: {history}");
    assert_eq!(items[0]["revision_id"], 3);
    assert_eq!(items[1]["revision_id"], 2);
    assert_eq!(items[2]["revision_id"], 1);
    assert_eq!(items[0]["latest"], true);
    assert_eq!(items[1]["latest"], false);
    assert_eq!(items[2]["latest"], false);
    assert!(items[0].get("body").is_none(), "history omits content");
    assert_eq!(items[0]["edited_by"]["id"], teacher);

    // One revision by its per-page number; summary=true drops content.
    let revision_url = format!("/courses/{course}/pages/{slug}/revisions/1");
    let resp = server
        .get(&revision_url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let first: Value = resp.json();
    assert_eq!(first["revision_id"], 1);
    assert_eq!(first["body"], "v1");
    assert_eq!(first["title"], "History Page");
    assert_eq!(first["latest"], false);

    let resp = server
        .get(&format!("{revision_url}?summary=true"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let summary: Value = resp.json();
    assert!(summary.get("title").is_none());
    assert!(summary.get("body").is_none());

    let resp = server
        .get(&format!("/courses/{course}/pages/{slug}/revisions/99"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Students may read the latest revision (Canvas :read) but not history.
    let resp = server
        .get(&format!("/courses/{course}/pages/{slug}/revisions/latest"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let latest: Value = resp.json();
    assert_eq!(latest["revision_id"], 3);
    assert_eq!(latest["latest"], true);
    assert_eq!(latest["body"], "v3");

    let resp = server
        .get(&history_url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn revision_routes_are_course_scoped() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    // The caller teaches BOTH courses, so only course scoping can 404.
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course_a = seed_course(&pool, &prefix, "RevA").await;
    let course_b = seed_course(&pool, &prefix, "RevB").await;
    seed_enrollment(&pool, teacher, course_a, "teacher").await;
    seed_enrollment(&pool, teacher, course_b, "teacher").await;
    let slug = format!("{prefix}-scoped");
    seed_page(&pool, course_a, &format!("{prefix}-Scoped"), &slug, "b").await;
    let bearer = auth(teacher, "teacher");

    for url in [
        format!("/courses/{course_b}/pages/{slug}/revisions"),
        format!("/courses/{course_b}/pages/{slug}/revisions/latest"),
        format!("/courses/{course_b}/pages/{slug}/revisions/1"),
    ] {
        let resp = server
            .get(&url)
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 404, "{url}; body: {}", resp.text());
        assert_eq!(resp.text(), "not found");
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn latest_revision_falls_back_to_page_state() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    // Pages seeded outside the API have no stored history.
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "LegacyCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let slug = format!("{prefix}-legacy");
    seed_page(
        &pool,
        course,
        &format!("{prefix}-Legacy"),
        &slug,
        "legacy body",
    )
    .await;

    let resp = server
        .get(&format!("/courses/{course}/pages/{slug}/revisions/latest"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let latest: Value = resp.json();
    assert_eq!(latest["revision_id"], 0);
    assert_eq!(latest["latest"], true);
    assert_eq!(latest["body"], "legacy body");

    let resp = server
        .get(&format!(
            "/courses/{course}/pages/{slug}/revisions/latest?summary=true"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let summary: Value = resp.json();
    assert!(summary.get("body").is_none());
    assert!(summary.get("title").is_none());

    // History is empty for legacy pages.
    let resp = server
        .get(&format!("/courses/{course}/pages/{slug}/revisions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!([]));

    cleanup(&pool, &prefix).await;
}
