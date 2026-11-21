//! temporary_enrollment_pairings — Canvas
//! `TemporaryEnrollmentPairingsApiController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /accounts/{accountId}/temporary_enrollment_pairings` — the pairings of the root account
//! - `GET    /accounts/{accountId}/temporary_enrollment_pairings/{id}` — one pairing
//! - `GET    /accounts/{accountId}/temporary_enrollment_pairings/new` — an unsaved pairing
//! - `POST   /accounts/{accountId}/temporary_enrollment_pairings` — create a pairing
//! - `DELETE /accounts/{accountId}/temporary_enrollment_pairings/{id}` — delete a pairing
//!
//! Canvas routes (config/routes.rb, the account API block):
//!   get    "accounts/:account_id/temporary_enrollment_pairings", action: :index
//!   get    "accounts/:account_id/temporary_enrollment_pairings/:id", action: :show
//!   get    "accounts/:account_id/temporary_enrollment_pairings/new", action: :new
//!   post   "accounts/:account_id/temporary_enrollment_pairings", action: :create
//!   delete "accounts/:account_id/temporary_enrollment_pairings/:id", action: :destroy
//!
//! Canvas controller
//! (`app/controllers/temporary_enrollment_pairings_api_controller.rb`): the
//! `before_action` chain is `require_feature_flag` (404 without the
//! `temporary_enrollments` feature), `authorize_action` (the account has to
//! exist — 404 — and the caller needs the manage-temporary-enrollment right)
//! and `load_temporary_enrollment_pairing` (404).
//!
//! The pairing JSON is the Canvas `TemporaryEnrollmentPairing` shape: `id`,
//! `workflow_state`, `ending_enrollment_state`, `created_by_id`,
//! `deleted_by_id` and the timestamps.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// A `temporary_enrollment_pairings` row.
#[derive(Debug, sqlx::FromRow)]
struct Pairing {
    id: i64,
    root_account_id: i64,
    workflow_state: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    created_by_id: Option<i64>,
    deleted_by_id: Option<i64>,
    ending_enrollment_state: Option<String>,
}

/// The columns of a `temporary_enrollment_pairings` row.
const PAIRING_COLUMNS: &str = "id, root_account_id, workflow_state, \
     created_at::timestamptz AS created_at, updated_at::timestamptz AS updated_at, \
     created_by_id, deleted_by_id, ending_enrollment_state";

/// The body of a create request.
#[derive(Debug, Default, Deserialize)]
struct PairingRequest {
    workflow_state: Option<String>,
    ending_enrollment_state: Option<String>,
}

/// Domain router for the Canvas temporary-enrollment-pairing routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{account_id}/temporary_enrollment_pairings",
            get(index).post(create),
        )
        .route(
            "/accounts/{account_id}/temporary_enrollment_pairings/new",
            get(new_pairing),
        )
        .route(
            "/accounts/{account_id}/temporary_enrollment_pairings/{id}",
            get(show).delete(destroy),
        )
}

/// `GET /accounts/:account_id/temporary_enrollment_pairings` — pairings#index.
async fn index(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let root_account_id = require_account(&state.db, account_id).await?;
    require_manage(&state.db, &auth, root_account_id).await?;

    let sql = format!(
        "SELECT {PAIRING_COLUMNS} FROM temporary_enrollment_pairings \
         WHERE root_account_id = $1 ORDER BY created_at ASC, id ASC"
    );
    let rows: Vec<Pairing> = sqlx::query_as(&sql)
        .bind(root_account_id)
        .fetch_all(&state.db)
        .await?;

    let items = rows.iter().map(pairing_json).collect::<Vec<Value>>();
    Ok(Json(items).into_response())
}

/// `GET /accounts/:account_id/temporary_enrollment_pairings/:id` — pairings#show.
async fn show(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let root_account_id = require_account(&state.db, account_id).await?;
    require_manage(&state.db, &auth, root_account_id).await?;
    let pairing = load_pairing(&state.db, root_account_id, id).await?;
    Ok(Json(pairing_json(&pairing)).into_response())
}

