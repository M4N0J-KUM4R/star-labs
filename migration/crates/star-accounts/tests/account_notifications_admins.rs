//! Integration tests for the Phase 2 account-notification and account-admin
//! routes added to star-accounts:
//!
//! - GET/POST /accounts/{accountId}/account_notifications
//! - DELETE /accounts/{accountId}/account_notifications/{id}
//! - DELETE /account_notifications/{id}
//! - GET/POST /accounts/{accountId}/admins
//! - DELETE /accounts/{accountId}/admins/{userId}
//!
//! Conventions mirror `accounts_integration.rs`: skipped cleanly when
//! DATABASE_URL is unset, unique per-run prefix, and full teardown so the
//! suite is safe against a shared database.
//!
//! Behavior pinned (Canvas parity):
//! - Account notifications serialize as
//!   `id, subject, start_at, end_at, icon, message, role_ids, roles, author`
//!   (Api::V1::AccountNotifications#account_notification_json). The create
//!   response leaves `author` null; the admin listing populates it.
//! - Listing is a bare JSON array ordered by `start_at` DESC, id DESC.
//! - Invalid payloads return 400 (Canvas renders the model errors as
//!   :bad_request, not 422).
//! - Admins serialize as `id, role, role_id, user, workflow_state` with the
//!   built-in role name "AccountAdmin" (canonical `account_users.role` is
//!   "admin"); DELETE reports workflow_state "deleted".
//! - Every new route requires account-admin privileges: a global admin or an
//!   account admin for the account/ancestor chain (`can_access_account`).

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "acc-p2-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
        n
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

    let server = TestServer::new(star_accounts_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn star_accounts_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_accounts::router())
        .with_state(state)
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role, sortable_name)
        VALUES ($1, $2, $3, $4, $1)
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

async fn seed_account(pool: &PgPool, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO accounts (name, parent_account_id, workflow_state)
        VALUES ($1, $2, 'active')
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // account_users and account_notifications cascade from users/accounts.
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

async fn notification_count(pool: &PgPool, account_id: i64) -> i64 {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM account_notifications WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(pool)
            .await
            .unwrap();
    count
}

async fn notification_role_count(pool: &PgPool, notification_id: i64) -> i64 {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM account_notification_roles WHERE account_notification_id = $1",
    )
    .bind(notification_id)
    .fetch_one(pool)
    .await
    .unwrap();
    count
}

async fn account_user_count(pool: &PgPool, account_id: i64, user_id: i64) -> i64 {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM account_users WHERE account_id = $1 AND user_id = $2")
            .bind(account_id)
            .bind(user_id)
            .fetch_one(pool)
            .await
            .unwrap();
    count
}

