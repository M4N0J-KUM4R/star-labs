//! Progress + JWTs — Canvas `ProgressController` and `JwtsController`
//! (parity gaps).
//!
//! Progress routes:
//!   - GET /progress/{id}         (show — PUBLIC: skip require_user)
//!   - POST /progress/{id}/cancel (cancel)
//!
//! Canvas semantics:
//!   - show: any caller may read (skip_before_action :require_user); `read`
//!     on the progress context is enforced via the context's shape, but
//!     with no user there is nothing to authorize — read is open.
//!   - cancel: `cancel` right (the progress owner or an admin) → sets
//!     workflow_state failed with the optional message.
//!   - progress_json: id, context_id, context_type, user_id, tag,
//!     completion, workflow_state, created_at, updated_at, message, url;
//!     results only when present and not pending.
//!
//! JWT routes:
//!   - POST /jwts         (create — a services JWT for the caller)
//!   - POST /jwts/refresh (refresh — requires the jwt param)
//!
//! Canvas semantics:
//!   - create requires NON-JWT auth (a session/bearer token, not a
//!     services JWT); workflows may require a context (workflows_require_context?)
//!     — CanvasWorkflow::ALLOWED_WORKFLOWS scopes what a user token may
//!     carry; the workspace carries the user identity claims only.
//!   - refresh: 400 {errors: {jwt: "required"}} when jwt is missing.
//!     Refreshing validates the old token and mints a fresh one.

use crate::AuthUser;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct ProgressRow {
    id: i64,
    context_id: i64,
    context_type: String,
    user_id: Option<i64>,
    tag: String,
    completion: Option<f64>,
    workflow_state: String,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
    updated_at: Option<chrono::DateTime<chrono::Utc>>,
    message: Option<String>,
    results: Option<String>,
}

fn progress_json(p: &ProgressRow) -> Value {
    let mut v = json!({
        "id": p.id,
        "context_id": p.context_id,
        "context_type": p.context_type,
        "user_id": p.user_id,
        "tag": p.tag,
        "completion": p.completion,
        "workflow_state": p.workflow_state,
        "created_at": p.created_at.map(|d| d.to_rfc3339()),
        "updated_at": p.updated_at.map(|d| d.to_rfc3339()),
        "message": p.message,
        "url": format!("/api/v1/progress/{}", p.id),
    });
    if p.workflow_state != "pending" {
        if let Some(results) = &p.results {
            v["results"] = json!(results);
        }
    }
    v
}

async fn load_progress(pool: &PgPool, id: i64) -> Result<ProgressRow, ApiError> {
    sqlx::query_as(
        "SELECT id, context_id, context_type, user_id, tag, completion, workflow_state, \
                created_at, updated_at, message, results \
         FROM progresses WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

/// GET /progress/{id} — public read (Canvas skips require_user).
async fn show_progress(
    State(state): State<AppState>,
    Path(progress_id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let progress = load_progress(&state.db, progress_id).await?;
    Ok(Json(progress_json(&progress)))
}

#[derive(Debug, Default, Deserialize)]
pub struct CancelPayload {
    pub message: Option<String>,
}

/// POST /progress/{id}/cancel — owner or admin; failed + message.
async fn cancel_progress(
    State(state): State<AppState>,
    Path(progress_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<CancelPayload>>,
) -> Result<Json<Value>, ApiError> {
    let progress = load_progress(&state.db, progress_id).await?;
    // `cancel` right: the progress's user (owner) or an admin.
    let is_owner = progress.user_id == Some(auth.id);
    if !is_owner && auth.role != "admin" {
        return Err(ApiError::Forbidden);
    }
    let message = payload.map(|Json(p)| p.message).unwrap_or(None);
    sqlx::query(
        "UPDATE progresses SET workflow_state = 'failed', message = $2, updated_at = now() \
         WHERE id = $1",
    )
    .bind(progress_id)
    .bind(&message)
    .execute(&state.db)
    .await?;
    let updated = load_progress(&state.db, progress_id).await?;
    Ok(Json(progress_json(&updated)))
}

// ---------------------------------------------------------------------------
// JWTs
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct CreateJwt {
    pub workflows: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RefreshJwt {
    pub jwt: Option<String>,
}

/// CanvasWorkflow::ALLOWED_WORKFLOWS (the only workflows a user-scoped
/// services JWT may carry).
const ALLOWED_WORKFLOWS: &[&str] = &[
    "lti_line_item",
    "lti_nrps",
    "quizzes2",
    "similarity_detection",
    "originality_report",
    "file_access",
];

/// POST /jwts — a services JWT for the caller (non-JWT auth required: the
/// bearer middleware already rejects other token kinds).
async fn create_jwt(
    State(state): State<AppState>,
    auth: AuthUser,
    payload: Option<Json<CreateJwt>>,
) -> Result<Json<Value>, ApiError> {
    if let Some(Json(p)) = &payload {
        for w in p.workflows.iter().flatten() {
            if !ALLOWED_WORKFLOWS.contains(&w.as_str()) {
                return Err(ApiError::BadRequest(format!("unsupported workflow {w}")));
            }
        }
    }
    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({
        "sub": auth.id,
        "role": auth.role,
        "iat": now,
    });
    if let Some(Json(p)) = &payload {
        if let Some(workflows) = &p.workflows {
            claims["workflows"] = json!(workflows);
        }
    }
    let token = crate::create_custom_token(&claims, &state.jwt_secret)
        .map_err(|e| ApiError::BadRequest(format!("token generation failed: {e}")))?;
    Ok(Json(json!({ "token": token })))
}

/// POST /jwts/refresh — the jwt param is required (400 with the errors
/// shape); a valid old token mints a fresh one.
async fn refresh_jwt(
    State(state): State<AppState>,
    payload: Option<Json<RefreshJwt>>,
) -> Result<Json<Value>, ApiError> {
    let jwt = payload
        .and_then(|Json(p)| p.jwt)
        .ok_or_else(|| ApiError::BadRequest("jwt is required".into()))?;
    // Validate the old token through the standard claims path.
    if crate::verify_token(&jwt, &state.jwt_secret).is_none() {
        return Err(ApiError::Unauthorized);
    }
    let now = chrono::Utc::now().timestamp();
    let claims = json!({ "iat": now, "refreshed": true });
    let token = crate::create_custom_token(&claims, &state.jwt_secret)
        .map_err(|e| ApiError::BadRequest(format!("token generation failed: {e}")))?;
    Ok(Json(json!({ "token": token })))
}

/// Mount the progress + jwt routes.
pub fn progress_jwt_routes() -> Router<AppState> {
    Router::new()
        .route("/progress/{id}", get(show_progress))
        .route("/progress/{id}/cancel", post(cancel_progress))
        .route("/jwts", post(create_jwt))
        .route("/jwts/refresh", post(refresh_jwt))
}
