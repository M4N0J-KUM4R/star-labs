//! Integration tests for the AssignmentGroup type +
//! `Course.assignmentGroupsConnection` (GraphQL parity gap completion).
//!
//! Mirrors the conventions of `assignments_connection_integration.rs`:
//! mounted at `/api/graphql` through axum-test against a real Postgres,
//! raw JSON bodies, unique-per-run email prefix with full teardown, SKIP
//! when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-asgg-{}-{}",
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

/// Create an assignment group directly; returns its id.
async fn seed_group(pool: &PgPool, course_id: i64, name: &str, position: i32, weight: f64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignment_groups (course_id, name, position, group_weight)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(name)
    .bind(position)
    .bind(weight)
    .fetch_one(pool)
    .await
    .expect("seed group")
}

async fn seed_rules(
    pool: &PgPool,
    group_id: i64,
    drop_lowest: i32,
    drop_highest: i32,
    never_drop: &[i64],
) {
    sqlx::query(
        r#"
        INSERT INTO assignment_group_rules
            (assignment_group_id, drop_lowest, drop_highest, never_drop)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(group_id)
    .bind(drop_lowest)
    .bind(drop_highest)
    .bind(never_drop)
    .execute(pool)
    .await
    .expect("seed rules");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

/// Groups/rules cascade on course deletion; users/enrollments by prefix.
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

// ---------------------------------------------------------------------------
// Field-name parity: camelCased Canvas shape + rules join
// ---------------------------------------------------------------------------

#[tokio::test]
async fn assignment_groups_connection_returns_canvas_field_names() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "GroupCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, course, "Essays", 1, 40.0).await;
    let asg1 = sqlx::query_scalar::<_, i64>(
        "INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, $2, 10) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-keep-me"))
    .fetch_one(&pool)
    .await
    .expect("seed asg for never_drop");
    seed_rules(&pool, group, 1, 2, &[asg1]).await;
    // A second group with no rules row → rules must resolve to null.
    seed_group(&pool, course, "NoRules", 2, 0.0).await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { assignmentGroupsConnection(first: 10) \
         { edges { cursor node { id courseId name position groupWeight \
         rules { dropLowest dropHighest neverDrop } createdAt updatedAt } } \
         pageInfo { hasNextPage endCursor } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    let conn = &body["data"]["course"]["assignmentGroupsConnection"];
    let edges = conn["edges"].as_array().expect("edges");
    assert_eq!(edges.len(), 2, "both groups: {body}");
    let first = &edges[0]["node"];
    assert_eq!(first["id"], group.to_string(), "position order: {body}");
    assert_eq!(first["courseId"], course.to_string());
    assert_eq!(first["name"], "Essays");
    assert_eq!(first["position"], 1);
    assert_eq!(first["groupWeight"], 40.0);
    assert_eq!(first["rules"]["dropLowest"], 1, "{body}");
    assert_eq!(first["rules"]["dropHighest"], 2, "{body}");
    assert_eq!(
        first["rules"]["neverDrop"],
        json!([asg1.to_string()]),
        "neverDrop ids as GraphQL ID strings: {body}"
    );
    assert!(first["createdAt"].is_string(), "{body}");
    assert!(first["updatedAt"].is_string(), "{body}");
    // LEFT JOIN semantics: no rules row → rules null.
    assert!(
        edges[1]["node"]["rules"].is_null(),
        "group without rules row: {body}"
    );
    assert!(conn["pageInfo"]["hasNextPage"].is_boolean(), "{body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Deleted groups are invisible; outsiders get a null course
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleted_groups_hidden_and_outsider_gets_null_course() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "DelCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let live = seed_group(&pool, course, "Live", 1, 0.0).await;
    let dead = seed_group(&pool, course, "Dead", 2, 0.0).await;
    sqlx::query("UPDATE assignment_groups SET workflow_state = 'deleted' WHERE id = $1")
        .bind(dead)
        .execute(&pool)
        .await
        .expect("soft delete group");

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { assignmentGroupsConnection(first: 10) \
         { edges { node { id name } } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    let edges = body["data"]["course"]["assignmentGroupsConnection"]["edges"]
        .as_array()
        .expect("edges");
    assert_eq!(edges.len(), 1, "only the live group: {body}");
    assert_eq!(edges[0]["node"]["id"], live.to_string(), "{body}");

    // Outsider without enrollment: course itself is null.
    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        "query ($id: ID!) { course(id: $id) { assignmentGroupsConnection(first: 10) \
         { edges { node { id } } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(body["data"]["course"].is_null(), "{body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Pagination across two pages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn assignment_groups_connection_paginates() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PageCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let mut ids = Vec::new();
    for i in 0..3 {
        ids.push(seed_group(&pool, course, &format!("G{i}"), i + 1, 0.0).await);
    }

    let token = auth(teacher, "teacher");
    let query = "query ($id: ID!, $first: Int, $after: String) { course(id: $id) \
                 { assignmentGroupsConnection(first: $first, after: $after) \
                 { edges { cursor node { id } } pageInfo { hasNextPage endCursor } } } }";

    let page1 = gql_post(
        &server,
        &token,
        query,
        json!({ "id": course.to_string(), "first": 2 }),
    )
    .await;
    let conn = &page1["data"]["course"]["assignmentGroupsConnection"];
    let e1 = conn["edges"].as_array().expect("page1 edges");
    assert_eq!(e1.len(), 2, "{page1}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], true, "{page1}");
    assert_eq!(
        conn["pageInfo"]["endCursor"], e1[1]["cursor"],
        "endCursor is the last edge's cursor: {page1}"
    );
    assert_eq!(e1[0]["node"]["id"], ids[0].to_string(), "{page1}");
    assert_eq!(e1[1]["node"]["id"], ids[1].to_string(), "{page1}");

    let page2 = gql_post(
        &server,
        &token,
        query,
        json!({
            "id": course.to_string(),
            "first": 2,
            "after": conn["pageInfo"]["endCursor"].as_str()
        }),
    )
    .await;
    let conn2 = &page2["data"]["course"]["assignmentGroupsConnection"];
    let e2 = conn2["edges"].as_array().expect("page2 edges");
    assert_eq!(e2.len(), 1, "{page2}");
    assert_eq!(conn2["pageInfo"]["hasNextPage"], false, "{page2}");
    assert_eq!(e2[0]["node"]["id"], ids[2].to_string(), "{page2}");

    cleanup(&pool, &prefix).await;
}
