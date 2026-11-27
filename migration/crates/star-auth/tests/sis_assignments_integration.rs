//! Integration tests for the Canvas SIS grade-export feed (issue #92,
//! spec `specs/p5-rest-sis.md` routes 10–11):
//!
//! - `GET /api/sis/courses/{courseId}/assignments` (SisApiController#sis_assignments)
//! - `GET /api/sis/accounts/{accountId}/assignments` (same action, account
//!   context — the account plus every sub-account, optionally windowed by
//!   `starts_before`/`ends_after`)
//!
//! Pinned behavior (against `canvas-src/app/controllers/sis_api_controller.rb`):
//! - the feed lists the **published** assignments with `post_to_sis` enabled,
//!   as a bare Canvas-shaped array;
//! - each entry carries the SIS ids (`sis_assignment_id` =
//!   `assignments.sis_source_id`, `integration_id`, `integration_data`,
//!   `include_in_final_grade`), the assignment group and the course sections
//!   (`Api::V1::SisAssignment#sis_assignment_json`);
//! - `require_view_all_grades` first: a course teacher/TA passes the course
//!   variant, the context's account admins pass the account variant, a
//!   student is a 403;
//! - `require_grade_export` answers
//!   `400 {"code": "not_enabled", "error": GRADE_EXPORT_NOT_ENABLED_ERROR}`;
//! - `require_published_course` answers
//!   `400 {"code": "unpublished_course", ...}` (star courses are published in
//!   `available`, not `published`);
//! - `include[]=student_overrides` adds `user_overrides` with the students'
//!   `sis_user_id`.
//!
//! Conventions: skipped cleanly without DATABASE_URL, unique per-run prefix,
//! dedicated accounts (never account 1), full teardown.

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
        "sisfeed-{}-{}-{}",
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

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    // The /api/sis routes carry their full paths (Canvas serves them outside
    // /api/v1), so the root router is mounted at the test root.
    let server = TestServer::new(star_auth::small_controllers_root_routes().with_state(state))
        .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(
    pool: &PgPool,
    prefix: &str,
    name: &str,
    role: &str,
    sis_id: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role, sis_source_id) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .bind(sis_id)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// A root account. `export` seeds the account-settings approximation of
