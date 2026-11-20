//! Integration tests for migration 0213 — course_paces + course_pace_module_items
//! and the M1/M3/M4 context_module_items_api routes added in this slice.
//!
//! Covers:
//! - CoursePace CRUD: GET/POST /courses/{courseId}/course_pacing,
//!   GET/PUT/DELETE /courses/{courseId}/course_pacing/{paceId},
//!   POST /courses/{courseId}/course_pacing/{paceId}/publish
//! - M1: GET /courses/{courseId}/module_item_sequence
//! - M3: POST /courses/{courseId}/modules/{moduleId}/items/{itemId}/mark_read
//! - M4: POST /courses/{courseId}/modules/{moduleId}/items/{itemId}/select_mastery_path
//!
//! Conventions: tests skip cleanly when DATABASE_URL is unset and clean up
//! every row they create.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// --------------------------------------------------------------------------|
// Test harness
// --------------------------------------------------------------------------|

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "pace-m0213-{}-{}",
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
        INSERT INTO module_items (module_id, item_type, item_id, position, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
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

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, description, points_possible)
        VALUES ($1, $2, 'test assignment', 10)
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

// --------------------------------------------------------------------------|
// Course Pace CRUD
// --------------------------------------------------------------------------|

#[tokio::test]
async fn get_course_pace_returns_404_when_none_exists() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PaceGetCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    // No pace exists yet -> 404.
    let resp = server
        .get(&format!("/courses/{course}/course_pacing"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "no pace -> 404");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_course_pace_and_fetch_it() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PaceCreateCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    let resp = server
        .post(&format!("/courses/{course}/course_pacing"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "course_pace": {
                "workflow_state": "unpublished",
                "hard_end_dates": false,
                "exclude_weekends": false,
                "time_to_complete_calendar_days": null,
                "assignments_weighting": null
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "create pace: {}", resp.text());
    let pace: Value = resp.json();
    let pace_id = pace["id"].as_i64().expect("pace id");
    assert_eq!(pace["course_id"], course);
    assert_eq!(pace["workflow_state"], "unpublished");

    // GET returns the same pace.
    let resp = server
        .get(&format!("/courses/{course}/course_pacing"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let fetched: Value = resp.json();
    assert_eq!(fetched["id"], pace_id);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_course_pace() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PaceUpdateCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    // Create a pace.
    let resp = server
        .post(&format!("/courses/{course}/course_pacing"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "course_pace": {
                "workflow_state": "unpublished",
                "hard_end_dates": false,
                "exclude_weekends": false
            }
        }))
        .await;
    let pace_id: i64 = resp.json()["id"].as_i64().expect("pace id");

    // Update it.
    let resp = server
        .put(&format!("/courses/{course}/course_pacing/{pace_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "course_pace": {
                "workflow_state": "active",
                "hard_end_dates": true,
                "end_date": "2025-12-31T23:59:59Z"
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "update pace: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["workflow_state"], "active");
    assert_eq!(updated["hard_end_dates"], true);

    // Non-existent pace -> 404.
    let resp = server
        .put(&format!("/courses/{course}/course_pacing/999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "course_pace": { "workflow_state": "active" } }))
        .await;
    assert_eq!(resp.status_code(), 404, "update non-existent pace");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_course_pace() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PaceDeleteCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    // Create.
    let resp = server
        .post(&format!("/courses/{course}/course_pacing"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "course_pace": { "workflow_state": "unpublished" } }))
        .await;
    let pace_id: i64 = resp.json()["id"].as_i64().expect("pace id");

    // Delete.
    let resp = server
        .delete(&format!("/courses/{course}/course_pacing/{pace_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "delete pace");

    // Re-fetch -> 404.
    let resp = server
        .get(&format!("/courses/{course}/course_pacing/{pace_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn publish_course_pace() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PacePublishCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    // Create.
    let resp = server
        .post(&format!("/courses/{course}/course_pacing"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "course_pace": { "workflow_state": "unpublished" } }))
        .await;
    let pace_id: i64 = resp.json()["id"].as_i64().expect("pace id");

    // Publish.
    let resp = server
        .post(&format!(
            "/courses/{course}/course_pacing/{pace_id}/publish"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "publish: {}", resp.text());
    let published: Value = resp.json();
    assert_eq!(published["workflow_state"], "active");

    cleanup(&pool, &prefix).await;
}

// --------------------------------------------------------------------------|
// M1: Module item sequence
// GET /courses/{courseId}/module_item_sequence?asset_type=...&asset_id=...
// --------------------------------------------------------------------------|

#[tokio::test]
async fn item_sequence_returns_empty_when_no_modules() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "SeqEmptyCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_type=Assignment&asset_id=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["items"], json!([]));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn item_sequence_returns_prev_next_current_around_assignment() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "SeqCourse").await;
    let module = seed_module(&pool, course, "Seq Module", 1).await;
    let a1 = seed_assignment(&pool, course, "Alpha").await;
    let a2 = seed_assignment(&pool, course, "Beta").await;
    let a3 = seed_assignment(&pool, course, "Gamma").await;
    let item1 = seed_module_item(&pool, module, "assignment", a1, 1).await;
    let item2 = seed_module_item(&pool, module, "assignment", a2, 2).await;
    let item3 = seed_module_item(&pool, module, "assignment", a3, 3).await;

    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    // Query for item2 (Beta) — should get prev=item1, next=item3.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_type=Assignment&asset_id={a2}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "item_sequence: {}", resp.text());
    let body: Value = resp.json();
    let items = body["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    let entry = &items[0];
    assert_eq!(entry["current"]["id"], item2);
    assert_eq!(entry["prev"]["id"], item1);
    assert_eq!(entry["next"]["id"], item3);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn item_sequence_returns_items_around_module_item_by_id() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "SeqByIdCourse").await;
    let module = seed_module(&pool, course, "SeqById Module", 1).await;
    let a1 = seed_assignment(&pool, course, "One").await;
    let a2 = seed_assignment(&pool, course, "Two").await;
    let item1 = seed_module_item(&pool, module, "assignment", a1, 1).await;
    let item2 = seed_module_item(&pool, module, "assignment", a2, 2).await;

    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    // asset_type=ModuleItem means find by module_item id directly.
    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_type=ModuleItem&asset_id={item2}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "item_sequence by id: {}",
        resp.text()
    );
    let body: Value = resp.json();
    let items = body["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["current"]["id"], item2);
    assert_eq!(items[0]["prev"]["id"], item1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn item_sequence_invalid_asset_type_returns_400() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "SeqBadTypeCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    let resp = server
        .get(&format!(
            "/courses/{course}/module_item_sequence?asset_type=NotValid&asset_id=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "invalid asset_type");

    cleanup(&pool, &prefix).await;
}

// --------------------------------------------------------------------------|
// M3: Mark item read
// POST /courses/{courseId}/modules/{moduleId}/items/{itemId}/mark_read
// --------------------------------------------------------------------------|

#[tokio::test]
async fn mark_item_read_records_completion() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "MarkReadCourse").await;
    let module = seed_module(&pool, course, "MarkRead Module", 1).await;
    let a1 = seed_assignment(&pool, course, "ReadMe").await;
    let item = seed_module_item(&pool, module, "assignment", a1, 1).await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    let resp = server
        .post(&format!(
            "/courses/{course}/modules/{module}/items/{item}/mark_read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "mark_read: {}", resp.text());

    // Confirm a completion was recorded.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM module_completions WHERE user_id = $1 AND module_item_id = $2",
    )
    .bind(student)
    .bind(item)
    .fetch_one(&pool)
    .await
    .expect("count completions");
    assert_eq!(count, 1, "completion recorded");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn mark_item_read_idempotent() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "MarkReadIdemCourse").await;
    let module = seed_module(&pool, course, "MarkReadIdem Module", 1).await;
    let a1 = seed_assignment(&pool, course, "ReadMeIdem").await;
    let item = seed_module_item(&pool, module, "assignment", a1, 1).await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Mark read twice.
    for _ in 0..2 {
        let resp = server
            .post(&format!(
                "/courses/{course}/modules/{module}/items/{item}/mark_read"
            ))
            .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
            .await;
        assert_eq!(resp.status_code(), 200, "mark_read idempotent");
    }

    // Exactly one completion record.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM module_completions WHERE user_id = $1 AND module_item_id = $2",
    )
    .bind(student)
    .bind(item)
    .fetch_one(&pool)
    .await
    .expect("count completions");
    assert_eq!(count, 1, "idempotent — one record");

    cleanup(&pool, &prefix).await;
}

// --------------------------------------------------------------------------|
// M4: Select mastery path
// POST /courses/{courseId}/modules/{moduleId}/items/{itemId}/select_mastery_path
// --------------------------------------------------------------------------|

#[tokio::test]
async fn select_mastery_path_on_assignment_returns_items() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "MasteryCourse").await;
    let module = seed_module(&pool, course, "Mastery Module", 1).await;
    let a1 = seed_assignment(&pool, course, "Mastery Source").await;
    let item = seed_module_item(&pool, module, "assignment", a1, 1).await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Create a dummy assignment set id for the request body.
    // Full CYOE is deferred; the stub returns a simplified response.
    let resp = server
        .post(&format!(
            "/courses/{course}/modules/{module}/items/{item}/select_mastery_path"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "assignment_set_id": 1 }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "select_mastery_path: {}",
        resp.text()
    );
    let body: Value = resp.json();
    // The stub returns items + assignments + meta.primaryCollection.
    assert!(body["items"].is_array());
    assert!(body["assignments"].is_array());
    assert_eq!(body["meta"]["primaryCollection"], "assignments");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn select_mastery_path_rejects_non_assignment_item() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "MasteryBadCourse").await;
    let module = seed_module(&pool, course, "MasteryBad Module", 1).await;
    let a1 = seed_assignment(&pool, course, "Not Assignment Item").await;
    // Seed a module item of type "page" with no linked assignment.
    let page_id = sqlx::query_scalar::<_, i64>(
        r#"INSERT INTO pages (course_id, title, body, url) VALUES ($1, 'Stub', 'body', 'stub') RETURNING id"#,
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed page");
    let item = seed_module_item(&pool, module, "page", page_id, 1).await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    let resp = server
        .post(&format!(
            "/courses/{course}/modules/{module}/items/{item}/select_mastery_path"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "assignment_set_id": 1 }))
        .await;
    assert_eq!(resp.status_code(), 400, "non-assignment item rejected");

    cleanup(&pool, &prefix).await;
}

// --------------------------------------------------------------------------|
// Authz — students cannot manage paces
// --------------------------------------------------------------------------|

#[tokio::test]
async fn student_cannot_create_pace() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "StudentPaceCourse").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    let resp = server
        .post(&format!("/courses/{course}/course_pacing"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "course_pace": { "workflow_state": "unpublished" } }))
        .await;
    assert_eq!(resp.status_code(), 403, "student cannot create pace");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn student_cannot_delete_pace() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "StudentDelPaceCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Create as teacher.
    let resp = server
        .post(&format!("/courses/{course}/course_pacing"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "course_pace": { "workflow_state": "unpublished" } }))
        .await;
    let pace_id: i64 = resp.json()["id"].as_i64().expect("pace id");

    // Student tries to delete.
    let resp = server
        .delete(&format!("/courses/{course}/course_pacing/{pace_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student cannot delete pace");

    cleanup(&pool, &prefix).await;
}
