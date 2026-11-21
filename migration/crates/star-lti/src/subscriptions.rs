//! lti/subscriptions — Canvas `Lti::SubscriptionsApiController` parity.
//!
//! Routes (the LTI Advantage router mounts this module at the app root, so
//! the paths are `/api/lti/...`):
//! - `POST   /api/lti/subscriptions` — create a webhook subscription
//! - `GET    /api/lti/subscriptions` — list the webhook subscriptions
//! - `GET    /api/lti/subscriptions/{id}` — show one subscription
//! - `PUT    /api/lti/subscriptions/{id}` — update one subscription
//! - `DELETE /api/lti/subscriptions/{id}` — delete one subscription
//!
//! Canvas routes (config/routes.rb, the LTI API block):
//!   post   "subscriptions", action: :create
//!   delete "subscriptions/:id", action: :destroy
//!   get    "subscriptions/:id", action: :show
//!   put    "subscriptions/:id", action: :update
//!   get    "subscriptions", action: :index
//!
//! Canvas controller (`app/controllers/lti/subscriptions_api_controller.rb`):
//! the `before_action` chain is `authorized_lti2_tool` (an LTI 2.0 access
//! token that names the tool proxy) and `verify_service_configured` (500
//! `{"error": "Subscription service not configured"}` when the live-events
//! subscription service is not reachable).
//!
//! The controller forwards the request to `Services::LiveEventsSubscriptionService`
//! and answers its body and status verbatim. The validator
//! (`app/controllers/lti/subscriptions_validator.rb`) answers 400
//! `{"error": "Invalid subscription"}` for a context type outside the
//! whitelist ("root_account" and "assignment") and 401
//! `{"error": "Unauthorized subscription"}` when the tool proxy lacks the
//! capability of an event type or is not installed in the requested context.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_common::AppState;

/// The scope the LTI 2.0 access token needs for the subscription service.
const SUBSCRIPTION_SCOPE: &str = "https://purl.imsglobal.org/spec/lti-ags/scope/subscription";

/// Canvas `Lti::SubscriptionsValidator::CONTEXT_WHITELIST`.
const CONTEXT_WHITELIST: [&str; 2] = ["root_account", "assignment"];

/// The subscription fields of the request body.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SubscriptionFields {
    context_id: Option<String>,
    context_type: Option<String>,
    event_types: Option<Vec<String>>,
    format: Option<String>,
    transport_metadata: Option<Value>,
    transport_type: Option<String>,
    id: Option<String>,
}

/// The request body of the create and update routes.
#[derive(Debug, Default, Deserialize)]
struct SubscriptionRequest {
    subscription: Option<SubscriptionFields>,
}

/// The domain router for the Canvas subscription routes.
///
/// The LTI API block of Canvas is mounted at the app root, so the paths
/// carry the literal `/api/lti` prefix.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/lti/subscriptions", get(index).post(create))
        .route(
            "/api/lti/subscriptions/{id}",
            get(show).put(update).delete(destroy),
        )
}

/// Canvas `Lti::SubscriptionsApiController#create`.
async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<SubscriptionRequest>>,
) -> Result<Response, Response> {
    let proxy_id = authenticate(&state, &headers).await?;
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let subscription = body.subscription.unwrap_or_default();

    validate_context_type(subscription.context_type.as_deref())?;
    validate_capabilities(&state.db, proxy_id, subscription.event_types.as_deref()).await?;
    validate_tool_context(&state.db, proxy_id, &subscription).await?;

    let row = insert_subscription(&state.db, proxy_id, &subscription).await?;
    Ok(Json(subscription_json(&row)).into_response())
}

