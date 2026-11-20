//! Canvas parity for LTI Data Services subscriptions
//! (`Lti::DataServicesController`).
//!
//! Canvas source: `app/controllers/lti/data_services_controller.rb`.
//!
//! Routes:
//! - `GET    /accounts/{accountId}/data_services`           — list subscriptions
//! - `POST   /accounts/{accountId}/data_services`           — create subscription
//! - `GET    /accounts/{accountId}/data_services/{id}`      — show subscription
//! - `PUT    /accounts/{accountId}/data_services/{id}`      — update subscription
//! - `DELETE /accounts/{accountId}/data_services/{id}`      — destroy subscription

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::can_manage_account;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const STATE_ACTIVE: &str = "active";
const STATE_DELETED: &str = "deleted";

const SUBSCRIPTION_COLUMNS: &str =
    "id, account_id, context_type, context_id, event_types, format, \
     transport_type, transport_metadata, owner_id, owner_type, workflow_state, \
     created_at, updated_at";

// ---------------------------------------------------------------------------
// Domain row
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct SubscriptionRow {
    id: i64,
    account_id: i64,
    context_type: String,
    context_id: String,
    event_types: Value,
    format: String,
    transport_type: String,
    transport_metadata: Value,
    owner_id: Option<String>,
    owner_type: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CreateSubscriptionRequest {
    pub subscription: Option<SubscriptionFields>,
}

#[derive(Debug, Deserialize)]
pub struct SubscriptionFields {
    pub context_id: Option<String>,
    pub context_type: Option<String>,
    pub event_types: Option<Vec<String>>,
    pub format: Option<String>,
    pub transport_metadata: Option<Value>,
    pub transport_type: Option<String>,
    pub owner_id: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSubscriptionRequest {
    pub subscription: Option<SubscriptionFields>,
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

async fn require_account(pool: &PgPool, account_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

async fn find_active_subscription(pool: &PgPool, id: i64) -> Result<SubscriptionRow, ApiError> {
    let row: Option<SubscriptionRow> = sqlx::query_as(&format!(
        "SELECT {SUBSCRIPTION_COLUMNS} FROM lti_data_service_subscriptions \
         WHERE id = $1 AND workflow_state <> '{STATE_DELETED}'"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

async fn find_subscription_in_account(
    pool: &PgPool,
    id: i64,
    account_id: i64,
) -> Result<SubscriptionRow, ApiError> {
    let sub = find_active_subscription(pool, id).await?;
    if sub.account_id != account_id {
        return Err(ApiError::NotFound);
    }
    Ok(sub)
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

fn subscription_json(sub: &SubscriptionRow) -> Value {
    json!({
        "id": sub.id,
        "ContextId": sub.context_id,
        "ContextType": sub.context_type,
        "EventTypes": sub.event_types,
        "Format": sub.format,
        "TransportMetadata": sub.transport_metadata,
        "TransportType": sub.transport_type,
        "OwnerId": sub.owner_id,
        "OwnerType": sub.owner_type,
        "workflow_state": sub.workflow_state,
        "created_at": sub.created_at,
        "updated_at": sub.updated_at,
    })
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{account_id}/data_services",
            get(list_subscriptions).post(create_subscription),
        )
        .route(
            "/accounts/{account_id}/data_services/{id}",
            get(show_subscription)
                .put(update_subscription)
                .delete(destroy_subscription),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_subscriptions(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "SELECT {SUBSCRIPTION_COLUMNS} FROM lti_data_service_subscriptions \
         WHERE account_id = $1 AND workflow_state <> '{STATE_DELETED}' \
         ORDER BY created_at DESC"
    );
    let rows: Vec<SubscriptionRow> = sqlx::query_as(&sql)
        .bind(account_id)
        .fetch_all(&state.db)
        .await?;

    let data: Vec<Value> = rows.iter().map(subscription_json).collect();
    Ok(Json(data))
}

async fn create_subscription(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateSubscriptionRequest>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let subscription = payload
        .subscription
        .ok_or_else(|| ApiError::BadRequest("subscription object is required".to_string()))?;

    let context_id = subscription
        .context_id
        .ok_or_else(|| ApiError::BadRequest("subscription.ContextId is required".to_string()))?;
    let context_type = subscription
        .context_type
        .ok_or_else(|| ApiError::BadRequest("subscription.ContextType is required".to_string()))?;

    let event_types = subscription
        .event_types
        .ok_or_else(|| ApiError::BadRequest("subscription.EventTypes is required".to_string()))?;
    let format = subscription
        .format
        .ok_or_else(|| ApiError::BadRequest("subscription.Format is required".to_string()))?;
    let transport_type = subscription.transport_type.ok_or_else(|| {
        ApiError::BadRequest("subscription.TransportType is required".to_string())
    })?;
    let transport_metadata = subscription.transport_metadata.ok_or_else(|| {
        ApiError::BadRequest("subscription.TransportMetadata is required".to_string())
    })?;

    let workflow_state = subscription
        .state
        .map(|s| match s.as_str() {
            "Deleted" => STATE_DELETED,
            _ => STATE_ACTIVE,
        })
        .unwrap_or(STATE_ACTIVE);

    let owner_id = subscription.owner_id.unwrap_or_else(|| auth.id.to_string());
    let owner_type = "person".to_string();

    let sql = format!(
        "INSERT INTO lti_data_service_subscriptions \
         (account_id, context_type, context_id, event_types, format, transport_type, \
          transport_metadata, owner_id, owner_type, workflow_state) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         RETURNING {SUBSCRIPTION_COLUMNS}"
    );
    let row: SubscriptionRow = sqlx::query_as(&sql)
        .bind(account_id)
        .bind(&context_type)
        .bind(&context_id)
        .bind(serde_json::json!(event_types))
        .bind(&format)
        .bind(&transport_type)
        .bind(&transport_metadata)
        .bind(&owner_id)
        .bind(&owner_type)
        .bind(workflow_state)
        .fetch_one(&state.db)
        .await?;

    Ok((StatusCode::CREATED, Json(subscription_json(&row))).into_response())
}

async fn show_subscription(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let sub = find_subscription_in_account(&state.db, id, account_id).await?;
    Ok(Json(subscription_json(&sub)))
}

async fn update_subscription(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateSubscriptionRequest>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let existing = find_subscription_in_account(&state.db, id, account_id).await?;

    let subscription = payload.subscription.as_ref();

    let context_type = subscription
        .and_then(|s| s.context_type.clone())
        .unwrap_or(existing.context_type);
    let context_id = subscription
        .and_then(|s| s.context_id.clone())
        .unwrap_or(existing.context_id);
    let event_types = subscription
        .and_then(|s| s.event_types.clone())
        .unwrap_or_else(|| {
            serde_json::from_value::<Vec<String>>(existing.event_types.clone()).unwrap_or_default()
        });
    let format = subscription
        .and_then(|s| s.format.clone())
        .unwrap_or(existing.format);
    let transport_type = subscription
        .and_then(|s| s.transport_type.clone())
        .unwrap_or(existing.transport_type);
    let transport_metadata = subscription
        .and_then(|s| s.transport_metadata.clone())
        .unwrap_or_else(|| existing.transport_metadata.clone());
    let workflow_state = subscription
        .and_then(|s| s.state.clone())
        .map(|s| match s.as_str() {
            "Deleted" => STATE_DELETED.to_string(),
            _ => STATE_ACTIVE.to_string(),
        })
        .unwrap_or(existing.workflow_state);

    let sql = format!(
        "UPDATE lti_data_service_subscriptions \
         SET context_type = $2, context_id = $3, event_types = $4, format = $5, \
             transport_type = $6, transport_metadata = $7, workflow_state = $8, \
             updated_at = now() \
         WHERE id = $1 AND workflow_state <> '{STATE_DELETED}' \
         RETURNING {SUBSCRIPTION_COLUMNS}"
    );
    let row: SubscriptionRow = sqlx::query_as(&sql)
        .bind(id)
        .bind(&context_type)
        .bind(&context_id)
        .bind(&event_types)
        .bind(&format)
        .bind(&transport_type)
        .bind(&transport_metadata)
        .bind(&workflow_state)
        .fetch_one(&state.db)
        .await?;

    Ok(Json(subscription_json(&row)))
}

async fn destroy_subscription(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _sub = find_subscription_in_account(&state.db, id, account_id).await?;

    let sql = format!(
        "UPDATE lti_data_service_subscriptions \
         SET workflow_state = '{STATE_DELETED}', updated_at = now() \
         WHERE id = $1 AND workflow_state <> '{STATE_DELETED}' \
         RETURNING {SUBSCRIPTION_COLUMNS}"
    );
    let row: Option<SubscriptionRow> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?;

    match row {
        Some(r) => Ok(Json(subscription_json(&r))),
        None => Err(ApiError::NotFound),
    }
}
