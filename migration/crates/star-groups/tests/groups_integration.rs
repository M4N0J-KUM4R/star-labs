//! Integration tests for star-groups.
//!
//! Exercises the real groups router (POST /courses/{courseId}/groups, POST
//! /groups/{groupId}/memberships) through axum-test against a real Postgres.
//!
//! Regression scope: every `SELECT 1 ...` existence probe in
//! star-groups/src/lib.rs must decode as (i32,), because Postgres types the
//! literal 1 as INT4. Decoding into (i64,) only fails when the probe finds a
//! row, so each test below uses an existing course/group/user: before the fix
//! these requests returned 500 instead of 201.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-groups

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "grp-tests-{}-{}",
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
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_groups::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
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

async fn seed_teacher(pool: &PgPool, user_id: i64, course_id: i64) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'teacher')")
        .bind(user_id)
        .bind(course_id)
        .execute(pool)
        .await
        .expect("seed teacher enrollment");
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM groups WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM group_categories \
         WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
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

#[tokio::test]
async fn create_group_for_existing_course_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;

    let resp = server
        .post(&format!("/courses/{course}/groups"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Study Group" }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["name"], "Study Group");
    assert_eq!(body["course_id"].as_i64(), Some(course));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_membership_for_existing_group_and_user_succeeds() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;
    let group = seed_group(&pool, course, &format!("{prefix}-Group")).await;

    let resp = server
        .post(&format!("/groups/{group}/memberships"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "user_id": student }))
        .await;

    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["user_id"].as_i64(), Some(student));
    assert_eq!(body["workflow_state"], "invited");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PATCH /groups/{id} — Canvas emits both PATCH and PUT for updates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn patch_group_updates_like_put() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;
    let group = seed_group(&pool, course, &format!("{prefix}-PatchGroup")).await;
    sqlx::query("UPDATE groups SET description = 'original desc', join_level = 'parent_context_request' WHERE id = $1")
        .bind(group)
        .execute(&pool)
        .await
        .expect("seed group fields");

    // PATCH-style partial body: only name changes, like PUT's partial update.
    let resp = server
        .patch(&format!("/groups/{group}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Patched Group" }))
        .await;

    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["name"], "Patched Group");
    assert_eq!(body["description"], "original desc");
    assert_eq!(body["join_level"], "parent_context_request");

    // Change persisted in the DB, same as PUT.
    let (name, join_level): (String, String) =
        sqlx::query_as("SELECT name, join_level FROM groups WHERE id = $1")
            .bind(group)
            .fetch_one(&pool)
            .await
            .expect("fetch group");
    assert_eq!(name, "Patched Group");
    assert_eq!(join_level, "parent_context_request");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn patch_group_requires_teacher_authz() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student')")
        .bind(student)
        .bind(course)
        .execute(&pool)
        .await
        .expect("seed student enrollment");
    let group = seed_group(&pool, course, &format!("{prefix}-Locked")).await;

    // Existing PUT authz applies to the PATCH alias too.
    let resp = server
        .patch(&format!("/groups/{group}"))
        .json(&json!({ "name": "Anonymous Edit" }))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    let resp = server
        .patch(&format!("/groups/{group}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "name": "Student Edit" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Rejected patches leave the row untouched.
    let (name,): (String,) = sqlx::query_as("SELECT name FROM groups WHERE id = $1")
        .bind(group)
        .fetch_one(&pool)
        .await
        .expect("fetch group");
    assert_eq!(name, format!("{prefix}-Locked"));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Group categories — Canvas parity for GET/POST /courses/{courseId}/group_categories,
// GET/PUT/DELETE /group_categories/{id}, GET /group_categories/{id}/groups
// ---------------------------------------------------------------------------

async fn seed_category(pool: &PgPool, course_id: i64, name: &str, role: Option<&str>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO group_categories (course_id, name, role) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed group category")
}

async fn seed_group_in_category(
    pool: &PgPool,
    course_id: i64,
    category_id: i64,
    name: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO groups (course_id, name, group_category_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(category_id)
    .fetch_one(pool)
    .await
    .expect("seed group in category")
}

#[tokio::test]
async fn group_category_crud_returns_canvas_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student')")
        .bind(student)
        .bind(course)
        .execute(&pool)
        .await
        .expect("seed student enrollment");

    // Invalid enum values are rejected like Canvas (400, not 422).
    let resp = server
        .post(&format!("/courses/{course}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Bad", "self_signup": "sometimes" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Create returns the Canvas category shape.
    let resp = server
        .post(&format!("/courses/{course}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "name": "Project Groups",
            "self_signup": "enabled",
            "group_limit": 3
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let category_id = body["id"].as_i64().expect("category id");
    assert_eq!(body["name"], "Project Groups");
    assert_eq!(body["context_type"], "Course");
    assert_eq!(body["course_id"].as_i64(), Some(course));
    assert_eq!(body["self_signup"], "enabled");
    assert_eq!(body["group_limit"].as_i64(), Some(3));
    assert_eq!(body["protected"], false);
    assert_eq!(body["allows_multiple_memberships"], false);

    // Canvas index requires manage-groups permission; students are forbidden.
    let resp = server
        .get(&format!("/courses/{course}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(|a| a.len()), Some(1));

    let resp = server
        .get(&format!("/courses/{course}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Show is readable by enrolled students and reports is_member.
    let resp = server
        .get(&format!("/group_categories/{category_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["is_member"], false);
    assert_eq!(body["self_signup"], "enabled");

    // Partial update keeps unspecified fields.
    let resp = server
        .put(&format!("/group_categories/{category_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Renamed Groups" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["name"], "Renamed Groups");
    assert_eq!(body["self_signup"], "enabled");
    assert_eq!(body["group_limit"].as_i64(), Some(3));

    // create_group_count creates the groups inline (Canvas on create).
    let resp = server
        .post(&format!("/courses/{course}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Teams", "create_group_count": 2 }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let teams_id = body["id"].as_i64().expect("teams category id");

    let resp = server
        .get(&format!("/group_categories/{teams_id}/groups"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let groups: Value = resp.json();
    let groups = groups.as_array().expect("groups array");
    assert_eq!(groups.len(), 2);
    assert!(groups
        .iter()
        .all(|g| g["group_category_id"].as_i64() == Some(teams_id)));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_group_category_soft_deletes_it_and_protects_builtin_roles() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;
    let category = seed_category(&pool, course, "Project Groups", None).await;
    let group = seed_group_in_category(&pool, course, category, "Group A").await;

    let resp = server
        .delete(&format!("/group_categories/{category}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    // Canvas destroy renders the category JSON (200), and destroys its groups.
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(category));

    let (category_state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM group_categories WHERE id = $1")
            .bind(category)
            .fetch_one(&pool)
            .await
            .expect("fetch category");
    assert_eq!(category_state, "deleted");
    let (group_state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM groups WHERE id = $1")
            .bind(group)
            .fetch_one(&pool)
            .await
            .expect("fetch group");
    assert_eq!(group_state, "deleted");

    let resp = server
        .get(&format!("/group_categories/{category}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Protected built-in roles cannot be deleted (Canvas 401 "unauthorized").
    let protected = seed_category(&pool, course, "Communities", Some("communities")).await;
    let resp = server
        .delete(&format!("/group_categories/{protected}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
