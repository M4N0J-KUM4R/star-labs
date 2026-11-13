//! Integration tests for the Assignment type + `Course.assignmentsConnection`
//! (issue #26).
//!
//! Mirrors the conventions of `graphql_integration.rs`: mounted at
//! `/api/graphql` through axum-test against a real Postgres, raw JSON
//! `{ "query": ..., "variables": ... }` bodies, unique-per-run email prefix
//! with full teardown, SKIP when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness (copied from tests/graphql_integration.rs)
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-asg-{}-{}",
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

/// Create an assignment directly in the DB. Returns assignment id.
async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str, points: f64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, description, points_possible, due_at)
        VALUES ($1, $2, NULL, $3, now() + interval '7 days')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(points)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Assignments cascade on course deletion; courses/users/enrollments are
/// cleaned by prefix exactly like the foundation tests.
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
// Field-name parity: camelCased Canvas shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn assignments_connection_returns_canvas_field_names() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "AsgCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_assignment(&pool, course, "FieldParity", 12.5).await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { assignmentsConnection(first: 10) \
         { edges { cursor node { id courseId name description pointsPossible dueAt \
         createdAt updatedAt } } pageInfo { hasNextPage endCursor } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let node = &body["data"]["course"]["assignmentsConnection"]["edges"][0]["node"];
    assert_eq!(node["name"], "FieldParity");
    assert_eq!(node["courseId"], course.to_string());
    assert_eq!(node["pointsPossible"], 12.5);
    assert!(
        node["description"].is_null(),
        "seeded NULL description must serialize as null: {body}"
    );
    assert!(
        node["dueAt"].as_str().is_some_and(|v| !v.is_empty()),
        "dueAt should be an ISO timestamp: {body}"
    );
    assert!(
        node["createdAt"].as_str().is_some_and(|v| !v.is_empty()),
        "createdAt should be an ISO timestamp: {body}"
    );
    assert!(
        node["id"].as_str().is_some_and(|v| !v.is_empty()),
        "id should be a string ID: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// REST parity: enrolled student sees all assignments (no published filter)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn student_sees_all_assignments_rest_parity() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "StudentAsgCourse").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_assignment(&pool, course, "Essay", 10.0).await;
    seed_assignment(&pool, course, "Quiz", 5.0).await;

    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { course(id: $id) { assignmentsConnection(first: 10) \
         { edges { node { name } } pageInfo { hasNextPage } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let edges = body["data"]["course"]["assignmentsConnection"]["edges"]
        .as_array()
        .expect("edges array");
    assert_eq!(edges.len(), 2, "REST parity: student sees both: {body}");
    let names: Vec<&str> = edges
        .iter()
        .map(|e| e["node"]["name"].as_str().expect("name string"))
        .collect();
    assert!(
        names.contains(&"Essay") && names.contains(&"Quiz"),
        "{names:?}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Authorization negatives: outsider gets a null course, admin sees
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outsider_gets_null_course_admin_sees_assignments() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "HiddenAsgCourse").await;
    seed_assignment(&pool, course, "Secret", 1.0).await;

    let query = "query ($id: ID!) { course(id: $id) { assignmentsConnection(first: 10) \
                 { edges { node { name } } } } }";

    // Outsider: course itself is null, so the connection is unreachable.
    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unauthorized visibility is null, not an error: {body}"
    );
    assert!(body["data"]["course"].is_null(), "body: {body}");

    // Admin: sees the assignment without an enrollment.
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    let edges = body["data"]["course"]["assignmentsConnection"]["edges"]
        .as_array()
        .expect("admin edges");
    assert_eq!(edges.len(), 1, "admin sees the assignment: {body}");
    assert_eq!(edges[0]["node"]["name"], "Secret");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Cursor pagination across two pages (first/after)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn assignments_connection_paginates_across_two_pages() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PagedAsgCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    // Three assignments, inserted in id order (ORDER BY id ascending).
    for n in 1..=3 {
        seed_assignment(&pool, course, &format!("Paged{n}"), n as f64).await;
    }

    let query = "query ($id: ID!, $first: Int, $after: String) \
                 { course(id: $id) { assignmentsConnection(first: $first, after: $after) \
                 { edges { cursor node { name } } pageInfo { hasNextPage endCursor } } } }";

    // Page 1: first=2 over 3 assignments -> 2 edges + next.
    let page1 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2 }),
    )
    .await;
    let conn = &page1["data"]["course"]["assignmentsConnection"];
    let edges = conn["edges"].as_array().expect("page1 edges");
    assert_eq!(edges.len(), 2, "page1: {page1}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], true, "page1: {page1}");
    let end_cursor = conn["pageInfo"]["endCursor"]
        .as_str()
        .expect("page1 endCursor")
        .to_string();
    assert_eq!(
        edges[1]["cursor"].as_str().expect("edge cursor"),
        end_cursor,
        "endCursor must equal the last edge's cursor: {page1}"
    );

    // Page 2: after the first page's cursor -> the remaining assignment.
    let page2 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2, "after": end_cursor }),
    )
    .await;
    let conn = &page2["data"]["course"]["assignmentsConnection"];
    let edges2 = conn["edges"].as_array().expect("page2 edges");
    assert_eq!(edges2.len(), 1, "page2: {page2}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], false, "page2: {page2}");

    // The two pages together cover every assignment exactly once.
    let mut all: Vec<&str> = edges
        .iter()
        .chain(edges2.iter())
        .map(|e| e["node"]["name"].as_str().expect("name"))
        .collect();
    all.sort_unstable();
    assert_eq!(
        all,
        vec!["Paged1", "Paged2", "Paged3"],
        "pages: {page1} {page2}"
    );

    cleanup(&pool, &prefix).await;
}