/// Canvas `Lti::SubscriptionsApiController#index`.
async fn index(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Response> {
    let proxy_id = authenticate(&state, &headers).await?;
    let rows = list_subscriptions(&state.db, proxy_id).await?;
    let subscriptions = rows.iter().map(subscription_json).collect::<Vec<Value>>();
    Ok(Json(subscriptions).into_response())
}

/// Canvas `Lti::SubscriptionsApiController#show`.
async fn show(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let proxy_id = authenticate(&state, &headers).await?;
    let row = find_subscription(&state.db, proxy_id, &id).await?;
    let Some(row) = row else {
        return Err(not_found());
    };
    Ok(Json(subscription_json(&row)).into_response())
}

/// Canvas `Lti::SubscriptionsApiController#update`.
async fn update(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<SubscriptionRequest>>,
) -> Result<Response, Response> {
    let proxy_id = authenticate(&state, &headers).await?;
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let mut subscription = body.subscription.unwrap_or_default();
    subscription.id = Some(id.clone());

    validate_context_type(subscription.context_type.as_deref())?;
    validate_capabilities(&state.db, proxy_id, subscription.event_types.as_deref()).await?;
    validate_tool_context(&state.db, proxy_id, &subscription).await?;

    let row = update_subscription(&state.db, proxy_id, &id, &subscription).await?;
    let Some(row) = row else {
        return Err(not_found());
    };
    Ok(Json(subscription_json(&row)).into_response())
}

/// Canvas `Lti::SubscriptionsApiController#destroy`.
async fn destroy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, Response> {
    let proxy_id = authenticate(&state, &headers).await?;
    let row = find_subscription(&state.db, proxy_id, &id).await?;
    let Some(row) = row else {
        return Err(not_found());
    };
    sqlx::query("DELETE FROM lti_data_service_subscriptions WHERE id = $1")
        .bind(row.id)
        .execute(&state.db)
        .await
        .map_err(server_error)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Errors — Canvas renders the LTI errors with their own body
// ---------------------------------------------------------------------------

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized" })),
    )
        .into_response()
}

fn invalid_subscription() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "Invalid subscription" })),
    )
        .into_response()
}

fn unauthorized_subscription() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "Unauthorized subscription" })),
    )
        .into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response()
}

fn server_error(error: sqlx::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": error.to_string() })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Authentication — Canvas `authorized_lti2_tool`
// ---------------------------------------------------------------------------

/// The tool proxy of an LTI 2.0 access token.
///
/// The token is a JWT signed with the platform secret; its `sub` claim names
/// the guid of the tool proxy and its `scopes` claim carries the
/// permissions.
async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<i64, Response> {
    let Some(value) = headers.get(header::AUTHORIZATION) else {
        return Err(unauthorized());
    };
    let Ok(value) = value.to_str() else {
        return Err(unauthorized());
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(unauthorized());
    };

    let decoded = jsonwebtoken::decode::<Value>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(state.jwt_secret.as_bytes()),
        &jsonwebtoken::Validation::default(),
    )
    .map_err(|_| unauthorized())?;
    let claims = decoded.claims;

    let scopes = claims
        .get("scopes")
        .and_then(Value::as_str)
        .map(|scopes| {
            scopes
                .split_whitespace()
                .map(|scope| scope.to_string())
                .collect::<Vec<String>>()
        })
        .ok_or_else(unauthorized)?;

    if !scopes.iter().any(|scope| scope == SUBSCRIPTION_SCOPE) {
        return Err(unauthorized());
    }

    // The `sub` of the token names the tool proxy.
    let sub = claims
        .get("sub")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let proxy = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM lti_tool_proxies WHERE guid = $1 AND workflow_state = 'active'",
    )
    .bind(sub)
    .fetch_optional(&state.db)
    .await
    .map_err(server_error)?;

    // Canvas: the token has to name an active tool proxy.
    proxy.ok_or_else(unauthorized)
}

// ---------------------------------------------------------------------------
// Validation — Canvas `Lti::SubscriptionsValidator`
// ---------------------------------------------------------------------------

/// Canvas `Lti::SubscriptionsValidator#check_required_capabilities!`.
async fn validate_capabilities(
    db: &PgPool,
    proxy_id: i64,
    event_types: Option<&[String]>,
) -> Result<(), Response> {
    let Some(event_types) = event_types else {
        return Ok(());
    };
    if event_types.is_empty() {
        return Ok(());
    }
    let row: Option<(Value,)> =
        sqlx::query_as("SELECT raw_data FROM lti_tool_proxies WHERE id = $1")
            .bind(proxy_id)
            .fetch_optional(db)
            .await
            .map_err(server_error)?;
    let Some((raw_data,)) = row else {
        return Err(unauthorized_subscription());
    };
    let capabilities = raw_data
        .get("enabled_capabilities")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(|value| value.to_string()))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    // Canvas allows the tool proxy to subscribe to everything with this
    // capability.
    if capabilities.iter().any(|capability| {
        capability == "https://purl.imsglobal.org/spec/lti-ags/scope/webhook_subscription.all"
    }) {
        return Ok(());
    }
    // Every event type needs a capability of its own.
    for event_type in event_types {
        if !capabilities
            .iter()
            .any(|capability| capability.contains(event_type.as_str()))
        {
            return Err(unauthorized_subscription());
        }
    }
    Ok(())
}

