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
//! - `GET    /accounts/{account_id}/external_tools/{id}` — show account tool
//! - `POST   /accounts/{account_id}/external_tools/rce_favorites/{id}` — mark
//! - `DELETE /accounts/{account_id}/external_tools/rce_favorites/{id}` — unmark
//! - `GET    /external_tools/visible_course_nav_tools` — nav tools for
//!   `context_codes[]=course_<id>`
//!
//! Slice lti-1 additions (brief `/Users/manojkumar/lms-migration/orchestration/briefs/lti-1.md`):
//!
//! - `PUT    /courses/{course_id}/external_tools/{id}` — update
//! - `DELETE /courses/{course_id}/external_tools/{id}` — soft delete
//! - `PUT    /accounts/{account_id}/external_tools/{id}` — update
//! - `DELETE /accounts/{account_id}/external_tools/{id}` — soft delete
//! - `POST   /courses/{course_id}/create_tool_with_verification` — create
//! - `POST   /accounts/{account_id}/create_tool_with_verification` — create
//! - `GET    /courses/{course_id}/external_tools/sessionless_launch` — 503
//!   until Redis exists (brief ambiguity 3)
//! - `GET    /accounts/{account_id}/external_tools/sessionless_launch` — 503
//! - `POST   /accounts/{account_id}/external_tools/top_nav_favorites/{id}`
//! - `DELETE /accounts/{account_id}/external_tools/top_nav_favorites/{id}`
//! - `GET    /courses/{course_id}/external_tools/visible_course_nav_tools`
//! - `GET    /courses/{course_id}/external_tools/{id}/migration_info`
//! - `GET    /accounts/{account_id}/external_tools/{id}/migration_info`
//! - `GET    /groups/{group_id}/external_tools` — tools via the group's course
//! - `GET    /courses/{course_id}/external_tools/{id}/eula_launch` — 200
//!   placeholder until LTI launch rendering exists (brief route 17)
//! - `GET    /api/lti/courses/{course_id}/jwt_token` — signed launch token
//! - `GET    /api/lti/accounts/{account_id}/jwt_token` — signed launch token
//!   (Canvas config/routes.rb:3153-3160, outside `/api/v1`)
//!
//! These live in their own table (`external_tools`, migration 0085) and never
//! touch the phase-3 `lti_registrations` table.

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
use url::Url;

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

/// Columns selected for an [`ExternalToolRow`]. `unified_tool_id` is Canvas
/// `ContextExternalTool#unified_tool_id` (lib/api/v1/external_tools.rb:49),
/// resolved through the tool's LTI registration link.
const TOOL_COLUMNS: &str = "id, name, description, url, domain, consumer_id, consumer_key, \
     privacy_level, workflow_state, developer_key_id, lti_registration_id, deployment_id, \
     course_id, account_id, context_type, context_id, tool_type, lti_version, icon_url, text, \
     enabled, visible, open_in_new_tab, root_url, ownership_type, tool_configuration, \
     custom_fields, selection_width, selection_height, not_selectable, created_by_id, \
     created_at, updated_at, \
     (SELECT r.unified_tool_id FROM lti_registrations r WHERE r.id = lti_registration_id) \
     AS unified_tool_id";

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
    /// Canvas `ContextExternalTool#unified_tool_id` — resolved from the
    /// linked LTI registration (see TOOL_COLUMNS).
    unified_tool_id: Option<String>,
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
    pub deployment_id: Option<String>,
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
    pub deployment_id: Option<String>,
    pub tool_configuration: Option<Value>,
    pub custom_fields: Option<Value>,
    pub selection_width: Option<i32>,
    pub selection_height: Option<i32>,
    pub not_selectable: Option<bool>,
}

/// Body for `POST /courses/{course_id}/create_tool_with_verification` and
/// `POST /accounts/{account_id}/create_tool_with_verification`.
///
/// Canvas external_tools_controller.rb:1594-1602 permits exactly these 7
/// params: consumer_key, shared_secret, name, app_center_id, context_id,
/// context_type, config_settings. Everything else is ignored, so the
/// request struct carries nothing more.
#[derive(Debug, Deserialize, Default)]
pub struct CreateToolWithVerificationRequest {
    pub consumer_key: Option<String>,
    pub shared_secret: Option<String>,
    pub name: Option<String>,
    pub app_center_id: Option<String>,
    pub context_id: Option<i64>,
    pub context_type: Option<String>,
    pub config_settings: Option<String>,
}

