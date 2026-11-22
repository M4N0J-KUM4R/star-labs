//! Integration tests for the ExternalTool family +
//! `Course.externalToolsConnection` (GraphQL parity gap completion).
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
        "gql-et-{}-{}",
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

#[allow(clippy::too_many_arguments)]
async fn seed_tool(
    pool: &PgPool,
    course_id: i64,
    name: &str,
    enabled: bool,
    state: &str,
    config: Value,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO external_tools (course_id, name, url, enabled, workflow_state, tool_configuration)
        VALUES ($1, $2, 'https://tool.example.com/launch', $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(name)
    .bind(enabled)
    .bind(state)
    .bind(config)
    .fetch_one(pool)
    .await
    .expect("seed tool")
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
async fn external_tools_connection_shape_and_filters() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "ToolCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // Live tool with two placements.
    let live = seed_tool(
        &pool,
        course,
        "Homework Helper",
        true,
        "active",
        json!({
            "placements": {
                "course_assignments_menu": { "url": "https://tool.example.com/menu", "message": "Open helper" },
                "link_selection": { "url": "https://tool.example.com/link" }
            }
        }),
    )
    .await;
    // Disabled tool: filtered.
    seed_tool(&pool, course, "Disabled", false, "active", json!({})).await;
    // Deleted tool: filtered.
    seed_tool(&pool, course, "Deleted", true, "deleted", json!({})).await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { externalToolsConnection(first: 10) \
         { edges { cursor node { id name description url domain workflowState \
         placements { placements { placementType url message iconUrl } } \
         createdAt updatedAt } } pageInfo { hasNextPage endCursor } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    let conn = &body["data"]["course"]["externalToolsConnection"];
    let edges = conn["edges"].as_array().expect("edges");
    assert_eq!(edges.len(), 1, "disabled+deleted filtered: {body}");
    let node = &edges[0]["node"];
    assert_eq!(node["id"], live.to_string(), "{body}");
    assert_eq!(node["name"], "Homework Helper", "{body}");
    assert_eq!(node["url"], "https://tool.example.com/launch", "{body}");
    assert_eq!(node["workflowState"], "active", "{body}");
    assert!(node["description"].is_null(), "{body}");
    assert!(node["domain"].is_null(), "{body}");
    assert!(node["createdAt"].is_string(), "{body}");

    // Placements: key-sorted, url/message surfaced, iconUrl null.
    let placements = node["placements"]["placements"]
        .as_array()
        .expect("placements");
    assert_eq!(placements.len(), 2, "{body}");
    assert_eq!(
        placements[0]["placementType"], "course_assignments_menu",
        "sorted by key: {body}"
    );
    assert_eq!(
        placements[0]["url"], "https://tool.example.com/menu",
        "{body}"
    );
    assert_eq!(placements[0]["message"], "Open helper", "{body}");
    assert!(placements[0]["iconUrl"].is_null(), "{body}");
    assert_eq!(placements[1]["placementType"], "link_selection", "{body}");
    assert!(placements[1]["message"].is_null(), "{body}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], false, "{body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn external_tools_empty_config_resolves_empty_placements() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "BareCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_tool(&pool, course, "Bare", true, "active", json!({})).await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { externalToolsConnection(first: 10) \
         { edges { node { name placements { placements { placementType } } } } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    let node = &body["data"]["course"]["externalToolsConnection"]["edges"][0]["node"];
    assert_eq!(node["name"], "Bare", "{body}");
    let placements = node["placements"]["placements"]
        .as_array()
        .expect("not null");
    assert!(placements.is_empty(), "empty config → empty list: {body}");

    cleanup(&pool, &prefix).await;
}
