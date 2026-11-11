//! Integration tests for the Phase 4 account-admin routes added to
//! star-accounts:
//!
//! - PUT    /accounts/{accountId}
//! - GET    /manually_created_courses_account
//! - DELETE /accounts/{accountId}/users/{userId}
//! - GET    /acceptable_use_policy
//!
//! Conventions mirror `account_settings.rs`: skipped cleanly when DATABASE_URL
//! is unset, unique per-run prefix, and full teardown (including cases where a
//! test touches the seeded root account) so the suite is safe against a shared
//! database.
//!
//! Behavior pinned (Canvas parity + known deviations):
//! - PUT accepts Canvas' nested `account` object and flat JSON. It renames,
//!   shallow-merges `settings`, and re-parents; roots cannot move (400), a
//!   move into the account's own subtree is 400, a missing target 404, a
//!   caller outside the account-admin gate 403 (checked before existence).
//!   Blank names are 422 (this crate's validation status; Canvas renders 400)
//!   and non-object `settings` are 400 like the settings route.
//! - GET /manually_created_courses_account is idempotent: it creates the
//!   "Manually-Created Courses" child of the root account once and remembers
//!   the id in `accounts.settings.manually_created_courses_account_id`.
//!   Authentication is required (401).
//! - DELETE .../users/{userId} is root-account only (404 on a sub-account),
//!   404 when the user has neither an `account_users` grant nor an enrollment
//!   in the account subtree, and 403 outside the account-admin gate. Because
//!   `users` has no workflow_state, the row is hard-deleted (cascading grants
//!   and enrollments) instead of Canvas's soft delete, and the pre-delete user
//!   object is returned.
//! - GET /acceptable_use_policy is anonymous (Canvas skips `require_user`) and
//!   renders `{"redirectUrl": ...}` when the root account has an external URL,
//!   otherwise `{"content": <nullable>}`.

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
        "acc-p4-{}-{}-{}",
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

