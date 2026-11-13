//! Integration tests for the terms/sections GraphQL slice (issue #27).
//!
//! Same harness and conventions as `graphql_integration.rs`: real Postgres
//! via DATABASE_URL (SKIP-clean without it), unique run prefix, full
//! teardown, raw JSON `{ "query", "variables" }` POSTs with Bearer tokens.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness (copied from tests/graphql_integration.rs)
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-ts-{}-{}",
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

/// Create an account directly in the DB (terms are account-scoped). Returns
/// account id. Deleted by name prefix in `cleanup`, cascading its terms.
async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO accounts (name)
        VALUES ($1)
        RETURNING id
        "#,
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

/// Create an enrollment term directly in the DB. Returns term id. `terms`
/// (migration 0034) requires account_id + name; start/end are nullable.
async fn seed_term(
    pool: &PgPool,
    account_id: i64,
    name: &str,
    start_at: Option<chrono::DateTime<chrono::Utc>>,
    end_at: Option<chrono::DateTime<chrono::Utc>>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO terms (account_id, name, start_at, end_at, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id
        "#,
    )
    .bind(account_id)
    .bind(name)
    .bind(start_at)
    .bind(end_at)
    .fetch_one(pool)
    .await
    .expect("seed term")
}

/// Create a course section directly in the DB. Returns section id.
/// `course_sections` (migrations 0034/0047) requires course_id + name.
async fn seed_section(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO course_sections (course_id, name, workflow_state)
        VALUES ($1, $2, 'active')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed section")
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run: enrollments + users, courses
/// (cascading their sections), and accounts (cascading their terms).
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
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
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
// enrollmentTerm: camelCase shape + unknown-id null
// ---------------------------------------------------------------------------

#[tokio::test]
async fn term_query_returns_seeded_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "TermAccount").await;
    let term = seed_term(&pool, account, "Fall 2026", Some(chrono::Utc::now()), None).await;

    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { enrollmentTerm(id: $id) \
         { id name startAt endAt state createdAt updatedAt } }",
        json!({ "id": term.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    assert_eq!(body["data"]["enrollmentTerm"]["id"], term.to_string());
    assert_eq!(body["data"]["enrollmentTerm"]["name"], "Fall 2026");
    assert_eq!(body["data"]["enrollmentTerm"]["state"], "active");
    assert!(
        body["data"]["enrollmentTerm"]["startAt"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "startAt should be the seeded ISO timestamp: {body}"
    );
    assert!(
        body["data"]["enrollmentTerm"]["endAt"].is_null(),
        "endAt is nullable and was not seeded: {body}"
    );
    assert!(
        body["data"]["enrollmentTerm"]["createdAt"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "createdAt should be an ISO timestamp: {body}"
    );
    assert!(
        body["data"]["enrollmentTerm"]["updatedAt"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "updatedAt should be an ISO timestamp: {body}"
    );

    // Unknown ids resolve to null, not an error — like Canvas.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { enrollmentTerm(id: $id) { id } }",
        json!({ "id": "999999999" }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unknown id is null, not an error: {body}"
    );
    assert!(body["data"]["enrollmentTerm"].is_null(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// courseSection: visible to enrolled student, teacher, and admin
// ---------------------------------------------------------------------------

#[tokio::test]
async fn section_query_visible_to_enrolled_teacher_and_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "SectionCourse").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let section = seed_section(&pool, course, "Section 1").await;

    let query = "query ($id: ID!) { courseSection(id: $id) \
                 { id name courseId state createdAt updatedAt } }";

    // Enrolled student (any role) sees the section, camelCase fields intact.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        query,
        json!({ "id": section.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    assert_eq!(body["data"]["courseSection"]["id"], section.to_string());
    assert_eq!(body["data"]["courseSection"]["name"], "Section 1");
    assert_eq!(
        body["data"]["courseSection"]["courseId"],
        course.to_string()
    );
    assert_eq!(body["data"]["courseSection"]["state"], "active");
    assert!(
        body["data"]["courseSection"]["createdAt"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "createdAt should be an ISO timestamp: {body}"
    );
    assert!(
        body["data"]["courseSection"]["updatedAt"]
            .as_str()
            .is_some_and(|v| !v.is_empty()),
        "updatedAt should be an ISO timestamp: {body}"
    );

    // Teacher of the course sees it too.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": section.to_string() }),
    )
    .await;
    assert_eq!(body["data"]["courseSection"]["name"], "Section 1");

    // Admin sees it without an enrollment.
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        query,
        json!({ "id": section.to_string() }),
    )
    .await;
    assert_eq!(body["data"]["courseSection"]["name"], "Section 1");

    // Unknown ids resolve to null, not an error.
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        query,
        json!({ "id": "999999999" }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unknown id is null, not an error: {body}"
    );
    assert!(body["data"]["courseSection"].is_null(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// courseSection: null for an outsider (no enrollment, not admin)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn section_query_null_for_outsider() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "HiddenSectionCourse").await;
    let section = seed_section(&pool, course, "Hidden Section").await;

    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        "query ($id: ID!) { courseSection(id: $id) { id name } }",
        json!({ "id": section.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unauthorized visibility is null, not an error: {body}"
    );
    assert!(body["data"]["courseSection"].is_null(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// enrollmentTerm: any-authenticated semantics (plain student, no enrollments)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn term_query_readable_by_plain_student() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // A plain student with no enrollments and no admin role still reads the
    // term shape (GraphQL parity with Canvas's authenticated term reads).
    let student = seed_user(&pool, &prefix, "PlainStudent", "student").await;
    let account = seed_account(&pool, &prefix, "PlainTermAccount").await;
    let term = seed_term(&pool, account, "Spring 2027", None, None).await;

    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { enrollmentTerm(id: $id) { id name startAt endAt } }",
        json!({ "id": term.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    assert_eq!(body["data"]["enrollmentTerm"]["id"], term.to_string());
    assert_eq!(body["data"]["enrollmentTerm"]["name"], "Spring 2027");
    assert!(
        body["data"]["enrollmentTerm"]["startAt"].is_null(),
        "startAt is nullable and was not seeded: {body}"
    );
    assert!(
        body["data"]["enrollmentTerm"]["endAt"].is_null(),
        "endAt is nullable and was not seeded: {body}"
    );

    cleanup(&pool, &prefix).await;
}
