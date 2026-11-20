//! Integration tests for the group membership update aliases.
//!
//! Canvas routes `group_memberships#update` under three paths
//! (config/routes.rb, `scope(controller: :group_memberships)`):
//!
//! - PUT   /groups/:group_id/memberships/:id
//! - PATCH /groups/:group_id/memberships/:id
//! - PUT   /groups/:group_id/users/:id
//! - PATCH /groups/:group_id/users/:id
//!
//! The `users` variant resolves the membership through the member's user id
//! (`find_membership` uses `params[:user_id]`), the `memberships` variant
//! through the membership id.
//!
//! Pinned behavior:
//! - a teacher or the member themselves may update the membership;
//! - `workflow_state` may only be set to "accepted";
//! - a membership of another group or a missing membership is 404;
//! - an unrelated user is 403.
//!
//! Fleet rule: dedicated accounts and courses per test, every seeded row is
//! cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gm_{}_{}",
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
            .merge(star_groups::router())
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

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_teacher(pool: &PgPool, user_id: i64, course_id: i64) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'teacher')")
        .bind(user_id)
        .bind(course_id)
        .execute(pool)
        .await
        .expect("seed teacher");
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

async fn seed_membership(pool: &PgPool, group_id: i64, user_id: i64, workflow_state: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(group_id)
    .bind(user_id)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed membership")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM groups WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn patch_membership_path_updates_like_put() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;
    let group = seed_group(&pool, course, "Group").await;
    let membership = seed_membership(&pool, group, student, "requested").await;

    let resp = server
        .patch(&format!("/groups/{group}/memberships/{membership}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "workflow_state": "accepted" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "accepted");
    assert_eq!(body["user_id"], student);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn user_scoped_membership_update_accepts_a_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;
    let group = seed_group(&pool, course, "Group").await;
    seed_membership(&pool, group, student, "requested").await;

    // PUT by user id.
    let resp = server
        .put(&format!("/groups/{group}/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "workflow_state": "accepted" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "accepted");

    // PATCH by user id works the same way.
    let resp = server
        .patch(&format!("/groups/{group}/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "moderator": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["moderator"], true);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn membership_update_rejects_other_states_and_other_users() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_teacher(&pool, teacher, course).await;
    let group = seed_group(&pool, course, "Group").await;
    seed_membership(&pool, group, student, "requested").await;

    // Only "accepted" may be set.
    let resp = server
        .put(&format!("/groups/{group}/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "workflow_state": "rejected" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // An unrelated user may not update the membership.
    let resp = server
        .put(&format!("/groups/{group}/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .json(&serde_json::json!({ "workflow_state": "accepted" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // A user that is not a member of the group is a 404.
    let resp = server
        .put(&format!("/groups/{group}/users/{stranger}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "workflow_state": "accepted" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Anonymous callers are rejected before the handler.
    let resp = server
        .patch(&format!("/groups/{group}/users/{student}"))
        .json(&serde_json::json!({ "workflow_state": "accepted" }))
        .await;
    assert_eq!(resp.status_code(), 401);

    cleanup(&pool, &prefix).await;
}
