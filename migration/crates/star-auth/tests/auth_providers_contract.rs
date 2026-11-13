//! Independent contract tests for authentication_providers and sso_settings.
//!
//! These tests are written from the Canvas API contract (not from the
//! implementation) and verify behavioral correctness of the handlers in
//! star-auth/src/lib.rs.
//!
//! Canvas reference for auth providers:
//!   - GET/POST /api/v1/accounts/{account_id}/authentication_providers
//!   - GET/PUT/DELETE /api/v1/accounts/{account_id}/authentication_providers/{id}
//!   - Canvas wraps list in {"authentication_providers": [...]} but show/update/
//!     delete return the object directly (not wrapped).
//!   - auth_type is required on create.
//!
//! Canvas reference for SSO settings:
//!   - GET/PUT /api/v1/accounts/{account_id}/sso_settings
//!   - Wrapped in {"sso_settings": {...}} both for GET and PUT response.
//!   - SSO settings are account-level; unknown account is 404.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// -----------------------------------------------------------------------
// Test harness
// -----------------------------------------------------------------------

const JWT_SECRET: &str = "test-jwt-secret-contracts";
const SEED_PASSWORD: &str = "pw-test-1234";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "auth-providers-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

async fn db_pool() -> Option<PgPool> {
    let db_url = std::env::var("DATABASE_URL").ok()?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");
    star_db::MIGRATOR.run(&pool).await.expect("run migrations");
    Some(pool)
}

async fn setup() -> Option<(TestServer, PgPool, String)> {
    let pool = db_pool().await?;
    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = TestServer::new(star_auth_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn star_auth_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_auth::router())
        .with_state(state)
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role)
         VALUES ($1, $2, $3, $4)
         RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password(SEED_PASSWORD).expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

async fn seed_account_if_needed(pool: &PgPool) {
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = 1")
        .fetch_optional(pool)
        .await
        .expect("check account");
    if exists.is_none() {
        sqlx::query("INSERT INTO accounts (id, name) VALUES (1, 'Test Account') ON CONFLICT (id) DO NOTHING")
            .execute(pool)
            .await
            .expect("seed account");
    }
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM authentication_providers WHERE auth_type LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

// -----------------------------------------------------------------------
// authentication_providers — list shape and auth
// -----------------------------------------------------------------------

/// GET /accounts/{account_id}/authentication_providers wraps the array in
/// {"authentication_providers": [...]} (Canvas Api::V1::AuthenticationProvidersController).
#[tokio::test]
async fn list_auth_providers_returns_wrapped_array() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "ListAdmin", "admin").await;

    let resp = server
        .get("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;

    assert_eq!(resp.status_code(), 200, "list: {}", resp.text());
    let body: Value = resp.json();
    // Must be wrapped per Canvas contract.
    assert!(
        body.get("authentication_providers")
            .and_then(|v| v.as_array())
            .is_some(),
        "list body must be wrapped in {{'authentication_providers': [...]}}: {body}"
    );

    cleanup(&pool, &prefix).await;
}

