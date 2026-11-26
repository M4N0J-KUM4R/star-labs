//! Integration tests for the Canvas account reports routes.
//!
//! Pinned behavior (ported from `canvas-src/spec/apis/v1/account_reports_api_spec.rb`):
//! - `GET /accounts/{account_id}/reports` lists the 20 report types of the
//!   Canvas registry, sorted by title, with their parameters and the last run
//!   of every type;
//! - `include[]=description_html` and `include[]=parameters_html` add the
//!   rendered partials;
//! - a missing account is a 404, a caller without the `read_reports` right is a
//!   403 and an anonymous caller is a 401 — in that order;
//! - `GET /accounts/{account_id}/reports/{report}` lists the reports of a type,
//!   newest first, and hides the deleted ones;
//! - `GET /accounts/{account_id}/reports/{report}/{id}` returns a report and
//!   404s for an unknown id, an unknown type or a deleted report;
//! - `POST /accounts/{account_id}/reports/{report}` creates a report in the
//!   `created` state, 404s for an unknown type, 400s for an unknown
//!   `enrollment_term_id` and 409s (with a `Location` header) when a report of
//!   the same type and parameters is still running;
//! - `DELETE .../{id}` marks the report as `deleted`;
//! - `PUT .../{id}/abort` marks a created or running report as `aborted` and
//!   404s for a report that is neither;
//! - the catalog carries the last run of every type as `last_run`;
//! - a report advances `created → running → complete` through the `jobs` row
//!   plus SQL state flips, and a complete report carries the generated CSV as
//!   an `attachment` (`file_url`, the attachment JSON, `run_time`,
//!   `current_line`);
//! - show / destroy / abort find the report scoped by account *and* type, so
//!   another account's report id and an unknown type are 404s (`index` has no
//!   registry lookup: an unknown type is simply an empty list);
//! - anonymous callers get 401 on every route of the controller.
//!
//! Fleet rule: dedicated users, accounts and reports per test, every seeded
//! row is cleaned up. Skipped cleanly when DATABASE_URL is unset.
//!
//! The `account_reports`, `attachments` and `enrollment_terms` tables are
//! created by this test with `CREATE TABLE IF NOT EXISTS` (the same DDL the
//! migrations use) so the suite runs before the migrations land.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// The DDL of the Canvas schema tables the account reports routes read.
const ACCOUNT_REPORTS_DDL: [&str; 3] = [
    r#"
    CREATE TABLE IF NOT EXISTS account_reports (
        id              BIGSERIAL PRIMARY KEY,
        user_id         BIGINT NOT NULL,
        message         TEXT,
        account_id      BIGINT NOT NULL,
        attachment_id   BIGINT,
        workflow_state  VARCHAR(255) NOT NULL DEFAULT 'created',
        report_type     VARCHAR(255),
        progress        INTEGER,
        created_at      TIMESTAMP NOT NULL,
        updated_at      TIMESTAMP NOT NULL,
        parameters      TEXT,
        current_line    INTEGER,
        total_lines     INTEGER,
        start_at        TIMESTAMP,
        end_at          TIMESTAMP,
        job_ids         BIGINT[] NOT NULL DEFAULT '{}'
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS attachments (
        id                       BIGSERIAL PRIMARY KEY,
        context_id               BIGINT,
        context_type             VARCHAR(255),
        size                     BIGINT,
        folder_id                BIGINT,
        content_type             VARCHAR(255),
        filename                 TEXT,
        uuid                     VARCHAR(255),
        display_name             TEXT,
        created_at               TIMESTAMP,
        updated_at               TIMESTAMP,
        workflow_state           VARCHAR(255),
        user_id                  BIGINT,
        locked                   BOOLEAN DEFAULT false,
        file_state               VARCHAR(255),
        deleted_at               TIMESTAMP,
        position                 INTEGER,
        lock_at                  TIMESTAMP,
        unlock_at                TIMESTAMP,
        could_be_locked          BOOLEAN,
        root_attachment_id       BIGINT,
        cloned_item_id           BIGINT,
        migration_id             VARCHAR(255),
        namespace                VARCHAR(255),
        media_entry_id           VARCHAR(255),
        md5                      VARCHAR(255),
        encoding                 VARCHAR(255),
        need_notify              BOOLEAN,
        upload_error_message     TEXT,
        replacement_attachment_id BIGINT,
        usage_rights_id          BIGINT,
        modified_at              TIMESTAMP,
        viewed_at                TIMESTAMP,
        instfs_uuid              VARCHAR,
        root_account_id          BIGINT,
        category                 VARCHAR NOT NULL DEFAULT 'uncategorized',
        word_count               INTEGER,
        visibility_level         VARCHAR(32) NOT NULL DEFAULT 'inherit',
        only_visible_to_overrides BOOLEAN NOT NULL DEFAULT false
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS enrollment_terms (
        id                      BIGSERIAL PRIMARY KEY,
        root_account_id         BIGINT NOT NULL,
        name                    VARCHAR(255),
        term_code               VARCHAR(255),
        sis_source_id           VARCHAR(255),
        sis_batch_id            BIGINT,
        start_at                TIMESTAMP,
        end_at                  TIMESTAMP,
        accepting_enrollments   BOOLEAN,
        can_manually_enroll     BOOLEAN,
        created_at              TIMESTAMP NOT NULL,
        updated_at              TIMESTAMP NOT NULL,
        workflow_state          VARCHAR(255) NOT NULL DEFAULT 'active',
        stuck_sis_fields        TEXT,
        integration_id          VARCHAR(255),
        grading_period_group_id BIGINT
    )
    "#,
];

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "ar-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
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

    for statement in ACCOUNT_REPORTS_DDL {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("create the account report tables");
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(star_account_reports::router().with_state(state))
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_inactive_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, workflow_state) VALUES ($1, 'deleted') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed deleted account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role, workflow_state) \
         VALUES ($1, $2, 'admin', 'active')",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account admin");
}

