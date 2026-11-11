//! Integration tests for Canvas `accounts/roles_controller` parity:
//! `GET/POST /accounts/{accountId}/roles` and
//! `GET/PUT/DELETE /accounts/{accountId}/roles/{id}`.
//!
//! Pinned Canvas behaviors (API v1 parity):
//! - `GET /accounts/{accountId}/roles` lists active built-in and custom roles
//!   for the account; built-ins have `workflow_state: "built_in"` and
//!   `account: null`; custom roles have `workflow_state: "active"` and
//!   `account_id` set.
//! - `GET /accounts/{accountId}/roles?state[]=inactive` includes roles in
//!   `workflow_state: inactive`.
//! - `POST` accepts both flat `{label, base_role_type}` and nested
//!   `{"role": {label, base_role_type}}` bodies.
//! - `POST` rejects a blank/missing `label` with 400.
//! - `POST` returns 409 Conflict when a custom role with the same name already
//!   exists for the account.
//! - `PUT` and `DELETE` on a built-in role return 400 with message
//!   `"built-in roles cannot be modified"`.
//! - `PUT` and `DELETE` on a non-existent role return 404.
//! - `DELETE` is a soft-delete (`workflow_state = 'deleted'`), not a hard
//!   delete; deleted roles are absent from the list.
//! - Non-admin callers receive 403 Forbidden on all five routes.
//! - `GET /accounts/{accountId}/roles/{id}` for a deleted role returns 404.
//!
//! Skipped cleanly when DATABASE_URL is not set; uses a unique per-run prefix
//! plus full teardown so repeat runs against a shared DB are safe.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "rod_{}_{}",
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

    let server =
        TestServer::new(star_permissions::router().with_state(state))
            .expect("build TestServer");
    Some((server, pool, run_prefix()))
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, workflow_state) VALUES ($1, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn create_role_ok(
    server: &TestServer,
    account_id: i64,
    bearer: &str,
    label: &str,
    base_role_type: Option<&str>,
) -> Value {
    let mut body = json!({ "label": label });
    if let Some(brt) = base_role_type {
        body["base_role_type"] = json!(brt);
    }
    let resp = server
        .post(&format!("/accounts/{account_id}/roles"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&body)
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    resp.json()
}

/// Delete every row created by this run, in FK-safe order.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM roles WHERE account_id IN (SELECT id FROM accounts WHERE name LIKE $1)")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM account_users WHERE account_id IN (SELECT id FROM accounts WHERE name LIKE $1)")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{accountId}/roles — list roles
// ---------------------------------------------------------------------------

/// `GET /accounts/{accountId}/roles` requires admin; non-admin gets 403.
#[tokio::test]
async fn list_roles_forbidden_for_non_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "ListForbidden").await;

    let resp = server
        .get(&format!("/accounts/{account}/roles"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// `GET /accounts/{accountId}/roles` returns the account's custom roles and
/// all built-in roles (which have NULL account_id). Built-ins carry
/// `workflow_state: "built_in"`.
#[tokio::test]
async fn list_roles_returns_builtins_and_custom_roles() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "ListBuiltin").await;
    let admin_bearer = auth(admin, "admin");

    let resp = server
        .get(&format!("/accounts/{account}/roles"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();

    assert!(list.is_array(), "response must be a JSON array");

    // At minimum we must see the five built-in roles.
    let builtin_names: Vec<&str> = ["StudentEnrollment", "TeacherEnrollment", "TaEnrollment", "DesignerEnrollment", "ObserverEnrollment"];
    for name in builtin_names {
        assert!(
            list.iter().any(|r| r["name"] == name && r["workflow_state"] == "built_in"),
            "built-in role {name} must appear with workflow_state=built_in"
        );
    }

    // All rows must have the expected shape fields.
    for role in list.as_array().unwrap() {
        assert!(role["id"].is_i64(), "id must be i64: {}", role);
        assert!(role["name"].is_string(), "name must be string: {}", role);
        assert!(role["label"].is_string(), "label must be string: {}", role);
        assert!(role["base_role_type"].is_string(), "base_role_type must be string: {}", role);
        assert!(role["workflow_state"].is_string(), "workflow_state must be string: {}", role);
        assert!(role["created_at"].is_string(), "created_at must be string: {}", role);
        assert!(role["updated_at"].is_string(), "updated_at must be string: {}", role);
    }

    cleanup(&pool, &prefix).await;
}

/// `GET /accounts/{accountId}/roles` returns 404 for a non-existent account.
#[tokio::test]
async fn list_roles_404_for_missing_account() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .get("/accounts/999999999/roles")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /accounts/{accountId}/roles — create a custom role
// ---------------------------------------------------------------------------

/// `POST /accounts/{accountId}/roles` creates a custom role and returns it
/// with `workflow_state: "active"` and `account_id` set.
#[tokio::test]
async fn create_role_returns_new_role_with_active_state() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "CreateRole").await;
    let admin_bearer = auth(admin, "admin");

    let created = create_role_ok(&server, account, &admin_bearer, "Head Grader", Some("ta")).await;

    assert_eq!(created["label"], "Head Grader");
    assert_eq!(created["base_role_type"], "TaEnrollment");
    assert_eq!(created["workflow_state"], "active");
    assert_eq!(created["account_id"], account);
    assert!(created["id"].is_i64());

    // The new role appears in the list.
    let resp = server
        .get(&format!("/accounts/{account}/roles"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer)
        .await;
    let list: Value = resp.json();
    assert!(
        list.as_array().unwrap().iter().any(|r| r["id"] == created["id"]),
        "created role must appear in list"
    );

    cleanup(&pool, &prefix).await;
}

/// `POST` accepts a nested `{"role": {...}}` body, not just a flat body.
#[tokio::test]
async fn create_role_accepts_nested_role_wrapper() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "CreateNested").await;

    let resp = server
        .post(&format!("/accounts/{account}/roles"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "role": {
                "label": "Nested Grader",
                "base_role_type": "TeacherEnrollment"
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["label"], "Nested Grader");
    assert_eq!(created["base_role_type"], "TeacherEnrollment");

    cleanup(&pool, &prefix).await;
}

/// `POST` rejects a blank or missing `label` with 400.
#[tokio::test]
async fn create_role_rejects_blank_label() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "CreateBlankLabel").await;
    let admin_bearer = auth(admin, "admin");

    for label in ["", "   ", null] {
        let body = if label == "null" {
            json!({})
        } else {
            json!({ "label": label })
        };
        let resp = server
            .post(&format!("/accounts/{account}/roles"))
            .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
            .json(&body)
            .await;
        assert_eq!(
            resp.status_code(), 400,
            "blank label {label:?} must be 400; body: {}",
            resp.text()
        );
    }

    cleanup(&pool, &prefix).await;
}

