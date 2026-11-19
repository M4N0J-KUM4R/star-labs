//! Integration tests for the content-migration deep routes:
//! `PUT /courses/{id}/content_migrations/{id}`,
//! `GET .../content_migrations/{id}/selective_data`, and the migration_issues
//! routes — the full Canvas route block (index/create/show/update) for every
//! context: courses, accounts, groups and users.
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

/// Seed a content migration attached to an account/group/user context
/// (migration 0126 polymorphic columns; Canvas `context.content_migrations`).
async fn seed_ctx_migration(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    user_id: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO content_migrations \
         (user_id, migration_type, workflow_state, migration_settings, \
          context_type, context_id) \
         VALUES ($1, 'common_cartridge_importer', 'importing', '{}', $2, $3) \
         RETURNING id",
    )
    .bind(user_id)
    .bind(context_type)
    .bind(context_id)
    .fetch_one(pool)
    .await
    .expect("seed context migration")
}

/// Seed a migration issue directly, with the site-admin `error_message`
/// that only `read_errors` holders may see.
async fn seed_issue(pool: &PgPool, migration_id: i64, description: &str, issue_type: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO migration_issues \
         (content_migration_id, description, issue_type, error_message) \
         VALUES ($1, $2, $3, 'secret admin detail') RETURNING id",
    )
    .bind(migration_id)
    .bind(description)
    .bind(issue_type)
    .fetch_one(pool)
    .await
    .expect("seed issue")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id) VALUES ($1, $2) \
         ON CONFLICT DO NOTHING",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account admin")
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str, state: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO groups (course_id, name, workflow_state) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(state)
    .fetch_one(pool)
    .await
    .expect("seed group")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // groups reference courses (ON DELETE SET NULL): drop them while the
    // course ids are still known.
    let _ = sqlx::query(
        "DELETE FROM groups \
         WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    // Context-attached migrations carry no FK to their account/group/user;
    // remove the ones seeded by this run (their migration_issues cascade)
    // while the seeded users can still scope them.
    let _ = sqlx::query(
        "DELETE FROM content_migrations WHERE context_type IS NOT NULL \
         AND user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    // account_users cascade on the account.
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
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
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let res = server
        .get("/courses/1/content_migrations/1/migration_issues")
        .await;
    assert_eq!(res.status_code(), 401);

    // The context fan-out routes require auth as well.
    let res = server
        .get("/accounts/1/content_migrations/1/migration_issues")
        .await;
    assert_eq!(res.status_code(), 401);
    let res = server
        .get("/groups/1/content_migrations/1/migration_issues")
        .await;
    assert_eq!(res.status_code(), 401);
    let res = server
        .get("/users/1/content_migrations/1/migration_issues")
        .await;
    assert_eq!(res.status_code(), 401);
}

// ---------------------------------------------------------------------------
// GET .../migration_issues/{issueId} (Canvas show) + before_action order
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_issues_show_is_scoped_and_canvas_error_order_holds() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ShowCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let migration_a = seed_migration(&pool, course, teacher).await;
    let migration_b = seed_migration(&pool, course, teacher).await;
    let issue_a = seed_issue(&pool, migration_a, "Quiz failed to convert", "warning").await;
    let issue_b = seed_issue(&pool, migration_b, "Other migration", "error").await;

    // Teacher show happy path: the full Canvas migration_issue_json shape.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration_a}/migration_issues/{issue_a}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], issue_a);
    assert_eq!(body["content_migration_id"], migration_a);
    assert_eq!(body["description"], "Quiz failed to convert");
    assert_eq!(body["workflow_state"], "active");
    assert_eq!(body["issue_type"], "warning");
    assert_eq!(body["unresolved"], true);
    assert_eq!(body["fixable_by_web_upload"], false);
    assert_eq!(body["fixable_by_admin"], false);
    assert_eq!(
        body["content_migration_url"],
        format!("/api/v1/courses/{course}/content_migrations/{migration_a}")
    );
    assert!(body["created_at"].as_str().is_some());
    assert!(body["updated_at"].as_str().is_some());
    // The site-admin error message stays hidden from non-admins (read_errors).
    assert!(body.get("error_message").is_none());

    // An issue of another migration is not visible through this migration.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration_a}/migration_issues/{issue_b}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Unknown issue id.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration_a}/migration_issues/999999999"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Canvas before_action order: the migration 404 comes before the
    // permission 403, so even an unauthorized student sees the 404 for a
    // migration that is not in the course.
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/999999999/migration_issues/{issue_a}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // A student lacks the granular manage-content rights on an existing
    // migration (403 after the 404 checks).
    let resp = server
        .get(&format!(
            "/courses/{course}/content_migrations/{migration_a}/migration_issues/{issue_a}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Unknown course id.
    let resp = server
        .get(&format!(
            "/courses/999999999/content_migrations/{migration_a}/migration_issues/{issue_a}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Account context fan-out (index/create/show/update)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_issues_account_context_routes() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let account_admin = seed_user(&pool, &prefix, "AcctAdmin", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "teacher").await;
    let site_admin = seed_user(&pool, &prefix, "SiteAdmin", "admin").await;
    let account = seed_account(&pool, &prefix, "IssuesAccount").await;
    seed_account_admin(&pool, account, account_admin).await;
    let migration = seed_ctx_migration(&pool, "Account", account, account_admin).await;

    // POST creates an issue (Canvas has no create action; 200 by convention).
    let resp = server
        .post(&format!(
            "/accounts/{account}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "teacher"),
        )
        .json(&json!({
            "description": "Announcements could not be imported",
            "issue_type": "todo",
            "fixable_by_admin": true
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    let created_id = created["id"].as_i64().expect("issue id");
    assert_eq!(created["issue_type"], "todo");
    // Canvas builds content_migration_url with the *course* URL helper for
    // every context, so the account id appears in the courses slot.
    assert_eq!(
        created["content_migration_url"],
        format!("/api/v1/courses/{account}/content_migrations/{migration}")
    );

    let second = seed_issue(&pool, migration, "second issue", "error").await;

    // Index is ordered by created_at and paginated (Canvas by_created_at).
    let resp = server
        .get(&format!(
            "/accounts/{account}/content_migrations/{migration}/migration_issues?per_page=1&page=1"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let page1: Value = resp.json();
    assert_eq!(page1.as_array().map(Vec::len), Some(1));
    assert_eq!(page1[0]["id"], created_id);

    let resp = server
        .get(&format!(
            "/accounts/{account}/content_migrations/{migration}/migration_issues?per_page=1&page=2"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let page2: Value = resp.json();
    assert_eq!(page2.as_array().map(Vec::len), Some(1));
    assert_eq!(page2[0]["id"], second);

    // Show.
    let resp = server
        .get(&format!(
            "/accounts/{account}/content_migrations/{migration}/migration_issues/{created_id}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["description"],
        "Announcements could not be imported"
    );

    // Site admins hold read_errors and see the error message; the account
    // admin does not.
    let resp = server
        .get(&format!(
            "/accounts/{account}/content_migrations/{migration}/migration_issues/{second}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(site_admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["error_message"], "secret admin detail");

    // Update: resolve, reactivate, then reject an invalid state (403 + Canvas
    // message).
    let resp = server
        .put(&format!(
            "/accounts/{account}/content_migrations/{migration}/migration_issues/{created_id}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "teacher"),
        )
        .json(&json!({ "workflow_state": "resolved" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let resolved: Value = resp.json();
    assert_eq!(resolved["workflow_state"], "resolved");
    assert_eq!(resolved["unresolved"], false);

    let resp = server
        .put(&format!(
            "/accounts/{account}/content_migrations/{migration}/migration_issues/{created_id}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "teacher"),
        )
        .json(&json!({ "workflow_state": "active" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["workflow_state"], "active");

    let resp = server
        .put(&format!(
            "/accounts/{account}/content_migrations/{migration}/migration_issues/{created_id}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "teacher"),
        )
        .json(&json!({ "workflow_state": "bogus" }))
        .await;
    assert_eq!(resp.status_code(), 403);
    assert_eq!(
        resp.json::<Value>()["message"],
        "Must send a valid workflow state"
    );

    // A teacher without an account_users row is forbidden.
    let resp = server
        .get(&format!(
            "/accounts/{account}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Canvas before_action order: unknown migration 404 beats permission 403.
    let resp = server
        .get(&format!(
            "/accounts/{account}/content_migrations/999999999/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Unknown account id.
    let resp = server
        .get(&format!(
            "/accounts/999999999/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(site_admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // A course-scoped migration is not visible through an account context.
    let course = seed_course(&pool, &prefix, "AccountFanoutCourse").await;
    let course_migration = seed_migration(&pool, course, account_admin).await;
    let resp = server
        .get(&format!(
            "/accounts/{account}/content_migrations/{course_migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(site_admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Group context fan-out (index/create/show/update)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_issues_group_context_routes() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "GroupFanoutCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let group = seed_group(&pool, course, "Project Group", "available").await;
    let other_group = seed_group(&pool, course, "Other Group", "available").await;
    let deleted_group = seed_group(&pool, course, "Deleted Group", "deleted").await;
    let migration = seed_ctx_migration(&pool, "Group", group, teacher).await;
    let other_migration = seed_ctx_migration(&pool, "Group", other_group, teacher).await;

    // The parent course's teacher drives the full round trip.
    let resp = server
        .post(&format!(
            "/groups/{group}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "description": "Files missing", "issue_type": "warning" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    let created_id = created["id"].as_i64().expect("issue id");
    assert_eq!(
        created["content_migration_url"],
        format!("/api/v1/courses/{group}/content_migrations/{migration}")
    );

    let resp = server
        .get(&format!(
            "/groups/{group}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert_eq!(list[0]["id"], created_id);

    let resp = server
        .get(&format!(
            "/groups/{group}/content_migrations/{migration}/migration_issues/{created_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);

    let resp = server
        .put(&format!(
            "/groups/{group}/content_migrations/{migration}/migration_issues/{created_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "workflow_state": "resolved" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "resolved");

    // A plain student of the parent course cannot manage migration issues.
    let resp = server
        .get(&format!(
            "/groups/{group}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // The migration of another group is not visible through this group.
    let resp = server
        .get(&format!(
            "/groups/{group}/content_migrations/{other_migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Canvas resolves groups through Group.active: deleted groups 404.
    let resp = server
        .get(&format!(
            "/groups/{deleted_group}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// User context fan-out (index/create/show/update)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migration_issues_user_context_routes() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "teacher").await;
    let migration = seed_ctx_migration(&pool, "User", owner, owner).await;

    // Canvas User#set_policy grants the manage_course_content_* set to the
    // user themselves: the owner drives the full round trip.
    let resp = server
        .post(&format!(
            "/users/{owner}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "description": "Calendar events skipped", "issue_type": "warning" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    let created_id = created["id"].as_i64().expect("issue id");
    assert_eq!(
        created["content_migration_url"],
        format!("/api/v1/courses/{owner}/content_migrations/{migration}")
    );

    let resp = server
        .get(&format!(
            "/users/{owner}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert_eq!(list[0]["id"], created_id);

    let resp = server
        .get(&format!(
            "/users/{owner}/content_migrations/{migration}/migration_issues/{created_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);

    let resp = server
        .put(&format!(
            "/users/{owner}/content_migrations/{migration}/migration_issues/{created_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "workflow_state": "resolved" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "resolved");

    // Anyone else is forbidden.
    let resp = server
        .get(&format!(
            "/users/{owner}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // The owner's migration is not visible through another user's context
    // (404 before the permission check).
    let resp = server
        .get(&format!(
            "/users/{other}/content_migrations/{migration}/migration_issues"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Unknown user id.
    let resp = server
        .get("/users/999999999/content_migrations/1/migration_issues")
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}
