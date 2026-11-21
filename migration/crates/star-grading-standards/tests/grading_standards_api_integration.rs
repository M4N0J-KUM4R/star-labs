//! Integration tests for the Canvas `GradingStandardsApiController` routes.
//!
//! Exercises the real router against a real Postgres:
//!   GET    /accounts/{accountId}/grading_standards
//!   GET    /accounts/{accountId}/grading_standards/{id}
//!   POST   /accounts/{accountId}/grading_standards
//!   PUT    /courses/{courseId}/grading_standards/{id}
//!   PUT    /accounts/{accountId}/grading_standards/{id}
//!   DELETE /accounts/{accountId}/grading_standards/{id}
//!
//! Spec: Canvas `app/controllers/grading_standards_api_controller.rb` — the
//! body is `grading_standard_json` (`id`, `title`, `context_type`,
//! `context_id`, `scaling_factor`, `points_based`, `grading_scheme`), create
//! and update answer 200, an invalid scheme answers 400 with the Rails error
//! hash, and delete is a soft delete that answers 200 with the standard.
//!
//! The routes live in `src/grading_standards_api.rs`; the coordinator mounts
//! them in `star_grading_standards::router()`, which is what this suite
//! exercises.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-grading-standards
//!
//! Test cases:
//!   1. An account admin lists the standards of the account; a stranger gets
//!      403 and an anonymous caller 401.
//!   2. The standard of another account is not found (404).
//!   3. An account admin creates a standard; the scheme is stored highest
//!      bound first and the response carries the Canvas fields.
//!   4. A scheme without a 0% floor is a 400 with the Rails error hash.
//!   5. An account admin renames a standard and replaces its scheme.
//!   6. A teacher updates a course standard; a student gets 403.
//!   7. An account admin deletes a standard (200) and the standard is gone
//!      afterwards (404).
//!   8. A scheme entry without a value is skipped, like Canvas does.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gsapi-tests-{}-{}",
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
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_grading_standards::router())
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

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
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

/// A grading standard owned by an account or a course, with its scheme.
async fn seed_standard(
    pool: &PgPool,
    title: &str,
    account_id: Option<i64>,
    course_id: Option<i64>,
    entries: &[(&str, f64)],
) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO grading_standards (account_id, course_id, title, points_based, scaling_factor) \
         VALUES ($1, $2, $3, false, 1.0) RETURNING id",
    )
    .bind(account_id)
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed standard");

    for (position, (name, value)) in entries.iter().enumerate() {
        sqlx::query(
            "INSERT INTO grading_scheme_entries (grading_standard_id, name, min_score, position) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(name)
        .bind(*value)
        .bind(position as i32)
        .execute(pool)
        .await
        .expect("seed scheme entry");
    }

    id
}

