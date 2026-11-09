//! Integration tests for star-api.
//!
//! Exercises the shared API helpers as pure functions where possible and
//! against a real Postgres where they need one:
//! - PaginationParams defaults, clamping, and offset computation plus the
//!   Paginated<T> response shape (pure functions, always run).
//! - check_enrollment / require_course_teacher authorization helpers
//!   (DB-backed).
//!
//! Conventions (mirrors the rest of the workspace):
//! - DB-backed tests are skipped cleanly when DATABASE_URL is not set (prints
//!   SKIP and returns); pagination tests always run.
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-api
//! - DB tests use a unique email/course_code prefix per run + full teardown
//!   so they are safe to run repeatedly against a shared/dev database.
//!
//! NOTE: the two authorization helpers take their arguments in a different
//! order, exactly as defined in star-api:
//!   check_enrollment(user_id, course_id, role, pool)
//!   require_course_teacher(user_id, role, course_id, pool)

use axum::http::StatusCode;
use serde::Serialize;
use serde_json::json;
use sqlx::PgPool;
use star_api::{check_enrollment, require_course_teacher, Paginated, PaginationParams};
use star_common::ApiError;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email / courses.course_code UNIQUE constraints.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    format!("api-tests-{}-{}", std::process::id(), millis + n as i64)
}

/// Connect and run migrations. Returns None when DATABASE_URL is unset so
/// callers can skip cleanly.
async fn db_pool() -> Option<PgPool> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    Some(pool)
}

/// Create a user directly in the DB (bypassing the register endpoint).
/// Returns user id.
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
    .bind("unused-hash")
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

/// Insert an active enrollment row with the given role.
async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state)
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// Delete every row created by this run (users cascade to enrollments).
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

/// Parse a query string into PaginationParams the same way axum's Query
/// extractor does internally.
fn parse_query(query: &str) -> PaginationParams {
    serde_urlencoded::from_str(query).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// PaginationParams (pure, always runs)
// ---------------------------------------------------------------------------

#[test]
fn pagination_defaults_from_empty_query() {
    let p = parse_query("");
    assert_eq!(p.page, 1);
    assert_eq!(p.per_page, 20);
}

#[test]
fn pagination_default_impl_matches_serde_defaults() {
    let p = PaginationParams::default();
    assert_eq!(p.page, 1);
    assert_eq!(p.per_page, 20);

    let q = parse_query("");
    assert_eq!(p.page, q.page);
    assert_eq!(p.per_page, q.per_page);
}

#[test]
fn pagination_explicit_values_parsed() {
    let p = parse_query("page=5&per_page=50");
    assert_eq!(p.page, 5);
    assert_eq!(p.per_page, 50);

    let p = parse_query("page=3");
    assert_eq!(p.page, 3);
    assert_eq!(p.per_page, 20, "per_page keeps its default");

    let p = parse_query("per_page=10");
    assert_eq!(p.page, 1, "page keeps its default");
    assert_eq!(p.per_page, 10);
}

#[test]
fn pagination_page_clamped_to_minimum_one() {
    let p = parse_query("page=0");
    assert_eq!(p.page(), 1, "page 0 clamps to 1");

    let p = parse_query("page=-7");
    assert_eq!(p.page(), 1, "negative page clamps to 1");

    let p = parse_query("page=2");
    assert_eq!(p.page(), 2, "in-range page is untouched");
}

#[test]
fn pagination_per_page_clamped_to_range() {
    let p = parse_query("per_page=0");
    assert_eq!(p.per_page(), 1, "per_page 0 clamps to 1");

    let p = parse_query("per_page=-5");
    assert_eq!(p.per_page(), 1, "negative per_page clamps to 1");

    let p = parse_query("per_page=1");
    assert_eq!(p.per_page(), 1, "lower bound is inclusive");

    let p = parse_query("per_page=100");
    assert_eq!(p.per_page(), 100, "upper bound is inclusive");

    let p = parse_query("per_page=101");
    assert_eq!(p.per_page(), 100, "per_page above 100 clamps to 100");

    let p = parse_query("per_page=9999");
    assert_eq!(p.per_page(), 100, "large per_page clamps to 100");
}

#[test]
fn pagination_offset_uses_effective_values() {
    let p = parse_query("page=1&per_page=20");
    assert_eq!(p.offset(), 0);

    let p = parse_query("page=3&per_page=20");
    assert_eq!(p.offset(), 40);

    let p = parse_query("page=3&per_page=50");
    assert_eq!(p.offset(), 100);

    // Out-of-range inputs are clamped before the arithmetic.
    let p = PaginationParams {
        page: 0,
        per_page: 500,
    };
    assert_eq!(p.offset(), 0, "(1-1) * 100 after clamping");

    let p = PaginationParams {
        page: -4,
        per_page: 0,
    };
    assert_eq!(p.offset(), 0, "(1-1) * 1 after clamping");

    let p = PaginationParams {
        page: 5,
        per_page: 0,
    };
    assert_eq!(p.offset(), 4, "(5-1) * 1 after clamping");
}

// ---------------------------------------------------------------------------
// Paginated<T> response shape (pure, always runs)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct Item {
    id: i64,
    label: String,
}

