//! Canvas parity for external tools (`ContextExternalTool`).
//!
//! Canvas source: `app/controllers/external_tools_controller.rb`,
//! `lib/api/v1/external_tools.rb`.
//!
//! Routes (mounted under `/api/v1` by star-http):
//!
//! - `GET    /courses/{course_id}/external_tools` — list course tools
//! - `POST   /courses/{course_id}/external_tools` — create course tool
//! - `GET    /courses/{course_id}/external_tools/{id}` — show course tool
//! - `GET    /accounts/{account_id}/external_tools` — list account tools
//! - `POST   /accounts/{account_id}/external_tools` — create account tool
//! - `POST   /accounts/{account_id}/external_tools/rce_favorites/{id}` — mark
//! - `DELETE /accounts/{account_id}/external_tools/rce_favorites/{id}` — unmark
//! - `GET    /external_tools/visible_course_nav_tools` — nav tools for
//!   `context_codes[]=course_<id>`
//!
//! These live in their own table (`external_tools`, migration 0085) and never
//! touch the phase-3 `lti_registrations` table.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{can_manage_account, is_admin};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Canvas allows at most two RCE favorites per context.
const MAX_RCE_FAVORITES: usize = 2;

/// Canvas' privacy levels for external tools.
const PRIVACY_LEVELS: [&str; 4] = ["anonymous", "name_only", "email_only", "public"];

/// Columns selected for an [`ExternalToolRow`].
const TOOL_COLUMNS: &str = "id, name, description, url, domain, consumer_id, consumer_key, \
     privacy_level, workflow_state, developer_key_id, lti_registration_id, deployment_id, \
     course_id, account_id, context_type, context_id, tool_type, lti_version, icon_url, text, \
     enabled, visible, open_in_new_tab, root_url, ownership_type, tool_configuration, \
     custom_fields, selection_width, selection_height, not_selectable, created_by_id, \
     created_at, updated_at";

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// A row of `external_tools`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ExternalToolRow {
    id: i64,
    name: String,
    description: Option<String>,
    url: String,
    domain: Option<String>,
    consumer_id: Option<String>,
    consumer_key: Option<String>,
    privacy_level: String,
    workflow_state: String,
    developer_key_id: Option<i64>,
    lti_registration_id: Option<i64>,
    deployment_id: Option<String>,
    course_id: Option<i64>,
    account_id: Option<i64>,
    context_type: String,
    context_id: Option<i64>,
    tool_type: String,
    lti_version: String,
    icon_url: Option<String>,
    text: Option<String>,
    enabled: bool,
    visible: bool,
    open_in_new_tab: bool,
    root_url: Option<String>,
    ownership_type: String,
    tool_configuration: Value,
    custom_fields: Value,
    selection_width: Option<i32>,
    selection_height: Option<i32>,
    not_selectable: bool,
    #[allow(dead_code)]
    created_by_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Request / query types
// ---------------------------------------------------------------------------

/// Query params for the tool index endpoints.
#[derive(Debug, Deserialize, Default)]
pub struct ListToolsQuery {
    /// Canvas `search_term` — filters on the tool name.
    pub search_term: Option<String>,
    /// Canvas `placement` — filters on a configured placement.
    pub placement: Option<String>,
    /// Canvas `selectable` — only tools that are selectable.
    pub selectable: Option<bool>,
    /// Canvas `include_parents` — accepted for parity, ignored.
    pub include_parents: Option<bool>,
}

/// Body for `POST /courses/{course_id}/external_tools` and
/// `POST /accounts/{account_id}/external_tools`.
#[derive(Debug, Deserialize, Default)]
pub struct CreateToolRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub domain: Option<String>,
    pub consumer_id: Option<String>,
    pub consumer_key: Option<String>,
    pub shared_secret: Option<String>,
    pub privacy_level: Option<String>,
    pub workflow_state: Option<String>,
    pub icon_url: Option<String>,
    pub text: Option<String>,
    pub enabled: Option<bool>,
    pub visible: Option<bool>,
    pub open_in_new_tab: Option<bool>,
    pub root_url: Option<String>,
    pub ownership_type: Option<String>,
    pub tool_type: Option<String>,
    pub lti_version: Option<String>,
    pub developer_key_id: Option<i64>,
    pub lti_registration_id: Option<i64>,
    pub deployment_id: Option<i64>,
    pub tool_configuration: Option<Value>,
    pub custom_fields: Option<Value>,
    pub selection_width: Option<i32>,
    pub selection_height: Option<i32>,
    pub not_selectable: Option<bool>,
}

