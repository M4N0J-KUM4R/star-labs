//! Integration tests for the issue #69 deep LTI registration routes:
//! update/reset/overlay history, bind/unbind, update requests, deployment
//! show/destroy/context_search, client id / unified tool id / domain /
//! install status lookups, configuration validation, context controls and
//! the lti_apps listings.
//!
//! Conventions mirror `lti_registrations.rs`: skipped cleanly without
//! DATABASE_URL, unique per-run prefix, full teardown.

use axum::http::header::AUTHORIZATION;
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
        "lti69-{}-{}",
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

    let server = TestServer::new(
        star_lti::router()
            .merge(star_lti::lti_advantage_router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) \
         RETURNING id",
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
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
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

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
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

async fn create_registration(
    server: &TestServer,
    account_id: i64,
    admin_id: i64,
    prefix: &str,
    configuration: Value,
) -> Value {
    let response = server
        .post(&format!("/accounts/{account_id}/lti_registrations"))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({
            "name": format!("{prefix}-tool"),
            "vendor": "Example Vendor",
            "configuration": configuration,
        }))
        .await;
    assert_eq!(
        response.status_code(),
        201,
        "create registration; body: {}",
        response.text()
    );
    response.json()
}

fn sample_configuration() -> Value {
    json!({
        "title": "Configured Tool",
        "description": "A configured tool",
        "target_link_uri": "https://example.com/launch",
        "oidc_initiation_url": "https://example.com/oidc",
        "client_id": "client-abc",
        "domain": "example.com",
        "launch_settings": { "icon_url": "https://example.com/icon.png" }
    })
}

