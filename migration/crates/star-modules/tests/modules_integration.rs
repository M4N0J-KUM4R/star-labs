//! Integration tests for star-modules.
//!
//! Exercises the real modules router (star_modules::router()) through
//! axum-test against a real Postgres, pinning ACTUAL behavior:
//! - module CRUD via the router (create/list/get/update/soft-delete),
//! - module items add/list/delete, including the (module_id, item_type,
//!   item_id) upsert backed by the unique index from migration 0029,
//! - read vs. write authorization shapes (401 / 403 / 404) on course
//!   modules and on the nested module/item reads,
//! - one full lifecycle test (module -> items -> soft delete).
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
//!   cargo test -p star-modules
//! - Uses a unique per-run prefix + full teardown so it is safe to run
//!   repeatedly against a shared/dev database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test AppState and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

/// An id far beyond any seeded row, for 404 probes.
const MISSING_ID: i64 = 999_999_999;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "mod-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the modules router
/// exactly the way star-http assembles it. Returns None when DATABASE_URL
/// is unset so callers can skip cleanly.
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
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Create a user directly in the DB with the given role. Returns user id.
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

/// Create a course directly in the DB. Returns course id.
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

/// Create a module directly in the DB (active). Returns module id.
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

/// Create a module item directly in the DB. Returns item id.
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

/// Enroll a user in a course with the given enrollment role and
/// workflow_state (defaults to 'active' per migration 0010).
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

