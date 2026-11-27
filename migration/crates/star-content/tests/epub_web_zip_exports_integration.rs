//! Integration tests for the ePub export and offline web zip export lifecycle
//! (Canvas `epub_exports_controller` and `web_zip_exports_controller`).
//!
//! Route parity for these controllers is complete (parity scoreboard:
//! `epub_exports` 3/3, `web_zip_exports` 2/2); these tests pin the lifecycle
//! semantics the (verb, path) scoreboard cannot see:
//!
//! - both controllers are feature gated: the ePub routes answer 404 until the
//!   domain root account allows `epub_export` (Canvas
//!   `EpubExportsController#check_feature_enabled`), the web zip routes until
//!   the course AND its account enable the offline web export (Canvas
//!   `WebZipExportHelper#course_allow_web_export_download?`);
//! - a created ePub export carries a `progress_id`/`progress_url` (Canvas
//!   `EpubExport#initialize_job_progress`) and is wrapped in the course body
//!   (`course_epub_export_json`);
//! - exports are owner scoped (Canvas `EpubExport.visible_to`): only the
//!   caller's own exports are listed, another user's export is 403, an admin
//!   may read any;
//! - a show for an export of another course is 403; a missing export id is
//!   404 (Canvas answers 401 through `authorized_action(nil, ...)` — the port
//!   answers the more conventional 404);
//! - web zip exports and plain ePub exports share the `epub_exports` table by
//!   single table inheritance (`type = 'WebZipExport'` vs `type IS NULL`) and
//!   each listing filters on its own type;
//! - the two export kinds are mutually exclusive on the root account (Canvas
//!   `EpubExportsController#check_feature_enabled`): once the root account
//!   enables the offline web export, the ePub export routes answer 404 even
//!   with the `epub_export` feature flag on;
//! - every route rejects anonymous callers with 401.
//!
//! Fleet rule: dedicated accounts, courses and users per test, every seeded
//! row (including `feature_flags`) is cleaned up. Skipped cleanly when
//! DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "epexp-{}-{}",
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

    let server =
        TestServer::new(star_content::router().with_state(state)).expect("build TestServer");
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

/// Dedicated per-test account (issue #75: never the shared root account 1).
/// With no parent it is itself a root account, so account-scoped feature
/// flags and offline-web-export settings resolve against it alone.
async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_course_in_account(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course in account")
}