/// Canvas `Lti::SubscriptionsValidator#check_tool_context!`.
async fn validate_tool_context(
    db: &PgPool,
    proxy_id: i64,
    subscription: &SubscriptionFields,
) -> Result<(), Response> {
    let context_type = subscription.context_type.as_deref().unwrap_or_default();
    let context_id = subscription.context_id.as_deref().unwrap_or_default();
    if context_id.is_empty() {
        return Err(invalid_subscription());
    }
    let context_id = context_id.parse::<i64>().unwrap_or_default();

    // The context of the subscription: an account or an assignment.
    let context_course_id = match context_type {
        "root_account" => {
            let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
                .bind(context_id)
                .fetch_optional(db)
                .await
                .map_err(server_error)?;
            if row.is_none() {
                return Err(invalid_subscription());
            }
            None
        }
        _ => {
            let row: Option<(i64,)> =
                sqlx::query_as("SELECT course_id FROM assignments WHERE id = $1")
                    .bind(context_id)
                    .fetch_optional(db)
                    .await
                    .map_err(server_error)?;
            let Some((course_id,)) = row else {
                return Err(invalid_subscription());
            };
            Some(course_id)
        }
    };

    // The tool proxy has to be active in the context of the subscription.
    let active: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM lti_tool_proxies \
         WHERE id = $1 AND workflow_state = 'active' \
           AND (context_id = $2 OR context_id = $3) LIMIT 1",
    )
    .bind(proxy_id)
    .bind(context_id)
    .bind(context_course_id.unwrap_or_default())
    .fetch_optional(db)
    .await
    .map_err(server_error)?;
    if active.is_none() {
        return Err(unauthorized_subscription());
    }
    Ok(())
}