/// POST a valid notification and return its id.
async fn create_notification(
    server: &TestServer,
    account_id: i64,
    token: &str,
    start_at: &str,
    end_at: &str,
) -> i64 {
    let resp = server
        .post(&format!("/accounts/{account_id}/account_notifications"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({
            "subject": "New notification",
            "message": "This is a global notification",
            "start_at": start_at,
            "end_at": end_at,
            "account_notification_roles": ["StudentEnrollment"]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    body["id"].as_i64().expect("notification id")
}

// ---------------------------------------------------------------------------
// Account notifications
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_and_list_account_notifications_as_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;

    let body = json!({
        "subject": "New notification",
        "message": "This is a global notification",
        "start_at": "2026-01-01T00:00:00Z",
        "end_at": "2026-02-01T00:00:00Z",
        "icon": "warning",
        "account_notification_roles": ["StudentEnrollment", "TeacherEnrollment", "StudentEnrollment"]
    });
    let resp = server
        .post(&format!("/accounts/{account}/account_notifications"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&body)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();

    let id = created["id"].as_i64().expect("id");
    assert_eq!(created["subject"], "New notification");
    assert_eq!(created["message"], "This is a global notification");
    assert_eq!(created["icon"], "warning");
    assert!(created["start_at"].is_string(), "body: {created}");
    assert!(created["end_at"].is_string(), "body: {created}");
    // Duplicate role entries collapse; ids are null because the entries were
    // role names (this schema has no roles table).
    assert_eq!(
        created["roles"],
        json!(["StudentEnrollment", "TeacherEnrollment"])
    );
    assert_eq!(created["role_ids"], json!([null, null]));
    // Canvas create serializes author as nil (display_author defaults false).
    assert!(created["author"].is_null(), "body: {created}");

    // Persisted with the caller as author and Canvas' active workflow state.
    let (stored_account, stored_user, workflow_state): (i64, Option<i64>, String) = sqlx::query_as(
        "SELECT account_id, user_id, workflow_state FROM account_notifications WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored_account, account);
    assert_eq!(stored_user, Some(admin));
    assert_eq!(workflow_state, "active");
    assert_eq!(notification_role_count(&pool, id).await, 2);

    // A second, newer notification must sort first (start_at DESC, id DESC).
    let newer = create_notification(
        &server,
        account,
        &auth(admin, "admin"),
        "2026-01-15T00:00:00Z",
        "2026-03-01T00:00:00Z",
    )
    .await;

    // The listing is a bare JSON array (Canvas renders the array directly).
    let resp = server
        .get(&format!("/accounts/{account}/account_notifications"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let rows = listed.as_array().expect("bare array, not a page object");
    assert_eq!(rows.len(), 2, "body: {listed}");
    assert_eq!(rows[0]["id"], newer, "newest start_at first: {listed}");
    assert_eq!(rows[1]["id"], id);

    let first = rows.iter().find(|n| n["id"].as_i64() == Some(id)).unwrap();
    assert_eq!(
        first["roles"],
        json!(["StudentEnrollment", "TeacherEnrollment"])
    );
    // The admin listing mirrors Canvas include_all: the author is populated.
    assert_eq!(first["author"]["id"], admin);
    assert_eq!(first["author"]["name"], "Admin");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_notification_validation_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    let token = auth(admin, "admin");

    // Non-admins are rejected by the account-admin gate.
    let resp = server
        .get(&format!("/accounts/{account}/account_notifications"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .post(&format!("/accounts/{account}/account_notifications"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "subject": "s",
            "message": "m",
            "start_at": "2026-01-01T00:00:00Z",
            "end_at": "2026-02-01T00:00:00Z"
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Missing account -> 404 for an admin.
    let resp = server
        .post("/accounts/999999999/account_notifications")
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({
            "subject": "s",
            "message": "m",
            "start_at": "2026-01-01T00:00:00Z",
            "end_at": "2026-02-01T00:00:00Z"
        }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Canvas renders model validation errors as 400 (:bad_request).
    for payload in [
        json!({
            "subject": "   ",
            "message": "m",
            "start_at": "2026-01-01T00:00:00Z",
            "end_at": "2026-02-01T00:00:00Z"
        }),
        json!({
            "subject": "s",
            "message": "",
            "start_at": "2026-01-01T00:00:00Z",
            "end_at": "2026-02-01T00:00:00Z"
        }),
        json!({
            "subject": "s",
            "message": "m",
            "start_at": "2026-02-01T00:00:00Z",
            "end_at": "2026-01-01T00:00:00Z"
        }),
    ] {
        let resp = server
            .post(&format!("/accounts/{account}/account_notifications"))
            .add_header(axum::http::header::AUTHORIZATION, token.clone())
            .json(&payload)
            .await;
        assert_eq!(
            resp.status_code(),
            400,
            "payload {payload}: {}",
            resp.text()
        );
    }

    assert_eq!(
        notification_count(&pool, account).await,
        0,
        "invalid payloads must not persist notifications"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_account_notification_routes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    let other = seed_account(&pool, &format!("{prefix}-Other"), None).await;
    let token = auth(admin, "admin");

    let first = create_notification(
        &server,
        account,
        &token,
        "2026-01-01T00:00:00Z",
        "2026-02-01T00:00:00Z",
    )
    .await;

    // Non-admins cannot delete.
    let resp = server
        .delete(&format!("/account_notifications/{first}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(notification_count(&pool, account).await, 1);

    // Canvas shape is echoed, then the row and its roles are gone.
    let resp = server
        .delete(&format!("/account_notifications/{first}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["id"], first);
    assert_eq!(deleted["subject"], "New notification");
    assert_eq!(notification_count(&pool, account).await, 0);
    assert_eq!(notification_role_count(&pool, first).await, 0);

    let resp = server
        .delete(&format!("/account_notifications/{first}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Scoped alias rejects a notification that belongs to another account.
    let second = create_notification(
        &server,
        account,
        &token,
        "2026-01-01T00:00:00Z",
        "2026-02-01T00:00:00Z",
    )
    .await;
    let resp = server
        .delete(&format!("/accounts/{other}/account_notifications/{second}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(notification_count(&pool, account).await, 1);

    let resp = server
        .delete(&format!(
            "/accounts/{account}/account_notifications/{second}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(notification_count(&pool, account).await, 0);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Account admins
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_admins_lifecycle() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let target = seed_user(&pool, &prefix, "Target", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    let sibling = seed_account(&pool, &format!("{prefix}-Sibling"), None).await;
    let token = auth(admin, "admin");

    // Canvas failure modes: missing user is 404, unsupported role is 422.
    let resp = server
        .post(&format!("/accounts/{account}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "user_id": 999999999 }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .post(&format!("/accounts/{account}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "user_id": target, "role": "teacher" }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    // Success is 200 (Canvas renders the new Admin directly).
    let resp = server
        .post(&format!("/accounts/{account}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "user_id": target }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    assert!(created["id"].as_i64().is_some(), "body: {created}");
    assert_eq!(created["role"], "AccountAdmin");
    assert!(created["role_id"].is_null(), "body: {created}");
    assert_eq!(created["workflow_state"], "active");
    assert_eq!(created["user"]["id"], target);
    assert_eq!(created["user"]["name"], "Target");
    assert_eq!(
        created["user"]["email"],
        format!("{prefix}-Target@test.local")
    );
    assert_eq!(created["user"]["sortable_name"], "Target");

    // Granting twice is idempotent.
    let resp = server
        .post(&format!("/accounts/{account}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "user_id": target, "role": "AccountAdmin" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(account_user_count(&pool, account, target).await, 1);

    // The listing is a bare array containing the grant.
    let resp = server
        .get(&format!("/accounts/{account}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let rows = listed.as_array().expect("bare array");
    let row = rows
        .iter()
        .find(|a| a["user"]["id"].as_i64() == Some(target))
        .unwrap_or_else(|| panic!("admin {target} not listed: {listed}"));
    assert_eq!(row["role"], "AccountAdmin");
    assert_eq!(row["workflow_state"], "active");

    // Being an account admin of this account is enough to list (Canvas'
    // manage_account_memberships), but not another account.
    let resp = server
        .get(&format!("/accounts/{account}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, auth(target, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let resp = server
        .get(&format!("/accounts/{sibling}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, auth(target, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Plain users are rejected.
    let resp = server
        .get(&format!("/accounts/{account}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .post(&format!("/accounts/{account}/admins"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "user_id": outsider }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Removal reports workflow_state "deleted" and deletes the grant row.
    let resp = server
        .delete(&format!("/accounts/{account}/admins/{target}"))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let removed: Value = resp.json();
    assert_eq!(removed["user"]["id"], target);
    assert_eq!(removed["workflow_state"], "deleted");
    assert_eq!(account_user_count(&pool, account, target).await, 0);

    let resp = server
        .delete(&format!("/accounts/{account}/admins/{target}"))
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
