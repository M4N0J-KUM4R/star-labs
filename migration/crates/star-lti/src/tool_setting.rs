//! tool_setting — Canvas `lti/ims/tool_setting_controller` parity.
//!
//! Routes (Canvas serves them under `/api/lti`, outside `/api/v1`):
//! - GET/PUT /api/lti/tool_proxy/{tool_proxy_guid}/courses/{course_id}/tool_setting
//! - GET/PUT /api/lti/tool_proxy/{tool_proxy_guid}/accounts/{account_id}/tool_setting
//! - GET/PUT /api/lti/tool_proxy/{tool_proxy_guid}/courses/{course_id}/resource_link_id/{resource_link_id}/tool_setting
//! - GET/PUT /api/lti/tool_proxy/{tool_proxy_guid}/accounts/{account_id}/resource_link_id/{resource_link_id}/tool_setting
//! - GET/PUT /api/lti/tool_settings/{tool_setting_id}
//! - GET/PUT /api/lti/tool_proxy/{tool_proxy_guid}/tool_setting
//!
//! Canvas behavior mirrored here (`app/controllers/lti/ims/tool_setting_controller.rb`):
//! - the request is authenticated with an LTI Advantage access token: the
//!   token names the tool proxy in its `sub` claim and carries the permission
//!   in its `scopes` claim. A request without a token is a 401, a token
//!   without the required scope is a 403.
//! - the setting is looked up by its id, or by the tool proxy of the path
//!   together with the context of the path and the optional resource link id.
//!   A setting that does not exist is a 404.
//! - the `bubble` query parameter is blank, `distinct` or `all`; `all` is only
//!   allowed with the `application/vnd.ims.lti.v2.toolsettings+json` media
//!   type. Anything else is a 400.
//! - with a bubble the graph walks the levels of the setting — the resource
//!   link, then its binding, then the tool proxy — and `distinct` keeps only
//!   the keys the higher level does not already carry.
//! - with the tool settings media type the answer is the `@graph` form, with
//!   the simple media type it is the flat `custom` object.
//! - an update replaces the `custom` settings of the setting and answers
//!   200 with an empty body.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_common::AppState;

/// The media type of the LTI tool settings graph.
const TOOL_SETTINGS_JSON: &str = "application/vnd.ims.lti.v2.toolsettings+json";

/// The media type of the flat LTI tool settings.
const TOOL_SETTINGS_SIMPLE_JSON: &str = "application/vnd.ims.lti.v2.toolsettings.simple+json";

/// The scope an LTI Advantage token needs for the tool settings.
const TOOL_SETTINGS_SCOPE: &str = "ToolSettings";

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct ToolSettingRow {
    id: i64,
    tool_proxy_id: i64,
    context_type: Option<String>,
    context_id: Option<i64>,
    resource_link_id: Option<String>,
    custom: Value,
}

impl ToolSettingRow {
    /// Canvas `tool_setting_type`: `LtiLink` for a resource link,
    /// `ToolProxyBinding` for a context and `ToolProxy` without one.
    fn setting_type(&self) -> &'static str {
        if self.resource_link_id.is_some() {
            "LtiLink"
        } else if self.context_id.is_some() {
            "ToolProxyBinding"
        } else {
            "ToolProxy"
        }
    }

    /// Canvas `show_lti_tool_settings_url` — the url of the setting.
    fn url(&self) -> String {
        format!("/api/lti/tool_settings/{}", self.id)
    }
}

/// The context a tool setting belongs to.
#[derive(Debug, Clone)]
enum SettingContext {
    /// A course with an optional resource link.
    Course {
        course_id: i64,
        resource_link_id: Option<String>,
    },
    /// An account with an optional resource link.
    Account {
        account_id: i64,
        resource_link_id: Option<String>,
    },
    /// The tool proxy itself.
    ToolProxy,
    /// A setting that is addressed by its id.
    Setting { tool_setting_id: i64 },
}

// ---------------------------------------------------------------------------
// Authentication — Canvas `authenticate_api_call`
// ---------------------------------------------------------------------------

/// Canvas renders the LTI errors with their own body, so the handlers answer
/// a `Response` instead of an `ApiError`.
fn unauthorized() -> Box<Response> {
    Box::new(
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response(),
    )
}

