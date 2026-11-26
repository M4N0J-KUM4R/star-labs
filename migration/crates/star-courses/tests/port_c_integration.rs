//! Integration tests for the port-C gap routes (star-courses).
//!
//! Covers the ai_paces_gaps_v2 batch (Microsoft Teams sync groups, section
//! roster/crosslisting, account-scoped course batch update + show) and a
//! shape sweep of the misc_gaps_v2 revival (planner, permissions help,
//! app center, immersive reader, canvadoc sessions, live assessments,
//! /api/sis disable_post_to_sis).
//!
//! The backup branch had no dedicated test file for these routes (its
//! tests directory only carried blueprint/courses/courses_deep/tabs), so
//! these tests are written fresh against the same conventions as
//! courses_integration.rs:
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP, returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-courses
//!   --test port_c_integration
//! - Unique email/course_code prefix per run + full teardown so it is
//!   safe to run repeatedly against a shared/dev database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness (mirrors courses_integration.rs)
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "portc-{}-{}",
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

    let server = TestServer::new(port_c_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same assembly as star-http, restricted to star-courses: the /api/v1
/// router (mounted at the test root, exactly like courses_integration.rs)
/// plus the root-mounted gap routes (canvadoc_session, /api/sis family).
fn port_c_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_courses::router())
        .merge(star_courses::root_routes())
        .with_state(state)
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
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO accounts (name)
        VALUES ($1)
        RETURNING id
        "#,
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_section(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO course_sections (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed section")
}

async fn seed_enrollment(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    role: &str,
    section_id: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, course_section_id) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .bind(section_id)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Issue a request with a bearer token; status + JSON body in one call.
/// The status is axum-test's `StatusCode` (compares with u16 literals,
/// exactly like the other test files in this crate).
async fn req(
    server: &TestServer,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (axum_test::http::StatusCode, Value) {
    let mut request = match method {
        "GET" => server.get(path),
        "POST" => server.post(path),
        "PUT" => server.put(path),
        "DELETE" => server.delete(path),
        _ => panic!("unsupported method {method}"),
    };
    if let Some(token) = token {
        request = request.add_header(axum::http::header::AUTHORIZATION, token);
    }
    let resp = match body {
        Some(payload) => {
            request
                .add_header(axum::http::header::CONTENT_TYPE, "application/json")
                .json(&payload)
                .await
        }
        None => request.await,
    };
    let status = resp.status_code();
    let text = resp.text();
    let value = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, value)
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // Live assessments have no FK to courses — remove results first, then
    // the assessments, then the sync groups and the progress rows.
    let _ = sqlx::query(
        "DELETE FROM live_assessments_results WHERE assessment_id IN ( \
             SELECT id FROM live_assessments_assessments \
             WHERE context_type = 'Course' AND context_id IN \
                 (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM live_assessments_assessments \
         WHERE context_type = 'Course' AND context_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM microsoft_sync_groups WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM progresses WHERE tag IN ('course_batch_update', \
             'differentiation_tag_conversion') AND user_id IN \
             (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
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
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Microsoft Teams sync groups
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ms_group_lifecycle_and_gates() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "ms-teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "ms-student", "student").await;
    let course = seed_course(&pool, &prefix, "ms-course").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;
    seed_enrollment(&pool, student, course, "student", None).await;

    // Unknown course → 404 (before the 403 gate).
    let (status, _) = req(
        &server,
        "POST",
        "/courses/999999999/microsoft_sync/group",
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 404, "unknown course must 404");

    // Non-teacher → 403.
    let (status, _) = req(
        &server,
        "POST",
        &format!("/courses/{course}/microsoft_sync/group"),
        Some(&auth(student, "student")),
        None,
    )
    .await;
    assert_eq!(status, 403, "student must not create a sync group");

    // Create → pending group; second create returns the same row.
    let path = format!("/courses/{course}/microsoft_sync/group");
    let (status, body) = req(
        &server,
        "POST",
        &path,
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200, "create should succeed");
    assert_eq!(body["workflow_state"], "pending");
    assert_eq!(body["course_id"], course);
    let group_id = body["id"].as_i64().expect("group id");

    let (status, again) = req(
        &server,
        "POST",
        &path,
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200, "idempotent create should succeed");
    assert_eq!(again["id"].as_i64(), Some(group_id), "same group returned");

    // Show as teacher.
    let (status, shown) = req(&server, "GET", &path, Some(&auth(teacher, "teacher")), None).await;
    assert_eq!(status, 200);
    assert_eq!(shown["id"].as_i64(), Some(group_id));

    // Schedule sync → running + last_synced_at.
    let (status, synced) = req(
        &server,
        "POST",
        &format!("/courses/{course}/microsoft_sync/schedule_sync"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200, "schedule_sync should succeed");
    assert_eq!(synced["workflow_state"], "running");
    assert!(
        synced["last_synced_at"].is_string(),
        "last_synced_at must be set after a scheduled sync"
    );

    // Destroy → 204, then 404 on repeat and on show.
    let (status, _) = req(
        &server,
        "DELETE",
        &path,
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 204);
    let (status, _) = req(
        &server,
        "DELETE",
        &path,
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = req(&server, "GET", &path, Some(&auth(teacher, "teacher")), None).await;
    assert_eq!(status, 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Section roster + crosslisting
// ---------------------------------------------------------------------------

#[tokio::test]
async fn section_users_roster_and_crosslist() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "sec-teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "sec-student", "student").await;
    let stranger = seed_user(&pool, &prefix, "sec-stranger", "student").await;
    let course = seed_course(&pool, &prefix, "sec-course").await;
    let target = seed_course(&pool, &prefix, "sec-target").await;
    let section = seed_section(&pool, course, "Port C Section").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;
    seed_enrollment(&pool, teacher, target, "teacher", None).await;
    seed_enrollment(&pool, student, course, "student", Some(section)).await;
    // The stranger stays UNENROLLED anywhere: Canvas sections#users serves
    // course participants (read_roster), so a genuinely outside caller is
    // the one that must draw the 403.

    // Section roster: one row for the section-enrolled student.
    let (status, roster) = req(
        &server,
        "GET",
        &format!("/sections/{section}/users"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200, "section roster should load");
    assert_eq!(roster["total"], 1, "only the section-enrolled student");
    assert_eq!(roster["data"][0]["id"], student);

    // Non-participant → 403; unknown section → 404.
    let other_course = seed_course(&pool, &prefix, "sec-other").await;
    let (status, _) = req(
        &server,
        "GET",
        &format!("/sections/{section}/users"),
        Some(&auth(stranger, "student")),
        None,
    )
    .await;
    assert_eq!(status, 403, "stranger must not read the roster");
    let (status, _) = req(
        &server,
        "GET",
        "/sections/999999999/users",
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 404, "unknown section must 404");
    let _ = other_course;

    // Crosslist: teacher of both → section moves, original recorded.
    let (status, moved) = req(
        &server,
        "POST",
        &format!("/sections/{section}/crosslist/{target}"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200, "crosslist should succeed");
    assert_eq!(moved["course_id"], target);
    assert_eq!(moved["original_course_id"], course);

    // Uncrosslist restores the birth course and clears original_course_id.
    let (status, back) = req(
        &server,
        "DELETE",
        &format!("/sections/{section}/crosslist"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200, "uncrosslist should succeed");
    assert_eq!(back["course_id"], course);
    assert!(
        back.get("original_course_id").is_none(),
        "original_course_id must be cleared: {}",
        back
    );

    // Uncrosslisting a non-crosslisted section → 400 with the Canvas error.
    let (status, err) = req(
        &server,
        "DELETE",
        &format!("/sections/{section}/crosslist"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(
        err["errors"][0]["message"],
        "can't uncrosslist a non-crosslisted section"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Account batch update + account-scoped course show
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_batch_update_and_scoped_show() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "acct-admin", "admin").await;
    let student = seed_user(&pool, &prefix, "acct-student", "student").await;
    let account = seed_account(&pool, &prefix, "acct-account").await;
    let other_account = seed_account(&pool, &prefix, "acct-other").await;
    let course: i64 = sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind("Batch Course")
    .bind(format!("{prefix}-batch"))
    .bind(account)
    .fetch_one(&pool)
    .await
    .expect("seed account course");
    seed_enrollment(&pool, student, course, "student", None).await;

    // Non-admin → 403; unknown account → 404.
    let (status, _) = req(
        &server,
        "PUT",
        &format!("/accounts/{account}/courses"),
        Some(&auth(student, "student")),
        Some(json!({ "courses": [{ "id": course, "name": "Nope" }] })),
    )
    .await;
    assert_eq!(status, 403, "only admins batch-update");
    let (status, _) = req(
        &server,
        "PUT",
        "/accounts/999999999/courses",
        Some(&auth(admin, "admin")),
        None,
    )
    .await;
    assert_eq!(status, 404, "unknown account must 404");

    // Admin batch update → completed progress + persisted rename.
    let (status, progress) = req(
        &server,
        "PUT",
        &format!("/accounts/{account}/courses"),
        Some(&auth(admin, "admin")),
        Some(json!({ "courses": [{ "id": course, "name": "Renamed Course" }] })),
    )
    .await;
    assert_eq!(status, 200, "batch update should succeed");
    assert_eq!(progress["progress"]["workflow_state"], "completed");
    assert_eq!(progress["progress"]["completion"], 100);
    let renamed: String = sqlx::query_scalar("SELECT name FROM courses WHERE id = $1")
        .bind(course)
        .fetch_one(&pool)
        .await
        .expect("reload course");
    assert_eq!(renamed, "Renamed Course");

    // Account-scoped show: enrolled student reads it through the account.
    let (status, shown) = req(
        &server,
        "GET",
        &format!("/accounts/{account}/courses/{course}"),
        Some(&auth(student, "student")),
        None,
    )
    .await;
    assert_eq!(status, 200, "enrolled student can read the scoped course");
    assert_eq!(shown["id"], course);
    assert_eq!(shown["name"], "Renamed Course");

    // Wrong account → 404.
    let (status, _) = req(
        &server,
        "GET",
        &format!("/accounts/{other_account}/courses/{course}"),
        Some(&auth(student, "student")),
        None,
    )
    .await;
    assert_eq!(status, 404, "course in another account must 404");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// misc_gaps_v2 shape sweep
// ---------------------------------------------------------------------------

#[tokio::test]
async fn misc_gaps_v2_shapes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "misc-teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "misc-student", "student").await;
    let course = seed_course(&pool, &prefix, "misc-course").await;
    seed_enrollment(&pool, teacher, course, "teacher", None).await;
    seed_enrollment(&pool, student, course, "student", None).await;

    // permissions groups + help (no auth needed — static handlers).
    let (status, groups) = req(&server, "GET", "/permissions/groups", None, None).await;
    assert_eq!(status, 200);
    assert!(groups.as_array().expect("groups array").len() >= 2);
    let (status, help) = req(
        &server,
        "GET",
        "/permissions/course/manage_courses/help",
        None,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(help["details"][0]["permission"], "manage_courses");
    let (status, _) = req(
        &server,
        "GET",
        "/permissions/course/not_a_perm/help",
        None,
        None,
    )
    .await;
    assert_eq!(status, 404);

    // Immersive reader + app center + microfrontends disabled shapes.
    let (status, _) = req(&server, "GET", "/immersive_reader/authenticate", None, None).await;
    assert_eq!(status, 400, "immersive reader not configured");
    let (status, apps) = req(
        &server,
        "GET",
        &format!("/courses/{course}/app_center/apps"),
        None,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(apps["objects"].as_array().map(Vec::len), Some(0));
    let (status, tag) = req(
        &server,
        "GET",
        "/microfrontends/release_tag_override",
        None,
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert!(tag["release_tag"].is_null());
    let (status, _) = req(
        &server,
        "DELETE",
        "/microfrontends/release_tag_override",
        None,
        None,
    )
    .await;
    assert_eq!(status, 204);

    // Root-mounted canvadoc sessions: disabled service → 401.
    let (status, _) = req(&server, "GET", "/canvadoc_session", None, None).await;
    assert_eq!(status, 401, "canvadoc GET must refuse");
    let (status, _) = req(&server, "POST", "/canvadoc_session", None, None).await;
    assert_eq!(status, 401, "canvadoc POST must refuse");

    // Planner stream: self read + stranger 403 (GET-only surface).
    let (status, items) = req(
        &server,
        "GET",
        "/planner/items",
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(items.as_array().map(Vec::len), Some(0));
    let (status, _) = req(
        &server,
        "GET",
        &format!("/users/{teacher}/planner/items"),
        Some(&auth(student, "student")),
        None,
    )
    .await;
    assert_eq!(status, 403, "another user's planner stream is private");

    // Login session token: mints the caller's own session.
    let (status, token) = req(
        &server,
        "GET",
        "/login/session_token",
        Some(&auth(student, "student")),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        token["session_token"]
            .as_str()
            .is_some_and(|t| !t.is_empty()),
        "session_token must be a non-empty string"
    );

    // Live assessments: create → index → result create → result index.
    let (status, created) = req(
        &server,
        "POST",
        &format!("/courses/{course}/live_assessments"),
        Some(&auth(teacher, "teacher")),
        Some(json!({ "name": "Port C Assessment" })),
    )
    .await;
    assert_eq!(status, 201, "live assessment create");
    let assessment = created["id"].as_i64().expect("assessment id");
    let (status, list) = req(
        &server,
        "GET",
        &format!("/courses/{course}/live_assessments"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(list[0]["name"], "Port C Assessment");
    let (status, result) = req(
        &server,
        "POST",
        &format!("/courses/{course}/live_assessments/{assessment}/results"),
        Some(&auth(teacher, "teacher")),
        Some(json!({ "user_id": student, "score": 5.0 })),
    )
    .await;
    assert_eq!(status, 201, "result create");
    assert_eq!(result["user_id"], student);
    let (status, results) = req(
        &server,
        "GET",
        &format!("/courses/{course}/live_assessments/{assessment}/results"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(results[0]["user_id"], student);
    assert_eq!(results[0]["passed"], true);

    // Root-mounted /api/sis disable_post_to_sis — Canvas serves a bare
    // `head :no_content` to a teacher (manage_assignments_edit), a bare
    // `head :unauthorized` (401) to anyone else and a 400
    // `{"code": "not_found", ...}` for an unknown grading period.
    let (status, cleared) = req(
        &server,
        "PUT",
        &format!("/api/sis/courses/{course}/disable_post_to_sis"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 204, "disable_post_to_sis should succeed");
    assert_eq!(cleared, Value::Null, "204 has no body");
    let (status, not_found) = req(
        &server,
        "PUT",
        &format!("/api/sis/courses/{course}/disable_post_to_sis?grading_period_id=99999999"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 400, "unknown grading period is a 400");
    assert_eq!(not_found["code"], "not_found");
    assert_eq!(not_found["error"], "The Grading Period cannot be found");
    let (status, _) = req(
        &server,
        "PUT",
        &format!("/api/sis/courses/{course}/disable_post_to_sis"),
        None,
        None,
    )
    .await;
    assert_eq!(status, 401, "requires authentication");

    // Course tag conversion: 204 start, status shows completed.
    let (status, _) = req(
        &server,
        "PUT",
        &format!("/courses/{course}/convert_tag_overrides"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 204, "conversion start is a 204");
    let (status, job) = req(
        &server,
        "GET",
        &format!("/courses/{course}/convert_tag_overrides/status"),
        Some(&auth(teacher, "teacher")),
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(job["workflow_state"], "completed");

    cleanup(&pool, &prefix).await;
}