/// Query params for `GET /external_tools/visible_course_nav_tools`.
#[derive(Debug, Deserialize, Default)]
pub struct VisibleNavToolsQuery {
    #[serde(rename = "context_codes")]
    pub context_codes: Option<String>,
    #[serde(rename = "context_codes[]")]
    pub context_codes_brackets: Option<String>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// The account a course belongs to (`courses.account_id`).
async fn course_account_id(pool: &PgPool, course_id: i64) -> Result<Option<i64>, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(account_id,)| account_id))
}

/// 404 when the course does not exist.
async fn require_course(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// 404 when the account does not exist.
async fn require_account(pool: &PgPool, account_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Whether the user has any enrollment in the course.
async fn is_course_member(pool: &PgPool, user_id: i64, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 LIMIT 1")
            .bind(course_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// Whether the user teaches the course.
async fn is_course_teacher(pool: &PgPool, user_id: i64, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 AND role = 'teacher' \
         LIMIT 1",
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `authorized_action(context, user, :read)` for a course.
async fn can_read_course(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    if is_course_member(pool, auth.id, course_id).await? {
        return Ok(true);
    }
    if let Some(account_id) = course_account_id(pool, course_id).await? {
        if can_manage_account(pool, auth, account_id).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Canvas `authorized_action(context, user, :manage_lti_edit)` for a course.
async fn can_manage_course_tools(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    if is_course_teacher(pool, auth.id, course_id).await? {
        return Ok(true);
    }
    if let Some(account_id) = course_account_id(pool, course_id).await? {
        if can_manage_account(pool, auth, account_id).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

/// Canvas `external_tool_json` shape.
fn tool_json(tool: &ExternalToolRow, is_rce_favorite: bool) -> Value {
    json!({
        "id": tool.id,
        "name": tool.name,
        "description": tool.description,
        "url": tool.url,
        "domain": tool.domain,
        "consumer_id": tool.consumer_id,
        "consumer_key": tool.consumer_key,
        "privacy_level": tool.privacy_level,
        "workflow_state": tool.workflow_state,
        "created_at": tool.created_at,
        "updated_at": tool.updated_at,
        "developer_key_id": tool.developer_key_id,
        "lti_registration_id": tool.lti_registration_id,
        "deployment_id": tool.deployment_id,
        "course_id": tool.course_id,
        "account_id": tool.account_id,
        "context_type": tool.context_type,
        "context_id": tool.context_id,
        "tool_type": tool.tool_type,
        "lti_version": tool.lti_version,
        "version": tool.lti_version,
        "icon_url": tool.icon_url,
        "text": tool.text,
        "enabled": tool.enabled,
        "visible": tool.visible,
        "open_in_new_tab": tool.open_in_new_tab,
        "root_url": tool.root_url,
        "ownership_type": tool.ownership_type,
        "tool_configuration": tool.tool_configuration,
        "custom_fields": tool.custom_fields,
        "selection_width": tool.selection_width,
        "selection_height": tool.selection_height,
        "not_selectable": tool.not_selectable,
        "is_rce_favorite": is_rce_favorite,
    })
}

/// Canvas `400 {"message": ...}` response.
fn bad_request(message: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "message": message }))).into_response()
}

/// Canvas error body for a failed tool create (`400 {"errors": [...]}`).
fn tool_errors(errors: Vec<String>) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "errors": errors }))).into_response()
}

/// Whether the tool configuration declares the given placement.
fn has_placement(configuration: &Value, placement: &str) -> bool {
    match configuration.get("placements") {
        Some(Value::Object(map)) => map.contains_key(placement),
        Some(Value::Array(items)) => items.iter().any(|item| item.as_str() == Some(placement)),
        Some(_) => false,
        None => configuration.get(placement).is_some(),
    }
}