/// `GET /accounts/:account_id/temporary_enrollment_pairings/new` — pairings#new.
async fn new_pairing(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let root_account_id = require_account(&state.db, account_id).await?;
    require_manage(&state.db, &auth, root_account_id).await?;

    // Canvas answers the unsaved pairing.
    Ok(Json(json!({
        "id": Value::Null,
        "root_account_id": root_account_id,
        "workflow_state": "active",
        "ending_enrollment_state": Value::Null,
        "created_by_id": auth.id,
        "deleted_by_id": Value::Null,
        "created_at": Value::Null,
        "updated_at": Value::Null,
    }))
    .into_response())
}

/// `POST /accounts/:account_id/temporary_enrollment_pairings` — pairings#create.
async fn create(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<PairingRequest>>,
) -> Result<Response, ApiError> {
    let root_account_id = require_account(&state.db, account_id).await?;
    require_manage(&state.db, &auth, root_account_id).await?;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    // Canvas only keeps the "completed" and "inactive" ending states, every
    // other value becomes "deleted".
    let ending_enrollment_state = match body.ending_enrollment_state.as_deref() {
        Some("completed") => "completed",
        Some("inactive") => "inactive",
        _ => "deleted",
    };
    let workflow_state = body
        .workflow_state
        .clone()
        .unwrap_or_else(|| "active".to_string());

    let sql = format!(
        "INSERT INTO temporary_enrollment_pairings \
             (root_account_id, workflow_state, created_at, updated_at, created_by_id, \
              ending_enrollment_state) \
         VALUES ($1, $2, now(), now(), $3, $4) RETURNING {PAIRING_COLUMNS}"
    );
    let pairing: Pairing = sqlx::query_as(&sql)
        .bind(root_account_id)
        .bind(&workflow_state)
        .bind(auth.id)
        .bind(ending_enrollment_state)
        .fetch_one(&state.db)
        .await?;

    Ok((StatusCode::CREATED, Json(pairing_json(&pairing))).into_response())
}

/// `DELETE /accounts/:account_id/temporary_enrollment_pairings/:id` —
/// pairings#destroy.
async fn destroy(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let root_account_id = require_account(&state.db, account_id).await?;
    require_manage(&state.db, &auth, root_account_id).await?;
    let pairing = load_pairing(&state.db, root_account_id, id).await?;

    sqlx::query(
        "UPDATE temporary_enrollment_pairings SET workflow_state = 'deleted', \
             deleted_by_id = $1, updated_at = now() WHERE id = $2",
    )
    .bind(auth.id)
    .bind(pairing.id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// The account of a route has to exist — 404. Canvas answers the root
/// account of the context.
async fn require_account(db: &PgPool, account_id: i64) -> Result<i64, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(db)
            .await?;
    match row {
        Some((None,)) => Ok(account_id),
        Some((Some(parent_id),)) => Ok(parent_id),
        None => Err(ApiError::NotFound),
    }
}

/// The caller has to manage the temporary enrollments of the account.
async fn require_manage(db: &PgPool, auth: &AuthUser, account_id: i64) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM account_users \
         WHERE account_id = $1 AND user_id = $2 AND role <> 'student' LIMIT 1",
    )
    .bind(account_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// The pairing of the root account — 404.
async fn load_pairing(db: &PgPool, root_account_id: i64, id: i64) -> Result<Pairing, ApiError> {
    let sql = format!(
        "SELECT {PAIRING_COLUMNS} FROM temporary_enrollment_pairings \
         WHERE id = $1 AND root_account_id = $2"
    );
    sqlx::query_as(&sql)
        .bind(id)
        .bind(root_account_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// The Canvas `TemporaryEnrollmentPairing` JSON.
fn pairing_json(pairing: &Pairing) -> Value {
    json!({
        "id": pairing.id,
        "root_account_id": pairing.root_account_id,
        "workflow_state": pairing.workflow_state,
        "ending_enrollment_state": pairing.ending_enrollment_state,
        "created_by_id": pairing.created_by_id,
        "deleted_by_id": pairing.deleted_by_id,
        "created_at": pairing.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "updated_at": pairing.updated_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    })
}
