//! Integration tests for the star-lti LTI context controls routes.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Unique per-run prefix + full teardown, so repeat runs against a shared
//!   database are safe.
//!
//! The routes under test are wired into `star_lti::router()` by the
//! coordinator; this file exercises them through that router.

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
        "lti-ctx-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// The DDL the coordinator has to add: `lti_context_controls` (migration
/// `0224`) has both context columns `NOT NULL` and a foreign key on
/// `deployment_id` that points at Canvas' `context_external_tools` instead of
/// the workspace's `lti_deployments`. Migration `0233` applies the same
/// statements; they are repeated here so the test also runs against a
/// database that has not seen it yet.
const REPAIR_DDL: &[&str] = &[
    "ALTER TABLE lti_context_controls \
       ALTER COLUMN account_id DROP NOT NULL, \
       ALTER COLUMN course_id DROP NOT NULL",
    "ALTER TABLE lti_context_controls DROP CONSTRAINT IF EXISTS chk_require_context_4",
    "ALTER TABLE lti_context_controls \
       DROP CONSTRAINT IF EXISTS fk_lti_context_controls_deployment_id",
    "ALTER TABLE lti_context_controls \
       ADD CONSTRAINT fk_lti_context_controls_deployment_id \
       FOREIGN KEY (deployment_id) REFERENCES lti_deployments (id) ON DELETE CASCADE",
    "CREATE UNIQUE INDEX IF NOT EXISTS index_lti_context_controls_on_account_and_deployment \
       ON lti_context_controls (account_id, deployment_id) WHERE account_id IS NOT NULL",
    "CREATE UNIQUE INDEX IF NOT EXISTS index_lti_context_controls_on_course_and_deployment \
       ON lti_context_controls (course_id, deployment_id) WHERE course_id IS NOT NULL",
];

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

    // Serialize the repair pass across the parallel tests of this binary:
    // concurrent DROP CONSTRAINT IF EXISTS + ADD CONSTRAINT sequences race on
    // the catalog and fail with "constraint ... already exists" (42710).
    static REPAIR_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
    {
        let _repair_guard = REPAIR_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for statement in REPAIR_DDL {
            sqlx::query(statement)
                .execute(&pool)
                .await
                .expect("repair lti_context_controls");
        }
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };

    let server = TestServer::new(
        star_lti::router()
            .merge(star_lti::lti_advantage_router())
            .with_state(state),
    )
    .expect("build TestServer");
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

/// An active LTI registration owned by the root account.
async fn seed_registration(pool: &PgPool, prefix: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO lti_registrations (account_id, root_account_id, name, workflow_state) \
         VALUES ($1, $1, $2, 'active') RETURNING id",
    )
    .bind(account_id)
    .bind(format!("{prefix}-registration"))
    .fetch_one(pool)
    .await
    .expect("seed registration")
}

/// A deployment of the registration in an account or course context.
async fn seed_deployment(
    pool: &PgPool,
    registration_id: i64,
    context_type: &str,
    context_id: i64,
    context_name: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO lti_deployments \
           (registration_id, deployment_id, context_type, context_id, context_name, workflow_state) \
         VALUES ($1, $2, $3, $4, $5, 'active') RETURNING id",
    )
    .bind(registration_id)
    .bind(format!("1:deployment-{registration_id}-{context_id}"))
    .bind(context_type)
    .bind(context_id)
    .bind(context_name)
    .fetch_one(pool)
    .await
    .expect("seed deployment")
}

/// Turn a Canvas feature flag on for an account.
async fn enable_feature(pool: &PgPool, account_id: i64, feature: &str) {
    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state) \
         VALUES ('Account', $1, $2, $2, 'enabled') \
         ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'enabled'",
    )
    .bind(account_id)
    .bind(feature)
    .execute(pool)
    .await
    .expect("enable feature flag");
}

