//! Integration tests for star-lti data_services and line_items.
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

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "lti-ags-tests-{}-{}",
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
        jwt_secret: JWT_SECRET.to_string(),
    };

    // Build router that includes all three sub-routers.
    let app = star_lti::router().with_state(state);

    let server = TestServer::new(app).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
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
        "INSERT INTO account_users (account_id, user_id, role) \
         VALUES ($1, $2, 'admin') ON CONFLICT (account_id, user_id) DO NOTHING",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account admin");
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) \
         VALUES ($1, $2, $3) RETURNING id",
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
    // Order matters for FK constraints.
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

// ---------------------------------------------------------------------------
// Data Services tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn data_services_crud() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let base = format!("/accounts/{account}/data_services");

    // Seed account admin permission.
    seed_account_admin(&pool, account, admin).await;

    // Anonymous caller gets 401.
    let resp = server.get(&base).await;
    assert_eq!(resp.status_code(), 401, "anonymous list");

    // Non-admin gets 403.
    let resp = server
        .get(&base)
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student list");

    // Create a subscription.
    let created = server
        .post(&base)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "subscription": {
                "ContextId": "assignment-1",
                "ContextType": "assignment",
                "EventTypes": ["asset_accessed"],
                "Format": "live-event",
                "TransportMetadata": { "Url": "https://example.com/sqs" },
                "TransportType": "https"
            }
        }))
        .await;
    assert_eq!(created.status_code(), 201, "create: {}", created.text());
    let sub: Value = created.json();
    let id = sub["id"].as_i64().expect("subscription id");
    assert_eq!(sub["workflow_state"], "active");

    // List shows the subscription.
    let resp = server
        .get(&base)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "list: {}", resp.text());
    let list: Vec<Value> = resp.json();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"].as_i64(), Some(id));

    // Show returns the subscription.
    let show_path = format!("{base}/{id}");
    let resp = server
        .get(&show_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "show: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"].as_i64(), Some(id));

    // Delete the subscription (soft-delete) BEFORE the update-to-Deleted
    // step: our destroy refuses already-deleted rows (404), and Canvas gives
    // no ground truth for destroy-after-delete — it merely proxies the
    // upstream live-events service response
    // (data_services_controller.rb:219-221 forward_service_response).
    let resp = server
        .delete(&show_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "destroy: {}", resp.text());

    // Update a fresh subscription to the Deleted state (Canvas reads
    // PascalCase subscription[State], data_services_controller.rb
    // L154/L166).
    let created2 = server
        .post(&base)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "subscription": {
                "ContextId": "assignment-2",
                "ContextType": "assignment",
                "EventTypes": ["asset_accessed"],
                "Format": "live-event",
                "TransportMetadata": { "Url": "https://example.com/sqs" },
                "TransportType": "https"
            }
        }))
        .await;
    assert_eq!(created2.status_code(), 201, "create 2: {}", created2.text());
    let sub2: Value = created2.json();
    let id2 = sub2["id"].as_i64().expect("subscription id 2");

    let updated = server
        .put(&format!("{base}/{id2}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "subscription": {
                "State": "Deleted"
            }
        }))
        .await;
    assert_eq!(updated.status_code(), 200, "update: {}", updated.text());
    let updated_sub: Value = updated.json();
    assert_eq!(updated_sub["workflow_state"], "deleted");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Line Items (IMS AGS) tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn line_items_crud() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let course = seed_course(&pool, &prefix, "main", account).await;
    let base = format!("/api/lti/courses/{course}/line_items");

    seed_account_admin(&pool, account, admin).await;

    // Anonymous caller gets 401.
    let resp = server.get(&base).await;
    assert_eq!(resp.status_code(), 401, "anonymous list");

    // Student gets 403 (no course-level permission).
    let resp = server
        .get(&base)
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student list");

    // Create a line item.
    let created = server
        .post(&base)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "label": "Homework 1",
            "scoreMaximum": 100.0,
            "resourceId": "hw-1",
            "tag": "homework"
        }))
        .await;
    assert_eq!(created.status_code(), 201, "create: {}", created.text());
    let item: Value = created.json();
    let id = item["id"].as_i64().expect("line item id");
    assert_eq!(item["label"], "Homework 1");
    assert_eq!(item["scoreMaximum"], 100.0);
    assert_eq!(item["resourceId"], "hw-1");
    assert_eq!(item["tag"], "homework");

    // List shows the line item.
    let resp = server
        .get(&base)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "list: {}", resp.text());
    let list: Vec<Value> = resp.json();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"].as_i64(), Some(id));

    // Show returns the line item.
    let show_path = format!("{base}/{id}");
    let resp = server
        .get(&show_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "show: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"].as_i64(), Some(id));
    assert_eq!(shown["label"], "Homework 1");

    // Update the line item.
    let updated = server
        .put(&show_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "label": "Homework 1 Updated",
            "scoreMaximum": 150.0
        }))
        .await;
    assert_eq!(updated.status_code(), 200, "update: {}", updated.text());
    let updated_item: Value = updated.json();
    assert_eq!(updated_item["label"], "Homework 1 Updated");
    assert_eq!(updated_item["scoreMaximum"], 150.0);

    // Delete the line item (soft-delete).
    let resp = server
        .delete(&show_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "destroy: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["workflowState"], "deleted");

    // 404 after delete.
    let resp = server
        .get(&show_path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "not found after delete");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Cross-route: not-found for wrong account scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn data_services_not_found_wrong_account() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account1 = seed_account(&pool, &prefix, "root1", None).await;
    let account2 = seed_account(&pool, &prefix, "root2", None).await;
    seed_account_admin(&pool, account1, admin).await;

    // Create subscription on account1.
    let created = server
        .post(&format!("/accounts/{account1}/data_services"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "subscription": {
                "ContextId": "assignment-1",
                "ContextType": "assignment",
                "EventTypes": ["asset_accessed"],
                "Format": "live-event",
                "TransportMetadata": { "Url": "https://example.com/sqs" },
                "TransportType": "https"
            }
        }))
        .await;
    let sub: Value = created.json();
    let id = sub["id"].as_i64().unwrap();

    // Accessing it via account2 returns 404.
    let resp = server
        .get(&format!("/accounts/{account2}/data_services/{id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "cross-account access");

    cleanup(&pool, &prefix).await;
}
