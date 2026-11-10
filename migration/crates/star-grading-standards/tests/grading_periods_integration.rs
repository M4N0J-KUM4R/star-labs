//! Integration tests for the Canvas grading-period routes.
//!
//! Exercises the real grading-standards router
//! (GET/POST /accounts/{accountId}/grading_periods and
//! GET/PUT/DELETE /grading_periods/{id}) through axum-test against Postgres.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-grading-standards
//!
//! Test cases:
//!   1. Batch create returns Canvas-shaped periods (group id, weight,
//!      is_closed, permissions) and list/get expose them.
//!   2. PUT updates a period, DELETE soft-deletes it (204, then 404).
//!   3. Authz: an enrolled student can read but not manage; a non-admin
//!      account admin and global admin both pass.
//!   4. Validation: bad ordering, missing title, and overlapping periods 422.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gp-tests-{}-{}",
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
    .bind(name)
    .bind(format!("{prefix}-{name}"))
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

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // Courses first: courses.account_id has no cascade, accounts cascade
    // grading_period_groups -> grading_periods.
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
async fn create_list_and_get_grading_periods() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root").await;

    let payload = json!({
        "grading_periods": [
            {
                "title": "Fall 2999",
                "start_date": "2999-08-01",
                "end_date": "2999-12-15",
                "close_date": "2999-12-22",
                "weight": 60.0
            },
            {
                "title": "Fall 1999",
                "start_date": "1999-08-01",
                "end_date": "1999-12-15"
            }
        ]
    });

    let resp = server
        .post(&format!("/accounts/{account}/grading_periods"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&payload)
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    let body: Value = resp.json();
    let periods = body["grading_periods"].as_array().expect("periods array");
    assert_eq!(periods.len(), 2);
    assert!(periods[0]["id"].as_i64().is_some());
    assert!(periods[0]["grading_period_group_id"].as_i64().is_some());
    assert_eq!(periods[0]["title"], "Fall 2999");
    assert_eq!(periods[0]["weight"], 60.0);
    assert_eq!(periods[0]["is_closed"], Value::Bool(false));
    assert_eq!(periods[0]["permissions"]["read"], Value::Bool(true));
    assert_eq!(periods[0]["permissions"]["update"], Value::Bool(true));
    // close_date defaults to end_date, which is already in the past.
    assert_eq!(periods[1]["close_date"], "1999-12-15T00:00:00Z");
    assert_eq!(periods[1]["weight"], Value::Null);
    assert_eq!(periods[1]["is_closed"], Value::Bool(true));

    let future_id = periods[0]["id"].as_i64().unwrap();

    let resp = server
        .get(&format!("/accounts/{account}/grading_periods"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let listed = body["grading_periods"].as_array().expect("periods array");
    assert_eq!(listed.len(), 2);
    // Ordered by start_date: 1999 period first.
    assert_eq!(listed[0]["title"], "Fall 1999");
    assert_eq!(listed[1]["id"].as_i64(), Some(future_id));
    assert_eq!(body["can_create_grading_periods"], Value::Bool(true));
    assert_eq!(body["grading_periods_read_only"], Value::Bool(false));

    let resp = server
        .get(&format!("/grading_periods/{future_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["grading_periods"][0]["id"].as_i64(), Some(future_id));
    assert_eq!(body["grading_periods"][0]["title"], "Fall 2999");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_and_delete_grading_period() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root").await;

    let resp = server
        .post(&format!("/accounts/{account}/grading_periods"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "title": "Block 1",
            "start_date": "2999-01-05",
            "end_date": "2999-03-20",
            "close_date": "2999-03-27"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let id = resp.json::<Value>()["grading_periods"][0]["id"]
        .as_i64()
        .unwrap();

    let resp = server
        .put(&format!("/grading_periods/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "title": "Block 1 (revised)", "weight": 25.5 }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let period = &body["grading_periods"][0];
    assert_eq!(period["id"].as_i64(), Some(id));
    assert_eq!(period["title"], "Block 1 (revised)");
    assert_eq!(period["weight"], 25.5);
    // Unspecified dates are preserved.
    assert_eq!(period["start_date"], "2999-01-05T00:00:00Z");
    assert_eq!(period["end_date"], "2999-03-20T00:00:00Z");
    assert_eq!(period["close_date"], "2999-03-27T00:00:00Z");

    let resp = server
        .delete(&format!("/grading_periods/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    let resp = server
        .get(&format!("/grading_periods/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .get(&format!("/accounts/{account}/grading_periods"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(
        resp.json::<Value>()["grading_periods"]
            .as_array()
            .map(|a| a.len()),
        Some(0)
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grading_period_authz() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let global_admin = seed_user(&pool, &prefix, "GlobalAdmin", "admin").await;
    let account_admin = seed_user(&pool, &prefix, "AccountAdmin", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_account_admin(&pool, account, account_admin).await;
    seed_enrollment(&pool, student, course, "student").await;

    let missing = account + 10_000_000;
    let resp = server
        .get(&format!("/accounts/{missing}/grading_periods"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(global_admin, "admin"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Enrolled student: read-only access.
    let resp = server
        .get(&format!("/accounts/{account}/grading_periods"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["can_create_grading_periods"],
        Value::Bool(false)
    );

    // Not enrolled anywhere in the account: forbidden.
    let resp = server
        .get(&format!("/accounts/{account}/grading_periods"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let payload = json!({
        "title": "Managed",
        "start_date": "2999-01-01",
        "end_date": "2999-02-01"
    });

    let resp = server
        .post(&format!("/accounts/{account}/grading_periods"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&payload)
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Account admin (non-global) can create; the resulting period is readable
    // by the student but not updatable/deletable by them.
    let resp = server
        .post(&format!("/accounts/{account}/grading_periods"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(account_admin, "teacher"),
        )
        .json(&payload)
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let id = resp.json::<Value>()["grading_periods"][0]["id"]
        .as_i64()
        .unwrap();

    let resp = server
        .get(&format!("/grading_periods/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["grading_periods"][0]["permissions"]["update"],
        Value::Bool(false)
    );

    let resp = server
        .put(&format!("/grading_periods/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "title": "Hijacked" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .delete(&format!("/grading_periods/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grading_period_validation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Root").await;
    let path = format!("/accounts/{account}/grading_periods");

    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "title": "Backwards",
            "start_date": "2999-03-01",
            "end_date": "2999-01-01"
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "start_date": "2999-01-01", "end_date": "2999-02-01" }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "title": "First",
            "start_date": "2999-01-01",
            "end_date": "2999-02-01"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "title": "Overlaps",
            "start_date": "2999-01-15",
            "end_date": "2999-03-01"
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    // Touching endpoints are not an overlap in Canvas.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "title": "Second",
            "start_date": "2999-02-01",
            "end_date": "2999-03-01"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