async fn disable_feature(pool: &PgPool, account_id: i64, feature: &str) {
    sqlx::query(
        "UPDATE feature_flags SET state = 'disabled' \
         WHERE context_type = 'Account' AND context_id = $1 AND feature = $2",
    )
    .bind(account_id)
    .bind(feature)
    .execute(pool)
    .await
    .expect("disable feature flag");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // The context controls reference the deployments and the registrations.
    let _ = sqlx::query(
        "DELETE FROM lti_context_controls WHERE registration_id IN \
           (SELECT id FROM lti_registrations WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM lti_registration_history_entries WHERE lti_registration_id IN \
           (SELECT id FROM lti_registrations WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM lti_deployments WHERE registration_id IN \
           (SELECT id FROM lti_registrations WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM lti_registrations WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM feature_flags \
         WHERE context_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    // Courses first: courses.account_id has no ON DELETE action.
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

/// The fixture the tests share: a root account, a sub account, a course, a
/// registration, a deployment in the root account and an admin.
struct Fixture {
    admin_id: i64,
    user_id: i64,
    root_id: i64,
    sub_id: i64,
    course_id: i64,
    registration_id: i64,
    deployment_id: i64,
}

async fn seed_fixture(pool: &PgPool, prefix: &str) -> Fixture {
    let admin_id = seed_user(pool, prefix, "admin", "admin").await;
    let user_id = seed_user(pool, prefix, "teacher", "teacher").await;
    let root_id = seed_account(pool, prefix, "root", None).await;
    let sub_id = seed_account(pool, prefix, "sub", Some(root_id)).await;
    let course_id = seed_course(pool, prefix, "course", sub_id).await;
    let registration_id = seed_registration(pool, prefix, root_id).await;
    let deployment_id =
        seed_deployment(pool, registration_id, "Account", root_id, "Root Account").await;
    enable_feature(pool, root_id, "lti_registrations_next").await;

    Fixture {
        admin_id,
        user_id,
        root_id,
        sub_id,
        course_id,
        registration_id,
        deployment_id,
    }
}

fn controls_path(fixture: &Fixture) -> String {
    format!(
        "/accounts/{}/lti_registrations/{}/controls",
        fixture.root_id, fixture.registration_id
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_guard_chain_rejects_anonymous_users_and_non_managers() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;
    let path = controls_path(&fixture);

    // Anonymous callers are rejected before the handler runs.
    let response = server.get(&path).await;
    assert_eq!(response.status_code(), 401, "body: {}", response.text());

    // A signed-in user without the account right is a 403.
    let response = server
        .get(&path)
        .add_header(AUTHORIZATION, auth(fixture.user_id, "teacher"))
        .await;
    assert_eq!(response.status_code(), 403, "body: {}", response.text());

    // A missing account is a 404.
    let response = server
        .get(&format!(
            "/accounts/{}/lti_registrations/{}/controls",
            fixture.root_id + 999_999,
            fixture.registration_id
        ))
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    // A sub account is a 404 — Canvas requires the root account.
    let response = server
        .get(&format!(
            "/accounts/{}/lti_registrations/{}/controls",
            fixture.sub_id, fixture.registration_id
        ))
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    // Without the feature flag the whole resource is hidden.
    disable_feature(&pool, fixture.root_id, "lti_registrations_next").await;
    let response = server
        .get(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["error"], "The specified resource does not exist.");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_control_is_created_shown_updated_and_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;
    let path = controls_path(&fixture);

    // --- create ---
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .json(&json!({
            "account_id": fixture.sub_id,
            "deployment_id": fixture.deployment_id,
            "available": true,
        }))
        .await;
    assert_eq!(response.status_code(), 201, "body: {}", response.text());
    let body: Value = response.json();
    let control_id = body["id"].as_i64().expect("control id");
    assert_eq!(body["account_id"], json!(fixture.sub_id));
    assert_eq!(body["course_id"], Value::Null);
    assert_eq!(body["deployment_id"], json!(fixture.deployment_id));
    assert_eq!(body["registration_id"], json!(fixture.registration_id));
    assert_eq!(body["available"], json!(true));
    assert_eq!(body["workflow_state"], json!("active"));
    assert_eq!(
        body["path"],
        json!(format!("a{}.a{}.", fixture.root_id, fixture.sub_id)),
        "the path is the account chain, root first"
    );
    assert_eq!(body["context_name"], json!(format!("{prefix}-sub")));
    assert_eq!(body["subaccount_count"], json!(0));
    assert_eq!(body["course_count"], json!(1));
    assert_eq!(body["child_control_count"], json!(0));
    assert_eq!(body["depth"], json!(0));
    assert_eq!(body["display_path"], json!([]));
    assert_eq!(body["created_by"]["id"], json!(fixture.admin_id));

    // --- a second control for the same context is rejected ---
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .json(&json!({
            "account_id": fixture.sub_id,
            "deployment_id": fixture.deployment_id,
        }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["errors"],
        json!("A context control for this deployment and context already exists.")
    );

    // --- a control without a context is rejected ---
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .json(&json!({ "deployment_id": fixture.deployment_id }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["errors"],
        json!("Either account_id or course_id must be present.")
    );

    // --- show ---
    let response = server
        .get(&format!("{path}/{control_id}"))
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"], json!(control_id));
    assert_eq!(body["account_id"], json!(fixture.sub_id));

    // --- update ---
    let response = server
        .put(&format!("{path}/{control_id}"))
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .json(&json!({ "available": false }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["available"], json!(false));
    assert_eq!(body["id"], json!(control_id));

    // --- update without `available` is a 400 ---
    let response = server
        .put(&format!("{path}/{control_id}"))
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["message"], "available is missing");

    // --- delete ---
    let response = server
        .delete(&format!("{path}/{control_id}"))
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["workflow_state"], json!("deleted"));

    // --- the deleted control is gone ---
    let response = server
        .get(&format!("{path}/{control_id}"))
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_index_groups_the_controls_by_deployment() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;
    let path = controls_path(&fixture);

    // An empty list is still a JSON array.
    let response = server
        .get(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body, json!([]));

    // One account control and one course control.
    for payload in [
        json!({ "account_id": fixture.sub_id, "deployment_id": fixture.deployment_id }),
        json!({ "course_id": fixture.course_id, "deployment_id": fixture.deployment_id }),
    ] {
        let response = server
            .post(&path)
            .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
            .json(&payload)
            .await;
        assert_eq!(response.status_code(), 201, "body: {}", response.text());
    }

    let response = server
        .get(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let deployments = body.as_array().expect("an array of deployments");
    assert_eq!(deployments.len(), 1, "one deployment holds both controls");
    let deployment = &deployments[0];
    assert_eq!(deployment["id"], json!(fixture.deployment_id));
    assert_eq!(deployment["context_type"], json!("Account"));
    assert_eq!(deployment["context_id"], json!(fixture.root_id));
    assert_eq!(
        deployment["registration_id"],
        json!(fixture.registration_id)
    );
    assert_eq!(deployment["workflow_state"], json!("active"));
    assert_eq!(deployment["root_account_deployment"], json!(true));
    assert_eq!(deployment["context_name"], json!("Root Account"));

    let controls = deployment["context_controls"].as_array().expect("controls");
    assert_eq!(controls.len(), 2);
    // The account control comes before the course control: the path
    // ordering replaces `a` with `1` and `c` with `0`.
    assert_eq!(controls[0]["account_id"], json!(fixture.sub_id));
    assert_eq!(controls[1]["course_id"], json!(fixture.course_id));
    assert_eq!(
        controls[1]["path"],
        json!(format!(
            "a{}.a{}.c{}.",
            fixture.root_id, fixture.sub_id, fixture.course_id
        ))
    );
    assert_eq!(controls[1]["subaccount_count"], json!(0));
    assert_eq!(controls[1]["course_count"], json!(0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_bulk_endpoint_creates_several_controls_at_once() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;
    let path = format!("{}/bulk", controls_path(&fixture));

    // An empty array is rejected.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .json(&json!([]))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["errors"],
        json!("Invalid parameters. Expected an array of context control parameters.")
    );

    // A control without a context is rejected.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .json(&json!([{ "deployment_id": fixture.deployment_id }]))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["errors"],
        json!("Either account_id or course_id must be present for each context control.")
    );

    // Two controls for the same context are rejected.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .json(&json!([
            { "account_id": fixture.sub_id, "deployment_id": fixture.deployment_id },
            { "account_id": fixture.sub_id, "deployment_id": fixture.deployment_id },
        ]))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["errors"],
        json!("Cannot create multiple context controls for the same context. Please specify unique account_id or course_id for each context control.")
    );

    // One account control and one course control are created.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(fixture.admin_id, "admin"))
        .json(&json!([
            { "account_id": fixture.sub_id, "deployment_id": fixture.deployment_id },
            { "course_id": fixture.course_id, "deployment_id": fixture.deployment_id },
        ]))
        .await;
    assert_eq!(response.status_code(), 201, "body: {}", response.text());
    let body: Value = response.json();
    let controls = body.as_array().expect("the created controls");
    assert_eq!(controls.len(), 2);
    // The response is ordered by id.
    let first = controls[0]["id"].as_i64().expect("id");
    let second = controls[1]["id"].as_i64().expect("id");
    assert!(first < second);
    assert_eq!(controls[0]["account_id"], json!(fixture.sub_id));
    assert_eq!(controls[1]["course_id"], json!(fixture.course_id));

    cleanup(&pool, &prefix).await;
}