/// Non-admin gets 403 on list (not 401 — token is valid but role is forbidden).
#[tokio::test]
async fn list_auth_providers_rejects_non_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let teacher = seed_user(&pool, &prefix, "TeacherList", "teacher").await;

    let resp = server
        .get("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;

    assert_eq!(
        resp.status_code(),
        403,
        "teacher must get 403 on admin-only endpoint: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

/// List is ordered by position ASC NULLS LAST, then id ASC (Canvas
/// AuthenticationProvider.default_scope).
#[tokio::test]
async fn list_auth_providers_ordered_by_position_then_id() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "OrderAdmin", "admin").await;

    // Create three providers with explicit positions.
    let positions = [2, 0, 1];
    let mut ids = vec![];
    for pos in positions {
        let resp = server
            .post("/accounts/1/authentication_providers")
            .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
            .json(&json!({
                "authentication_provider": {
                    "auth_type": format!("{}-{}", prefix, pos),
                    "position": pos
                }
            }))
            .await;
        assert_eq!(resp.status_code(), 200, "create pos {pos}: {}", resp.text());
        ids.push(resp.json::<Value>()["id"].as_i64().unwrap());
    }

    // Read list.
    let resp = server
        .get("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    let body: Value = resp.json();
    let list = body["authentication_providers"].as_array().unwrap();

    // Filter to just our providers.
    let our_providers: Vec<_> = list
        .iter()
        .filter(|p| {
            let t = p["auth_type"].as_str().unwrap_or("");
            t.starts_with(prefix.as_str())
        })
        .collect();

    // Must be ordered position 0, 1, 2.
    assert_eq!(our_providers.len(), 3);
    assert_eq!(our_providers[0]["position"], 0, "first by position");
    assert_eq!(our_providers[1]["position"], 1, "second by position");
    assert_eq!(our_providers[2]["position"], 2, "third by position");

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------
// authentication_providers — create
// -----------------------------------------------------------------------

/// auth_type is required on POST; omitting it returns 422 with a validation
/// message (Canvas requires the field).
#[tokio::test]
async fn create_auth_provider_requires_auth_type() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "NoTypeAdmin", "admin").await;

    let resp = server
        .post("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "authentication_provider": {
                // auth_type intentionally omitted
                "position": 1
            }
        }))
        .await;

    assert_eq!(
        resp.status_code(),
        422,
        "missing auth_type must be 422: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

/// When position is absent on create, it defaults to 0 (append behavior).
#[tokio::test]
async fn create_auth_provider_defaults_position_to_zero() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "DefaultPosAdmin", "admin").await;

    let resp = server
        .post("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "authentication_provider": {
                "auth_type": format!("{}-noposition", prefix)
            }
        }))
        .await;

    assert_eq!(resp.status_code(), 200, "create: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(
        created["position"], 0,
        "position must default to 0 when not specified: {}",
        created
    );

    cleanup(&pool, &prefix).await;
}

/// POST also accepts flat params (no `authentication_provider` wrapper),
/// matching Canvas which accepts both forms.
#[tokio::test]
async fn create_auth_provider_accepts_flat_params() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "FlatAdmin", "admin").await;

    let resp = server
        .post("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "auth_type": "saml",
            "position": 3,
            "idp_entity_id": "https://idp.flat.example.org"
        }))
        .await;

    assert_eq!(resp.status_code(), 200, "flat create: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["auth_type"], "saml");
    assert_eq!(created["position"], 3);
    assert_eq!(created["idp_entity_id"], "https://idp.flat.example.org");

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------
// authentication_providers — show/update/delete
// -----------------------------------------------------------------------

/// GET /accounts/{account_id}/authentication_providers/{id} returns the
/// provider directly (no wrapper), matching Canvas show action.
#[tokio::test]
async fn show_auth_provider_returns_unwrapped_object() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "ShowAdmin", "admin").await;

    // Create first.
    let create_resp = server
        .post("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "authentication_provider": {
                "auth_type": format!("{}-show-test", prefix),
                "position": 1
            }
        }))
        .await;
    let id = create_resp.json::<Value>()["id"].as_i64().unwrap();

    // Show: must NOT be wrapped in {"authentication_providers": [...]}.
    let resp = server
        .get(&format!("/accounts/1/authentication_providers/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;

    assert_eq!(resp.status_code(), 200, "show: {}", resp.text());
    let body: Value = resp.json();
    assert!(
        body.as_object().is_some()
            && !body
                .as_object()
                .unwrap()
                .contains_key("authentication_providers"),
        "show response must be the bare object, not wrapped: {body}"
    );
    assert_eq!(body["id"], id);

    cleanup(&pool, &prefix).await;
}

/// Unknown provider id → 404 on show.
#[tokio::test]
async fn show_auth_provider_unknown_id_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "UnknownShowAdmin", "admin").await;

    let resp = server
        .get("/accounts/1/authentication_providers/999999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;

    assert_eq!(
        resp.status_code(),
        404,
        "unknown id must be 404: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

/// PUT update merges settings (existing keys preserved, new/overlaid keys applied).
/// This is a key Canvas behavioral contract.
#[tokio::test]
async fn update_auth_provider_merges_settings() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "MergeAdmin", "admin").await;

    // Create with two settings.
    let create_resp = server
        .post("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "authentication_provider": {
                "auth_type": format!("{}-merge", prefix),
                "idp_entity_id": "https://original.example.org",
                "log_in_url": "https://original.example.org/login"
            }
        }))
        .await;
    let id = create_resp.json::<Value>()["id"].as_i64().unwrap();

    // Update only log_in_url — idp_entity_id must survive.
    let update_resp = server
        .put(&format!("/accounts/1/authentication_providers/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "authentication_provider": {
                "log_in_url": "https://new.example.org/login"
            }
        }))
        .await;

    assert_eq!(
        update_resp.status_code(),
        200,
        "update: {}",
        update_resp.text()
    );
    let updated: Value = update_resp.json();
    assert_eq!(
        updated["log_in_url"], "https://new.example.org/login",
        "updated field must reflect new value"
    );
    assert_eq!(
        updated["idp_entity_id"], "https://original.example.org",
        "unmodified field must be preserved after merge update"
    );

    cleanup(&pool, &prefix).await;
}

