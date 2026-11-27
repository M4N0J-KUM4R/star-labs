//! Integration tests for the Phase 3 account routes added to star-accounts:
//!
//! - GET/PUT /accounts/{accountId}/settings
//! - GET     /accounts/{accountId}/sub_accounts
//! - GET     /manageable_accounts
//! - GET     /course_creation_accounts
//!
//! Conventions mirror `accounts_integration.rs`: skipped cleanly when
//! DATABASE_URL is unset, unique per-run prefix, and full teardown so the
//! suite is safe against a shared database.
//!
//! Behavior pinned (Canvas parity):
//! - Settings serialize as a bare JSON object (`Accounts#show_settings`
//!   renders `settings_hash` without a wrapper); an unconfigured account
//!   returns `{}`. PUT shallow-merges the posted object into
//!   `accounts.settings` and echoes the merged object.
//! - The account-admin gate matches the rest of the crate: global admin or an
//!   `account_users` grant on the account/parent chain. Permission is checked
//!   before existence (403 wins over 404).
//! - Sub-account listings are bare arrays of `account_json`-shaped objects
//!   with computed `root_account_id` (nil for a root account). Deleted
//!   accounts are hidden; `recursive=1` returns the whole subtree tier by
//!   tier; `order=name` sorts; `include[]=course_count` /
//!   `include[]=sub_account_count` add the counts.
//! - /manageable_accounts and /course_creation_accounts list the caller's
//!   adminable accounts plus their non-deleted subtrees; students and
//!   teachers without account grants get `[]`.

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
        "acc-p3-{}-{}-{}",
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

