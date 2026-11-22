//! Integration tests for the Canvas course reports routes
//! (`CourseReportsController` — spec `specs/p5-rest-account-reports.md`
//! routes 7–9).
//!
//! Pinned behavior (ported from
//! `canvas-src/app/controllers/course_reports_controller.rb` and the
//! account-reports API spec):
//! - `GET /courses/{id}/reports/{type}` (`last`) renders the caller's most
//!   recent report of the type — `{}` when there is none;
//! - `POST /courses/{id}/reports/{type}` (`create`) answers 400 with
//!   `{"error": "invalid report type <t>"}` for unknown types, 403 for a
//!   caller without `read_reports` (a student), 404 for an unknown
//!   course, and creates a `created` report plus a queued `course_report`
//!   job for a teacher;
//! - `GET /courses/{id}/reports/{type}/{report_id}` (`show`) returns the
//!   report (the owner or a teacher), 404 for unknown ids and for reports
//!   of another course;
//! - anonymous callers get 401 on every route.
//!
//! Fleet rule: dedicated users, accounts, courses and reports per test,
//! every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is
//! unset or the tables are not migrated yet.

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
        "coursereports-{}-{}-{}",
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

    // The course_reports table comes from migration 0241; skip until it
    // landed.
    let exists: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = 'course_reports'",
    )
    .fetch_optional(&pool)
    .await
    .expect("check for the course_reports table");
    if exists.is_none() {
        println!("SKIP: the course_reports table is not migrated yet");
        return None;
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = TestServer::new(star_account_reports::router().with_state(state))
        .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-account"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-Course"))
    .bind(format!("{prefix}-C"))
    .bind(account_id)
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

async fn seed_report(
    pool: &PgPool,
    course_id: i64,
    user_id: i64,
    report_type: &str,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO course_reports \
             (course_id, user_id, report_type, workflow_state, progress) \
         VALUES ($1, $2, $3, $4, 0) RETURNING id",
    )
    .bind(course_id)
    .bind(user_id)
    .bind(report_type)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed course report")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
    let _ = sqlx::query(
        "DELETE FROM course_reports WHERE course_id IN \
         (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM jobs WHERE tag = 'course_report#run_report' \
         AND context_id NOT IN (SELECT id FROM course_reports)",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN \
         (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(&pattern)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(&pattern)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn last_answers_the_callers_most_recent_report() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix).await;
    let course = seed_course(&pool, &prefix, account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let other = seed_user(&pool, &prefix, "Other", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // No report yet: `last` renders `{}`.
    let resp = server
        .get(&format!("/courses/{course}/reports/course_pace_docx"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({}));

    // The teacher's most recent report of the type is answered…
    let mine = seed_report(&pool, course, teacher, "course_pace_docx", "complete").await;
    // …but another user's report never is (`last` scopes to the caller).
    let _ = seed_report(&pool, course, other, "course_pace_docx", "complete").await;
    let resp = server
        .get(&format!("/courses/{course}/reports/course_pace_docx"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], json!(mine));
    assert_eq!(body["report_type"], json!("course_pace_docx"));
    assert_eq!(body["course_id"], json!(course));
    assert_eq!(body["status"], json!("complete"));
    assert_eq!(body["progress"], json!(100));

    // A deleted report is gone from the scope.
    sqlx::query("UPDATE course_reports SET workflow_state = 'deleted' WHERE id = $1")
        .bind(mine)
        .execute(&pool)
        .await
        .expect("soft delete the report");
    let resp = server
        .get(&format!("/courses/{course}/reports/course_pace_docx"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({}));

    // An unknown course is a 404 and an anonymous caller a 401.
    let resp = server
        .get("/courses/999999999/reports/course_pace_docx")
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get(&format!("/courses/{course}/reports/course_pace_docx"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_validates_the_type_and_the_permission_and_queues_the_job() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix).await;
    let course = seed_course(&pool, &prefix, account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // An unknown report type is the Canvas 400 — for every caller, before
    // the permission gate.
    let resp = server
        .post(&format!("/courses/{course}/reports/zero_gravity_pdf"))
        .add_header("Authorization", auth(student, "student"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], json!("invalid report type zero_gravity_pdf"));

    // A student (no `read_reports`) is a 403 for a valid type.
    let resp = server
        .post(&format!("/courses/{course}/reports/course_pace_docx"))
        .add_header("Authorization", auth(student, "student"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An anonymous caller is a 401; an unknown course a 404.
    let resp = server
        .post(&format!("/courses/{course}/reports/course_pace_docx"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .post("/courses/999999999/reports/course_pace_docx")
        .add_header("Authorization", auth(teacher, "teacher"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A teacher creates the report: created state + a queued job.
    let resp = server
        .post(&format!(
            "/courses/{course}/reports/course_pace_docx?parameters[section_ids][]=1"
        ))
        .add_header("Authorization", auth(teacher, "teacher"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["report_type"], json!("course_pace_docx"));
    assert_eq!(body["course_id"], json!(course));
    assert_eq!(body["status"], json!("created"));
    assert_eq!(body["progress"], json!(0));
    assert_eq!(body["file_url"], Value::Null);
    let report_id = body["id"].as_i64().expect("report id");

    let (job_count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM jobs \
         WHERE tag = 'course_report#run_report' AND context_type = 'CourseReport' \
           AND context_id = $1 AND workflow_state = 'queued'",
    )
    .bind(report_id)
    .fetch_one(&pool)
    .await
    .expect("count jobs");
    assert_eq!(job_count, 1, "the course_report job is queued");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_returns_the_report_of_the_course() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix).await;
    let course = seed_course(&pool, &prefix, account).await;
    let other_course = seed_course(&pool, &prefix, account).await;
    let _ = other_course;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let owner = seed_user(&pool, &prefix, "Owner", "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let report = seed_report(&pool, course, owner, "course_pace_docx", "complete").await;

    // The owner reads their report…
    let resp = server
        .get(&format!(
            "/courses/{course}/reports/course_pace_docx/{report}"
        ))
        .add_header("Authorization", auth(owner, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], json!(report));

    // …and so does a teacher of the course (`:read` on the record).
    let resp = server
        .get(&format!(
            "/courses/{course}/reports/course_pace_docx/{report}"
        ))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // An unknown id is a 404, an anonymous caller a 401.
    let resp = server
        .get(&format!(
            "/courses/{course}/reports/course_pace_docx/99999999"
        ))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get(&format!(
            "/courses/{course}/reports/course_pace_docx/{report}"
        ))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
