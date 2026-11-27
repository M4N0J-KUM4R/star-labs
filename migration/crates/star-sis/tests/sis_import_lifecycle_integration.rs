//! Integration tests for the SIS import lifecycle (issue #92).
//!
//! The routes themselves are covered by `sis_imports_api_integration.rs`
//! and `sis_import_errors_integration.rs`; this file pins the batch
//! **state machine** and the finished-import shapes that the issue's
//! acceptance calls out:
//!
//! - `POST /accounts/{id}/sis_imports` stores the batch in `created` and
//!   records the durable import job per the shared job-model convention
//!   (`specs/p5-rest-misc-index.md` §3): tag `SisBatch#process_instructure_csv`,
//!   `strand`/`singleton` `sis_batch:{account_id}` (Canvas
//!   `SisBatch.queue_job_for_account` sets `singleton: strand_for_account` —
//!   one import processes at a time per account), priority 20
//!   (`Delayed::LOW_PRIORITY`), `max_attempts` 1;
//! - a batch walks `created` → `importing` → `imported_with_messages` /
//!   `failed_with_messages`; while it runs it is listed by
//!   `GET .../sis_imports/importing` and the `workflow_state[]` filter, once
//!   terminal it disappears from both;
//! - a finished import surfaces `data.counts.error_count`/`warning_count`,
//!   the `processing_errors`/`processing_warnings` CSV-file/message pairs and
//!   the `errors_attachment` error file (Canvas
//!   `SisBatch#populate_old_warnings_and_errors` + `#write_errors_to_file`
//!   build exactly these; star-lms has no worker, so the tests seed them with
//!   the SQL a worker would run);
//! - `PUT .../abort` refuses a terminal (`failed_with_messages`) import with a
//!   404 (Canvas `sis_batches.not_completed.find`) and `abort_all_pending`
//!   leaves terminal rows untouched;
//! - tenant isolation: a batch (and its error listing) of another root
//!   account is a 404 and the account error listing never leaks foreign rows.
//!
//! Fleet rule: dedicated users, accounts, batches, errors and attachments per
//! test, every seeded row is cleaned up in FK-safe order. Skipped cleanly
//! when DATABASE_URL is unset.

use axum::body::Bytes;
use axum_test::multipart::{MultipartForm, Part};
use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "sislife-{}-{}-{}",
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
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = TestServer::new(star_sis::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_root_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, allow_sis_import) VALUES ($1, true) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed root account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id) VALUES ($1, $2)")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
}

async fn seed_batch(pool: &PgPool, account_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO sis_batches (account_id, workflow_state, progress, data, user_id, created_at, updated_at) \
         VALUES ($1, 'created', 0, '{\"import_type\":\"instructure_csv\"}', $2, now(), now()) \
         RETURNING id",
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed sis batch")
}

/// One `sis_import_errors` row (`failure` = a hard error, `!failure` = a
/// warning — Canvas `SisBatchError.failed` / `.warnings`).
async fn seed_error(
    pool: &PgPool,
    account_id: i64,
    batch_id: i64,
    file: &str,
    message: &str,
    failure: bool,
) {
    sqlx::query(
        "INSERT INTO sis_import_errors \
             (account_id, sis_batch_id, file, message, row_info, row, failure) \
         VALUES ($1, $2, $3, $4, $5, 3, $6)",
    )
    .bind(account_id)
    .bind(batch_id)
    .bind(file)
    .bind(message)
    .bind(format!("{file},line 3"))
    .bind(failure)
    .execute(pool)
    .await
    .expect("seed sis import error");
}