/// Canvas `Lti::SubscriptionsValidator.validate_subscription_context!`.
fn validate_context_type(context_type: Option<&str>) -> Result<(), Response> {
    let Some(context_type) = context_type else {
        return Ok(());
    };
    if CONTEXT_WHITELIST.contains(&context_type) {
        Ok(())
    } else {
        Err(invalid_subscription())
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// A stored subscription.
#[derive(Debug, sqlx::FromRow)]
struct SubscriptionRow {
    id: i64,
    context_type: String,
    context_id: String,
    event_types: Value,
    format: String,
    transport_type: String,
    transport_metadata: Value,
    owner_id: Option<String>,
    owner_type: String,
    workflow_state: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// The columns of a stored subscription.
const SUBSCRIPTION_COLUMNS: &str = "id, context_type, context_id, event_types, format, \
     transport_type, transport_metadata, owner_id, owner_type, workflow_state, \
     created_at, updated_at";

/// The stored subscriptions of a tool proxy.
async fn list_subscriptions(db: &PgPool, proxy_id: i64) -> Result<Vec<SubscriptionRow>, Response> {
    let sql = format!(
        "SELECT {SUBSCRIPTION_COLUMNS} FROM lti_data_service_subscriptions \
         WHERE owner_id = $1 AND owner_type = 'tool_proxy' AND workflow_state = 'active' \
         ORDER BY id ASC"
    );
    sqlx::query_as(&sql)
        .bind(proxy_id.to_string())
        .fetch_all(db)
        .await
        .map_err(server_error)
}

/// One stored subscription of a tool proxy.
async fn find_subscription(
    db: &PgPool,
    proxy_id: i64,
    id: &str,
) -> Result<Option<SubscriptionRow>, Response> {
    let sql = format!(
        "SELECT {SUBSCRIPTION_COLUMNS} FROM lti_data_service_subscriptions \
         WHERE id = $1 AND owner_id = $2 AND owner_type = 'tool_proxy' LIMIT 1"
    );
    sqlx::query_as(&sql)
        .bind(id.parse::<i64>().unwrap_or_default())
        .bind(proxy_id.to_string())
        .fetch_optional(db)
        .await
        .map_err(server_error)
}

/// Store a subscription of a tool proxy.
async fn insert_subscription(
    db: &PgPool,
    proxy_id: i64,
    subscription: &SubscriptionFields,
) -> Result<SubscriptionRow, Response> {
    let context_id = subscription.context_id.clone().unwrap_or_default();
    let context_type = subscription.context_type.clone().unwrap_or_default();
    let event_types = json!(subscription.event_types.clone().unwrap_or_default());
    let format = subscription.format.clone().unwrap_or_default();
    let transport_type = subscription.transport_type.clone().unwrap_or_default();
    let transport_metadata = subscription
        .transport_metadata
        .clone()
        .unwrap_or_else(|| json!({}));

    let sql = format!(
        "INSERT INTO lti_data_service_subscriptions \
             (account_id, context_type, context_id, event_types, format, transport_type, \
              transport_metadata, owner_id, owner_type, workflow_state) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'tool_proxy', 'active') \
         RETURNING {SUBSCRIPTION_COLUMNS}"
    );
    // The account of the tool proxy carries the subscription.
    let account_id = tool_proxy_account(db, proxy_id).await?;
    sqlx::query_as(&sql)
        .bind(account_id)
        .bind(&context_type)
        .bind(&context_id)
        .bind(&event_types)
        .bind(&format)
        .bind(&transport_type)
        .bind(&transport_metadata)
        .bind(proxy_id.to_string())
        .fetch_one(db)
        .await
        .map_err(server_error)
}

/// Update a subscription of a tool proxy.
async fn update_subscription(
    db: &PgPool,
    proxy_id: i64,
    id: &str,
    subscription: &SubscriptionFields,
) -> Result<Option<SubscriptionRow>, Response> {
    let event_types = subscription.event_types.clone();
    let format = subscription.format.clone();
    let transport_type = subscription.transport_type.clone();
    let transport_metadata = subscription.transport_metadata.clone();

    let sql = format!(
        "UPDATE lti_data_service_subscriptions SET \
             event_types = COALESCE($1, event_types), \
             format = COALESCE($2, format), \
             transport_type = COALESCE($3, transport_type), \
             transport_metadata = COALESCE($4, transport_metadata), \
             updated_at = now() \
         WHERE id = $5 AND owner_id = $6 AND owner_type = 'tool_proxy' \
         RETURNING {SUBSCRIPTION_COLUMNS}"
    );
    sqlx::query_as(&sql)
        .bind(event_types.map(|values| json!(values)))
        .bind(format)
        .bind(transport_type)
        .bind(transport_metadata)
        .bind(id.parse::<i64>().unwrap_or_default())
        .bind(proxy_id.to_string())
        .fetch_optional(db)
        .await
        .map_err(server_error)
}

/// The account a tool proxy is installed in.
async fn tool_proxy_account(db: &PgPool, proxy_id: i64) -> Result<i64, Response> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT context_id FROM lti_tool_proxies \
         WHERE id = $1 AND context_type = 'Account' LIMIT 1",
    )
    .bind(proxy_id)
    .fetch_optional(db)
    .await
    .map_err(server_error)?;
    match row {
        Some((account_id,)) => Ok(account_id),
        None => {
            // A course-scoped tool proxy uses the account of the course.
            let course: Option<(i64,)> = sqlx::query_as(
                "SELECT c.account_id FROM lti_tool_proxies tp \
                 JOIN courses c ON c.id = tp.context_id \
                 WHERE tp.id = $1 AND tp.context_type = 'Course' LIMIT 1",
            )
            .bind(proxy_id)
            .fetch_optional(db)
            .await
            .map_err(server_error)?;
            course.map(|row| row.0).ok_or_else(unauthorized)
        }
    }
}

/// The JSON body of a subscription.
fn subscription_json(row: &SubscriptionRow) -> Value {
    json!({
        "Id": row.id.to_string(),
        "ContextId": row.context_id,
        "ContextType": row.context_type,
        "EventTypes": row.event_types,
        "Format": row.format,
        "TransportMetadata": row.transport_metadata,
        "TransportType": row.transport_type,
        "OwnerId": row.owner_id,
        "OwnerType": row.owner_type,
        "WorkflowState": row.workflow_state,
        "CreatedAt": row.created_at.to_rfc3339(),
        "UpdatedAt": row.updated_at.to_rfc3339(),
    })
}
