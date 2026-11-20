//! Canvas parity for the LTI Apps listings (`lti/lti_apps_controller.rb`).
//!
//! Routes (mounted under `/api/v1` by star-http):
//!
//! - `GET /accounts/{account_id}/lti_apps` — app definitions for the account
//! - `GET /accounts/{account_id}/lti_apps/launch_definitions`
//! - `GET /courses/{course_id}/lti_apps` — app definitions for the course
//! - `GET /courses/{course_id}/lti_apps/launch_definitions`
//!
//! Canvas collates app definitions from the context's external tools
//! (AppCollator) and launch definitions from the tools that carry the
//! requested placements (lib/lti/app_launch_collator.rb:77-88, 102-153).
//! The workspace maps both over the `external_tools` table (migration 0085)
//! in the context chain, nearest context first.

use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{can_manage_account, is_admin};

/// The slice of `external_tools` the listings read.
#[derive(Debug, Clone, sqlx::FromRow)]
struct AppToolRow {
    id: i64,
    name: String,
    description: Option<String>,
    url: String,
    domain: Option<String>,
    context_type: String,
    context_id: Option<i64>,
    tool_configuration: Value,
}

const APP_TOOL_COLUMNS: &str =
    "id, name, description, url, domain, context_type, context_id, tool_configuration";

/// Query params for `launch_definitions` — Canvas `placements[]`,
/// `only_visible` (default true) and `include_context_name`
/// (lti_apps_controller.rb:119-159).
#[derive(Debug, Deserialize, Default)]
pub struct LaunchDefinitionsQuery {
    pub placements: Option<String>,
    #[serde(rename = "placements[]")]
    pub placements_brackets: Option<String>,
    pub include_context_name: Option<bool>,
}

