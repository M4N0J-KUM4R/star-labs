//! Integration tests for the Canvas SIS imports routes.
//!
//! Pinned behavior:
//! - the controller guards run in the Ruby order: an unknown account is a 404,
//!   a sub-account is a 400, an account without SIS imports is a 403 and only
//!   then a caller without the right gets a 403;
//! - `GET /accounts/{account_id}/sis_imports` answers
//!   `{"sis_imports": [...]}` newest first and honours the `created_since`,
//!   `created_before` and `workflow_state[]` filters;
//! - `GET /accounts/{account_id}/sis_imports/importing` answers the running
//!   imports only;
//! - `GET /accounts/{account_id}/sis_imports/{id}` answers the bare import and
//!   is scoped to the account;
//! - `PUT .../abort` flips a running import to `aborted`, a finished import is
//!   a 404;
//! - `PUT .../abort_all_pending` answers `{"aborted": true}` and only touches
//!   the imports that have not started yet;
//! - `PUT .../restore_states` answers a bare Progress object, needs roll back
//!   data and refuses `undelete_only` together with `unconclude_only`;
//! - `POST /accounts/{account_id}/sis_imports` stores the batch (raw body or
//!   multipart upload), answers the import object and rejects an unknown
//!   `import_type` with a 500.
//!
//! Fleet rule: dedicated users, accounts and batches per test, every seeded
//! row is cleaned up. Skipped cleanly when DATABASE_URL is unset.
//!
//! The `allow_sis_import` column of `accounts` is added by this test (the same
//! DDL the migration needs) so the suite runs before the migration lands;
//! `ADD COLUMN IF NOT EXISTS` keeps it a no-op afterwards.
//!
//! The router of this module is mounted through `star_sis::sis_imports_api`,
//! so `crates/star-sis/src/lib.rs` has to declare `pub mod sis_imports_api;`.

use axum::body::Bytes;
use axum_test::multipart::{MultipartForm, Part};
use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

/// The DDL the SIS imports routes need.
const SIS_DDL: [&str; 1] = ["ALTER TABLE accounts \
     ADD COLUMN IF NOT EXISTS allow_sis_import BOOLEAN NOT NULL DEFAULT false"];

/// A unique prefix for the rows of one test.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "sis-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
        n
    )
}

/// Connects to the database, runs the migrations and builds the test server.
async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    for statement in SIS_DDL {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("create the sis import tables");
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(star_sis::sis_imports_api::router().with_state(state))
        .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

/// Seeds a user.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Seeds a root account.
async fn seed_root_account(pool: &PgPool, prefix: &str, name: &str, sis_imports: bool) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name, allow_sis_import) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(sis_imports)
        .fetch_one(pool)
        .await
        .expect("seed root account")
}

/// Seeds a sub account.
async fn seed_sub_account(pool: &PgPool, prefix: &str, name: &str, parent_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id, allow_sis_import) \
         VALUES ($1, $2, true) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .expect("seed sub account")
}

/// Makes the user an admin of the account.
async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role, workflow_state) \
         VALUES ($1, $2, 'admin', 'active')",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account user");
}

/// Seeds a `sis_batches` row.
async fn seed_batch(
    pool: &PgPool,
    account_id: i64,
    user_id: Option<i64>,
    workflow_state: &str,
    created_at: &str,
    data: &str,
    options: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO sis_batches (account_id, workflow_state, created_at, updated_at, progress, \
         data, options, user_id) \
         VALUES ($1, $2, $3::timestamp, $3::timestamp, 0, $4, $5, $6) RETURNING id",
    )
    .bind(account_id)
    .bind(workflow_state)
    .bind(created_at)
    .bind(data)
    .bind(options)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed sis batch")
}

/// The bearer token of the user.
fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

