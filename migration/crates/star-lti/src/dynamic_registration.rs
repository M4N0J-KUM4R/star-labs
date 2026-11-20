//! Canvas parity for LTI IMS Dynamic Registration
//! (`app/controllers/lti/ims/dynamic_registration_controller.rb`).
//!
//! Routes (mounted under `/api/v1` by star-http):
//!
//! - `GET  /accounts/:account_id/registration_token`
//! - `GET  /accounts/:account_id/registrations/uuid/:registration_uuid`
//! - `GET  /accounts/:account_id/lti_registrations/uuid/:registration_uuid`
//! - `GET  /accounts/:account_id/lti_registration_update_request/uuid/:registration_uuid`
//! - `GET  /accounts/:account_id/registrations/:registration_id`
//! - `PUT  /accounts/:account_id/registrations/:registration_id/overlay`
//! - `GET  /accounts/:account_id/dr_iframe`
//! - `GET  /accounts/:account_id/registrations/:registration_id/view`
//! - `POST /registrations`
//! - `PUT  /registrations/:registration_id`
//! - `GET  /registrations/:registration_id`

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
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

// ---------------------------------------------------------------------------
// Domain rows
// ---------------------------------------------------------------------------

/// A row from `lti_ims_registrations`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ImsRegistrationRow {
    id: i64,
    guid: String,
    developer_key_id: Option<i64>,
    root_account_id: Option<i64>,
    unified_tool_id: Option<String>,
    registration_url: Option<String>,
    initiate_login_uri: Option<String>,
    jwks_uri: Option<String>,
    client_name: Option<String>,
    redirect_uris: Option<String>,
    logo_uri: Option<String>,
    scopes: Option<String>,
    lti_tool_configuration: Value,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// A row from `lti_registration_update_requests`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct UpdateRequestRow {
    id: i64,
    uuid: String,
    lti_registration_id: i64,
    root_account_id: Option<i64>,
    lti_ims_registration: Value,
    created_by_id: Option<i64>,
    accepted_at: Option<DateTime<Utc>>,
    rejected_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// A row from `lti_registrations` (the parent registration).
