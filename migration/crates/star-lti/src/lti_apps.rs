//! lti_apps — Canvas `Lti::LtiAppsController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET /courses/{courseId}/lti_apps` — the app definitions of a course
//! - `GET /accounts/{accountId}/lti_apps` — the app definitions of an account
//! - `GET /courses/{courseId}/lti_apps/launch_definitions`
//! - `GET /accounts/{accountId}/lti_apps/launch_definitions`
//!
//! Canvas routes (config/routes.rb, the course and account API blocks):
//!   get "courses/:course_id/lti_apps", action: :index
//!   get "accounts/:account_id/lti_apps", action: :index
//!   get "courses/:course_id/lti_apps/launch_definitions", action: :launch_definitions
//!   get "accounts/:account_id/lti_apps/launch_definitions", action: :launch_definitions
//!
//! Canvas controller (`app/controllers/lti/lti_apps_controller.rb`):
//! - `index` needs the `read_as_admin` right on the context; it answers the
//!   `AppCollator#app_definitions` shape — the external tools of the context
//!   and the tool proxies installed in it.
//! - `launch_definitions` needs the `read` right on the context; the
//!   `global_navigation` placement of an account is readable by every user
//!   associated with the account. It answers the
//!   `AppLaunchCollator#launch_definitions` shape.
//!
//! The `index` response is a bare array (Canvas renders the collection
//! without a wrapper), the `launch_definitions` response is a bare array too.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// The context an LTI app lives in.
#[derive(Debug, Clone, Copy)]
enum AppContext {
    Course(i64),
    Account(i64),
}

impl AppContext {
    /// The Canvas context type of the context.
    fn context_type(self) -> &'static str {
        match self {
            AppContext::Course(_) => "Course",
            AppContext::Account(_) => "Account",
        }
    }

    /// The Canvas context id of the context.
    fn context_id(self) -> i64 {
        match self {
            AppContext::Course(id) => id,
            AppContext::Account(id) => id,
        }
    }
}

/// The query parameters of the launch-definitions route.
#[derive(Debug, Default, Deserialize)]
struct LaunchQuery {
    placements: Option<String>,
    only_visible: Option<String>,
    include_context_name: Option<String>,
}

/// Domain router for the Canvas LTI apps routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/lti_apps", get(index_course))
        .route("/accounts/{accountId}/lti_apps", get(index_account))
        .route(
            "/courses/{courseId}/lti_apps/launch_definitions",
            get(launch_definitions_course),
        )
        .route(
            "/accounts/{accountId}/lti_apps/launch_definitions",
            get(launch_definitions_account),
        )
}

/// Canvas `Lti::LtiAppsController#index` for a course.
async fn index_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    index(state, AppContext::Course(course_id), auth).await
}

/// Canvas `Lti::LtiAppsController#index` for an account.
async fn index_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    index(state, AppContext::Account(account_id), auth).await
}

/// Canvas `Lti::LtiAppsController#index`.
async fn index(state: AppState, context: AppContext, auth: AuthUser) -> Result<Response, ApiError> {
    require_read_as_admin(&state.db, &auth, context).await?;
    let definitions = app_definitions(&state.db, context).await?;
    Ok(Json(definitions).into_response())
}

/// Canvas `Lti::LtiAppsController#launch_definitions` for a course.
async fn launch_definitions_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<LaunchQuery>,
) -> Result<Response, ApiError> {
    launch_definitions(state, AppContext::Course(course_id), auth, params).await
}

/// Canvas `Lti::LtiAppsController#launch_definitions` for an account.
async fn launch_definitions_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<LaunchQuery>,
) -> Result<Response, ApiError> {
    launch_definitions(state, AppContext::Account(account_id), auth, params).await
}

/// Canvas `Lti::LtiAppsController#launch_definitions`.
async fn launch_definitions(
    state: AppState,
    context: AppContext,
    auth: AuthUser,
    params: LaunchQuery,
) -> Result<Response, ApiError> {
    let placements = parse_placements(params.placements.as_deref());
    // Canvas allows every user associated with an account to read the
    // global-navigation launch definitions of that account.
    let global_navigation = placements == ["global_navigation"];
    if matches!(context, AppContext::Account(_)) && global_navigation {
        if !user_in_account(&state.db, &auth, context.context_id()).await? {
            return Err(ApiError::Forbidden);
        }
    } else {
        require_read(&state.db, &auth, context).await?;
    }

    // Canvas `Lti::LtiAppsController#launch_definitions`: the global
    // navigation pulls every launch definition unless the caller asks for the
    // visible ones only; every other request only returns the visible ones.
    let only_visible = !global_navigation || value_to_boolean(params.only_visible.as_deref());
    let include_context_name = value_to_boolean(params.include_context_name.as_deref());

    let definitions = launch_definition_list(
        &state.db,
        context,
        &placements,
        global_navigation,
        only_visible,
        include_context_name,
        &auth,
    )
    .await?;
    Ok(Json(definitions).into_response())
}

/// The `placements` parameter: a JSON array of placement names.
fn parse_placements(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw.filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<String>>(raw) {
        Ok(values) => values,
        // Canvas also accepts a bare comma-separated list.
        Err(_) => raw
            .split(',')
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect(),
    }
}

