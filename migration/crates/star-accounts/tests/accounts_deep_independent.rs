//! Independent behavioral tests for the Phase 4 accounts-deep routes.
//!
//! Routes under test (star-accounts crate):
//! - PUT    /accounts/{accountId}
//! - GET    /manually_created_courses_account
//! - DELETE /accounts/{accountId}/users/{userId}
//! - GET    /acceptable_use_policy
//!
//! These tests are derived from Canvas API contracts and do NOT copy the
//! developer integration tests in `account_admin_deep.rs`.  Each test
//! focuses on a different behavioral axis: edge-case payloads, permission
//! ordering, response-field shapes, and negative paths.
//!
//! Canvas source evidence:
//! - acceptable_use_policy JSON: `{ redirectUrl: <url> }` when external_url set,
//!   `{ content: <nullable> }` otherwise (accounts_controller.rb:1679-1684).
//! - remove_user: `@account.root_account?` guard raises RecordNotFound (1849),
//!   then checks association exists, then permission (1854).
//! - manually_created_courses_account: returns `account_json` with read_only
//!   based on `:read` right (accounts_controller.rb:707-710).
//! - update_api: root account move blocked with "You cannot move a root
//!   account." (1146), non-admin caller blocked with 401/403 (1116).

use axum::http::header::AUTHORIZATION;
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
        "acc-ind-{}-{}-{}",
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
        INSERT INTO accounts (name, parent_account_id)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn grant_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        r#"
        INSERT INTO account_users (account_id, user_id, role)
        VALUES ($1, $2, 'admin')
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("grant account admin");
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn root_account(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        r#"
        SELECT id FROM accounts
        WHERE parent_account_id IS NULL AND workflow_state <> 'deleted'
        ORDER BY id ASC
        LIMIT 1
        "#,
    )
    .fetch_one(pool)
    .await
    .expect("root account")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// PUT /accounts/{accountId} — edge cases and response shape
// ---------------------------------------------------------------------------

/// Canvas accepts an empty `{}` body (no-op update) returning 200 with the
/// unchanged account.  A body with only unknown fields is also accepted.
#[tokio::test]
async fn update_account_empty_body_is_ok() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let token = auth(admin, "admin");
    let root = seed_account(&pool, &format!("{prefix}-Root"), None).await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(root)).await;

    // Empty object: no fields to update, but valid request.
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], child);
    assert_eq!(body["name"], format!("{prefix}-Child"));

    // Unknown fields are silently ignored (Canvas: strong params only permit
    // known keys; unknown keys do not cause an error).
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "unknown_field": "ignored", "another": 123 }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// PUT returns the full AccountResponse shape: id, name, parent_account_id,
/// root_account_id, workflow_state, created_at, updated_at.
#[tokio::test]
async fn update_account_response_has_all_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let token = auth(admin, "admin");
    let root = seed_account(&pool, &format!("{prefix}-Root"), None).await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(root)).await;

    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "account": { "name": format!("{prefix}-Renamed") } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();

    // All required AccountResponse fields present.
    for field in [
        "id",
        "name",
        "parent_account_id",
        "root_account_id",
        "workflow_state",
        "created_at",
        "updated_at",
    ] {
        assert!(
            body.get(field).is_some(),
            "missing field: {field} in {body}"
        );
    }

    // root_account_id is the topmost ancestor (root in this case).
    assert_eq!(body["root_account_id"], root);

    cleanup(&pool, &prefix).await;
}

/// Moving an account into its own descendant subtree is rejected.
#[tokio::test]
async fn update_account_move_into_own_subtree_is_bad_request() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let token = auth(admin, "admin");
    let root = seed_account(&pool, &format!("{prefix}-Root"), None).await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(root)).await;
    let grandchild = seed_account(&pool, &format!("{prefix}-Grandchild"), Some(child)).await;

    // Attempt to move child under grandchild (child's own descendant).
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "account": { "parent_account_id": grandchild } }))
        .await;
    // Canvas returns 400 for a cycle detected via parent walk.
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /manually_created_courses_account — auth and idempotency axis
// ---------------------------------------------------------------------------

/// Any authenticated user can hit GET /manually_created_courses_account, not
/// just admins.  (The route does not require manage_account_settings; it
/// requires only read on the account, and every authenticated user can read
/// the domain root.)
#[tokio::test]
async fn manually_created_courses_account_any_authenticated_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // A student (non-admin) user who has no account admin grants.
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let token = auth(student, "student");

    let resp = server
        .get("/manually_created_courses_account")
        .add_header(AUTHORIZATION, token)
        .await;
    // Canvas returns 200; any logged-in user can read the domain root's
    // manually-created-courses sub-account.
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["name"], "Manually-Created Courses");

    cleanup(&pool, &prefix).await;
}

/// Repeated calls return the same account id (idempotent creation).
#[tokio::test]
async fn manually_created_courses_account_idempotent_by_id() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let token = auth(admin, "admin");

    let r1 = server
        .get("/manually_created_courses_account")
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(r1.status_code(), 200);
    let id1 = r1.json::<Value>()["id"].as_i64().unwrap();

    // Second call — same id.
    let r2 = server
        .get("/manually_created_courses_account")
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(r2.status_code(), 200);
    let id2 = r2.json::<Value>()["id"].as_i64().unwrap();

    assert_eq!(id1, id2, "idempotent: same account returned on every call");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /accounts/{accountId}/users/{userId} — permission ordering & root guard
// ---------------------------------------------------------------------------

