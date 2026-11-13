//! Integration tests for star-graphql.
//!
//! Exercises the GraphQL router mounted exactly like star-http does
//! (`/api/graphql`) through axum-test against a real Postgres, using raw
//! JSON `{ "query": ..., "variables": ... }` POST bodies (and one GET) like a
//! real GraphQL client.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-graphql
//! - Uses a unique email prefix per run + full teardown so it is safe to run
//!   repeatedly against a shared/dev database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness (copied from star-enrollments/tests/enrollments_integration.rs)
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the GraphQL router
/// mounted the same way star-http mounts it. Returns None when DATABASE_URL
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
        jwt_secret: "test-jwt-secret".to_string(),
    };

    // Same nesting as star-http main.rs: /api/graphql outside /api/v1.
    let app = axum::Router::new()
        .nest("/api/graphql", star_graphql::router())
        .with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
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
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
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

/// Give a user an active enrollment in a course with the given role.
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

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run (enrollments + users cascade, courses
/// cascade their sections).
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

/// POST a raw JSON GraphQL request `{ query, variables }` with the given
/// bearer token. Returns the full JSON body (data + errors).
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
// Unauthenticated requests are 401 before execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unauthenticated_request_is_rejected_with_401() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // POST without an Authorization header: rejected before execution, so the
    // body must NOT be a GraphQL response (no "data"/"errors" JSON shape).
    let resp = server
        .post("/api/graphql")
        .json(&json!({ "query": "{ __typename }" }))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    assert!(
        !resp.text().contains("\"errors\""),
        "unauthenticated must not be a GraphQL error: {}",
        resp.text()
    );

    // GET without an Authorization header is rejected the same way.
    let resp = server
        .get("/api/graphql?query=%7B%20__typename%20%7D")
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // A garbage token is rejected too.
    let resp = server
        .post("/api/graphql")
        .add_header(axum::http::header::AUTHORIZATION, "Bearer not-a-token")
        .json(&json!({ "query": "{ __typename }" }))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// course(id) returns seeded fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_query_returns_seeded_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "GqlCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { id name state createdAt updatedAt } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    assert_eq!(body["data"]["course"]["id"], course.to_string());
    assert_eq!(body["data"]["course"]["name"], "GqlCourse");
    assert_eq!(body["data"]["course"]["state"], "available");
    assert!(
        body["data"]["course"]["createdAt"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "createdAt should be an ISO timestamp: {body}"
    );
    assert!(
        body["data"]["course"]["updatedAt"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "updatedAt should be an ISO timestamp: {body}"
    );

    cleanup(&pool, &prefix).await;
}

/// A student with no enrollment in the course (and not admin) gets null,
/// mirroring Canvas GraphQL visibility.
#[tokio::test]
async fn course_query_requires_enrollment_or_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "HiddenCourse").await;

    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        "query ($id: ID!) { course(id: $id) { id name } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unauthorized visibility is null, not an error: {body}"
    );
    assert!(body["data"]["course"].is_null(), "body: {body}");

    // Admin sees the course without an enrollment.
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        "query ($id: ID!) { course(id: $id) { id name } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    assert_eq!(body["data"]["course"]["name"], "HiddenCourse");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Roster visibility: student sees only own enrollment, teacher sees all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn student_sees_only_own_enrollment_in_course_connection() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "RosterCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other, course, "student").await;

    let query = "query ($id: ID!) { course(id: $id) { enrollmentsConnection(first: 10) \
                 { edges { cursor node { userId role state } } \
                 pageInfo { hasNextPage endCursor } } } }";

    // Student: only their own enrollment comes back.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let edges = body["data"]["course"]["enrollmentsConnection"]["edges"]
        .as_array()
        .expect("edges array");
    assert_eq!(edges.len(), 1, "student sees only own enrollment: {body}");
    assert_eq!(edges[0]["node"]["userId"], student.to_string());
    assert_eq!(edges[0]["node"]["role"], "student");
    assert_eq!(edges[0]["node"]["state"], "active");
    assert!(
        edges[0]["cursor"].as_str().is_some_and(|c| !c.is_empty()),
        "edge cursor must be present: {body}"
    );

    // Teacher: the whole roster comes back.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    let edges = body["data"]["course"]["enrollmentsConnection"]["edges"]
        .as_array()
        .expect("edges array");
    assert_eq!(edges.len(), 3, "teacher sees all enrollments: {body}");
    let user_ids: Vec<&str> = edges
        .iter()
        .map(|e| e["node"]["userId"].as_str().expect("userId string"))
        .collect();
    for expected in [teacher, student, other] {
        assert!(
            user_ids.contains(&expected.to_string().as_str()),
            "teacher roster must contain user {expected}: {body}"
        );
    }

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Cursor pagination across two pages (first/after)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enrollments_connection_paginates_across_two_pages() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PagedCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    // Three students, enrolled in id order (ORDER BY id ascending).
    let mut students = vec![];
    for n in 1..=3 {
        let student = seed_user(&pool, &prefix, &format!("Paged{n}"), "student").await;
        seed_enrollment(&pool, student, course, "student").await;
        students.push(student);
    }

    let query = "query ($id: ID!, $first: Int, $after: String) \
                 { course(id: $id) { enrollmentsConnection(first: $first, after: $after) \
                 { edges { cursor node { userId } } pageInfo { hasNextPage endCursor } } } }";

    // Page 1: first=2 over the whole roster (4 enrollments) -> 2 edges + next.
    let page1 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2 }),
    )
    .await;
    let conn = &page1["data"]["course"]["enrollmentsConnection"];
    let edges = conn["edges"].as_array().expect("page1 edges");
    assert_eq!(edges.len(), 2, "page1: {page1}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], true, "page1: {page1}");
    let end_cursor = conn["pageInfo"]["endCursor"]
        .as_str()
        .expect("page1 endCursor")
        .to_string();

    // Page 2: after the first page's cursor -> remaining 2 enrollments.
    let page2 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2, "after": end_cursor }),
    )
    .await;
    let conn = &page2["data"]["course"]["enrollmentsConnection"];
    let edges2 = conn["edges"].as_array().expect("page2 edges");
    assert_eq!(edges2.len(), 2, "page2: {page2}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], false, "page2: {page2}");

    // The two pages together cover the roster exactly once, in id order.
    let page1_ids: Vec<&str> = edges
        .iter()
        .map(|e| e["node"]["userId"].as_str().expect("userId"))
        .collect();
    let page2_ids: Vec<&str> = edges2
        .iter()
        .map(|e| e["node"]["userId"].as_str().expect("userId"))
        .collect();
    let all: Vec<&str> = page1_ids.iter().chain(page2_ids.iter()).copied().collect();
    assert_eq!(all.len(), 4, "no overlap between pages: {all:?}");
    assert!(
        all.contains(&teacher.to_string().as_str()),
        "teacher enrollment missing: {all:?}"
    );
    for student in &students {
        assert!(
            all.contains(&student.to_string().as_str()),
            "student {student} missing: {all:?}"
        );
    }

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// User.enrollmentsConnection: self-or-admin, camelCase shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_enrollments_connection_is_self_or_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "SelfUser", "student").await;
    let other = seed_user(&pool, &prefix, "OtherUser", "student").await;
    let admin = seed_user(&pool, &prefix, "AdminUser", "admin").await;
    let course1 = seed_course(&pool, &prefix, "UserCourse1").await;
    let course2 = seed_course(&pool, &prefix, "UserCourse2").await;
    seed_enrollment(&pool, student, course1, "student").await;
    seed_enrollment(&pool, student, course2, "student").await;
    seed_enrollment(&pool, other, course2, "student").await;

    let query = "query ($id: ID!) { user(id: $id) { id name email sisUserId createdAt \
                 enrollmentsConnection(first: 10) { edges { cursor node { courseId role state } } \
                 pageInfo { hasNextPage endCursor } } } }";

    // Self: sees own enrollments, camelCase fields populated.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        query,
        json!({ "id": student.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    assert_eq!(body["data"]["user"]["id"], student.to_string());
    assert_eq!(body["data"]["user"]["name"], "SelfUser");
    assert_eq!(
        body["data"]["user"]["email"],
        format!("{prefix}-SelfUser@test.local")
    );
    assert!(
        body["data"]["user"]["sisUserId"].is_null(),
        "no SIS columns yet: {body}"
    );
    let edges = body["data"]["user"]["enrollmentsConnection"]["edges"]
        .as_array()
        .expect("self edges");
    assert_eq!(edges.len(), 2, "self sees own enrollments: {body}");
    let course_ids: Vec<&str> = edges
        .iter()
        .map(|e| e["node"]["courseId"].as_str().expect("courseId"))
        .collect();
    assert!(course_ids.contains(&course1.to_string().as_str()));
    assert!(course_ids.contains(&course2.to_string().as_str()));

    // Another (non-admin) user: profile visible, connection null.
    let body = gql_post(
        &server,
        &auth(other, "student"),
        query,
        json!({ "id": student.to_string() }),
    )
    .await;
    assert!(
        body["data"]["user"]["enrollmentsConnection"].is_null(),
        "non-admin others get null: {body}"
    );

    // Admin: sees the user's enrollments too.
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        query,
        json!({ "id": student.to_string() }),
    )
    .await;
    let edges = body["data"]["user"]["enrollmentsConnection"]["edges"]
        .as_array()
        .expect("admin edges");
    assert_eq!(edges.len(), 2, "admin sees the user's enrollments: {body}");

    cleanup(&pool, &prefix).await;
}
