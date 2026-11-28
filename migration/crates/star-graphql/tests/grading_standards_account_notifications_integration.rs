//! Integration tests for GradingStandard/GradingStandardItem on Course
//! and AccountNotification on Query (GraphQL parity gap completion).
//! Conventions mirror `assignments_connection_integration.rs`.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-gs-{}-{}",
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
    let app = axum::Router::new()
        .nest("/api/graphql", star_graphql::router())
        .with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
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

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active') RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

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

async fn gql_post(server: &TestServer, token: &str, query: &str, variables: Value) -> Value {
    let resp = server
        .post("/api/graphql")
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "query": query, "variables": variables }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "graphql post should succeed; body: {}",
        resp.text()
    );
    resp.json()
}

#[tokio::test]
async fn grading_standards_returns_nested_scheme_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "GSCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let std_id: i64 = sqlx::query_scalar(
        "INSERT INTO grading_standards (course_id, title, points_based, scaling_factor) \
         VALUES ($1, 'Letter Grades', true, 1.0) RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed standard");
    for (name, min_score, pos) in [("A", 0.9, 1), ("B", 0.8, 2), ("C", 0.7, 3)] {
        sqlx::query(
            "INSERT INTO grading_scheme_entries (grading_standard_id, name, min_score, position) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(std_id)
        .bind(name)
        .bind(min_score)
        .bind(pos)
        .execute(&pool)
        .await
        .expect("seed entry");
    }
    // A deleted standard: must be filtered.
    sqlx::query(
        "INSERT INTO grading_standards (course_id, title, workflow_state) \
         VALUES ($1, 'Old', 'deleted')",
    )
    .bind(course)
    .execute(&pool)
    .await
    .expect("seed deleted standard");

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { gradingStandards { id title pointsBased \
         scalingFactor gradingStandardItemData { name value } createdAt updatedAt } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    let standards = body["data"]["course"]["gradingStandards"]
        .as_array()
        .expect("standards");
    assert_eq!(standards.len(), 1, "deleted filtered: {body}");
    let s = &standards[0];
    assert_eq!(s["id"], std_id.to_string(), "{body}");
    assert_eq!(s["title"], "Letter Grades", "{body}");
    assert_eq!(s["pointsBased"], true, "{body}");
    assert_eq!(s["scalingFactor"], 1.0, "{body}");
    assert!(s["createdAt"].is_string(), "{body}");

    let items = s["gradingStandardItemData"].as_array().expect("items");
    assert_eq!(items.len(), 3, "{body}");
    assert_eq!(items[0]["name"], "A", "position order: {body}");
    assert_eq!(items[0]["value"], 0.9, "{body}");
    assert_eq!(items[2]["name"], "C", "{body}");
    assert_eq!(items[2]["value"], 0.7, "{body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_notifications_window_and_admin_gate() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let (account_id,): (i64,) =
        sqlx::query_as("INSERT INTO accounts (name) VALUES ('GQL Account') RETURNING id")
            .fetch_one(&pool)
            .await
            .expect("seed account");

    // Window-valid announcement (should appear).
    let live: i64 = sqlx::query_scalar(
        "INSERT INTO account_notifications (account_id, subject, message, start_at, end_at, icon) \
         VALUES ($1, 'Maintenance', 'Tonight', now() - interval '1 day', now() + interval '1 day', 'calendar') RETURNING id",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("seed live");
    // Not yet started (hidden).
    sqlx::query(
        "INSERT INTO account_notifications (account_id, subject, message, start_at, end_at) \
         VALUES ($1, 'Future', 'Later', now() + interval '2 days', now() + interval '3 days')",
    )
    .bind(account_id)
    .execute(&pool)
    .await
    .expect("seed future");
    // Expired (hidden).
    sqlx::query(
        "INSERT INTO account_notifications (account_id, subject, message, start_at, end_at) \
         VALUES ($1, 'Past', 'Over', now() - interval '3 days', now() - interval '2 days')",
    )
    .bind(account_id)
    .execute(&pool)
    .await
    .expect("seed past");

    let query = "query ($id: ID!) { accountNotifications(accountId: $id) { id accountId subject \
                 message icon notificationType startAt endAt } }";

    // Admin sees only the window-valid one.
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        query,
        json!({ "id": account_id.to_string() }),
    )
    .await;
    let notes = body["data"]["accountNotifications"]
        .as_array()
        .expect("notes");
    assert_eq!(notes.len(), 1, "window filter: {body}");
    assert_eq!(notes[0]["id"], live.to_string(), "{body}");
    assert_eq!(notes[0]["accountId"], account_id.to_string(), "{body}");
    assert_eq!(notes[0]["subject"], "Maintenance", "{body}");
    assert_eq!(notes[0]["icon"], "calendar", "{body}");
    assert_eq!(notes[0]["notificationType"], "active", "{body}");
    assert!(notes[0]["startAt"].is_string(), "{body}");

    // Non-admin sees none of the account-wide announcements. Canvas types
    // the field `[AccountNotification!]!` (never null), so the port answers
    // with an empty list rather than null.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        query,
        json!({ "id": account_id.to_string() }),
    )
    .await;
    assert_eq!(
        body["data"]["accountNotifications"]
            .as_array()
            .map(Vec::len),
        Some(0),
        "{body}"
    );

    // Cleanup the account.
    let _ = sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(account_id)
        .execute(&pool)
        .await;
    cleanup(&pool, &prefix).await;
}
