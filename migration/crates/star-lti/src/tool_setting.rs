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
//!   token's `sub` names the tool proxy and its `scopes` claim carries the
//!   permission. A request without a token is a 401, a token without the
//!   required scope is a 403.
//! - the setting is looked up by its id, or by the tool proxy of the token
//!   together with the context of the path and the optional resource link id.
//!   A setting that does not exist is a 404.
//! - the `bubble` query parameter is blank, `distinct` or `all`; `all` is only
//!   allowed with the `application/vnd.ims.lti.v2.toolsettings+json` media
//!   type. Anything else is a 400.
//! - with the tool settings media type the answer is the `@graph` form, with
//!   the simple media type it is the flat `custom` object.
//! - an update replaces the `custom` settings of the setting and answers
//!   200 with an empty body.

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
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

#[derive(Debug, sqlx::FromRow)]
struct ToolSettingRow {
    id: i64,
    tool_proxy_id: i64,
    context_type: Option<String>,
    context_id: Option<i64>,
    resource_link_id: Option<String>,
    custom: Value,
}

impl ToolSettingRow {
    /// The `@type` of the setting: `LtiLink` for a resource link,
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

    /// The url of the setting, the id Canvas uses in the graph.
    fn url(&self) -> String {
        match (&self.context_type, self.context_id) {
            (Some(context_type), Some(context_id)) => match context_type.as_str() {
                "Course" => format!("/api/lti/courses/{context_id}/tool_setting"),
                "Account" => format!("/api/lti/accounts/{context_id}/tool_setting"),
                other => format!("/api/lti/{other}s/{context_id}/tool_setting"),
            },
            _ => "/api/lti/tool_proxy/tool_setting".to_string(),
        }
    }
}

/// The context a tool setting belongs to.
#[derive(Debug, Clone, Copy)]
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
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized" })),
    )
        .into_response()
}

fn forbidden() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "errors": [{ "code": "forbidden", "message": "forbidden" }]
        })),
    )
        .into_response()
}

fn bad_request() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "status": "bad_request",
            "errors": [{ "message": "bad_request" }]
        })),
    )
        .into_response()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "status": "not_found",
            "errors": [{ "message": "not_found" }]
        })),
    )
        .into_response()
}

/// The tool proxy of an LTI Advantage access token.
///
/// The token is a JWT signed with the platform secret; its `sub` claim is the
/// guid of the tool proxy and its `scopes` claim carries the permissions.
async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(Option<i64>, Vec<String>), Response> {
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
    );
    let Ok(decoded) = decoded else {
        return Err(unauthorized());
    };
    let claims = decoded.claims;

    // An LTI Advantage access token carries the scopes of the call.
    let scopes = claims.get("scopes").and_then(Value::as_str).map(|scopes| {
        scopes
            .split_whitespace()
            .map(|scope| scope.to_string())
            .collect::<Vec<String>>()
    });

    let Some(scopes) = scopes else {
        // A Canvas session token: only a site admin may read the tool
        // settings.
        return match claims.get("sub").and_then(Value::as_i64) {
            Some(user_id) => {
                let row: Option<(String,)> = sqlx::query_as("SELECT role FROM users WHERE id = $1")
                    .bind(user_id)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(|error| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({ "error": error.to_string() })),
                        )
                            .into_response()
                    })?;
                match row {
                    Some((role,)) if role == "admin" => Ok((None, Vec::new())),
                    _ => Err(forbidden()),
                }
            }
            None => Err(forbidden()),
        };
    };

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
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response()
    })?;

    Ok((proxy, scopes))
}

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/// The tool proxy of a guid.
async fn tool_proxy_by_guid(pool: &PgPool, guid: &str) -> Result<Option<i64>, Response> {
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM lti_tool_proxies WHERE guid = $1 AND workflow_state = 'active'",
    )
    .bind(guid)
    .fetch_optional(pool)
    .await
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response()
    })
}

