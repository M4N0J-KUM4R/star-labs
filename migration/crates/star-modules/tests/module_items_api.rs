//! Phase 3 module-items parity tests (migration 0065).
//!
//! Covers the Canvas `context_module_items_api` routes added on top of the
//! Phase 2 module slice:
//! - GET/PUT/DELETE /courses/{courseId}/modules/items/{itemId}
//! - POST /courses/{courseId}/modules/items/{itemId}/duplicate
//! - PUT /courses/{courseId}/modules/{moduleId}/items/{itemId}
//!
//! Conventions mirror the rest of the workspace: tests skip cleanly when
//! DATABASE_URL is unset and clean up every row they create.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

/// An id far beyond any seeded row, for 404 probes.
const MISSING_ID: i64 = 999_999_999;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "mod-items-{}-{}",
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

    let server =
        TestServer::new(star_modules::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

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

async fn seed_module(pool: &PgPool, course_id: i64, name: &str, position: i32) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO modules (course_id, name, position, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(name)
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed module")
}

async fn seed_module_item(
    pool: &PgPool,
    module_id: i64,
    item_type: &str,
    item_id: i64,
    position: i32,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO module_items (module_id, item_type, item_id, position)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(module_id)
    .bind(item_type)
    .bind(item_id)
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed module item")
}

async fn seed_page(pool: &PgPool, course_id: i64, title: &str, url: Option<&str>) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO pages (course_id, title, body, url)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(format!("{title} body"))
    .bind(url)
    .fetch_one(pool)
    .await
    .expect("seed page")
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, description, points_possible)
        VALUES ($1, $2, 'assignment description', 10)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str, state: &str) {
    sqlx::query(
        r#"
        INSERT INTO enrollments (user_id, course_id, role, workflow_state)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .bind(state)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

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
// GET /courses/{courseId}/modules/items/{itemId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_item_detail_returns_canvas_shape_and_honors_read_authz() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "ItemDetailCourse").await;
    let other_course = seed_course(&pool, &prefix, "ItemDetailOther").await;
    let module = seed_module(&pool, course, "Detail Module", 1).await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let item = seed_module_item(&pool, module, "assignment", assignment, 1).await;

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Anonymous -> 401 from the AuthUser extractor.
    let resp = server
        .get(&format!("/courses/{course}/modules/items/{item}"))
        .await;
    assert_eq!(resp.status_code(), 401, "anonymous item detail");

    // No enrollment -> 403 (Canvas authorizes :read before resolving).
    let resp = server
        .get(&format!("/courses/{course}/modules/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "unenrolled item detail");

    // Enrolled student -> 200 with the Canvas module item shape.
    let resp = server
        .get(&format!("/courses/{course}/modules/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], item);
    assert_eq!(body["module_id"], module);
    assert_eq!(body["position"], 1);
    assert_eq!(body["title"], "Essay");
    assert_eq!(body["indent"], 0);
    assert_eq!(body["type"], "Assignment", "Canvas API_DATA_TYPE name");
    assert_eq!(body["content_id"], assignment);
    assert_eq!(body["published"], true);
    assert_eq!(
        body["html_url"],
        format!("/courses/{course}/modules/items/{item}")
    );
    assert_eq!(
        body["url"],
        format!("/api/v1/courses/{course}/assignments/{assignment}")
    );

    // include[]=content_details (percent-encoded brackets) adds the
    // assignment content details.
    let resp = server
        .get(&format!(
            "/courses/{course}/modules/items/{item}?include%5B%5D=content_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["content_details"]["points_possible"], 10.0);

    // Canonical Canvas path (module-scoped) returns the same resource.
    let resp = server
        .get(&format!("/courses/{course}/modules/{module}/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let nested: Value = resp.json();
    assert_eq!(nested["id"], item, "nested show mirrors the flat path");

    // Wrong course in the path -> 404 (item is scoped to its course);
    // probe as admin so the read gate passes and the lookup decides.
    let resp = server
        .get(&format!("/courses/{other_course}/modules/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "cross-course item detail");

    // Missing item -> 404.
    let resp = server
        .get(&format!("/courses/{course}/modules/items/{MISSING_ID}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "missing item detail");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /courses/{courseId}/modules/items/{itemId}
// PUT /courses/{courseId}/modules/{moduleId}/items/{itemId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_item_routes_apply_title_indent_position_and_move() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ItemUpdateCourse").await;
    let other_course = seed_course(&pool, &prefix, "ItemUpdateOther").await;
    let module = seed_module(&pool, course, "Update Module", 1).await;
    let target_module = seed_module(&pool, course, "Target Module", 2).await;
    let foreign_module = seed_module(&pool, other_course, "Foreign Module", 1).await;

    let page_a = seed_page(&pool, course, "Alpha", Some("alpha")).await;
    let page_b = seed_page(&pool, course, "Beta", Some("beta")).await;
    let item_a = seed_module_item(&pool, module, "page", page_a, 1).await;
    let item_b = seed_module_item(&pool, module, "page", page_b, 2).await;

    // Read access (nested GET probes) needs an active enrollment.
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    let auth_header = auth(teacher, "teacher");

    // Students cannot update items.
    let resp = server
        .put(&format!("/courses/{course}/modules/items/{item_a}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "module_item": { "title": "Nope" } }))
        .await;
    assert_eq!(resp.status_code(), 403, "student item update");

    // Canvas item route: rename + indent.
    let resp = server
        .put(&format!("/courses/{course}/modules/items/{item_a}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .json(&json!({ "module_item": { "title": "Intro", "indent": 2 } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["title"], "Intro");
    assert_eq!(updated["indent"], 2);
    assert_eq!(updated["position"], 1, "title/indent do not reorder");

    // Canvas module-scoped route: position 2 moves the item after Beta.
    let resp = server
        .put(&format!(
            "/courses/{course}/modules/{module}/items/{item_a}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .json(&json!({ "module_item": { "position": 2 } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let moved: Value = resp.json();
    assert_eq!(moved["position"], 2, "item moved to position 2");
    let positions: Vec<(i64, i32)> = sqlx::query_as(
        "SELECT id, position FROM module_items WHERE module_id = $1 ORDER BY position",
    )
    .bind(module)
    .fetch_all(&pool)
    .await
    .expect("read positions");
    assert_eq!(
        positions,
        vec![(item_b, 1), (item_a, 2)],
        "siblings renumber contiguously (Canvas insert_at)"
    );

    // Nested paths 404 when the item lives in another module.
    let resp = server
        .get(&format!(
            "/courses/{course}/modules/{target_module}/items/{item_a}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "item not in the path module");

    // Move the item into another module of the same course.
    let resp = server
        .put(&format!(
            "/courses/{course}/modules/{module}/items/{item_a}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .json(&json!({ "module_item": { "module_id": target_module } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let moved: Value = resp.json();
    assert_eq!(moved["module_id"], target_module, "item moved modules");
    assert_eq!(moved["position"], 1, "appended into the (empty) target");

    // Moving into a module from another course is rejected by Canvas message.
    let resp = server
        .put(&format!("/courses/{course}/modules/items/{item_b}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .json(&json!({ "module_item": { "module_id": foreign_module } }))
        .await;
    assert_eq!(resp.status_code(), 400, "foreign module_id");
    let body: Value = resp.json();
    assert_eq!(body["message"], "invalid module_id");

    // No module_item attributes -> Canvas 400 shape.
    let resp = server
        .put(&format!("/courses/{course}/modules/items/{item_b}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header)
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "missing module item parameter");
    let body: Value = resp.json();
    assert_eq!(body["message"], "missing module item parameter");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /courses/{courseId}/modules/items/{itemId}/duplicate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_item_copies_content_after_original() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ItemDuplicateCourse").await;
    let module = seed_module(&pool, course, "Duplicate Module", 1).await;

    let page = seed_page(&pool, course, "Syllabus", Some("syllabus")).await;
    let item = seed_module_item(&pool, module, "page", page, 1).await;

    // Students cannot duplicate.
    let resp = server
        .post(&format!("/courses/{course}/modules/items/{item}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student duplicate");

    // Teacher duplicate -> new item right after the original.
    let resp = server
        .post(&format!("/courses/{course}/modules/items/{item}/duplicate"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let copy: Value = resp.json();
    let copy_id = copy["id"].as_i64().expect("copy id");
    assert_ne!(copy_id, item, "duplicate is a new module item");
    assert_eq!(copy["module_id"], module);
    assert_eq!(copy["position"], 2, "inserted after the original");
    assert_eq!(copy["title"], "Syllabus Copy");
    assert_eq!(copy["type"], "Page");
    assert_eq!(copy["page_url"], "syllabus-copy");
    assert_eq!(
        copy["new_positions"]
            .as_array()
            .expect("new_positions")
            .len(),
        2,
        "Canvas returns the reordered module positions"
    );

    // The linked page was duplicated rather than shared.
    let copies: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pages WHERE course_id = $1")
        .bind(course)
        .fetch_one(&pool)
        .await
        .expect("count pages");
    assert_eq!(copies, 2, "one page copy created");
    let (copied_page_id,): (i64,) =
        sqlx::query_as("SELECT item_id FROM module_items WHERE id = $1")
            .bind(copy_id)
            .fetch_one(&pool)
            .await
            .expect("copy module item");
    assert_ne!(copied_page_id, page, "copy points at the new page");
    let (copied_title,): (String,) = sqlx::query_as("SELECT title FROM pages WHERE id = $1")
        .bind(copied_page_id)
        .fetch_one(&pool)
        .await
        .expect("copied page");
    assert_eq!(copied_title, "Syllabus Copy");

    // Non-duplicable item types answer with the Canvas 400 message.
    let sub_header = seed_module_item(&pool, module, "sub_header", 0, 3).await;
    let resp = server
        .post(&format!(
            "/courses/{course}/modules/items/{sub_header}/duplicate"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "sub header duplicate");
    let body: Value = resp.json();
    assert_eq!(body["message"], "Item cannot be duplicated");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /courses/{courseId}/modules/items/{itemId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_item_detail_returns_item_and_removes_row() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ItemDeleteCourse").await;
    let other_course = seed_course(&pool, &prefix, "ItemDeleteOther").await;
    let module = seed_module(&pool, course, "Delete Module", 1).await;
    let assignment = seed_assignment(&pool, course, "Quiz 1").await;
    let item = seed_module_item(&pool, module, "assignment", assignment, 1).await;

    // Students cannot delete module items.
    let resp = server
        .delete(&format!("/courses/{course}/modules/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student item delete");

    // Cross-course path -> 404 and the row survives.
    let resp = server
        .delete(&format!("/courses/{other_course}/modules/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "cross-course item delete");

    // Teacher delete -> 200 with the deleted item JSON (Canvas destroy shape).
    let resp = server
        .delete(&format!("/courses/{course}/modules/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["id"], item);
    assert_eq!(deleted["title"], "Quiz 1");
    assert_eq!(deleted["type"], "Assignment");

    // The module item is gone; the linked assignment survives.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM module_items WHERE id = $1")
        .bind(item)
        .fetch_one(&pool)
        .await
        .expect("count item");
    assert_eq!(count, 0, "module item removed");
    let assignments: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM assignments WHERE id = $1")
        .bind(assignment)
        .fetch_one(&pool)
        .await
        .expect("count assignment");
    assert_eq!(assignments, 1, "ContentTag destroy keeps the content");

    // Repeat delete -> 404.
    let resp = server
        .delete(&format!("/courses/{course}/modules/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "repeat item delete");

    cleanup(&pool, &prefix).await;
}