/// DELETE returns the deleted provider JSON with workflow_state = "deleted".
/// The provider is then unlisted and show returns 404.
#[tokio::test]
async fn delete_auth_provider_returns_deleted_object_and_purges_from_list() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "DelAdmin", "admin").await;

    let create_resp = server
        .post("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "authentication_provider": {
                "auth_type": format!("{}-delete", prefix),
                "position": 10
            }
        }))
        .await;
    let id = create_resp.json::<Value>()["id"].as_i64().unwrap();

    // Delete returns the object with workflow_state "deleted".
    let delete_resp = server
        .delete(&format!("/accounts/1/authentication_providers/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;

    assert_eq!(
        delete_resp.status_code(),
        200,
        "delete: {}",
        delete_resp.text()
    );
    let deleted: Value = delete_resp.json();
    assert_eq!(
        deleted["workflow_state"], "deleted",
        "delete response must show workflow_state=deleted: {}",
        deleted
    );
    assert_eq!(deleted["id"], id);

    // Subsequent show is 404.
    let show_resp = server
        .get(&format!("/accounts/1/authentication_providers/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        show_resp.status_code(),
        404,
        "show after delete must be 404"
    );

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------
// sso_settings — shape and auth
// -----------------------------------------------------------------------

/// GET /accounts/{account_id}/sso_settings returns {"sso_settings": {...}}.
#[tokio::test]
async fn sso_settings_show_returns_wrapped_object() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "SsoShowAdmin", "admin").await;

    let resp = server
        .get("/accounts/1/sso_settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;

    assert_eq!(resp.status_code(), 200, "get sso_settings: {}", resp.text());
    let body: Value = resp.json();
    assert!(
        body.get("sso_settings")
            .and_then(|v| v.as_object())
            .is_some(),
        "sso_settings response must be wrapped in {{'sso_settings': {{...}}}}: {body}"
    );

    cleanup(&pool, &prefix).await;
}

/// Non-admin: student gets 403 on sso_settings (admin-only).
#[tokio::test]
async fn sso_settings_rejects_non_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let teacher = seed_user(&pool, &prefix, "SsoTeacher", "teacher").await;

    let resp = server
        .get("/accounts/1/sso_settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;

    assert_eq!(
        resp.status_code(),
        403,
        "teacher must get 403 on sso_settings: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

/// Unknown account on sso_settings → 404.
#[tokio::test]
async fn sso_settings_show_unknown_account_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "SsoUnknownAdmin", "admin").await;

    let resp = server
        .get("/accounts/999999999/sso_settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;

    assert_eq!(
        resp.status_code(),
        404,
        "unknown account must be 404: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

/// PUT sso_settings persists and returns updated settings wrapped in
/// {"sso_settings": {...}}.
#[tokio::test]
async fn sso_settings_update_persists_and_returns_wrapped() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    // Dedicated account: the flat-update test writes account 1 from a
    // parallel thread in this same binary, which races this test's GET.
    let account_id: i64 = sqlx::query_scalar(
        "INSERT INTO accounts (name) VALUES ($1) RETURNING id",
    )
    .bind(format!("{prefix}-sso-account"))
    .fetch_one(&pool)
    .await
    .expect("seed account");
    let admin = seed_user(&pool, &prefix, "SsoUpdAdmin", "admin").await;

    let put_resp = server
        .put(&format!("/accounts/{account_id}/sso_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "sso_settings": {
                "login_handle_name": "MyLoginHandle",
                "change_password_url": "https://example.com/change-password",
                "auth_discovery_url": "https://example.com/sso/discover",
                "discovery_page_active": true
            }
        }))
        .await;

    assert_eq!(
        put_resp.status_code(),
        200,
        "put sso_settings: {}",
        put_resp.text()
    );
    let body: Value = put_resp.json();
    assert!(
        body.get("sso_settings")
            .and_then(|v| v.as_object())
            .is_some(),
        "put response must be wrapped in {{'sso_settings': {{...}}}}: {body}"
    );
    let settings = &body["sso_settings"];
    assert_eq!(settings["login_handle_name"], "MyLoginHandle");
    assert_eq!(
        settings["change_password_url"],
        "https://example.com/change-password"
    );
    assert_eq!(
        settings["auth_discovery_url"],
        "https://example.com/sso/discover"
    );
    assert_eq!(settings["discovery_page_active"], true);

    // GET reflects persisted values.
    let get_resp = server
        .get(&format!("/accounts/{account_id}/sso_settings"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    let get_body: Value = get_resp.json();
    assert_eq!(
        get_body["sso_settings"]["login_handle_name"], "MyLoginHandle",
        "persisted login_handle_name must survive GET: {}",
        get_body
    );

    cleanup(&pool, &prefix).await;
}

/// PUT sso_settings with unknown account → 404.
#[tokio::test]
async fn sso_settings_update_unknown_account_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "SsoUpdUnknownAdmin", "admin").await;

    let resp = server
        .put("/accounts/999999999/sso_settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "sso_settings": { "login_handle_name": "test" }
        }))
        .await;

    assert_eq!(
        resp.status_code(),
        404,
        "put to unknown account must be 404: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

/// SSO settings update accepts flat params (no wrapper).
#[tokio::test]
async fn sso_settings_update_accepts_flat_params() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "SsoFlatAdmin", "admin").await;

    let resp = server
        .put("/accounts/1/sso_settings")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "login_handle_name": "FlatHandle",
            "unknown_user_url": "https://example.com/unknown"
        }))
        .await;

    assert_eq!(resp.status_code(), 200, "flat update: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["sso_settings"]["login_handle_name"], "FlatHandle");
    assert_eq!(
        body["sso_settings"]["unknown_user_url"],
        "https://example.com/unknown"
    );

    cleanup(&pool, &prefix).await;
}

