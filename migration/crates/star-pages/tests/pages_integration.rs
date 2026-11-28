//! Integration tests for star-pages.
//!
//! Exercises the real pages router against a real Postgres: the Canvas v1
//! nested routes (GET/PUT/DELETE /courses/{courseId}/pages/{id}) return the
//! page, 404 when the page belongs to another course, and enforce the
//! authorization rules. Revision coverage:
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
async fn nested_page_routes_get_put_delete() {
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
    // Canvas serves pages only under a course context (routes.rb:2305-2307,
    // 2317-2321); the bare `/pages/{id}` twin was dropped as extra surface.
    let alias = format!("/courses/{course}/pages/{page}");

    // GET the nested route returns the page.
    let resp = server
        .get(&alias)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let alias_body: Value = resp.json();
    assert_eq!(alias_body["id"].as_i64(), Some(page));

    // Canvas url_or_id: the route also resolves the page's url slug.
    let resp = server
        .get(&format!("/courses/{course}/pages/{prefix}-alias"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["id"].as_i64(), Some(page));

    // PUT updates the row with merge semantics.
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

    // DELETE soft-deletes and the route hides the page afterwards.
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

    let resp = server
        .get(&alias)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

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
    assert_eq!(
        resp.text(),
        r#"{"errors":[{"message":"The specified resource does not exist."}]}"#
    );

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

    // Enrolled students may read through the course-nested route.
    let student_bearer = auth(student, "student");
    let resp = server
        .get(&alias)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "{alias}; body: {}", resp.text());

    // ...but may not mutate through it (teacher-only, as before).
    let resp = server
        .put(&alias)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "title": "hijack" }))
        .await;
    assert_eq!(resp.status_code(), 403, "{alias}; body: {}", resp.text());
    assert_eq!(
        resp.text(),
        r#"{"status":"unauthorized","errors":[{"message":"user not authorized to perform that action"}]}"#
    );

    let resp = server
        .delete(&alias)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "{alias}; body: {}", resp.text());

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
            .put(&format!("/courses/{course}/pages/{page_id}"))
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
        assert_eq!(
            resp.text(),
            r#"{"errors":[{"message":"The specified resource does not exist."}]}"#
        );
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

// ---------------------------------------------------------------------------
// Title availability, group alt text, accessibility scan tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn title_availability_no_conflict_for_unique_title() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "TitleAvail").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let resp = server
        .get(&format!(
            "/courses/{course}/page_title_availability?title=UniquePage{}",
            chrono::Utc::now().timestamp()
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["conflict"], false, "unique title has no conflict");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn title_availability_conflict_when_page_exists() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "TitleConflict").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let existing = seed_page(&pool, course, "Existing Title", "existing-title", "body").await;
    let bearer = auth(teacher, "teacher");

    let resp = server
        .get(&format!(
            "/courses/{course}/page_title_availability?title=Existing%20Title"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["conflict"], true, "existing title should conflict");

    // current_page_id excludes itself from conflict check.
    let resp = server
        .get(&format!(
            "/courses/{course}/page_title_availability?title=Existing%20Title&current_page_id={existing}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["conflict"], false,
        "excluding self should show no conflict"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn title_availability_requires_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "TitleNoAuth").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .get(&format!(
            "/courses/{course}/page_title_availability?title=Foo"
        ))
        .await;
    assert!(
        resp.status_code() == 401 || resp.status_code() == 403,
        "expected 401/403, got {}: {}",
        resp.status_code(),
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_title_availability_always_no_conflict() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "GroupTitleCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group: i64 =
        sqlx::query_scalar("INSERT INTO groups (name, course_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix}-Group"))
            .bind(course)
            .fetch_one(&pool)
            .await
            .expect("seed group");
    let bearer = auth(teacher, "teacher");

    let resp = server
        .get(&format!(
            "/groups/{group}/page_title_availability?title=AnyTitle{}",
            chrono::Utc::now().timestamp()
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["conflict"], false,
        "group titles never conflict in this schema"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_alt_text_happy_path() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "GroupAltTextCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group: i64 =
        sqlx::query_scalar("INSERT INTO groups (name, course_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix}-Group"))
            .bind(course)
            .fetch_one(&pool)
            .await
            .expect("seed group");
    let bearer = auth(teacher, "teacher");

    let resp = server
        .post(&format!("/groups/{group}/pages_ai/alt_text"))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .json(&json!({ "attachment_id": 999 }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["image"]["altText"], "Image of attachment 999");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_alt_text_requires_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "GroupAltTextNoAuth").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group: i64 =
        sqlx::query_scalar("INSERT INTO groups (name, course_id) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix}-Group"))
            .bind(course)
            .fetch_one(&pool)
            .await
            .expect("seed group");

    let resp = server
        .post(&format!("/groups/{group}/pages_ai/alt_text"))
        .json(&json!({ "attachment_id": 999 }))
        .await;
    assert!(
        resp.status_code() == 401 || resp.status_code() == 403,
        "expected 401/403, got {}: {}",
        resp.status_code(),
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn accessibility_scan_happy_path() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "A11yCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let page = seed_page(&pool, course, "A11y Page", "a11y-page", "body").await;
    let bearer = auth(teacher, "teacher");

    let resp = server
        .post(&format!(
            "/courses/{course}/pages/{page}/accessibility/scan"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["resource_type"], "WikiPage");
    assert_eq!(body["workflow_state"], "completed");
    assert_eq!(body["issue_count"], 0);
    assert!(body["issues"].as_array().unwrap().is_empty());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn accessibility_scan_by_url_slug() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "A11ySlugCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_page(&pool, course, "Slug Page", "slug-page", "body").await;
    let bearer = auth(teacher, "teacher");

    let resp = server
        .post(&format!(
            "/courses/{course}/pages/slug-page/accessibility/scan"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["resource_type"], "WikiPage");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn accessibility_scan_requires_teacher() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "A11yStudentCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let page = seed_page(&pool, course, "Student Page", "student-page", "body").await;
    let student_bearer = auth(student, "student");

    let resp = server
        .post(&format!(
            "/courses/{course}/pages/{page}/accessibility/scan"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer)
        .await;
    assert_eq!(resp.status_code(), 403, "student should get 403");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn accessibility_scan_page_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "A11y404Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let resp = server
        .post(&format!(
            "/courses/{course}/pages/nonexistent-page/accessibility/scan"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 404, "nonexistent page should 404");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn accessibility_queue_scan_happy_path() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "QueueA11yCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let page = seed_page(&pool, course, "Queue Page", "queue-page", "body").await;
    let bearer = auth(teacher, "teacher");

    let resp = server
        .post(&format!(
            "/courses/{course}/pages/{page}/accessibility/queue_scan"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "completed");
    assert_eq!(body["issue_count"], 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn accessibility_queue_scan_requires_teacher() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "QueueStudentCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let page = seed_page(&pool, course, "Q Page", "q-page", "body").await;
    let student_bearer = auth(student, "student");

    let resp = server
        .post(&format!(
            "/courses/{course}/pages/{page}/accessibility/queue_scan"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer)
        .await;
    assert_eq!(resp.status_code(), 403, "student should get 403");

    cleanup(&pool, &prefix).await;
}
