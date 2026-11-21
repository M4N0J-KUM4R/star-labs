//! lti/tool_configurations — Canvas `Lti::ToolConfigurationsApiController`
//! parity.
//!
//! Routes (the LTI Advantage router mounts this module at the app root, so
//! the paths carry the literal `/api/lti` prefix):
//! - `GET    /api/lti/accounts/{accountId}/developer_keys/{developerKeyId}/tool_configuration`
//! - `GET    /api/lti/courses/{courseId}/developer_keys/{developerKeyId}/tool_configuration`
//! - `POST   /api/lti/accounts/{accountId}/developer_keys/tool_configuration`
//! - `PUT    /api/lti/developer_keys/{developerKeyId}/tool_configuration`
//! - `DELETE /api/lti/developer_keys/{developerKeyId}/tool_configuration`
//!
//! Canvas routes (config/routes.rb, the LTI API block):
//!   put    "developer_keys/:developer_key_id/tool_configuration", action: :update
//!   post   "accounts/:account_id/developer_keys/tool_configuration", action: :create
//!   delete "developer_keys/:developer_key_id/tool_configuration", action: :destroy
//!   get    "accounts/:account_id/developer_keys/:developer_key_id/tool_configuration", action: :show
//!   get    "courses/:course_id/developer_keys/:developer_key_id/tool_configuration", action: :show
//!
//! Canvas controller (`app/controllers/lti/tool_configurations_api_controller.rb`):
//! the `before_action` chain is `require_context` (404 when the account or
//! the course does not exist), `require_settings_or_url` (400 when neither
//! `settings` nor `settings_url` is given), `require_manage_developer_keys`
//! (403), `require_modify_site_admin_developer_keys` (403),
//! `require_key_in_context` (404), `require_manage_lti` (403) and
//! `require_tool_configuration` (404).
//!
//! The response is the Canvas `Lti::ToolConfigurationSerializer` shape: the
//! tool configuration with its `developer_key` and the `settings` of the
//! configuration.

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

/// An `lti_tool_configurations` row.
#[derive(Debug, sqlx::FromRow)]
struct ToolConfiguration {
    id: i64,
    developer_key_id: Option<i64>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    disabled_placements: Vec<String>,
    privacy_level: Option<String>,
    unified_tool_id: Option<String>,
    lti_registration_id: Option<i64>,
    title: Option<String>,
    description: Option<String>,
    target_link_uri: Option<String>,
    domain: Option<String>,
    tool_id: Option<String>,
    public_jwk_url: Option<String>,
    oidc_initiation_url: Option<String>,
    custom_fields: Value,
    launch_settings: Value,
}

/// The columns of an `lti_tool_configurations` row.
///
/// The Canvas schema stores the timestamps without a time zone, so they are
/// cast on the way out.
const CONFIG_COLUMNS: &str = "id, developer_key_id, created_at::timestamptz AS created_at, \
     updated_at::timestamptz AS updated_at, \
     disabled_placements, privacy_level, unified_tool_id, lti_registration_id, title, description, \
     target_link_uri, domain, tool_id, public_jwk_url, oidc_initiation_url, custom_fields, \
     launch_settings";

/// The body of a create or update request.
#[derive(Debug, Default, Deserialize)]
struct ConfigurationRequest {
    settings: Option<Value>,
    settings_url: Option<String>,
    developer_key: Option<DeveloperKeyRequest>,
    redirect_uris: Option<Vec<String>>,
    privacy_level: Option<String>,
    disabled_placements: Option<Vec<String>>,
}

/// The developer key of a create request.
#[derive(Debug, Default, Deserialize)]
struct DeveloperKeyRequest {
    name: Option<String>,
    email: Option<String>,
    owner_account_id: Option<i64>,
    public_jwk_url: Option<String>,
}

/// Domain router for the Canvas tool-configuration routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/developer_keys/{developer_key_id}/tool_configuration",
            axum::routing::put(update_configuration).delete(destroy_configuration),
        )
        .route(
            "/api/lti/accounts/{account_id}/developer_keys/tool_configuration",
            axum::routing::post(create_configuration),
        )
        .route(
            "/api/lti/accounts/{account_id}/developer_keys/{developer_key_id}/tool_configuration",
            get(show_account_configuration),
        )
        .route(
            "/api/lti/courses/{course_id}/developer_keys/{developer_key_id}/tool_configuration",
            get(show_course_configuration),
        )
}