/// Query params for the jwt_token endpoints.
///
/// Canvas external_tools_controller.rb:1678-1682 reads `tool_launch_url`
/// (matched against tool URLs and domains) and `tool_id` (the preferred
/// tool).
#[derive(Debug, Deserialize, Default)]
pub struct JwtTokenQuery {
    pub tool_launch_url: Option<String>,
    pub tool_id: Option<i64>,
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
    // Canvas serializes placements as an array of
    // {placement: "<name>", ...} objects (Lti::IMS::Registration#placements,
    // lti/ims/registration.rb:231). The stored tool_configuration uses the
    // map form {name: settings}, so convert; a configuration that already
    // carries the array form passes through unchanged.
    let placements = match tool.tool_configuration.get("placements") {
        Some(Value::Object(map)) => json!(map
            .iter()
            .map(|(name, settings)| {
                let mut entry = json!({ "placement": name });
                if let Value::Object(fields) = settings {
                    for (key, value) in fields {
                        entry[key] = value.clone();
                    }
                }
                entry
            })
            .collect::<Vec<_>>()),
        Some(Value::Array(items)) => json!(items),
        _ => json!([]),
    };
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
        "unified_tool_id": tool.unified_tool_id,
        "message_settings": tool.tool_configuration.get("message_settings").cloned().unwrap_or(json!({})),
        "placements": placements,
    });
    // Canvas external_tools.rb:44 only emits is_top_nav_favorite when the
    // tool can be a top-nav favorite (top_navigation placement); it never
    // renders `false`, so the key is added conditionally.
    if let Some(fav) = is_top_nav_favorite {
        json["is_top_nav_favorite"] = json!(fav);
    }
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
// jwt_token lookups (Canvas `Lti::ToolFinder.from_url`, scoped)
// ---------------------------------------------------------------------------

/// The slice of `external_tools` jwt_token reads. `shared_secret` is included
/// although TOOL_COLUMNS omits it — tool JSON never renders it, but the
/// launch token is signed with it.
#[derive(Debug, Clone, sqlx::FromRow)]
struct JwtToolRow {
    id: i64,
    url: String,
    domain: Option<String>,
    consumer_key: Option<String>,
    shared_secret: Option<String>,
}

/// An account and its ancestors — the context chain Canvas
/// `contexts_to_search` walks. A NULL root (a course without an account)
/// yields an empty chain.
async fn account_chain(pool: &PgPool, root: Option<i64>) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "WITH RECURSIVE up AS ( \
             SELECT id, parent_account_id FROM accounts WHERE id = $1 \
             UNION ALL \
             SELECT a.id, a.parent_account_id FROM accounts a \
             JOIN up ON a.id = up.parent_account_id \
         ) SELECT id FROM up",
    )
    .bind(root)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Active tools in the context chain, nearest context first — the candidate
/// list `Lti::ToolFinder.from_url` orders over (app/services/lti/
/// tool_finder.rb:249-318; the LTI 1.3 and duplicate-identity preference
/// clauses are scoped out — the workspace models no duplicate identity
/// hashes).
async fn launch_candidates(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    account_chain: &[i64],
) -> Result<Vec<JwtToolRow>, ApiError> {
    let rows: Vec<JwtToolRow> = sqlx::query_as(
        "SELECT id, url, domain, consumer_key, shared_secret FROM external_tools \
         WHERE workflow_state <> 'deleted' \
           AND (($1::text = 'Course' AND context_type = 'Course' AND context_id = $2) \
             OR (context_type = 'Account' AND context_id = ANY($3))) \
         ORDER BY CASE WHEN $1::text = 'Course' AND context_type = 'Course' THEN 0 ELSE 1 END, \
                  ARRAY_POSITION($3::bigint[], context_id), \
                  id ASC",
    )
    .bind(context_type)
    .bind(context_id)
    .bind(account_chain.to_vec())
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Canvas `ContextExternalTool.standardize_url` (context_external_tool.rb:
/// 957-967): trim, default the scheme to http, normalize — the `url` crate
/// lowercases hosts and drops default ports.
fn standardize_url(raw: &str) -> Option<Url> {
    let trimmed = raw.trim();
    let prefixed;
    let input = if trimmed.contains("://") {
        trimmed
    } else {
        prefixed = format!("http://{trimmed}");
        &prefixed
    };
    Url::parse(input).ok()
}

/// The query pairs of a standardized URL, sorted — Canvas standardize_url
/// sorts them so exact matching is order-insensitive.
fn sorted_query(url: &Url) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|pair| (pair.0.into_owned(), pair.1.into_owned()))
        .collect();
    pairs.sort();
    pairs
}

