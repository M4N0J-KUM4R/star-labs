//! GraphQL mutation integration tests — Batch A: assignment
//! create/update/delete (issue #29).
//!
//! Test list follows `specs/p5-gql-mutations.md` §5 Batch A exactly:
//! - create_assignment_positive_teacher
//! - create_assignment_student_gets_invalid_course (same message for a
//!   nonexistent course id — Canvas conflates deliberately)
//! - create_assignment_outsider_unauthorized
//! - update_assignment_not_found
//! - update_assignment_student_insufficient_permission
//! - update_assignment_delete_via_state_deleted (soft delete + idempotent)
//! - update_assignment_state_deleted_student_insufficient_permission
//! - update_assignment_validation_name_required (payload errors, HTTP 200)
//! plus the shared-extractor 401 check on a mutation.
//!
//! Harness conventions are copied from `graphql_integration.rs`: skip
//! cleanly without DATABASE_URL, unique run prefix, full teardown, real
//! bearer tokens.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness (mirrors graphql_integration.rs)
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-mut-tests-{}-{}",
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

/// Seed an assignment directly (the mutations under test also write rows;
/// this gives tests a known starting state). Returns assignment id.
async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, points_possible)
        VALUES ($1, $2, 10)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
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

async fn cleanup(pool: &PgPool, prefix: &str) {
    // Assignments cascade with their courses; wipe courses by code prefix
    // first is not possible before deleting enrollments (no FK path), so
    // follow the base harness order and rely on ON DELETE CASCADE.
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

/// POST a raw JSON GraphQL request with the given bearer token. Mutations
/// return HTTP 200 even for payload validation errors; execution errors are
/// asserted through the body, not the status.
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

const CREATE_MUTATION: &str = r#"
mutation CreateAssignment($input: CreateAssignmentInput!) {
  createAssignment(input: $input) {
    assignment { id courseId name description pointsPossible }
    errors { attribute message }
  }
}"#;

const UPDATE_MUTATION: &str = r#"
mutation UpdateAssignment($input: UpdateAssignmentInput!) {
  updateAssignment(input: $input) {
    assignment { id courseId name description pointsPossible }
    errors { attribute message }
  }
}"#;

/// Read `assignments.workflow_state` straight from the DB (the column is
/// not exposed on the GraphQL Assignment type yet — spec §2.1 defers it).
async fn db_workflow_state(pool: &PgPool, assignment_id: i64) -> String {
    sqlx::query_scalar("SELECT workflow_state FROM assignments WHERE id = $1")
        .bind(assignment_id)
        .fetch_one(pool)
        .await
        .expect("read workflow_state")
}

// ---------------------------------------------------------------------------
// createAssignment
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_assignment_positive_teacher() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "teacher-a1", "student").await;
    let course = seed_course(&pool, &prefix, "course-a1").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let body = gql_post(
        &server,
        &auth(teacher, "student"),
        CREATE_MUTATION,
        json!({
            "input": {
                "courseId": course.to_string(),
                "name": "Essay 1",
                "description": "write it",
                "pointsPossible": 25.5,
                "state": "PUBLISHED"
            }
        }),
    )
    .await;

    assert!(
        body["errors"].is_null() || body["errors"].as_array().map_or(true, |e| e.is_empty()),
        "unexpected top-level errors: {}",
        body
    );
    let payload = &body["data"]["createAssignment"];
    // async-graphql omits null fields; the payload must carry no errors.
    assert!(payload.get("errors").map_or(true, |e| e.is_null()), "body: {body}");
    let a = &payload["assignment"];
    assert_eq!(a["name"], "Essay 1");
    assert_eq!(a["description"], "write it");
    assert_eq!(a["pointsPossible"], 25.5);
    assert_eq!(a["courseId"], course.to_string());

    let new_id: i64 = a["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(db_workflow_state(&pool, new_id).await, "published");

    // Omitted state defaults to unpublished (Canvas model default).
    let body = gql_post(
        &server,
        &auth(teacher, "student"),
        CREATE_MUTATION,
        json!({ "input": { "courseId": course.to_string(), "name": "Draft task" } }),
    )
    .await;
    let draft_id: i64 = body["data"]["createAssignment"]["assignment"]["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(db_workflow_state(&pool, draft_id).await, "unpublished");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_assignment_student_gets_invalid_course() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "student-a2", "student").await;
    let course = seed_course(&pool, &prefix, "course-a2").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Enrolled student lacks manage_assignments_add.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        CREATE_MUTATION,
        json!({ "input": { "courseId": course.to_string(), "name": "Nope" } }),
    )
    .await;
    assert_eq!(
        body["errors"][0]["message"].as_str(),
        Some(format!("invalid course: {course}").as_str()),
        "body: {body}"
    );
    assert!(body["data"]["createAssignment"].is_null());

    // Nonexistent course id produces the SAME message (Canvas conflates
    // deliberately; no existence leak).
    let ghost = course + 999_999;
    let body = gql_post(
        &server,
        &auth(student, "student"),
        CREATE_MUTATION,
        json!({ "input": { "courseId": ghost.to_string(), "name": "Nope" } }),
    )
    .await;
    assert_eq!(
        body["errors"][0]["message"].as_str(),
        Some(format!("invalid course: {ghost}").as_str()),
        "body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_assignment_outsider_unauthorized() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let outsider = seed_user(&pool, &prefix, "outsider-a3", "student").await;
    let course = seed_course(&pool, &prefix, "course-a3").await;

    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        CREATE_MUTATION,
        json!({ "input": { "courseId": course.to_string(), "name": "Nope" } }),
    )
    .await;
    assert_eq!(
        body["errors"][0]["message"].as_str(),
        Some(format!("invalid course: {course}").as_str()),
        "body: {body}"
    );

    // Global admin may create in any course.
    let admin = seed_user(&pool, &prefix, "admin-a3", "admin").await;
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        CREATE_MUTATION,
        json!({ "input": { "courseId": course.to_string(), "name": "Admin task", "state": "PUBLISHED" } }),
    )
    .await;
    assert_eq!(
        body["data"]["createAssignment"]["assignment"]["name"].as_str(),
        Some("Admin task"),
        "body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// updateAssignment
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_assignment_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "teacher-a4", "teacher").await;
    let missing = 987_654_321_i64;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        UPDATE_MUTATION,
        json!({ "input": { "id": missing.to_string(), "name": "Ghost" } }),
    )
    .await;
    assert_eq!(
        body["errors"][0]["message"].as_str(),
        Some(format!("assignment not found: {missing}").as_str()),
        "body: {body}"
    );
    assert!(body["data"]["updateAssignment"].is_null());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_assignment_student_insufficient_permission() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "student-a5", "student").await;
    let course = seed_course(&pool, &prefix, "course-a5").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Task A5").await;

    let body = gql_post(
        &server,
        &auth(student, "student"),
        UPDATE_MUTATION,
        json!({ "input": { "id": assignment.to_string(), "name": "Hijacked" } }),
    )
    .await;
    assert_eq!(
        body["errors"][0]["message"].as_str(),
        Some("insufficient permission"),
        "body: {body}"
    );

    // Row untouched.
    let name: String = sqlx::query_scalar("SELECT title FROM assignments WHERE id = $1")
        .bind(assignment)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "Task A5");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_assignment_delete_via_state_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "teacher-a6", "teacher").await;
    let course = seed_course(&pool, &prefix, "course-a6").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, "Task A6").await;

    // Soft delete via updateAssignment(state: deleted) — Canvas has no
    // deleteAssignment mutation.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        UPDATE_MUTATION,
        json!({ "input": { "id": assignment.to_string(), "state": "DELETED" } }),
    )
    .await;
    assert_eq!(
        body["data"]["updateAssignment"]["assignment"]["id"].as_str(),
        Some(assignment.to_string().as_str()),
        "body: {body}"
    );
    assert_eq!(db_workflow_state(&pool, assignment).await, "deleted");

    // Idempotent second call returns early, still deleted.
    let updated_at_before: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT updated_at FROM assignments WHERE id = $1",
    )
    .bind(assignment)
    .fetch_one(&pool)
    .await
    .unwrap();
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        UPDATE_MUTATION,
        json!({ "input": { "id": assignment.to_string(), "state": "DELETED" } }),
    )
    .await;
    assert!(body["errors"].is_null() || body["errors"].as_array().map_or(true, |e| e.is_empty()));
    assert_eq!(db_workflow_state(&pool, assignment).await, "deleted");
    let updated_at_after: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT updated_at FROM assignments WHERE id = $1",
    )
    .bind(assignment)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(updated_at_before, updated_at_after, "idempotent delete must not touch updated_at");

    // Restore through published state (ensure_restored path).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        UPDATE_MUTATION,
        json!({ "input": { "id": assignment.to_string(), "state": "PUBLISHED" } }),
    )
    .await;
    assert_eq!(
        body["data"]["updateAssignment"]["assignment"]["name"].as_str(),
        Some("Task A6"),
        "body: {body}"
    );
    assert_eq!(db_workflow_state(&pool, assignment).await, "published");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_assignment_state_deleted_student_insufficient_permission() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "student-a7", "student").await;
    let course = seed_course(&pool, &prefix, "course-a7").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Task A7").await;

    let body = gql_post(
        &server,
        &auth(student, "student"),
        UPDATE_MUTATION,
        json!({ "input": { "id": assignment.to_string(), "state": "DELETED" } }),
    )
    .await;
    assert_eq!(
        body["errors"][0]["message"].as_str(),
        Some("insufficient permission"),
        "body: {body}"
    );
    // Destroy re-check happens before any write: row stays intact.
    assert_eq!(db_workflow_state(&pool, assignment).await, "published");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_assignment_validation_name_required() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "teacher-a8", "teacher").await;
    let course = seed_course(&pool, &prefix, "course-a8").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, "Task A8").await;

    // Blank name: payload errors inside HTTP 200 (gql_post asserts 200).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        UPDATE_MUTATION,
        json!({ "input": { "id": assignment.to_string(), "name": "   " } }),
    )
    .await;
    let payload = &body["data"]["updateAssignment"];
    assert!(payload["assignment"].is_null());
    assert_eq!(payload["errors"][0]["attribute"].as_str(), Some("name"));
    assert_eq!(payload["errors"][0]["message"].as_str(), Some("can't be blank"));

    // Negative points: payload error on points_possible.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        UPDATE_MUTATION,
        json!({ "input": { "id": assignment.to_string(), "pointsPossible": -5 } }),
    )
    .await;
    let payload = &body["data"]["updateAssignment"];
    assert_eq!(
        payload["errors"][0]["attribute"].as_str(),
        Some("points_possible")
    );
    assert_eq!(
        payload["errors"][0]["message"].as_str(),
        Some("must be greater than or equal to 0")
    );

    // Row untouched by both failed updates.
    let name: String = sqlx::query_scalar("SELECT title FROM assignments WHERE id = $1")
        .bind(assignment)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "Task A8");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Unauthenticated mutation calls are rejected before execution (spec §3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unauthenticated_mutation_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server
        .post("/api/graphql")
        .json(&json!({
            "query": CREATE_MUTATION,
            "variables": { "input": { "courseId": "1", "name": "x" } }
        }))
        .await;

    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    // The 401 comes from the AuthUser extractor, never a GraphQL body.
    let text = resp.text();
    assert!(
        !text.contains("\"data\""),
        "401 must not be a GraphQL response shape: {text}"
    );

    cleanup(&pool, &prefix).await;
}