/// Mint a real bearer token the same way star-auth does at login time.
fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Delete every course/user row created by this run. Modules cascade from
/// their course; module_items cascade from their module; enrollments
/// cascade from both course and user.
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
// POST /courses/{courseId}/modules — create
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_module_returns_201_with_active_state() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "CreateCourse").await;

    let resp = server
        .post(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Week 1", "position": 3 }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "teacher create module; body: {}",
        resp.text()
    );
    let module: Value = resp.json();
    assert!(
        module["id"].as_i64().unwrap_or(0) > 0,
        "module id: {module}"
    );
    assert_eq!(module["course_id"], course);
    assert_eq!(module["name"], "Week 1");
    assert_eq!(module["position"], 3);
    assert_eq!(
        module["workflow_state"], "active",
        "new modules are created active (Canvas parity)"
    );
    assert!(module["created_at"].is_string(), "timestamps: {module}");
    assert!(module["updated_at"].is_string(), "timestamps: {module}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_module_defaults_position_to_zero() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "DefaultPosCourse").await;

    let resp = server
        .post(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "No Position Given" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let module: Value = resp.json();
    assert_eq!(
        module["position"], 0,
        "position must default to 0 when omitted"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_module_allows_teacher_via_enrollment_row() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // DB role is 'student', but the enrollment row says 'teacher' — the
    // write-authz helper accepts the enrollment role.
    let course_teacher = seed_user(&pool, &prefix, "CourseTeacher", "student").await;
    let course = seed_course(&pool, &prefix, "EnrollTeacherCourse").await;
    seed_enrollment(&pool, course_teacher, course, "teacher", "active").await;

    let resp = server
        .post(&format!("/courses/{course}/modules"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(course_teacher, "student"),
        )
        .json(&json!({ "name": "By Enrollment" }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "enrollment role 'teacher' must grant module create; body: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_module_authz_runs_before_course_existence_check() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;

    // Teacher: authz passes (session role), then the course lookup fails.
    let resp = server
        .post(&format!("/courses/{MISSING_ID}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Ghost" }))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "teacher on missing course gets 404; body: {}",
        resp.text()
    );
    assert_eq!(resp.text(), "not found", "error body is plain text");

    // Student: the Forbidden is returned BEFORE the course existence check,
    // so a missing course surfaces as 403 for unauthorized callers.
    let resp = server
        .post(&format!("/courses/{MISSING_ID}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "name": "Ghost" }))
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "student on missing course must hit the authz gate (403), not 404"
    );
    assert_eq!(resp.text(), "forbidden", "error body is plain text");

    // Plain student on a real course is forbidden too.
    let course = seed_course(&pool, &prefix, "AuthzOrderCourse").await;
    let resp = server
        .post(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "name": "Nope" }))
        .await;
    assert_eq!(resp.status_code(), 403, "student create must be 403");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn token_role_claim_is_advisory_db_role_wins() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "ClaimCourse").await;

    // Token claims 'teacher' but the users row says 'student' -> 403.
    let fake_teacher = seed_user(&pool, &prefix, "FakeTeacher", "student").await;
    let resp = server
        .post(&format!("/courses/{course}/modules"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fake_teacher, "teacher"),
        )
        .json(&json!({ "name": "Sneaky" }))
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "the token role claim must never authorize; the DB role is used"
    );

    // Token claims 'student' but the users row says 'teacher' -> allowed.
    let real_teacher = seed_user(&pool, &prefix, "RealTeacher", "teacher").await;
    let resp = server
        .post(&format!("/courses/{course}/modules"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(real_teacher, "student"),
        )
        .json(&json!({ "name": "Humble" }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "DB role 'teacher' authorizes even when the token claims 'student'"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/modules — list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_modules_ordered_by_position_then_id() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "OrderCourse").await;

    // Read access requires an ACTIVE enrollment even for teachers — a
    // session 'teacher' role alone does not grant module list access.
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;

    // Insert out of order; two share position 2 — the tie breaks by id
    // (insertion order: A before C).
    let a = seed_module(&pool, course, "Pos2 A", 2).await;
    let b = seed_module(&pool, course, "Pos1 B", 1).await;
    let c = seed_module(&pool, course, "Pos2 C", 2).await;

    let resp = server
        .get(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let ids: Vec<i64> = list
        .as_array()
        .expect("list is a JSON array")
        .iter()
        .map(|m| m["id"].as_i64().expect("module id"))
        .collect();
    assert_eq!(ids, vec![b, a, c], "ORDER BY position ASC, id ASC");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_modules_excludes_soft_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "DelListCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;
    let doomed = seed_module(&pool, course, "Doomed", 1).await;
    let survivor = seed_module(&pool, course, "Survivor", 2).await;

    let resp = server
        .delete(&format!("/modules/{doomed}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "delete module");

    let resp = server
        .get(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let list: Value = resp.json();
    let ids: Vec<i64> = list
        .as_array()
        .expect("list is a JSON array")
        .iter()
        .map(|m| m["id"].as_i64().expect("module id"))
        .collect();
    assert!(
        !ids.contains(&doomed),
        "soft-deleted module must be excluded from the list: {ids:?}"
    );
    assert!(ids.contains(&survivor), "survivor must remain: {ids:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_modules_pagination_limit_offset() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PageCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher", "active").await;
    let m1 = seed_module(&pool, course, "P1", 1).await;
    let m2 = seed_module(&pool, course, "P2", 2).await;
    let m3 = seed_module(&pool, course, "P3", 3).await;

    let auth_header = auth(teacher, "teacher");

    let resp = server
        .get(&format!("/courses/{course}/modules?limit=2"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let ids: Vec<i64> = ids_of(resp);
    assert_eq!(ids, vec![m1, m2], "limit=2 returns the first two");

    let resp = server
        .get(&format!("/courses/{course}/modules?limit=2&offset=1"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let ids: Vec<i64> = ids_of(resp);
    assert_eq!(ids, vec![m2, m3], "offset=1 skips the first module");

    let resp = server
        .get(&format!("/courses/{course}/modules?offset=50"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header)
        .await;
    assert_eq!(resp.status_code(), 200);
    let ids: Vec<i64> = ids_of(resp);
    assert!(ids.is_empty(), "offset past the end returns an empty list");

    cleanup(&pool, &prefix).await;
}

/// Extract the module ids from a list response.
fn ids_of(resp: axum_test::TestResponse) -> Vec<i64> {
    resp.json::<Value>()
        .as_array()
        .expect("list is a JSON array")
        .iter()
        .map(|m| m["id"].as_i64().expect("module id"))
        .collect()
}

#[tokio::test]
async fn list_modules_read_authz_matrix() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "ReadAuthzCourse").await;
    seed_module(&pool, course, "Visible", 1).await;

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let active = seed_user(&pool, &prefix, "Active", "student").await;
    let invited = seed_user(&pool, &prefix, "Invited", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    seed_enrollment(&pool, active, course, "student", "active").await;
    seed_enrollment(&pool, invited, course, "student", "invited").await;

    // Anonymous -> 401 from the AuthUser extractor, before any handler work.
    let resp = server.get(&format!("/courses/{course}/modules")).await;
    assert_eq!(resp.status_code(), 401, "anonymous list must be 401");
    assert_eq!(resp.text(), "unauthorized");

    // Active enrollment -> 200.
    let resp = server
        .get(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(active, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "active enrollment grants read access"
    );

    // Invited (non-active) enrollment -> 403.
    let resp = server
        .get(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(invited, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "only workflow_state='active' enrollments grant read access"
    );
    assert_eq!(resp.text(), "forbidden");

    // No enrollment -> 403, even with a 'teacher' session-role claim
    // (read access comes from enrollment rows / admin role only).
    let resp = server
        .get(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "no enrollment -> 403");

    // Admin (DB role) reads any course without an enrollment.
    let resp = server
        .get(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "admin reads any course");

    // Quirk: the list handler never checks that the course exists — an
    // admin listing a missing course gets 200 with an empty array.
    let resp = server
        .get(&format!("/courses/{MISSING_ID}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "list does not verify course existence"
    );
    let list: Value = resp.json();
    assert!(
        list.as_array().expect("array").is_empty(),
        "missing course lists empty: {list}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /modules/{id} — detail with items (flattened)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_module_returns_flattened_module_with_items() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "DetailCourse").await;
    let module = seed_module(&pool, course, "Detail Module", 1).await;
    let item_a = seed_module_item(&pool, module, "page", 101, 1).await;
    let item_b = seed_module_item(&pool, module, "assignment", 202, 2).await;

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Anonymous -> 401 from the AuthUser extractor, before any handler work.
    let resp = server.get(&format!("/modules/{module}")).await;
    assert_eq!(resp.status_code(), 401, "anonymous module detail is 401");
    assert_eq!(resp.text(), "unauthorized");

    // Authenticated but not enrolled -> 403 (module found, then authz).
    let resp = server
        .get(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "non-enrolled module detail is 403");
    assert_eq!(resp.text(), "forbidden");

    // Unauthorized callers still get 404 for a missing module: the lookup
    // precedes the authz check.
    let resp = server
        .get(&format!("/modules/{MISSING_ID}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "missing module 404s even for unauthorized callers (lookup precedes authz)"
    );

    // Admin (DB role) reads any course without an enrollment.
    let resp = server
        .get(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "admin reads any module detail");

    // Active enrollment -> 200 with the flattened module and its items.
    let resp = server
        .get(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "enrolled student reads module detail"
    );
    let detail: Value = resp.json();

    // ModuleWithItems flattens the module: fields sit at the top level.
    assert_eq!(detail["id"], module);
    assert_eq!(detail["course_id"], course);
    assert_eq!(detail["name"], "Detail Module");
    assert_eq!(detail["workflow_state"], "active");

    let items = detail["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2, "both seeded items returned: {detail}");
    assert_eq!(items[0]["id"], item_a);
    assert_eq!(items[0]["item_type"], "page");
    assert_eq!(items[1]["id"], item_b);
    assert_eq!(items[1]["item_type"], "assignment");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_module_missing_and_deleted_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Missing404Course").await;
    let module = seed_module(&pool, course, "Soon Gone", 1).await;

    let resp = server
        .get(&format!("/modules/{MISSING_ID}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "missing module 404s");
    assert_eq!(resp.text(), "not found");

    // Soft-deleted modules 404 on detail too.
    let resp = server
        .delete(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204);

    let resp = server
        .get(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "deleted module 404s on GET");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /modules/{id} — partial update
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_module_partial_update_coalesce() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UpdateCourse").await;
    let module = seed_module(&pool, course, "Original", 2).await;
    let auth_header = auth(teacher, "teacher");

    // Name only: position must survive via COALESCE.
    let resp = server
        .put(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .json(&json!({ "name": "Renamed" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["name"], "Renamed");
    assert_eq!(
        updated["position"], 2,
        "position untouched by name-only PUT"
    );

    // Position only: name must survive.
    let resp = server
        .put(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .json(&json!({ "position": 7 }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let updated: Value = resp.json();
    assert_eq!(
        updated["name"], "Renamed",
        "name untouched by position-only PUT"
    );
    assert_eq!(updated["position"], 7);

    // Empty body: everything survives (both fields COALESCEd).
    let resp = server
        .put(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header)
        .json(&json!({}))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "empty PATCH-style body is accepted"
    );
    let updated: Value = resp.json();
    assert_eq!(updated["name"], "Renamed");
    assert_eq!(updated["position"], 7);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_module_authz_matrix() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "UpdateAuthzCourse").await;
    let module = seed_module(&pool, course, "Guarded", 1).await;

    // Anonymous -> 401.
    let resp = server
        .put(&format!("/modules/{module}"))
        .json(&json!({ "name": "Anon" }))
        .await;
    assert_eq!(resp.status_code(), 401, "anonymous update must be 401");

    // Student on an existing module -> 403 (module found, then authz).
    let resp = server
        .put(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "name": "Nope" }))
        .await;
    assert_eq!(resp.status_code(), 403, "student update must be 403");
    assert_eq!(resp.text(), "forbidden");

    // Student on a MISSING module -> 404: the handler resolves the module
    // before the authz check, so existence leaks before authorization.
    let resp = server
        .put(&format!("/modules/{MISSING_ID}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "name": "Nope" }))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "missing module 404s even for unauthorized callers (lookup precedes authz)"
    );

    // Teacher on a missing module -> 404 as well.
    let resp = server
        .put(&format!("/modules/{MISSING_ID}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Ghost" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /modules/{id} — soft delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_module_is_soft_delete_and_idempotent() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "SoftDelCourse").await;
    let module = seed_module(&pool, course, "Doomed Lifecycle", 1).await;
    let item = seed_module_item(&pool, module, "page", 303, 1).await;
    let auth_header = auth(teacher, "teacher");

    // First delete -> 204, empty body.
    let resp = server
        .delete(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "delete module");
    assert_eq!(resp.text(), "", "204 has an empty body");

    // Row survives as workflow_state='deleted' (Canvas parity soft delete).
    let (state,): (String,) = sqlx::query_as("SELECT workflow_state FROM modules WHERE id = $1")
        .bind(module)
        .fetch_one(&pool)
        .await
        .expect("module row must still exist after soft delete");
    assert_eq!(state, "deleted", "soft delete marks the row, not drops it");

    // The item row is NOT removed — soft delete does not cascade.
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM module_items WHERE id = $1 AND module_id = $2")
            .bind(item)
            .bind(module)
            .fetch_one(&pool)
            .await
            .expect("count items");
    assert_eq!(count, 1, "module_items survive a module soft delete");

    // Re-DELETE returns 204 again: the delete lookup has no
    // workflow_state filter, so deleting a deleted module "succeeds".
    let resp = server
        .delete(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header)
        .await;
    assert_eq!(
        resp.status_code(),
        204,
        "DELETE on an already-deleted module is idempotent 204 (pinned quirk)"
    );

    // A deleted module is invisible to GET, PUT and item creation...
    let resp = server
        .get(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .put(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Zombie" }))
        .await;
    assert_eq!(resp.status_code(), 404, "PUT on deleted module 404s");
    let resp = server
        .post(&format!("/modules/{module}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "item_type": "page", "item_id": 1 }))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "item creation on a deleted module 404s before the upsert"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_module_authz_matrix() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "DelAuthzCourse").await;
    let module = seed_module(&pool, course, "Guarded Delete", 1).await;

    // Anonymous -> 401.
    let resp = server.delete(&format!("/modules/{module}")).await;
    assert_eq!(resp.status_code(), 401, "anonymous delete must be 401");

    // Student -> 403 (authz precedes the delete).
    let resp = server
        .delete(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student delete must be 403");
    assert_eq!(resp.text(), "forbidden");

    // Missing module -> 404 for anyone (lookup precedes authz).
    let resp = server
        .delete(&format!("/modules/{MISSING_ID}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .delete(&format!("/modules/{MISSING_ID}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Teacher succeeds and the module survives as a row (see lifecycle test).
    let resp = server
        .delete(&format!("/modules/{module}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /modules/{id}/items — item creation and upsert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_item_invalid_type_422_validation_precedes_authz() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ItemTypeCourse").await;
    let module = seed_module(&pool, course, "Items Home", 1).await;

    // Anonymous -> 401 (extractor runs before the handler).
    let resp = server
        .post(&format!("/modules/{module}/items"))
        .json(&json!({ "item_type": "quiz", "item_id": 1 }))
        .await;
    assert_eq!(resp.status_code(), 401);

    // Only 'page' and 'assignment' pass validation.
    let resp = server
        .post(&format!("/modules/{module}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "item_type": "quiz", "item_id": 1 }))
        .await;
    assert_eq!(resp.status_code(), 422, "item_type 'quiz' is rejected");
    assert_eq!(
        resp.text(),
        "validation: item_type must be 'page' or 'assignment'"
    );

    // Validation runs BEFORE module lookup and authz: a student with a bad
    // item_type on a nonexistent module gets 422, not 403/404.
    let resp = server
        .post(&format!("/modules/{MISSING_ID}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "item_type": "quiz", "item_id": 1 }))
        .await;
    assert_eq!(
        resp.status_code(),
        422,
        "item_type validation precedes authz and existence checks"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_item_valid_payload_upserts_and_repost_updates_position() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "UpsertCourse").await;
    let module = seed_module(&pool, course, "Item Upsert", 1).await;

    // Authz still gates first: a student gets 403 before the insert.
    let resp = server
        .post(&format!("/modules/{module}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "item_type": "page", "item_id": 1 }))
        .await;
    assert_eq!(resp.status_code(), 403, "student item creation is 403");

    // A teacher with a valid payload inserts the item and returns it.
    let resp = server
        .post(&format!("/modules/{module}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "item_type": "page", "item_id": 42, "position": 1 }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "valid item creation must succeed; body: {}",
        resp.text()
    );
    let item: Value = resp.json();
    let item_row_id = item["id"].as_i64().expect("item id");
    assert_eq!(item["module_id"], module);
    assert_eq!(item["item_type"], "page");
    assert_eq!(item["item_id"], 42);
    assert_eq!(item["position"], 1);

    // The row is really in the DB.
    let (row_id, position): (i64, i32) = sqlx::query_as(
        r#"SELECT id, position FROM module_items
           WHERE module_id = $1 AND item_type = $2 AND item_id = $3"#,
    )
    .bind(module)
    .bind("page")
    .bind(42)
    .fetch_one(&pool)
    .await
    .expect("item row must exist after a successful POST");
    assert_eq!(row_id, item_row_id, "returned id matches the stored row");
    assert_eq!(position, 1);

    // Re-posting the same (module_id, item_type, item_id) identity updates
    // the existing row's position instead of inserting a duplicate.
    let resp = server
        .post(&format!("/modules/{module}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "item_type": "page", "item_id": 42, "position": 7 }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "re-posting the same item identity must succeed; body: {}",
        resp.text()
    );
    let updated: Value = resp.json();
    assert_eq!(updated["id"], item_row_id, "the upsert reuses the row id");
    assert_eq!(updated["position"], 7, "position is updated in place");

    let count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM module_items
           WHERE module_id = $1 AND item_type = $2 AND item_id = $3"#,
    )
    .bind(module)
    .bind("page")
    .bind(42)
    .fetch_one(&pool)
    .await
    .expect("count items");
    assert_eq!(count, 1, "the re-post must not insert a duplicate row");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /modules/{id}/items/{itemId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_module_item_lifecycle_and_authz() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "DelItemCourse").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    let module = seed_module(&pool, course, "Item Host", 1).await;
    let other_module = seed_module(&pool, other_course, "Other Host", 1).await;
    let item = seed_module_item(&pool, module, "assignment", 505, 1).await;

    // Anonymous -> 401.
    let resp = server
        .delete(&format!("/modules/{module}/items/{item}"))
        .await;
    assert_eq!(resp.status_code(), 401);

    // Student -> 403.
    let resp = server
        .delete(&format!("/modules/{module}/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student item delete is 403");

    // Missing module -> 404 even for the teacher.
    let resp = server
        .delete(&format!("/modules/{MISSING_ID}/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Item belonging to a different module -> 404 (module_id must match).
    let resp = server
        .delete(&format!("/modules/{other_module}/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "an item cannot be deleted through another module's path"
    );

    // Teacher deletes the real item -> 204, row gone.
    let resp = server
        .delete(&format!("/modules/{module}/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "teacher item delete");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM module_items WHERE id = $1")
        .bind(item)
        .fetch_one(&pool)
        .await
        .expect("count item");
    assert_eq!(count, 0, "item row is hard-deleted");

    // Repeat delete -> 404 (0 rows affected).
    let resp = server
        .delete(&format!("/modules/{module}/items/{item}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "deleting a gone item is 404");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/modules/{moduleId}/items
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_module_items_returns_seeded_items_with_authz() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "ItemsListCourse").await;
    let other_course = seed_course(&pool, &prefix, "OtherListCourse").await;
    let module = seed_module(&pool, course, "Ordered Items", 1).await;
    let first = seed_module_item(&pool, module, "assignment", 22, 1).await;
    let second = seed_module_item(&pool, module, "page", 11, 2).await;

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    seed_enrollment(&pool, student, course, "student", "active").await;

    // Anonymous -> 401 from the AuthUser extractor, before any handler work.
    let resp = server
        .get(&format!("/courses/{course}/modules/{module}/items"))
        .await;
    assert_eq!(resp.status_code(), 401, "anonymous items list is 401");
    assert_eq!(resp.text(), "unauthorized");

    // Authenticated but not enrolled -> 403.
    let resp = server
        .get(&format!("/courses/{course}/modules/{module}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "non-enrolled items list is 403");
    assert_eq!(resp.text(), "forbidden");

    // Unauthorized callers still get 404 for a missing module: the module
    // lookup precedes the authz check.
    let resp = server
        .get(&format!("/courses/{course}/modules/{MISSING_ID}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "missing module 404s even for unauthorized callers (lookup precedes authz)"
    );

    // The module must belong to the course in the path.
    let resp = server
        .get(&format!("/courses/{other_course}/modules/{module}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "module must belong to the course in the path"
    );

    // Admin (DB role) reads any course without an enrollment.
    let resp = server
        .get(&format!("/courses/{course}/modules/{module}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "admin reads any module's items");

    // Active enrollment -> 200 with the seeded items in position order.
    let resp = server
        .get(&format!("/courses/{course}/modules/{module}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "enrolled student reads module items; body: {}",
        resp.text()
    );
    let items: Value = resp.json();
    let rows = items.as_array().expect("items is a JSON array");
    let ids: Vec<i64> = rows
        .iter()
        .map(|item| item["id"].as_i64().expect("item id"))
        .collect();
    assert_eq!(
        ids,
        vec![first, second],
        "items ordered by position ASC, id ASC"
    );
    assert_eq!(rows[0]["item_type"], "assignment");
    assert_eq!(rows[1]["item_type"], "page");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Full lifecycle: module -> items -> soft delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_lifecycle_module_to_items_to_soft_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "LifecycleCourse").await;
    seed_enrollment(&pool, student, course, "student", "active").await;
    let auth_header = auth(teacher, "teacher");

    // 1. Teacher creates a module through the API.
    let resp = server
        .post(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .json(&json!({ "name": "Lifecycle Module", "position": 1 }))
        .await;
    assert_eq!(resp.status_code(), 201, "create module");
    let module: Value = resp.json();
    let module_id = module["id"].as_i64().expect("module id");

    // 2. Seed items directly so their identities and order are deterministic.
    let page_item = seed_module_item(&pool, module_id, "page", 9001, 1).await;
    let assignment_item = seed_module_item(&pool, module_id, "assignment", 9002, 2).await;

    // 3. The enrolled student can see the module in the course list.
    let resp = server
        .get(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let ids: Vec<i64> = ids_of(resp);
    assert_eq!(ids, vec![module_id]);

    // 4. Detail flattens the module and its ordered items.
    let resp = server
        .get(&format!("/modules/{module_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let detail: Value = resp.json();
    assert_eq!(detail["name"], "Lifecycle Module");
    let items = detail["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], page_item);
    assert_eq!(items[1]["id"], assignment_item);

    // 5. The nested items route returns the same ordered list.
    let resp = server
        .get(&format!("/courses/{course}/modules/{module_id}/items"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let nested: Value = resp.json();
    let nested_ids: Vec<i64> = nested
        .as_array()
        .expect("items array")
        .iter()
        .map(|item| item["id"].as_i64().expect("item id"))
        .collect();
    assert_eq!(nested_ids, vec![page_item, assignment_item]);

    // 6. Teacher repositions the module.
    let resp = server
        .put(&format!("/modules/{module_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header.clone())
        .json(&json!({ "position": 5 }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let updated: Value = resp.json();
    assert_eq!(updated["position"], 5);

    // 7. Soft delete -> module vanishes from list/detail, but the rows
    //    (and the items) survive in the DB.
    let resp = server
        .delete(&format!("/modules/{module_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth_header)
        .await;
    assert_eq!(resp.status_code(), 204);

    let resp = server
        .get(&format!("/courses/{course}/modules"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    let ids: Vec<i64> = ids_of(resp);
    assert!(!ids.contains(&module_id), "deleted module leaves the list");

    let resp = server
        .get(&format!("/modules/{module_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "deleted module detail 404s");

    let (state,): (String,) = sqlx::query_as("SELECT workflow_state FROM modules WHERE id = $1")
        .bind(module_id)
        .fetch_one(&pool)
        .await
        .expect("module row survives soft delete");
    assert_eq!(state, "deleted");

    let item_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM module_items WHERE module_id = $1")
            .bind(module_id)
            .fetch_one(&pool)
            .await
            .expect("count items");
    assert_eq!(item_count, 2, "items survive the module soft delete");

    cleanup(&pool, &prefix).await;
}