#[test]
fn paginated_response_json_shape() {
    let resp = Paginated::new(
        vec![
            Item {
                id: 1,
                label: "a".to_string(),
            },
            Item {
                id: 2,
                label: "b".to_string(),
            },
        ],
        2,
        25,
        101,
    );

    let v = serde_json::to_value(&resp).expect("serialize Paginated");

    // Exactly the two coded top-level fields, no more, no fewer.
    let obj = v.as_object().expect("object response");
    assert_eq!(obj.len(), 2, "unexpected extra fields: {obj:?}");
    assert!(obj.contains_key("data"));
    assert!(obj.contains_key("pagination"));

    // Data array carries the serialized items in order.
    let data = v["data"].as_array().expect("data is an array");
    assert_eq!(data.len(), 2);
    assert_eq!(v["data"][0]["id"], json!(1));
    assert_eq!(v["data"][0]["label"], json!("a"));
    assert_eq!(v["data"][1]["id"], json!(2));
    assert_eq!(v["data"][1]["label"], json!("b"));

    // Pagination metadata carries page/per_page/total as coded.
    let meta = v["pagination"].as_object().expect("pagination object");
    assert_eq!(meta.len(), 3, "unexpected extra meta fields: {meta:?}");
    assert_eq!(v["pagination"]["page"], json!(2));
    assert_eq!(v["pagination"]["per_page"], json!(25));
    assert_eq!(v["pagination"]["total"], json!(101));
}

#[test]
fn paginated_empty_data_serializes_empty_array() {
    let empty: Vec<Item> = Vec::new();
    let resp = Paginated::new(empty, 1, 20, 0);

    let v = serde_json::to_value(&resp).expect("serialize Paginated");
    assert_eq!(v["data"], json!([]));
    assert_eq!(v["pagination"]["page"], json!(1));
    assert_eq!(v["pagination"]["per_page"], json!(20));
    assert_eq!(v["pagination"]["total"], json!(0));
}