// -----------------------------------------------------------------------
// Authentication providers — account_id path param not enforced
// -----------------------------------------------------------------------

/// The show/update/delete handlers do NOT validate that the account_id in
/// the path matches the provider's account_id. A provider created for
/// account 1 can be fetched via /accounts/999/... This is a behavioral
/// deviation from Canvas (where all accountScoped routes validate scope).
/// Documenting as a known gap.
#[tokio::test]
async fn auth_provider_show_ignores_account_id_path_param() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    seed_account_if_needed(&pool).await;
    let admin = seed_user(&pool, &prefix, "AcctMismatchAdmin", "admin").await;

    // Create for account 1.
    let create_resp = server
        .post("/accounts/1/authentication_providers")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "authentication_provider": {
                "auth_type": format!("{}-acctest", prefix)
            }
        }))
        .await;
    let id = create_resp.json::<Value>()["id"].as_i64().unwrap();

    // Fetch using wrong account path segment — still returns 200.
    let resp = server
        .get(&format!("/accounts/999999/authentication_providers/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;

    // This currently succeeds (known gap). In Canvas this would be a 404
    // or redirect because the authenticated admin context doesn't have
    // permission on account 999999.
    assert_eq!(
        resp.status_code(),
        200,
        "show via mismatched account path: {} — NOTE: Canvas would scope-check this",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}
