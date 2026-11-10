//! Integration tests for star-lti against a real Postgres.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Unique per-run prefix + full teardown, so repeat runs against a shared
//!   database are safe.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test AppState and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so repeated runs against a shared DB never collide.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "lti-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, migrate, and build a TestServer over the LTI router.
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
        jwt_secret: JWT_SECRET.to_string(),
    };

    let server = TestServer::new(star_lti::router().with_state(state)).expect("build TestServer");
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
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin') \
         ON CONFLICT (account_id, user_id) DO NOTHING",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account admin");
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // Courses first: courses.account_id has no ON DELETE action, so the
    // account rows cannot disappear while a seeded course still points at
    // them. Deleting the accounts then cascades registrations, bindings, and
    // deployments.
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

async fn create_registration(
    server: &TestServer,
    account_id: i64,
    admin_id: i64,
    prefix: &str,
) -> Value {
    let response = server
        .post(&format!("/accounts/{account_id}/lti_registrations"))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({
            "name": format!("{prefix}-tool"),
            "vendor": "Example Vendor",
            "admin_nickname": "Campus Tool",
            "configuration": {
                "title": "Configured Tool",
                "target_link_uri": "https://example.com/launch",
                "oidc_initiation_url": "https://example.com/oidc",
                "launch_settings": { "icon_url": "https://example.com/icon.png" }
            }
        }))
        .await;
    assert_eq!(
        response.status_code(),
        201,
        "create registration; body: {}",
        response.text()
    );
    response.json()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_creates_lists_and_shows_registration() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let list_path = format!("/accounts/{account}/lti_registrations");

    // Anonymous callers are rejected by the auth extractor.
    let response = server.get(&list_path).await;
    assert_eq!(response.status_code(), 401, "anonymous list");

    // Non-admins get Canvas' 403.
    let response = server
        .get(&list_path)
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 403, "student list");

    let created = create_registration(&server, account, admin, &prefix).await;
    let id = created["id"].as_i64().expect("registration id");

    assert_eq!(created["account_id"].as_i64(), Some(account));
    assert_eq!(created["root_account_id"].as_i64(), Some(account));
    assert_eq!(created["name"], format!("{prefix}-tool"));
    assert_eq!(created["admin_nickname"], "Campus Tool");
    assert_eq!(created["vendor"], "Example Vendor");
    assert_eq!(created["lti_version"], "1.3");
    assert_eq!(created["workflow_state"], "active");
    assert_eq!(created["inherited"], false);
    assert_eq!(created["icon_url"], "https://example.com/icon.png");
    assert_eq!(
        created["configuration"]["title"], "Configured Tool",
        "create always embeds configuration"
    );
    assert_eq!(
        created["account_binding"]["workflow_state"], "on",
        "create binds the account"
    );
    assert_eq!(
        created["created_by"]["id"].as_i64(),
        Some(admin),
        "created_by is the acting admin"
    );
    assert!(created["created_at"].is_string());

    // List: `{ total, data }`, binding always embedded, configuration only
    // when requested.
    let response = server
        .get(&list_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let list: Value = response.json();
    assert_eq!(list["total"].as_i64(), Some(1));
    let data = list["data"].as_array().expect("data array");
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["id"].as_i64(), Some(id));
    assert!(data[0]["account_binding"].is_object());
    assert!(
        data[0].get("configuration").is_none(),
        "configuration is opt-in for list"
    );

    let response = server
        .get(&format!("{list_path}?include%5B%5D=configuration"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let list: Value = response.json();
    assert_eq!(list["data"][0]["configuration"]["title"], "Configured Tool");

    // Show: configuration and binding are always present.
    let response = server
        .get(&format!("/lti_registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let shown: Value = response.json();
    assert_eq!(shown["id"].as_i64(), Some(id));
    assert_eq!(shown["configuration"]["title"], "Configured Tool");
    assert_eq!(shown["account_binding"]["workflow_state"], "on");

    // Canvas-nested alias resolves the same registration.
    let response = server
        .get(&format!("/accounts/{account}/lti_registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "nested show");

    // Students cannot read registrations.
    let response = server
        .get(&format!("/lti_registrations/{id}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 403, "student show");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_validates_configuration_and_derives_name() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let path = format!("/accounts/{account}/lti_registrations");

    // Configuration is required.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "name": "No config" }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"][0], "configuration is required");

    // Configuration must be an object.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "configuration": "not-an-object" }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"][0], "configuration must be an object");

    // Name falls back to configuration.title.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "configuration": { "title": "Derived Tool Name" }
        }))
        .await;
    assert_eq!(response.status_code(), 201, "body: {}", response.text());
    let created: Value = response.json();
    assert_eq!(created["name"], "Derived Tool Name");

    // Unknown workflow states are rejected.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "configuration": { "title": "Bad state" },
            "workflow_state": "enabled"
        }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_admins_manage_their_account_tree() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let account_admin = seed_user(&pool, &prefix, "account-admin", "student").await;
    let outsider = seed_user(&pool, &prefix, "outsider", "student").await;
    let root = seed_account(&pool, &prefix, "root", None).await;
    let child = seed_account(&pool, &prefix, "child", Some(root)).await;
    seed_account_admin(&pool, &root, account_admin).await;

    // An account admin of the root can install in the child account.
    let response = server
        .post(&format!("/accounts/{child}/lti_registrations"))
        .add_header(AUTHORIZATION, auth(account_admin, "student"))
        .json(&json!({ "configuration": { "title": "Child Tool" } }))
        .await;
    assert_eq!(response.status_code(), 201, "body: {}", response.text());
    let created: Value = response.json();
    assert_eq!(created["account_id"].as_i64(), Some(child));
    assert_eq!(
        created["root_account_id"].as_i64(),
        Some(root),
        "registrations record the root account"
    );

    // A plain user with no account_users row cannot list or create.
    let response = server
        .get(&format!("/accounts/{child}/lti_registrations"))
        .add_header(AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(response.status_code(), 403, "outsider list");

    let response = server
        .post(&format!("/accounts/{child}/lti_registrations"))
        .add_header(AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "configuration": { "title": "Nope" } }))
        .await;
    assert_eq!(response.status_code(), 403, "outsider create");

    // Unknown accounts are 404 before any permission check.
    let response = server
        .get("/accounts/9223372036854775807/lti_registrations")
        .add_header(AUTHORIZATION, auth(account_admin, "student"))
        .await;
    assert_eq!(response.status_code(), 404, "missing account");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn deployments_lifecycle_and_soft_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let root = seed_account(&pool, &prefix, "root", None).await;
    let course = seed_course(&pool, &prefix, "course", root).await;

    let registration = create_registration(&server, root, admin, &prefix).await;
    let id = registration["id"].as_i64().expect("registration id");
    let deployments_path = format!("/lti_registrations/{id}/deployments");

    // Account deployment defaults to the registration's account.
    let response = server
        .post(&deployments_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let deployment: Value = response.json();
    assert_eq!(deployment["registration_id"].as_i64(), Some(id));
    assert_eq!(deployment["context_type"], "Account");
    assert_eq!(deployment["context_id"].as_i64(), Some(root));
    assert_eq!(deployment["context_name"], format!("{prefix}-root"));
    assert_eq!(deployment["workflow_state"], "active");
    assert_eq!(deployment["root_account_deployment"], true);
    assert!(
        deployment["deployment_id"]
            .as_str()
            .expect("deployment_id string")
            .starts_with(&format!("{root}:")),
        "Canvas deployment id format"
    );

    // Course deployment.
    let response = server
        .post(&deployments_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "for_course_id": course }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let course_deployment: Value = response.json();
    assert_eq!(course_deployment["context_type"], "Course");
    assert_eq!(course_deployment["context_id"].as_i64(), Some(course));
    assert_eq!(course_deployment["root_account_deployment"], false);

    // Conflicting contexts are rejected with Canvas' message.
    let response = server
        .post(&deployments_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "for_subaccount_id": root, "for_course_id": course }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["error"],
        "Cannot specify both for_subaccount_id and for_course_id"
    );

    // Missing course is a 404.
    let response = server
        .post(&deployments_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "for_course_id": 9223372036854775807_i64 }))
        .await;
    assert_eq!(response.status_code(), 404, "missing course");

    // Both list routes return bare arrays.
    let response = server
        .get(&deployments_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let deployments: Value = response.json();
    assert_eq!(deployments.as_array().expect("array").len(), 2);

    let response = server
        .get(&format!(
            "/accounts/{root}/lti_registrations/{id}/deployments"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "nested list");

    // Soft delete returns the registration with workflow_state deleted.
    let response = server
        .delete(&format!("/lti_registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let deleted: Value = response.json();
    assert_eq!(deleted["workflow_state"], "deleted");

    // Deleted registrations disappear from show and list.
    let response = server
        .get(&format!("/lti_registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "show after delete");

    let response = server
        .get(&format!("/accounts/{root}/lti_registrations"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let list: Value = response.json();
    assert_eq!(list["total"].as_i64(), Some(0));

    // Deployment routes are gone with the registration.
    let response = server
        .get(&deployments_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "deployments after delete");

    cleanup(&pool, &prefix).await;
}
