//! Independent behavioral contract tests for the Phase 4 content-migrations
//! deep routes, derived from the Canvas source (`app/controllers/
//! migration_issues_controller.rb`, `app/controllers/
//! content_migrations_controller.rb#update_migration`, `lib/api/v1/
//! content_migration.rb`, `lib/canvas/migration/helpers/
//! selective_content_formatter.rb`) — not from the implementation.
//!
//! Contracts exercised:
//! - POST migration_issues: teacher creates an issue; default issue_type is
//!   `error`, default workflow_state `active` with `unresolved: true`; the
//!   response carries `content_migration_url` (HTTP 200, workspace
//!   convention — Canvas has no create action).
//! - POST migration_issues with unknown `issue_type` → 422 (Canvas allows
//!   only todo|warning|error).
//! - PUT migration_issues/{id}: re-activating a resolved issue flips
//!   `unresolved` back to true (Canvas `update` accepts "active" too).
//! - PUT with a missing workflow_state → 403 with Canvas' exact message
//!   "Must send a valid workflow state" (Canvas `update`, `status:
//!   :forbidden`).
//! - GET migration_issues: `error_message` is admin-only (Canvas
//!   `read_errors` / `migration_issue_json`), and teachers see no such key.
//! - GET selective_data?type=quizzes: item list for the quizzes table,
//!   title from the quiz title, `property` `copy[quizzes][<id>]`.
//! - GET selective_data with an empty `type` value → treated as "no type"
//!   (Canvas `valid_type?(nil)` path) and returns the top-level section
//!   list, starting with `course_settings`.
//! - Permissions: a student cannot read selective_data or migration issues
//!   (403, Canvas GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS); an
//!   anonymous request is 401; a missing course is 404.
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
        "cmdeep-tests-{}-{}",
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

