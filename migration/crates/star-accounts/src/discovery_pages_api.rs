//! Discovery pages — Canvas `DiscoveryPagesApiController` (parity gap).
//!
//! Routes:
//!   - GET  /discovery_pages        (show)
//!   - PUT  /discovery_pages        (upsert)
//!   - POST /discovery_pages/token  (token)
//!
//! Canvas semantics:
//!   - Context: the domain ROOT account; `require_root_account_management`
//!     (admin of a root account) guards every action.
//!   - show: `{discovery_page: {primary: [], secondary: [], active: bool}}`
//!     — the stored `settings.discovery_page` (defaults primary/secondary to
//!     arrays) with `active` recomputed from the stored flag.
//!   - upsert: stores `discovery_page` into the root account settings
//!     (primary/secondary default to [], permitted item keys are kept).
//!   - token: a short-lived (30s) scoped JWT for the preview iframe —
//!     `discovery.preview` scope, iat/exp, the caller's claims. The
//!     workspace JWT secret signs it (Canvas ServicesJwt).

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct DiscoveryPagePayload {
    pub discovery_page: Option<DiscoveryPage>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DiscoveryPage {
    pub primary: Option<Vec<Value>>,
    pub secondary: Option<Vec<Value>>,
    pub active: Option<bool>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The domain root account (first root), like the workspace-wide
/// domain-root-account convention.
async fn root_account_id(pool: &PgPool) -> Result<i64, ApiError> {
    sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL \
         AND workflow_state <> 'deleted' ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

/// require_root_account_management: global admin, or account admin of a
/// root account.
async fn can_manage_root(pool: &PgPool, root_id: i64, auth: &AuthUser) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM account_users \
         WHERE account_id = $1 AND user_id = $2 AND role = 'admin')",
    )
    .bind(root_id)
    .bind(auth.id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

/// Read the stored discovery_page settings object ({} when absent).
async fn stored_discovery_page(
    pool: &PgPool,
    root_id: i64,
) -> Result<Map<String, Value>, ApiError> {
    let settings: Option<Value> = sqlx::query_scalar("SELECT settings FROM accounts WHERE id = $1")
        .bind(root_id)
        .fetch_optional(pool)
        .await?;
    let settings = settings.unwrap_or(Value::Null);
    let page = settings
        .as_object()
        .and_then(|s| s.get("discovery_page"))
        .and_then(Value::as_object)
        .cloned();
    Ok(page.unwrap_or_default())
}

/// The show shape: primary/secondary defaulted to arrays, active recomputed
/// from the stored flag (Canvas discovery_page builder).
fn page_response(page: &Map<String, Value>) -> Value {
    json!({
        "discovery_page": {
            "primary": page.get("primary").cloned().unwrap_or_else(|| json!([])),
            "secondary": page.get("secondary").cloned().unwrap_or_else(|| json!([])),
            "active": page.get("active").and_then(Value::as_bool).unwrap_or(false),
        }
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /discovery_pages
async fn show_discovery_page(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let root_id = root_account_id(&state.db).await?;
    if !can_manage_root(&state.db, root_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let page = stored_discovery_page(&state.db, root_id).await?;
    Ok(Json(page_response(&page)))
}

/// PUT /discovery_pages
async fn upsert_discovery_page(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<DiscoveryPagePayload>,
) -> Result<Json<Value>, ApiError> {
    let root_id = root_account_id(&state.db).await?;
    if !can_manage_root(&state.db, root_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let page = payload.discovery_page.unwrap_or_default();
    let stored = json!({
        "primary": page.primary.unwrap_or_default(),
        "secondary": page.secondary.unwrap_or_default(),
        "active": page.active.unwrap_or(false),
    });

    sqlx::query("UPDATE accounts SET settings = settings || $2::jsonb WHERE id = $1")
        .bind(root_id)
        .bind(json!({ "discovery_page": stored }))
        .execute(&state.db)
        .await?;

    let page = stored_discovery_page(&state.db, root_id).await?;
    Ok(Json(page_response(&page)))
}

/// POST /discovery_pages/token
async fn discovery_page_token(
    State(state): State<AppState>,
    auth: AuthUser,
    payload: Option<Json<DiscoveryPagePayload>>,
) -> Result<Json<Value>, ApiError> {
    let root_id = root_account_id(&state.db).await?;
    if !can_manage_root(&state.db, root_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let page = payload
        .map(|Json(p)| p.discovery_page.unwrap_or_default())
        .unwrap_or_default();

    // Canvas claims: the root account, user, and the page config, plus
    // iat/exp (30s) and the discovery.preview scope.
    let now = chrono::Utc::now().timestamp();
    let payload = json!({
        "root_account_id": root_id,
        "user_id": auth.id,
        "primary": page.primary.unwrap_or_default(),
        "secondary": page.secondary.unwrap_or_default(),
        "active": page.active.unwrap_or(false),
        "iat": now,
        "exp": now + 30,
        "scope": "discovery.preview",
    });
    let token = star_auth::create_custom_token(&payload, &state.jwt_secret)
        .map_err(|e| ApiError::BadRequest(format!("token generation failed: {e}")))?;
    Ok(Json(json!({ "token": token })))
}

/// Mount the discovery-page routes.
pub fn discovery_pages_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/discovery_pages",
            get(show_discovery_page).put(upsert_discovery_page),
        )
        .route("/discovery_pages/token", post(discovery_page_token))
}