/// `GET /api/lti/accounts/:account_id/developer_keys/:developer_key_id/tool_configuration`.
async fn show_account_configuration(
    State(state): State<AppState>,
    Path((account_id, developer_key_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_context(&state.db, Context::Account(account_id)).await?;
    require_manage_lti(&state.db, &auth, account_id).await?;
    let configuration = load_configuration(&state.db, developer_key_id).await?;
    Ok(Json(configuration_json(&state.db, &configuration).await?).into_response())
}

/// `GET /api/lti/courses/:course_id/developer_keys/:developer_key_id/tool_configuration`.
async fn show_course_configuration(
    State(state): State<AppState>,
    Path((course_id, developer_key_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_context(&state.db, Context::Course(course_id)).await?;
    require_manage_lti(&state.db, &auth, course_id).await?;
    let configuration = load_configuration(&state.db, developer_key_id).await?;
    Ok(Json(configuration_json(&state.db, &configuration).await?).into_response())
}

/// `POST /api/lti/accounts/:account_id/developer_keys/tool_configuration`.
async fn create_configuration(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<ConfigurationRequest>,
) -> Result<Response, ApiError> {
    require_context(&state.db, Context::Account(account_id)).await?;
    require_manage_developer_keys(&state.db, &auth, account_id).await?;

    // Canvas needs the settings of the configuration.
    let Some(settings) = body.settings.clone().filter(|value| value.is_object()) else {
        return Err(ApiError::BadRequest(
            "Either settings or settings_url is required".to_string(),
        ));
    };

    // The developer key of the configuration.
    let key = body.developer_key.clone().unwrap_or_default();
    let developer_key_id: i64 = sqlx::query_scalar(
        "INSERT INTO developer_keys (name, email, workflow_state, api_key, user_id) \
         VALUES ($1, $2, 'active', $3, $4) RETURNING id",
    )
    .bind(key.name.clone().unwrap_or_else(|| "LTI Tool".to_string()))
    .bind(&key.email)
    .bind(format!("lti-{}-{}", account_id, uuid::Uuid::new_v4()))
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    let configuration = insert_configuration(
        &state.db,
        developer_key_id,
        &settings,
        body.privacy_level.clone(),
        body.disabled_placements.clone(),
    )
    .await?;

    Ok(Json(configuration_json(&state.db, &configuration).await?).into_response())
}

/// `PUT /api/lti/developer_keys/:developer_key_id/tool_configuration`.
async fn update_configuration(
    State(state): State<AppState>,
    Path(developer_key_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<ConfigurationRequest>,
) -> Result<Response, ApiError> {
    require_manage_developer_keys(&state.db, &auth, developer_key_id).await?;

    let existing = load_configuration(&state.db, developer_key_id).await?;
    let settings = body
        .settings
        .clone()
        .filter(|value| value.is_object())
        .unwrap_or_else(|| existing.launch_settings.clone());

    sqlx::query(
        "UPDATE lti_tool_configurations SET launch_settings = $1, privacy_level = $2, \
             disabled_placements = $3, updated_at = now() WHERE id = $4",
    )
    .bind(&settings)
    .bind(
        body.privacy_level
            .clone()
            .or(existing.privacy_level.clone()),
    )
    .bind(
        body.disabled_placements
            .clone()
            .unwrap_or(existing.disabled_placements.clone()),
    )
    .bind(existing.id)
    .execute(&state.db)
    .await?;

    let configuration = load_configuration(&state.db, developer_key_id).await?;
    Ok(Json(configuration_json(&state.db, &configuration).await?).into_response())
}

/// `DELETE /api/lti/developer_keys/:developer_key_id/tool_configuration`.
async fn destroy_configuration(
    State(state): State<AppState>,
    Path(developer_key_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_manage_developer_keys(&state.db, &auth, developer_key_id).await?;
    let configuration = load_configuration(&state.db, developer_key_id).await?;
    sqlx::query("DELETE FROM lti_tool_configurations WHERE id = $1")
        .bind(configuration.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// The context of a tool-configuration route.
#[derive(Debug, Clone, Copy)]
enum Context {
    Account(i64),
    Course(i64),
}

/// `require_context` — the account or the course has to exist.
async fn require_context(db: &PgPool, context: Context) -> Result<(), ApiError> {
    let (table, id) = match context {
        Context::Account(id) => ("accounts", id),
        Context::Course(id) => ("courses", id),
    };
    let sql = format!("SELECT id FROM {table} WHERE id = $1");
    let row: Option<(i64,)> = sqlx::query_as(&sql).bind(id).fetch_optional(db).await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// `require_manage_developer_keys` — the caller has to be a site admin or
/// the owner of the developer key.
async fn require_manage_developer_keys(
    db: &PgPool,
    auth: &AuthUser,
    developer_key_id: i64,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM developer_keys WHERE id = $1 AND user_id = $2 LIMIT 1")
            .bind(developer_key_id)
            .bind(auth.id)
            .fetch_optional(db)
            .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// `require_manage_lti` — the caller has to be an admin of the account.
async fn require_manage_lti(db: &PgPool, auth: &AuthUser, account_id: i64) -> Result<(), ApiError> {
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

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// The tool configuration of a developer key — 404 when there is none.
async fn load_configuration(
    db: &PgPool,
    developer_key_id: i64,
) -> Result<ToolConfiguration, ApiError> {
    let sql = format!(
        "SELECT {CONFIG_COLUMNS} FROM lti_tool_configurations \
         WHERE developer_key_id = $1 LIMIT 1"
    );
    sqlx::query_as(&sql)
        .bind(developer_key_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Store the tool configuration of a developer key.
async fn insert_configuration(
    db: &PgPool,
    developer_key_id: i64,
    settings: &Value,
    privacy_level: Option<String>,
    disabled_placements: Option<Vec<String>>,
) -> Result<ToolConfiguration, ApiError> {
    let sql = format!(
        "INSERT INTO lti_tool_configurations \
             (developer_key_id, created_at, updated_at, disabled_placements, privacy_level, \
              custom_fields, launch_settings, oidc_initiation_urls) \
         VALUES ($1, now(), now(), $2, $3, '{{}}', $4, '{{}}') \
         RETURNING {CONFIG_COLUMNS}"
    );
    sqlx::query_as(&sql)
        .bind(developer_key_id)
        .bind(disabled_placements.unwrap_or_default())
        .bind(privacy_level)
        .bind(settings)
        .fetch_one(db)
        .await
        .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Canvas `Lti::ToolConfigurationSerializer#as_json`.
async fn configuration_json(
    db: &PgPool,
    configuration: &ToolConfiguration,
) -> Result<Value, ApiError> {
    let developer_key = developer_key_json(db, configuration.developer_key_id).await?;
    Ok(json!({
        "id": configuration.id,
        "developer_key_id": configuration.developer_key_id,
        "created_at": configuration.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "updated_at": configuration.updated_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "disabled_placements": configuration.disabled_placements,
        "privacy_level": configuration.privacy_level,
        "unified_tool_id": configuration.unified_tool_id,
        "lti_registration_id": configuration.lti_registration_id,
        "title": configuration.title,
        "description": configuration.description,
        "target_link_uri": configuration.target_link_uri,
        "domain": configuration.domain,
        "tool_id": configuration.tool_id,
        "public_jwk_url": configuration.public_jwk_url,
        "oidc_initiation_url": configuration.oidc_initiation_url,
        "custom_fields": configuration.custom_fields,
        "launch_settings": configuration.launch_settings,
        "developer_key": developer_key,
        "tool_configuration": {
            "settings": configuration.launch_settings,
        },
    }))
}

/// The developer key of a tool configuration.
async fn developer_key_json(db: &PgPool, developer_key_id: Option<i64>) -> Result<Value, ApiError> {
    let Some(developer_key_id) = developer_key_id else {
        return Ok(Value::Null);
    };
    let row: Option<(i64, Option<String>, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT id, name, email, user_id FROM developer_keys WHERE id = $1")
            .bind(developer_key_id)
            .fetch_optional(db)
            .await?;
    let Some((id, name, email, user_id)) = row else {
        return Ok(Value::Null);
    };
    Ok(json!({
        "id": id,
        "name": name,
        "email": email,
        "user_id": user_id,
        "workflow_state": "active",
        "developer_key": id.to_string(),
        "api_key": id.to_string(),
        "created_at": Value::Null,
    }))
}