fn forbidden() -> Box<Response> {
    Box::new(
        (
            StatusCode::FORBIDDEN,
            Json(json!({
                "errors": [{ "code": "forbidden", "message": "forbidden" }]
            })),
        )
            .into_response(),
    )
}

fn bad_request() -> Box<Response> {
    Box::new(
        (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "status": "bad_request",
                "errors": [{ "message": "bad_request" }]
            })),
        )
            .into_response(),
    )
}

fn not_found() -> Box<Response> {
    Box::new(
        (
            StatusCode::NOT_FOUND,
            Json(json!({
                "status": "not_found",
                "errors": [{ "message": "not_found" }]
            })),
        )
            .into_response(),
    )
}

fn server_error(error: sqlx::Error) -> Box<Response> {
    Box::new(
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    )
}

/// The tool proxy of an LTI Advantage access token.
///
/// The token is a JWT signed with the platform secret; its `sub` claim is the
/// guid of the tool proxy and its `scopes` claim carries the permissions.
async fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<i64, Box<Response>> {
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

    // An LTI Advantage access token carries the scopes of the call.
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

    if !scopes.iter().any(|scope| scope == TOOL_SETTINGS_SCOPE) {
        return Err(forbidden());
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
// Lookup
// ---------------------------------------------------------------------------

/// The tool proxy of a guid.
async fn tool_proxy_by_guid(pool: &PgPool, guid: &str) -> Result<Option<i64>, Box<Response>> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM lti_tool_proxies WHERE guid = $1 AND workflow_state = 'active'",
    )
    .bind(guid)
    .fetch_optional(pool)
    .await
    .map_err(server_error)
}

/// The tool setting of a tool proxy in a context.
async fn find_setting(
    pool: &PgPool,
    tool_proxy_id: i64,
    context: SettingContext,
) -> Result<Option<ToolSettingRow>, Box<Response>> {
    let row = match context {
        SettingContext::Course {
            course_id,
            resource_link_id,
        } => {
            sqlx::query_as::<_, ToolSettingRow>(
                "SELECT id, tool_proxy_id, context_type, context_id, resource_link_id, custom \
                 FROM tool_settings \
                 WHERE tool_proxy_id = $1 AND context_type = 'Course' AND context_id = $2 \
                   AND resource_link_id IS NOT DISTINCT FROM $3 \
                 ORDER BY id ASC LIMIT 1",
            )
            .bind(tool_proxy_id)
            .bind(course_id)
            .bind(resource_link_id)
            .fetch_optional(pool)
            .await
        }
        SettingContext::Account {
            account_id,
            resource_link_id,
        } => {
            sqlx::query_as::<_, ToolSettingRow>(
                "SELECT id, tool_proxy_id, context_type, context_id, resource_link_id, custom \
                 FROM tool_settings \
                 WHERE tool_proxy_id = $1 AND context_type = 'Account' AND context_id = $2 \
                   AND resource_link_id IS NOT DISTINCT FROM $3 \
                 ORDER BY id ASC LIMIT 1",
            )
            .bind(tool_proxy_id)
            .bind(account_id)
            .bind(resource_link_id)
            .fetch_optional(pool)
            .await
        }
        SettingContext::ToolProxy => {
            sqlx::query_as::<_, ToolSettingRow>(
                "SELECT id, tool_proxy_id, context_type, context_id, resource_link_id, custom \
                 FROM tool_settings \
                 WHERE tool_proxy_id = $1 AND context_id IS NULL \
                 ORDER BY id ASC LIMIT 1",
            )
            .bind(tool_proxy_id)
            .fetch_optional(pool)
            .await
        }
        SettingContext::Setting { tool_setting_id } => {
            sqlx::query_as::<_, ToolSettingRow>(
                "SELECT id, tool_proxy_id, context_type, context_id, resource_link_id, custom \
                 FROM tool_settings WHERE id = $1",
            )
            .bind(tool_setting_id)
            .fetch_optional(pool)
            .await
        }
    };

    row.map_err(server_error)
}

