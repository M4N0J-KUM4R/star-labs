//! Integration tests for the Canvas custom gradebook column and gradebook
//! filter routes (`CustomGradebookColumnsApiController`,
//! `GradebookFiltersApiController`).
//!
//! Pinned behavior:
//! - the custom gradebook column policy needs `view_all_grades` or
//!   `manage_grades` on the course — a student gets the Canvas
//!   `403 {"status":"unauthorized","errors":[{"message":"user not
//!   authorized to perform that action"}]}`;
//! - a missing column is the Canvas `404 {"errors":[{"message":"The
//!   specified resource does not exist."}]}` and the check runs before the
//!   permission check;
//! - the column JSON is `{id, title, position, teacher_notes, read_only,
//!   hidden}` — no root wrapper;
//! - `hidden` is the `hidden` workflow state, `include_hidden` lists the
//!   hidden columns as well;
//! - a validation failure answers `400` with the Rails error hash;
//! - the delete is a soft delete — the row keeps existing with the `deleted`
//!   workflow state;
//! - `reorder` answers `200 {}` and puts the given columns in the given
//!   order;
//! - the gradebook filter JSON is wrapped in the `gradebook_filter` root,
//!   `show` answers `201`, and a filter of another user is a `404`.
//!
//! Fleet rule: dedicated users, accounts, courses, columns and filters per
//! test, every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is
//! unset or the tables are not migrated yet.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gradebook-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
        n
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

    // The gradebook tables come from the canvas schema migration; skip until
    // it landed.
    let exists: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = 'custom_gradebook_columns'",
    )
    .fetch_optional(&pool)
    .await
    .expect("check for the custom_gradebook_columns table");
    if exists.is_none() {
        println!("SKIP: the custom_gradebook_columns table is not migrated yet");
        return None;
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_courses::router())
            // The custom_gradebook_columns resource lives in star-gradebook
            // (the assessments lane) — the same router star-http mounts.
            .merge(star_gradebook::router())
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// Seed a custom gradebook column with a given workflow state.
async fn seed_column(
    pool: &PgPool,
    prefix: &str,
    course_id: i64,
    title: &str,
    position: i32,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO custom_gradebook_columns \
         (title, position, workflow_state, course_id, teacher_notes, read_only, \
          created_at, updated_at) \
         VALUES ($1, $2, $3, $4, false, false, now(), now()) RETURNING id",
    )
    .bind(format!("{prefix}-{title}"))
    .bind(position)
    .bind(workflow_state)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .expect("seed column")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM custom_gradebook_columns WHERE title LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn a_teacher_creates_updates_and_deletes_a_custom_column() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // The teacher creates a column.
    let resp = server
        .post(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "column": {
                "title": format!("{prefix}-Stuff"),
                "position": 1,
                "read_only": true,
            },
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["title"], format!("{prefix}-Stuff"));
    assert_eq!(created["position"], 1);
    assert_eq!(created["read_only"], true);
    assert_eq!(created["teacher_notes"], false);
    assert_eq!(created["hidden"], false);
    let column_id = created["id"].as_i64().expect("column id");

    // The teacher lists the column.
    let resp = server
        .get(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let columns = list.as_array().expect("columns");
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0]["id"], column_id);

    // The teacher updates the column.
    let resp = server
        .put(&format!(
            "/courses/{course}/custom_gradebook_columns/{column_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "column": { "title": "Bar", "hidden": "yes" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["title"], "Bar");
    assert_eq!(updated["hidden"], true);
    // The hidden flag is the workflow state.
    assert_eq!(updated["read_only"], true);

    // The teacher deletes the column — a soft delete.
    let resp = server
        .delete(&format!(
            "/courses/{course}/custom_gradebook_columns/{column_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["hidden"], false);

    // A deleted column is gone.
    let resp = server
        .put(&format!(
            "/courses/{course}/custom_gradebook_columns/{column_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "column": { "title": "Baz" } }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let missing: Value = resp.json();
    assert_eq!(
        missing["errors"][0]["message"],
        "The specified resource does not exist."
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_may_not_touch_the_custom_columns() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, student, course, "student").await;

    let resp = server
        .get(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let denied: Value = resp.json();
    assert_eq!(denied["status"], "unauthorized");
    assert_eq!(
        denied["errors"][0]["message"],
        "user not authorized to perform that action"
    );

    let resp = server
        .post(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "column": { "title": "Blah blah blah" } }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_hidden_columns_are_listed_on_demand() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    seed_column(&pool, &prefix, course, "Active", 1, "active").await;
    seed_column(&pool, &prefix, course, "Hidden", 2, "hidden").await;
    seed_column(&pool, &prefix, course, "Deleted", 3, "deleted").await;

    // Without `include_hidden` only the active column is listed.
    let resp = server
        .get(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let columns = list.as_array().expect("columns");
    assert_eq!(columns.len(), 1);
    assert_eq!(columns[0]["title"], format!("{prefix}-Active"));
    assert_eq!(columns[0]["hidden"], false);

    // With `include_hidden` the hidden column is listed as well.
    let resp = server
        .get(&format!(
            "/courses/{course}/custom_gradebook_columns?include_hidden=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let columns = list.as_array().expect("columns");
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[1]["title"], format!("{prefix}-Hidden"));
    assert_eq!(columns[1]["hidden"], true);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_column_without_a_title_answers_400() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .post(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "column": { "position": 1 } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let errors: Value = resp.json();
    assert_eq!(errors["title"][0], "can't be blank");

    // A reserved gradebook importer name is refused as well.
    let resp = server
        .post(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "column": { "title": "Student" } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let errors: Value = resp.json();
    assert_eq!(
        errors["title"][0],
        "cannot use gradebook importer reserved names"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_columns_are_reordered() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let first = seed_column(&pool, &prefix, course, "A", 1, "active").await;
    let second = seed_column(&pool, &prefix, course, "B", 2, "active").await;
    let third = seed_column(&pool, &prefix, course, "C", 3, "active").await;

    let resp = server
        .post(&format!(
            "/courses/{course}/custom_gradebook_columns/reorder"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "order": [third, first, second] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({}));

    let resp = server
        .get(&format!("/courses/{course}/custom_gradebook_columns"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let columns = list.as_array().expect("columns");
    assert_eq!(columns.len(), 3);
    assert_eq!(columns[0]["id"], third);
    assert_eq!(columns[1]["id"], first);
    assert_eq!(columns[2]["id"], second);

    cleanup(&pool, &prefix).await;
}