/// Canvas `value_to_boolean`.
fn value_to_boolean(raw: Option<&str>) -> bool {
    matches!(
        raw,
        Some("true") | Some("TRUE") | Some("1") | Some("yes") | Some("on")
    )
}

/// Canvas `Lti::LtiAppsController#authorized_action(context, user, :read_as_admin)`.
async fn require_read_as_admin(
    db: &PgPool,
    auth: &AuthUser,
    context: AppContext,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    match context {
        AppContext::Course(course_id) => {
            if is_course_admin(db, auth.id, course_id).await? {
                Ok(())
            } else {
                Err(ApiError::Forbidden)
            }
        }
        AppContext::Account(account_id) => {
            if is_account_admin(db, auth.id, account_id).await? {
                Ok(())
            } else {
                Err(ApiError::Forbidden)
            }
        }
    }
}

/// Canvas `Lti::LtiAppsController#authorized_action(context, user, :read)`.
async fn require_read(db: &PgPool, auth: &AuthUser, context: AppContext) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    match context {
        AppContext::Course(course_id) => {
            if is_course_member(db, auth.id, course_id).await? {
                Ok(())
            } else {
                Err(ApiError::Forbidden)
            }
        }
        AppContext::Account(account_id) => {
            if user_in_account(db, auth, account_id).await? {
                Ok(())
            } else {
                Err(ApiError::Forbidden)
            }
        }
    }
}