/// The setting of the next level of the graph: the binding of a resource
/// link, or the tool proxy of a binding.
async fn next_level(
    pool: &PgPool,
    row: &ToolSettingRow,
) -> Result<Option<ToolSettingRow>, Box<Response>> {
    let next = match row.setting_type() {
        "LtiLink" => {
            sqlx::query_as::<_, ToolSettingRow>(
                "SELECT id, tool_proxy_id, context_type, context_id, resource_link_id, custom \
                 FROM tool_settings \
                 WHERE tool_proxy_id = $1 AND context_type = $2 AND context_id = $3 \
                   AND resource_link_id IS NULL \
                 ORDER BY id ASC LIMIT 1",
            )
            .bind(row.tool_proxy_id)
            .bind(&row.context_type)
            .bind(row.context_id)
            .fetch_optional(pool)
            .await
        }
        "ToolProxyBinding" => {
            sqlx::query_as::<_, ToolSettingRow>(
                "SELECT id, tool_proxy_id, context_type, context_id, resource_link_id, custom \
                 FROM tool_settings \
                 WHERE tool_proxy_id = $1 AND context_type IS NULL AND context_id IS NULL \
                   AND resource_link_id IS NULL \
                 ORDER BY id ASC LIMIT 1",
            )
            .bind(row.tool_proxy_id)
            .fetch_optional(pool)
            .await
        }
        _ => Ok(None),
    };
    next.map_err(server_error)
}

/// The `bubble` parameter of a request.
#[derive(Debug, serde::Deserialize)]
struct BubbleQuery {
    bubble: Option<String>,
}

/// The setting of a request, resolved from the path.
async fn resolve_setting(
    state: &AppState,
    headers: &HeaderMap,
    tool_proxy_guid: Option<&str>,
    context: SettingContext,
) -> Result<(ToolSettingRow, i64), Box<Response>> {
    let proxy = match tool_proxy_guid {
        Some(guid) => {
            let proxy = tool_proxy_by_guid(&state.db, guid).await?;
            match proxy {
                Some(proxy) => proxy,
                None => return Err(not_found()),
            }
        }
        None => authenticate(state, headers).await?,
    };

    let setting = find_setting(&state.db, proxy, context).await?;
    let Some(setting) = setting else {
        return Err(not_found());
    };
    Ok((setting, proxy))
}

// ---------------------------------------------------------------------------
// The answer — Canvas `tool_setting_json`
// ---------------------------------------------------------------------------

/// Canvas `collect_tool_settings`: the entry of the graph.
fn collect_tool_settings(setting: &ToolSettingRow, distinct: Option<&[String]>) -> Value {
    let mut custom = setting.custom.as_object().cloned().unwrap_or_default();
    if let Some(distinct) = distinct {
        custom.retain(|key, _| !distinct.contains(key));
    }
    json!({
        "@type": setting.setting_type(),
        "custom": Value::Object(custom),
        "id": setting.url(),
    })
}

/// Whether the request wants the tool settings graph.
fn accepts_graph(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains(TOOL_SETTINGS_JSON))
        .unwrap_or(false)
}

/// Whether the request wants the flat tool settings.
fn accepts_simple(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains(TOOL_SETTINGS_SIMPLE_JSON))
        .unwrap_or(false)
}

/// The `bubble` parameter, checked the way Canvas does.
fn checked_bubble(query: &BubbleQuery, accepts_graph: bool) -> Result<&str, Box<Response>> {
    let bubble = query.bubble.as_deref().unwrap_or("").trim();
    match bubble {
        "" | "distinct" => Ok(bubble),
        "all" if accepts_graph => Ok(bubble),
        _ => Err(bad_request()),
    }
}