// ---------------------------------------------------------------------------
// Update, reset, overlay history, history
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_reset_and_histories() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct", None).await;
    let admin_id = seed_user(&pool, &prefix, "admin", "admin").await;
    seed_account_admin(&pool, account_id, admin_id).await;

    let registration = create_registration(
        &server,
        account_id,
        admin_id,
        &prefix,
        sample_configuration(),
    )
    .await;
    let id = registration["id"].as_i64().unwrap();

    // PUT update: permitted fields move, the response carries the version
    // includes (registrations_controller.rb:1637-1682).
    let response = server
        .put(&format!("/accounts/{account_id}/lti_registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({
            "name": "Renamed Tool",
            "admin_nickname": "Renicknamed",
            "overlay": { "privacy_level": "public" },
            "comment": "rename"
        }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let updated: Value = response.json();
    assert_eq!(updated["name"], "Renamed Tool");
    assert_eq!(updated["admin_nickname"], "Renicknamed");
    assert_eq!(updated["overlay"]["privacy_level"], "public");
    assert!(!updated["overlay_versions"].as_array().unwrap().is_empty());
    assert_eq!(updated["overlaid_configuration"]["privacy_level"], "public");

    // Invalid workflow_state and dynamic-registration configuration updates
    // answer Canvas' render_error shape.
    let response = server
        .put(&format!("/accounts/{account_id}/lti_registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "workflow_state": "bogus" }))
        .await;
    assert_eq!(response.status_code(), 422);
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["code"], "invalid_workflow_state");

    // overlay_history lists the recorded version.
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/overlay_history"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let versions: Vec<Value> = response.json();
    assert!(!versions.is_empty());
    assert_eq!(versions[0]["overlay"]["privacy_level"], "public");
    assert_eq!(versions[0]["caused_by_reset"], false);

    // reset clears the overlay and flags the version (:1684-1706).
    let response = server
        .put(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/reset"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let reset: Value = response.json();
    assert_eq!(reset["overlay"], json!({}));

    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/overlay_history"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    let versions: Vec<Value> = response.json();
    assert_eq!(versions[0]["caused_by_reset"], true);
    assert_eq!(versions[0]["overlay"], json!({}));

    // history carries the update and overlay_reset entries, newest first.
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/history"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let entries: Vec<Value> = response.json();
    assert!(entries.len() >= 2);
    assert_eq!(entries[0]["update_type"], "overlay_reset");
    assert_eq!(entries[1]["update_type"], "update");
    assert_eq!(entries[1]["comment"], "rename");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Bind / unbind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bind_and_unbind_registration() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let owner_account = seed_account(&pool, &prefix, "owner", None).await;
    let other_account = seed_account(&pool, &prefix, "other", None).await;
    let admin_id = seed_user(&pool, &prefix, "admin", "admin").await;
    seed_account_admin(&pool, owner_account, admin_id).await;
    seed_account_admin(&pool, other_account, admin_id).await;

    // A registration owned by the first account, bound into the second.
    let registration = create_registration(
        &server,
        owner_account,
        admin_id,
        &prefix,
        sample_configuration(),
    )
    .await;
    let id = registration["id"].as_i64().unwrap();

    // bind requires on/off (:1747-1780).
    let response = server
        .post(&format!(
            "/accounts/{other_account}/lti_registrations/{id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "workflow_state": "maybe" }))
        .await;
    assert_eq!(response.status_code(), 422);
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["code"], "invalid_workflow_state");

    let response = server
        .post(&format!(
            "/accounts/{other_account}/lti_registrations/{id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "workflow_state": "on" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let binding: Value = response.json();
    assert_eq!(binding["workflow_state"], "on");
    assert_eq!(binding["account_id"], other_account);

    // re-bind flips the state.
    let response = server
        .post(&format!(
            "/accounts/{other_account}/lti_registrations/{id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "workflow_state": "off" }))
        .await;
    assert_eq!(response.status_code(), 200);
    let binding: Value = response.json();
    assert_eq!(binding["workflow_state"], "off");

    // unbind of a locally-owned registration is refused (:1786-1796).
    let response = server
        .delete(&format!(
            "/accounts/{owner_account}/lti_registrations/{id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 422);
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["code"], "invalid_registration");

    // unbind of the inherited binding returns and deletes it.
    let response = server
        .delete(&format!(
            "/accounts/{other_account}/lti_registrations/{id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let deleted: Value = response.json();
    assert_eq!(deleted["workflow_state"], "off");

    // A second unbind finds no binding -> 404 render_error (:1801).
    let response = server
        .delete(&format!(
            "/accounts/{other_account}/lti_registrations/{id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 404);
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["code"], "not_found");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Update requests: show, latest, apply
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_request_show_latest_apply() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct", None).await;
    let admin_id = seed_user(&pool, &prefix, "admin", "admin").await;
    seed_account_admin(&pool, account_id, admin_id).await;

    let registration = create_registration(
        &server,
        account_id,
        admin_id,
        &prefix,
        sample_configuration(),
    )
    .await;
    let id = registration["id"].as_i64().unwrap();

    // Seed two pending update requests; the second is newer.
    let (rur_old,): (i64,) = sqlx::query_as(
        "INSERT INTO lti_registration_update_requests \
         (uuid, lti_registration_id, root_account_id, lti_ims_registration, created_by_id, created_at) \
         VALUES ($1, $2, $3, $4, $5, now() - interval '2 hours') RETURNING id",
    )
    .bind(format!("{prefix}-rur-old"))
    .bind(id)
    .bind(account_id)
    .bind(json!({
        "client_name": "Updated Tool",
        "lti_tool_configuration": {
            "title": "Updated Tool",
            "target_link_uri": "https://v2.example.com/launch"
        }
    }))
    .bind(admin_id)
    .fetch_one(&pool)
    .await
    .expect("seed old rur");
    let (rur_new,): (i64,) = sqlx::query_as(
        "INSERT INTO lti_registration_update_requests \
         (uuid, lti_registration_id, root_account_id, lti_ims_registration, created_by_id, created_at) \
         VALUES ($1, $2, $3, $4, $5, now() - interval '1 hour') RETURNING id",
    )
    .bind(format!("{prefix}-rur-new"))
    .bind(id)
    .bind(account_id)
    .bind(json!({
        "client_name": "Newest Tool",
        "lti_tool_configuration": {
            "title": "Newest Tool",
            "target_link_uri": "https://v3.example.com/launch"
        }
    }))
    .bind(admin_id)
    .fetch_one(&pool)
    .await
    .expect("seed new rur");

    // latest_update_request returns the newest (:2058-2094).
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/latest_update_request"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let latest: Value = response.json();
    assert_eq!(latest["id"], rur_new);
    assert_eq!(latest["status"], "pending");

    // show one request (:2028-2056); an unknown one 404s.
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/update_requests/{rur_old}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let shown: Value = response.json();
    assert_eq!(shown["uuid"], format!("{prefix}-rur-old"));
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/update_requests/987654"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 404);

    // Applying the older request is refused — a newer one exists
    // (:2125-2130).
    let response = server
        .put(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/update_requests/{rur_old}/apply"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "accepted": true }))
        .await;
    assert_eq!(response.status_code(), 400);

    // accepted is required (:2121-2124).
    let response = server
        .put(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/update_requests/{rur_new}/apply"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 400);
    let body: Value = response.json();
    assert_eq!(body["errors"], "accepted parameter is required");

    // Accepting the newest request replaces the configuration and the name
    // (:2131-2141).
    let response = server
        .put(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/update_requests/{rur_new}/apply"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "accepted": true, "comment": "ship it" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let applied: Value = response.json();
    assert_eq!(applied["name"], "Newest Tool");
    assert_eq!(
        applied["configuration"]["target_link_uri"],
        "https://v3.example.com/launch"
    );

    // The applied request is no longer active -> 404 on re-apply.
    let response = server
        .put(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/update_requests/{rur_new}/apply"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "accepted": false }))
        .await;
    assert_eq!(response.status_code(), 404);

    // The old one is now the most recent pending; reject it.
    let response = server
        .put(&format!(
            "/accounts/{account_id}/lti_registrations/{id}/update_requests/{rur_old}/apply"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "accepted": false }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let rejected: Value = response.json();
    assert_eq!(rejected["name"], "Newest Tool");

    let (status,): (String,) = sqlx::query_as(
        "SELECT CASE WHEN rejected_at IS NOT NULL THEN 'rejected' ELSE 'other' END \
         FROM lti_registration_update_requests WHERE id = $1",
    )
    .bind(rur_old)
    .fetch_one(&pool)
    .await
    .expect("rur status");
    assert_eq!(status, "rejected");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Deployments: show, delete, context_search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deployments_show_delete_context_search() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let root_account = seed_account(&pool, &prefix, "root", None).await;
    let sub_account = seed_account(&pool, &prefix, "sub", Some(root_account)).await;
    let admin_id = seed_user(&pool, &prefix, "admin", "admin").await;
    seed_account_admin(&pool, root_account, admin_id).await;
    let course_id = seed_course(&pool, &prefix, "course", sub_account).await;

    let registration = create_registration(
        &server,
        root_account,
        admin_id,
        &prefix,
        sample_configuration(),
    )
    .await;
    let id = registration["id"].as_i64().unwrap();

    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let deployment: Value = response.json();
    let deployment_id = deployment["id"].as_i64().unwrap();

    // Course deployment: context_search answers empty lists (:1893-1895).
    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "for_course_id": course_id }))
        .await;
    let course_deployment: Value = response.json();
    let course_deployment_id = course_deployment["id"].as_i64().unwrap();
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/{course_deployment_id}/context_search"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let body: Value = response.json();
    assert_eq!(body["accounts"], json!([]));
    assert_eq!(body["courses"], json!([]));

    // show (:103-110) and 404 for a foreign deployment.
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/{deployment_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let shown: Value = response.json();
    assert_eq!(shown["id"], deployment_id);
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/987654"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 404);

    // context_search walks the deployment account subtree
    // (:1864-1966).
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/{deployment_id}/context_search?search_term=course"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let courses = body["courses"].as_array().unwrap();
    assert_eq!(courses.len(), 1);
    assert_eq!(courses[0]["course_code"], format!("{prefix}-course"));
    // The course's display path names its account chain, root excluded —
    // here just the sub account.
    let path = courses[0]["display_path"].as_array().unwrap();
    assert_eq!(path, &vec![json!(format!("{prefix}-sub"))]);
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/{deployment_id}/context_search?only_children_of={sub_account}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let body: Value = response.json();
    assert!(!body["courses"].as_array().unwrap().is_empty());

    // only_children_of outside the subtree is refused.
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/{deployment_id}/context_search?only_children_of=987654"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 422);

    // delete soft-deletes and renders the JSON (:158-175).
    let response = server
        .delete(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/{deployment_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let deleted: Value = response.json();
    assert_eq!(deleted["workflow_state"], "deleted");

    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/{deployment_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// client_id / by_utid / domain duplicates / install_status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn client_id_utid_domain_and_install_status() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct", None).await;
    let other_account = seed_account(&pool, &prefix, "other", None).await;
    let admin_id = seed_user(&pool, &prefix, "admin", "admin").await;
    seed_account_admin(&pool, account_id, admin_id).await;

    // Canvas carries unified_tool_id on the registration itself, not inside
    // the configuration. The client id is prefix-unique because the shard
    // runs the sibling suites in parallel against the shared database and
    // the lookup matches on the configuration alone.
    let client_id = format!("client-{prefix}");
    let mut configuration = sample_configuration();
    configuration["client_id"] = json!(client_id);
    let response = server
        .post(&format!("/accounts/{account_id}/lti_registrations"))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({
            "name": format!("{prefix}-tool"),
            "vendor": "Example Vendor",
            "unified_tool_id": "utid-123",
            "configuration": configuration,
        }))
        .await;
    assert_eq!(response.status_code(), 201, "body: {}", response.text());
    let registration: Value = response.json();
    let id = registration["id"].as_i64().unwrap();

    // show by client id (:1462-1480).
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registration_by_client_id/{client_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let shown: Value = response.json();
    assert_eq!(shown["id"], id);
    assert!(shown["overlaid_configuration"].is_object());

    // Not visible from another account -> 404.
    let response = server
        .get(&format!(
            "/accounts/{other_account}/lti_registration_by_client_id/{client_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 404);

    // by_utid (:1502-1541).
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registrations/by_utid/utid-123"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let shown: Value = response.json();
    assert_eq!(shown["id"], id);

    // check_domain_duplicates (:1312-1338).
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registrations/check_domain_duplicates?domain=example.com"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let body: Value = response.json();
    let duplicates = body["duplicates"].as_array().unwrap();
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates[0]["id"], id.to_string());

    // install_status: installed directly in this account (:1543-1635).
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_registrations/install_status/{client_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let status: Value = response.json();
    assert_eq!(status["id"], id);

    // Install the template into the other account, then install_status
    // resolves the local copy there (:1822-1862).
    let response = server
        .post(&format!(
            "/accounts/{other_account}/lti_registrations/{id}/install_from_template"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let local_copy: Value = response.json();
    assert_eq!(local_copy["template_registration_id"], id);
    assert_eq!(local_copy["account_id"], other_account);

    let response = server
        .get(&format!(
            "/accounts/{other_account}/lti_registrations/install_status/{client_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let status: Value = response.json();
    assert_eq!(status["id"], local_copy["id"]);

    // A second install is refused.
    let response = server
        .post(&format!(
            "/accounts/{other_account}/lti_registrations/{id}/install_from_template"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 422);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// configuration/validate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_configuration_contract() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct", None).await;
    let admin_id = seed_user(&pool, &prefix, "admin", "admin").await;
    seed_account_admin(&pool, account_id, admin_id).await;

    // Exactly one of lti_configuration or url is required
    // (registrations_controller.rb:1244-1260).
    let response = server
        .post(&format!(
            "/accounts/{account_id}/lti_registrations/configuration/validate"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 422);
    let body: Value = response.json();
    assert_eq!(
        body["errors"][0],
        "one of lti_configuration or url is required"
    );

    let response = server
        .post(&format!(
            "/accounts/{account_id}/lti_registrations/configuration/validate"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({
            "lti_configuration": { "title": "T" },
            "url": "https://example.com/config.json"
        }))
        .await;
    assert_eq!(response.status_code(), 422);
    let body: Value = response.json();
    assert_eq!(
        body["errors"][0],
        "only one of lti_configuration or url is allowed"
    );

    // Missing required fields are reported (lib/schemas/
    // lti_configuration.rb: title, description, target_link_uri,
    // oidc_initiation_url).
    let response = server
        .post(&format!(
            "/accounts/{account_id}/lti_registrations/configuration/validate"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "lti_configuration": { "title": "T" } }))
        .await;
    assert_eq!(response.status_code(), 422);
    let body: Value = response.json();
    let errors: Vec<Value> = body["errors"].as_array().unwrap().clone();
    assert!(errors.contains(&json!("description is missing")));
    assert!(errors.contains(&json!("target_link_uri is missing")));
    assert!(errors.contains(&json!("oidc_initiation_url is missing")));

    // A valid configuration echoes back with redirect_uris defaulted
    // (:1294-1308).
    let response = server
        .post(&format!(
            "/accounts/{account_id}/lti_registrations/configuration/validate"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({
            "lti_configuration": {
                "title": "Valid Tool",
                "description": "desc",
                "target_link_uri": "https://tool.example.com/launch",
                "oidc_initiation_url": "https://tool.example.com/oidc"
            }
        }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["configuration"]["redirect_uris"],
        json!(["https://tool.example.com/launch"])
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Context controls
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_controls_crud_and_bulk() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let root_account = seed_account(&pool, &prefix, "root", None).await;
    let sub_account = seed_account(&pool, &prefix, "sub", Some(root_account)).await;
    let admin_id = seed_user(&pool, &prefix, "admin", "admin").await;
    seed_account_admin(&pool, root_account, admin_id).await;
    let course_id = seed_course(&pool, &prefix, "course", sub_account).await;

    let registration = create_registration(
        &server,
        root_account,
        admin_id,
        &prefix,
        sample_configuration(),
    )
    .await;
    let id = registration["id"].as_i64().unwrap();

    // Without a deployment the create is refused
    // (context_controls_controller.rb:309-311).
    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "account_id": sub_account }))
        .await;
    assert_eq!(response.status_code(), 422);
    let body: Value = response.json();
    assert_eq!(
        body["errors"][0],
        "No active deployment found for the root account."
    );

    // Deploy at the root, then create a control for the sub account.
    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({}))
        .await;
    let deployment: Value = response.json();
    let deployment_id = deployment["id"].as_i64().unwrap();

    // Missing context is refused (:313-315).
    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "deployment_id": deployment_id }))
        .await;
    assert_eq!(response.status_code(), 422);

    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({
            "account_id": sub_account,
            "deployment_id": deployment_id,
            "available": false,
            "comment": "disable for sub"
        }))
        .await;
    assert_eq!(response.status_code(), 201, "body: {}", response.text());
    let control: Value = response.json();
    let control_id = control["id"].as_i64().unwrap();
    assert_eq!(control["available"], false);
    assert_eq!(control["account_id"], sub_account);
    assert_eq!(control["context_name"], format!("{prefix}-sub"));
    assert_eq!(control["registration_id"], id);

    // Duplicates are refused (:317-320).
    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "account_id": sub_account, "deployment_id": deployment_id }))
        .await;
    assert_eq!(response.status_code(), 422);
    let body: Value = response.json();
    assert_eq!(
        body["errors"][0],
        "A context control for this deployment and context already exists."
    );

    // show (:273-285) and update (:503-516).
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls/{control_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let shown: Value = response.json();
    assert_eq!(shown["id"], control_id);

    let response = server
        .put(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls/{control_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "available": true }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let updated: Value = response.json();
    assert_eq!(updated["available"], true);

    // available is required on update.
    let response = server
        .put(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls/{control_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 400);

    // bulk create with course + account contexts, upserting the existing
    // account control (:359-434).
    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls/bulk"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!([
            { "account_id": sub_account, "available": false },
            { "course_id": course_id, "available": true }
        ]))
        .await;
    assert_eq!(response.status_code(), 201, "body: {}", response.text());
    let bulk: Vec<Value> = response.json();
    assert_eq!(bulk.len(), 2);
    let course_control = bulk
        .iter()
        .find(|c| c["course_id"] == course_id)
        .expect("course control");
    let course_control_id = course_control["id"].as_i64().unwrap();

    // Duplicate contexts inside one bulk payload are refused (:617-620).
    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls/bulk"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!([
            { "course_id": course_id },
            { "course_id": course_id }
        ]))
        .await;
    assert_eq!(response.status_code(), 422);

    // index groups by deployment (:232-270).
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let index: Vec<Value> = response.json();
    assert_eq!(index.len(), 1);
    assert_eq!(index[0]["id"], deployment_id);
    assert_eq!(index[0]["context_controls"].as_array().unwrap().len(), 2);

    // deployment controls list (deployments_controller.rb:197-210).
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/{deployment_id}/controls"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let controls: Vec<Value> = response.json();
    assert_eq!(controls.len(), 2);

    // The primary control (deployment's own context) cannot be deleted
    // (:533-548); the course control can.
    let response = server
        .post(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .json(&json!({ "account_id": root_account, "deployment_id": deployment_id }))
        .await;
    assert_eq!(response.status_code(), 201);
    let primary: Value = response.json();
    let primary_id = primary["id"].as_i64().unwrap();

    let response = server
        .delete(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls/{primary_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 422);

    let response = server
        .delete(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/controls/{course_control_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let deleted: Value = response.json();
    assert_eq!(deleted["id"], course_control_id);

    // A deleted control disappears from the listings.
    let response = server
        .get(&format!(
            "/accounts/{root_account}/lti_registrations/{id}/deployments/{deployment_id}/controls"
        ))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    let controls: Vec<Value> = response.json();
    assert_eq!(controls.len(), 2); // primary + sub account control

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// lti_apps listings
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lti_apps_listings() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct", None).await;
    let admin_id = seed_user(&pool, &prefix, "admin", "admin").await;
    let student_id = seed_user(&pool, &prefix, "student", "student").await;
    let outsider_id = seed_user(&pool, &prefix, "outsider", "student").await;
    seed_account_admin(&pool, account_id, admin_id).await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student')")
        .bind(student_id)
        .bind(course_id)
        .execute(&pool)
        .await
        .expect("seed enrollment");

    // An account tool and a course tool.
    sqlx::query(
        "INSERT INTO external_tools \
         (name, url, context_type, context_id, account_id, tool_configuration) \
         VALUES ($1, $2, 'Account', $3, $3, $4)",
    )
    .bind(format!("{prefix}-account-app"))
    .bind("https://app.example.com/launch")
    .bind(account_id)
    .bind(json!({
        "placements": { "global_navigation": { "message_type": "LtiResourceLinkRequest" } },
        "launch_settings": { "icon_url": "https://app.example.com/icon.png" }
    }))
    .execute(&pool)
    .await
    .expect("seed account tool");
    sqlx::query(
        "INSERT INTO external_tools \
         (name, url, context_type, context_id, course_id, tool_configuration) \
         VALUES ($1, $2, 'Course', $3, $3, $4)",
    )
    .bind(format!("{prefix}-course-app"))
    .bind("https://courseapp.example.com/launch")
    .bind(course_id)
    .bind(json!({ "placements": { "course_navigation": {} } }))
    .execute(&pool)
    .await
    .expect("seed course tool");

    // account_apps is admin-gated (lti_apps_controller.rb:97-105).
    let response = server
        .get(&format!("/accounts/{account_id}/lti_apps"))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let apps: Vec<Value> = response.json();
    assert!(!apps.is_empty());
    let account_app = apps
        .iter()
        .find(|app| app["name"] == format!("{prefix}-account-app"))
        .expect("account app listed");
    assert_eq!(account_app["app_type"], "lti_tool");
    assert_eq!(account_app["installed"], true);

    let response = server
        .get(&format!("/accounts/{account_id}/lti_apps"))
        .add_header(AUTHORIZATION, auth(outsider_id, "student"))
        .await;
    assert_eq!(response.status_code(), 403);

    // course_apps lists the course tool for a teacher/admin.
    let response = server
        .get(&format!("/courses/{course_id}/lti_apps"))
        .add_header(AUTHORIZATION, auth(admin_id, "admin"))
        .await;
    assert_eq!(response.status_code(), 200);
    let apps: Vec<Value> = response.json();
    assert!(apps
        .iter()
        .any(|app| app["name"] == format!("{prefix}-course-app")));

    // launch_definitions for a placement (app_launch_collator.rb:102-153).
    let response = server
        .get(&format!(
            "/courses/{course_id}/lti_apps/launch_definitions?placements[]=course_navigation"
        ))
        .add_header(AUTHORIZATION, auth(student_id, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let definitions: Vec<Value> = response.json();
    assert!(!definitions.is_empty());
    let definition = &definitions[0];
    assert_eq!(definition["definition_type"], "ContextExternalTool");
    assert!(definition["placements"]["course_navigation"].is_object());

    // global_navigation launch definitions are open to associated users
    // (lti_apps_controller.rb:170-181) — the enrolled student passes, the
    // outsider does not.
    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_apps/launch_definitions?placements[]=global_navigation&include_context_name=true"
        ))
        .add_header(AUTHORIZATION, auth(student_id, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let definitions: Vec<Value> = response.json();
    assert!(!definitions.is_empty());
    assert_eq!(
        definitions[0]["placements"]["global_navigation"]["message_type"],
        "LtiResourceLinkRequest"
    );
    assert!(definitions[0]["context_name"].is_string());

    let response = server
        .get(&format!(
            "/accounts/{account_id}/lti_apps/launch_definitions?placements[]=global_navigation"
        ))
        .add_header(AUTHORIZATION, auth(outsider_id, "student"))
        .await;
    assert_eq!(response.status_code(), 403);

    cleanup(&pool, &prefix).await;
}