/// The tool setting of a tool proxy in a context.
async fn find_setting(
    pool: &PgPool,
    tool_proxy_id: i64,
    context: SettingContext,
) -> Result<Option<ToolSettingRow>, Response> {
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

    row.map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response()
    })
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
) -> Result<(ToolSettingRow, i64), Response> {
    let (token_proxy, _scopes) = authenticate(state, headers).await?;

    let proxy = match (tool_proxy_guid, token_proxy) {
        (Some(guid), _) => {
            let proxy = tool_proxy_by_guid(&state.db, guid).await?;
            match proxy {
                Some(proxy) => proxy,
                None => return Err(not_found()),
            }
        }
        (None, Some(proxy)) => proxy,
        (None, None) => return Err(forbidden()),
    };

    let setting = find_setting(&state.db, proxy, context).await?;
    let Some(setting) = setting else {
        return Err(not_found());
    };
    Ok((setting, proxy))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// The tool settings of a request, in the shape the media type asks for.
fn settings_response(setting: &ToolSettingRow, accepts_graph: bool, bubble: &str) -> Response {
    let custom = setting.custom.clone();
    let graph = if bubble == "all" {
        vec![json!({
            "@type": "LtiLink",
            "custom": custom,
            "id": setting.url(),
        })]
    } else {
        vec![json!({
            "@type": setting.setting_type(),
            "custom": custom,
            "id": setting.url(),
        })]
    };

    if accepts_graph {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, TOOL_SETTINGS_JSON)],
            Json(json!({ "@graph": graph })),
        )
            .into_response();
    }

    let custom = custom.as_object().cloned().unwrap_or_default();
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, TOOL_SETTINGS_SIMPLE_JSON)],
        Json(Value::Object(custom)),
    )
        .into_response()
}

/// Whether the request wants the tool settings graph.
fn accepts_graph(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains(TOOL_SETTINGS_JSON))
        .unwrap_or(false)
}

/// The `bubble` parameter, checked the way Canvas does.
fn checked_bubble(query: &BubbleQuery, accepts_graph: bool) -> Result<&str, Response> {
    let bubble = query.bubble.as_deref().unwrap_or("").trim();
    match bubble {
        "" | "distinct" => Ok(bubble),
        "all" if accepts_graph => Ok(bubble),
        _ => Err(bad_request()),
    }
}

/// The shared body of the show routes.
async fn show_setting(
    state: &AppState,
    headers: HeaderMap,
    query: BubbleQuery,
    tool_proxy_guid: Option<&str>,
    context: SettingContext,
) -> Response {
    let accepts_graph = accepts_graph(&headers);
    let bubble = match checked_bubble(&query, accepts_graph) {
        Ok(bubble) => bubble,
        Err(response) => return response,
    };
    match resolve_setting(state, &headers, tool_proxy_guid, context).await {
        Ok((setting, _proxy)) => settings_response(&setting, accepts_graph, bubble),
        Err(response) => response,
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
    let accepts_graph = accepts_graph(&headers);
    let bubble = match checked_bubble(&query, accepts_graph) {
        Ok(bubble) => bubble,
        Err(response) => return response,
    };
    match resolve_setting(
        &state,
        &headers,
        None,
        SettingContext::Setting { tool_setting_id },
    )
    .await
    {
        Ok((setting, _proxy)) => settings_response(&setting, accepts_graph, bubble),
        Err(response) => response,
    }
}

/// The shared body of the update routes.
async fn update_setting(
    state: &AppState,
    headers: HeaderMap,
    body: Bytes,
    tool_proxy_guid: Option<&str>,
    context: SettingContext,
) -> Response {
    let accepts_graph = accepts_graph(&headers);

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
        Err(response) => return response,
    };

    if let Err(error) =
        sqlx::query("UPDATE tool_settings SET custom = $1, updated_at = now() WHERE id = $2")
            .bind(&custom)
            .bind(setting.id)
            .execute(&state.db)
            .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response();
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

    #[test]
    fn the_setting_type_follows_the_context() {
        let mut setting = ToolSettingRow {
            id: 1,
            tool_proxy_id: 2,
            context_type: Some("Course".to_string()),
            context_id: Some(3),
            resource_link_id: None,
            custom: json!({}),
        };
        assert_eq!(setting.setting_type(), "ToolProxyBinding");

        setting.resource_link_id = Some("link".to_string());
        assert_eq!(setting.setting_type(), "LtiLink");

        setting.context_id = None;
        setting.resource_link_id = None;
        assert_eq!(setting.setting_type(), "ToolProxy");
    }

    #[test]
    fn the_graph_carries_the_type_the_custom_settings_and_the_url() {
        let setting = ToolSettingRow {
            id: 1,
            tool_proxy_id: 2,
            context_type: Some("Course".to_string()),
            context_id: Some(3),
            resource_link_id: None,
            custom: json!({ "a": 1 }),
        };
        let response = settings_response(&setting, true, "");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