/// Canvas' `post_grades` feature flag (a `sis_integration_id`).
async fn seed_root_account(pool: &PgPool, prefix: &str, name: &str, export: bool) -> i64 {
    let settings = if export {
        json!({ "sis_integration_id": format!("{prefix}-integration") })
    } else {
        json!({})
    };
    sqlx::query_scalar("INSERT INTO accounts (name, settings) VALUES ($1, $2::jsonb) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(settings.to_string())
        .fetch_one(pool)
        .await
        .expect("seed root account")
}

async fn seed_sub_account(pool: &PgPool, prefix: &str, name: &str, parent_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .expect("seed sub account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id) VALUES ($1, $2)")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
}

async fn seed_course(
    pool: &PgPool,
    prefix: &str,
    name: &str,
    account_id: i64,
    workflow_state: &str,
    start_at: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id, workflow_state, start_at) \
         VALUES ($1, $2, $3, $4, $5::timestamptz) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .bind(workflow_state)
    .bind(start_at)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

#[allow(clippy::too_many_arguments)]
async fn seed_assignment(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    workflow_state: &str,
    post_to_sis: bool,
    sis_source_id: Option<&str>,
    integration_id: Option<&str>,
    integration_data: Option<&str>,
    group_id: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments \
             (course_id, title, points_possible, workflow_state, post_to_sis, sis_source_id, \
              integration_id, integration_data, submission_types, assignment_group_id) \
         VALUES ($1, $2, 10, $3, $4, $5, $6, $7, 'online_upload,online_text_entry', $8) \
         RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(workflow_state)
    .bind(post_to_sis)
    .bind(sis_source_id)
    .bind(integration_id)
    .bind(integration_data)
    .bind(group_id)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str, sis_source_id: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignment_groups (course_id, name, group_weight, sis_source_id) \
         VALUES ($1, $2, 20, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(sis_source_id)
    .fetch_one(pool)
    .await
    .expect("seed assignment group")
}

async fn seed_section(
    pool: &PgPool,
    course_id: i64,
    name: &str,
    sis_source_id: &str,
    integration_id: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO course_sections (course_id, name, sis_source_id, integration_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(sis_source_id)
    .bind(integration_id)
    .fetch_one(pool)
    .await
    .expect("seed section")
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

async fn seed_student_override(pool: &PgPool, assignment_id: i64, student_id: i64) {
    sqlx::query(
        "INSERT INTO assignment_overrides (assignment_id, title, student_ids) \
         VALUES ($1, $2, ARRAY[$3::bigint])",
    )
    .bind(assignment_id)
    .bind("extra time")
    .bind(student_id)
    .execute(pool)
    .await
    .expect("seed override");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
    for statement in [
        "DELETE FROM assignment_overrides WHERE assignment_id IN \
           (SELECT id FROM assignments WHERE course_id IN \
              (SELECT id FROM courses WHERE account_id IN \
                 (SELECT id FROM accounts WHERE name LIKE $1)))",
        "DELETE FROM assignments WHERE course_id IN \
           (SELECT id FROM courses WHERE account_id IN \
              (SELECT id FROM accounts WHERE name LIKE $1))",
        "DELETE FROM assignment_groups WHERE course_id IN \
           (SELECT id FROM courses WHERE account_id IN \
              (SELECT id FROM accounts WHERE name LIKE $1))",
        "DELETE FROM enrollments WHERE course_id IN \
           (SELECT id FROM courses WHERE account_id IN \
              (SELECT id FROM accounts WHERE name LIKE $1))",
        "DELETE FROM course_sections WHERE course_id IN \
           (SELECT id FROM courses WHERE account_id IN \
              (SELECT id FROM accounts WHERE name LIKE $1))",
        "DELETE FROM courses WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
        "DELETE FROM account_users WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
        "DELETE FROM accounts WHERE name LIKE $1",
        "DELETE FROM users WHERE email LIKE $1",
    ] {
        let _ = sqlx::query(statement).bind(&pattern).execute(pool).await;
    }
}

#[tokio::test]
async fn the_feed_lists_published_post_to_sis_assignments_with_their_sis_ids() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher", None).await;
    let student = seed_user(&pool, &prefix, "Student", "student", None).await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    let course = seed_course(&pool, &prefix, "Course", account, "available", None).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let group = seed_group(&pool, course, "Group", "group-sis-1").await;
    let section = seed_section(&pool, course, "Section A", "section-sis-1", "section-int-1").await;
    let listed = seed_assignment(
        &pool,
        course,
        "Exported essay",
        "published",
        true,
        Some("assign-sis-1"),
        Some("assign-int-1"),
        Some(r#"{"bulk_id": "42"}"#),
        Some(group),
    )
    .await;
    seed_assignment(
        &pool,
        course,
        "Not flagged",
        "published",
        false,
        None,
        None,
        None,
        None,
    )
    .await;
    seed_assignment(
        &pool,
        course,
        "Unpublished flag",
        "unpublished",
        true,
        None,
        None,
        None,
        None,
    )
    .await;

    let resp = server
        .get(&format!("/api/sis/courses/{course}/assignments"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("Canvas renders a bare array");
    assert_eq!(items.len(), 1, "body: {body}");
    let item = &items[0];
    assert_eq!(item["id"], listed);
    assert_eq!(item["course_id"], course);
    assert_eq!(item["name"], "Exported essay");
    // The SIS ids (Canvas `alias_attribute :sis_assignment_id, :sis_source_id`).
    assert_eq!(item["sis_assignment_id"], "assign-sis-1");
    assert_eq!(item["integration_id"], "assign-int-1");
    assert_eq!(item["integration_data"]["bulk_id"], "42");
    assert_eq!(item["include_in_final_grade"], true);
    assert_eq!(
        item["submission_types"],
        json!(["online_upload", "online_text_entry"])
    );
    assert_eq!(item["assignment_group"]["id"], group);
    assert_eq!(item["assignment_group"]["name"], "Group");
    assert_eq!(item["assignment_group"]["sis_source_id"], "group-sis-1");
    assert_eq!(item["assignment_group"]["group_weight"], 20.0);
    assert_eq!(item["sections"].as_array().expect("sections").len(), 1);
    assert_eq!(item["sections"][0]["id"], section);
    assert_eq!(item["sections"][0]["sis_id"], "section-sis-1");
    assert_eq!(item["sections"][0]["integration_id"], "section-int-1");
    assert_eq!(item["sections"][0]["origin_course"]["id"], course);

    // `require_view_all_grades` — a student is a 403.
    let resp = server
        .get(&format!("/api/sis/courses/{course}/assignments"))
        .add_header("Authorization", auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_gates_and_student_overrides_match_canvas() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher", None).await;
    let student = seed_user(&pool, &prefix, "Student", "student", Some("student-sis-1")).await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    let plain = seed_root_account(&pool, &prefix, "Plain", false).await;
    let course = seed_course(&pool, &prefix, "Course", account, "available", None).await;
    let unpublished = seed_course(&pool, &prefix, "Hidden", account, "unpublished", None).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, teacher, unpublished, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(
        &pool,
        course,
        "Exported quiz",
        "published",
        true,
        None,
        None,
        None,
        None,
    )
    .await;
    seed_student_override(&pool, assignment, student).await;

    // `require_view_all_grades` runs first (Canvas before_action order): a
    // caller who cannot view the grades is a 403 even when the export is off.
    let resp = server
        .get(&format!("/api/sis/courses/{course}/assignments"))
        .add_header("Authorization", auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // `require_grade_export` on the plain account (the teacher is enrolled
    // there so the view gate passes): 400 GRADE_EXPORT_NOT_ENABLED_ERROR.
    let plain_course = seed_course(&pool, &prefix, "PlainCourse", plain, "available", None).await;
    seed_enrollment(&pool, teacher, plain_course, "teacher").await;
    let resp = server
        .get(&format!("/api/sis/courses/{plain_course}/assignments"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["code"], "not_enabled");
    assert_eq!(
        body["error"],
        "A SIS integration is not configured and the bulk SIS Grade Export feature is not enabled"
    );

    // `require_published_course`: the unpublished course on the exporting
    // account answers 400 COURSE_NOT_PUBLISHED_ERROR.
    let resp = server
        .get(&format!("/api/sis/courses/{unpublished}/assignments"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["code"], "unpublished_course");
    assert_eq!(
        body["error"],
        "Grade data is not available for non-published courses"
    );

    // An unknown course is a 404 (Canvas api_find).
    let resp = server
        .get("/api/sis/courses/999999999/assignments")
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // An anonymous caller is a 401.
    let resp = server
        .get(&format!("/api/sis/courses/{course}/assignments"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // `include[]=student_overrides` adds the ad-hoc overrides with the
    // students' `sis_user_id`.
    let resp = server
        .get(&format!(
            "/api/sis/courses/{course}/assignments?include%5B%5D=student_overrides"
        ))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("bare array");
    assert_eq!(items.len(), 1, "body: {body}");
    let overrides = items[0]["user_overrides"].as_array().expect("overrides");
    assert_eq!(overrides.len(), 1, "body: {body}");
    assert_eq!(overrides[0]["students"][0]["user_id"], student);
    assert_eq!(overrides[0]["students"][0]["sis_user_id"], "student-sis-1");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_account_variant_walks_sub_accounts_and_windows_courses() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher", None).await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "teacher", None).await;
    let root = seed_root_account(&pool, &prefix, "Root", true).await;
    let sub = seed_sub_account(&pool, &prefix, "Sub", root).await;
    seed_account_admin(&pool, root, admin).await;

    // The sub-account's course is inside the root account's feed.
    let sub_course = seed_course(&pool, &prefix, "SubCourse", sub, "available", None).await;
    let in_window = seed_assignment(
        &pool,
        sub_course,
        "Sub assignment",
        "published",
        true,
        Some("sub-sis-1"),
        None,
        None,
        None,
    )
    .await;

    // A future-dated course drops out under the `starts_before` window
    // (Canvas `courses.start_at < starts_before`).
    let future_course = seed_course(
        &pool,
        &prefix,
        "FutureCourse",
        sub,
        "available",
        Some("2030-01-01T00:00:00Z"),
    )
    .await;
    seed_assignment(
        &pool,
        future_course,
        "Future assignment",
        "published",
        true,
        None,
        None,
        None,
        None,
    )
    .await;

    let resp = server
        .get(&format!("/api/sis/accounts/{root}/assignments"))
        .add_header("Authorization", auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("bare array");
    // Without the window both courses feed the listing.
    let ids: Vec<i64> = items
        .iter()
        .map(|item| item["id"].as_i64().expect("id"))
        .collect();
    assert!(ids.contains(&in_window), "body: {body}");

    // The window excludes the future-dated course
    // (`courses.start_at < starts_before` in Canvas).
    let resp = server
        .get(&format!(
            "/api/sis/accounts/{root}/assignments?starts_before=2025-01-01T00:00:00Z"
        ))
        .add_header("Authorization", auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("bare array");
    assert_eq!(items.len(), 1, "body: {body}");
    assert_eq!(items[0]["id"], in_window);
    assert_eq!(items[0]["course_id"], sub_course);
    assert_eq!(items[0]["sis_assignment_id"], "sub-sis-1");

    // A stranger is not the account's admin: 403 (view_all_grades).
    let resp = server
        .get(&format!("/api/sis/accounts/{root}/assignments"))
        .add_header("Authorization", auth(outsider, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An unknown account is a 404.
    let resp = server
        .get("/api/sis/accounts/999999999/assignments")
        .add_header("Authorization", auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