fn path_or_root(url: &Url) -> &str {
    if url.path().is_empty() {
        "/"
    } else {
        url.path()
    }
}

/// Canvas `ContextExternalTool#matches_url?` (context_external_tool.rb:
/// 1017-1037): scheme, host, port and path on standardized URLs; the sorted
/// query only on the exact pass (`match_queries_exactly:`).
fn url_matches(tool_url: &str, launch: &Url, exact_query: bool) -> bool {
    let Some(tool) = standardize_url(tool_url) else {
        return false;
    };
    if tool.scheme() != launch.scheme()
        || tool.host_str() != launch.host_str()
        || tool.port() != launch.port()
        || path_or_root(&tool) != path_or_root(launch)
    {
        return false;
    }
    if exact_query {
        sorted_query(&tool) == sorted_query(launch)
    } else {
        true
    }
}

/// Canvas `ContextExternalTool#matches_tool_domain?`
/// (context_external_tool.rb:1039-1048): the launch host, with its explicit
/// port when present, must end with `.{domain}`.
fn domain_matches(tool_domain: &str, launch: &Url) -> bool {
    let domain = tool_domain
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_lowercase();
    if domain.is_empty() {
        return false;
    }
    let Some(host) = launch.host_str() else {
        return false;
    };
    let host_with_port = match launch.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    format!(".{host_with_port}").ends_with(&format!(".{domain}"))
}

/// Canvas `ContextExternalTool#matches_host?` (context_external_tool.rb:
/// 1010-1015): the launch host equals the tool URL's host, or the tool
/// domain matches.
fn tool_matches_host(tool: &JwtToolRow, launch: &Url) -> bool {
    if tool
        .domain
        .as_deref()
        .is_some_and(|domain| domain_matches(domain, launch))
    {
        return true;
    }
    standardize_url(&tool.url).is_some_and(|tool_url| tool_url.host_str() == launch.host_str())
}

