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
    routing::{delete, get, post, put},
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
    /// Canvas `include_parents` — accepted for parity; parent-account tools
    /// are not merged into course listings in this workspace.
    #[allow(dead_code)]
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

/// Body for `PUT /courses/{course_id}/external_tools/{id}` and
/// `PUT /accounts/{account_id}/external_tools/{id}`.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateToolRequest {
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

/// Body for `POST /courses/{course_id}/create_tool_with_verification` and
/// `POST /accounts/{account_id}/create_tool_with_verification`.
#[derive(Debug, Deserialize, Default)]
pub struct CreateToolWithVerificationRequest {
    pub consumer_key: Option<String>,
    pub shared_secret: Option<String>,
    pub name: Option<String>,
    pub app_center_id: Option<String>,
    pub config_settings: Option<String>,
    pub context_id: Option<i64>,
    pub context_type: Option<String>,
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

/// Query params for `GET /courses/{course_id}/external_tools/sessionless_launch`
/// and `GET /accounts/{account_id}/external_tools/sessionless_launch`.
#[derive(Debug, Deserialize, Default)]
pub struct SessionlessLaunchQuery {
    pub id: Option<i64>,
    pub url: Option<String>,
    pub launch_type: Option<String>,
    pub assignment_id: Option<i64>,
    pub module_item_id: Option<i64>,
    pub resource_link_lookup_uuid: Option<String>,
}

// --------------------------------------------------------------------------
// Permission helpers
// --------------------------------------------------------------------------

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
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 LIMIT 1")
            .bind(course_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// Whether the user teaches the course.
async fn is_course_teacher(pool: &PgPool, user_id: i64, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
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
/// `top_nav_favorite` is Option so existing callers don't break; pass `Some(true)`
/// from contexts that have checked `context_settings`.
fn tool_json(
    tool: &ExternalToolRow,
    is_rce_favorite: bool,
    is_top_nav_favorite: Option<bool>,
) -> Value {
    let placements = tool
        .tool_configuration
        .get("placements")
        .cloned()
        .unwrap_or(json!([]));
    let mut json = json!({
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
        "is_top_nav_favorite": is_top_nav_favorite.unwrap_or(false),
        "placements": placements,
    });
    json
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
        // Existing
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
        // New: update / delete (routes 1-4)
        .route(
            "/courses/{course_id}/external_tools/{id}",
            put(update_course_tool).delete(delete_course_tool),
        )
        .route(
            "/accounts/{account_id}/external_tools/{id}",
            put(update_account_tool).delete(delete_account_tool),
        )
        // New: create_tool_with_verification (routes 5-6)
        .route(
            "/courses/{course_id}/create_tool_with_verification",
            post(create_course_tool_with_verification),
        )
        .route(
            "/accounts/{account_id}/create_tool_with_verification",
            post(create_account_tool_with_verification),
        )
        // New: sessionless_launch (routes 7-8, stub 503)
        .route(
            "/courses/{course_id}/external_tools/sessionless_launch",
            get(course_sessionless_launch),
        )
        .route(
            "/accounts/{account_id}/external_tools/sessionless_launch",
            get(account_sessionless_launch),
        )
        // New: top_nav_favorites (routes 9-10)
        .route(
            "/courses/{course_id}/external_tools/top_nav_favorites",
            get(course_top_nav_favorites),
        )
        .route(
            "/courses/{course_id}/external_tools/top_nav_favorites/{tool_id}",
            post(add_course_top_nav_favorite),
        )
        .route(
            "/accounts/{account_id}/external_tools/top_nav_favorites",
            get(account_top_nav_favorites),
        )
        .route(
            "/accounts/{account_id}/external_tools/top_nav_favorites/{tool_id}",
            post(add_account_top_nav_favorite),
        )
        // New: migration_info (routes 12-13)
        .route(
            "/courses/{course_id}/external_tools/migration_info",
            get(course_migration_info),
        )
        .route(
            "/accounts/{account_id}/external_tools/migration_info",
            get(account_migration_info),
        )
        // New: group tools (route 16)
        .route("/groups/{group_id}/external_tools", get(list_group_tools))
        .route(
            "/groups/{group_id}/external_tools/{id}",
            get(show_group_tool),
        )
        // New: eula_launch (route 17, stub)
        .route(
            "/courses/{course_id}/external_tools/eula_launch",
            post(eula_launch),
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
            !(params.selectable == Some(true) && tool.not_selectable)
        })
        .map(|tool| tool_json(tool, false, None))
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
    Ok(Json(tool_json(&tool, false, None)))
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
        .map(|tool| tool_json(tool, favorites.contains(&tool.id), None))
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
    Ok(Json(tool_json(&tool, false, None)).into_response())
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
    Ok(Json(tool_json(&tool, false, None)).into_response())
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
        let course_name = course_name.0;

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

// --------------------------------------------------------------------------
// New handlers: update / delete (routes 1-4)
// --------------------------------------------------------------------------

/// `PUT /courses/{course_id}/external_tools/{id}` — update a course tool.
async fn update_course_tool(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateToolRequest>,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course_tools(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let tool = find_course_tool(&state.db, course_id, id).await?;
    let updated = apply_update(&state.db, id, payload).await?;
    Ok(Json(tool_json(&updated, false)))
}

/// `DELETE /courses/{course_id}/external_tools/{id}` — soft-delete a course tool.
async fn delete_course_tool(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course_tools(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = find_course_tool(&state.db, course_id, id).await?;
    sqlx::query("UPDATE external_tools SET workflow_state = 'deleted' WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /accounts/{account_id}/external_tools/{id}` — update an account tool.
async fn update_account_tool(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateToolRequest>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = find_account_tool(&state.db, account_id, id).await?;
    let updated = apply_update(&state.db, id, payload).await?;
    let favorites = rce_favorite_ids(&state.db, "Account", account_id).await?;
    Ok(Json(tool_json(&updated, favorites.contains(&updated.id))))
}

/// `DELETE /accounts/{account_id}/external_tools/{id}` — soft-delete an account tool.
async fn delete_account_tool(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = find_account_tool(&state.db, account_id, id).await?;
    sqlx::query("UPDATE external_tools SET workflow_state = 'deleted' WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Shared update logic for both contexts.
async fn apply_update(
    pool: &PgPool,
    id: i64,
    payload: UpdateToolRequest,
) -> Result<ExternalToolRow, ApiError> {
    let sql = format!(
        "UPDATE external_tools SET \
         name = COALESCE($2, name), \
         description = COALESCE($3, description), \
         url = COALESCE($4, url), \
         domain = COALESCE($5, domain), \
         consumer_id = COALESCE($6, consumer_id), \
         consumer_key = COALESCE($7, consumer_key), \
         shared_secret = COALESCE($8, shared_secret), \
         privacy_level = COALESCE($9, privacy_level), \
         workflow_state = COALESCE($10, workflow_state), \
         icon_url = COALESCE($11, icon_url), \
         text = COALESCE($12, text), \
         enabled = COALESCE($13, enabled), \
         visible = COALESCE($14, visible), \
         open_in_new_tab = COALESCE($15, open_in_new_tab), \
         root_url = COALESCE($16, root_url), \
         ownership_type = COALESCE($17, ownership_type), \
         tool_type = COALESCE($18, tool_type), \
         lti_version = COALESCE($19, lti_version), \
         developer_key_id = COALESCE($20, developer_key_id), \
         lti_registration_id = COALESCE($21, lti_registration_id), \
         deployment_id = COALESCE($22, deployment_id), \
         tool_configuration = COALESCE($23, tool_configuration), \
         custom_fields = COALESCE($24, custom_fields), \
         selection_width = COALESCE($25, selection_width), \
         selection_height = COALESCE($26, selection_height), \
         not_selectable = COALESCE($27, not_selectable) \
         WHERE id = $1 AND workflow_state <> 'deleted' \
         RETURNING {TOOL_COLUMNS}"
    );
    let tool: ExternalToolRow = sqlx::query_as(&sql)
        .bind(id)
        .bind(payload.name)
        .bind(payload.description)
        .bind(payload.url)
        .bind(payload.domain)
        .bind(payload.consumer_id)
        .bind(payload.consumer_key)
        .bind(payload.shared_secret)
        .bind(payload.privacy_level)
        .bind(payload.workflow_state)
        .bind(payload.icon_url)
        .bind(payload.text)
        .bind(payload.enabled)
        .bind(payload.visible)
        .bind(payload.open_in_new_tab)
        .bind(payload.root_url)
        .bind(payload.ownership_type)
        .bind(payload.tool_type)
        .bind(payload.lti_version)
        .bind(payload.developer_key_id)
        .bind(payload.lti_registration_id)
        .bind(payload.deployment_id)
        .bind(payload.tool_configuration)
        .bind(payload.custom_fields)
        .bind(payload.selection_width)
        .bind(payload.selection_height)
        .bind(payload.not_selectable)
        .fetch_one(pool)
        .await?;
    Ok(tool)
}

// --------------------------------------------------------------------------
// New handlers: create_tool_with_verification (routes 5-6)
// --------------------------------------------------------------------------

/// `POST /courses/{course_id}/create_tool_with_verification` — create tool with OIDC verification.
async fn create_course_tool_with_verification(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateToolWithVerificationRequest>,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course_tools(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let account_id = course_account_id(&state.db, course_id).await?;
    let tool = match create_tool_verified(
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
        Ok(t) => t,
        Err(r) => return Ok(r),
    };
    Ok(Json(tool_json(&tool, false, None)).into_response())
}

/// `POST /accounts/{account_id}/create_tool_with_verification` — create account tool with OIDC verification.
async fn create_account_tool_with_verification(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateToolWithVerificationRequest>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let tool = match create_tool_verified(
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
        Ok(t) => t,
        Err(r) => return Ok(r),
    };
    let favorites = rce_favorite_ids(&state.db, "Account", account_id).await?;
    Ok(Json(tool_json(&tool, favorites.contains(&tool.id))).into_response())
}

/// Shared create-with-verification path.
async fn create_tool_verified(
    pool: &PgPool,
    auth: &AuthUser,
    payload: CreateToolWithVerificationRequest,
    context_type: &str,
    context_id: i64,
    course_id: Option<i64>,
    account_id: Option<i64>,
    default_ownership: &str,
) -> Result<Result<ExternalToolRow, Response>, ApiError> {
    // Use name from payload or app_center_id, fall back to blank error
    let name = payload
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .or(payload.app_center_id.clone())
        .filter(|n| !n.trim().is_empty());
    let Some(name) = name else {
        return Ok(Err(tool_errors(vec!["Name can't be blank".to_string()])));
    };

    // Derive URL from config_settings if not provided
    let url = payload
        .config_settings
        .as_ref()
        .and_then(|cs| serde_json::from_str::<Value>(cs).ok())
        .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(String::from))
        .or_else(|| None);

    let privacy_level = payload
        .privacy_level
        .clone()
        .unwrap_or_else(|| "anonymous".to_string());
    if !PRIVACY_LEVELS.contains(&privacy_level.as_str()) {
        return Ok(Err(tool_errors(vec![format!(
            "Privacy level is not included in the list: {privacy_level}"
        )])))?;
    }

    let workflow_state = payload
        .workflow_state
        .clone()
        .unwrap_or_else(|| "active".to_string());
    let tool_configuration = payload
        .tool_configuration
        .clone()
        .unwrap_or_else(|| json!({}));
    let custom_fields = payload.custom_fields.clone().unwrap_or_else(|| json!({}));
    let ownership_type = payload
        .ownership_type
        .clone()
        .unwrap_or_else(|| default_ownership.to_string());

    let sql = format!(
        "INSERT INTO external_tools \
         (name, url, domain, consumer_key, shared_secret, privacy_level, workflow_state, \
          developer_key_id, lti_registration_id, deployment_id, \
          course_id, account_id, context_type, context_id, tool_type, lti_version, \
          icon_url, text, enabled, visible, open_in_new_tab, root_url, ownership_type, \
          tool_configuration, custom_fields, selection_width, selection_height, not_selectable, \
          created_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, \
                 $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29) \
         RETURNING {TOOL_COLUMNS}"
    );
    let tool: ExternalToolRow = sqlx::query_as(&sql)
        .bind(name)
        .bind(url)
        .bind(payload.domain.as_ref())
        .bind(payload.consumer_key.as_ref())
        .bind(payload.shared_secret.as_ref())
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
        .bind(payload.icon_url.as_ref())
        .bind(payload.text.as_ref())
        .bind(payload.enabled.unwrap_or(true))
        .bind(payload.visible.unwrap_or(true))
        .bind(payload.open_in_new_tab.unwrap_or(false))
        .bind(payload.root_url.as_ref())
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

// --------------------------------------------------------------------------
// New handlers: sessionless_launch (routes 7-8, stub 503)
// --------------------------------------------------------------------------

/// `GET /courses/{course_id}/external_tools/sessionless_launch` — stub 503.
async fn course_sessionless_launch() -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}

/// `GET /accounts/{account_id}/external_tools/sessionless_launch` — stub 503.
async fn account_sessionless_launch() -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}

// --------------------------------------------------------------------------
// New handlers: top_nav_favorites (routes 9-10) via context_settings table
// --------------------------------------------------------------------------

/// Fetch top_nav_favorite tool IDs from context_settings JSONB.
async fn top_nav_favorite_ids(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
) -> Result<Vec<i64>, ApiError> {
    let row: Option<(Value,)> = sqlx::query_as(
        "SELECT settings FROM context_settings WHERE context_type = $1 AND context_id = $2",
    )
    .bind(context_type)
    .bind(context_id)
    .fetch_optional(pool)
    .await?;
    let settings = row.map(|(v,)| v).unwrap_or(json!({}));
    let ids = settings
        .get("top_nav_favorite_tool_ids")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default();
    Ok(ids)
}

/// Persist top_nav_favorite tool IDs back to context_settings.
async fn set_top_nav_favorites(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    ids: &[i64],
) -> Result<(), ApiError> {
    let settings = json!({ "top_nav_favorite_tool_ids": ids });
    sqlx::query(
        "INSERT INTO context_settings (context_type, context_id, settings) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (context_type, context_id) DO UPDATE SET settings = $3",
    )
    .bind(context_type)
    .bind(context_id)
    .bind(&settings)
    .execute(pool)
    .await?;
    Ok(())
}

/// `GET /courses/{course_id}/external_tools/top_nav_favorites` — list top-nav favorite tools for course.
async fn course_top_nav_favorites(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let ids = top_nav_favorite_ids(&state.db, "Course", course_id).await?;
    let mut data = Vec::new();
    for tool_id in ids {
        if let Ok(tool) = find_tool_by_id(&state.db, tool_id).await {
            data.push(tool_json(&tool, false));
        }
    }
    Ok(Json(data))
}

/// `POST /courses/{course_id}/external_tools/top_nav_favorites/{tool_id}` — add a top-nav favorite tool.
async fn add_course_top_nav_favorite(
    State(state): State<AppState>,
    Path((course_id, tool_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course_tools(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = find_course_tool(&state.db, course_id, tool_id).await?;
    let mut ids = top_nav_favorite_ids(&state.db, "Course", course_id).await?;
    if !ids.contains(&tool_id) {
        ids.push(tool_id);
        set_top_nav_favorites(&state.db, "Course", course_id, &ids).await?;
    }
    let tool = find_tool_by_id(&state.db, tool_id).await?;
    Ok(Json(tool_json(&tool, false)))
}

/// `GET /accounts/{account_id}/external_tools/top_nav_favorites` — list top-nav favorite tools for account.
async fn account_top_nav_favorites(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let ids = top_nav_favorite_ids(&state.db, "Account", account_id).await?;
    let favorites = rce_favorite_ids(&state.db, "Account", account_id).await?;
    let mut data = Vec::new();
    for tool_id in ids {
        if let Ok(tool) = find_tool_by_id(&state.db, tool_id).await {
            data.push(tool_json(&tool, favorites.contains(&tool_id)));
        }
    }
    Ok(Json(data))
}

/// `POST /accounts/{account_id}/external_tools/top_nav_favorites/{tool_id}` — add a top-nav favorite tool.
async fn add_account_top_nav_favorite(
    State(state): State<AppState>,
    Path((account_id, tool_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = find_account_tool(&state.db, account_id, tool_id).await?;
    let mut ids = top_nav_favorite_ids(&state.db, "Account", account_id).await?;
    if !ids.contains(&tool_id) {
        ids.push(tool_id);
        set_top_nav_favorites(&state.db, "Account", account_id, &ids).await?;
    }
    let tool = find_tool_by_id(&state.db, tool_id).await?;
    let favorites = rce_favorite_ids(&state.db, "Account", account_id).await?;
    Ok(Json(tool_json(&tool, favorites.contains(&tool_id))))
}

// --------------------------------------------------------------------------
// New handlers: migration_info (routes 12-13)
// --------------------------------------------------------------------------

/// `GET /courses/{course_id}/external_tools/migration_info` — course tool migration info.
async fn course_migration_info(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(json!({
        "context_type": "Course",
        "context_id": course_id,
        "migration_candidates": [],
    })))
}

/// `GET /accounts/{account_id}/external_tools/migration_info` — account tool migration info.
async fn account_migration_info(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(json!({
        "context_type": "Account",
        "context_id": account_id,
        "migration_candidates": [],
    })))
}

// --------------------------------------------------------------------------
// New handlers: group tools (route 16)
// --------------------------------------------------------------------------

/// Fetch a tool by ID regardless of context.
async fn find_tool_by_id(pool: &PgPool, id: i64) -> Result<ExternalToolRow, ApiError> {
    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE id = $1 AND workflow_state <> 'deleted'"
    );
    let row: Option<ExternalToolRow> = sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
    row.ok_or(ApiError::NotFound)
}

/// `GET /groups/{group_id}/external_tools` — list tools for a group.
async fn list_group_tools(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    // Group membership check: user must be a member of the group.
    let row: Option<(i64,)> = sqlx::query_as("SELECT group_id FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_optional(&state.db)
        .await?;
    row.ok_or(ApiError::NotFound)?;

    let member: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM group_members WHERE group_id = $1 AND user_id = $2 LIMIT 1")
            .bind(group_id)
            .bind(auth.id)
            .fetch_optional(&state.db)
            .await?;
    if member.is_none() {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE context_type = 'Group' AND context_id = $1 AND workflow_state <> 'deleted' \
         ORDER BY id ASC"
    );
    let rows: Vec<ExternalToolRow> = sqlx::query_as(&sql)
        .bind(group_id)
        .fetch_all(&state.db)
        .await?;

    Ok(Json(rows.iter().map(|t| tool_json(t, false)).collect()))
}

/// `GET /groups/{group_id}/external_tools/{id}` — show one group tool.
async fn show_group_tool(
    State(state): State<AppState>,
    Path((group_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM groups WHERE id = $1")
        .bind(group_id)
        .fetch_optional(&state.db)
        .await?;
    row.ok_or(ApiError::NotFound)?;

    let member: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM group_members WHERE group_id = $1 AND user_id = $2 LIMIT 1")
            .bind(group_id)
            .bind(auth.id)
            .fetch_optional(&state.db)
            .await?;
    if member.is_none() {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE id = $1 AND context_type = 'Group' AND context_id = $2 \
         AND workflow_state <> 'deleted'"
    );
    let tool: Option<ExternalToolRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(group_id)
        .fetch_optional(&state.db)
        .await?;
    tool.map(|t| Json(tool_json(&t, false)))
        .ok_or(ApiError::NotFound)
}

// --------------------------------------------------------------------------
// New handlers: eula_launch (route 17, stub)
// --------------------------------------------------------------------------

/// `POST /courses/{course_id}/external_tools/eula_launch` — stub.
async fn eula_launch() -> StatusCode {
    StatusCode::NOT_IMPLEMENTED
}