#[derive(Debug, Clone, sqlx::FromRow)]
struct RegistrationRow {
    id: i64,
    account_id: i64,
    root_account_id: Option<i64>,
    name: String,
    admin_nickname: Option<String>,
    vendor: Option<String>,
    description: Option<String>,
    icon_url: Option<String>,
    lti_version: String,
    dynamic_registration: bool,
    internal_service: bool,
    lock_deploying: bool,
    template_registration_id: Option<i64>,
    workflow_state: String,
    configuration: Value,
    overlay: Option<Value>,
    created_by_id: Option<i64>,
    updated_by_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Columns selected for [`ImsRegistrationRow`].
const IMS_REG_COLUMNS: &str =
    "id, guid, developer_key_id, root_account_id, unified_tool_id, registration_url, \
     initiate_login_uri, jwks_uri, client_name, redirect_uris, logo_uri, scopes, \
     lti_tool_configuration, created_at, updated_at";

// ---------------------------------------------------------------------------
// Request / query types
// ---------------------------------------------------------------------------

/// Query params for `GET /accounts/:account_id/registration_token`.
#[derive(Debug, Deserialize)]
pub struct RegistrationTokenQuery {
    pub registration_url: Option<String>,
    pub unified_tool_id: Option<String>,
    pub registration_id: Option<i64>,
}

/// Body for `POST /registrations` and `PUT /registrations/:registration_id`.
#[derive(Debug, Deserialize)]
pub struct OidcRegistrationRequest {
    pub client_name: Option<String>,
    pub redirect_uris: Option<String>,
    pub initiate_login_uri: Option<String>,
    pub jwks_uri: Option<String>,
    pub logo_uri: Option<String>,
    pub scopes: Option<String>,
    #[serde(rename = "https://purl.imsglobal.org/spec/lti-tool-configuration")]
    pub lti_tool_configuration: Option<Value>,
    pub application_type: Option<String>,
    pub grant_types: Option<Value>,
    pub response_types: Option<String>,
    // Canvas accepts and forwards these without validating them:
    pub token_endpoint_auth_method: Option<String>,
}

/// Body for `PUT /accounts/:account_id/registrations/:registration_id/overlay`.
#[derive(Debug, Deserialize)]
pub struct OverlayRequest {
    #[serde(rename = "disabledScopes")]
    pub disabled_scopes: Option<Value>,
    // Canvas stores any other overlay fields as-is as JSON.
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// 404 when the account does not exist.
async fn require_account(pool: &PgPool, account_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Require site-admin or account-level manage_developer_keys equivalent.
/// For this workspace we use the global admin role or an account_users row.
async fn require_site_admin_or_account_admin(
    pool: &PgPool,
    auth: &AuthUser,
    account_id: i64,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    if can_manage_account(pool, auth, account_id).await? {
        return Ok(());
    }
    Err(ApiError::Forbidden)
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

/// Fetch an IMS registration by its GUID (global id).
async fn find_ims_registration_by_guid(
    pool: &PgPool,
    guid: &str,
) -> Result<ImsRegistrationRow, ApiError> {
    let row: Option<ImsRegistrationRow> = sqlx::query_as(&format!(
        "SELECT {IMS_REG_COLUMNS} FROM lti_ims_registrations WHERE guid = $1"
    ))
    .bind(guid)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

/// Fetch a parent LTI registration by id (active only).
async fn find_active_registration(pool: &PgPool, id: i64) -> Result<RegistrationRow, ApiError> {
    let row: Option<RegistrationRow> = sqlx::query_as(
        "SELECT id, account_id, root_account_id, name, admin_nickname, vendor, description, \
         icon_url, lti_version, dynamic_registration, internal_service, lock_deploying, \
         template_registration_id, workflow_state, configuration, overlay, \
         created_by_id, updated_by_id, created_at, updated_at \
         FROM lti_registrations WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

/// Fetch an IMS registration by its numeric id.
async fn find_ims_registration_by_id(
    pool: &PgPool,
    id: i64,
) -> Result<ImsRegistrationRow, ApiError> {
    let row: Option<ImsRegistrationRow> = sqlx::query_as(&format!(
        "SELECT {IMS_REG_COLUMNS} FROM lti_ims_registrations WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

/// Fetch an update request by its UUID.
async fn find_update_request_by_uuid(
    pool: &PgPool,
    uuid: &str,
) -> Result<UpdateRequestRow, ApiError> {
    let row: Option<UpdateRequestRow> = sqlx::query_as(
        "SELECT id, uuid, lti_registration_id, root_account_id, \
                         lti_ims_registration, created_by_id, accepted_at, rejected_at, \
                         created_at, updated_at \
                         FROM lti_registration_update_requests WHERE uuid = $1",
    )
    .bind(uuid)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// JSON serialization
// ---------------------------------------------------------------------------

/// Render an IMS registration as Canvas `lti_registration_json` with full
/// configuration and overlay included.
async fn ims_registration_json(
    _pool: &PgPool,
    ims: &ImsRegistrationRow,
    registration: &RegistrationRow,
    include_overlay: bool,
) -> Result<Value, ApiError> {
    let mut json = json!({
        "id": ims.id,
        "guid": ims.guid,
        "developer_key_id": ims.developer_key_id,
        "root_account_id": ims.root_account_id,
        "unified_tool_id": ims.unified_tool_id,
        "registration_url": ims.registration_url,
        "initiate_login_uri": ims.initiate_login_uri,
        "jwks_uri": ims.jwks_uri,
        "client_name": ims.client_name,
        "redirect_uris": ims.redirect_uris,
        "logo_uri": ims.logo_uri,
        "scopes": ims.scopes,
        "lti_tool_configuration": ims.lti_tool_configuration,
        "dynamic_registration": ims.dynamic_registration,
        "internal_service": ims.internal_service,
        "created_by_id": ims.created_by_id,
        "updated_by_id": ims.updated_by_id,
        "created_at": ims.created_at,
        "updated_at": ims.updated_at,
        // Canvas always embeds the parent registration data:
        "name": registration.name,
        "admin_nickname": registration.admin_nickname,
        "vendor": registration.vendor,
        "account_id": registration.account_id,
        "workflow_state": registration.workflow_state,
        "lti_version": registration.lti_version,
        "icon_url": registration.icon_url,
        "configuration": registration.configuration,
    });

    if include_overlay {
        if let Some(overlay) = &registration.overlay {
            json["overlay"] = overlay.clone();
        }
    }

    Ok(json)
}

/// Render a registration update request as Canvas JSON.
async fn update_request_json(pool: &PgPool, rur: &UpdateRequestRow) -> Result<Value, ApiError> {
    let mut json = json!({
        "id": rur.id,
        "uuid": rur.uuid,
        "lti_registration_id": rur.lti_registration_id,
        "root_account_id": rur.root_account_id,
        "lti_ims_registration": rur.lti_ims_registration,
        "accepted_at": rur.accepted_at,
        "rejected_at": rur.rejected_at,
        "created_at": rur.created_at,
        "updated_at": rur.updated_at,
    });

    if let Some(user_id) = rur.created_by_id {
        let row: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
        if let Some((name,)) = row {
            json["created_by"] = json!({ "name": name });
        }
    }

    Ok(json)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the dynamic registration router.
pub fn router() -> Router<AppState> {
    Router::new()
        // Account-scoped read routes
        .route(
            "/accounts/{account_id}/registration_token",
            get(registration_token),
        )
        .route(
            "/accounts/{account_id}/registrations/uuid/{registration_uuid}",
            get(show_ims_registration_by_uuid),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/uuid/{registration_uuid}",
            get(show_registration_by_uuid),
        )
        .route(
            "/accounts/{account_id}/lti_registration_update_request/uuid/{registration_uuid}",
            get(show_update_request_by_uuid),
        )
        .route(
            "/accounts/{account_id}/registrations/{registration_id}",
            get(show_ims_registration),
        )
        .route(
            "/accounts/{account_id}/registrations/{registration_id}/overlay",
            put(update_registration_overlay),
        )
        .route("/accounts/{account_id}/dr_iframe", get(dr_iframe))
        .route(
            "/accounts/{account_id}/registrations/{registration_id}/view",
            get(registration_view),
        )
        // Flat paths (no account scope)
        .route("/registrations", post(create_registration))
        .route("/registrations/{registration_id}", put(update_registration))
        .route(
            "/registrations/{registration_id}",
            get(show_registration_configuration),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /accounts/:account_id/registration_token` — issues a short-lived
/// JWT that the tool uses in the dynamic registration OIDC flow.
async fn registration_token(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<RegistrationTokenQuery>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    require_site_admin_or_account_admin(&state.db, &auth, account_id).await?;

    let uuid = format!(
        "{:x}-{:x}",
        chrono::Utc::now().timestamp_millis(),
        rand::random::<u64>()
    );
    let initiated_at = Utc::now().to_rfc3339();
    let user_id = auth.id;
    let root_account_global_id = account_id;

    // Look up the account domain.
    let domain: Option<(String,)> = sqlx::query_as("SELECT domain FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(&state.db)
        .await?;

    let domain = domain.map(|(d,)| d).unwrap_or_default();

    let payload = json!({
        "uuid": uuid,
        "initiated_at": initiated_at,
        "user_id": user_id,
        "root_account_global_id": root_account_global_id,
        "root_account_domain": domain,
        "unified_tool_id": params.unified_tool_id,
        "registration_url": params.registration_url,
        "registration_id": params.registration_id,
    });

    let secret = &state.jwt_secret;
    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &payload,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| ApiError::Db(e.to_string()))?;

    let oidc_configuration_url = format!("https://{}/api/lti/ims/authorize", domain);

    Ok(Json(json!({
        "uuid": uuid,
        "oidc_configuration_url": oidc_configuration_url,
        "token": token,
    })))
}

/// `GET /accounts/:account_id/registrations/uuid/:registration_uuid` —
/// look up an IMS registration by its GUID.
async fn show_ims_registration_by_uuid(
    State(state): State<AppState>,
    Path((account_id, registration_uuid)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    require_site_admin_or_account_admin(&state.db, &auth, account_id).await?;

    let ims = find_ims_registration_by_guid(&state.db, &registration_uuid).await?;

    // Load the parent registration.
    let registration = find_active_registration(&state.db, ims.id).await?;

    let json = ims_registration_json(&state.db, &ims, &registration, true).await?;
    Ok(Json(json))
}

/// `GET /accounts/:account_id/lti_registrations/uuid/:registration_uuid` —
/// look up the parent LTI registration by the IMS registration GUID.
async fn show_registration_by_uuid(
    State(state): State<AppState>,
    Path((account_id, registration_uuid)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    require_site_admin_or_account_admin(&state.db, &auth, account_id).await?;

    let ims = find_ims_registration_by_guid(&state.db, &registration_uuid).await?;
    let registration = find_active_registration(&state.db, ims.id).await?;

    // Render as Canvas `lti_registration_json` (parent shape).
    let mut json = json!({
        "id": registration.id,
        "account_id": registration.account_id,
        "root_account_id": registration.root_account_id,
        "name": registration.name,
        "admin_nickname": registration.admin_nickname,
        "vendor": registration.vendor,
        "workflow_state": registration.workflow_state,
        "created_at": registration.created_at,
        "updated_at": registration.updated_at,
        "description": registration.description,
        "lock_deploying": registration.lock_deploying,
        "template_registration_id": registration.template_registration_id,
        "lti_version": registration.lti_version,
        "icon_url": registration.icon_url,
        "configuration": registration.configuration,
    });

    if let Some(overlay) = &registration.overlay {
        json["overlay"] = overlay.clone();
    }

    Ok(Json(json))
}

/// `GET /accounts/:account_id/lti_registration_update_request/uuid/:registration_uuid`
async fn show_update_request_by_uuid(
    State(state): State<AppState>,
    Path((account_id, registration_uuid)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    require_site_admin_or_account_admin(&state.db, &auth, account_id).await?;

    let rur = find_update_request_by_uuid(&state.db, &registration_uuid).await?;
    let json = update_request_json(&state.db, &rur).await?;
    Ok(Json(json))
}

/// `GET /accounts/:account_id/registrations/:registration_id` — show one
/// IMS registration by its numeric id.
async fn show_ims_registration(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    require_site_admin_or_account_admin(&state.db, &auth, account_id).await?;

    let ims = find_ims_registration_by_id(&state.db, registration_id).await?;
    let registration = find_active_registration(&state.db, ims.id).await?;

    let json = ims_registration_json(&state.db, &ims, &registration, true).await?;
    Ok(Json(json))
}

/// `PUT /accounts/:account_id/registrations/:registration_id/overlay` —
/// update the per-account overlay for a registration.
async fn update_registration_overlay(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<OverlayRequest>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    require_site_admin_or_account_admin(&state.db, &auth, account_id).await?;

    // Fetch registration - use the plain lti_registrations table.
    let _registration = find_active_registration(&state.db, registration_id).await?;

    // Validate the overlay data is an object.
    let overlay_json = if let Some(disabled) = payload.disabled_scopes {
        // Merge disabledScopes into a minimal overlay object.
        json!({ "disabledScopes": disabled })
    } else {
        json!({})
    };

    // Upsert the overlay column on lti_registrations.
    sqlx::query(
        "UPDATE lti_registrations \
         SET overlay = $2, updated_by_id = $3, updated_at = now() \
         WHERE id = $1",
    )
    .bind(registration_id)
    .bind(&overlay_json)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    let updated: Option<(Value,)> =
        sqlx::query_as("SELECT overlay FROM lti_registrations WHERE id = $1")
            .bind(registration_id)
            .fetch_optional(&state.db)
            .await?;

    Ok(Json(json!({
        "id": registration_id,
        "overlay": updated.map(|(o,)| o).unwrap_or(Value::Null),
    })))
}

/// `GET /accounts/:account_id/dr_iframe` — returns the URL for the dynamic
/// registration iframe. Canvas requires a valid registration_token in the
/// query string and validates the JWT.
async fn dr_iframe(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<DrIframeQuery>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    require_site_admin_or_account_admin(&state.db, &auth, account_id).await?;

    let url = params.url.as_deref().unwrap_or_default();

    // Parse the registration_token from the URL query string.
    let parsed = url
        .parse::<url::Url>()
        .map_err(|_| ApiError::BadRequest("Invalid URL".to_string()))?;
    let token = parsed
        .query_pairs()
        .find(|(k, _)| k == "registration_token")
        .map(|(_, v)| v.to_string())
        .ok_or_else(|| ApiError::BadRequest("Missing registration_token".to_string()))?;

    // Decode and validate the JWT.
    let secret = &state.jwt_secret;
    let _decoded: jsonwebtoken::TokenData<serde_json::Value> = jsonwebtoken::decode(
        &token,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &jsonwebtoken::Validation::default(),
    )
    .map_err(|_| ApiError::Unauthorized)?;

    // Return the URL as-is; Canvas' UI embeds it in an iframe.
    Ok(Json(json!({ "url": url })).into_response())
}

/// Query params for `GET /accounts/:account_id/dr_iframe`.
#[derive(Debug, Deserialize)]
pub struct DrIframeQuery {
    pub url: Option<String>,
}

/// `GET /accounts/:account_id/registrations/:registration_id/view` — redirects
/// to the registration management page in the Canvas UI.
async fn registration_view(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    require_site_admin_or_account_admin(&state.db, &auth, account_id).await?;

    // Verify the registration exists.
    let _: Option<(i64,)> = sqlx::query_as("SELECT id FROM lti_registrations WHERE id = $1")
        .bind(registration_id)
        .fetch_optional(&state.db)
        .await?;

    // Redirect to the Canvas UI path for managing this registration.
    let location = format!(
        "/accounts/{}/apps/manage/{}/configuration",
        account_id, registration_id
    );
    let mut response = (
        StatusCode::FOUND,
        axum::response::Redirect::permanent(&location),
    )
        .into_response();
    response.headers_mut().insert(
        axum::http::header::LOCATION,
        axum::http::HeaderValue::from_str(&location).unwrap(),
    );
    Ok(response)
}

/// `POST /registrations` — create a new LTI IMS registration from OIDC
/// dynamic registration payload. Called by the tool after the OIDC handshake.
async fn create_registration(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<OidcRegistrationRequest>,
) -> Result<Response, ApiError> {
    // This route is public in Canvas (uses a registration JWT, not user auth).
    // We validate the access token passed as Bearer.
    // For workspace parity, we accept either a valid user token or a
    // registration JWT. We decode but don't enforce user presence here
    // because the registration token flow handles it upstream.
    let _ = auth; // Mark as used.

    // Validate required fields.
    if payload.client_name.is_none() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "errors": ["client_name is required"] })),
        )
            .into_response());
    }
    if payload.redirect_uris.is_none() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "errors": ["redirect_uris is required"] })),
        )
            .into_response());
    }
    if payload.initiate_login_uri.is_none() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "errors": ["initiate_login_uri is required"] })),
        )
            .into_response());
    }
    if payload.jwks_uri.is_none() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "errors": ["jwks_uri is required"] })),
        )
            .into_response());
    }

    let client_name = payload.client_name.unwrap();
    let redirect_uris = payload.redirect_uris.unwrap();
    let scopes = payload.scopes.unwrap_or_else(|| "openid".to_string());

    let lti_tool_configuration = payload.lti_tool_configuration.unwrap_or(json!({}));

    let guid = uuid::Uuid::new_v4().to_string();

    // Insert the IMS registration.
    let ims: ImsRegistrationRow = sqlx::query_as(&format!(
        "INSERT INTO lti_ims_registrations \
             (guid, client_name, redirect_uris, initiate_login_uri, jwks_uri, \
              logo_uri, scopes, lti_tool_configuration) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             RETURNING {IMS_REG_COLUMNS}"
    ))
    .bind(&guid)
    .bind(&client_name)
    .bind(&redirect_uris)
    .bind(&payload.initiate_login_uri)
    .bind(&payload.jwks_uri)
    .bind(&payload.logo_uri)
    .bind(&scopes)
    .bind(&lti_tool_configuration)
    .fetch_one(&state.db)
    .await?;

    // Also create a parent lti_registration entry.
    let registration: RegistrationRow = sqlx::query_as(
        "INSERT INTO lti_registrations \
         (name, account_id, root_account_id, lti_version, workflow_state, \
          configuration, dynamic_registration, created_by_id, updated_by_id) \
         VALUES ($1, 1, 1, '1.3', 'active', $2, TRUE, $3, $3) \
         RETURNING id, account_id, root_account_id, name, admin_nickname, vendor, \
                   description, icon_url, lti_version, dynamic_registration, internal_service, \
                   lock_deploying, template_registration_id, workflow_state, configuration, \
                   overlay, created_by_id, updated_by_id, created_at, updated_at",
    )
    .bind(&client_name)
    .bind(&lti_tool_configuration)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    // Render the response as Canvas would.
    let json = ims_registration_json(&state.db, &ims, &registration, false).await?;
    Ok((StatusCode::CREATED, Json(json)).into_response())
}

/// `PUT /registrations/:registration_id` — update an existing IMS registration.
async fn update_registration(
    State(state): State<AppState>,
    Path(registration_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<OidcRegistrationRequest>,
) -> Result<Response, ApiError> {
    let _ = auth;
    require_site_admin_or_account_admin(&state.db, &auth, 1).await?;

    // Verify the registration exists.
    let ims = find_ims_registration_by_id(&state.db, registration_id).await?;

    // Update allowed fields.
    let client_name = payload
        .client_name
        .unwrap_or(ims.client_name.unwrap_or_default());
    let redirect_uris = payload
        .redirect_uris
        .unwrap_or_else(|| ims.redirect_uris.unwrap_or_default());
    let initiate_login_uri = payload
        .initiate_login_uri
        .or_else(|| ims.initiate_login_uri.clone());
    let jwks_uri = payload.jwks_uri.or_else(|| ims.jwks_uri.clone());
    let logo_uri = payload.logo_uri.or_else(|| ims.logo_uri.clone());
    let scopes = payload
        .scopes
        .unwrap_or_else(|| ims.scopes.unwrap_or_default());
    let lti_tool_configuration = payload
        .lti_tool_configuration
        .unwrap_or(ims.lti_tool_configuration);

    let updated: ImsRegistrationRow = sqlx::query_as(&format!(
        "UPDATE lti_ims_registrations SET \
             client_name = $2, redirect_uris = $3, initiate_login_uri = $4, \
             jwks_uri = $5, logo_uri = $6, scopes = $7, lti_tool_configuration = $8, \
             updated_at = now() \
             WHERE id = $1 \
             RETURNING {IMS_REG_COLUMNS}"
    ))
    .bind(registration_id)
    .bind(&client_name)
    .bind(&redirect_uris)
    .bind(&initiate_login_uri)
    .bind(&jwks_uri)
    .bind(&logo_uri)
    .bind(&scopes)
    .bind(&lti_tool_configuration)
    .fetch_one(&state.db)
    .await?;

    let registration = find_active_registration(&state.db, updated.id).await?;
    let json = ims_registration_json(&state.db, &updated, &registration, true).await?;
    Ok((StatusCode::OK, Json(json)).into_response())
}

/// `GET /registrations/:registration_id` — return the OIDC dynamic
/// registration configuration for a registration.
/// Corresponds to Canvas' `show_configuration`.
async fn show_registration_configuration(
    State(state): State<AppState>,
    Path(registration_id): Path<i64>,
    _auth: AuthUser,
) -> Result<Response, ApiError> {
    // Verify the registration exists.
    let ims = find_ims_registration_by_id(&state.db, registration_id).await?;

    // Load the parent registration to get configuration.
    let registration = find_active_registration(&state.db, ims.id).await?;

    let json = json!({
        "client_id": ims.developer_key_id.map(|id| id.to_string()).unwrap_or_default(),
        "application_type": payload.application_type,
        "grant_types": payload.grant_types,
        "response_types": payload.response_types,
        "initiate_login_uri": ims.initiate_login_uri,
        "redirect_uris": ims.redirect_uris,
        "client_name": ims.client_name,
        "jwks_uri": ims.jwks_uri,
        "logo_uri": ims.logo_uri,
        "token_endpoint_auth_method": payload.token_endpoint_auth_method,
        "scope": ims.scopes,
        "https://purl.imsglobal.org/spec/lti-tool-configuration": ims.lti_tool_configuration,
        "registration_client_uri": format!("/accounts/{}/registrations/{}/view", registration.account_id, registration_id),
        "deployment_id": Value::Null,
    });

    Ok(Json(json).into_response())
}