// ---------------------------------------------------------------------------
// check_enrollment (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn check_enrollment_ok_for_enrolled_student() {
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "EnrolledCourse").await;
    seed_enrollment(&pool, student, course, "student").await;

    let res = check_enrollment(student, course, "student", &pool).await;
    assert!(res.is_ok(), "enrolled student must pass: {res:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn check_enrollment_ok_for_teacher_row_as_coded() {
    // As coded: the SQL checks for ANY enrollment row, without a role filter,
    // so a teacher-role enrollment also satisfies check_enrollment.
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "TeacherRowCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let res = check_enrollment(teacher, course, "teacher", &pool).await;
    assert!(res.is_ok(), "teacher enrollment row must pass: {res:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn check_enrollment_forbidden_without_enrollment_row() {
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "EmptyCourse").await;

    // User and course both exist, but no enrollment row links them.
    let err = check_enrollment(student, course, "student", &pool)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Forbidden));
    assert_eq!(err.status(), StatusCode::FORBIDDEN);

    // An unknown user id is equally forbidden.
    let err = check_enrollment(999_999_999, course, "student", &pool)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Forbidden));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn check_enrollment_forbidden_for_different_course() {
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course_a = seed_course(&pool, &prefix, "CourseA").await;
    let course_b = seed_course(&pool, &prefix, "CourseB").await;
    seed_enrollment(&pool, student, course_a, "student").await;

    let err = check_enrollment(student, course_b, "student", &pool)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Forbidden));
    assert_eq!(err.status(), StatusCode::FORBIDDEN);

    // Sanity: the same user in the course they are enrolled in passes.
    let res = check_enrollment(student, course_a, "student", &pool).await;
    assert!(res.is_ok());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn check_enrollment_admin_bypasses_without_row() {
    // As coded: role == "admin" short-circuits before any SQL runs, so an
    // admin passes even with no enrollment row at all.
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "AdminBypassCourse").await;

    let res = check_enrollment(admin, course, "admin", &pool).await;
    assert!(
        res.is_ok(),
        "admin must bypass the enrollment check: {res:?}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// require_course_teacher (DB-backed)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn require_course_teacher_ok_for_teacher_enrollment() {
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "TaughtCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let res = require_course_teacher(teacher, "teacher", course, &pool).await;
    assert!(res.is_ok(), "teacher of the course must pass: {res:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn require_course_teacher_forbidden_for_student_enrollment() {
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "StudentOnlyCourse").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Enrolled, but as a student: no teacher privileges.
    let err = require_course_teacher(student, "student", course, &pool)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Forbidden));
    assert_eq!(err.status(), StatusCode::FORBIDDEN);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn require_course_teacher_forbidden_without_enrollment() {
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UnrelatedCourse").await;

    // Global teacher role alone is not enough without an enrollment row.
    let err = require_course_teacher(teacher, "teacher", course, &pool)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Forbidden));
    assert_eq!(err.status(), StatusCode::FORBIDDEN);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn require_course_teacher_admin_bypasses_without_row() {
    // As coded: role == "admin" short-circuits before any SQL runs.
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "AdminTeacherBypass").await;

    let res = require_course_teacher(admin, "admin", course, &pool).await;
    assert!(res.is_ok(), "admin must bypass the teacher check: {res:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn require_course_teacher_forbidden_for_teacher_of_other_course() {
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let own = seed_course(&pool, &prefix, "OwnCourse").await;
    let other = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, teacher, own, "teacher").await;

    let err = require_course_teacher(teacher, "teacher", other, &pool)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Forbidden));
    assert_eq!(err.status(), StatusCode::FORBIDDEN);

    // Sanity: the same teacher passes for the course they actually teach.
    let res = require_course_teacher(teacher, "teacher", own, &pool).await;
    assert!(res.is_ok());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn require_course_teacher_uses_enrollment_row_not_global_role() {
    // As coded: the role argument is only consulted for the admin bypass;
    // the teacher check looks at the enrollment row's role. A user whose
    // global role is "student" but who holds a teacher enrollment passes.
    let Some(pool) = db_pool().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let prefix = run_prefix();

    let student = seed_user(&pool, &prefix, "StudentTeacher", "student").await;
    let course = seed_course(&pool, &prefix, "RowRoleCourse").await;
    seed_enrollment(&pool, student, course, "teacher").await;

    let res = require_course_teacher(student, "student", course, &pool).await;
    assert!(res.is_ok(), "teacher enrollment row must win: {res:?}");

    cleanup(&pool, &prefix).await;
}