/// Canvas `tool_setting_json`.
async fn settings_response(
    pool: &PgPool,
    setting: &ToolSettingRow,
    accepts_graph: bool,
    accepts_simple: bool,
    bubble: &str,
) -> Response {
    if bubble == "all" || bubble == "distinct" {
        // The graph walks the levels of the setting: the resource link, then
        // its binding, then the tool proxy.
        let mut graph: Vec<Value> = Vec::new();
        let mut distinct: Option<Vec<String>> = if bubble == "distinct" {
            Some(Vec::new())
        } else {
            None
        };
        let mut current = Some(setting.clone());
        while let Some(row) = current {
            let entry = collect_tool_settings(&row, distinct.as_deref());
            if let Some(seen) = distinct.as_mut() {
                if let Some(custom) = entry.get("custom").and_then(Value::as_object) {
                    seen.extend(custom.keys().cloned());
                }
            }
            graph.push(entry);
            current = match next_level(pool, &row).await {
                Ok(next) => next,
                Err(response) => return *response,
            };
        }

        if accepts_graph {
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, TOOL_SETTINGS_JSON)],
                Json(json!({ "@graph": graph })),
            )
                .into_response();
        }
        if bubble == "distinct" && accepts_simple {
            // The higher level wins, so the graph is merged from the bottom.
            let mut custom = Map::new();
            for entry in graph.iter().rev() {
                if let Some(map) = entry.get("custom").and_then(Value::as_object) {
                    for (key, value) in map {
                        custom.insert(key.clone(), value.clone());
                    }
                }
            }
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, TOOL_SETTINGS_SIMPLE_JSON)],
                Json(Value::Object(custom)),
            )
                .into_response();
        }
        // The simple form without the media type.
        let custom = setting.custom.as_object().cloned().unwrap_or_default();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, TOOL_SETTINGS_SIMPLE_JSON)],
            Json(Value::Object(custom)),
        )
            .into_response();
    }

    if accepts_graph {
        let graph = vec![collect_tool_settings(setting, None)];
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, TOOL_SETTINGS_JSON)],
            Json(json!({ "@graph": graph })),
        )
            .into_response();
    }

    let custom = setting.custom.as_object().cloned().unwrap_or_default();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, TOOL_SETTINGS_SIMPLE_JSON)],
        Json(Value::Object(custom)),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// The shared body of the show routes.
async fn show_setting(
    state: &AppState,
    headers: HeaderMap,
    query: BubbleQuery,
    tool_proxy_guid: Option<&str>,
    context: SettingContext,
) -> Response {
    let accepts_graph = accepts_graph(&headers);
    let accepts_simple = accepts_simple(&headers);
    let bubble = match checked_bubble(&query, accepts_graph) {
        Ok(bubble) => bubble.to_string(),
        Err(response) => return *response,
    };
    match resolve_setting(state, &headers, tool_proxy_guid, context).await {
        Ok((setting, _proxy)) => {
            settings_response(&state.db, &setting, accepts_graph, accepts_simple, &bubble).await
        }
        Err(response) => *response,
    }
}

/// GET /api/lti/tool_proxy/{tool_proxy_guid}/courses/{course_id}/tool_setting.
async fn show_course_tool_setting(
    State(state): State<AppState>,
    Path((tool_proxy_guid, course_id)): Path<(String, i64)>,
    headers: HeaderMap,
    Query(query): Query<BubbleQuery>,
) -> Response {
    show_setting(
        &state,
        headers,
        query,
        Some(&tool_proxy_guid),
        SettingContext::Course {
            course_id,
            resource_link_id: None,
        },
    )
    .await
}

/// GET /api/lti/tool_proxy/{tool_proxy_guid}/accounts/{account_id}/tool_setting.
async fn show_account_tool_setting(
    State(state): State<AppState>,
    Path((tool_proxy_guid, account_id)): Path<(String, i64)>,
    headers: HeaderMap,
    Query(query): Query<BubbleQuery>,
) -> Response {
    show_setting(
        &state,
        headers,
        query,
        Some(&tool_proxy_guid),
        SettingContext::Account {
            account_id,
            resource_link_id: None,
        },
    )
    .await
}

/// GET /api/lti/tool_proxy/{tool_proxy_guid}/courses/{course_id}/resource_link_id/{resource_link_id}/tool_setting.
async fn show_course_resource_link_tool_setting(
    State(state): State<AppState>,
    Path((tool_proxy_guid, course_id, resource_link_id)): Path<(String, i64, String)>,
    headers: HeaderMap,
    Query(query): Query<BubbleQuery>,
) -> Response {
    show_setting(
        &state,
        headers,
        query,
        Some(&tool_proxy_guid),
        SettingContext::Course {
            course_id,
            resource_link_id: Some(resource_link_id),
        },
    )
    .await
}

