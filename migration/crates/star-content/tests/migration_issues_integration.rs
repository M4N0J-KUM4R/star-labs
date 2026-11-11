//! Integration tests for the content-migration deep routes:
//! `PUT /courses/{id}/content_migrations/{id}`,
//! `GET .../content_migrations/{id}/selective_data`, and the
//! migration_issues list/create/update routes.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
//!   cargo test -p star-content
//! - Unique per-run prefix + full teardown, safe against a shared database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "cmissues-tests-{}-{}",
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
        TestServer::new(star_content::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
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

/// Seed a content migration row directly (no worker runs in this workspace).
async fn seed_migration(pool: &PgPool, course_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO content_migrations \
         (course_id, user_id, migration_type, workflow_state, migration_settings) \
         VALUES ($1, $2, 'common_cartridge_importer', 'importing', '{}') RETURNING id",
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed migration")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
    )
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
// PUT /courses/{courseId}/content_migrations/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_migration_merges_settings_and_rejects_conflicting_date_shift() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UpdateCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let migration = seed_migration(&pool, course, teacher).await;

    let resp = server
        .put(&format!("/courses/{course}/content_migrations/{migration}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "settings": { "file_url": "https://example.com/pkg.zip" },
            "date_shift_options": { "shift_dates": true, "old_start_date": "2024-01-01" }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], migration);
    assert_eq!(body["migration_type"], "common_cartridge_importer");

    let settings: String =
        sqlx::query_scalar("SELECT migration_settings FROM content_migrations WHERE id = $1")
            .bind(migration)
            .fetch_one(&pool)
            .await
            .expect("read settings");
    let settings: Value = serde_json::from_str(&settings).expect("settings json");
    assert_eq!(settings["file_url"], "https://example.com/pkg.zip");
    assert_eq!(settings["date_shift_options"]["shift_dates"], true);

    // Canvas rejects shift_dates + remove_dates together.
    let resp = server
        .put(&format!("/courses/{course}/content_migrations/{migration}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "date_shift_options": { "shift_dates": true, "remove_dates": true }
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // A non-enrolled teacher of another course is forbidden.
    let outsider = seed_user(&pool, &prefix, "Outsider", "teacher").await;
    let resp = server
        .put(&format!("/courses/{course}/content_migrations/{migration}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "teacher"))
        .json(&json!({ "settings": { "file_url": "x" } }))
        .await;
    assert_eq!(resp.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// migration issues
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_issues_create_list_update_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "IssuesCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let migration = seed_migration(&pool, course, teacher).await;

    // POST creates an issue (Canvas has no create action; 200 by convention).
    let resp = server
        .post(&format!(
            "/courses/{course}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "description": "Questions in this quiz couldn't be converted",
            "issue_type": "warning",
            "fixable_by_admin": true
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let issue: Value = resp.json();
    let issue_id = issue["id"].as_i64().expect("issue id");
    assert_eq!(issue["content_migration_id"], migration);
    assert_eq!(issue["issue_type"], "warning");
    assert_eq!(issue["workflow_state"], "active");
    assert_eq!(issue["unresolved"], true);
    assert_eq!(issue["fixable_by_admin"], true);
    assert_eq!(
        issue["content_migration_url"],
        format!("/api/v1/courses/{course}/content_migrations/{migration}")
    );

    // GET index lists it.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert_eq!(list[0]["id"], issue_id);

    // PUT resolves it.
    let resp = server
        .put(&format!(
            "/courses/{course}/content_migrations/{migration}/migration_issues/{issue_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "workflow_state": "resolved" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let issue: Value = resp.json();
    assert_eq!(issue["workflow_state"], "resolved");
    assert_eq!(issue["unresolved"], false);

    // Canvas answers an invalid state with 403 + message.
    let resp = server
        .put(&format!(
            "/courses/{course}/content_migrations/{migration}/migration_issues/{issue_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "workflow_state": "nonsense" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "Must send a valid workflow state"
    );

    // A non-enrolled student cannot read or write issues.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Unknown migration id is a 404.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/999999999/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET .../selective_data
// ---------------------------------------------------------------------------

#[tokio::test]
async fn selective_data_lists_sections_and_rejects_unknown_type() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "SelectiveCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let migration = seed_migration(&pool, course, teacher).await;

    sqlx::query("INSERT INTO assignments (course_id, title) VALUES ($1, $2)")
        .bind(course)
        .bind("Selective Assignment")
        .execute(&pool)
        .await
        .expect("seed assignment");

    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/selective_data"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let sections: Value = resp.json();
    assert_eq!(sections[0]["type"], "course_settings");
    let assignments = sections
        .as_array()
        .expect("array")
        .iter()
        .find(|node| node["type"] == "assignments")
        .expect("assignments section");
    assert_eq!(assignments["count"], 1);

    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/selective_data?type=assignments"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let items: Value = resp.json();
    assert_eq!(items.as_array().map(Vec::len), Some(1));
    assert_eq!(items[0]["title"], "Selective Assignment");
    assert!(items[0]["property"]
        .as_str()
        .expect("property")
        .starts_with("copy[assignments]["));

    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/selective_data?type=bogus"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "unsupported migration type"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn migration_issues_require_authentication() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let res = server
        .get("/courses/1/content_migrations/1/migration_issues")
        .await;
    assert_eq!(res.status_code(), 401);
}