/// Canvas `Lti::ToolFinder.from_url` scoped to the context chain
/// (app/services/lti/tool_finder.rb:197-247): the preferred tool wins when
/// it is available in the searched contexts and matches the launch host —
/// or outright when no launch URL is given (:216-217); otherwise the first
/// exact-URL, then query-ignoring-URL, then domain match, in candidate
/// order.
fn pick_tool(
    candidates: &[JwtToolRow],
    launch_url: Option<&Url>,
    tool_id: Option<i64>,
) -> Option<JwtToolRow> {
    if let Some(tool) = tool_id.and_then(|id| candidates.iter().find(|tool| tool.id == id)) {
        match launch_url {
            None => return Some(tool.clone()),
            Some(launch) if tool_matches_host(tool, launch) => {
                return Some(tool.clone());
            }
            Some(_) => {}
        }
    }
    let launch = launch_url?;
    candidates
        .iter()
        .find(|tool| url_matches(&tool.url, launch, true))
        .or_else(|| {
            candidates
                .iter()
                .find(|tool| url_matches(&tool.url, launch, false))
        })
        .or_else(|| {
            candidates.iter().find(|tool| {
                tool.domain
                    .as_deref()
                    .is_some_and(|d| domain_matches(d, launch))
            })
        })
        .cloned()
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
        // Route 15: GET show (API path) — Canvas routes.rb:1704 nests show
        // under the account scope as well; account reads are admin-gated,
        // matching list_account_tools.
        .route(
            "/accounts/{account_id}/external_tools/{id}",
            get(show_account_tool)
                .put(update_account_tool)
                .delete(delete_account_tool),
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
        // Brief route 11 — Canvas routes.rb:1703, course-scoped nav tools.
        .route(
            "/courses/{course_id}/external_tools/visible_course_nav_tools",
            get(visible_course_nav_tools),
        )
        // Canvas routes.rb:1695-1696 — top_nav_favorites exist only for
        // accounts, and only POST (add) + DELETE (remove). There is no list
        // route and no course-scoped variant.
        .route(
            "/accounts/{account_id}/external_tools/top_nav_favorites/{tool_id}",
            post(add_account_top_nav_favorite).delete(remove_account_top_nav_favorite),
        )
        // New: migration_info (routes 12-13) — Canvas routes.rb:1707 nests
        // it under the tool: .../external_tools/:external_tool_id/migration_info
        .route(
            "/courses/{course_id}/external_tools/{id}/migration_info",
            get(course_migration_info),
        )
        .route(
            "/accounts/{account_id}/external_tools/{id}/migration_info",
            get(account_migration_info),
        )
        // New: group tools (route 16)
        .route("/groups/{group_id}/external_tools", get(list_group_tools))
        .route(
            "/groups/{group_id}/external_tools/{id}",
            get(show_group_tool),
        )
        // New: eula_launch (route 17, stub) — Canvas routes.rb:1216 is
        // GET .../external_tools/:context_external_tool_id/eula_launch
        .route(
            "/courses/{course_id}/external_tools/{id}/eula_launch",
            get(eula_launch),
        )
}

/// Router for the Canvas `/api/lti` jwt_token routes.
///
/// Canvas registers `get "#{prefix}/jwt_token"` for the course and account
/// prefixes inside the `/api/lti` route set (config/routes.rb:3153-3160), so
/// the paths carry the prefix and the router mounts through
/// `lti_advantage_router` (star-http merges that at the application root).
pub fn advantage_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/courses/{course_id}/jwt_token",
            get(course_jwt_token),
        )
        .route(
            "/api/lti/accounts/{account_id}/jwt_token",
            get(account_jwt_token),
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

/// `GET /accounts/{account_id}/external_tools/{id}` — show one account tool
/// (brief route 15, `external_tools#show` API path). Account-scoped reads are
/// admin-gated, mirroring `list_account_tools`.
async fn show_account_tool(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let tool = find_account_tool(&state.db, account_id, id).await?;
    let favorites = rce_favorite_ids(&state.db, "Account", account_id).await?;
    let top_nav = top_nav_favorite_ids(&state.db, "Account", account_id).await?;
    let is_top_nav = has_placement(&tool.tool_configuration, "top_navigation")
        .then(|| top_nav.contains(&tool.id));
    Ok(Json(tool_json(
        &tool,
        favorites.contains(&tool.id),
        is_top_nav,
    )))
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
        data.extend(visible_nav_tools_for_course(&state.db, course_id).await?);
    }

    Ok(Json(data).into_response())
}

/// Course nav tools enriched with `context_id` / `context_name`, shared by
/// `all_visible_nav_tools` (global, `context_codes[]`) and
/// `visible_course_nav_tools` (brief route 11, course-scoped).
///
/// Canvas external_tools_controller.rb:1854-1861:
/// `external_tools_json_for_courses([@context])` — enabled + visible tools,
/// filtered to the `course_navigation` placement, each result enriched with
/// the course id and name.
async fn visible_nav_tools_for_course(db: &PgPool, course_id: i64) -> Result<Vec<Value>, ApiError> {
    let course_name: (String,) = sqlx::query_as("SELECT name FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_one(db)
        .await?;
    let course_name = course_name.0;

    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE context_type = 'Course' AND context_id = $1 \
         AND workflow_state <> 'deleted' AND enabled AND visible ORDER BY id ASC"
    );
    let rows: Vec<ExternalToolRow> = sqlx::query_as(&sql).bind(course_id).fetch_all(db).await?;

    let mut data = Vec::new();
    for tool in rows {
        let has_placements = tool.tool_configuration.get("placements").is_some();
        if has_placements && !has_placement(&tool.tool_configuration, "course_navigation") {
            continue;
        }
        let mut value = tool_json(&tool, false, None);
        value["context_id"] = json!(course_id);
        value["context_name"] = json!(course_name);
        data.push(value);
    }
    Ok(data)
}