async fn seed_enrollment_term(pool: &PgPool, root_account_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollment_terms (root_account_id, name, sis_source_id, workflow_state, \
             created_at, updated_at) \
         VALUES ($1, $2, $3, 'active', now(), now()) RETURNING id",
    )
    .bind(root_account_id)
    .bind(name)
    .bind(name.to_lowercase().replace(' ', "_"))
    .fetch_one(pool)
    .await
    .expect("seed enrollment term")
}

/// One `account_reports` row.
#[allow(clippy::too_many_arguments)]
async fn seed_report(
    pool: &PgPool,
    account_id: i64,
    user_id: i64,
    report_type: &str,
    workflow_state: &str,
    created_at: &str,
    parameters: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO account_reports (user_id, message, account_id, attachment_id, \
             workflow_state, report_type, progress, created_at, updated_at, parameters) \
         VALUES ($1, NULL, $2, NULL, $3, $4, 0, $5::timestamp, $5::timestamp, $6) RETURNING id",
    )
    .bind(user_id)
    .bind(account_id)
    .bind(workflow_state)
    .bind(report_type)
    .bind(created_at)
    .bind(parameters)
    .fetch_one(pool)
    .await
    .expect("seed account report")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM jobs WHERE context_type = 'AccountReport' AND context_id IN \
           (SELECT id FROM account_reports WHERE account_id IN \
             (SELECT id FROM accounts WHERE name LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_reports WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM attachments WHERE uuid LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM enrollment_terms WHERE root_account_id IN \
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
async fn lists_the_available_reports_of_an_account() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;

    let resp = server
        .get(&format!("/accounts/{account}/reports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let reports = body.as_array().expect("array");
    assert_eq!(reports.len(), 20, "body: {body}");

    // The registry is sorted by title.
    let titles: Vec<&str> = reports
        .iter()
        .map(|report| report["title"].as_str().expect("title"))
        .collect();
    let mut sorted = titles.clone();
    sorted.sort_unstable();
    assert_eq!(titles, sorted, "the reports are sorted by title");

    // The grade export is known to Canvas.
    let grade_export = reports
        .iter()
        .find(|report| report["report"] == json!("grade_export_csv"))
        .expect("grade export");
    assert_eq!(grade_export["title"], json!("Grade Export"));
    assert_eq!(
        grade_export["parameters"]["enrollment_term_id"]["required"],
        json!(false)
    );
    assert_eq!(
        grade_export["parameters"]["enrollment_term_id"]["description"],
        json!("The canvas id of the term to get grades from")
    );
    assert_eq!(grade_export["last_run"], Value::Null);

    // A report without parameters keeps a null `parameters`.
    let outcome_export = reports
        .iter()
        .find(|report| report["report"] == json!("outcome_export_csv"))
        .expect("outcome export");
    assert_eq!(outcome_export["parameters"], Value::Null);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn renders_the_description_and_parameters_partials() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_enrollment_term(&pool, account, "Fall Term").await;

    let resp = server
        .get(&format!(
            "/accounts/{account}/reports?include[]=description_html&include[]=parameters_html"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let reports = body.as_array().expect("array");

    let grade_export = reports
        .iter()
        .find(|report| report["report"] == json!("grade_export_csv"))
        .expect("grade export");
    let description = grade_export["description_html"]
        .as_str()
        .expect("description");
    assert!(description.contains("<p>"), "description: {description}");
    let parameters = grade_export["parameters_html"]
        .as_str()
        .expect("parameters");
    assert!(parameters.contains("<form"), "parameters: {parameters}");
    assert!(
        parameters.contains("grade_export_csv_form"),
        "parameters: {parameters}"
    );
    // The enrollment term selector is rendered from the root account terms.
    assert!(parameters.contains("Fall Term"), "parameters: {parameters}");

    // The outcome export has no parameters partial in Canvas.
    let outcome_export = reports
        .iter()
        .find(|report| report["report"] == json!("outcome_export_csv"))
        .expect("outcome export");
    assert!(outcome_export.get("parameters_html").is_none());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn refuses_callers_without_the_read_reports_right() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let deleted_account = seed_inactive_account(&pool, &prefix, "Deleted").await;

    // An anonymous caller is a 401.
    let resp = server.get(&format!("/accounts/{account}/reports")).await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // A caller without the right is a 403.
    let resp = server
        .get(&format!("/accounts/{account}/reports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An account administrator of the account may read the reports.
    seed_account_admin(&pool, account, member).await;
    let resp = server
        .get(&format!("/accounts/{account}/reports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // A missing account is a 404.
    let resp = server
        .get("/accounts/987654321/reports")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"][0]["message"],
        json!("The specified resource does not exist.")
    );

    // A deleted account is a 404 as well.
    let resp = server
        .get(&format!("/accounts/{deleted_account}/reports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn lists_the_reports_of_a_type_newest_first() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let older = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "complete",
        "2020-01-13 10:00:00",
        Some(r#"{"enrollment_term_id":1}"#),
    )
    .await;
    let newer = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "complete",
        "2020-01-15 10:00:00",
        Some(r#"{"enrollment_term_id":1}"#),
    )
    .await;
    seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "deleted",
        "2020-01-16 10:00:00",
        None,
    )
    .await;
    seed_report(
        &pool,
        account,
        admin,
        "sis_export_csv",
        "complete",
        "2020-01-16 10:00:00",
        None,
    )
    .await;

    let resp = server
        .get(&format!("/accounts/{account}/reports/grade_export_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let reports = body.as_array().expect("array");
    assert_eq!(reports.len(), 2, "body: {body}");
    assert_eq!(reports[0]["id"], newer);
    assert_eq!(reports[1]["id"], older);
    assert_eq!(reports[0]["status"], json!("complete"));
    assert_eq!(reports[0]["report"], json!("grade_export_csv"));
    assert_eq!(reports[0]["parameters"]["enrollment_term_id"], json!(1));
    assert_eq!(reports[0]["progress"], json!(0));
    assert_eq!(reports[0]["current_line"], Value::Null);
    assert_eq!(reports[0]["message"], Value::Null);
    assert_eq!(reports[0]["file_url"], Value::Null);
    assert_eq!(reports[0]["user"]["id"], admin);
    assert_eq!(reports[0]["user"]["display_name"], json!("Admin"));
    assert_eq!(reports[0]["started_at"], Value::Null);
    assert_eq!(reports[0]["ended_at"], Value::Null);
    assert!(reports[0]["created_at"].as_str().is_some());
    assert!(reports[0]["run_time"].as_f64().is_some());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn shows_a_report_and_404s_for_the_unknown_ones() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let report = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "complete",
        "2020-01-15 10:00:00",
        None,
    )
    .await;
    let deleted = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "deleted",
        "2020-01-16 10:00:00",
        None,
    )
    .await;

    let resp = server
        .get(&format!(
            "/accounts/{account}/reports/grade_export_csv/{report}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], report);
    assert_eq!(body["status"], json!("complete"));

    // An unknown id is a 404.
    let resp = server
        .get(&format!(
            "/accounts/{account}/reports/grade_export_csv/987654321"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A report of another type is a 404 as well.
    let resp = server
        .get(&format!(
            "/accounts/{account}/reports/sis_export_csv/{report}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A deleted report is a 404.
    let resp = server
        .get(&format!(
            "/accounts/{account}/reports/grade_export_csv/{deleted}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn creates_a_report_and_queues_the_job() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let term = seed_enrollment_term(&pool, account, "Fall Term").await;

    let resp = server
        .post(&format!("/accounts/{account}/reports/grade_export_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "parameters": { "enrollment_term_id": term } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], json!("created"));
    assert_eq!(body["report"], json!("grade_export_csv"));
    assert_eq!(body["progress"], json!(0));
    assert_eq!(body["parameters"]["enrollment_term_id"], json!(term));
    assert_eq!(body["user"]["id"], admin);
    assert_eq!(body["file_url"], Value::Null);
    assert!(body["id"].as_i64().is_some());

    // The report generation is queued.
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE tag = 'AccountReport#generate' \
           AND context_type = 'AccountReport' AND context_id = $1",
    )
    .bind(body["id"].as_i64().expect("id"))
    .fetch_one(&pool)
    .await
    .expect("count the queued jobs");
    assert_eq!(queued, 1);

    // A form encoded body works as well.
    let resp = server
        .post(&format!("/accounts/{account}/reports/provisioning_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .content_type("application/x-www-form-urlencoded")
        .text("parameters[users]=true&parameters[courses]=true")
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], json!("created"));
    assert_eq!(body["parameters"]["users"], json!("true"));
    assert_eq!(body["parameters"]["courses"], json!("true"));

    // An unknown report type is a 404.
    let resp = server
        .post(&format!("/accounts/{account}/reports/no_such_report"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "parameters": {} }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // An unknown enrollment term is a 400.
    let resp = server
        .post(&format!("/accounts/{account}/reports/grade_export_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "parameters": { "enrollment_term_id": 987654321 } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["error"],
        json!("invalid enrollment_term_id '987654321'")
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn refuses_a_second_running_report_of_the_same_type() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let term = seed_enrollment_term(&pool, account, "Fall Term").await;

    let resp = server
        .post(&format!("/accounts/{account}/reports/grade_export_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "parameters": { "enrollment_term_id": term } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();

    // The same type with the same parameters is a conflict.
    let resp = server
        .post(&format!("/accounts/{account}/reports/grade_export_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "parameters": { "enrollment_term_id": term } }))
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());
    let location = resp
        .headers()
        .get(axum::http::header::LOCATION)
        .expect("location header")
        .to_str()
        .expect("location header text");
    assert!(
        location.contains(&format!(
            "/api/v1/accounts/{account}/reports/grade_export_csv/{}",
            created["id"].as_i64().expect("id")
        )),
        "location: {location}"
    );
    let body: Value = resp.json();
    assert_eq!(body["id"], created["id"]);

    // Another type is not a conflict.
    let resp = server
        .post(&format!("/accounts/{account}/reports/sis_export_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "parameters": { "enrollment_term_id": term } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn deletes_a_report() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let report = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "complete",
        "2020-01-15 10:00:00",
        None,
    )
    .await;

    let resp = server
        .delete(&format!(
            "/accounts/{account}/reports/grade_export_csv/{report}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], report);
    assert_eq!(body["status"], json!("deleted"));

    // The report is gone from the index and from the show route.
    let resp = server
        .get(&format!(
            "/accounts/{account}/reports/grade_export_csv/{report}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .get(&format!("/accounts/{account}/reports/grade_export_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body.as_array().expect("array").is_empty());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn aborts_a_running_report() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let running = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "running",
        "2020-01-15 10:00:00",
        None,
    )
    .await;
    let complete = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "complete",
        "2020-01-15 10:00:00",
        None,
    )
    .await;

    let resp = server
        .put(&format!(
            "/accounts/{account}/reports/grade_export_csv/{running}/abort"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], running);
    assert_eq!(body["status"], json!("aborted"));

    // A report that is neither created nor running cannot be aborted.
    let resp = server
        .put(&format!(
            "/accounts/{account}/reports/grade_export_csv/{complete}/abort"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn catalog_populates_the_last_run_after_a_run() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let report = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "complete",
        "2020-01-15 10:00:00",
        Some(r#"{"enrollment_term_id":1}"#),
    )
    .await;

    let resp = server
        .get(&format!("/accounts/{account}/reports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let reports = body.as_array().expect("array");

    // The type that ran carries its most recent report as `last_run`.
    let grade_export = reports
        .iter()
        .find(|entry| entry["report"] == json!("grade_export_csv"))
        .expect("grade export");
    assert_eq!(grade_export["last_run"]["id"], json!(report));
    assert_eq!(grade_export["last_run"]["status"], json!("complete"));
    assert_eq!(
        grade_export["last_run"]["report"],
        json!("grade_export_csv")
    );
    assert_eq!(
        grade_export["last_run"]["parameters"]["enrollment_term_id"],
        json!(1)
    );

    // A type that never ran keeps a null `last_run`.
    let sis_export = reports
        .iter()
        .find(|entry| entry["report"] == json!("sis_export_csv"))
        .expect("sis export");
    assert_eq!(sis_export["last_run"], Value::Null);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn advances_a_report_through_the_lifecycle_with_an_attachment() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;

    // `create` starts the report in the `created` state without an
    // attachment and with the generation queued as a job row.
    let resp = server
        .post(&format!("/accounts/{account}/reports/grade_export_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "parameters": { "enrollment_term_id": 1 } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let report = body["id"].as_i64().expect("report id");
    assert_eq!(body["status"], json!("created"));
    assert_eq!(body["file_url"], Value::Null);
    assert!(body.get("attachment").is_none(), "no attachment yet");

    // The worker advances the row in SQL (star-lms has no worker process);
    // `show` reflects the running state.
    sqlx::query(
        "UPDATE account_reports SET workflow_state = 'running', progress = 50, \
             current_line = 2000, start_at = now() WHERE id = $1",
    )
    .bind(report)
    .execute(&pool)
    .await
    .expect("advance the report to running");
    let resp = server
        .get(&format!(
            "/accounts/{account}/reports/grade_export_csv/{report}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], json!("running"));
    assert_eq!(body["progress"], json!(50));
    assert_eq!(body["current_line"], json!(2000));
    assert!(body["started_at"].as_str().is_some(), "started_at is set");
    assert_eq!(body["file_url"], Value::Null);
    assert!(body["run_time"].as_f64().unwrap_or(-1.0) >= 0.0);

    // Completing attaches the generated CSV the way the generator would:
    // an `attachments` row referenced by the report.
    let display_name = format!("{prefix}-grade_export_csv.csv");
    let attachment: i64 = sqlx::query_scalar(
        "INSERT INTO attachments (display_name, filename, uuid, content_type, size, \
             workflow_state, created_at, updated_at) \
         VALUES ($1, $1, $2, 'text/csv', 42, 'processed', now(), now()) RETURNING id",
    )
    .bind(display_name.as_str())
    .bind(format!("{prefix}-uuid"))
    .fetch_one(&pool)
    .await
    .expect("seed the report attachment");
    sqlx::query(
        "UPDATE account_reports SET workflow_state = 'complete', progress = 100, \
             current_line = 4200, end_at = now(), attachment_id = $2 WHERE id = $1",
    )
    .bind(report)
    .bind(attachment)
    .execute(&pool)
    .await
    .expect("complete the report");

    let resp = server
        .get(&format!(
            "/accounts/{account}/reports/grade_export_csv/{report}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], json!("complete"));
    assert_eq!(body["progress"], json!(100));
    assert_eq!(body["current_line"], json!(4200));
    assert!(body["ended_at"].as_str().is_some(), "ended_at is set");
    assert_eq!(
        body["file_url"],
        json!(format!("/accounts/{account}/files/{attachment}/download"))
    );
    assert_eq!(body["attachment"]["id"], json!(attachment));
    assert_eq!(body["attachment"]["display_name"], json!(display_name));
    assert_eq!(body["attachment"]["content-type"], json!("text/csv"));
    assert_eq!(body["attachment"]["mime_class"], json!("file"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn aborts_a_created_report_and_404s_the_scoped_misses() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let created = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "created",
        "2020-01-15 10:00:00",
        None,
    )
    .await;
    let complete = seed_report(
        &pool,
        account,
        admin,
        "grade_export_csv",
        "complete",
        "2020-01-15 10:00:00",
        None,
    )
    .await;

    // A `created` report aborts like a running one.
    let resp = server
        .put(&format!(
            "/accounts/{account}/reports/grade_export_csv/{created}/abort"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], json!(created));
    assert_eq!(body["status"], json!("aborted"));

    // A complete report cannot be aborted (`created_or_running` scope).
    let resp = server
        .put(&format!(
            "/accounts/{account}/reports/grade_export_csv/{complete}/abort"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // An unknown id or an unknown type is a scoped miss: 404 on show,
    // destroy and abort.
    let resp = server
        .put(&format!(
            "/accounts/{account}/reports/grade_export_csv/987654321/abort"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .put(&format!(
            "/accounts/{account}/reports/no_such_report/{created}/abort"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .delete(&format!(
            "/accounts/{account}/reports/no_such_report/{created}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get(&format!(
            "/accounts/{account}/reports/no_such_report/{created}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // `index` has no registry lookup: an unknown type is an empty list
    // (Canvas `type_scope` is a plain `where`, not a catalog check).
    let resp = server
        .get(&format!("/accounts/{account}/reports/no_such_report"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body.as_array().expect("array").is_empty(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn anonymous_callers_get_401_on_every_account_route() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // The workspace sentinel id: an anonymous caller is rejected by the
    // auth layer before any record lookup, so every route of the
    // controller answers 401 — never 404, never 200.
    let account = 999999999;
    let resp = server.get(&format!("/accounts/{account}/reports")).await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .get(&format!("/accounts/{account}/reports/grade_export_csv"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .get(&format!(
            "/accounts/{account}/reports/grade_export_csv/999999999"
        ))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .post(&format!("/accounts/{account}/reports/grade_export_csv"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .delete(&format!(
            "/accounts/{account}/reports/grade_export_csv/999999999"
        ))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .put(&format!(
            "/accounts/{account}/reports/grade_export_csv/999999999/abort"
        ))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn cross_account_report_ids_are_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    // Two account administrators (plain users with `account_users` rows —
    // not site admins) so the account hierarchy of `read_reports` is
    // actually exercised.
    let admin_a = seed_user(&pool, &prefix, "AdminA", "student").await;
    let admin_b = seed_user(&pool, &prefix, "AdminB", "student").await;
    let account_a = seed_account(&pool, &prefix, "AccountA").await;
    let account_b = seed_account(&pool, &prefix, "AccountB").await;
    seed_account_admin(&pool, account_a, admin_a).await;
    seed_account_admin(&pool, account_b, admin_b).await;
    let foreign = seed_report(
        &pool,
        account_b,
        admin_b,
        "grade_export_csv",
        "complete",
        "2020-01-15 10:00:00",
        None,
    )
    .await;

    // The index of account A does not list account B's report.
    let resp = server
        .get(&format!("/accounts/{account_a}/reports/grade_export_csv"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_a, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body.as_array().expect("array").is_empty(), "body: {body}");

    // And the scoped finds of show, destroy and abort answer 404 for
    // another account's report id.
    let resp = server
        .get(&format!(
            "/accounts/{account_a}/reports/grade_export_csv/{foreign}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_a, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .delete(&format!(
            "/accounts/{account_a}/reports/grade_export_csv/{foreign}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_a, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .put(&format!(
            "/accounts/{account_a}/reports/grade_export_csv/{foreign}/abort"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_a, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
