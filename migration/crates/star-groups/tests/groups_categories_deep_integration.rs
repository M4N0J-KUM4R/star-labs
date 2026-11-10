//! Integration tests for the Phase 4 group-categories-deep slice.
//!
//! Covers the routes added on top of the existing star-groups API:
//!   GET/POST /accounts/{accountId}/group_categories
//!   PATCH    /group_categories/{id}
//!   POST     /group_categories/{id}/assign_unassigned_members
//!   POST     /groups
//!
//! Skipped cleanly when DATABASE_URL is unset. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-groups

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gcd-tests-{}-{}",
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

async fn seed_account(pool: &PgPool, prefix: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-Account"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(course_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed enrollment");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
    let email_pattern = format!("{prefix}%@test.local");
    let _ = sqlx::query(
        "DELETE FROM groups \
         WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1) \
            OR group_category_id IN \
               (SELECT id FROM group_categories \
                WHERE account_id IN (SELECT id FROM accounts WHERE name LIKE $1))",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM group_categories \
         WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1) \
            OR account_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(&pattern)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(&email_pattern)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(&pattern)
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{accountId}/group_categories
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_group_categories_require_admin_and_reject_bad_collaboration_state() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix).await;

    let forbidden = server
        .get(&format!("/accounts/{account}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "body: {}", forbidden.text());

    let invalid = server
        .get(&format!(
            "/accounts/{account}/group_categories?collaboration_state=sometimes"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(invalid.status_code(), 400, "body: {}", invalid.text());
    assert!(invalid
        .text()
        .contains("Invalid collaboration_state parameter"));

    // Missing account is 404, not an empty list.
    let missing = server
        .get("/accounts/0/group_categories")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing.status_code(), 404, "body: {}", missing.text());

    let ok = server
        .get(&format!("/accounts/{account}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(ok.status_code(), 200, "body: {}", ok.text());
    assert_eq!(ok.json::<Value>(), json!([]));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /accounts/{accountId}/group_categories
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_account_group_category_returns_account_context() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix).await;

    let forbidden = server
        .post(&format!("/accounts/{account}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Account Groups" }))
        .await;
    assert_eq!(forbidden.status_code(), 403, "body: {}", forbidden.text());

    let created = server
        .post(&format!("/accounts/{account}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "name": "Account Groups" }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let body: Value = created.json();
    assert_eq!(body["name"], "Account Groups");
    assert_eq!(body["context_type"], "Account");
    assert_eq!(body["account_id"].as_i64(), Some(account));
    assert!(body.get("course_id").is_none());

    let duplicate = server
        .post(&format!("/accounts/{account}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "name": "Account Groups" }))
        .await;
    assert_eq!(duplicate.status_code(), 400, "body: {}", duplicate.text());

    let listed = server
        .get(&format!(
            "/accounts/{account}/group_categories?collaboration_state=all"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(listed.status_code(), 200, "body: {}", listed.text());
    assert_eq!(listed.json::<Value>().as_array().map(Vec::len), Some(1));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PATCH /group_categories/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn patch_group_category_updates_name_like_put() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let created = server
        .post(&format!("/courses/{course}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Project Groups" }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let category_id = created.json::<Value>()["id"].as_i64().expect("category id");

    let patched = server
        .patch(&format!("/group_categories/{category_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Renamed Groups" }))
        .await;
    assert_eq!(patched.status_code(), 200, "body: {}", patched.text());
    assert_eq!(patched.json::<Value>()["name"], "Renamed Groups");

    let (name,): (String,) = sqlx::query_as("SELECT name FROM group_categories WHERE id = $1")
        .bind(category_id)
        .fetch_one(&pool)
        .await
        .expect("fetch category");
    assert_eq!(name, "Renamed Groups");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /group_categories/{id}/assign_unassigned_members and POST /groups
// ---------------------------------------------------------------------------

#[tokio::test]
async fn assign_unassigned_members_distributes_students_across_groups() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student_a = seed_user(&pool, &prefix, "StudentA", "student").await;
    let student_b = seed_user(&pool, &prefix, "StudentB", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student_a, course, "student").await;
    seed_enrollment(&pool, student_b, course, "student").await;

    let created = server
        .post(&format!("/courses/{course}/group_categories"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Lab Groups", "create_group_count": 2 }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let category_id = created.json::<Value>()["id"].as_i64().expect("category id");

    // POST /groups resolves its context from the category (Canvas groups#create).
    let group = server
        .post("/groups")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Lab Group 3", "group_category_id": category_id }))
        .await;
    assert_eq!(group.status_code(), 200, "body: {}", group.text());
    let group_body: Value = group.json();
    assert_eq!(group_body["course_id"].as_i64(), Some(course));
    assert_eq!(group_body["group_category_id"].as_i64(), Some(category_id));

    let assigned = server
        .post(&format!(
            "/group_categories/{category_id}/assign_unassigned_members"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(assigned.status_code(), 200, "body: {}", assigned.text());
    let groups: Vec<Value> = assigned.json();
    assert_eq!(groups.len(), 2, "two groups get one student each");
    let assigned_users: Vec<i64> = groups
        .iter()
        .flat_map(|group| group["new_members"].as_array().cloned().unwrap_or_default())
        .map(|member| member["user_id"].as_i64().expect("user id"))
        .collect();
    assert_eq!(assigned_users.len(), 2);
    assert!(assigned_users.contains(&student_a));
    assert!(assigned_users.contains(&student_b));

    let states: Vec<(String,)> = sqlx::query_as(
        "SELECT gm.workflow_state FROM group_memberships gm \
         JOIN groups g ON g.id = gm.group_id \
         WHERE g.group_category_id = $1 AND gm.user_id = ANY($2)",
    )
    .bind(category_id)
    .bind(vec![student_a, student_b])
    .fetch_all(&pool)
    .await
    .expect("fetch memberships");
    assert_eq!(states.len(), 2);
    assert!(states.iter().all(|(state,)| state == "accepted"));

    // Nothing is left unassigned on a second run.
    let second = server
        .post(&format!(
            "/group_categories/{category_id}/assign_unassigned_members"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(second.status_code(), 200, "body: {}", second.text());
    assert_eq!(second.json::<Value>(), json!([]));

    cleanup(&pool, &prefix).await;
}