/// The error file Canvas `SisBatch#write_errors_to_file` attaches
/// (`sis_errors_attachment_{id}.csv`, context SisBatch).
async fn seed_error_file(pool: &PgPool, account_id: i64, user_id: i64, batch_id: i64) -> i64 {
    let name = format!("sis_errors_attachment_{batch_id}.csv");
    let uuid = format!("err-{batch_id}-{prefix}", prefix = run_prefix());
    let attachment_id: i64 = sqlx::query_scalar(
        "INSERT INTO attachments (context_id, context_type, size, content_type, filename, uuid, \
         display_name, created_at, updated_at, modified_at, workflow_state, file_state, \
         user_id, root_account_id, category) \
         VALUES ($1, 'SisBatch', 64, 'text/csv', $2, $3, $2, now(), now(), now(), 'available', \
         'available', $4, $5, 'uncategorized') RETURNING id",
    )
    .bind(batch_id)
    .bind(&name)
    .bind(&uuid)
    .bind(user_id)
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed the error file attachment");
    sqlx::query("UPDATE sis_batches SET errors_attachment_id = $1 WHERE id = $2")
        .bind(attachment_id)
        .bind(batch_id)
        .execute(pool)
        .await
        .expect("link the error file");
    attachment_id
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// FK-safe teardown: the job/progress rows first, then the error tables, the
/// attachments (their `errors_attachment_id` reference is nulled first) and
/// the batches, and finally the account fleet.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
    for statement in [
        "DELETE FROM jobs WHERE context_type = 'SisBatch' AND context_id IN \
           (SELECT id FROM sis_batches WHERE account_id IN \
              (SELECT id FROM accounts WHERE name LIKE $1))",
        "DELETE FROM progress WHERE context_type = 'SisBatch' AND context_id IN \
           (SELECT id FROM sis_batches WHERE account_id IN \
              (SELECT id FROM accounts WHERE name LIKE $1))",
        "UPDATE sis_batches SET errors_attachment_id = NULL, attachment_id = NULL \
           WHERE account_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
        "DELETE FROM attachments WHERE context_type = 'SisBatch' AND context_id IN \
           (SELECT id FROM sis_batches WHERE account_id IN \
              (SELECT id FROM accounts WHERE name LIKE $1))",
        "DELETE FROM sis_import_errors WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
        "DELETE FROM sis_batch_roll_back_data WHERE sis_batch_id IN \
           (SELECT id FROM sis_batches WHERE account_id IN \
              (SELECT id FROM accounts WHERE name LIKE $1))",
        "DELETE FROM sis_batches WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
        "DELETE FROM account_users WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
        "DELETE FROM accounts WHERE name LIKE $1",
        "DELETE FROM users WHERE email LIKE $1",
    ] {
        let _ = sqlx::query(statement).bind(&pattern).execute(pool).await;
    }
}