/// A scheme with the usual Canvas bounds, highest bound first.
fn scheme() -> Vec<(&'static str, f64)> {
    vec![("A", 0.94), ("B", 0.80), ("F", 0.0)]
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // The courses reference the accounts, so they go first.
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
async fn an_account_admin_lists_the_account_grading_standards() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "user").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "user").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;
    let standard = seed_standard(&pool, "Standard A", Some(account), None, &scheme()).await;

    let resp = server
        .get(&format!("/accounts/{account}/grading_standards"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let list: Vec<Value> = resp.json();
    assert_eq!(list.len(), 1, "body: {list:?}");
    assert_eq!(list[0]["id"], standard);
    assert_eq!(list[0]["title"], "Standard A");
    assert_eq!(list[0]["context_type"], "Account");
    assert_eq!(list[0]["context_id"], account);
    assert_eq!(list[0]["points_based"], false);
    assert_eq!(list[0]["scaling_factor"], 1.0);

    // The scheme is rendered highest bound first, with the calculated value.
    let entries = list[0]["grading_scheme"].as_array().expect("scheme");
    assert_eq!(entries.len(), 3, "body: {list:?}");
    assert_eq!(entries[0]["name"], "A");
    assert_eq!(entries[0]["value"], 0.94);
    assert_eq!(entries[0]["calculated_value"], 94.0);
    assert_eq!(entries[1]["name"], "B");
    assert_eq!(entries[1]["value"], 0.80);
    assert_eq!(entries[1]["calculated_value"], 80.0);
    assert_eq!(entries[2]["name"], "F");
    assert_eq!(entries[2]["value"], 0.0);
    assert_eq!(entries[2]["calculated_value"], 0.0);

    // A user without any relation to the account is refused.
    let resp = server
        .get(&format!("/accounts/{account}/grading_standards"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "user"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An anonymous caller is unauthorized.
    let resp = server
        .get(&format!("/accounts/{account}/grading_standards"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_standard_of_another_account_is_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "user").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let other_account = seed_account(&pool, &prefix, "Other").await;
    seed_account_admin(&pool, account, admin).await;
    seed_account_admin(&pool, other_account, admin).await;
    let standard = seed_standard(&pool, "Standard A", Some(other_account), None, &scheme()).await;

    // The standard belongs to another account: it is not found (404).
    let resp = server
        .get(&format!("/accounts/{account}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // It is found through its own account.
    let resp = server
        .get(&format!(
            "/accounts/{other_account}/grading_standards/{standard}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], standard);
    assert_eq!(body["context_type"], "Account");
    assert_eq!(body["context_id"], other_account);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_account_admin_creates_a_grading_standard() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "user").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "user").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;

    let payload = json!({
        "title": "New standard name",
        "points_based": false,
        "scaling_factor": 1.0,
        "grading_scheme_entry": [
            { "name": "A", "value": 94 },
            { "name": "B", "value": 80 },
            { "name": "F", "value": 0 }
        ]
    });

    // A user without the manage right may not create a standard.
    let resp = server
        .post(&format!("/accounts/{account}/grading_standards"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "user"))
        .json(&payload)
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .post(&format!("/accounts/{account}/grading_standards"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .json(&payload)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let body: Value = resp.json();
    assert_eq!(body["title"], "New standard name");
    assert_eq!(body["context_type"], "Account");
    assert_eq!(body["context_id"], account);
    assert_eq!(body["points_based"], false);
    assert_eq!(body["scaling_factor"], 1.0);

    let standard = body["id"].as_i64().expect("id");

    // The submitted values are stored as lower bounds, highest bound first.
    let entries: Vec<(String, f64, i32)> = sqlx::query_as(
        "SELECT name, min_score::float8, position FROM grading_scheme_entries \
         WHERE grading_standard_id = $1 ORDER BY position ASC",
    )
    .bind(standard)
    .fetch_all(&pool)
    .await
    .expect("read the scheme");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0], ("A".to_string(), 0.94, 0));
    assert_eq!(entries[1], ("B".to_string(), 0.80, 1));
    assert_eq!(entries[2], ("F".to_string(), 0.0, 2));

    let owner: (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT account_id, course_id FROM grading_standards WHERE id = $1")
            .bind(standard)
            .fetch_one(&pool)
            .await
            .expect("read the standard");
    assert_eq!(owner, (Some(account), None));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_scheme_without_a_zero_floor_is_a_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "user").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;

    let payload = json!({
        "title": "No floor",
        "grading_scheme_entry": [
            { "name": "A", "value": 94 },
            { "name": "B", "value": 80 }
        ]
    });

    let resp = server
        .post(&format!("/accounts/{account}/grading_standards"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .json(&payload)
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    let errors: Value = resp.json();
    assert_eq!(
        errors["data"][0],
        "grading schemes must have 0% for the lowest grade"
    );

    // A scheme without any entry is not valid either.
    let resp = server
        .post(&format!("/accounts/{account}/grading_standards"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .json(&json!({ "title": "Empty" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let errors: Value = resp.json();
    assert_eq!(errors["data"][0], "can't be blank");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_entry_without_a_value_is_skipped() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "user").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;

    // `GradingStandard#standard_data=` skips a row without a value, so the
    // only entry of this scheme is dropped and the scheme stays empty.
    let resp = server
        .post(&format!("/accounts/{account}/grading_standards"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .json(&json!({
            "title": "Incomplete entry",
            "grading_scheme_entry": [{ "name": "A" }]
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let errors: Value = resp.json();
    assert_eq!(errors["data"][0], "can't be blank");

    // A complete row next to an incomplete one keeps the complete one.
    let resp = server
        .post(&format!("/accounts/{account}/grading_standards"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .json(&json!({
            "title": "One good entry",
            "grading_scheme_entry": [
                { "name": "A" },
                { "name": "F", "value": 0 }
            ]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let entries = body["grading_scheme"].as_array().expect("scheme");
    assert_eq!(entries.len(), 1, "body: {body:?}");
    assert_eq!(entries[0]["name"], "F");
    assert_eq!(entries[0]["value"], 0.0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_account_admin_updates_the_title_and_the_scheme() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "user").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;
    let standard = seed_standard(&pool, "Standard A", Some(account), None, &scheme()).await;

    // A title-only update keeps the scheme.
    let resp = server
        .put(&format!("/accounts/{account}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .json(&json!({ "title": "Renamed" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], "Renamed");
    assert_eq!(body["id"], standard);
    let entries = body["grading_scheme"].as_array().expect("scheme");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["name"], "A");

    // A scheme update replaces the entries.
    let resp = server
        .put(&format!("/accounts/{account}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .json(&json!({
            "title": "Renamed",
            "grading_scheme_entry": [
                { "name": "A-", "value": 90 },
                { "name": "F", "value": 0 }
            ]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let entries = body["grading_scheme"].as_array().expect("scheme");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["name"], "A-");
    assert_eq!(entries[0]["value"], 0.90);
    assert_eq!(entries[0]["calculated_value"], 90.0);
    assert_eq!(entries[1]["name"], "F");

    // A standard of another context is not found.
    let other_account = seed_account(&pool, &prefix, "Other").await;
    let resp = server
        .put(&format!(
            "/accounts/{other_account}/grading_standards/{standard}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .json(&json!({ "title": "Nope" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_teacher_updates_a_course_grading_standard() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "user").await;
    let student = seed_user(&pool, &prefix, "Student", "user").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let standard = seed_standard(&pool, "Standard A", None, Some(course), &scheme()).await;

    // A student may not manage the scheme.
    let resp = server
        .put(&format!("/courses/{course}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "user"))
        .json(&json!({ "title": "Nope" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .put(&format!("/courses/{course}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "user"))
        .json(&json!({ "title": "Course standard" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], "Course standard");
    assert_eq!(body["context_type"], "Course");
    assert_eq!(body["context_id"], course);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_account_admin_deletes_a_grading_standard() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "user").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;
    let standard = seed_standard(&pool, "Standard A", Some(account), None, &scheme()).await;

    let resp = server
        .delete(&format!("/accounts/{account}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], standard);
    assert_eq!(body["title"], "Standard A");

    // The delete is a soft delete and the standard is gone afterwards.
    let workflow_state: String =
        sqlx::query_scalar("SELECT workflow_state FROM grading_standards WHERE id = $1")
            .bind(standard)
            .fetch_one(&pool)
            .await
            .expect("read the standard");
    assert_eq!(workflow_state, "deleted");

    let resp = server
        .get(&format!("/accounts/{account}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "user"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
