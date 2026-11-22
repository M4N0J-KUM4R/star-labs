//! Integration tests for GradingPeriodGroup + GradingPeriod and
//! `Course.gradingPeriods` (GraphQL parity gap completion).
//!
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
        "gql-gp-{}-{}",
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
    let prefix = run_prefix();
    Some((server, pool, prefix))
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

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO enrollments (user_id, course_id, role, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

async fn seed_group(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    weighted: bool,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO grading_period_groups (course_id, title, weighted)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(weighted)
    .fetch_one(pool)
    .await
    .expect("seed grading period group")
}

#[allow(clippy::too_many_arguments)]
async fn seed_period(
    pool: &PgPool,
    group_id: i64,
    title: &str,
    start: &str,
    end: &str,
    close: &str,
    weight: Option<f64>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO grading_periods
            (grading_period_group_id, title, start_date, end_date, close_date, weight)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(group_id)
    .bind(title)
    .bind(start)
    .bind(end)
    .bind(close)
    .bind(weight)
    .fetch_one(pool)
    .await
    .expect("seed grading period")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

/// Periods/groups cascade on course deletion; users by prefix.
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
async fn grading_periods_returns_canvas_field_names() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "GPCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, course, "Semester Set", true).await;
    // Inserted out of order: Q2 first, then Q1 — the connection must sort
    // by start_date (Canvas GradingPeriodGroup.grading_periods ordering).
    let q2 = seed_period(&pool, group, "Q2", "2026-01-01", "2026-03-31", "2026-04-05", Some(50.0)).await;
    let q1 = seed_period(&pool, group, "Q1", "2025-09-01", "2025-12-15", "2025-12-20", Some(50.0)).await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { gradingPeriods { id title weighted \
         createdAt updatedAt gradingPeriods { id title startDate endDate closeDate \
         weight createdAt updatedAt } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    let group_node = &body["data"]["course"]["gradingPeriods"];
    assert_eq!(group_node["id"], group.to_string(), "{body}");
    assert_eq!(group_node["title"], "Semester Set", "{body}");
    assert_eq!(group_node["weighted"], true, "{body}");
    assert!(group_node["createdAt"].is_string(), "{body}");
    assert!(group_node["updatedAt"].is_string(), "{body}");

    let periods = group_node["gradingPeriods"].as_array().expect("periods");
    assert_eq!(periods.len(), 2, "{body}");
    assert_eq!(periods[0]["id"], q1.to_string(), "start-date order: {body}");
    assert_eq!(periods[1]["id"], q2.to_string(), "{body}");
    let first = &periods[0];
    assert_eq!(first["title"], "Q1", "{body}");
    assert_eq!(first["weight"], 50.0, "{body}");
    assert!(
        first["startDate"].as_str().unwrap().starts_with("2025-09-01"),
        "{body}"
    );
    assert!(
        first["endDate"].as_str().unwrap().starts_with("2025-12-15"),
        "{body}"
    );
    assert!(
        first["closeDate"].as_str().unwrap().starts_with("2025-12-20"),
        "{body}"
    );
    assert!(first["createdAt"].is_string(), "{body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grading_periods_null_without_group_and_deleted_filtered() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let bare = seed_course(&pool, &prefix, "BareCourse").await;
    let with_group = seed_course(&pool, &prefix, "GrpCourse").await;
    seed_enrollment(&pool, teacher, bare, "teacher").await;
    seed_enrollment(&pool, teacher, with_group, "teacher").await;

    // No group → gradingPeriods is null (GradingPeriodGroup.for_course nil).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { gradingPeriods { id } } }",
        json!({ "id": bare.to_string() }),
    )
    .await;
    assert!(body["data"]["course"]["gradingPeriods"].is_null(), "{body}");

    // Deleted period filtered out of its group's list.
    let group = seed_group(&pool, with_group, "Set", false).await;
    let live = seed_period(&pool, group, "Live", "2026-01-01", "2026-02-01", "2026-02-05", None).await;
    let dead = seed_period(&pool, group, "Dead", "2026-03-01", "2026-04-01", "2026-04-05", None).await;
    sqlx::query("UPDATE grading_periods SET workflow_state = 'deleted' WHERE id = $1")
        .bind(dead)
        .execute(&pool)
        .await
        .expect("soft delete period");

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { gradingPeriods { weighted \
         gradingPeriods { id title weight } } } }",
        json!({ "id": with_group.to_string() }),
    )
    .await;
    let group_node = &body["data"]["course"]["gradingPeriods"];
    assert_eq!(group_node["weighted"], false, "{body}");
    let periods = group_node["gradingPeriods"].as_array().expect("periods");
    assert_eq!(periods.len(), 1, "deleted period hidden: {body}");
    assert_eq!(periods[0]["id"], live.to_string(), "{body}");
    // Unweighted set: weight is null (Canvas serializes the nil column).
    assert!(periods[0]["weight"].is_null(), "{body}");

    cleanup(&pool, &prefix).await;
}