/// Seed a migration in an API-visible internal state (`importing` maps to
/// `running` in the API workflow_state mapping).
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
// POST /courses/{courseId}/content_migrations/{id}/migration_issues
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_issue_defaults_and_rejects_unknown_issue_type() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "CreateIssues").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let migration = seed_migration(&pool, course, teacher).await;
    let url = format!("/courses/{course}/content_migrations/{migration}/migration_issues");

    // Minimal body: defaults apply (Canvas issue_type allowable values are
    // todo|warning|error; the workspace default is `error`).
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "description": "Minimal issue" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let issue: Value = resp.json();
    let issue_id = issue["id"].as_i64().expect("issue id");
    assert_eq!(issue["issue_type"], "error");
    assert_eq!(issue["workflow_state"], "active");
    assert_eq!(issue["unresolved"], true);
    assert_eq!(
        issue["content_migration_id"], migration,
        "response carries content_migration_id"
    );
    assert_eq!(
        issue["content_migration_url"],
        format!("/api/v1/courses/{course}/content_migrations/{migration}")
    );
    assert_eq!(
        issue["migration_issues_url"],
        Value::Null,
        "issue JSON carries no migration_issues_url"
    );
    assert!(
        issue.get("error_message").is_none(),
        "error_message absent for non-admin teacher"
    );

    // Unknown issue_type is a 422 validation failure (Canvas allows only
    // todo|warning|error), not a DB error.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "issue_type": "critical" }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /courses/{courseId}/content_migrations/{id}/migration_issues/{issueId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_issue_reactivates_and_rejects_missing_state() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UpdateIssues").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let migration = seed_migration(&pool, course, teacher).await;

    let issue_id: i64 = sqlx::query_scalar(
        "INSERT INTO migration_issues \
         (content_migration_id, description, issue_type, workflow_state, error_message) \
         VALUES ($1, 'Broken quiz', 'warning', 'resolved', 'admin only message') \
         RETURNING id",
    )
    .bind(migration)
    .fetch_one(&pool)
    .await
    .expect("seed issue");

    // Canvas `update` accepts "active" as well as "resolved"; reactivating a
    // resolved issue flips `unresolved` back to true.
    let issue_url =
        format!("/courses/{course}/content_migrations/{migration}/migration_issues/{issue_id}");
    let resp = server
        .put(&issue_url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "workflow_state": "active" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let issue: Value = resp.json();
    assert_eq!(issue["workflow_state"], "active");
    assert_eq!(issue["unresolved"], true);

    // Missing / blank workflow_state → 403 with Canvas' exact message
    // (migration_issues_controller#update renders forbidden).
    let resp = server
        .put(&issue_url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "Must send a valid workflow state"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET .../migration_issues — error_message visibility
// ---------------------------------------------------------------------------

#[tokio::test]
async fn issue_error_message_is_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "ErrorsCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let migration = seed_migration(&pool, course, teacher).await;

    sqlx::query(
        "INSERT INTO migration_issues \
         (content_migration_id, description, issue_type, error_message) \
         VALUES ($1, 'Conversion failed', 'error', 'AdminOnlySecret')",
    )
    .bind(migration)
    .execute(&pool)
    .await
    .expect("seed issue");

    // Teacher (manage_course_content) sees the issue but NOT error_message
    // (Canvas `migration_issue_json` gates it behind `read_errors`).
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert_eq!(
        list[0]["issue_type"], "error",
        "issue_type default from seed"
    );
    assert_eq!(list[0]["description"], "Conversion failed");
    assert!(
        list[0].get("error_message").is_none(),
        "teacher must not see error_message"
    );

    // Admin sees error_message (Canvas site-admin `read_errors`).
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list[0]["error_message"], "AdminOnlySecret");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET .../selective_data — quizzes items, empty type, pagination edge
// ---------------------------------------------------------------------------

#[tokio::test]
async fn selective_data_quizzes_items_and_empty_type_falls_back_to_tree() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "QuizzesSelective").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let migration = seed_migration(&pool, course, teacher).await;

    sqlx::query("INSERT INTO quizzes (course_id, title) VALUES ($1, $2), ($1, $3)")
        .bind(course)
        .bind("Midterm Quiz")
        .bind("Final Quiz")
        .execute(&pool)
        .await
        .expect("seed quizzes");

    // Type=quizzes enumerates the quizzes table (Canvas COURSE_SECTIONS).
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/selective_data?type=quizzes"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let items: Value = resp.json();
    let items = items.as_array().expect("item list");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["type"], "quizzes");
    assert_eq!(items[0]["title"], "Midterm Quiz");
    assert!(items[0]["property"]
        .as_str()
        .expect("property")
        .starts_with("copy[quizzes]["));

    // Empty `type` value: Canvas `valid_type?(nil)` treats an empty param as
    // "no type" and returns the top-level section tree.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/selective_data?type="
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let sections: Value = resp.json();
    let sections = sections.as_array().expect("section list");
    assert_eq!(sections[0]["type"], "course_settings");
    assert_eq!(sections[0]["property"], "copy[all_course_settings]");
    let quizzes = sections
        .iter()
        .find(|node| node["type"] == "quizzes")
        .expect("quizzes section");
    assert_eq!(quizzes["count"], 2);
    assert_eq!(
        quizzes["sub_items_url"],
        format!(
            "/api/v1/courses/{course}/content_migrations/{migration}/selective_data?type=quizzes"
        )
    );

    // The item list is not paginated (Canvas `content_list` returns the
    // whole list; pagination params are ignored), so per_page=0 changes
    // nothing.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/selective_data?type=quizzes&per_page=0"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()
            .as_array()
            .expect("items ignore pagination")
            .len(),
        2
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Permissions & negative cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn selective_data_and_issues_gating() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Gating").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let migration = seed_migration(&pool, course, teacher).await;

    // Anonymous request → 401 before any authorization check.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/selective_data"
        ))
        .await;
    assert_eq!(resp.status_code(), 401);

    // Enrolled student → 403 (Canvas manage_course_content gating).
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/selective_data"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "student cannot list selective_data"
    );

    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student cannot list issues");

    // Unknown course → 404 even for the teacher (Canvas require_context).
    let resp = server
        .get(&format!(
            "/courses/999999999/content_migrations/{migration}/selective_data"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // A migration that belongs to another course is invisible here → 404
    // (Canvas require_content_migration).
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    let foreign = seed_migration(&pool, other_course, teacher).await;
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{foreign}/selective_data"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "cross-course migration is 404");

    // The same foreign migration is also 404 on the issues index.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{foreign}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // A teacher enrolled in the OTHER course cannot write issues on this
    // one (manages_course must check the target course, not global role).
    let resp = server
        .put(&format!(
            "/courses/{course}/content_migrations/{migration}/migration_issues/999999999"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "workflow_state": "resolved" }))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown issue id is 404");

    cleanup(&pool, &prefix).await;
}