/// Whether the user is associated with the account the way Canvas'
/// `user_in_account?` (lti_apps_controller.rb:183-186) means it: an admin of
/// the account/chain, or enrolled in one of its courses.
async fn associated_with_account(
    pool: &PgPool,
    auth: &AuthUser,
    account_id: i64,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let admin: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(account_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if admin.is_some() {
        return Ok(true);
    }
    let enrolled: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments e JOIN courses c ON c.id = e.course_id \
         WHERE c.account_id = $1 AND e.user_id = $2 LIMIT 1",
    )
    .bind(account_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    Ok(enrolled.is_some())
}

/// Build the LTI apps router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/accounts/{account_id}/lti_apps", get(account_apps))
        .route(
            "/accounts/{account_id}/lti_apps/launch_definitions",
            get(account_launch_definitions),
        )
        .route("/courses/{course_id}/lti_apps", get(course_apps))
        .route(
            "/courses/{course_id}/lti_apps/launch_definitions",
            get(course_launch_definitions),
        )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse the placements list (`placements=a,b` or `placements[]=a`
/// repeated as a comma join).
fn placements_from(query: &LaunchDefinitionsQuery) -> Vec<String> {
    query
        .placements
        .as_deref()
        .or(query.placements_brackets.as_deref())
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|placement| !placement.is_empty())
        .map(str::to_string)
        .collect()
}

/// The account chain of an account, nearest first (the context chain Canvas
/// `contexts_to_search` walks).
async fn account_chain(pool: &PgPool, account_id: i64) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "WITH RECURSIVE up AS ( \
             SELECT id, parent_account_id FROM accounts WHERE id = $1 \
             UNION ALL \
             SELECT a.id, a.parent_account_id FROM accounts a \
             JOIN up ON a.id = up.parent_account_id \
         ) SELECT id FROM up",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Whether a tool configuration declares the given placement
/// (Canvas `ContextExternalTool#has_placement?`).
fn has_placement(configuration: &Value, placement: &str) -> bool {
    match configuration.get("placements") {
        Some(Value::Object(map)) => map.contains_key(placement),
        Some(Value::Array(items)) => items.iter().any(|item| {
            item.as_str() == Some(placement)
                || item.get("placement").and_then(Value::as_str) == Some(placement)
        }),
        Some(_) => false,
        None => configuration.get(placement).is_some(),
    }
}

/// Load the active tools of the context chain — course context first, then
/// the account chain (Canvas AppCollator/AppLaunchCollator scope).
async fn context_tools(
    pool: &PgPool,
    course_id: Option<i64>,
    account_id: i64,
) -> Result<Vec<AppToolRow>, ApiError> {
    let chain = account_chain(pool, account_id).await?;
    let sql = format!(
        "SELECT {APP_TOOL_COLUMNS} FROM external_tools \
         WHERE workflow_state <> 'deleted' \
           AND (($1::bigint IS NOT NULL AND context_type = 'Course' AND context_id = $1) \
             OR (context_type = 'Account' AND context_id = ANY($2))) \
         ORDER BY CASE WHEN $1::bigint IS NOT NULL AND context_type = 'Course' THEN 0 ELSE 1 END, \
                  ARRAY_POSITION($2::bigint[], context_id), id ASC"
    );
    let rows: Vec<AppToolRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(chain)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// The context name of a tool's owning context (for
/// `include_context_name`).
async fn context_name_of(pool: &PgPool, tool: &AppToolRow) -> Result<Value, ApiError> {
    match (tool.context_type.as_str(), tool.context_id) {
        ("Account", Some(id)) => {
            let row: Option<(String,)> = sqlx::query_as("SELECT name FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
            Ok(row.map(|(name,)| json!(name)).unwrap_or(Value::Null))
        }
        ("Course", Some(id)) => {
            let row: Option<(String,)> = sqlx::query_as("SELECT name FROM courses WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
            Ok(row.map(|(name,)| json!(name)).unwrap_or(Value::Null))
        }
        _ => Ok(Value::Null),
    }
}

/// Canvas app definition shape (AppCollator over ContextExternalTool):
/// `{id, name, short_name, description, app_type, enabled, installed,
/// has_placements, tool_id, context_id, context_type}`.
async fn app_definition_json(_pool: &PgPool, tool: &AppToolRow) -> Result<Value, ApiError> {
    let tool_id = tool
        .tool_configuration
        .get("tool_id")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "id": tool.id,
        "name": tool.name,
        "short_name": tool.name,
        "description": tool.description,
        "app_type": "lti_tool",
        "enabled": true,
        "installed": true,
        "has_placements": tool.tool_configuration.get("placements").is_some(),
        "tool_id": tool_id,
        "context_id": tool.context_id,
        "context_type": tool.context_type,
    }))
}

/// Canvas launch definition shape (lib/lti/app_launch_collator.rb:102-153):
/// `{definition_type, definition_id, url, name, description, domain,
/// placements: {<placement>: {message_type, url, title}}}`.
async fn launch_definition_json(
    pool: &PgPool,
    tool: &AppToolRow,
    placements: &[String],
    include_context_name: bool,
) -> Result<Value, ApiError> {
    let mut definition = json!({
        "definition_type": "ContextExternalTool",
        "definition_id": tool.id,
        "url": tool.url,
        "name": tool.name,
        "description": tool.description,
        "domain": tool.domain,
        "placements": {},
    });

    let settings = tool
        .tool_configuration
        .get("launch_settings")
        .cloned()
        .unwrap_or_else(|| json!({}));
    for placement in placements {
        if !has_placement(&tool.tool_configuration, placement) {
            continue;
        }
        let placement_settings = tool
            .tool_configuration
            .pointer(&format!("/placements/{placement}"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        let message_type = placement_settings
            .get("message_type")
            .cloned()
            .unwrap_or_else(|| json!("basic-lti-launch-request"));
        let placement_url = placement_settings
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                settings
                    .get("target_link_uri")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| tool.url.clone());
        let mut entry = json!({
            "message_type": message_type,
            "url": placement_url,
            "title": placement_settings
                .get("text")
                .cloned()
                .unwrap_or_else(|| json!(tool.name)),
        });
        if placement == "global_navigation" {
            if let Some(icon_url) = settings.get("icon_url") {
                entry["icon_url"] = icon_url.clone();
            }
        }
        definition["placements"][placement] = entry;
    }

    if include_context_name {
        definition["context_name"] = context_name_of(pool, tool).await?;
    }
    Ok(definition)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /accounts/{account_id}/lti_apps` — Canvas gates the app index behind
/// `read_as_admin` (lti_apps_controller.rb:97-105).
async fn account_apps(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    let tools = account_tools(&state, account_id, &auth).await?;
    let mut apps = Vec::with_capacity(tools.len());
    for tool in &tools {
        apps.push(app_definition_json(&state.db, tool).await?);
    }
    Ok(Json(apps))
}

async fn account_tools(
    state: &AppState,
    account_id: i64,
    auth: &AuthUser,
) -> Result<Vec<AppToolRow>, ApiError> {
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    if !can_manage_account(&state.db, auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    context_tools(&state.db, None, account_id).await
}

/// `GET /accounts/{account_id}/lti_apps/launch_definitions`
/// (lti_apps_controller.rb:119-159): any user associated with the account
/// may read `global_navigation` launch definitions; everything else needs
/// read rights.
async fn account_launch_definitions(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<LaunchDefinitionsQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let exists: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    let placements = placements_from(&params);
    // authorized_for_launch_definitions (:170-181): account +
    // global_navigation + associated user, or read (admin) rights.
    let allowed = if placements.as_slice() == ["global_navigation"] {
        associated_with_account(&state.db, &auth, account_id).await?
    } else {
        can_manage_account(&state.db, &auth, account_id).await?
    };
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let tools = context_tools(&state.db, None, account_id).await?;
    let mut definitions = Vec::with_capacity(tools.len());
    for tool in &tools {
        definitions.push(
            launch_definition_json(
                &state.db,
                tool,
                &placements,
                params.include_context_name.unwrap_or(false),
            )
            .await?,
        );
    }
    Ok(Json(definitions))
}

/// `GET /courses/{course_id}/lti_apps` — read_as_admin for a course means
/// teacher or account admin.
async fn course_apps(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    let tools = course_tools(&state, course_id, &auth).await?;
    let mut apps = Vec::with_capacity(tools.len());
    for tool in &tools {
        apps.push(app_definition_json(&state.db, tool).await?);
    }
    Ok(Json(apps))
}

async fn course_tools(
    state: &AppState,
    course_id: i64,
    auth: &AuthUser,
) -> Result<Vec<AppToolRow>, ApiError> {
    let course: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((account_id,)) = course else {
        return Err(ApiError::NotFound);
    };
    let account_id = account_id.unwrap_or_default();

    // read_as_admin for a course: teacher enrollment or account admin.
    let teacher: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 AND role = 'teacher' \
         LIMIT 1",
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    if !teacher.is_some() && !can_manage_account(&state.db, auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    context_tools(&state.db, Some(course_id), account_id).await
}

/// `GET /courses/{course_id}/lti_apps/launch_definitions` — course read
/// access (member, teacher or admin).
async fn course_launch_definitions(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<LaunchDefinitionsQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let course: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((account_id,)) = course else {
        return Err(ApiError::NotFound);
    };
    let account_id = account_id.unwrap_or_default();

    let member: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 LIMIT 1")
            .bind(course_id)
            .bind(auth.id)
            .fetch_optional(&state.db)
            .await?;
    if !member.is_some() && !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let placements = placements_from(&params);
    let tools = context_tools(&state.db, Some(course_id), account_id).await?;
    let mut definitions = Vec::with_capacity(tools.len());
    for tool in &tools {
        definitions.push(
            launch_definition_json(
                &state.db,
                tool,
                &placements,
                params.include_context_name.unwrap_or(false),
            )
            .await?,
        );
    }
    Ok(Json(definitions))
}