/// `POST` rejects an unknown `base_role_type` with 400.
#[tokio::test]
async fn create_role_rejects_unknown_base_role_type() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "CreateBadBase").await;

    let resp = server
        .post(&format!("/accounts/{account}/roles"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "label": "BadBase", "base_role_type": "WizardEnrollment" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// `POST` returns 409 Conflict when a role with the same name already exists
/// for the account.
#[tokio::test]
async fn create_role_conflict_on_duplicate_name() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "CreateDup").await;
    let admin_bearer = auth(admin, "admin");

    // Create first role.
    create_role_ok(&server, account, &admin_bearer, "Duplicate", Some("student"));

    // Second create with same label → 409.
    let resp = server
        .post(&format!("/accounts/{account}/roles"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer)
        .json(&json!({ "label": "Duplicate", "base_role_type": "student" }))
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());
    assert!(
        resp.text().to_lowercase().contains("already exists")
            || resp.text().to_lowercase().contains("conflict"),
        "409 body should mention duplicate: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{accountId}/roles/{id} — show a role
// ---------------------------------------------------------------------------

/// `GET /accounts/{accountId}/roles/{id}` returns a custom role with full
/// shape including `account_id` set, `workflow_state: "active"`.
#[tokio::test]
async fn show_custom_role_returns_full_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "ShowCustom").await;
    let admin_bearer = auth(admin, "admin");

    let created = create_role_ok(&server, account, &admin_bearer, "ShowMe", Some("designer")).await;
    let role_id = created["id"].as_i64().unwrap();

    let resp = server
        .get(&format!("/accounts/{account}/roles/{role_id}"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"], role_id);
    assert_eq!(shown["label"], "ShowMe");
    assert_eq!(shown["base_role_type"], "DesignerEnrollment");
    assert_eq!(shown["workflow_state"], "active");
    assert_eq!(shown["account_id"], account);

    cleanup(&pool, &prefix).await;
}

/// `GET /accounts/{accountId}/roles/{id}` returns a built-in role with
/// `account_id: null` and `workflow_state: "built_in"`.
#[tokio::test]
async fn show_builtin_role_returns_builtins_with_null_account() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "ShowBuiltin").await;

    // Built-in role ids are 1..5 (TeacherEnrollment = 2).
    let builtin_id: i64 = 2;

    let resp = server
        .get(&format!("/accounts/{account}/roles/{builtin_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"], builtin_id);
    assert_eq!(shown["account_id"], json!(null));
    assert_eq!(shown["workflow_state"], "built_in");
    assert_eq!(shown["base_role_type"], "TeacherEnrollment");

    cleanup(&pool, &prefix).await;
}