/// The handler's `@domain_root_account`: the lowest surviving top-level account.
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
// PUT /accounts/{accountId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_account_rename_settings_and_move() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let token = auth(admin, "admin");

    let root = seed_account(&pool, &format!("{prefix}-Root"), None).await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(root)).await;
    let other = seed_account(&pool, &format!("{prefix}-Other"), None).await;

    // Canvas nests the attributes under `account`; settings shallow-merge.
    let renamed = format!("{prefix}-Renamed");
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({
            "account": { "name": renamed, "settings": { "deep_flag": true } }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["id"], child);
    assert_eq!(updated["name"], renamed);
    assert_eq!(updated["parent_account_id"], root);
    assert_eq!(updated["root_account_id"], root);
    assert_eq!(updated["workflow_state"], "active");
    assert!(
        updated.get("course_count").is_none(),
        "counts are optional includes: {updated}"
    );

    let stored: String = sqlx::query_scalar("SELECT settings::text FROM accounts WHERE id = $1")
        .bind(child)
        .fetch_one(&pool)
        .await
        .expect("load settings");
    assert_eq!(
        serde_json::from_str::<Value>(&stored).expect("settings json")["deep_flag"],
        true
    );

    // Flat JSON is accepted too (JSON clients may skip the `account` wrapper).
    let flat = format!("{prefix}-Flat");
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "name": flat }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["name"], flat);

    // Moving re-parents and recomputes root_account_id.
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "account": { "parent_account_id": other } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let moved: Value = resp.json();
    assert_eq!(moved["parent_account_id"], other);
    assert_eq!(moved["root_account_id"], other);

    // A root account cannot be moved; an account cannot move into itself or
    // into its own subtree.
    let resp = server
        .put(&format!("/accounts/{root}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "account": { "parent_account_id": other } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "account": { "parent_account_id": child } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Missing move target 404s; blank name 422s; bad settings type 400s.
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "account": { "parent_account_id": 999_999_999i64 } }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "account": { "name": "   " } }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "account": { "settings": "nope" } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Account-admin gate first (403 even for unknown ids), 404 for admins,
    // 401 without a token.
    let resp = server
        .put(&format!("/accounts/{child}"))
        .add_header(AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "account": { "name": "Nope" } }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .put("/accounts/999999999")
        .add_header(AUTHORIZATION, token.clone())
        .json(&json!({ "account": { "name": "Nope" } }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .put(&format!("/accounts/{child}"))
        .json(&json!({ "account": { "name": "Nope" } }))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /manually_created_courses_account
// ---------------------------------------------------------------------------

#[tokio::test]
async fn manually_created_courses_account_is_created_once() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let root = root_account(&pool).await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let token = auth(admin, "admin");

    // Snapshot root settings and any pre-existing account so teardown restores
    // a shared database exactly.
    let settings_before: String =
        sqlx::query_scalar("SELECT settings::text FROM accounts WHERE id = $1")
            .bind(root)
            .fetch_one(&pool)
            .await
            .expect("root settings");
    let existing: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM accounts
        WHERE parent_account_id = $1 AND name = 'Manually-Created Courses'
        "#,
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .expect("existing manually created accounts");

    // Authentication is required.
    let resp = server.get("/manually_created_courses_account").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    let resp = server
        .get("/manually_created_courses_account")
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let account: Value = resp.json();
    let created = account["id"].as_i64().expect("account id");
    assert_eq!(account["name"], "Manually-Created Courses");
    assert_eq!(account["parent_account_id"], root);
    assert_eq!(account["root_account_id"], root);
    assert_eq!(account["workflow_state"], "active");

    // Idempotent: the remembered id is reused, and the root settings point at
    // the account.
    let resp = server
        .get("/manually_created_courses_account")
        .add_header(AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["id"], created);

    let stored: Option<String> = sqlx::query_scalar(
        "SELECT settings->>'manually_created_courses_account_id' FROM accounts WHERE id = $1",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .expect("stored settings id");
    assert_eq!(
        stored.and_then(|value| value.parse::<i64>().ok()),
        Some(created)
    );

    if !existing.contains(&created) {
        let _ = sqlx::query("DELETE FROM accounts WHERE id = $1")
            .bind(created)
            .execute(&pool)
            .await;
    }
    let _ = sqlx::query("UPDATE accounts SET settings = $2::jsonb WHERE id = $1")
        .bind(root)
        .bind(settings_before)
        .execute(&pool)
        .await;

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /accounts/{accountId}/users/{userId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn remove_user_from_root_account() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let root = root_account(&pool).await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let member = seed_user(&pool, &prefix, "Member", "teacher").await;
    let unassociated = seed_user(&pool, &prefix, "Stranger", "student").await;
    let nested_member = seed_user(&pool, &prefix, "Nested", "teacher").await;

    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(root)).await;
    grant_account_admin(&pool, root, member).await;
    grant_account_admin(&pool, child, nested_member).await;

    // Unauthenticated 401, non-admin 403.
    let resp = server
        .delete(&format!("/accounts/{root}/users/{member}"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .delete(&format!("/accounts/{root}/users/{member}"))
        .add_header(AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Non-root accounts are always 404 (Canvas raises RecordNotFound).
    let resp = server
        .delete(&format!("/accounts/{child}/users/{nested_member}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A user without any account association is 404.
    let resp = server
        .delete(&format!("/accounts/{root}/users/{unassociated}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Associated user: 200 with the pre-delete user object, row removed.
    let resp = server
        .delete(&format!("/accounts/{root}/users/{member}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let removed: Value = resp.json();
    assert_eq!(removed["id"], member);
    assert_eq!(removed["name"], "Member");
    assert_eq!(
        removed["email"],
        format!("{prefix}-Member@test.local"),
        "user object is returned: {removed}"
    );
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = $1")
        .bind(member)
        .fetch_one(&pool)
        .await
        .expect("count users");
    assert_eq!(remaining, 0, "user row is hard-deleted");

    // Second delete 404s because the user no longer exists.
    let resp = server
        .delete(&format!("/accounts/{root}/users/{member}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /acceptable_use_policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acceptable_use_policy_renders_content_or_redirect() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _ = &prefix;

    // Serialize against the other ToS test binaries (shared root account).
    let mut lock_conn = pool.acquire().await.expect("lock connection");
    sqlx::query("SELECT pg_advisory_lock(8901)")
        .execute(&mut *lock_conn)
        .await
        .expect("acquire ToS lock");
    let root = root_account(&pool).await;
    let before: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT external_url, content, terms_type FROM account_terms_of_service WHERE account_id = $1",
    )
    .bind(root)
    .fetch_optional(&pool)
    .await
    .expect("snapshot terms of service");
    let _ = sqlx::query("DELETE FROM account_terms_of_service WHERE account_id = $1")
        .bind(root)
        .execute(&pool)
        .await;

    // Anonymous access (Canvas skips require_user); no policy row -> null.
    let resp = server.get("/acceptable_use_policy").await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({ "content": null }));

    // Stored content is rendered under `content`.
    sqlx::query(
        "INSERT INTO account_terms_of_service (account_id, terms_type, content) VALUES ($1, 'built_in:default', $2)",
    )
    .bind(root)
    .bind(format!("{prefix}-policy-body"))
    .execute(&pool)
    .await
    .expect("insert policy");

    let resp = server.get("/acceptable_use_policy").await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>(),
        json!({ "content": format!("{prefix}-policy-body") })
    );

    // An external URL wins and replaces `content` with `redirectUrl`.
    let external = format!("https://{prefix}.example.test/aup");
    sqlx::query("UPDATE account_terms_of_service SET external_url = $2 WHERE account_id = $1")
        .bind(root)
        .bind(&external)
        .execute(&pool)
        .await
        .expect("set external url");

    let resp = server.get("/acceptable_use_policy").await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["redirectUrl"], external);
    assert!(
        body.get("content").is_none(),
        "Canvas renders only redirectUrl: {body}"
    );

    // Restore any pre-existing row.
    let _ = sqlx::query("DELETE FROM account_terms_of_service WHERE account_id = $1")
        .bind(root)
        .execute(&pool)
        .await;
    if let Some((external_url, content, terms_type)) = before {
        let _ = sqlx::query(
            r#"
            INSERT INTO account_terms_of_service
                (account_id, terms_type, content, external_url)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(root)
        .bind(terms_type)
        .bind(content)
        .bind(external_url)
        .execute(&pool)
        .await;
    }

    let _ = sqlx::query("SELECT pg_advisory_unlock(8901)")
        .execute(&mut *lock_conn)
        .await;
    drop(lock_conn);
    cleanup(&pool, &prefix).await;
}
