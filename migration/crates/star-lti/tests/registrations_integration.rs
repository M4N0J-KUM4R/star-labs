//! Integration tests for the star-lti LTI registration sub-routes.
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
        "lti-reg-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

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

#[allow(dead_code)]
async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin') \
         ON CONFLICT (account_id, user_id) DO NOTHING",
    )
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

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // The registrations of the site-admin account survive the account
    // cleanup, so they are removed by name.
    let _ = sqlx::query("DELETE FROM lti_registrations WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    // Feature flags and the IMS rows reference the seeded rows without a
    // cascade, so they go first.
    let _ = sqlx::query(
        "DELETE FROM feature_flags \
         WHERE context_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM lti_ims_registrations WHERE guid LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM developer_keys WHERE name LIKE $1")
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

/// Create a registration through the crate's own endpoint.
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
            "admin_nickname": "Campus Tool",
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

/// A valid internal LTI configuration.
fn tool_configuration() -> Value {
    json!({
        "title": "Configured Tool",
        "description": "A tool",
        "target_link_uri": "https://example.com/launch",
        "oidc_initiation_url": "https://example.com/oidc",
        "redirect_uris": ["https://example.com/launch"],
        "scopes": ["openid"],
        "placements": [{ "placement": "course_navigation", "enabled": true }],
        "launch_settings": { "icon_url": "https://example.com/icon.png" },
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_configuration_validates_and_reports_errors() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let sub = seed_account(&pool, &prefix, "sub", Some(account)).await;
    let path = format!("/accounts/{account}/lti_registrations/configuration/validate");

    // A valid LTI configuration becomes the internal configuration.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "lti_configuration": {
                "title": "Configured Tool",
                "description": "A tool",
                "target_link_uri": "https://example.com/launch",
                "oidc_initiation_url": "https://example.com/oidc",
            }
        }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let configuration = &body["configuration"];
    assert_eq!(configuration["title"], "Configured Tool");
    assert_eq!(
        configuration["target_link_uri"],
        "https://example.com/launch"
    );
    assert_eq!(configuration["scopes"], json!([]));
    assert_eq!(
        configuration["redirect_uris"],
        json!(["https://example.com/launch"]),
        "Canvas defaults the redirect URIs to the target link URI"
    );
    assert!(configuration["launch_settings"].is_object());
    assert!(configuration["placements"].is_array());

    // Neither parameter.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["errors"],
        json!(["one of lti_configuration or url is required"])
    );

    // Both parameters.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "lti_configuration": { "title": "x" },
            "url": "https://example.com/config.json"
        }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["errors"],
        json!(["only one of lti_configuration or url is allowed"])
    );

    // A missing required property.
    let response = server
        .post(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "lti_configuration": { "title": "Configured Tool" }
        }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    let errors = body["errors"].as_array().expect("errors array");
    assert!(
        errors
            .iter()
            .any(|error| error.as_str().unwrap_or_default().contains("description")),
        "the missing property is reported: {errors:?}"
    );

    // Sub-accounts may not validate configurations.
    let response = server
        .post(&format!(
            "/accounts/{sub}/lti_registrations/configuration/validate"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "lti_configuration": { "title": "x" } }))
        .await;
    assert_eq!(
        response.status_code(),
        404,
        "sub-accounts need canvas_apps_sub_account_access; body: {}",
        response.text()
    );

    enable_feature(&pool, account, "canvas_apps_sub_account_access").await;
    let response = server
        .post(&format!(
            "/accounts/{sub}/lti_registrations/configuration/validate"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "lti_configuration": { "title": "x" } }))
        .await;
    assert_eq!(
        response.status_code(),
        403,
        "sub-accounts are read-only; body: {}",
        response.text()
    );
    let body: Value = response.json();
    assert_eq!(body["errors"], "sub-accounts can only view registrations");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn check_domain_duplicates_finds_matching_registrations() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let path = format!("/accounts/{account}/lti_registrations/check_domain_duplicates");

    // No domain, no duplicates.
    let response = server
        .get(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["duplicates"], json!([]));

    let mut configuration = tool_configuration();
    configuration["domain"] = json!("Example.COM");
    let created = create_registration(&server, account, admin, &prefix, configuration).await;
    let id = created["id"].as_i64().expect("registration id");

    // The domain match is case insensitive.
    let response = server
        .get(&format!("{path}?domain=example.com"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let duplicates = body["duplicates"].as_array().expect("duplicates array");
    assert_eq!(duplicates.len(), 1);
    assert_eq!(duplicates[0]["id"].as_i64(), Some(id));
    assert_eq!(duplicates[0]["name"], format!("{prefix}-tool"));
    assert_eq!(duplicates[0]["admin_nickname"], "Campus Tool");

    // A different domain matches nothing.
    let response = server
        .get(&format!("{path}?domain=other.example"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["duplicates"], json!([]));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn context_search_returns_accounts_and_courses() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let sub = seed_account(&pool, &prefix, "sub", Some(account)).await;
    let _course = seed_course(&pool, &prefix, "course", account).await;
    enable_feature(&pool, account, "lti_registrations_next").await;

    let created = create_registration(&server, account, admin, &prefix, tool_configuration()).await;
    let registration_id = created["id"].as_i64().expect("registration id");

    let deployment_id: i64 = sqlx::query_scalar(
        "INSERT INTO lti_deployments \
             (registration_id, deployment_id, context_type, context_id, context_name, available, \
              workflow_state) \
         VALUES ($1, $2, 'Account', $3, $4, TRUE, 'active') RETURNING id",
    )
    .bind(registration_id)
    .bind(format!("{prefix}-deployment"))
    .bind(account)
    .bind(format!("{prefix}-root"))
    .fetch_one(&pool)
    .await
    .expect("seed deployment");

    let path = format!(
        "/accounts/{account}/lti_registrations/{registration_id}/deployments/{deployment_id}/context_search"
    );

    let response = server
        .get(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let accounts = body["accounts"].as_array().expect("accounts array");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0]["id"], sub.to_string());
    assert_eq!(accounts[0]["name"], format!("{prefix}-sub"));
    assert!(accounts[0]["display_path"].is_array());
    let courses = body["courses"].as_array().expect("courses array");
    assert_eq!(courses.len(), 1);
    assert!(courses[0]["course_code"]
        .as_str()
        .unwrap()
        .starts_with(&prefix));

    // A search term filters the accounts.
    let response = server
        .get(&format!("{path}?search_term=nothing-matches"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["accounts"], json!([]));
    assert_eq!(body["courses"], json!([]));

    // `only_children_of` outside the deployment is a 422.
    let response = server
        .get(&format!("{path}?only_children_of=999999"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["code"], "invalid_account_id");

    // Without the feature flag the route is a 404.
    sqlx::query(
        "UPDATE feature_flags SET state = 'disabled' \
         WHERE feature = 'lti_registrations_next' AND context_id = $1",
    )
    .bind(account)
    .execute(&pool)
    .await
    .expect("disable feature flag");
    let response = server
        .get(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn overlay_history_and_history_return_collections() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    enable_feature(&pool, account, "lti_registrations_next").await;
    enable_feature(&pool, account, "lti_registrations_history").await;

    let created = create_registration(&server, account, admin, &prefix, tool_configuration()).await;
    let id = created["id"].as_i64().expect("registration id");

    // No overlay, no overlay history.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registrations/{id}/overlay_history"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body, json!([]));

    // The registration history is empty as well.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registrations/{id}/history"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body, json!([]));

    // Canvas validates the limit only for a registration that carries an
    // overlay (registrations_controller.rb:1968-1985); without one the
    // answer stays [] whatever the limit says.
    let response = server
        .post(&format!("/accounts/{account}/lti_registrations"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "name": format!("{prefix}-overlay-tool"),
            "vendor": "Example Vendor",
            "configuration": tool_configuration(),
            "overlay": { "privacy_level": "public" },
        }))
        .await;
    assert_eq!(response.status_code(), 201, "body: {}", response.text());
    let id = response.json::<Value>()["id"]
        .as_i64()
        .expect("registration id");

    // An invalid limit is a 422.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registrations/{id}/overlay_history?limit=0"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["code"], "invalid_limit");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_requests_are_shown_latest_and_applied() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let other = seed_account(&pool, &prefix, "other", None).await;

    let created = create_registration(&server, account, admin, &prefix, tool_configuration()).await;
    let id = created["id"].as_i64().expect("registration id");

    let update_request_id: i64 = sqlx::query_scalar(
        "INSERT INTO lti_registration_update_requests \
             (uuid, lti_registration_id, root_account_id, lti_ims_registration) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-request"))
    .bind(id)
    .bind(account)
    .bind(json!({ "client_name": "Renamed Tool" }))
    .fetch_one(&pool)
    .await
    .expect("seed update request");

    // The single update request.
    let path =
        format!("/accounts/{account}/lti_registrations/{id}/update_requests/{update_request_id}");
    let response = server
        .get(&path)
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"].as_i64(), Some(update_request_id));
    assert_eq!(body["lti_registration_id"].as_i64(), Some(id));
    assert_eq!(body["status"], "pending");
    assert_eq!(body["reinstall"], false);
    assert_eq!(body["tool_initiated"], false);
    assert_eq!(body["comment"], Value::Null);

    // The latest update request.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registrations/{id}/latest_update_request"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"].as_i64(), Some(update_request_id));

    // A registration of another account is a 400.
    let response = server
        .get(&format!(
            "/accounts/{other}/lti_registrations/{id}/latest_update_request"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"], "registration does not belong to account");

    // `accepted` is required.
    let response = server
        .put(&format!("{path}/apply"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"], "accepted parameter is required");

    // Rejecting the request marks it rejected.
    let response = server
        .put(&format!("{path}/apply"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "accepted": false }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"].as_i64(), Some(id));

    let rejected_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT rejected_at FROM lti_registration_update_requests WHERE id = $1",
    )
    .bind(update_request_id)
    .fetch_one(&pool)
    .await
    .expect("read update request");
    assert!(rejected_at.is_some(), "the request is rejected");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_and_reset_change_the_registration() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    enable_feature(&pool, account, "lti_registrations_next").await;

    let created = create_registration(&server, account, admin, &prefix, tool_configuration()).await;
    let id = created["id"].as_i64().expect("registration id");

    // Update the registration.
    let response = server
        .put(&format!("/accounts/{account}/lti_registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "name": format!("{prefix}-renamed"),
            "configuration": {
                "title": "Updated Tool",
                "target_link_uri": "https://example.com/launch",
                "oidc_initiation_url": "https://example.com/oidc",
                "redirect_uris": ["https://example.com/launch"],
                "scopes": ["openid"],
                "placements": [{ "placement": "course_navigation", "enabled": true }],
                "launch_settings": {},
            },
            "overlay": { "title": "Overlaid Tool" },
        }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["name"], format!("{prefix}-renamed"));
    assert_eq!(
        body["configuration"]["scopes"],
        json!(["openid"]),
        "the untouched configuration keys survive"
    );
    assert_eq!(
        body["overlay"],
        json!({}),
        "a manual registration merges the overlay into the configuration"
    );
    assert_eq!(
        body["configuration"]["title"], "Overlaid Tool",
        "the overlay overwrites the configuration"
    );

    // An invalid workflow state is a 422.
    let response = server
        .put(&format!("/accounts/{account}/lti_registrations/{id}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "workflow_state": "bogus" }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["code"], "invalid_workflow_state");

    // Reset empties the overlay.
    let response = server
        .put(&format!("/accounts/{account}/lti_registrations/{id}/reset"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["overlay"], json!({}));
    assert_eq!(
        body["overlaid_configuration"]["title"], "Overlaid Tool",
        "the merged configuration is still there"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bind_unbind_and_install_from_template_manage_the_bindings() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    enable_feature(&pool, account, "lti_deactivate_registrations").await;
    enable_feature(&pool, account, "lti_registrations_templates").await;

    // A template registration owned by the site admin (the root account).
    let site_admin_id: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("site admin account");
    let template =
        create_registration(&server, site_admin_id, admin, &prefix, tool_configuration()).await;
    let template_id = template["id"].as_i64().expect("template id");

    // Binding from the site admin itself is refused.
    let response = server
        .post(&format!(
            "/accounts/{site_admin_id}/lti_registrations/{template_id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "workflow_state": "on" }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["code"], "invalid_context");

    // An invalid workflow state is a 422.
    let response = server
        .post(&format!(
            "/accounts/{account}/lti_registrations/{template_id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "workflow_state": "active" }))
        .await;
    assert_eq!(response.status_code(), 422, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"][0]["code"], "invalid_workflow_state");

    // Binding installs the template.
    let response = server
        .post(&format!(
            "/accounts/{account}/lti_registrations/{template_id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "workflow_state": "on" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["registration_id"].as_i64(), Some(template_id));
    assert_eq!(body["account_id"].as_i64(), Some(account));
    assert_eq!(body["workflow_state"], "on");

    // The local copy now exists.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registrations/{template_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"].as_i64(), Some(template_id));

    let local_copies: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM lti_registrations \
         WHERE template_registration_id = $1 AND account_id = $2",
    )
    .bind(template_id)
    .bind(account)
    .fetch_one(&pool)
    .await
    .expect("count local copies");
    assert_eq!(
        local_copies.0, 1,
        "the template is installed in the account"
    );

    // Installing the template explicitly creates the local copy.
    let response = server
        .post(&format!(
            "/accounts/{account}/lti_registrations/{template_id}/install_from_template"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["template_registration_id"].as_i64(), Some(template_id));
    assert_eq!(body["account_id"].as_i64(), Some(account));
    assert!(body["account_binding"].is_object());

    // Unbinding removes the binding again.
    let response = server
        .delete(&format!(
            "/accounts/{account}/lti_registrations/{template_id}/bind"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["registration_id"].as_i64(), Some(template_id));
    assert_eq!(body["workflow_state"], "on");

    let bindings: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM lti_registration_account_bindings \
         WHERE registration_id = $1 AND account_id = $2",
    )
    .bind(template_id)
    .bind(account)
    .fetch_one(&pool)
    .await
    .expect("count bindings");
    assert_eq!(bindings.0, 0, "the binding is gone");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn client_id_unified_tool_id_and_install_status_resolve_registrations() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    enable_feature(&pool, account, "lti_registrations_templates").await;

    let created = create_registration(&server, account, admin, &prefix, tool_configuration()).await;
    let id = created["id"].as_i64().expect("registration id");

    // A developer key with an IMS registration for the registration.
    let developer_key_id: i64 = sqlx::query_scalar(
        "INSERT INTO developer_keys (name, api_key) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-key"))
    .bind(format!("{prefix}-api-key"))
    .fetch_one(&pool)
    .await
    .expect("seed developer key");
    sqlx::query(
        "INSERT INTO lti_ims_registrations \
             (guid, developer_key_id, lti_registration_id, client_name, lti_tool_configuration, \
              unified_tool_id) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(format!("{prefix}-ims"))
    .bind(developer_key_id)
    .bind(id)
    .bind("Campus Tool")
    .bind(tool_configuration())
    .bind(format!("{prefix}-utid"))
    .execute(&pool)
    .await
    .expect("seed ims registration");

    // The client id resolves the registration.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registration_by_client_id/{developer_key_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"].as_i64(), Some(id));
    assert_eq!(body["developer_key_id"].as_i64(), Some(developer_key_id));
    assert!(body["ims_registration_id"].is_number());
    assert!(body["configuration"].is_object());
    assert!(body["overlaid_configuration"].is_object());

    // An unknown client id is a 404.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registration_by_client_id/99999999"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"], "LTI registration not found");

    // The unified tool id resolves the IMS registration.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registrations/by_utid/{prefix}-utid"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"].as_i64(), Some(id));

    // An unknown unified tool id is a 404.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registrations/by_utid/nothing-here"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["errors"], "LTI registration not found");

    // The install status of a registration of this account is the registration.
    let response = server
        .get(&format!(
            "/accounts/{account}/lti_registrations/install_status/{developer_key_id}"
        ))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"].as_i64(), Some(id));

    cleanup(&pool, &prefix).await;
}