/// Canvas: `remove_user` checks `@account.root_account?` BEFORE looking up the
/// user (accounts_controller.rb:1849 raises RecordNotFound immediately for
/// non-root accounts).  This means a sub-account delete always 404s — even
/// when the caller has no permission to that sub-account at all.
#[tokio::test]
async fn delete_user_on_subaccount_always_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let token = auth(admin, "admin");
    let root = root_account(&pool).await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(root)).await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;

    // Give outsider an account grant on the root so they "exist" in the system.
    grant_account_admin(&pool, root, outsider).await;

    // DELETE on a sub-account — must be 404 per Canvas root_account? guard.
    let resp = server
        .delete(&format!("/accounts/{child}/users/{outsider}"))
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// An authenticated caller who has NO grant on the root and is NOT enrolled
/// in any of its courses should receive 404 (not 403) because the
/// account-admin gate is checked before the user-existence check in Canvas's
/// `remove_user` order: root_account? guard first, then user lookup, then
/// permission.  Our implementation checks `can_access_account` first which
/// may return 403 for unauthorized callers before the user-association check.
#[tokio::test]
async fn delete_user_unassociated_user_not_found() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let token = auth(admin, "admin");
    let root = root_account(&pool).await;
    // A user that exists in the DB but has no account_users grant and no
    // enrollments in any course of this account's subtree.
    let ghost = seed_user(&pool, &prefix, "Ghost", "student").await;

    let resp = server
        .delete(&format!("/accounts/{root}/users/{ghost}"))
        .add_header(AUTHORIZATION, token.clone())
        .await;
    // Canvas: user_account_associations check fails -> RecordNotFound (404).
    // Our implementation checks user_in_account which returns false -> 404.
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /acceptable_use_policy — anonymous access and response field shape
// ---------------------------------------------------------------------------

/// GET /acceptable_use_policy is the one truly anonymous route in this group.
/// Canvas skips `require_user` via `skip_before_action :require_user, only:
///
/// [acceptable_use_policy]` (accounts_controller.rb:304).
#[tokio::test]
async fn acceptable_use_policy_no_auth_required() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = &prefix;

    // No Authorization header at all — must not be 401.
    let resp = server.get("/acceptable_use_policy").await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// Canvas JSON response uses camelCase `redirectUrl` (not snake_case).
/// Confirmed from accounts_controller.rb:1681:
///   `render json: { redirectUrl: external_url }, status: :ok`
#[tokio::test]
async fn acceptable_use_policy_uses_camel_case_redirect_url() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let root = root_account(&pool).await;
    // Insert a row with an external URL.
    sqlx::query(
        "INSERT INTO account_terms_of_service (account_id, terms_type, content, external_url) VALUES ($1, 'built_in:default', 'ignored', $2)",
    )
    .bind(root)
    .bind(format!("https://{prefix}.example.test/policy"))
    .execute(&pool)
    .await
    .expect("insert external url");

    let resp = server.get("/acceptable_use_policy").await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();

    // Must be camelCase `redirectUrl`, not `redirect_url`.
    assert!(
        body.get("redirectUrl").is_some(),
        "missing camelCase 'redirectUrl' in {body}"
    );
    assert!(
        body.get("redirect_url").is_none(),
        "snake_case 'redirect_url' must not appear (Canvas uses camelCase)"
    );

    // Clean up.
    let _ = sqlx::query("DELETE FROM account_terms_of_service WHERE account_id = $1")
        .bind(root)
        .execute(&pool)
        .await;
    cleanup(&pool, &prefix).await;
}

/// When no external URL is set, Canvas returns `{ content: <string or null> }`.
/// Verify both null-content and populated-content cases.
#[tokio::test]
async fn acceptable_use_policy_content_field_when_no_external_url() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let root = root_account(&pool).await;

    // Ensure no terms of service row at all.
    let _ = sqlx::query("DELETE FROM account_terms_of_service WHERE account_id = $1")
        .bind(root)
        .execute(&pool)
        .await;

    let resp = server.get("/acceptable_use_policy").await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();

    // Must have `content` (not `redirectUrl`) when no external URL.
    assert!(
        body.get("content").is_some(),
        "missing 'content' field when no external url: {body}"
    );
    assert!(
        body.get("redirectUrl").is_none(),
        "no redirectUrl when no external URL set: {body}"
    );
    assert_eq!(
        body["content"],
        json!(null),
        "null content when no row: {body}"
    );

    // Now insert content (no external URL).
    sqlx::query(
        "INSERT INTO account_terms_of_service (account_id, terms_type, content) VALUES ($1, 'built_in:default', $2)",
    )
    .bind(root)
    .bind("This is the AUP content.")
    .execute(&pool)
    .await
    .expect("insert content");

    let resp2 = server.get("/acceptable_use_policy").await;
    assert_eq!(resp2.status_code(), 200);
    let body2: Value = resp2.json();
    assert!(
        body2.get("redirectUrl").is_none(),
        "no redirectUrl when content-only: {body2}"
    );
    assert_eq!(body2["content"], "This is the AUP content.");

    // Clean up.
    let _ = sqlx::query("DELETE FROM account_terms_of_service WHERE account_id = $1")
        .bind(root)
        .execute(&pool)
        .await;
    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Suspected defects from Canvas evidence
// ---------------------------------------------------------------------------

// DEFECT-1 (minor): blank-name validation returns 422. Canvas returns 400.
// The crate documents this as an intentional deviation, not a bug.
//
// DEFECT-2 (behavioural): DELETE hard-deletes the user row. Canvas soft-deletes
// (User#destroy sets workflow_state to 'deleted').  The pre-delete user object
// is returned by both, but the actual deletion semantics differ for audit trails
// and multi-tenant scenarios.
//
// DEFECT-3 (contract): `can_access_account` on DELETE may overreject callers
// whose only association is a course enrollment (not an account_users grant).
// Canvas's `allows_user_to_remove_from_account?` / `remove_from_root_account`
// handle course-context associations that our simplified `user_in_account`
// check may not fully cover.