async fn seed_account(pool: &PgPool, name: &str, parent: Option<i64>, workflow_state: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO accounts (name, parent_account_id, workflow_state)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(parent)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_course(pool: &PgPool, name: &str, account_id: i64, workflow_state: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code, account_id, workflow_state)
        VALUES ($1, $1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(account_id)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed course")
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

async fn cleanup(pool: &PgPool, prefix: &str) {
    // courses.account_id has no cascade, so drop the courses first.
    let _ = sqlx::query("DELETE FROM courses WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    // account_users cascades from both users and accounts.
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

fn account_ids(body: &Value) -> Vec<i64> {
    body.as_array()
        .unwrap_or_else(|| panic!("bare array expected, got {body}"))
        .iter()
        .map(|row| row["id"].as_i64().expect("account id"))
        .collect()
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_settings_get_update_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let manager = seed_user(&pool, &prefix, "Manager", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None, "active").await;
    let token = auth(admin, "admin");

    // A fresh account exposes an empty settings object (Canvas renders {}).
    let resp = server
        .get(&format!("/accounts/{account}/settings"))
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({}));

    // PUT stores the posted keys and echoes the merged object.
    let resp = server
        .put(&format!("/accounts/{account}/settings"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({
            "microsoft_sync_enabled": true,
            "enable_as_k5_account": true
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["microsoft_sync_enabled"], true);
    assert_eq!(created["enable_as_k5_account"], true);

    let resp = server
        .get(&format!("/accounts/{account}/settings"))
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let stored_body: Value = resp.json();
    assert_eq!(stored_body, created, "GET must echo the stored settings");

    // A second PUT shallow-merges: existing keys survive, new keys land.
    let resp = server
        .put(&format!("/accounts/{account}/settings"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({
            "microsoft_sync_enabled": false,
            "custom_key": "custom-value"
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let merged: Value = resp.json();
    assert_eq!(merged["microsoft_sync_enabled"], false);
    assert_eq!(merged["enable_as_k5_account"], true);
    assert_eq!(merged["custom_key"], "custom-value");

    let stored: String = sqlx::query_scalar("SELECT settings::text FROM accounts WHERE id = $1")
        .bind(account)
        .fetch_one(&pool)
        .await
        .expect("load settings");
    let stored: Value = serde_json::from_str(&stored).expect("settings json");
    assert_eq!(stored, merged, "merged settings must persist");

    // Non-object payloads are rejected without touching the stored settings.
    for payload in [json!(["not", "an", "object"]), json!("plain string")] {
        let resp = server
            .put(&format!("/accounts/{account}/settings"))
            .add_header(AUTHORIZATION, token.clone())
            .json(&payload)
            .await;
        assert_eq!(
            resp.status_code(),
            400,
            "payload {payload}: {}",
            resp.text()
        );
    }
    let resp = server
        .get(&format!("/accounts/{account}/settings"))
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.json::<Value>(), merged);

    // Non-admins are rejected by the account-admin gate.
    let resp = server
        .get(&format!("/accounts/{account}/settings"))
        .add_header(AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .put(&format!("/accounts/{account}/settings"))
        .add_header(AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "microsoft_sync_enabled": true }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Missing accounts 404 for an admin.
    let resp = server
        .get("/accounts/999999999/settings")
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .put("/accounts/999999999/settings")
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "microsoft_sync_enabled": true }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A scoped account admin (manage_account_settings) can read and write.
    grant_account_admin(&pool, account, manager).await;
    let resp = server
        .get(&format!("/accounts/{account}/settings"))
        .add_header(AUTHORIZATION, auth(manager, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), merged);

    let resp = server
        .put(&format!("/accounts/{account}/settings"))
        .add_header(AUTHORIZATION, auth(manager, "teacher"))
        .json(&json!({ "teachers_can_create_courses": false }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let managed: Value = resp.json();
    assert_eq!(managed["teachers_can_create_courses"], false);
    assert_eq!(managed["custom_key"], "custom-value");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Sub-accounts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sub_accounts_listing_recursive_order_and_includes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let token = auth(admin, "admin");

    let root = seed_account(&pool, &format!("{prefix}-Root"), None, "active").await;
    let alpha = seed_account(&pool, &format!("{prefix}-Alpha"), Some(root), "active").await;
    // Grandchild is inserted before Beta, so tier ordering is observable in
    // the recursive listing (Beta has the higher id but is one tier up).
    let grandchild = seed_account(
        &pool,
        &format!("{prefix}-Grandchild"),
        Some(alpha),
        "active",
    )
    .await;
    let beta = seed_account(&pool, &format!("{prefix}-Beta"), Some(root), "active").await;
    let deleted = seed_account(&pool, &format!("{prefix}-Deleted"), Some(root), "deleted").await;

    seed_course(&pool, &format!("{prefix}-Course1"), alpha, "available").await;
    seed_course(&pool, &format!("{prefix}-Course2"), alpha, "deleted").await;

    // Direct children only: deleted accounts are hidden, id order.
    let resp = server
        .get(&format!("/accounts/{root}/sub_accounts"))
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let direct: Value = resp.json();
    assert_eq!(account_ids(&direct), vec![alpha, beta]);
    let first = &direct.as_array().unwrap()[0];
    assert_eq!(first["parent_account_id"], root);
    assert_eq!(first["root_account_id"], root);
    assert_eq!(first["workflow_state"], "active");
    assert!(
        first.get("course_count").is_none(),
        "counts only appear when included: {first}"
    );
    assert!(
        !account_ids(&direct).contains(&deleted),
        "deleted accounts must not be listed: {direct}"
    );

    // recursive=1 walks the whole subtree tier by tier (children before
    // grandchildren, regardless of id order).
    let resp = server
        .get(&format!("/accounts/{root}/sub_accounts"))
        .add_query_params([("recursive", "1")])
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let recursive: Value = resp.json();
    assert_eq!(account_ids(&recursive), vec![alpha, beta, grandchild]);
    let leaf = recursive
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"].as_i64() == Some(grandchild))
        .expect("grandchild listed");
    assert_eq!(leaf["parent_account_id"], alpha);
    assert_eq!(leaf["root_account_id"], root);

    // order=name sorts alphabetical; includes add the Canvas counts.
    let resp = server
        .get(&format!("/accounts/{root}/sub_accounts"))
        .add_query_params([
            ("order", "name"),
            ("include[]", "course_count"),
            ("include[]", "sub_account_count"),
        ])
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let named: Value = resp.json();
    let rows = named.as_array().expect("bare array");
    let names: Vec<String> = rows
        .iter()
        .map(|row| row["name"].as_str().expect("name").to_string())
        .collect();
    assert_eq!(
        names,
        vec![format!("{prefix}-Alpha"), format!("{prefix}-Beta")]
    );
    let alpha_row = &rows[0];
    assert_eq!(alpha_row["course_count"], 1, "deleted courses excluded");
    assert_eq!(alpha_row["sub_account_count"], 1);
    let beta_row = &rows[1];
    assert_eq!(beta_row["course_count"], 0);
    assert_eq!(beta_row["sub_account_count"], 0);

    // Account-admin gate: non-admins 403, missing accounts 404.
    let resp = server
        .get(&format!("/accounts/{root}/sub_accounts"))
        .add_header(AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .get("/accounts/999999999/sub_accounts")
        .add_header(AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Manageable + course-creation accounts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn manageable_and_course_creation_accounts() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let manager = seed_user(&pool, &prefix, "Manager", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;

    let root = seed_account(&pool, &format!("{prefix}-Root"), None, "active").await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(root), "active").await;
    let grandchild = seed_account(
        &pool,
        &format!("{prefix}-Grandchild"),
        Some(child),
        "active",
    )
    .await;
    let sibling = seed_account(&pool, &format!("{prefix}-Sibling"), None, "active").await;
    grant_account_admin(&pool, root, manager).await;

    let admin_token = auth(admin, "admin");

    // Global admins administer every account; hierarchy fields are present.
    let resp = server
        .get("/manageable_accounts")
        .add_header(AUTHORIZATION, admin_token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let all: Value = resp.json();
    let ids = account_ids(&all);
    for expected in [root, child, grandchild, sibling] {
        assert!(ids.contains(&expected), "account {expected} missing: {all}");
    }
    let rows = all.as_array().unwrap();
    let root_row = rows.iter().find(|row| row["id"] == root).unwrap();
    assert!(
        root_row["root_account_id"].is_null(),
        "root accounts have null root_account_id: {root_row}"
    );
    let child_row = rows.iter().find(|row| row["id"] == child).unwrap();
    assert_eq!(child_row["root_account_id"], root);
    assert_eq!(child_row["parent_account_id"], root);

    // Pagination applies to the flattened list.
    let resp = server
        .get("/manageable_accounts")
        .add_query_params([("per_page", "1")])
        .add_header(AUTHORIZATION, admin_token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(account_ids(&resp.json::<Value>()).len(), 1);

    // Account admins see their account plus its subtree, and nothing else.
    let manager_token = auth(manager, "teacher");
    let resp = server
        .get("/manageable_accounts")
        .add_header(AUTHORIZATION, manager_token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        account_ids(&resp.json::<Value>()),
        vec![root, child, grandchild]
    );

    let resp = server
        .get("/course_creation_accounts")
        .add_header(AUTHORIZATION, manager_token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let creatable: Value = resp.json();
    assert_eq!(account_ids(&creatable), vec![root, child, grandchild]);
    assert!(
        !account_ids(&creatable).contains(&sibling),
        "sibling account must stay hidden: {creatable}"
    );

    // Global admins can create courses anywhere, including the sibling tree.
    let resp = server
        .get("/course_creation_accounts")
        .add_header(AUTHORIZATION, admin_token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert!(account_ids(&resp.json::<Value>()).contains(&sibling));

    // Students without account grants get an empty list on both routes.
    let student_token = auth(student, "student");
    for path in ["/manageable_accounts", "/course_creation_accounts"] {
        let resp = server
            .get(path)
            .add_header(AUTHORIZATION, student_token.clone())
            .await;
        assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
        assert_eq!(resp.json::<Value>(), json!([]));
    }

    // Authentication is required.
    let resp = server.get("/manageable_accounts").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