/// Removes the rows of the test.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM attachments WHERE context_type = 'SisBatch' AND context_id IN \
           (SELECT id FROM sis_batches WHERE account_id IN \
              (SELECT id FROM accounts WHERE name LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM sis_batches WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
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

#[tokio::test]
async fn an_account_admin_lists_the_sis_imports_newest_first() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    seed_account_admin(&pool, account, admin).await;

    let old = seed_batch(
        &pool,
        account,
        None,
        "created",
        "2024-01-01 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;
    let middle = seed_batch(
        &pool,
        account,
        None,
        "imported",
        "2024-02-01 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;
    let new = seed_batch(
        &pool,
        account,
        None,
        "aborted",
        "2024-03-01 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;

    let resp = server
        .get(&format!("/accounts/{account}/sis_imports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let imports = body["sis_imports"].as_array().expect("sis_imports array");
    assert_eq!(imports.len(), 3, "body: {body}");
    assert_eq!(imports[0]["id"], new);
    assert_eq!(imports[1]["id"], middle);
    assert_eq!(imports[2]["id"], old);

    // The Canvas field names.
    assert_eq!(imports[0]["workflow_state"], "aborted");
    assert_eq!(imports[0]["data"]["import_type"], "instructure_csv");
    assert_eq!(imports[0]["progress"], 0.0);
    assert_eq!(imports[0]["batch_mode"], Value::Null);
    assert_eq!(imports[0]["skip_deletes"], Value::Null);
    assert!(
        imports[0].get("processing_errors").is_none(),
        "body: {body}"
    );
    assert!(imports[0].get("csv_attachments").is_none(), "body: {body}");

    // The workflow_state filter intersects with the list of valid states.
    let resp = server
        .get(&format!(
            "/accounts/{account}/sis_imports?workflow_state%5B%5D=imported"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let imports = body["sis_imports"].as_array().expect("sis_imports array");
    assert_eq!(imports.len(), 1, "body: {body}");
    assert_eq!(imports[0]["id"], middle);

    // An unknown state is not a filter at all.
    let resp = server
        .get(&format!(
            "/accounts/{account}/sis_imports?workflow_state%5B%5D=nonsense"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["sis_imports"]
            .as_array()
            .expect("sis_imports array")
            .len(),
        3
    );

    // The date bounds.
    let resp = server
        .get(&format!(
            "/accounts/{account}/sis_imports?created_since=2024-01-15T00:00:00Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["sis_imports"]
            .as_array()
            .expect("sis_imports array")
            .len(),
        2
    );

    let resp = server
        .get(&format!(
            "/accounts/{account}/sis_imports?created_before=2024-02-15T00:00:00Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["sis_imports"]
            .as_array()
            .expect("sis_imports array")
            .len(),
        2
    );

    // The pagination.
    let resp = server
        .get(&format!("/accounts/{account}/sis_imports?per_page=1"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["sis_imports"]
            .as_array()
            .expect("sis_imports array")
            .len(),
        1
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_account_guards_run_before_the_permission_check() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    let sub = seed_sub_account(&pool, &prefix, "Sub", account).await;
    let disabled = seed_root_account(&pool, &prefix, "Disabled", false).await;
    seed_account_admin(&pool, account, admin).await;
    seed_account_admin(&pool, sub, admin).await;
    seed_account_admin(&pool, disabled, admin).await;

    // A sub account is a 400.
    let resp = server
        .get(&format!("/accounts/{sub}/sis_imports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"][0],
        "SIS imports can only be executed on root accounts"
    );

    // An account without SIS imports is a 403.
    let resp = server
        .get(&format!("/accounts/{disabled}/sis_imports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"][0],
        "SIS imports are not enabled for this account"
    );

    // An unknown account is a 404.
    let resp = server
        .get("/accounts/999999999/sis_imports")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"][0]["message"],
        "The specified resource does not exist."
    );

    // A caller without the right is a 403.
    let resp = server
        .get(&format!("/accounts/{account}/sis_imports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "unauthorized");
    assert_eq!(
        body["errors"][0]["message"],
        "user not authorized to perform that action"
    );

    // An anonymous caller is a 401.
    let resp = server
        .get(&format!("/accounts/{account}/sis_imports"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn importing_lists_only_the_running_imports() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    seed_account_admin(&pool, account, admin).await;

    let running = seed_batch(
        &pool,
        account,
        None,
        "importing",
        "2024-01-01 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;
    seed_batch(
        &pool,
        account,
        None,
        "cleanup_batch",
        "2024-01-02 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;
    seed_batch(
        &pool,
        account,
        None,
        "imported",
        "2024-01-03 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;
    let finished = seed_batch(
        &pool,
        account,
        None,
        "imported",
        "2024-01-04 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;

    let resp = server
        .get(&format!("/accounts/{account}/sis_imports/importing"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let imports = body["sis_imports"].as_array().expect("sis_imports array");
    assert_eq!(imports.len(), 2, "body: {body}");
    let ids: Vec<i64> = imports
        .iter()
        .map(|item| item["id"].as_i64().expect("id"))
        .collect();
    assert!(ids.contains(&running), "body: {body}");
    assert!(!ids.contains(&finished), "body: {body}");
    let states: Vec<String> = imports
        .iter()
        .map(|item| item["workflow_state"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(states.contains(&"importing".to_string()), "body: {body}");
    assert!(states.contains(&"cleanup_batch".to_string()), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_answers_the_import_and_is_scoped_to_the_account() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    let other = seed_root_account(&pool, &prefix, "Other", true).await;
    seed_account_admin(&pool, account, admin).await;
    seed_account_admin(&pool, other, admin).await;

    let batch = seed_batch(
        &pool,
        account,
        Some(admin),
        "imported",
        "2024-01-01 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        r#"{"skip_deletes":true}"#,
    )
    .await;
    let foreign = seed_batch(
        &pool,
        other,
        None,
        "imported",
        "2024-01-01 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;

    let resp = server
        .get(&format!("/accounts/{account}/sis_imports/{batch}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], batch);
    assert_eq!(body["workflow_state"], "imported");
    assert_eq!(body["skip_deletes"], true);
    assert_eq!(body["user"]["id"], admin);

    // A batch of another account is a 404.
    let resp = server
        .get(&format!("/accounts/{account}/sis_imports/{foreign}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // An unknown batch is a 404.
    let resp = server
        .get(&format!("/accounts/{account}/sis_imports/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn abort_flips_a_running_import_and_refuses_a_finished_one() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    seed_account_admin(&pool, account, admin).await;

    let running = seed_batch(
        &pool,
        account,
        None,
        "importing",
        "2024-01-01 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;
    let finished = seed_batch(
        &pool,
        account,
        None,
        "imported",
        "2024-01-02 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;

    let resp = server
        .put(&format!("/accounts/{account}/sis_imports/{running}/abort"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], running);
    assert_eq!(body["workflow_state"], "aborted");

    let state: String = sqlx::query_scalar("SELECT workflow_state FROM sis_batches WHERE id = $1")
        .bind(running)
        .fetch_one(&pool)
        .await
        .expect("read the batch");
    assert_eq!(state, "aborted");

    // A finished import cannot be aborted, it is a 404.
    let resp = server
        .put(&format!("/accounts/{account}/sis_imports/{finished}/abort"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn abort_all_pending_only_touches_the_imports_that_have_not_started() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    seed_account_admin(&pool, account, admin).await;

    let pending = seed_batch(
        &pool,
        account,
        None,
        "created",
        "2024-01-01 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;
    let running = seed_batch(
        &pool,
        account,
        None,
        "importing",
        "2024-01-02 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;

    let resp = server
        .put(&format!(
            "/accounts/{account}/sis_imports/abort_all_pending"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["aborted"], true);

    let pending_state: String =
        sqlx::query_scalar("SELECT workflow_state FROM sis_batches WHERE id = $1")
            .bind(pending)
            .fetch_one(&pool)
            .await
            .expect("read the batch");
    assert_eq!(pending_state, "aborted");
    let pending_progress: i64 =
        sqlx::query_scalar("SELECT progress FROM sis_batches WHERE id = $1")
            .bind(pending)
            .fetch_one(&pool)
            .await
            .expect("read the batch");
    assert_eq!(pending_progress, 100);

    let running_state: String =
        sqlx::query_scalar("SELECT workflow_state FROM sis_batches WHERE id = $1")
            .bind(running)
            .fetch_one(&pool)
            .await
            .expect("read the batch");
    assert_eq!(running_state, "importing");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn restore_states_answers_a_progress_object() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    seed_account_admin(&pool, account, admin).await;

    let batch = seed_batch(
        &pool,
        account,
        None,
        "imported",
        "2024-01-01 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;
    let without_roll_back = seed_batch(
        &pool,
        account,
        None,
        "imported",
        "2024-01-02 10:00:00",
        r#"{"import_type":"instructure_csv"}"#,
        "{}",
    )
    .await;

    // Without roll back data the restore is a 400.
    let resp = server
        .put(&format!(
            "/accounts/{account}/sis_imports/{without_roll_back}/restore_states"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "restore data unavailable");

    // With roll back data the restore answers a progress object.
    sqlx::query(
        "INSERT INTO sis_batch_roll_back_data (sis_batch_id, context_id, context_type, \
         previous_workflow_state, updated_workflow_state, batch_mode_delete, workflow_state, \
         created_at, updated_at) \
         VALUES ($1, 1, 'Course', 'active', 'deleted', false, 'active', now(), now())",
    )
    .bind(batch)
    .execute(&pool)
    .await
    .expect("seed the roll back data");

    let resp = server
        .put(&format!(
            "/accounts/{account}/sis_imports/{batch}/restore_states?undelete_only=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["tag"], "sis_batch_state_restore");
    assert_eq!(body["context_type"], "SisBatch");
    assert_eq!(body["context_id"], batch);
    assert_eq!(body["workflow_state"], "queued");
    assert_eq!(body["completion"], 0.0);
    assert_eq!(body["user_id"], Value::Null);
    assert!(
        body["url"]
            .as_str()
            .expect("url")
            .starts_with("/api/v1/progress/"),
        "body: {body}"
    );

    // `undelete_only` and `unconclude_only` cannot be combined.
    let resp = server
        .put(&format!(
            "/accounts/{account}/sis_imports/{batch}/restore_states?undelete_only=1&unconclude_only=1"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(admin, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["message"],
        "cannot set both undelete_only and unconclude_only"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_stores_the_batch_of_a_raw_post() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    seed_account_admin(&pool, account, admin).await;

    let resp = server
        .post(&format!("/accounts/{account}/sis_imports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .content_type("text/csv")
        .bytes(Bytes::from_static(b"user_id,login_id\n1,student1\n"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "created");
    assert_eq!(body["data"]["import_type"], "instructure_csv");
    assert_eq!(body["skip_deletes"], false);
    assert_eq!(body["progress"], 0.0);
    let batch_id = body["id"].as_i64().expect("id");

    // The attachment of the upload is stored with the batch.
    let attachments: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT filename, content_type, size FROM attachments \
         WHERE context_type = 'SisBatch' AND context_id = $1",
    )
    .bind(batch_id)
    .fetch_all(&pool)
    .await
    .expect("read the attachment");
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].0, "sis_import.csv");
    assert_eq!(attachments[0].1, "text/csv");
    assert_eq!(attachments[0].2, 28);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_stores_the_batch_of_a_multipart_upload() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    seed_account_admin(&pool, account, admin).await;

    let form = MultipartForm::new()
        .add_text("import_type", "instructure_csv")
        .add_text("skip_deletes", "true")
        .add_part(
            "attachment",
            Part::bytes("user_id,login_id\n1,student1\n")
                .file_name("users.csv")
                .mime_type("text/csv"),
        );

    let resp = server
        .post(&format!("/accounts/{account}/sis_imports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .multipart(form)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "created");
    assert_eq!(body["data"]["import_type"], "instructure_csv");
    assert_eq!(body["skip_deletes"], true);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_rejects_an_unknown_import_type_and_a_missing_change_threshold() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_root_account(&pool, &prefix, "Account", true).await;
    seed_account_admin(&pool, account, admin).await;

    // An unknown import type is the 500 of the Ruby `raise`.
    let resp = server
        .post(&format!(
            "/accounts/{account}/sis_imports?import_type=nonsense"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .content_type("text/csv")
        .bytes(Bytes::from_static(b"user_id\n1\n"))
        .await;
    assert_eq!(resp.status_code(), 500, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["errors"][0]["message"], "An error occurred.");
    assert_eq!(body["errors"][0]["error_code"], "internal_server_error");

    // The multi term batch mode needs a change threshold.
    let resp = server
        .post(&format!(
            "/accounts/{account}/sis_imports?multi_term_batch_mode=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .content_type("text/csv")
        .bytes(Bytes::from_static(b"user_id\n1\n"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["message"],
        "change_threshold is required to use multi term_batch mode."
    );

    cleanup(&pool, &prefix).await;
}