#[tokio::test]
async fn create_enqueues_the_account_strand_import_job() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_root_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;

    let form = MultipartForm::new().add_part(
        "attachment",
        Part::bytes("user_id,login_id\n1,student1\n")
            .file_name("users.csv")
            .mime_type("text/csv"),
    );
    let resp = server
        .post(&format!("/accounts/{account}/sis_imports"))
        .add_header("Authorization", auth(admin, "admin"))
        .multipart(form)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "created");
    let batch_id = body["id"].as_i64().expect("id");

    // The durable import job: the shared job-model convention maps Canvas's
    // `queue_job_for_account` onto strand/singleton `sis_batch:{account_id}`.
    let job: (String, String, Option<String>, Option<String>, i32, i32, String, i64) = sqlx::query_as(
        "SELECT tag, workflow_state, strand, singleton, priority, max_attempts, context_type, context_id \
         FROM jobs WHERE context_type = 'SisBatch' AND context_id = $1",
    )
    .bind(batch_id)
    .fetch_one(&pool)
    .await
    .expect("the import job row");
    assert_eq!(job.0, "SisBatch#process_instructure_csv");
    assert_eq!(job.1, "queued");
    let strand = format!("sis_batch:{account}");
    assert_eq!(job.2.as_deref(), Some(strand.as_str()), "strand");
    assert_eq!(job.3.as_deref(), Some(strand.as_str()), "singleton");
    assert_eq!(job.4, 20, "Delayed::LOW_PRIORITY");
    assert_eq!(job.5, 1, "Canvas queue_job_for_account max_attempts");
    assert_eq!(job.6, "SisBatch");
    assert_eq!(job.7, batch_id);

    // The upload is stored as the batch attachment (Canvas
    // `Attachment.create_data_attachment(batch, ...)`).
    let attachment: (String, String) = sqlx::query_as(
        "SELECT filename, context_type FROM attachments \
         WHERE context_type = 'SisBatch' AND context_id = $1",
    )
    .bind(batch_id)
    .fetch_one(&pool)
    .await
    .expect("the upload attachment");
    assert_eq!(attachment.0, "users.csv");
    assert_eq!(attachment.1, "SisBatch");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_import_walks_the_state_machine_to_imported_with_messages() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_root_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;

    // Canvas `create_with_attachment` answers the batch in `created`.
    let resp = server
        .post(&format!("/accounts/{account}/sis_imports"))
        .add_header("Authorization", auth(admin, "admin"))
        .content_type("text/csv")
        .bytes(Bytes::from_static(b"user_id,login_id\n1,student1\n"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    let batch = created["id"].as_i64().expect("id");
    assert_eq!(created["workflow_state"], "created");

    // The worker picks the batch up (`SisBatch#process_without_send_later`).
    sqlx::query(
        "UPDATE sis_batches SET workflow_state = 'importing', started_at = now(), updated_at = now() \
         WHERE id = $1",
    )
    .bind(batch)
    .execute(&pool)
    .await
    .expect("advance to importing");

    let resp = server
        .get(&format!("/accounts/{account}/sis_imports/{batch}"))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "importing");
    assert!(body["started_at"].is_string(), "body: {body}");

    // The running import is listed by `/importing` and by the state filter.
    for path in [
        format!("/accounts/{account}/sis_imports/importing"),
        format!("/accounts/{account}/sis_imports?workflow_state%5B%5D=importing"),
    ] {
        let resp = server
            .get(&path)
            .add_header("Authorization", auth(admin, "admin"))
            .await;
        assert_eq!(resp.status_code(), 200, "path: {path}");
        let body: Value = resp.json();
        let ids: Vec<i64> = body["sis_imports"]
            .as_array()
            .expect("sis_imports array")
            .iter()
            .map(|item| item["id"].as_i64().expect("id"))
            .collect();
        assert!(ids.contains(&batch), "path: {path}, body: {body}");
    }

    // Canvas `SisBatch#populate_old_warnings_and_errors` records the failed
    // and warning counts plus the first 24 file/message pairs.
    seed_error(&pool, account, batch, "courses.csv", "bad course", true).await;
    seed_error(&pool, account, batch, "users.csv", "claimable login", false).await;
    sqlx::query(
        "UPDATE sis_batches SET workflow_state = 'imported_with_messages', progress = 100, \
         ended_at = now(), updated_at = now(), \
         data = '{\"import_type\":\"instructure_csv\",\"counts\":{\"error_count\":1,\"warning_count\":1}}', \
         processing_errors = '[\"courses.csv\",\"bad course\"]', \
         processing_warnings = '[\"users.csv\",\"claimable login\"]' \
         WHERE id = $1",
    )
    .bind(batch)
    .execute(&pool)
    .await
    .expect("advance to imported_with_messages");

    let resp = server
        .get(&format!("/accounts/{account}/sis_imports/{batch}"))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "imported_with_messages");
    assert_eq!(body["progress"], 100.0);
    assert_eq!(body["data"]["counts"]["error_count"], 1);
    assert_eq!(body["data"]["counts"]["warning_count"], 1);
    assert_eq!(
        body["processing_errors"],
        json!([["courses.csv", "bad course"]])
    );
    assert_eq!(
        body["processing_warnings"],
        json!([["users.csv", "claimable login"]])
    );
    assert!(body["ended_at"].is_string(), "body: {body}");

    // The terminal import left both running listings.
    for path in [
        format!("/accounts/{account}/sis_imports/importing"),
        format!("/accounts/{account}/sis_imports?workflow_state%5B%5D=importing"),
    ] {
        let resp = server
            .get(&path)
            .add_header("Authorization", auth(admin, "admin"))
            .await;
        let body: Value = resp.json();
        let ids: Vec<i64> = body["sis_imports"]
            .as_array()
            .expect("sis_imports array")
            .iter()
            .map(|item| item["id"].as_i64().expect("id"))
            .collect();
        assert!(!ids.contains(&batch), "path: {path}, body: {body}");
    }
    // ... and is reachable through the terminal-state filter.
    let resp = server
        .get(&format!(
            "/accounts/{account}/sis_imports?workflow_state%5B%5D=imported_with_messages"
        ))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    let body: Value = resp.json();
    let ids: Vec<i64> = body["sis_imports"]
        .as_array()
        .expect("sis_imports array")
        .iter()
        .map(|item| item["id"].as_i64().expect("id"))
        .collect();
    assert_eq!(ids, vec![batch], "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_finished_import_cannot_be_aborted_and_survives_abort_all_pending() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_root_account(&pool, &prefix, "Account").await;
    seed_account_admin(&pool, account, admin).await;

    let failed = seed_batch(&pool, account, admin).await;
    sqlx::query(
        "UPDATE sis_batches SET workflow_state = 'failed_with_messages', progress = 100, \
         ended_at = now() WHERE id = $1",
    )
    .bind(failed)
    .execute(&pool)
    .await
    .expect("seed the failed import");

    // Canvas `abort` reads `sis_batches.not_completed.find` — a terminal
    // batch is a 404.
    let resp = server
        .put(&format!("/accounts/{account}/sis_imports/{failed}/abort"))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let state: String = sqlx::query_scalar("SELECT workflow_state FROM sis_batches WHERE id = $1")
        .bind(failed)
        .fetch_one(&pool)
        .await
        .expect("read the batch");
    assert_eq!(state, "failed_with_messages");

    // `abort_all_pending` answers `{"aborted": true}` and only touches the
    // imports that have not started (`abort_all_pending_for_account`).
    let pending = seed_batch(&pool, account, admin).await;
    let resp = server
        .put(&format!(
            "/accounts/{account}/sis_imports/abort_all_pending"
        ))
        .add_header("Authorization", auth(admin, "admin"))
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
    let failed_state: String =
        sqlx::query_scalar("SELECT workflow_state FROM sis_batches WHERE id = $1")
            .bind(failed)
            .fetch_one(&pool)
            .await
            .expect("read the batch");
    assert_eq!(failed_state, "failed_with_messages");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_error_file_and_tenant_scoping_hold_on_finished_imports() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_root_account(&pool, &prefix, "Account").await;
    let other = seed_root_account(&pool, &prefix, "Other").await;
    seed_account_admin(&pool, account, admin).await;

    let batch = seed_batch(&pool, account, admin).await;
    seed_error(&pool, account, batch, "courses.csv", "bad course", true).await;
    let attachment_id = seed_error_file(&pool, account, admin, batch).await;

    // A finished import of another account is a 404 for both the import
    // itself and its error listing (Canvas scopes every lookup through
    // `@account.sis_batches`).
    let foreign = seed_batch(&pool, other, admin).await;
    seed_error(&pool, other, foreign, "users.csv", "foreign error", true).await;
    for path in [
        format!("/accounts/{account}/sis_imports/{foreign}"),
        format!("/accounts/{account}/sis_imports/{foreign}/errors"),
    ] {
        let resp = server
            .get(&path)
            .add_header("Authorization", auth(admin, "admin"))
            .await;
        assert_eq!(
            resp.status_code(),
            404,
            "path: {path}, body: {}",
            resp.text()
        );
    }

    // The account error listing never leaks the other account's rows.
    let resp = server
        .get(&format!("/accounts/{account}/sis_import_errors"))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let errors = body["sis_import_errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 1, "body: {body}");
    assert_eq!(errors[0]["sis_import_id"], batch);
    assert_eq!(errors[0]["message"], "bad course");

    // The error file shows up on the import JSON (Canvas
    // `write_errors_to_file` names it `sis_errors_attachment_{id}.csv`).
    let resp = server
        .get(&format!("/accounts/{account}/sis_imports/{batch}"))
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let error_file = &body["errors_attachment"];
    assert_eq!(error_file["id"], attachment_id);
    assert_eq!(
        error_file["display_name"],
        format!("sis_errors_attachment_{batch}.csv")
    );
    assert_eq!(error_file["content-type"], "text/csv");
    let url = error_file["url"].as_str().expect("url");
    assert!(
        url.contains(&format!("/files/{attachment_id}/download")),
        "url: {url}"
    );
    assert!(url.contains("verifier="), "url: {url}");

    cleanup(&pool, &prefix).await;
}