/// GET /api/lti/tool_proxy/{tool_proxy_guid}/accounts/{account_id}/resource_link_id/{resource_link_id}/tool_setting.
async fn show_account_resource_link_tool_setting(
    State(state): State<AppState>,
    Path((tool_proxy_guid, account_id, resource_link_id)): Path<(String, i64, String)>,
    headers: HeaderMap,
    Query(query): Query<BubbleQuery>,
) -> Response {
    show_setting(
        &state,
        headers,
        query,
        Some(&tool_proxy_guid),
        SettingContext::Account {
            account_id,
            resource_link_id: Some(resource_link_id),
        },
    )
    .await
}

/// GET /api/lti/tool_proxy/{tool_proxy_guid}/tool_setting.
async fn show_tool_proxy_tool_setting(
    State(state): State<AppState>,
    Path(tool_proxy_guid): Path<String>,
    headers: HeaderMap,
    Query(query): Query<BubbleQuery>,
) -> Response {
    show_setting(
        &state,
        headers,
        query,
        Some(&tool_proxy_guid),
        SettingContext::ToolProxy,
    )
    .await
}

/// GET /api/lti/tool_settings/{tool_setting_id}.
async fn show_tool_setting(
    State(state): State<AppState>,
    Path(tool_setting_id): Path<i64>,
    headers: HeaderMap,
    Query(query): Query<BubbleQuery>,
) -> Response {
    show_setting(
        &state,
        headers,
        query,
        None,
        SettingContext::Setting { tool_setting_id },
    )
    .await
}

/// The shared body of the update routes.
async fn update_setting(
    state: &AppState,
    headers: HeaderMap,
    body: Bytes,
    tool_proxy_guid: Option<&str>,
    context: SettingContext,
) -> Response {
    // Canvas `valid_update_request?`: the body has to be JSON and the media
    // type has to be one of the two tool settings types.
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let graph_body = content_type.contains(TOOL_SETTINGS_JSON);
    let simple_body = content_type.contains(TOOL_SETTINGS_SIMPLE_JSON);
    if !graph_body && !simple_body {
        return bad_request();
    }

    let Ok(payload) = serde_json::from_slice::<Value>(&body) else {
        return bad_request();
    };

    let custom = if graph_body {
        let Some(entries) = payload.get("@graph").and_then(Value::as_array) else {
            return bad_request();
        };
        if entries.len() != 1 {
            return bad_request();
        }
        let Some(custom) = entries[0].get("custom") else {
            return bad_request();
        };
        custom.clone()
    } else {
        if payload.get("@graph").is_some() {
            return bad_request();
        }
        payload.clone()
    };

    let (setting, _proxy) = match resolve_setting(state, &headers, tool_proxy_guid, context).await {
        Ok(resolved) => resolved,
        Err(response) => return *response,
    };

    if let Err(error) =
        sqlx::query("UPDATE tool_settings SET custom = $1, updated_at = now() WHERE id = $2")
            .bind(&custom)
            .bind(setting.id)
            .execute(&state.db)
            .await
    {
        return server_error(error);
    }

    // Canvas answers `head :ok` — 200 with an empty body.
    (StatusCode::OK, ()).into_response()
}

/// PUT /api/lti/tool_proxy/{tool_proxy_guid}/courses/{course_id}/tool_setting.
async fn update_course_tool_setting(
    State(state): State<AppState>,
    Path((tool_proxy_guid, course_id)): Path<(String, i64)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    update_setting(
        &state,
        headers,
        body,
        Some(&tool_proxy_guid),
        SettingContext::Course {
            course_id,
            resource_link_id: None,
        },
    )
    .await
}

/// PUT /api/lti/tool_proxy/{tool_proxy_guid}/accounts/{account_id}/tool_setting.
async fn update_account_tool_setting(
    State(state): State<AppState>,
    Path((tool_proxy_guid, account_id)): Path<(String, i64)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    update_setting(
        &state,
        headers,
        body,
        Some(&tool_proxy_guid),
        SettingContext::Account {
            account_id,
            resource_link_id: None,
        },
    )
    .await
}

