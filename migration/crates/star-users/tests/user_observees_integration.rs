//! Integration tests for user_observees routes in star-users:
//! - GET    /users/{userId}/observers
//! - GET    /users/{userId}/observees
//! - POST   /users/{userId}/observees
//! - GET    /users/{userId}/observees/{observeeId}
//! - GET    /users/{userId}/observers/{observerId}
//! - PUT    /users/{userId}/observees/{observeeId}
//! - DELETE /users/{userId}/observees/{observeeId}
//!
//! Conventions (mirrors users_integration.rs):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-users
//! - Uses a unique email prefix per run + full teardown so it is safe to run
//!   repeatedly against a shared/dev database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "usr-obs-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the users router
/// exactly the way star-http assembles it. Returns None when DATABASE_URL is
/// unset so callers can skip cleanly.
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

    let server = TestServer::new(star_users_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same merge pattern as star-http::build_api_router, restricted to the
/// users router.
fn star_users_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_users::router())
        .with_state(state)
}

/// Create a user directly in the DB with a deterministic password hash and
/// the given role.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(sortable_name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run (users cascade to their dependent
/// rows).
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET /users/{userId}/observees  (self)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_observees_returns_empty_for_user_with_no_links() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice Observer", "student").await;

    let resp = server
        .get(&format!("/users/{alice}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(
        body.as_array().unwrap().is_empty(),
        "should be empty: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_observees_returns_linked_observees() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Parent Observer", "student").await;
    let child = seed_user(&pool, &prefix, "Child Student", "student").await;

    // Link observer -> child (global, no course).
    let resp = server
        .post(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "user_id": child }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 1, "should have 1 observee: {body}");
    assert_eq!(arr[0]["id"], child);
    assert_eq!(arr[0]["name"], "Child Student");
    assert!(arr[0]["enrollments"].as_array().unwrap()[0]["course_id"].is_null());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_observees_requires_auth_or_is_forbidden() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "student").await;

    // Unauthenticated.
    assert_eq!(
        server
            .get(&format!("/users/{alice}/observees"))
            .await
            .status_code(),
        401
    );

    // Another non-observer/non-self user is forbidden.
    let bob = seed_user(&pool, &prefix, "Bob Stranger", "student").await;
    let resp = server
        .get(&format!("/users/{alice}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_observees_404_for_unknown_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .get("/users/999999999/observees")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /users/{userId}/observees
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_observee_links_observer_to_target_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let child = seed_user(&pool, &prefix, "Child", "student").await;

    let resp = server
        .post(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "user_id": child }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], child);
    assert_eq!(body["name"], "Child");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_observee_404_for_unknown_observee() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;

    let resp = server
        .post(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "user_id": 999999999 }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_observee_idempotent_on_duplicate_link() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let child = seed_user(&pool, &prefix, "Child", "student").await;

    for _ in 0..2 {
        let resp = server
            .post(&format!("/users/{observer}/observees"))
            .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
            .json(&json!({ "user_id": child }))
            .await;
        assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    }

    // Exactly one link should exist.
    let resp = server
        .get(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    let body: Value = resp.json();
    assert_eq!(
        body.as_array().unwrap().len(),
        1,
        "should be exactly 1: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /users/{userId}/observees/{observeeId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_single_observee_returns_correct_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let child = seed_user(&pool, &prefix, "Child", "student").await;

    // Create the link.
    let _ = server
        .post(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "user_id": child }))
        .await;

    let resp = server
        .get(&format!("/users/{observer}/observees/{child}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], child);
    assert_eq!(body["name"], "Child");
    assert!(body["enrollments"].as_array().is_some());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_single_observee_404_when_no_link_exists() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;

    let resp = server
        .get(&format!("/users/{observer}/observees/{stranger}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /users/{userId}/observers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_observers_returns_users_who_watch_this_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer1 = seed_user(&pool, &prefix, "Observer One", "student").await;
    let observer2 = seed_user(&pool, &prefix, "Observer Two", "student").await;
    let child = seed_user(&pool, &prefix, "Child Student", "student").await;

    // Both observers link to the same child.
    for obs in [observer1, observer2] {
        let resp = server
            .post(&format!("/users/{obs}/observees"))
            .add_header(axum::http::header::AUTHORIZATION, auth(obs, "student"))
            .json(&json!({ "user_id": child }))
            .await;
        assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    }

    // The child's observers list.
    let resp = server
        .get(&format!("/users/{child}/observers"))
        .add_header(axum::http::header::AUTHORIZATION, auth(child, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 2, "should have 2 observers: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_observers_forbidden_for_unrelated_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let child = seed_user(&pool, &prefix, "Child", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;

    // Link observer -> child.
    let _ = server
        .post(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "user_id": child }))
        .await;

    // Stranger trying to list child's observers is forbidden.
    let resp = server
        .get(&format!("/users/{child}/observers"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /users/{userId}/observers/{observerId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_single_observer_returns_correct_user_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let child = seed_user(&pool, &prefix, "Child", "student").await;

    let _ = server
        .post(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "user_id": child }))
        .await;

    let resp = server
        .get(&format!("/users/{child}/observers/{observer}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(child, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], observer);
    assert_eq!(body["name"], "Observer");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_single_observer_404_when_no_link() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let child = seed_user(&pool, &prefix, "Child", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;

    let resp = server
        .get(&format!("/users/{child}/observers/{stranger}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(child, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /users/{userId}/observees/{observeeId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_observee_changes_course_scope() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Create a course first (star-enrollments creates courses in tests, but
    // we need one here for the course-scoped link).
    let course_id: i64 = sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id, workflow_state) \
         VALUES ('Test Course', 'TC', 1, 'available') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("seed course");

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let child = seed_user(&pool, &prefix, "Child", "student").await;

    // Create a global link first.
    let _ = server
        .post(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "user_id": child }))
        .await;

    // Update it to be course-scoped.
    let resp = server
        .put(&format!("/users/{observer}/observees/{child}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "course_id": course_id }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["enrollments"][0]["course_id"], course_id);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_observee_404_for_nonexistent_link() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;

    let resp = server
        .put(&format!("/users/{observer}/observees/{stranger}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "course_id": null }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /users/{userId}/observees/{observeeId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_observee_removes_the_link() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let child = seed_user(&pool, &prefix, "Child", "student").await;

    // Create the link.
    let _ = server
        .post(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "user_id": child }))
        .await;

    // Delete it.
    let resp = server
        .delete(&format!("/users/{observer}/observees/{child}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // Confirm it's gone.
    let resp = server
        .get(&format!("/users/{observer}/observees/{child}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_observee_404_when_link_does_not_exist() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;

    let resp = server
        .delete(&format!("/users/{observer}/observees/{stranger}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Admin can manage any observee link
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_can_list_observees_for_any_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let observer = seed_user(&pool, &prefix, "Observer", "student").await;
    let child = seed_user(&pool, &prefix, "Child", "student").await;

    // Observer links to child.
    let _ = server
        .post(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(observer, "student"))
        .json(&json!({ "user_id": child }))
        .await;

    // Admin can read observer's observees.
    let resp = server
        .get(&format!("/users/{observer}/observees"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().unwrap().len(), 1, "body: {body}");

    cleanup(&pool, &prefix).await;
}