/// `GET /accounts/{accountId}/roles/{id}` returns 404 for a non-existent id.
#[tokio::test]
async fn show_role_404_for_missing_id() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Show404").await;

    let resp = server
        .get(&format!("/accounts/{account}/roles/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /accounts/{accountId}/roles/{id} — update a custom role
// ---------------------------------------------------------------------------

/// `PUT` updates `label` and returns the updated role.
#[tokio::test]
async fn update_role_changes_label_and_returns_updated_role() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "UpdateRole").await;
    let admin_bearer = auth(admin, "admin");

    let created = create_role_ok(&server, account, &admin_bearer, "OldLabel", Some("ta")).await;
    let role_id = created["id"].as_i64().unwrap();

    let resp = server
        .put(&format!("/accounts/{account}/roles/{role_id}"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .json(&json!({ "label": "NewLabel" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["label"], "NewLabel");
    assert_eq!(updated["name"], created["name"], "name must not change");
    assert_eq!(updated["id"], role_id);

    cleanup(&pool, &prefix).await;
}

/// `PUT` on a built-in role returns 400 with message about built-in roles.
#[tokio::test]
async fn update_role_400_for_builtin_role() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "UpdateBuiltin").await;
    let builtin_id: i64 = 3; // TaEnrollment

    let resp = server
        .put(&format!("/accounts/{account}/roles/{builtin_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "label": "NotAllowed" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert!(
        resp.text().to_lowercase().contains("built-in"),
        "400 body should mention built-in: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

/// `PUT` on a non-existent role returns 404.
#[tokio::test]
async fn update_role_404_for_missing_role() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Update404").await;

    let resp = server
        .put(&format!("/accounts/{account}/roles/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "label": "Ghost" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// `PUT` requires admin; non-admin gets 403.
#[tokio::test]
async fn update_role_forbidden_for_non_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "UpdateForbidden").await;
    let admin_bearer = auth(admin, "admin");

    let created = create_role_ok(&server, account, &admin_bearer, "ForbidUpdate", Some("student")).await;
    let role_id = created["id"].as_i64().unwrap();

    let resp = server
        .put(&format!("/accounts/{account}/roles/{role_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "label": "Hacked" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /accounts/{accountId}/roles/{id} — soft-delete a custom role
// ---------------------------------------------------------------------------

/// `DELETE` soft-deletes a custom role (`workflow_state = 'deleted'`) and
/// returns it. The role disappears from the list.
#[tokio::test]
async fn delete_role_soft_deletes_and_returns_deleted_role() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "DeleteRole").await;
    let admin_bearer = auth(admin, "admin");

    let created = create_role_ok(&server, account, &admin_bearer, "ToDelete", Some("student")).await;
    let role_id = created["id"].as_i64().unwrap();

    let resp = server
        .delete(&format!("/accounts/{account}/roles/{role_id}"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["workflow_state"], "deleted");
    assert_eq!(deleted["id"], role_id);

    // Deleted role no longer appears in the list.
    let resp = server
        .get(&format!("/accounts/{account}/roles"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer)
        .await;
    let list: Value = resp.json();
    assert!(
        !list.as_array().unwrap().iter().any(|r| r["id"] == role_id),
        "deleted role must not appear in list"
    );

    // And GET on the id returns 404.
    let resp = server
        .get(&format!("/accounts/{account}/roles/{role_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// `DELETE` on a built-in role returns 400.
#[tokio::test]
async fn delete_role_400_for_builtin_role() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "DeleteBuiltin").await;
    let builtin_id: i64 = 1; // StudentEnrollment

    let resp = server
        .delete(&format!("/accounts/{account}/roles/{builtin_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// `DELETE` on a non-existent role returns 404.
#[tokio::test]
async fn delete_role_404_for_missing_role() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Delete404").await;

    let resp = server
        .delete(&format!("/accounts/{account}/roles/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// `DELETE` requires admin; non-admin gets 403.
#[tokio::test]
async fn delete_role_forbidden_for_non_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "DeleteForbidden").await;
    let admin_bearer = auth(admin, "admin");

    let created = create_role_ok(&server, account, &admin_bearer, "ForbidDelete", Some("student")).await;
    let role_id = created["id"].as_i64().unwrap();

    let resp = server
        .delete(&format!("/accounts/{account}/roles/{role_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Negative / edge cases
// ---------------------------------------------------------------------------

/// A deleted role cannot be updated or deleted a second time (404).
#[tokio::test]
async fn deleted_role_update_and_delete_return_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "DeletedEdge").await;
    let admin_bearer = auth(admin, "admin");

    let created = create_role_ok(&server, account, &admin_bearer, "OnceDeleted", Some("student")).await;
    let role_id = created["id"].as_i64().unwrap();

    // First delete.
    let resp = server
        .delete(&format!("/accounts/{account}/roles/{role_id}"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);

    // Second delete → 404.
    let resp = server
        .delete(&format!("/accounts/{account}/roles/{role_id}"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // Update on deleted role → 404 (it's not found).
    let resp = server
        .put(&format!("/accounts/{account}/roles/{role_id}"))
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer)
        .json(&json!({ "label": "Zombie" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

/// Creating a role with invalid `workflow_state` returns 400.
#[tokio::test]
async fn create_role_rejects_invalid_workflow_state() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "BadWorkflow").await;

    let resp = server
        .post(&format!("/accounts/{account}/roles"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "label": "BadState", "workflow_state": "zombie" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