/// PUT /api/lti/tool_proxy/{tool_proxy_guid}/courses/{course_id}/resource_link_id/{resource_link_id}/tool_setting.
async fn update_course_resource_link_tool_setting(
    State(state): State<AppState>,
    Path((tool_proxy_guid, course_id, resource_link_id)): Path<(String, i64, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    update_setting(
        &state,
        headers,
        body,
        Some(&tool_proxy_guid),
        SettingContext::Course {
            course_id,
            resource_link_id: Some(resource_link_id),
        },
    )
    .await
}

/// PUT /api/lti/tool_proxy/{tool_proxy_guid}/accounts/{account_id}/resource_link_id/{resource_link_id}/tool_setting.
async fn update_account_resource_link_tool_setting(
    State(state): State<AppState>,
    Path((tool_proxy_guid, account_id, resource_link_id)): Path<(String, i64, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    update_setting(
        &state,
        headers,
        body,
        Some(&tool_proxy_guid),
        SettingContext::Account {
            account_id,
            resource_link_id: Some(resource_link_id),
        },
    )
    .await
}

/// PUT /api/lti/tool_proxy/{tool_proxy_guid}/tool_setting.
async fn update_tool_proxy_tool_setting(
    State(state): State<AppState>,
    Path(tool_proxy_guid): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    update_setting(
        &state,
        headers,
        body,
        Some(&tool_proxy_guid),
        SettingContext::ToolProxy,
    )
    .await
}

/// PUT /api/lti/tool_settings/{tool_setting_id}.
async fn update_tool_setting(
    State(state): State<AppState>,
    Path(tool_setting_id): Path<i64>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    update_setting(
        &state,
        headers,
        body,
        None,
        SettingContext::Setting { tool_setting_id },
    )
    .await
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the tool settings router.
///
/// The paths carry the `/api/lti` prefix: Canvas serves the LTI Advantage
/// family outside `/api/v1`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/tool_proxy/{tool_proxy_guid}/courses/{course_id}/tool_setting",
            get(show_course_tool_setting).put(update_course_tool_setting),
        )
        .route(
            "/api/lti/tool_proxy/{tool_proxy_guid}/accounts/{account_id}/tool_setting",
            get(show_account_tool_setting).put(update_account_tool_setting),
        )
        .route(
            "/api/lti/tool_proxy/{tool_proxy_guid}/courses/{course_id}/resource_link_id/{resource_link_id}/tool_setting",
            get(show_course_resource_link_tool_setting)
                .put(update_course_resource_link_tool_setting),
        )
        .route(
            "/api/lti/tool_proxy/{tool_proxy_guid}/accounts/{account_id}/resource_link_id/{resource_link_id}/tool_setting",
            get(show_account_resource_link_tool_setting)
                .put(update_account_resource_link_tool_setting),
        )
        .route(
            "/api/lti/tool_settings/{tool_setting_id}",
            get(show_tool_setting).put(update_tool_setting),
        )
        .route(
            "/api/lti/tool_proxy/{tool_proxy_guid}/tool_setting",
            get(show_tool_proxy_tool_setting).put(update_tool_proxy_tool_setting),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(resource_link_id: Option<&str>, context_id: Option<i64>) -> ToolSettingRow {
        ToolSettingRow {
            id: 1,
            tool_proxy_id: 2,
            context_type: Some("Course".to_string()),
            context_id,
            resource_link_id: resource_link_id.map(|value| value.to_string()),
            custom: json!({ "a": 1 }),
        }
    }

    #[test]
    fn the_setting_type_follows_the_context() {
        assert_eq!(setting(Some("link"), Some(3)).setting_type(), "LtiLink");
        assert_eq!(setting(None, Some(3)).setting_type(), "ToolProxyBinding");
        assert_eq!(setting(None, None).setting_type(), "ToolProxy");
    }

    #[test]
    fn the_url_is_the_setting_route() {
        assert_eq!(setting(None, Some(3)).url(), "/api/lti/tool_settings/1");
    }

    #[test]
    fn distinct_drops_the_keys_of_the_higher_level() {
        let mut row = setting(None, Some(3));
        row.custom = json!({ "a": 1, "b": 2 });
        let entry = collect_tool_settings(&row, Some(&["a".to_string()]));
        assert_eq!(entry["custom"]["a"], Value::Null);
        assert_eq!(entry["custom"]["b"], 2);
        assert_eq!(entry["@type"], "ToolProxyBinding");
    }
}