async fn enable_feature(pool: &PgPool, account_id: i64, feature: &str) {
    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state) \
         VALUES ('Account', $1, $2, $2, 'enabled') \
         ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'enabled'",
    )
    .bind(account_id)
    .bind(feature)
    .execute(pool)
    .await
    .expect("enable feature");
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// Seed a web zip export row (Canvas single table inheritance: the row lives
/// in `epub_exports` with `type = 'WebZipExport'`) plus its job progress.
/// There is no REST route to create one — Canvas generates the rows from the
/// course offline-export job — so the tests seed the table directly.
async fn seed_web_zip_export(
    pool: &PgPool,
    course_id: i64,
    user_id: i64,
    workflow_state: &str,
) -> i64 {
    let export_id: i64 = sqlx::query_scalar(
        "INSERT INTO epub_exports (course_id, user_id, workflow_state, type) \
         VALUES ($1, $2, $3, 'WebZipExport') RETURNING id",
    )
    .bind(course_id)
    .bind(user_id)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed web zip export");
    sqlx::query(
        "INSERT INTO progress (tag, workflow_state, completion, context_type, context_id, user_id) \
         VALUES ($1, 'completed', 100, 'EpubExport', $2, $3)",
    )
    .bind(format!("web_zip_export_{export_id}"))
    .bind(export_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed web zip progress");
    export_id
}

/// Seed a plain ePub export row (`type IS NULL`) with its job progress.
async fn seed_plain_epub_export(
    pool: &PgPool,
    course_id: i64,
    user_id: i64,
    workflow_state: &str,
) -> i64 {
    let export_id: i64 = sqlx::query_scalar(
        "INSERT INTO epub_exports (course_id, user_id, workflow_state) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(user_id)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed plain epub export");
    sqlx::query(
        "INSERT INTO progress (tag, workflow_state, completion, context_type, context_id, user_id) \
         VALUES ($1, 'completed', 100, 'EpubExport', $2, $3)",
    )
    .bind(format!("epub_export_{export_id}"))
    .bind(export_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed epub progress");
    export_id
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM progress WHERE context_type = 'EpubExport' \
           AND context_id IN (SELECT id FROM epub_exports WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM epub_exports WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    // feature_flags rows for the dedicated accounts (no FK; delete explicitly),
    // then the accounts themselves (issue #75).
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Account' \
           AND context_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

/// Turn the offline web export on for the account and the course (Canvas
/// `course_allow_web_export_download?` needs both).
async fn enable_offline_web_export(pool: &PgPool, account_id: i64, course_id: i64) {
    sqlx::query(
        "UPDATE accounts SET settings = settings || \
            jsonb_build_object('enable_offline_web_export', jsonb_build_object('value', true)) \
         WHERE id = $1",
    )
    .bind(account_id)
    .execute(pool)
    .await
    .expect("enable the offline web export of the account");
    sqlx::query(
        "UPDATE courses SET settings = settings || \
            jsonb_build_object('enable_offline_web_export', jsonb_build_object('value', true)) \
         WHERE id = $1",
    )
    .bind(course_id)
    .execute(pool)
    .await
    .expect("enable the offline web export of the course");
}

// ---------------------------------------------------------------------------
// ePub export lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_epub_export_lifecycle_is_feature_gated_and_owner_scoped() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    let course = seed_course_in_account(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Canvas `EpubExportsController#check_feature_enabled`: without the
    // `epub_export` feature on the domain root account every action of the
    // controller is a 404 — create, the course-less index and the show.
    let resp = server
        .post(&format!("/courses/{course}/epub_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get("/epub_exports")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Enable the feature on the DEDICATED root account.
    enable_feature(&pool, account, "epub_export").await;

    // A student may not start an export (Canvas
    // `authorized_action(EpubExport.new(course:), user, :create)`).
    let resp = server
        .post(&format!("/courses/{course}/epub_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The teacher may: 201, the course wrapper and the fresh export.
    let resp = server
        .post(&format!("/courses/{course}/epub_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let first: Value = resp.json();
    assert_eq!(first["id"], course, "{first}");
    assert_eq!(first["name"], format!("{prefix}-Course"));
    let export = &first["epub_export"];
    assert_eq!(export["workflow_state"], "created");
    assert_eq!(export["course_id"], course);
    assert_eq!(export["user_id"], teacher);
    assert_eq!(export["type"], Value::Null, "a plain ePub export");
    let first_export_id = export["id"].as_i64().expect("export id");
    assert_eq!(
        export["progress_url"],
        format!(
            "/api/v1/progress/{}",
            export["progress_id"].as_i64().expect("progress id")
        )
    );

    // A second export for the same course; the newest one wins the index.
    let resp = server
        .post(&format!("/courses/{course}/epub_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let second_export_id = resp.json::<Value>()["epub_export"]["id"]
        .as_i64()
        .expect("second export id");
    assert_ne!(second_export_id, first_export_id);

    // A WebZipExport row of the same course (single table inheritance:
    // `type = 'WebZipExport'`) is the NEWEST row of the teacher — and still
    // must not leak into the ePub listing (Canvas scopes the ePub index on
    // `EpubExport.where(type: nil)`).
    let web_zip_id = seed_web_zip_export(&pool, course, teacher, "exported").await;
    assert!(web_zip_id > second_export_id);

    // The course-less index lists the caller's courses with their LATEST
    // own PLAIN export (Canvas `CourseEpubExportsPresenter`).
    let resp = server
        .get("/epub_exports")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let index: Value = resp.json();
    let courses = index["courses"].as_array().expect("courses");
    assert_eq!(courses.len(), 1, "{index}");
    assert_eq!(courses[0]["id"], course);
    assert_eq!(courses[0]["epub_export"]["id"], second_export_id);

    // The student is enrolled but owns no export: the course entry has no
    // `epub_export` key (Canvas `course_epub_export_json`).
    let resp = server
        .get("/epub_exports")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let index: Value = resp.json();
    let courses = index["courses"].as_array().expect("courses");
    assert_eq!(courses.len(), 1, "{index}");
    assert!(courses[0].get("epub_export").is_none(), "{index}");

    // An admin enrolled nowhere has no domain-root context for the
    // user-scoped index → 404 (the port resolves the account chain from the
    // caller's first enrolled course).
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let resp = server
        .get("/epub_exports")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_epub_export_show_scopes_to_course_and_owner() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    let course = seed_course_in_account(&pool, &prefix, "Course", account).await;
    let other_course = seed_course_in_account(&pool, &prefix, "Other", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, teacher, other_course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    enable_feature(&pool, account, "epub_export").await;

    // The teacher creates an export in each of the two courses.
    let resp = server
        .post(&format!("/courses/{course}/epub_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let export_id = resp.json::<Value>()["epub_export"]["id"]
        .as_i64()
        .expect("export id");
    let resp = server
        .post(&format!("/courses/{other_course}/epub_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let other_export_id = resp.json::<Value>()["epub_export"]["id"]
        .as_i64()
        .expect("other export id");

    // The owner reads it through the course; the body wraps the export.
    let resp = server
        .get(&format!("/courses/{course}/epub_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"], course, "{shown}");
    assert_eq!(shown["epub_export"]["id"], export_id);
    assert_eq!(shown["epub_export"]["course_id"], course);

    // A student who is not the owner is 403 (Canvas
    // `EpubExport.visible_to(user)`).
    let resp = server
        .get(&format!("/courses/{course}/epub_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An admin may read any export.
    let resp = server
        .get(&format!("/courses/{course}/epub_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // The export of ANOTHER course shown through this course is 403 (the
    // port checks `row.course_id != course_id` after the feature gate).
    let resp = server
        .get(&format!("/courses/{course}/epub_exports/{other_export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A missing export id is 404 (Canvas answers 401 through
    // `authorized_action(nil, ...)`; the port answers 404).
    let resp = server
        .get(&format!("/courses/{course}/epub_exports/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A missing course is 404 (Canvas `require_context`).
    let resp = server
        .get(&format!("/courses/999999999/epub_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Web zip (offline) export lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_web_zip_export_lifecycle_is_gated_type_scoped_and_paginated() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    let course = seed_course_in_account(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Canvas `WebZipExportHelper#course_allow_web_export_download?`: without
    // the offline web export on the account AND the course both actions are
    // 404, even for an enrolled user.
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Canvas `allow_web_export_for_course_user?`: a user of another course
    // is refused even with the export enabled.
    enable_offline_web_export(&pool, account, course).await;
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Canvas `EpubExportsController#check_feature_enabled` makes the ePub
    // export and the offline web export MUTUALLY EXCLUSIVE on the root
    // account: with `enable_offline_web_export` set, the ePub routes are 404
    // even though the `epub_export` feature flag is on.
    enable_feature(&pool, account, "epub_export").await;
    let resp = server
        .get("/epub_exports")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // With the feature on, the empty lists answer 200 for enrolled users
    // and admins.
    for (user_id, role) in [(teacher, "teacher"), (admin, "admin")] {
        let resp = server
            .get(&format!("/courses/{course}/web_zip_exports"))
            .add_header(axum::http::header::AUTHORIZATION, auth(user_id, role))
            .await;
        assert_eq!(resp.status_code(), 200, "{role}: {}", resp.text());
        assert!(
            resp.json::<Value>().as_array().expect("array").is_empty(),
            "{role} starts with an empty list"
        );
    }

    // Two web zip exports of the teacher (newest first in the index) and one
    // plain ePub export that must NOT leak into the web zip list.
    let older = seed_web_zip_export(&pool, course, teacher, "exported").await;
    let newer = seed_web_zip_export(&pool, course, teacher, "generated").await;
    let plain = seed_plain_epub_export(&pool, course, teacher, "exported").await;

    // The index lists only the caller's OWN WebZipExport rows, newest first.
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 2, "{list}");
    assert_eq!(items[0]["id"], newer);
    assert_eq!(items[1]["id"], older);
    assert_eq!(items[0]["type"], "WebZipExport");
    assert_eq!(items[0]["workflow_state"], "generated");
    assert_eq!(items[0]["course_id"], course);
    assert_eq!(items[0]["user_id"], teacher);
    assert_eq!(
        items[0]["progress_url"],
        format!(
            "/api/v1/progress/{}",
            items[0]["progress_id"].as_i64().expect("progress id")
        )
    );

    // Pagination: per_page=1 keeps only the newest row.
    let resp = server
        .get(&format!(
            "/courses/{course}/web_zip_exports?per_page=1&page=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let items = resp.json::<Value>();
    let items = items.as_array().expect("array");
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["id"], newer);

    // Show serves one export with its progress.
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports/{older}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"], older, "{shown}");
    assert_eq!(shown["workflow_state"], "exported");
    assert_eq!(shown["type"], "WebZipExport");

    // An enrolled user who does not own the export is 403 (Canvas
    // `EpubExport.visible_to(user)`); the admin may read it.
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports/{older}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports/{older}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // A missing export id is 404 (Canvas answers 401 through
    // `authorized_action(nil, ...)`; the port answers 404).
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Single table inheritance: the plain ePub export never shows up in the
    // web zip routes (Canvas scopes the web zip listing on
    // `web_zip_exports`, i.e. `type = 'WebZipExport'`).
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports/{plain}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Anonymous probes
// ---------------------------------------------------------------------------

/// Every route of the two controllers requires a token: 401 before any
/// authorization check (sentinel ids, never account/course 1).
#[tokio::test]
async fn epub_and_web_zip_routes_reject_anonymous_callers() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.get("/epub_exports").await;
    assert_eq!(resp.status_code(), 401, "anonymous GET /epub_exports");
    let resp = server.post("/courses/999999999/epub_exports").await;
    assert_eq!(resp.status_code(), 401, "anonymous POST epub create");
    let resp = server
        .get("/courses/999999999/epub_exports/999999999")
        .await;
    assert_eq!(resp.status_code(), 401, "anonymous GET epub show");
    let resp = server.get("/courses/999999999/web_zip_exports").await;
    assert_eq!(resp.status_code(), 401, "anonymous GET web zip index");
    let resp = server
        .get("/courses/999999999/web_zip_exports/999999999")
        .await;
    assert_eq!(resp.status_code(), 401, "anonymous GET web zip show");
}