/// Canvas `Lti::LtiAppsController#user_in_account?`.
async fn user_in_account(db: &PgPool, auth: &AuthUser, account_id: i64) -> Result<bool, ApiError> {
    let membership: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users \
         WHERE account_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(account_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    Ok(membership.is_some())
}

/// Whether the user manages the course.
async fn is_course_admin(db: &PgPool, user_id: i64, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE course_id = $1 AND user_id = $2 AND role = 'teacher' LIMIT 1",
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Whether the user is a member of the course.
async fn is_course_member(db: &PgPool, user_id: i64, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 LIMIT 1")
            .bind(course_id)
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

/// Whether the user manages the account.
async fn is_account_admin(db: &PgPool, user_id: i64, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users \
         WHERE account_id = $1 AND user_id = $2 AND role <> 'student' LIMIT 1",
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Canvas `AppCollator#app_definitions` — the external tools of the context
/// and the tool proxies installed in it.
async fn app_definitions(db: &PgPool, context: AppContext) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<(i64, String, Option<String>, String, String, Value)> = sqlx::query_as(
        "SELECT id, name, description, context_type, lti_version, tool_configuration \
         FROM external_tools \
         WHERE context_type = $1 AND context_id = $2 AND workflow_state = 'active' \
         ORDER BY name ASC",
    )
    .bind(context.context_type())
    .bind(context.context_id())
    .fetch_all(db)
    .await?;

    let mut definitions = Vec::with_capacity(rows.len());
    for (id, name, description, context_type, lti_version, tool_configuration) in rows {
        definitions.push(json!({
            "app_type": "ContextExternalTool",
            "app_id": id,
            "name": name,
            "description": description,
            "installed_locally": true,
            "installed": true,
            "enabled": true,
            "tool_configuration": tool_configuration,
            "context": context_type,
            "context_id": context.context_id(),
            "reregistration_url": Value::Null,
            "has_update": Value::Null,
            "lti_version": lti_version,
            "deployment_id": Value::Null,
            "editor_button_settings": Value::Null,
            "top_navigation_settings": Value::Null,
            "migration_running": false,
        }));
    }

    // The tool proxies installed in the context.
    let proxies: Vec<(i64, Value)> = sqlx::query_as(
        "SELECT id, raw_data FROM lti_tool_proxies \
         WHERE context_type = $1 AND context_id = $2 AND workflow_state = 'active' \
         ORDER BY id ASC",
    )
    .bind(context.context_type())
    .bind(context.context_id())
    .fetch_all(db)
    .await?;

    for (id, raw_data) in proxies {
        let name = raw_data
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let description = raw_data
            .get("description")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        definitions.push(json!({
            "app_type": "ToolProxy",
            "app_id": id,
            "name": name,
            "description": description,
            "installed_locally": true,
            "installed": true,
            "enabled": true,
            "tool_configuration": Value::Null,
            "context": context.context_type(),
            "context_id": context.context_id(),
            "reregistration_url": Value::Null,
            "has_update": Value::Null,
            "lti_version": "1.3",
            "deployment_id": Value::Null,
            "editor_button_settings": Value::Null,
            "top_navigation_settings": Value::Null,
            "migration_running": false,
        }));
    }

    Ok(definitions)
}

/// A row of the `external_tools` table.
#[derive(Debug, sqlx::FromRow)]
struct ExternalToolRow {
    id: i64,
    name: String,
    description: Option<String>,
    url: Option<String>,
    domain: Option<String>,
    tool_configuration: Value,
    context_type: String,
    context_id: i64,
}

/// Canvas `AppLaunchCollator#launch_definitions`.
async fn launch_definition_list(
    db: &PgPool,
    context: AppContext,
    placements: &[String],
    global_navigation: bool,
    only_visible: bool,
    include_context_name: bool,
    auth: &AuthUser,
) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<ExternalToolRow> = sqlx::query_as(
        "SELECT id, name, description, url, domain, tool_configuration, context_type, context_id \
             FROM external_tools \
             WHERE context_type = $1 AND context_id = $2 AND workflow_state = 'active' \
             ORDER BY name ASC",
    )
    .bind(context.context_type())
    .bind(context.context_id())
    .fetch_all(db)
    .await?;

    let mut definitions = Vec::with_capacity(rows.len());
    for row in rows {
        if only_visible
            && !tool_visible_to_user(db, auth, context, placements, &row.tool_configuration).await?
        {
            continue;
        }
        let placement_settings = placement_settings(&row.tool_configuration, placements);
        let mut definition = json!({
            "definition_type": "ContextExternalTool",
            "definition_id": row.id.to_string(),
            "url": row.url,
            "name": row.name,
            "description": row.description,
            "domain": row.domain,
            "placements": placement_settings,
        });
        // Canvas `AppLaunchCollator#batch_load_context_names` — the name of
        // the account or the course the tool is deployed in.
        if include_context_name {
            definition["context_name"] =
                json!(context_name(db, &row.context_type, row.context_id).await?);
        }
        definitions.push(definition);
    }

    // The tool proxies of the context become LTI 2.0 launch definitions.
    let proxies: Vec<(i64, Value)> = sqlx::query_as(
        "SELECT id, raw_data FROM lti_tool_proxies \
         WHERE context_type = $1 AND context_id = $2 AND workflow_state = 'active' \
         ORDER BY id ASC",
    )
    .bind(context.context_type())
    .bind(context.context_id())
    .fetch_all(db)
    .await?;

    for (id, raw_data) in proxies {
        let name = raw_data
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let description = raw_data
            .get("description")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let url = raw_data
            .get("launch_url")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let domain = raw_data
            .get("domain")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());
        let placement_settings = placement_settings(&raw_data, placements);
        definitions.push(json!({
            "definition_type": "MessageHandler",
            "definition_id": id.to_string(),
            "url": url,
            "name": name,
            "description": description,
            "domain": domain,
            "placements": placement_settings,
        }));
    }

    // Without placements Canvas returns an empty list.
    if placements.is_empty() && !global_navigation {
        return Ok(Vec::new());
    }

    Ok(definitions)
}

/// Canvas `ContextExternalTool.visible` — a user with the `read_as_admin`
/// right sees every tool, every other user only the tools whose placement
/// visibility allows it.
async fn tool_visible_to_user(
    db: &PgPool,
    auth: &AuthUser,
    context: AppContext,
    placements: &[String],
    tool_configuration: &Value,
) -> Result<bool, ApiError> {
    if is_context_admin(db, auth, context).await? {
        return Ok(true);
    }
    let members_allowed = can_participate_as_student(db, auth, context).await?;
    for placement in placements {
        let Some(settings) = tool_configuration
            .get("placements")
            .and_then(|value| value.get(placement))
        else {
            continue;
        };
        match settings.get("visibility").and_then(|value| value.as_str()) {
            None | Some("public") => return Ok(true),
            Some("members") if members_allowed => return Ok(true),
            _ => {}
        }
    }
    Ok(false)
}

/// Whether the user manages the context — Canvas `read_as_admin`.
async fn is_context_admin(
    db: &PgPool,
    auth: &AuthUser,
    context: AppContext,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    match context {
        AppContext::Course(course_id) => is_course_admin(db, auth.id, course_id).await,
        AppContext::Account(account_id) => is_account_admin(db, auth.id, account_id).await,
    }
}

/// Canvas `participate_as_student` — a member of a course may participate as
/// a student, an account does not grant the right.
async fn can_participate_as_student(
    db: &PgPool,
    auth: &AuthUser,
    context: AppContext,
) -> Result<bool, ApiError> {
    match context {
        AppContext::Course(course_id) => is_course_member(db, auth.id, course_id).await,
        AppContext::Account(_) => Ok(false),
    }
}

/// The name of the account or the course a tool is deployed in.
async fn context_name(
    db: &PgPool,
    context_type: &str,
    context_id: i64,
) -> Result<Option<String>, ApiError> {
    let sql = if context_type == "Course" {
        "SELECT name FROM courses WHERE id = $1"
    } else {
        "SELECT name FROM accounts WHERE id = $1"
    };
    let row: Option<(String,)> = sqlx::query_as(sql)
        .bind(context_id)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|row| row.0))
}

/// The placement-specific configuration of a tool for the requested
/// placements.
fn placement_settings(tool_configuration: &Value, placements: &[String]) -> Value {
    let mut settings = serde_json::Map::new();
    for placement in placements {
        let entry = tool_configuration
            .get("placements")
            .and_then(|value| value.get(placement))
            .cloned()
            .unwrap_or_else(|| json!({}));
        settings.insert(placement.clone(), entry);
    }
    Value::Object(settings)
}