/// Canvas `context.get_rce_favorite_tool_ids`.
async fn rce_favorite_ids(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT tool_id FROM external_tool_favorites \
         WHERE context_type = $1 AND context_id = $2 AND favorite_type = 'rce' \
         ORDER BY id ASC",
    )
    .bind(context_type)
    .bind(context_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(tool_id,)| tool_id).collect())
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

/// Fetch a tool configured for the given course context.
async fn find_course_tool(
    pool: &PgPool,
    course_id: i64,
    tool_id: i64,
) -> Result<ExternalToolRow, ApiError> {
    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE id = $1 AND context_type = 'Course' AND context_id = $2 \
         AND workflow_state <> 'deleted'"
    );
    let row: Option<ExternalToolRow> = sqlx::query_as(&sql)
        .bind(tool_id)
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// Fetch a tool configured for the given account context.
async fn find_account_tool(
    pool: &PgPool,
    account_id: i64,
    tool_id: i64,
) -> Result<ExternalToolRow, ApiError> {
    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE id = $1 AND context_type = 'Account' AND context_id = $2 \
         AND workflow_state <> 'deleted'"
    );
    let row: Option<ExternalToolRow> = sqlx::query_as(&sql)
        .bind(tool_id)
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the external tools router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{course_id}/external_tools",
            get(list_course_tools).post(create_course_tool),
        )
        .route(
            "/courses/{course_id}/external_tools/{id}",
            get(show_course_tool),
        )
        .route(
            "/accounts/{account_id}/external_tools",
            get(list_account_tools).post(create_account_tool),
        )
        .route(
            "/accounts/{account_id}/external_tools/rce_favorites/{id}",
            post(mark_rce_favorite).delete(unmark_rce_favorite),
        )
        .route(
            "/external_tools/visible_course_nav_tools",
            get(all_visible_nav_tools),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /courses/{course_id}/external_tools` — list the tools configured for
/// the course. Canvas returns a JSON array.
async fn list_course_tools(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<ListToolsQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let search = params
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| format!("%{term}%"));

    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE context_type = 'Course' AND context_id = $1 AND workflow_state <> 'deleted' \
         AND ($2::text IS NULL OR name ILIKE $2) ORDER BY id ASC"
    );
    let rows: Vec<ExternalToolRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(search.as_deref())
        .fetch_all(&state.db)
        .await?;

    let data = rows
        .iter()
        .filter(|tool| {
            if let Some(placement) = params.placement.as_deref() {
                if !has_placement(&tool.tool_configuration, placement) {
                    return false;
                }
            }
            if params.selectable == Some(true) && tool.not_selectable {
                return false;
            }
            true
        })
        .map(|tool| tool_json(tool, false))
        .collect();

    Ok(Json(data))
}

/// `GET /courses/{course_id}/external_tools/{id}` — show one course tool.
async fn show_course_tool(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let tool = find_course_tool(&state.db, course_id, id).await?;
    Ok(Json(tool_json(&tool, false)))
}

/// `GET /accounts/{account_id}/external_tools` — list the tools configured for
/// the account.
async fn list_account_tools(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<ListToolsQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let search = params
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| format!("%{term}%"));

    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE context_type = 'Account' AND context_id = $1 AND workflow_state <> 'deleted' \
         AND ($2::text IS NULL OR name ILIKE $2) ORDER BY id ASC"
    );
    let rows: Vec<ExternalToolRow> = sqlx::query_as(&sql)
        .bind(account_id)
        .bind(search.as_deref())
        .fetch_all(&state.db)
        .await?;

    let favorites = rce_favorite_ids(&state.db, "Account", account_id).await?;
    let data = rows
        .iter()
        .filter(|tool| {
            if let Some(placement) = params.placement.as_deref() {
                if !has_placement(&tool.tool_configuration, placement) {
                    return false;
                }
            }
            true
        })
        .map(|tool| tool_json(tool, favorites.contains(&tool.id)))
        .collect();

    Ok(Json(data))
}

/// `POST /courses/{course_id}/external_tools` — create a course tool.
async fn create_course_tool(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateToolRequest>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course_tools(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let account_id = course_account_id(&state.db, course_id).await?;
    let tool = match insert_tool(
        &state.db,
        &auth,
        payload,
        "Course",
        course_id,
        Some(course_id),
        account_id,
        "account",
    )
    .await?
    {
        Ok(tool) => tool,
        Err(response) => return Ok(response),
    };
    Ok(Json(tool_json(&tool, false)).into_response())
}

/// `POST /accounts/{account_id}/external_tools` — create an account tool.
async fn create_account_tool(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateToolRequest>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let tool = match insert_tool(
        &state.db,
        &auth,
        payload,
        "Account",
        account_id,
        None,
        Some(account_id),
        "account",
    )
    .await?
    {
        Ok(tool) => tool,
        Err(response) => return Ok(response),
    };
    Ok(Json(tool_json(&tool, false)).into_response())
}

/// Shared create path for both contexts. `Err(Response)` carries a Canvas
/// `400 {"errors": [...]}` validation response.
#[allow(clippy::too_many_arguments)]
async fn insert_tool(
    pool: &PgPool,
    auth: &AuthUser,
    payload: CreateToolRequest,
    context_type: &str,
    context_id: i64,
    course_id: Option<i64>,
    account_id: Option<i64>,
    default_ownership: &str,
) -> Result<Result<ExternalToolRow, Response>, ApiError> {
    let Some(name) = payload.name.clone().filter(|name| !name.trim().is_empty()) else {
        return Ok(Err(tool_errors(vec!["Name can't be blank".to_string()])));
    };
    let Some(url) = payload.url.clone().filter(|url| !url.trim().is_empty()) else {
        return Ok(Err(tool_errors(vec!["Url can't be blank".to_string()])));
    };

    let privacy_level = payload
        .privacy_level
        .clone()
        .unwrap_or_else(|| "anonymous".to_string());
    if !PRIVACY_LEVELS.contains(&privacy_level.as_str()) {
        return Ok(Err(tool_errors(vec![format!(
            "Privacy level is not included in the list: {privacy_level}"
        )])));
    }

    let workflow_state = payload
        .workflow_state
        .clone()
        .unwrap_or_else(|| "active".to_string());

    let tool_configuration = match payload.tool_configuration {
        Some(value) if value.is_object() => value,
        Some(_) => {
            return Ok(Err(tool_errors(vec![
                "tool_configuration must be an object".to_string(),
            ])))
        }
        None => json!({}),
    };

    let custom_fields = match payload.custom_fields {
        Some(value) if value.is_object() => value,
        Some(_) => {
            return Ok(Err(tool_errors(vec![
                "custom_fields must be an object".to_string()
            ])))
        }
        None => json!({}),
    };

    let ownership_type = payload
        .ownership_type
        .clone()
        .unwrap_or_else(|| default_ownership.to_string());

    let sql = format!(
        "INSERT INTO external_tools \
         (name, description, url, domain, consumer_id, consumer_key, shared_secret, \
          privacy_level, workflow_state, developer_key_id, lti_registration_id, deployment_id, \
          course_id, account_id, context_type, context_id, tool_type, lti_version, icon_url, \
          text, enabled, visible, open_in_new_tab, root_url, ownership_type, tool_configuration, \
          custom_fields, selection_width, selection_height, not_selectable, created_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, \
                 $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31) \
         RETURNING {TOOL_COLUMNS}"
    );
    let tool: ExternalToolRow = sqlx::query_as(&sql)
        .bind(name)
        .bind(payload.description)
        .bind(url)
        .bind(payload.domain)
        .bind(payload.consumer_id)
        .bind(payload.consumer_key)
        .bind(payload.shared_secret)
        .bind(privacy_level)
        .bind(workflow_state)
        .bind(payload.developer_key_id)
        .bind(payload.lti_registration_id)
        .bind(payload.deployment_id)
        .bind(course_id)
        .bind(account_id)
        .bind(context_type)
        .bind(context_id)
        .bind(
            payload
                .tool_type
                .clone()
                .unwrap_or_else(|| "lti".to_string()),
        )
        .bind(
            payload
                .lti_version
                .clone()
                .unwrap_or_else(|| "1.3".to_string()),
        )
        .bind(payload.icon_url)
        .bind(payload.text)
        .bind(payload.enabled.unwrap_or(true))
        .bind(payload.visible.unwrap_or(true))
        .bind(payload.open_in_new_tab.unwrap_or(false))
        .bind(payload.root_url)
        .bind(ownership_type)
        .bind(tool_configuration)
        .bind(custom_fields)
        .bind(payload.selection_width)
        .bind(payload.selection_height)
        .bind(payload.not_selectable.unwrap_or(false))
        .bind(auth.id)
        .fetch_one(pool)
        .await?;

    Ok(Ok(tool))
}

/// `POST /accounts/{account_id}/external_tools/rce_favorites/{id}` — mark the
/// tool as an RCE favorite. Canvas answers `400` when the tool has no
/// `editor_button` placement or when the limit of two is exceeded.
async fn mark_rce_favorite(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let tool = find_account_tool(&state.db, account_id, id).await?;
    if !has_placement(&tool.tool_configuration, "editor_button") {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Tool does not have an editor_button placement" })),
        )
            .into_response());
    }

    let mut favorites = rce_favorite_ids(&state.db, "Account", account_id).await?;
    if !favorites.contains(&tool.id) {
        favorites.push(tool.id);
    }
    if favorites.len() > MAX_RCE_FAVORITES {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Cannot have more than 2 favorited tools" })),
        )
            .into_response());
    }

    sqlx::query(
        "INSERT INTO external_tool_favorites \
         (context_type, context_id, tool_id, favorite_type) \
         VALUES ('Account', $1, $2, 'rce') \
         ON CONFLICT (context_type, context_id, tool_id, favorite_type) DO NOTHING",
    )
    .bind(account_id)
    .bind(tool.id)
    .execute(&state.db)
    .await?;

    Ok(Json(json!({ "rce_favorite_tool_ids": favorites })).into_response())
}

/// `DELETE /accounts/{account_id}/external_tools/rce_favorites/{id}` — unmark
/// the tool as an RCE favorite.
async fn unmark_rce_favorite(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    sqlx::query(
        "DELETE FROM external_tool_favorites \
         WHERE context_type = 'Account' AND context_id = $1 AND tool_id = $2 \
         AND favorite_type = 'rce'",
    )
    .bind(account_id)
    .bind(id)
    .execute(&state.db)
    .await?;

    let favorites = rce_favorite_ids(&state.db, "Account", account_id).await?;
    Ok(Json(json!({ "rce_favorite_tool_ids": favorites })))
}

/// `GET /external_tools/visible_course_nav_tools?context_codes[]=course_5` —
/// the course navigation tools of the requested courses.
async fn all_visible_nav_tools(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<VisibleNavToolsQuery>,
) -> Result<Response, ApiError> {
    let raw = params
        .context_codes
        .as_deref()
        .or(params.context_codes_brackets.as_deref())
        .unwrap_or_default();

    let mut course_ids = Vec::new();
    for code in raw.split(',') {
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        let Some(id) = code.strip_prefix("course_") else {
            return Ok(bad_request(
                "Invalid context_codes; only `course` codes are supported".to_string(),
            ));
        };
        let Ok(id) = id.parse::<i64>() else {
            return Ok(bad_request(
                "Invalid context_codes; only `course` codes are supported".to_string(),
            ));
        };
        course_ids.push(id);
    }

    if course_ids.is_empty() {
        return Ok(bad_request("Missing context_codes".to_string()));
    }

    let mut data = Vec::new();
    for course_id in course_ids {
        require_course(&state.db, course_id).await?;
        if !can_read_course(&state.db, &auth, course_id).await? {
            return Err(ApiError::Forbidden);
        }

        let course_name: (String,) = sqlx::query_as("SELECT name FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_one(&state.db)
            .await?;

        let sql = format!(
            "SELECT {TOOL_COLUMNS} FROM external_tools \
             WHERE context_type = 'Course' AND context_id = $1 \
             AND workflow_state <> 'deleted' AND enabled AND visible ORDER BY id ASC"
        );
        let rows: Vec<ExternalToolRow> = sqlx::query_as(&sql)
            .bind(course_id)
            .fetch_all(&state.db)
            .await?;

        for tool in rows {
            let has_placements = tool.tool_configuration.get("placements").is_some();
            if has_placements && !has_placement(&tool.tool_configuration, "course_navigation") {
                continue;
            }
            let mut value = tool_json(&tool, false);
            value["context_id"] = json!(course_id);
            value["context_name"] = json!(course_name);
            data.push(value);
        }
    }

    Ok(Json(data).into_response())
}