/// `GET /courses/{course_id}/external_tools/visible_course_nav_tools` — brief
/// route 11 (Canvas external_tools_controller.rb:1854-1861). Only the Course
/// context is supported by Canvas; here the path is course-scoped so the 400
/// "Only course context is supported" branch is unreachable.
async fn visible_course_nav_tools(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let data = visible_nav_tools_for_course(&state.db, course_id).await?;
    Ok(Json(data))
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
    // The lookup is the 404 guard for a tool outside this course.
    find_course_tool(&state.db, course_id, id).await?;
    let updated = apply_update(&state.db, id, payload).await?;
    Ok(Json(tool_json(&updated, false, None)))
}

/// `DELETE /courses/{course_id}/external_tools/{id}` — soft-delete a course tool.
async fn delete_course_tool(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course_tools(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let tool = find_course_tool(&state.db, course_id, id).await?;
    // Canvas delete_tool renders the destroyed tool's JSON (controller.rb:
    // 2143-2149), with workflow_state 'deleted' on the returned record.
    sqlx::query(
        "UPDATE external_tools SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;
    let mut deleted = tool_json(&tool, false, None);
    deleted["workflow_state"] = json!("deleted");
    Ok(Json(deleted))
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
    Ok(Json(tool_json(
        &updated,
        favorites.contains(&updated.id),
        None,
    )))
}

/// `DELETE /accounts/{account_id}/external_tools/{id}` — soft-delete an account tool.
async fn delete_account_tool(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let tool = find_account_tool(&state.db, account_id, id).await?;
    // Canvas delete_tool renders the destroyed tool's JSON (controller.rb:
    // 2143-2149).
    sqlx::query(
        "UPDATE external_tools SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;
    let mut deleted = tool_json(&tool, false, None);
    deleted["workflow_state"] = json!("deleted");
    Ok(Json(deleted))
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
         not_selectable = COALESCE($27, not_selectable), \
         updated_at = now() \
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
    )
    .await?
    {
        Ok(t) => t,
        Err(r) => return Ok(r),
    };
    let favorites = rce_favorite_ids(&state.db, "Account", account_id).await?;
    Ok(Json(tool_json(&tool, favorites.contains(&tool.id), None)).into_response())
}

/// Shared create-with-verification path. Only the 7 params Canvas permits
/// (external_tools_controller.rb:1594-1602) are honored; the URL comes from
/// the app center config URL derived from app_center_id + config_settings
/// (controller.rb:1605: `config_url = app_api.get_app_config_url(...)`,
/// `config_type = "by_url"`).
async fn create_tool_verified(
    pool: &PgPool,
    auth: &AuthUser,
    payload: CreateToolWithVerificationRequest,
    context_type: &str,
    context_id: i64,
    course_id: Option<i64>,
    account_id: Option<i64>,
) -> Result<Result<ExternalToolRow, Response>, ApiError> {
    // Canvas accepts context_id/context_type params (controller.rb:1599)
    // but the tool is created in the route's context; when the caller
    // supplies them they must agree with that context.
    if let Some(provided) = payload.context_id {
        if provided != context_id {
            return Ok(Err(tool_errors(vec![format!(
                "Context id {provided} does not match route context {context_id}"
            )])));
        }
    }
    if let Some(provided) = payload.context_type.as_deref() {
        if !provided.eq_ignore_ascii_case(context_type) {
            return Ok(Err(tool_errors(vec![format!(
                "Context type {provided} does not match route context {context_type}"
            )])));
        }
    }

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

    // Canvas sets config_type "by_url" with a URL derived from the app
    // center; star-lms has no AppCenter, so config_settings must carry the
    // tool url itself.
    let url = payload
        .config_settings
        .as_ref()
        .and_then(|cs| serde_json::from_str::<Value>(cs).ok())
        .and_then(|v| v.get("url").and_then(|u| u.as_str()).map(String::from));
    let Some(url) = url else {
        return Ok(Err(tool_errors(vec!["Url can't be blank".to_string()])));
    };

    let sql = format!(
        "INSERT INTO external_tools \
         (name, url, consumer_key, shared_secret, privacy_level, workflow_state, \
          course_id, account_id, context_type, context_id, tool_type, lti_version, \
          enabled, visible, open_in_new_tab, ownership_type, tool_configuration, \
          custom_fields, created_by_id) \
         VALUES ($1, $2, $3, $4, 'anonymous', 'active', \
                 $5, $6, $7, $8, 'lti', '1.3', \
                 true, true, false, $9, '{{}}', '{{}}', $10) \
         RETURNING {TOOL_COLUMNS}"
    );
    let tool: ExternalToolRow = sqlx::query_as(&sql)
        .bind(name)
        .bind(url)
        .bind(payload.consumer_key.as_ref())
        .bind(payload.shared_secret.as_ref())
        .bind(course_id)
        .bind(account_id)
        .bind(context_type)
        .bind(context_id)
        .bind("account")
        .bind(auth.id)
        .fetch_one(pool)
        .await?;

    Ok(Ok(tool))
}

// --------------------------------------------------------------------------
// New handlers: sessionless_launch (routes 7-8, stub 503)
// --------------------------------------------------------------------------

/// `GET /courses/{course_id}/external_tools/sessionless_launch` — stub 503
/// behind session auth (Canvas' SessionScopedLaunch; anonymous → 401).
async fn course_sessionless_launch(_auth: AuthUser) -> StatusCode {
    StatusCode::SERVICE_UNAVAILABLE
}

/// `GET /accounts/{account_id}/external_tools/sessionless_launch` — stub 503
/// behind session auth (anonymous → 401).
async fn account_sessionless_launch(_auth: AuthUser) -> StatusCode {
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

/// `POST /accounts/{account_id}/external_tools/top_nav_favorites/{tool_id}` —
/// Canvas `add_top_nav_favorite` (external_tools_controller.rb:1760-1791):
/// requires manage_lti_add, 404 when the tool is unknown, 400 when the tool
/// lacks the top_navigation placement, 400 when more than 2 tools would be
/// favorited; on success renders `{top_nav_favorite_tool_ids: [...]}`.
async fn add_account_top_nav_favorite(
    State(state): State<AppState>,
    Path((account_id, tool_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let tool = find_account_tool(&state.db, account_id, tool_id).await?;

    // Canvas: "Tool does not have top_navigation placement" → 400.
    if !has_placement(&tool.tool_configuration, "top_navigation") {
        return Ok(bad_request(
            "Tool does not have top_navigation placement".to_string(),
        ));
    }

    let mut ids = top_nav_favorite_ids(&state.db, "Account", account_id).await?;
    if !ids.contains(&tool_id) {
        ids.push(tool_id);
    }
    // Canvas: "Cannot have more than 2 favorited tools" → 400.
    if ids.len() > 2 {
        return Ok(bad_request(
            "Cannot have more than 2 favorited tools".to_string(),
        ));
    }
    set_top_nav_favorites(&state.db, "Account", account_id, &ids).await?;
    Ok(Json(json!({ "top_nav_favorite_tool_ids": ids })).into_response())
}

/// `DELETE /accounts/{account_id}/external_tools/top_nav_favorites/{tool_id}` —
/// Canvas `remove_top_nav_favorite` (external_tools_controller.rb:1793-1802):
/// requires manage_lti_delete, removes the id when present, always renders
/// `{top_nav_favorite_tool_ids: [...]}`.
async fn remove_account_top_nav_favorite(
    State(state): State<AppState>,
    Path((account_id, tool_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let mut ids = top_nav_favorite_ids(&state.db, "Account", account_id).await?;
    ids.retain(|id| *id != tool_id);
    set_top_nav_favorites(&state.db, "Account", account_id, &ids).await?;
    Ok(Json(json!({ "top_nav_favorite_tool_ids": ids })))
}

// --------------------------------------------------------------------------
// New handlers: migration_info (routes 12-13)
// --------------------------------------------------------------------------

/// `GET /courses/{course_id}/external_tools/{id}/migration_info` — Canvas
/// `migration_info` (external_tools_controller.rb:994-1014): requires
/// read_as_admin, finds the tool by id, and renders
/// `{migration_running: false}` when no migration is running.
async fn course_migration_info(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_manage_course_tools(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    find_course_tool(&state.db, course_id, id).await?;
    Ok(Json(json!({ "migration_running": false })))
}

/// `GET /accounts/{account_id}/external_tools/{id}/migration_info` — the
/// account twin of the course route above.
async fn account_migration_info(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    find_account_tool(&state.db, account_id, id).await?;
    Ok(Json(json!({ "migration_running": false })))
}

// --------------------------------------------------------------------------
// New handlers: group tools (route 16)
// --------------------------------------------------------------------------

/// Canvas group read gate (app/models/group.rb:662): course-level groups
/// grant :read when the containing course grants :read, so course
/// teachers/admins and group members may both read group tools.
async fn can_read_group_tools(
    state: &AppState,
    group_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    let course: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT course_id FROM groups WHERE id = $1")
            .bind(group_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((course_id,)) = course else {
        return Err(ApiError::NotFound);
    };

    let member: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM group_memberships \
         WHERE group_id = $1 AND user_id = $2 AND workflow_state <> 'deleted' LIMIT 1",
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    if member.is_some() {
        return Ok(());
    }

    if let Some(course_id) = course_id {
        if can_read_course(&state.db, auth, course_id).await? {
            return Ok(());
        }
    }
    Err(ApiError::Forbidden)
}

/// `GET /groups/{group_id}/external_tools` — list tools for a group.
async fn list_group_tools(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    can_read_group_tools(&state, group_id, &auth).await?;

    // Canvas resolves a Group's tools through its course
    // (Lti::ToolFinderUtils.contexts_to_search: Group -> [:self, :recursive];
    // Group#context_external_tools is none, group.rb:196-198, so the course
    // chain supplies the tools).
    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE context_type = 'Course' AND context_id = \
               (SELECT course_id FROM groups WHERE id = $1) \
           AND workflow_state <> 'deleted' \
         ORDER BY id ASC"
    );
    let rows: Vec<ExternalToolRow> = sqlx::query_as(&sql)
        .bind(group_id)
        .fetch_all(&state.db)
        .await?;

    Ok(Json(
        rows.iter().map(|t| tool_json(t, false, None)).collect(),
    ))
}

/// `GET /groups/{group_id}/external_tools/{id}` — show one group tool.
async fn show_group_tool(
    State(state): State<AppState>,
    Path((group_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    can_read_group_tools(&state, group_id, &auth).await?;

    // Canvas resolves the tool through the group's course chain.
    let sql = format!(
        "SELECT {TOOL_COLUMNS} FROM external_tools \
         WHERE id = $1 AND context_type = 'Course' \
           AND context_id = (SELECT course_id FROM groups WHERE id = $2) \
           AND workflow_state <> 'deleted'"
    );
    let tool: Option<ExternalToolRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(group_id)
        .fetch_optional(&state.db)
        .await?;
    tool.map(|t| Json(tool_json(&t, false, None)))
        .ok_or(ApiError::NotFound)
}

// --------------------------------------------------------------------------
// New handlers: eula_launch (route 17, stub)
// --------------------------------------------------------------------------

/// `GET /courses/{course_id}/external_tools/{id}/eula_launch` — brief route
/// 17 (Canvas `lti/eula_launch_controller.rb:20-54`). Guard order:
/// `require_tool` → 404, `require_access_to_context` → 403,
/// `require_1_3_tool` → 400. Canvas renders the LTI launch HTML; this
/// workspace has no LTI launch renderer yet, so the API returns the 200
/// placeholder from the brief until launch rendering exists.
async fn eula_launch(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let tool = find_course_tool(&state.db, course_id, id).await?;
    // require_1_3_tool: EULA launches are an LTI 1.3 feature.
    if tool.lti_version != "1.3" {
        return Ok(bad_request(
            "EULA launch requires an LTI 1.3 tool".to_string(),
        ));
    }
    Ok(Json(json!({ "message": "eula_launch not implemented" })).into_response())
}

// ---------------------------------------------------------------------------
// New handlers: jwt_token (Canvas /api/lti routes)
// ---------------------------------------------------------------------------

/// `GET /api/lti/courses/{course_id}/jwt_token` — a signed launch token for
/// a tool available in the course.
///
/// Canvas external_tools_controller.rb:613-620 skips `require_user` for
/// jwt_token, but `require_access_to_context` (:2116-2121) still demands
/// course `grants_right?(:read)`, so the endpoint is member/admin gated in
/// practice.
async fn course_jwt_token(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<JwtTokenQuery>,
) -> Result<Json<Value>, ApiError> {
    require_course(&state.db, course_id).await?;
    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let title: Option<(String,)> = sqlx::query_as("SELECT name FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;
    issue_jwt_token(
        &state,
        "Course",
        course_id,
        course_account_id(&state.db, course_id).await?,
        title.map(|(name,)| name),
        &params,
    )
    .await
}

/// `GET /api/lti/accounts/{account_id}/jwt_token` — a signed launch token
/// for a tool available in the account.
///
/// Canvas `require_access_to_context` (:2116-2118) only calls `require_user`
/// for account contexts: any logged-in user passes, no admin role is
/// checked.
async fn account_jwt_token(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    _auth: AuthUser,
    Query(params): Query<JwtTokenQuery>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    let title: Option<(String,)> = sqlx::query_as("SELECT name FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(&state.db)
        .await?;
    issue_jwt_token(
        &state,
        "Account",
        account_id,
        Some(account_id),
        title.map(|(name,)| name),
        &params,
    )
    .await
}

/// Shared jwt_token body — Canvas external_tools_controller.rb:1678-1690:
/// find the tool by launch URL (or the preferred id) in the context chain,
/// 404 when nothing matches, then render
/// `{ jwt_token: Canvas::Security.create_jwt(params, nil, shared_secret) }`.
async fn issue_jwt_token(
    state: &AppState,
    context_type: &str,
    context_id: i64,
    chain_root: Option<i64>,
    title: Option<String>,
    params: &JwtTokenQuery,
) -> Result<Json<Value>, ApiError> {
    let chain = account_chain(&state.db, chain_root).await?;
    let candidates = launch_candidates(&state.db, context_type, context_id, &chain).await?;
    let launch_url = params
        .tool_launch_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .and_then(standardize_url);
    let tool =
        pick_tool(&candidates, launch_url.as_ref(), params.tool_id).ok_or(ApiError::NotFound)?;

    // The token is signed with the tool's shared secret
    // (Canvas::Security.create_jwt, gems/canvas_security/lib/
    // canvas_security.rb:238-247 — HS256, no exp claim because jwt_token
    // passes expires=nil). An LTI 1.3-only tool has no secret; Canvas
    // raises there, this answers the Canvas 400 envelope instead.
    let shared_secret = tool
        .shared_secret
        .clone()
        .filter(|secret| !secret.is_empty())
        .ok_or_else(|| ApiError::BadRequest("Tool has no shared secret".to_string()))?;

    // Canvas passes the launch params through after rejecting oauth_* ones
    // and adds consumer_key + iat (controller.rb:1685-1690). The payload
    // below is the workspace's scoped launch set — full BLTI variable
    // expansion belongs to the LTI launch renderer (see the
    // sessionless_launch stubs). resource_link_id substitutes the tool id
    // for Canvas' opaque identifier (context_external_tool.rb:1192-1215);
    // the workspace schema has no opaque id column.
    let payload = json!({
        "lti_message_type": "basic-lti-launch-request",
        "lti_version": "LTI-1p0",
        "context_id": format!("{}_{}", context_type.to_lowercase(), context_id),
        "context_title": title,
        "resource_link_id": format!(
            "{}_{}_tool_{}",
            context_type.to_lowercase(),
            context_id,
            tool.id
        ),
        "consumer_key": tool.consumer_key,
        "iat": Utc::now().timestamp(),
    });

    let token = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &payload,
        &jsonwebtoken::EncodingKey::from_secret(shared_secret.as_bytes()),
    )
    .map_err(|e| ApiError::Db(e.to_string()))?;

    Ok(Json(json!({ "jwt_token": token })))
}
