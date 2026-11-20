//! star-lti — the LTI registration sub-routes that were still missing.
//!
//! Canvas parity: `app/controllers/lti/registrations_controller.rb`
//! (routes registered in `config/routes.rb`, `ApiRouteSet.draw`).
//!
//! Routes (all mounted under `/api/v1` by star-http):
//!
//! - `POST   /accounts/{account_id}/lti_registrations/configuration/validate`
//! - `GET    /accounts/{account_id}/lti_registrations/check_domain_duplicates`
//! - `GET    /accounts/{account_id}/lti_registrations/{registration_id}/deployments/{deployment_id}/context_search`
//! - `GET    /accounts/{account_id}/lti_registrations/{id}/overlay_history`
//! - `GET    /accounts/{account_id}/lti_registrations/{id}/history`
//! - `GET    /accounts/{account_id}/lti_registration_by_client_id/{client_id}`
//! - `GET    /accounts/{account_id}/lti_registrations/by_utid/{utid}`
//! - `GET    /accounts/{account_id}/lti_registrations/install_status/{client_id}`
//! - `GET    /accounts/{account_id}/lti_registrations/{id}/update_requests/{update_request_id}`
//! - `GET    /accounts/{account_id}/lti_registrations/{id}/latest_update_request`
//! - `PUT    /accounts/{account_id}/lti_registrations/{id}/update_requests/{update_request_id}/apply`
//! - `PUT    /accounts/{account_id}/lti_registrations/{id}`
//! - `PUT    /accounts/{account_id}/lti_registrations/{id}/reset`
//! - `POST   /accounts/{account_id}/lti_registrations/{id}/bind`
//! - `DELETE /accounts/{account_id}/lti_registrations/{id}/bind`
//! - `POST   /accounts/{account_id}/lti_registrations/{id}/install_from_template`
//!
//! Authorization mirrors Canvas' controller-level `before_action` chain, in
//! order:
//!
//! 1. `require_root_account_instrumented_or_sub_account_and_feature_flag`
//!    — `404` when the account is missing, or when it is a sub-account and
//!    the root account does not have `canvas_apps_sub_account_access`.
//! 2. `require_lti_registrations_next_feature_flag` (`reset`, `context_search`,
//!    `overlay_history`), `require_lti_registrations_history_feature_flag`
//!    (`history`) and `require_lti_deactivate_registrations_flag` (`unbind`)
//!    — `404` when the root account lacks the flag.
//! 3. `require_manage_lti_registrations` — `403` without the account admin
//!    right.
//! 4. `require_manage_lti_registrations_in_registrations_account` (`reset`,
//!    `update`) — `400 {errors: "registration does not belong to account"}`.
//! 5. `restrict_sub_account_to_read_only` — `403
//!    {errors: "sub-accounts can only view registrations"}` for every action
//!    that is not in Canvas' `except:` list.
//! 6. `require_in_account_or_site_admin` — `400 {errors: "registration does
//!    not belong to account"}`.
//!
//! The workspace has no `lti_overlays` / `lti_overlay_versions` /
//! `lti_registration_history_entries` / `lti_tool_configurations` /
//! `developer_key_account_bindings` tables; the endpoints that read them
//! answer Canvas' empty-collection shape until the DDL lands (see the
//! migration report).

use axum::{
    body::Bytes,
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

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum account-hierarchy walk, mirroring star-accounts' cycle guard.
const MAX_ACCOUNT_TREE_DEPTH: i64 = 10;

/// Canvas `Api::PER_PAGE` — the default page size for the history endpoints.
const DEFAULT_PER_PAGE: i64 = 10;

/// Canvas `Api::MAX_PER_PAGE`.
const MAX_PER_PAGE: i64 = 100;

/// Canvas `Lti::ResourcePlacement::PLACEMENTS` — the subset that matters for
/// the launch-settings filter in `Schemas::InternalLtiConfiguration
/// .from_lti_configuration`.
const PLACEMENTS: &[&str] = &[
    "account_navigation",
    "analytics_hub",
    "assignment_edit",
    "assignment_group_menu",
    "assignment_index_menu",
    "assignment_menu",
    "assignment_selection",
    "assignment_view",
    "collaboration",
    "conference_selection",
    "course_assignments_menu",
    "course_home_sub_navigation",
    "course_navigation",
    "course_settings_sub_navigation",
    "discussion_topic_index_menu",
    "discussion_topic_menu",
    "editor_button",
    "file_index_menu",
    "file_menu",
    "global_navigation",
    "homework_submission",
    "link_selection",
    "migration_selection",
    "module_group_menu",
    "module_index_menu",
    "module_index_menu_modal",
    "module_menu",
    "module_menu_modal",
    "post_grades",
    "quiz_index_menu",
    "quiz_menu",
    "resource_selection",
    "similarity_detection",
    "student_context_card",
    "submission_type_selection",
    "tool_configuration",
    "top_navigation",
    "user_navigation",
    "wiki_index_menu",
    "wiki_page_menu",
    "ActivityAssetProcessor",
    "ActivityAssetProcessorContribution",
];

/// `Schemas::Lti::Overlay::ROOT_KEYS` — the overlay keys that overwrite the
/// root of the internal configuration.
const OVERLAY_ROOT_KEYS: &[&str] = &[
    "title",
    "description",
    "custom_fields",
    "target_link_uri",
    "oidc_initiation_url",
    "domain",
    "privacy_level",
];

/// `Schemas::Lti::Overlay::LAUNCH_SETTINGS_KEYS` — the overlay keys that
/// overwrite `launch_settings`.
const OVERLAY_LAUNCH_SETTINGS_KEYS: &[&str] = &["custom_fields", "icon_url", "message_type"];

/// `Schemas::InternalLtiConfiguration.allowed_base_properties` — the internal
/// configuration keys that may be written.
const INTERNAL_CONFIG_KEYS: &[&str] = &[
    "title",
    "description",
    "custom_fields",
    "target_link_uri",
    "oidc_initiation_url",
    "oidc_initiation_urls",
    "public_jwk_url",
    "public_jwk",
    "scopes",
    "redirect_uris",
    "domain",
    "tool_id",
    "privacy_level",
    "launch_settings",
    "placements",
];

/// The `lti_registrations` columns read by this module.
const REGISTRATION_COLUMNS: &str = "id, account_id, root_account_id, name, admin_nickname, \
     vendor, description, icon_url, lti_version, dynamic_registration, internal_service, \
     lock_deploying, template_registration_id, unified_tool_id, workflow_state, configuration, \
     overlay, created_by_id, updated_by_id, created_at, updated_at";

// ---------------------------------------------------------------------------
// Domain rows
// ---------------------------------------------------------------------------

/// A row of `lti_registrations`.
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
    unified_tool_id: Option<String>,
    workflow_state: String,
    configuration: Value,
    overlay: Option<Value>,
    created_by_id: Option<i64>,
    updated_by_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// A row of `lti_registration_account_bindings`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct BindingRow {
    id: i64,
    registration_id: i64,
    account_id: i64,
    root_account_id: Option<i64>,
    workflow_state: String,
    created_by_id: Option<i64>,
    updated_by_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// The IMS registration linked to an LTI registration
/// (`lti_ims_registrations.lti_registration_id`).
#[derive(Debug, Clone, sqlx::FromRow)]
struct ImsRow {
    id: i64,
    developer_key_id: Option<i64>,
    registration_url: Option<String>,
    unified_tool_id: Option<String>,
    lti_registration_id: Option<i64>,
}

/// A row of `lti_registration_update_requests`.
///
/// `reinstall`, `tool_initiated`, `comment` and `updated_by_id` are Canvas
/// columns the workspace table does not carry yet; they are read out of the
/// row's JSONB projection so the handler keeps working once the DDL lands
/// (a missing key decodes to `NULL`).
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
    extra: Value,
}

/// A row of `lti_deployments`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct DeploymentRow {
    id: i64,
    registration_id: i64,
    context_type: String,
    context_id: i64,
}

// ---------------------------------------------------------------------------
// Request / query types
// ---------------------------------------------------------------------------

/// Body of `POST /lti_registrations/configuration/validate`.
#[derive(Debug, Deserialize, Default)]
pub struct ValidateConfigurationBody {
    pub lti_configuration: Option<Value>,
    pub url: Option<String>,
}

/// Query of `GET /lti_registrations/check_domain_duplicates`.
#[derive(Debug, Deserialize, Default)]
pub struct DomainDuplicatesQuery {
    pub domain: Option<String>,
}

/// Query of `GET /lti_registrations/{id}/overlay_history`.
#[derive(Debug, Deserialize, Default)]
pub struct LimitQuery {
    pub limit: Option<String>,
}

/// Query of `GET /lti_registrations/{id}/history`.
#[derive(Debug, Deserialize, Default)]
pub struct HistoryQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// Query of
/// `GET /lti_registrations/{registration_id}/deployments/{deployment_id}/context_search`.
#[derive(Debug, Deserialize, Default)]
pub struct ContextSearchQuery {
    pub only_children_of: Option<String>,
    pub search_term: Option<String>,
}

/// Body of `POST /lti_registrations/{id}/bind`.
#[derive(Debug, Deserialize, Default)]
pub struct BindBody {
    pub workflow_state: Option<String>,
}

/// Body of `PUT /lti_registrations/{id}`.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateRegistrationBody {
    pub name: Option<String>,
    pub admin_nickname: Option<String>,
    pub vendor: Option<String>,
    pub description: Option<String>,
    pub workflow_state: Option<String>,
    pub lock_deploying: Option<bool>,
    pub configuration: Option<Value>,
    pub overlay: Option<Value>,
    pub comment: Option<String>,
}

/// Body of `PUT /lti_registrations/{id}/update_requests/{update_request_id}/apply`.
#[derive(Debug, Deserialize, Default)]
pub struct ApplyUpdateRequestBody {
    pub accepted: Option<bool>,
    pub overlay: Option<Value>,
    pub comment: Option<String>,
}

// ---------------------------------------------------------------------------
// Response helpers
// ---------------------------------------------------------------------------

/// Canvas `render json: { errors: "..." }, status: <status>`.
fn errors_string(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "errors": message }))).into_response()
}

/// Canvas `render_error(code, message, status:)` — the error body carries an
/// array of `{ code, message }` objects.
fn render_error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(json!({ "errors": [{ "code": code, "message": message }] })),
    )
        .into_response()
}

/// Canvas `render_configuration_errors(errors)` — `422 { "errors": [...] }` with
/// plain string entries.
fn configuration_errors(errors: Vec<String>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "errors": errors })),
    )
        .into_response()
}

/// Parse an optional JSON body.
///
/// An empty body is `None` (Canvas treats a missing parameter as "not
/// present"); a malformed body is a `400`, the way Rails' parameter parser
/// rejects it.
fn parse_body<T: serde::de::DeserializeOwned>(body: &Bytes) -> Result<Option<T>, ApiError> {
    if body.is_empty() {
        return Ok(None);
    }
    serde_json::from_slice(body)
        .map(Some)
        .map_err(|_| ApiError::BadRequest("invalid JSON body".to_string()))
}

/// `404` with Canvas' "The specified resource does not exist." body.
fn not_found_render_error() -> Response {
    render_error(
        StatusCode::NOT_FOUND,
        "not_found",
        "The specified resource does not exist.",
    )
}

// ---------------------------------------------------------------------------
// Permission / context helpers
// ---------------------------------------------------------------------------

/// The request context resolved by Canvas' controller-level `before_action`s.
struct Ctx {
    account_id: i64,
    root_account_id: i64,
    is_root_account: bool,
    site_admin_id: i64,
}

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Whether the user has an `account_users` row for the given account.
async fn is_account_admin(pool: &PgPool, user_id: i64, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Global admin OR account admin for the account or one of its ancestors.
async fn can_manage_account(
    pool: &PgPool,
    auth: &AuthUser,
    account_id: i64,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let mut current_id = Some(account_id);
    let mut depth = 0;
    while let Some(id) = current_id {
        if depth >= MAX_ACCOUNT_TREE_DEPTH {
            return Ok(false);
        }
        depth += 1;

        if is_account_admin(pool, auth.id, id).await? {
            return Ok(true);
        }

        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        current_id = parent.and_then(|(parent_id,)| parent_id);
    }

    Ok(false)
}

/// `404` when the account does not exist (Canvas `require_account_context`).
async fn require_account(pool: &PgPool, account_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Walk to the root of the account hierarchy (cycles are bounded).
async fn root_account_id(pool: &PgPool, account_id: i64) -> Result<i64, ApiError> {
    let mut current = account_id;
    for _ in 0..MAX_ACCOUNT_TREE_DEPTH {
        let row: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(current)
                .fetch_optional(pool)
                .await?;
        match row {
            Some((Some(parent),)) => current = parent,
            _ => return Ok(current),
        }
    }
    Ok(current)
}

/// The site-admin account. Canvas keeps a dedicated `Account.site_admin`
/// row; the workspace models it as the root account (same convention as
/// `star-accounts`).
async fn site_admin_account_id(pool: &PgPool) -> Result<i64, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL \
           AND workflow_state <> 'deleted' ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    row.map(|(id,)| id).ok_or(ApiError::NotFound)
}

/// Canvas `Account#feature_enabled?`.
async fn feature_enabled(pool: &PgPool, account_id: i64, feature: &str) -> Result<bool, ApiError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT state FROM feature_flags \
         WHERE context_type = 'Account' AND context_id = $1 AND feature = $2 LIMIT 1",
    )
    .bind(account_id)
    .bind(feature)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(state,)| state == "enabled").unwrap_or(false))
}

/// Canvas `require_root_account_instrumented_or_sub_account_and_feature_flag`.
async fn account_context(pool: &PgPool, account_id: i64) -> Result<Ctx, ApiError> {
    require_account(pool, account_id).await?;

    let root_id = root_account_id(pool, account_id).await?;
    let is_root = root_id == account_id;

    if !is_root && !feature_enabled(pool, root_id, "canvas_apps_sub_account_access").await? {
        return Err(ApiError::NotFound);
    }

    Ok(Ctx {
        account_id,
        root_account_id: root_id,
        is_root_account: is_root,
        site_admin_id: site_admin_account_id(pool).await?,
    })
}

/// Canvas `require_manage_lti_registrations` — `403` without the account
/// admin right.
async fn require_manage(pool: &PgPool, auth: &AuthUser, account_id: i64) -> Result<(), ApiError> {
    if can_manage_account(pool, auth, account_id).await? {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Canvas `restrict_sub_account_to_read_only` — `403` for sub-accounts.
fn restrict_sub_account_to_read_only(ctx: &Ctx) -> Option<Response> {
    if ctx.is_root_account {
        None
    } else {
        Some(errors_string(
            StatusCode::FORBIDDEN,
            "sub-accounts can only view registrations",
        ))
    }
}

/// Canvas `require_in_account_or_site_admin` — `400` when the registration
/// belongs to neither the context account nor the site admin.
fn require_in_account_or_site_admin(ctx: &Ctx, registration: &RegistrationRow) -> Option<Response> {
    if registration.account_id == ctx.account_id || registration.account_id == ctx.site_admin_id {
        None
    } else {
        Some(errors_string(
            StatusCode::BAD_REQUEST,
            "registration does not belong to account",
        ))
    }
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

/// Canvas `Lti::Registration.active.find(id)` — `404` when the registration
/// does not exist or was soft-deleted.
async fn find_active_registration(pool: &PgPool, id: i64) -> Result<RegistrationRow, ApiError> {
    let sql = format!(
        "SELECT {REGISTRATION_COLUMNS} FROM lti_registrations \
         WHERE id = $1 AND workflow_state <> 'deleted'"
    );
    let row: Option<RegistrationRow> = sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
    row.ok_or(ApiError::NotFound)
}

/// The IMS registration linked to an LTI registration, if any.
async fn find_ims_registration(
    pool: &PgPool,
    lti_registration_id: i64,
) -> Result<Option<ImsRow>, ApiError> {
    let row: Option<ImsRow> = sqlx::query_as(
        "SELECT id, developer_key_id, registration_url, unified_tool_id, lti_registration_id \
         FROM lti_ims_registrations WHERE lti_registration_id = $1 LIMIT 1",
    )
    .bind(lti_registration_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// The LTI registration behind a developer key (`client_id`).
///
/// Canvas links the developer key to the registration through
/// `developer_keys.lti_registration_id`; the workspace table does not carry
/// that column yet, so the IMS registration (which references both) is used
/// instead.
async fn registration_for_client_id(
    pool: &PgPool,
    client_id: i64,
) -> Result<Option<RegistrationRow>, ApiError> {
    let ids: Vec<(Option<i64>,)> = sqlx::query_as(
        "SELECT lti_registration_id FROM lti_ims_registrations \
         WHERE developer_key_id = $1 AND lti_registration_id IS NOT NULL",
    )
    .bind(client_id)
    .fetch_all(pool)
    .await?;

    for (id,) in ids {
        if let Some(id) = id {
            let sql = format!(
                "SELECT {REGISTRATION_COLUMNS} FROM lti_registrations \
                 WHERE id = $1 AND workflow_state <> 'deleted'"
            );
            let row: Option<RegistrationRow> =
                sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
            if row.is_some() {
                return Ok(row);
            }
        }
    }
    Ok(None)
}

/// The local copy of a template registration in the given account.
async fn find_local_copy(
    pool: &PgPool,
    template_id: i64,
    account_id: i64,
) -> Result<Option<RegistrationRow>, ApiError> {
    let sql = format!(
        "SELECT {REGISTRATION_COLUMNS} FROM lti_registrations \
         WHERE template_registration_id = $1 AND account_id = $2 AND workflow_state <> 'deleted'"
    );
    let row: Option<RegistrationRow> = sqlx::query_as(&sql)
        .bind(template_id)
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// The account binding of a registration for the given account.
async fn find_binding(
    pool: &PgPool,
    registration_id: i64,
    account_id: i64,
) -> Result<Option<BindingRow>, ApiError> {
    let row: Option<BindingRow> = sqlx::query_as(
        "SELECT id, registration_id, account_id, root_account_id, workflow_state, \
                created_by_id, updated_by_id, created_at, updated_at \
         FROM lti_registration_account_bindings \
         WHERE registration_id = $1 AND account_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(registration_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Canvas `Lti::Registration#account_binding_for(context)`.
///
/// The site-admin binding wins; otherwise the binding of the account (its
/// root account when the context is a sub-account).
async fn account_binding_for(
    pool: &PgPool,
    registration: &RegistrationRow,
    ctx: &Ctx,
) -> Result<Option<BindingRow>, ApiError> {
    if let Some(binding) = find_binding(pool, registration.id, ctx.site_admin_id).await? {
        return Ok(Some(binding));
    }
    find_binding(pool, registration.id, ctx.root_account_id).await
}

/// Whether the table exists in the database — used by the endpoints whose
/// Canvas table the workspace does not have yet.
async fn table_exists(pool: &PgPool, name: &str) -> Result<bool, ApiError> {
    let row: Option<(bool,)> = sqlx::query_as("SELECT to_regclass($1) IS NOT NULL")
        .bind(format!("public.{name}"))
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(exists,)| exists).unwrap_or(false))
}

// ---------------------------------------------------------------------------
// JSON serialization
// ---------------------------------------------------------------------------

/// Canvas `user_json` subset used for `created_by` / `updated_by`.
async fn user_summary(pool: &PgPool, user_id: Option<i64>) -> Result<Option<Value>, ApiError> {
    let Some(user_id) = user_id else {
        return Ok(None);
    };
    let row: Option<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT id, name, sortable_name FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id, name, sortable_name)| {
        let sortable = sortable_name.unwrap_or_else(|| name.clone());
        json!({ "id": id, "name": name, "sortable_name": sortable })
    }))
}

/// Canvas `lti_registration_account_binding_json` shape.
fn binding_json(
    binding: &BindingRow,
    created_by: Option<Value>,
    updated_by: Option<Value>,
) -> Value {
    let mut json = json!({
        "id": binding.id,
        "account_id": binding.account_id,
        "root_account_id": binding.root_account_id,
        "registration_id": binding.registration_id,
        "workflow_state": binding.workflow_state,
        "created_at": binding.created_at,
        "updated_at": binding.updated_at,
    });
    if let Some(created_by) = created_by {
        json["created_by"] = created_by;
    }
    if let Some(updated_by) = updated_by {
        json["updated_by"] = updated_by;
    }
    json
}

/// Canvas `lti_registration_update_request_json` shape.
fn update_request_json(
    request: &UpdateRequestRow,
    created_by: Option<Value>,
    updated_by: Option<Value>,
) -> Value {
    let status = if request.accepted_at.is_some() {
        "applied"
    } else if request.rejected_at.is_some() {
        "rejected"
    } else {
        "pending"
    };

    let mut json = json!({
        "id": request.id,
        "root_account_id": request.root_account_id,
        "lti_registration_id": request.lti_registration_id,
        "uuid": request.uuid,
        "reinstall": request
            .extra
            .get("reinstall")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "tool_initiated": request
            .extra
            .get("tool_initiated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "comment": request.extra.get("comment").cloned().unwrap_or(Value::Null),
        "created_at": request.created_at,
        "updated_at": request.updated_at,
        "accepted_at": request.accepted_at,
        "rejected_at": request.rejected_at,
        "status": status,
    });
    if let Some(created_by) = created_by {
        json["created_by"] = created_by;
    }
    if let Some(updated_by) = updated_by {
        json["updated_by"] = updated_by;
    }
    json
}

/// The optional registration fields to embed in the response.
#[derive(Debug, Clone, Copy, Default)]
struct RegIncludes {
    configuration: bool,
    overlaid_configuration: bool,
    overlay: bool,
    overlay_versions: bool,
    account_binding: bool,
}

/// Canvas `lti_registration_json` shape.
async fn registration_json(
    pool: &PgPool,
    registration: &RegistrationRow,
    ctx: &Ctx,
    includes: RegIncludes,
) -> Result<Value, ApiError> {
    let ims = find_ims_registration(pool, registration.id).await?;

    let mut json = json!({
        "id": registration.id,
        "account_id": registration.account_id,
        "root_account_id": registration.root_account_id,
        "internal_service": registration.internal_service,
        "vendor": registration.vendor,
        "name": registration.name,
        "admin_nickname": registration.admin_nickname,
        "workflow_state": registration.workflow_state,
        "created_at": registration.created_at,
        "updated_at": registration.updated_at,
        "description": registration.description,
        "lock_deploying": registration.lock_deploying,
        "template_registration_id": registration.template_registration_id,
        "inherited": registration.template_registration_id.is_some()
            || registration.account_id != ctx.account_id,
        "lti_version": registration.lti_version,
        "icon_url": registration.icon_url.clone().or_else(|| {
            registration
                .configuration
                .get("launch_settings")
                .and_then(|settings| settings.get("icon_url"))
                .and_then(Value::as_str)
                .map(str::to_string)
        }),
        "developer_key_id": ims.as_ref().and_then(|ims| ims.developer_key_id),
        "ims_registration_id": ims.as_ref().map(|ims| ims.id),
        "dynamic_registration_url": ims.as_ref().and_then(|ims| ims.registration_url.clone()),
        "reinstall_disabled": Value::Null,
        "manual_configuration_id": Value::Null,
        "pending_update": Value::Null,
    });

    // Canvas only emits dynamic_registration when it is set.
    if registration.dynamic_registration {
        json["dynamic_registration"] = json!(true);
    }

    if let Some(created_by) = user_summary(pool, registration.created_by_id).await? {
        json["created_by"] = created_by;
    }
    if let Some(updated_by) = user_summary(pool, registration.updated_by_id).await? {
        json["updated_by"] = updated_by;
    }

    if includes.configuration {
        json["configuration"] = registration.configuration.clone();
    }
    if includes.overlaid_configuration {
        json["overlaid_configuration"] =
            apply_overlay(&registration.configuration, &overlay_data(registration));
    }
    if includes.overlay {
        json["overlay"] = overlay_data(registration);
    }
    if includes.overlay_versions {
        // Canvas nests the versions inside the overlay; the workspace keeps
        // them beside it because there is no `lti_overlays` table yet.
        json["overlay_versions"] = json!(overlay_versions(pool).await?);
    }
    if includes.account_binding {
        if let Some(binding) = account_binding_for(pool, registration, ctx).await? {
            let created_by = user_summary(pool, binding.created_by_id).await?;
            let updated_by = user_summary(pool, binding.updated_by_id).await?;
            json["account_binding"] = binding_json(&binding, created_by, updated_by);
        }
    }

    Ok(json)
}

/// The overlay data of a registration.
///
/// Canvas keeps one overlay per (registration, account) in `lti_overlays`;
/// the workspace stores a single overlay blob on the registration row.
fn overlay_data(registration: &RegistrationRow) -> Value {
    registration
        .overlay
        .clone()
        .unwrap_or_else(|| Value::Object(Default::default()))
}

/// Canvas `Lti::Overlay.apply_to(overlay, internal_config)`.
fn apply_overlay(config: &Value, overlay: &Value) -> Value {
    let mut result = config.clone();
    if !result.is_object() {
        return result;
    }
    let Some(overlay_map) = overlay.as_object() else {
        return result;
    };
    if overlay_map.is_empty() {
        return result;
    }

    let result_map = result.as_object_mut().expect("checked above");

    for key in OVERLAY_ROOT_KEYS {
        if let Some(value) = overlay_map.get(*key) {
            result_map.insert((*key).to_string(), value.clone());
        }
    }

    let mut launch_settings = result_map
        .get("launch_settings")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for key in OVERLAY_LAUNCH_SETTINGS_KEYS {
        if let Some(value) = overlay_map.get(*key) {
            launch_settings.insert((*key).to_string(), value.clone());
        }
    }
    if !launch_settings.is_empty() {
        result_map.insert(
            "launch_settings".to_string(),
            Value::Object(launch_settings),
        );
    }

    // disabled_scopes takes precedence over scopes.
    if let Some(disabled) = overlay_map.get("disabled_scopes").and_then(Value::as_array) {
        if let Some(scopes) = result_map.get_mut("scopes").and_then(Value::as_array_mut) {
            scopes.retain(|scope| !disabled.iter().any(|d| d == scope));
        }
    }

    // The per-placement overlay wins over the base placement.
    if let Some(placements_overlay) = overlay_map.get("placements").and_then(Value::as_object) {
        if let Some(placements) = result_map
            .get_mut("placements")
            .and_then(Value::as_array_mut)
        {
            for placement in placements.iter_mut() {
                let Some(placement_map) = placement.as_object_mut() else {
                    continue;
                };
                let Some(name) = placement_map.get("placement").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(overlay_placement) = placements_overlay.get(name) {
                    if let Some(overlay_placement) = overlay_placement.as_object() {
                        for (key, value) in overlay_placement {
                            placement_map.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
        }
    }

    // disabled_placements takes precedence over placements.
    if let Some(disabled) = overlay_map
        .get("disabled_placements")
        .and_then(Value::as_array)
    {
        if let Some(placements) = result_map
            .get_mut("placements")
            .and_then(Value::as_array_mut)
        {
            for placement in placements.iter_mut() {
                let Some(placement_map) = placement.as_object_mut() else {
                    continue;
                };
                let Some(name) = placement_map.get("placement").and_then(Value::as_str) else {
                    continue;
                };
                if disabled.iter().any(|d| d.as_str() == Some(name)) {
                    placement_map.insert("enabled".to_string(), json!(false));
                }
            }
        }
    }

    result
}

/// The overlay edit history of a registration.
///
/// Canvas reads `lti_overlay_versions`; the workspace has no such table yet,
/// so the collection is empty until the DDL lands.
async fn overlay_versions(pool: &PgPool) -> Result<Vec<Value>, ApiError> {
    if !table_exists(pool, "lti_overlay_versions").await? {
        return Ok(Vec::new());
    }

    let rows: Vec<(
        i64,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Value,
        bool,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, account_id, root_account_id, lti_overlay_id, diff, caused_by_reset, \
                created_at, updated_at \
         FROM lti_overlay_versions ORDER BY created_at DESC LIMIT $1",
    )
    .bind(DEFAULT_PER_PAGE)
    .fetch_all(pool)
    .await?;

    let mut versions = Vec::with_capacity(rows.len());
    for (
        id,
        account_id,
        root_account_id,
        lti_overlay_id,
        diff,
        caused_by_reset,
        created_at,
        updated_at,
    ) in rows
    {
        versions.push(json!({
            "id": id,
            "account_id": account_id,
            "root_account_id": root_account_id,
            "lti_overlay_id": lti_overlay_id,
            "created_at": created_at,
            "updated_at": updated_at,
            "diff": diff,
            "caused_by_reset": caused_by_reset,
        }));
    }
    Ok(versions)
}

// ---------------------------------------------------------------------------
// Schema validation (Schemas::LtiConfiguration / InternalLtiConfiguration)
// ---------------------------------------------------------------------------

/// JSONSchemer's message for a missing required property.
fn required_error(pointer: &str, missing: &str) -> String {
    format!("The property '{pointer}' did not contain a required property of '{missing}'.")
}

/// JSONSchemer's message for a type mismatch.
fn type_error(pointer: &str, actual: &str, expected: &str) -> String {
    format!(
        "The property '{pointer}' of type {actual} did not match the following type: {expected}."
    )
}

/// The JSON type name of a value, as JSONSchemer reports it.
fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Validate the value against a JSON schema property type.
fn check_type(errors: &mut Vec<String>, pointer: &str, value: &Value, expected: &[&str]) -> bool {
    let actual = json_type(value);
    if expected.contains(&actual) {
        true
    } else {
        errors.push(type_error(pointer, actual, &expected.join(" or ")));
        false
    }
}

/// Canvas `Schemas::LtiConfiguration.validation_errors(config, allow_nil: true)`.
///
/// The schema requires `title`, `description`, `target_link_uri` and
/// `oidc_initiation_url` and type-checks the base properties.
fn lti_configuration_errors(config: &Value) -> Vec<String> {
    let mut errors = Vec::new();

    let Some(map) = config.as_object() else {
        errors.push(type_error("#/", json_type(config), "object"));
        return errors;
    };

    for required in [
        "title",
        "description",
        "target_link_uri",
        "oidc_initiation_url",
    ] {
        if !map.contains_key(required) {
            errors.push(required_error("#/", required));
        }
    }

    for property in [
        "title",
        "description",
        "target_link_uri",
        "oidc_initiation_url",
        "oidc_initiation_urls",
        "public_jwk_url",
    ] {
        if let Some(value) = map.get(property) {
            check_type(&mut errors, &format!("#/{property}"), value, &["string"]);
        }
    }

    if let Some(value) = map.get("custom_fields") {
        check_type(&mut errors, "#/custom_fields", value, &["object", "string"]);
    }
    if let Some(value) = map.get("public_jwk") {
        check_type(&mut errors, "#/public_jwk", value, &["object", "array"]);
    }
    if let Some(value) = map.get("scopes") {
        if check_type(&mut errors, "#/scopes", value, &["array"]) {
            for (index, item) in value
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                check_type(&mut errors, &format!("#/scopes/{index}"), item, &["string"]);
            }
        }
    }

    if let Some(value) = map.get("extensions") {
        validate_extensions(&mut errors, value);
    }

    errors
}

/// The `extensions` array of the LTI configuration schema.
fn validate_extensions(errors: &mut Vec<String>, extensions: &Value) {
    if !check_type(errors, "#/extensions", extensions, &["array"]) {
        return;
    }

    for (index, extension) in extensions
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .enumerate()
    {
        let pointer = format!("#/extensions/{index}");
        let Some(extension) = extension.as_object() else {
            errors.push(type_error(&pointer, json_type(extension), "object"));
            continue;
        };

        for required in ["platform", "settings"] {
            if !extension.contains_key(required) {
                errors.push(required_error(&pointer, required));
            }
        }
        if let Some(value) = extension.get("platform") {
            check_type(errors, &format!("{pointer}/platform"), value, &["string"]);
        }
        for property in ["domain", "tool_id", "privacy_level"] {
            if let Some(value) = extension.get(property) {
                check_type(errors, &format!("{pointer}/{property}"), value, &["string"]);
            }
        }
        if let Some(settings) = extension.get("settings") {
            let settings_pointer = format!("{pointer}/settings");
            if !check_type(errors, &settings_pointer, settings, &["object"]) {
                continue;
            }
            let settings = settings.as_object().unwrap();
            if !settings.contains_key("placements") {
                errors.push(required_error(&settings_pointer, "placements"));
            }
        }
    }
}

/// Canvas `Schemas::InternalLtiConfiguration.validation_errors(config, allow_nil: true)`.
fn internal_configuration_errors(config: &Value) -> Vec<String> {
    let mut errors = Vec::new();

    let Some(map) = config.as_object() else {
        errors.push(type_error("#/", json_type(config), "object"));
        return errors;
    };

    for required in [
        "title",
        "target_link_uri",
        "oidc_initiation_url",
        "redirect_uris",
        "scopes",
        "placements",
        "launch_settings",
    ] {
        if !map.contains_key(required) {
            errors.push(required_error("#/", required));
        }
    }

    for property in [
        "title",
        "description",
        "target_link_uri",
        "oidc_initiation_url",
        "oidc_initiation_urls",
        "public_jwk_url",
        "domain",
        "tool_id",
        "privacy_level",
    ] {
        if let Some(value) = map.get(property) {
            check_type(&mut errors, &format!("#/{property}"), value, &["string"]);
        }
    }

    if let Some(value) = map.get("custom_fields") {
        check_type(&mut errors, "#/custom_fields", value, &["object", "string"]);
    }
    if let Some(value) = map.get("public_jwk") {
        check_type(&mut errors, "#/public_jwk", value, &["object", "array"]);
    }
    if let Some(value) = map.get("redirect_uris") {
        if check_type(&mut errors, "#/redirect_uris", value, &["array"]) {
            let items = value.as_array().map(Vec::as_slice).unwrap_or_default();
            if items.is_empty() {
                errors.push(
                    "The property '#/redirect_uris' did not have a minimum of 1 items.".to_string(),
                );
            }
            for (index, item) in items.iter().enumerate() {
                check_type(
                    &mut errors,
                    &format!("#/redirect_uris/{index}"),
                    item,
                    &["string"],
                );
            }
        }
    }
    if let Some(value) = map.get("scopes") {
        if check_type(&mut errors, "#/scopes", value, &["array"]) {
            for (index, item) in value
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                check_type(&mut errors, &format!("#/scopes/{index}"), item, &["string"]);
            }
        }
    }
    if let Some(value) = map.get("placements") {
        if check_type(&mut errors, "#/placements", value, &["array"]) {
            for (index, placement) in value
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default()
                .iter()
                .enumerate()
            {
                let pointer = format!("#/placements/{index}");
                let Some(placement) = placement.as_object() else {
                    errors.push(type_error(&pointer, json_type(placement), "object"));
                    continue;
                };
                if !placement.contains_key("placement") {
                    errors.push(required_error(&pointer, "placement"));
                }
            }
        }
    }
    if let Some(value) = map.get("launch_settings") {
        if check_type(&mut errors, "#/launch_settings", value, &["object"]) {
            let settings = value.as_object().unwrap();
            if let Some(icon_url) = settings.get("icon_url") {
                check_type(
                    &mut errors,
                    "#/launch_settings/icon_url",
                    icon_url,
                    &["string"],
                );
            }
        }
    }

    errors
}

/// Canvas `Schemas::Lti::Overlay.validation_errors(overlay, allow_nil: true)`.
fn overlay_errors(overlay: &Value) -> Vec<String> {
    let mut errors = Vec::new();

    let Some(map) = overlay.as_object() else {
        errors.push(type_error("#/", json_type(overlay), "object"));
        return errors;
    };

    for property in [
        "title",
        "description",
        "target_link_uri",
        "oidc_initiation_url",
        "icon_url",
        "domain",
        "privacy_level",
    ] {
        if let Some(value) = map.get(property) {
            check_type(&mut errors, &format!("#/{property}"), value, &["string"]);
        }
    }
    if let Some(value) = map.get("custom_fields") {
        check_type(&mut errors, "#/custom_fields", value, &["object"]);
    }
    for property in ["disabled_scopes", "disabled_placements"] {
        if let Some(value) = map.get(property) {
            if check_type(&mut errors, &format!("#/{property}"), value, &["array"]) {
                for (index, item) in value
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    check_type(
                        &mut errors,
                        &format!("#/{property}/{index}"),
                        item,
                        &["string"],
                    );
                }
            }
        }
    }
    if let Some(value) = map.get("placements") {
        if check_type(&mut errors, "#/placements", value, &["object"]) {
            for (name, placement) in value.as_object().unwrap().iter() {
                let pointer = format!("#/placements/{name}");
                if !check_type(&mut errors, &pointer, placement, &["object"]) {
                    continue;
                }
                for property in ["text", "target_link_uri", "icon_url"] {
                    if let Some(value) = placement.as_object().unwrap().get(property) {
                        check_type(
                            &mut errors,
                            &format!("{pointer}/{property}"),
                            value,
                            &["string"],
                        );
                    }
                }
            }
        }
    }

    errors
}

/// Canvas `Schemas::InternalLtiConfiguration.from_lti_configuration(config)`.
fn from_lti_configuration(config: &Value) -> Value {
    let mut result = config.clone();
    if !result.is_object() {
        return result;
    }
    let map = result.as_object_mut().expect("checked above");

    let extensions = map
        .remove("extensions")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();

    let mut canvas_extension: Option<Value> = None;
    let mut vendor_extensions = Vec::new();
    for extension in extensions {
        let platform = extension.get("platform").and_then(Value::as_str);
        if platform == Some("canvas.instructure.com") {
            // Canvas keeps the first Canvas extension only.
            if canvas_extension.is_none() {
                canvas_extension = Some(extension);
            }
        } else {
            vendor_extensions.push(extension);
        }
    }

    if !vendor_extensions.is_empty() {
        map.insert(
            "vendor_extensions".to_string(),
            Value::Array(vendor_extensions),
        );
    }

    if !map.contains_key("scopes") {
        map.insert("scopes".to_string(), json!([]));
    }

    let mut canvas_map = canvas_extension
        .and_then(|extension| extension.as_object().cloned())
        .unwrap_or_default();
    canvas_map.remove("platform");

    let mut launch_settings = canvas_map
        .remove("settings")
        .and_then(|settings| settings.as_object().cloned())
        .unwrap_or_default();
    let placements = launch_settings
        .remove("placements")
        .unwrap_or_else(|| json!([]));
    launch_settings.retain(|key, _| !PLACEMENTS.contains(&key.as_str()));

    for (key, value) in canvas_map {
        map.insert(key, value);
    }
    map.insert(
        "launch_settings".to_string(),
        Value::Object(launch_settings),
    );
    map.insert("placements".to_string(), placements);

    result
}

// ---------------------------------------------------------------------------
// Workflow state
// ---------------------------------------------------------------------------

/// Canvas `Lti::AccountBindingService::WORKFLOW_STATE_RESOLUTION`.
fn resolve_workflow_state(value: Option<&str>) -> Option<(&'static str, &'static str)> {
    match value.unwrap_or("on") {
        "on" => Some(("on", "active")),
        "off" => Some(("off", "inactive")),
        "allow" => Some(("allow", "active")),
        "active" => Some(("on", "active")),
        "inactive" => Some(("off", "inactive")),
        _ => None,
    }
}

/// Canvas `validate_workflow_state`.
fn validate_workflow_state(ctx: &Ctx, value: Option<&str>) -> Option<Response> {
    let Some(value) = value else {
        return None;
    };
    if resolve_workflow_state(Some(value)).is_some() && value != "allow" {
        return None;
    }
    if value == "allow" && ctx.account_id == ctx.site_admin_id {
        return None;
    }
    if value == "allow" {
        Some(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_workflow_state",
            "only site admin registrations can have a state of 'allow'",
        ))
    } else {
        Some(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_workflow_state",
            "workflow_state must be one of 'on', 'off', 'allow', 'active', or 'inactive'",
        ))
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /accounts/{account_id}/lti_registrations/configuration/validate`.
///
/// Canvas `Lti::RegistrationsController#validate_lti_configuration`.
pub async fn validate_configuration(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    // `validate_lti_configuration` is not in the controller's `except:` list,
    // so sub-accounts may not use it.
    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    let payload: ValidateConfigurationBody = parse_body(&body)?.unwrap_or_default();

    let has_configuration = payload
        .lti_configuration
        .as_ref()
        .is_some_and(|value| !value.is_null());
    let has_url = payload
        .url
        .as_ref()
        .is_some_and(|url| !url.trim().is_empty());

    if !has_configuration && !has_url {
        return Ok(configuration_errors(vec![
            "one of lti_configuration or url is required".to_string(),
        ]));
    }
    if has_configuration && has_url {
        return Ok(configuration_errors(vec![
            "only one of lti_configuration or url is allowed".to_string(),
        ]));
    }

    let config = match payload.lti_configuration {
        Some(config) => config,
        // Canvas fetches the configuration over HTTP here. The workspace crate
        // has no HTTP client dependency, so the URL branch cannot be served.
        None => {
            return Ok(configuration_errors(vec![
                "invalid configuration url".to_string()
            ]))
        }
    };

    let errors = lti_configuration_errors(&config);
    if !errors.is_empty() {
        return Ok(configuration_errors(errors));
    }

    let mut configuration = from_lti_configuration(&config);

    // Canvas adds the redirect URIs so the UI can display them.
    {
        let map = configuration.as_object_mut().unwrap();
        let target_link_uri = map.get("target_link_uri").cloned().unwrap_or(Value::Null);
        match map.get("redirect_uris") {
            Some(Value::Array(_)) => {}
            Some(Value::String(value)) => {
                let single = value.clone();
                map.insert("redirect_uris".to_string(), json!([single]));
            }
            _ => {
                map.insert("redirect_uris".to_string(), json!([target_link_uri]));
            }
        }
    }

    Ok(Json(json!({ "configuration": configuration })).into_response())
}

/// `GET /accounts/{account_id}/lti_registrations/check_domain_duplicates`.
///
/// Canvas `Lti::RegistrationsController#check_domain_duplicates` backed by
/// `Lti::CheckDomainDuplicatesService`.
pub async fn check_domain_duplicates(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<DomainDuplicatesQuery>,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    let domain = params.domain.unwrap_or_default();
    if domain.trim().is_empty() {
        return Ok(Json(json!({ "duplicates": [] })).into_response());
    }
    let normalized = domain.trim().to_lowercase();

    let templates_enabled = feature_enabled(
        &state.db,
        ctx.root_account_id,
        "lti_registrations_templates",
    )
    .await?;

    let mut duplicates: Vec<Value> = Vec::new();

    if templates_enabled {
        // With templates enabled only the account's own registrations count
        // (the local copies carry the domain of their template).
        duplicates.extend(
            registrations_with_domain(
                &state.db,
                &ctx,
                &normalized,
                "r.account_id = $1 AND r.workflow_state <> 'deleted'",
            )
            .await?,
        );
    } else {
        // The account's own registrations, without the local copies.
        duplicates.extend(
            registrations_with_domain(
                &state.db,
                &ctx,
                &normalized,
                "r.account_id = $1 AND r.workflow_state <> 'deleted' \
                 AND r.template_registration_id IS NULL",
            )
            .await?,
        );

        // The site-admin registrations that are forced on.
        let forced_on = registrations_with_domain(
            &state.db,
            &ctx,
            &normalized,
            "r.account_id = $2 AND r.workflow_state <> 'deleted' \
             AND EXISTS (SELECT 1 FROM lti_registration_account_bindings b \
                         WHERE b.registration_id = r.id AND b.account_id = $2 \
                           AND b.workflow_state = 'on')",
        )
        .await?;
        let forced_on_ids: Vec<i64> = forced_on
            .iter()
            .filter_map(|entry| entry.get("id").and_then(Value::as_i64))
            .collect();
        duplicates.extend(forced_on);

        // The inherited registrations that are enabled for this account.
        let inherited = registrations_with_domain(
            &state.db,
            &ctx,
            &normalized,
            "r.account_id = $2 AND r.workflow_state <> 'deleted' \
             AND EXISTS (SELECT 1 FROM lti_registration_account_bindings b \
                         WHERE b.registration_id = r.id AND b.account_id = $1 \
                           AND b.workflow_state = 'on')",
        )
        .await?;
        duplicates.extend(
            inherited
                .into_iter()
                .filter(|entry| {
                    !forced_on_ids.contains(&entry.get("id").and_then(Value::as_i64).unwrap_or(-1))
                })
                .collect::<Vec<Value>>(),
        );
    }

    Ok(Json(json!({ "duplicates": duplicates })).into_response())
}

/// The registrations of a scope whose domain matches, limited to three.
async fn registrations_with_domain(
    pool: &PgPool,
    ctx: &Ctx,
    normalized_domain: &str,
    scope: &str,
) -> Result<Vec<Value>, ApiError> {
    let sql = format!(
        "SELECT r.id, r.name, r.admin_nickname FROM lti_registrations r \
         WHERE {scope} \
         AND (LOWER(r.configuration->>'domain') = $3 \
              OR LOWER((SELECT i.lti_tool_configuration->>'domain' \
                        FROM lti_ims_registrations i \
                        WHERE i.lti_registration_id = r.id LIMIT 1)) = $3) \
         LIMIT 3"
    );

    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(&sql)
        .bind(ctx.account_id)
        .bind(ctx.site_admin_id)
        .bind(normalized_domain)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|(id, name, admin_nickname)| {
            json!({ "id": id, "name": name, "admin_nickname": admin_nickname })
        })
        .collect())
}

/// `GET /accounts/{account_id}/lti_registrations/{registration_id}/deployments/{deployment_id}/context_search`.
///
/// Canvas `Lti::RegistrationsController#context_search`.
pub async fn context_search(
    State(state): State<AppState>,
    Path((account_id, registration_id, deployment_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Query(params): Query<ContextSearchQuery>,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;

    // require_lti_registrations_next_feature_flag
    if !feature_enabled(&state.db, ctx.root_account_id, "lti_registrations_next").await? {
        return Ok(not_found_render_error());
    }

    require_manage(&state.db, &auth, account_id).await?;

    let registration = find_active_registration(&state.db, registration_id).await?;

    let deployment: Option<DeploymentRow> = sqlx::query_as(
        "SELECT id, registration_id, context_type, context_id FROM lti_deployments \
         WHERE id = $1 AND registration_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(deployment_id)
    .bind(registration.id)
    .fetch_optional(&state.db)
    .await?;

    let Some(deployment) = deployment else {
        return Err(ApiError::NotFound);
    };

    if deployment.context_type != "Account" {
        return Ok(Json(json!({ "accounts": [], "courses": [] })).into_response());
    }

    // The accounts below the deployment's account.
    let descendants: Vec<(i64,)> = sqlx::query_as(
        "WITH RECURSIVE sub_accounts AS ( \
             SELECT id, parent_account_id FROM accounts WHERE parent_account_id = $1 \
             UNION ALL \
             SELECT a.id, a.parent_account_id FROM accounts a \
               JOIN sub_accounts s ON a.parent_account_id = s.id \
         ) SELECT id FROM sub_accounts",
    )
    .bind(deployment.context_id)
    .fetch_all(&state.db)
    .await?;
    let descendant_ids: Vec<i64> = descendants.into_iter().map(|(id,)| id).collect();

    let only_children_of = params
        .only_children_of
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if let Some(children_of) = &only_children_of {
        let children_of_id = children_of.parse::<i64>().unwrap_or(0);
        if !descendant_ids.contains(&children_of_id) && children_of_id != deployment.context_id {
            return Ok(render_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_account_id",
                "only_children_of account ID must be or belong to deployment's account",
            ));
        }
    }

    let search_term = params
        .search_term
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let search_term_int = search_term.parse::<i64>().ok().filter(|id| *id > 0);

    // The accounts.
    let account_rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM accounts \
         WHERE workflow_state <> 'deleted' AND id = ANY($1) \
         AND ($2::text IS NULL OR name ILIKE '%' || $2 || '%' \
              OR ($3::bigint IS NOT NULL AND id = $3)) \
         ORDER BY name LIMIT 20",
    )
    .bind(descendant_ids.clone())
    .bind(search_term.as_str())
    .bind(search_term_int)
    .fetch_all(&state.db)
    .await?;

    // The courses.
    let course_rows: Vec<(i64, String, String, i64)> = if let Some(children_of) = &only_children_of
    {
        let children_of_id = children_of.parse::<i64>().unwrap_or(0);
        sqlx::query_as(
            "SELECT id, name, course_code, account_id FROM courses \
             WHERE workflow_state = 'available' AND account_id = $1 \
             AND ($2::text IS NULL OR name ILIKE '%' || $2 || '%' \
                  OR course_code ILIKE '%' || $2 || '%' \
                  OR ($3::bigint IS NOT NULL AND id = $3)) \
             ORDER BY name LIMIT 20",
        )
        .bind(children_of_id)
        .bind(search_term.as_str())
        .bind(search_term_int)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, name, course_code, account_id FROM courses \
             WHERE workflow_state = 'available' \
             AND (account_id = ANY($1) OR account_id = $2) \
             AND ($3::text IS NULL OR name ILIKE '%' || $3 || '%' \
                  OR course_code ILIKE '%' || $3 || '%' \
                  OR ($4::bigint IS NOT NULL AND id = $4)) \
             ORDER BY name LIMIT 20",
        )
        .bind(descendant_ids.clone())
        .bind(deployment.context_id)
        .bind(search_term.as_str())
        .bind(search_term_int)
        .fetch_all(&state.db)
        .await?
    };

    if account_rows.is_empty() && course_rows.is_empty() {
        return Ok(Json(json!({ "accounts": [], "courses": [] })).into_response());
    }

    // The account chain of every account involved, for the display paths.
    let mut chain_ids: Vec<i64> = account_rows.iter().map(|(id, _)| *id).collect();
    chain_ids.extend(course_rows.iter().map(|(_, _, _, account_id)| *account_id));
    chain_ids.sort_unstable();
    chain_ids.dedup();

    let chain: Vec<(i64, i64, i64)> = sqlx::query_as(
        "WITH RECURSIVE account_chain AS ( \
             SELECT id, parent_account_id, id AS original_id, 0 AS level \
               FROM accounts WHERE id = ANY($1) \
             UNION ALL \
             SELECT a.id, a.parent_account_id, c.original_id, c.level + 1 \
               FROM accounts a JOIN account_chain c ON a.id = c.parent_account_id \
         ) SELECT id, original_id, level FROM account_chain ORDER BY original_id, level",
    )
    .bind(chain_ids.clone())
    .fetch_all(&state.db)
    .await?;

    let names: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM accounts WHERE id = ANY($1)")
            .bind(chain_ids)
            .fetch_all(&state.db)
            .await?;
    let mut name_map: std::collections::HashMap<i64, String> = Default::default();
    for (id, name) in names {
        name_map.insert(id, name);
    }

    // The chain of an account, from just below the root down to the account.
    let chain_for = |account_id: i64| -> Vec<i64> {
        let mut ids: Vec<i64> = chain
            .iter()
            .filter(|(_, original, _)| *original == account_id)
            .map(|(id, _, _)| *id)
            .collect();
        ids.reverse();
        ids
    };

    let accounts_json: Vec<Value> = account_rows
        .iter()
        .map(|(id, name)| {
            let display_path: Vec<String> = chain_for(*id)
                .into_iter()
                .filter(|chain_id| chain_id != id)
                .filter_map(|chain_id| name_map.get(&chain_id).cloned())
                .collect();
            json!({
                "id": id.to_string(),
                "name": name,
                "sis_id": Value::Null,
                "display_path": display_path,
            })
        })
        .collect();

    let courses_json: Vec<Value> = course_rows
        .iter()
        .map(|(id, name, course_code, account_id)| {
            let display_path: Vec<String> = chain_for(*account_id)
                .into_iter()
                .filter_map(|chain_id| name_map.get(&chain_id).cloned())
                .collect();
            json!({
                "id": id.to_string(),
                "name": name,
                "sis_id": Value::Null,
                "course_code": course_code,
                "display_path": display_path,
            })
        })
        .collect();

    Ok(Json(json!({ "accounts": accounts_json, "courses": courses_json })).into_response())
}

/// `GET /accounts/{account_id}/lti_registrations/{id}/overlay_history`.
///
/// Canvas `Lti::RegistrationsController#overlay_history`.
pub async fn overlay_history(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<LimitQuery>,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;

    // require_lti_registrations_next_feature_flag
    if !feature_enabled(&state.db, ctx.root_account_id, "lti_registrations_next").await? {
        return Ok(not_found_render_error());
    }

    require_manage(&state.db, &auth, account_id).await?;

    let registration = find_active_registration(&state.db, id).await?;

    if let Some(response) = require_in_account_or_site_admin(&ctx, &registration) {
        return Ok(response);
    }

    // Canvas only returns the history when the account has an overlay.
    if registration.overlay.is_none() {
        return Ok(Json(json!([])).into_response());
    }

    let limit = match params.limit.as_deref().map(str::trim) {
        None | Some("") => DEFAULT_PER_PAGE,
        Some(raw) => {
            let limit = raw.parse::<i64>().unwrap_or(0);
            if limit <= 0 {
                return Ok(render_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_limit",
                    "limit must be a positive integer",
                ));
            }
            if limit > MAX_PER_PAGE {
                return Ok(render_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_limit",
                    &format!("limit cannot exceed {MAX_PER_PAGE}"),
                ));
            }
            limit
        }
    };

    let versions = overlay_versions(&state.db).await?;
    let versions: Vec<Value> = versions.into_iter().take(limit as usize).collect();
    Ok(Json(json!(versions)).into_response())
}

/// `GET /accounts/{account_id}/lti_registrations/{id}/history`.
///
/// Canvas `Lti::RegistrationsController#history`.
pub async fn history(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<HistoryQuery>,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;

    // require_lti_registrations_history_feature_flag
    if !feature_enabled(&state.db, ctx.root_account_id, "lti_registrations_history").await? {
        return Ok(not_found_render_error());
    }

    require_manage(&state.db, &auth, account_id).await?;

    let registration = find_active_registration(&state.db, id).await?;

    if let Some(response) = require_in_account_or_site_admin(&ctx, &registration) {
        return Ok(response);
    }

    let page = params.page.unwrap_or(1).max(1);
    let per_page = params
        .per_page
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let offset = (page - 1) * per_page;

    if !table_exists(&state.db, "lti_registration_history_entries").await? {
        return Ok(Json(json!([])).into_response());
    }

    let rows: Vec<(
        i64,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Value,
        String,
        Option<String>,
        Option<Value>,
        Option<Value>,
        Option<Value>,
        Option<Value>,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        "SELECT id, root_account_id, lti_registration_id, created_by_id, diff, update_type, \
                comment, old_configuration, new_configuration, old_context_controls, \
                new_context_controls, created_at, updated_at \
         FROM lti_registration_history_entries \
         WHERE lti_registration_id = $1 AND root_account_id = $2 \
         ORDER BY created_at DESC, id DESC LIMIT $3 OFFSET $4",
    )
    .bind(registration.id)
    .bind(ctx.account_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let mut entries = Vec::with_capacity(rows.len());
    for (
        entry_id,
        root_account_id,
        lti_registration_id,
        created_by_id,
        diff,
        update_type,
        comment,
        old_configuration,
        new_configuration,
        old_context_controls,
        new_context_controls,
        created_at,
        updated_at,
    ) in rows
    {
        let mut json = json!({
            "id": entry_id,
            "root_account_id": root_account_id,
            "lti_registration_id": lti_registration_id,
            "created_at": created_at,
            "updated_at": updated_at,
            "diff": diff,
            "update_type": update_type,
            "comment": comment,
            "old_configuration": old_configuration,
            "new_configuration": new_configuration,
            "old_controls_by_deployment": old_context_controls
                .as_ref()
                .map(|_| Vec::<Value>::new())
                .unwrap_or_else(|| Vec::new()),
            "new_controls_by_deployment": new_context_controls
                .as_ref()
                .map(|_| Vec::<Value>::new())
                .unwrap_or_else(|| Vec::new()),
        });
        if let Some(created_by) = user_summary(&state.db, created_by_id).await? {
            json["created_by"] = created_by;
        }
        entries.push(json);
    }

    Ok(Json(json!(entries)).into_response())
}

/// `GET /accounts/{account_id}/lti_registration_by_client_id/{client_id}`.
///
/// Canvas `Lti::RegistrationsController#show_by_client_id`.
pub async fn show_by_client_id(
    State(state): State<AppState>,
    Path((account_id, client_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    let registration = registration_for_client_id(&state.db, client_id).await?;
    let Some(registration) = registration else {
        return Ok(errors_string(
            StatusCode::NOT_FOUND,
            "LTI registration not found",
        ));
    };

    // The registration must be active and owned by the context or the site
    // admin.
    if !(registration.account_id == ctx.account_id || registration.account_id == ctx.site_admin_id)
    {
        return Ok(errors_string(
            StatusCode::NOT_FOUND,
            "LTI registration not found",
        ));
    }

    let json = registration_json(
        &state.db,
        &registration,
        &ctx,
        RegIncludes {
            configuration: true,
            overlaid_configuration: true,
            account_binding: true,
            ..Default::default()
        },
    )
    .await?;

    Ok(Json(json).into_response())
}

/// `GET /accounts/{account_id}/lti_registrations/by_utid/{utid}`.
///
/// Canvas `Lti::RegistrationsController#show_by_utid`.
pub async fn show_by_utid(
    State(state): State<AppState>,
    Path((account_id, utid)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    // The template feature is checked on the context account.
    if !feature_enabled(&state.db, ctx.account_id, "lti_registrations_templates").await? {
        return Ok(errors_string(StatusCode::NOT_FOUND, "Not found"));
    }

    // The manual configuration first, then the IMS registration.
    let sql = format!(
        "SELECT {REGISTRATION_COLUMNS} FROM lti_registrations \
         WHERE unified_tool_id = $1 AND workflow_state <> 'deleted' LIMIT 1"
    );
    let manual: Option<RegistrationRow> = sqlx::query_as(&sql)
        .bind(&utid)
        .fetch_optional(&state.db)
        .await?;

    let registration = match manual {
        Some(registration) => Some(registration),
        None => {
            let ims_ids: Vec<(Option<i64>,)> = sqlx::query_as(
                "SELECT lti_registration_id FROM lti_ims_registrations \
                 WHERE unified_tool_id = $1 AND lti_registration_id IS NOT NULL",
            )
            .bind(&utid)
            .fetch_all(&state.db)
            .await?;
            let mut found = None;
            for (id,) in ims_ids {
                if let Some(id) = id {
                    if let Ok(registration) = find_active_registration(&state.db, id).await {
                        found = Some(registration);
                        break;
                    }
                }
            }
            found
        }
    };

    let Some(registration) = registration else {
        return Ok(errors_string(
            StatusCode::NOT_FOUND,
            "LTI registration not found",
        ));
    };
    if registration.account_id != ctx.account_id {
        return Ok(errors_string(
            StatusCode::NOT_FOUND,
            "LTI registration not found",
        ));
    }

    let json = registration_json(&state.db, &registration, &ctx, RegIncludes::default()).await?;

    Ok(Json(json).into_response())
}

/// `GET /accounts/{account_id}/lti_registrations/install_status/{client_id}`.
///
/// Canvas `Lti::RegistrationsController#install_status`.
pub async fn install_status(
    State(state): State<AppState>,
    Path((account_id, client_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    if !feature_enabled(&state.db, ctx.account_id, "lti_registrations_templates").await? {
        return Ok(errors_string(StatusCode::NOT_FOUND, "Not found"));
    }

    let template = registration_for_client_id(&state.db, client_id).await?;
    let Some(template) = template else {
        return Ok(errors_string(
            StatusCode::NOT_FOUND,
            "LTI registration not found",
        ));
    };

    let registration = if template.account_id == ctx.site_admin_id {
        match find_local_copy(&state.db, template.id, ctx.account_id).await? {
            Some(local_copy) => local_copy,
            None => {
                return Ok(errors_string(
                    StatusCode::NOT_FOUND,
                    "LTI registration not found",
                ))
            }
        }
    } else if template.account_id == ctx.account_id {
        template
    } else {
        return Ok(errors_string(
            StatusCode::NOT_FOUND,
            "LTI registration not found",
        ));
    };

    if registration.workflow_state != "active" {
        return Ok(errors_string(
            StatusCode::NOT_FOUND,
            "LTI registration not found",
        ));
    }

    let json = registration_json(&state.db, &registration, &ctx, RegIncludes::default()).await?;

    Ok(Json(json).into_response())
}

/// `GET /accounts/{account_id}/lti_registrations/{id}/update_requests/{update_request_id}`.
///
/// Canvas `Lti::RegistrationsController#show_registration_update_request`.
pub async fn show_update_request(
    State(state): State<AppState>,
    Path((account_id, id, update_request_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    let registration = find_active_registration(&state.db, id).await?;

    let request =
        find_update_request(&state.db, update_request_id, Some(registration.id), false).await?;
    let Some(request) = request else {
        return Err(ApiError::NotFound);
    };

    if registration.account_id != ctx.account_id {
        return Ok(errors_string(
            StatusCode::BAD_REQUEST,
            "registration does not belong to account",
        ));
    }

    let created_by = user_summary(&state.db, request.created_by_id).await?;
    let updated_by = user_summary(
        &state.db,
        request.extra.get("updated_by_id").and_then(Value::as_i64),
    )
    .await?;

    Ok(Json(update_request_json(&request, created_by, updated_by)).into_response())
}

/// `GET /accounts/{account_id}/lti_registrations/{id}/latest_update_request`.
///
/// Canvas `Lti::RegistrationsController#latest_registration_update_request`.
pub async fn latest_update_request(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    let registration = find_active_registration(&state.db, id).await?;

    if registration.account_id != ctx.account_id {
        return Ok(errors_string(
            StatusCode::BAD_REQUEST,
            "registration does not belong to account",
        ));
    }

    let request = find_latest_update_request(&state.db, registration.id).await?;
    let Some(request) = request else {
        return Err(ApiError::NotFound);
    };

    let created_by = user_summary(&state.db, request.created_by_id).await?;
    let updated_by = user_summary(
        &state.db,
        request.extra.get("updated_by_id").and_then(Value::as_i64),
    )
    .await?;

    Ok(Json(update_request_json(&request, created_by, updated_by)).into_response())
}

/// `PUT /accounts/{account_id}/lti_registrations/{id}/update_requests/{update_request_id}/apply`.
///
/// Canvas `Lti::RegistrationsController#apply_registration_update_request`.
pub async fn apply_update_request(
    State(state): State<AppState>,
    Path((account_id, id, update_request_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    let registration = find_active_registration(&state.db, id).await?;

    // Only the pending requests can be applied.
    let request = find_update_request(&state.db, update_request_id, None, true).await?;
    let Some(request) = request else {
        return Err(ApiError::NotFound);
    };

    if registration.account_id != ctx.account_id {
        return Ok(errors_string(
            StatusCode::BAD_REQUEST,
            "registration does not belong to account",
        ));
    }

    let payload: ApplyUpdateRequestBody = parse_body(&body)?.unwrap_or_default();
    let Some(accepted) = payload.accepted else {
        return Ok(errors_string(
            StatusCode::BAD_REQUEST,
            "accepted parameter is required",
        ));
    };

    // Only the most recent request may be applied.
    if !is_most_recent_update_request(&state.db, request.lti_registration_id, request.id).await? {
        return Ok(errors_string(
            StatusCode::BAD_REQUEST,
            "Cannot apply an outdated registration update request. A newer update request exists.",
        ));
    }

    if accepted {
        let target = find_active_registration(&state.db, request.lti_registration_id).await?;
        apply_registration_update_request(&state.db, &target, &request, payload.overlay).await?;
    } else {
        sqlx::query(
            "UPDATE lti_registration_update_requests SET rejected_at = now() WHERE id = $1",
        )
        .bind(request.id)
        .execute(&state.db)
        .await?;
    }

    let registration = find_active_registration(&state.db, registration.id).await?;
    let json = registration_json(
        &state.db,
        &registration,
        &ctx,
        RegIncludes {
            configuration: true,
            overlay: true,
            overlay_versions: true,
            account_binding: true,
            ..Default::default()
        },
    )
    .await?;

    Ok(Json(json).into_response())
}

/// Canvas `Lti::RegistrationUpdateRequest.find_by(...)`.
async fn find_update_request(
    pool: &PgPool,
    update_request_id: i64,
    lti_registration_id: Option<i64>,
    only_pending: bool,
) -> Result<Option<UpdateRequestRow>, ApiError> {
    let sql = "SELECT id, uuid, lti_registration_id, root_account_id, lti_ims_registration, \
                      created_by_id, accepted_at, rejected_at, created_at, updated_at, \
                      (to_jsonb(t) - 'id' - 'uuid' - 'lti_registration_id' - 'root_account_id' \
                       - 'lti_ims_registration' - 'created_by_id' - 'accepted_at' - 'rejected_at' \
                       - 'created_at' - 'updated_at') AS extra \
               FROM lti_registration_update_requests t \
               WHERE t.id = $1 AND ($2::bigint IS NULL OR t.lti_registration_id = $2) \
               AND ($3::boolean IS FALSE \
                    OR (t.accepted_at IS NULL AND t.rejected_at IS NULL))";

    let row: Option<UpdateRequestRow> = sqlx::query_as(sql)
        .bind(update_request_id)
        .bind(lti_registration_id)
        .bind(only_pending)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// The most recent update request of a registration, whatever its status.
async fn find_latest_update_request(
    pool: &PgPool,
    lti_registration_id: i64,
) -> Result<Option<UpdateRequestRow>, ApiError> {
    let sql = "SELECT id, uuid, lti_registration_id, root_account_id, lti_ims_registration, \
                      created_by_id, accepted_at, rejected_at, created_at, updated_at, \
                      (to_jsonb(t) - 'id' - 'uuid' - 'lti_registration_id' - 'root_account_id' \
                       - 'lti_ims_registration' - 'created_by_id' - 'accepted_at' - 'rejected_at' \
                       - 'created_at' - 'updated_at') AS extra \
               FROM lti_registration_update_requests t \
               WHERE t.lti_registration_id = $1 ORDER BY created_at DESC LIMIT 1";

    let row: Option<UpdateRequestRow> = sqlx::query_as(sql)
        .bind(lti_registration_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// Canvas `Lti::RegistrationUpdateRequest#most_recent?`.
async fn is_most_recent_update_request(
    pool: &PgPool,
    lti_registration_id: i64,
    update_request_id: i64,
) -> Result<bool, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_registration_update_requests \
         WHERE lti_registration_id = $1 ORDER BY created_at DESC LIMIT 1",
    )
    .bind(lti_registration_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(id,)| id == update_request_id).unwrap_or(false))
}

/// Canvas `Lti::ApplyRegistrationUpdateRequestService.call`.
async fn apply_registration_update_request(
    pool: &PgPool,
    registration: &RegistrationRow,
    request: &UpdateRequestRow,
    overlay_data: Option<Value>,
) -> Result<(), ApiError> {
    let new_config = &request.lti_ims_registration;

    // Only dynamic registrations can be updated this way.
    let ims = find_ims_registration(pool, registration.id).await?;
    let Some(ims) = ims else {
        // Canvas raises, which surfaces as a 500.
        return Err(ApiError::Db(
            "Only Registration update requests for Dynamic Registrations are currently supported"
                .to_string(),
        ));
    };

    let client_name = new_config.get("client_name").cloned();
    let jwks_uri = new_config.get("jwks_uri").cloned();
    let initiate_login_uri = new_config.get("initiate_login_uri").cloned();
    let redirect_uris = new_config.get("redirect_uris").cloned();
    let lti_tool_configuration = new_config.get("lti_tool_configuration").cloned();
    let scopes = new_config.get("scopes").cloned();
    let logo_uri = new_config.get("logo_uri").cloned();

    let redirect_uris_text = match &redirect_uris {
        Some(Value::Array(items)) => serde_json::to_string(items).ok(),
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    };
    let scopes_text = match &scopes {
        Some(Value::Array(items)) => serde_json::to_string(items).ok(),
        Some(Value::String(value)) => Some(value.clone()),
        _ => None,
    };

    sqlx::query(
        "UPDATE lti_ims_registrations SET \
             client_name = $2, jwks_uri = $3, initiate_login_uri = $4, redirect_uris = $5, \
             lti_tool_configuration = $6, scopes = $7, logo_uri = $8, updated_at = now() \
         WHERE id = $1",
    )
    .bind(ims.id)
    .bind(client_name.as_ref().and_then(Value::as_str))
    .bind(jwks_uri.as_ref().and_then(Value::as_str))
    .bind(initiate_login_uri.as_ref().and_then(Value::as_str))
    .bind(redirect_uris_text.as_deref())
    .bind(lti_tool_configuration.unwrap_or_else(|| json!({})))
    .bind(scopes_text.as_deref())
    .bind(logo_uri.as_ref().and_then(Value::as_str))
    .execute(pool)
    .await?;

    // The registration takes the new client name.
    if let Some(name) = client_name.as_ref().and_then(Value::as_str) {
        sqlx::query("UPDATE lti_registrations SET name = $2, updated_at = now() WHERE id = $1")
            .bind(registration.id)
            .bind(name)
            .execute(pool)
            .await?;
    }

    // The developer key follows the new configuration.
    if let Some(developer_key_id) = ims.developer_key_id {
        let scopes_value = scopes
            .as_ref()
            .map(|value| match value {
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<&str>>()
                    .join(","),
                Value::String(value) => value.clone(),
                _ => String::new(),
            })
            .unwrap_or_default();
        sqlx::query(
            "UPDATE developer_keys SET \
                 name = COALESCE($2, name), \
                 icon_url = COALESCE($3, icon_url), \
                 scopes = COALESCE($4::text[], scopes), \
                 updated_at = now() \
             WHERE id = $1",
        )
        .bind(developer_key_id)
        .bind(client_name.as_ref().and_then(Value::as_str))
        .bind(logo_uri.as_ref().and_then(Value::as_str))
        .bind(
            scopes_value
                .split(',')
                .map(str::to_string)
                .collect::<Vec<String>>(),
        )
        .execute(pool)
        .await?;
    }

    // The overlay is replaced by the applied overlay data.
    if let Some(overlay) = overlay_data {
        sqlx::query("UPDATE lti_registrations SET overlay = $2, updated_at = now() WHERE id = $1")
            .bind(registration.id)
            .bind(overlay)
            .execute(pool)
            .await?;
    }

    sqlx::query("UPDATE lti_registration_update_requests SET accepted_at = now() WHERE id = $1")
        .bind(request.id)
        .execute(pool)
        .await?;

    Ok(())
}

/// `PUT /accounts/{account_id}/lti_registrations/{id}`.
///
/// Canvas `Lti::RegistrationsController#update` backed by
/// `Lti::UpdateRegistrationService`.
pub async fn update_registration(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    let registration = find_active_registration(&state.db, id).await?;

    // require_manage_lti_registrations_in_registrations_account
    if registration.account_id != ctx.account_id {
        return Ok(errors_string(
            StatusCode::BAD_REQUEST,
            "registration does not belong to account",
        ));
    }

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    let payload: UpdateRegistrationBody = parse_body(&body)?.unwrap_or_default();

    if let Some(response) = validate_workflow_state(&ctx, payload.workflow_state.as_deref()) {
        return Ok(response);
    }

    // validate_registration_params
    if let Some(configuration) = payload.configuration.as_ref() {
        if !configuration.is_object() {
            return Ok(configuration_errors(vec![
                "configuration must be an object".to_string(),
            ]));
        }
    }
    if let Some(overlay) = payload.overlay.as_ref() {
        if !overlay.is_object() {
            return Ok(configuration_errors(vec![
                "overlay must be an object".to_string()
            ]));
        }
    }

    let configuration_params = payload.configuration.as_ref().map(|configuration| {
        if configuration.get("extensions").is_some() {
            from_lti_configuration(configuration)
        } else {
            configuration.clone()
        }
    });

    // Canvas validates the raw parameters: a configuration carrying the
    // `extensions` array is checked against the LTI configuration schema,
    // everything else against the internal configuration schema.
    if let Some(configuration) = payload.configuration.as_ref() {
        let errors = if configuration.get("extensions").is_some() {
            lti_configuration_errors(configuration)
        } else {
            internal_configuration_errors(configuration)
        };
        if !errors.is_empty() {
            return Ok(configuration_errors(errors));
        }
    }

    if let Some(overlay) = payload.overlay.as_ref() {
        let errors = overlay_errors(overlay);
        if !errors.is_empty() {
            return Ok(configuration_errors(errors));
        }
    }

    // restrict_dynamic_registration_updates
    if configuration_params.is_some() && registration.dynamic_registration {
        return Ok(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "tool_configuration_required",
            "Only manual configurations can be updated. Please create a new registration if you need to update the base tool configuration of a Dynamic Registration.",
        ));
    }

    if let Some(response) = require_in_account_or_site_admin(&ctx, &registration) {
        return Ok(response);
    }

    // `lock_deploying` is only permitted with the feature flag.
    let lock_deploying_permitted =
        feature_enabled(&state.db, ctx.root_account_id, "lock_lti_registrations").await?;

    let mut new_configuration = registration.configuration.clone();
    let mut new_overlay = registration.overlay.clone();

    if let Some(configuration) = configuration_params.as_ref() {
        // Only the allowed base properties are stored.
        let mut merged = new_configuration.as_object().cloned().unwrap_or_default();
        if let Some(map) = configuration.as_object() {
            for key in INTERNAL_CONFIG_KEYS {
                if let Some(value) = map.get(*key) {
                    merged.insert((*key).to_string(), value.clone());
                }
            }
        }
        new_configuration = Value::Object(merged);
    }

    if let Some(overlay) = payload.overlay.as_ref() {
        if registration.dynamic_registration {
            // A dynamic registration keeps its overlay.
            new_overlay = Some(overlay.clone());
        } else {
            // A manual registration merges the overlay into the configuration
            // and clears the overlay.
            let merged = apply_overlay(&new_configuration, overlay);
            let mut merged_map = merged.as_object().cloned().unwrap_or_default();
            merged_map.retain(|key, _| INTERNAL_CONFIG_KEYS.contains(&key.as_str()));
            new_configuration = Value::Object(merged_map);
            new_overlay = Some(json!({}));
        }
    }

    // The workflow state resolution.
    let (binding_state, registration_state) =
        match resolve_workflow_state(payload.workflow_state.as_deref()) {
            Some((binding, registration)) => (binding.to_string(), registration.to_string()),
            None => (
                registration.workflow_state.clone(),
                registration.workflow_state.clone(),
            ),
        };

    let name = payload
        .name
        .clone()
        .unwrap_or_else(|| registration.name.clone());
    let admin_nickname = payload
        .admin_nickname
        .clone()
        .or_else(|| registration.admin_nickname.clone());
    let vendor = payload
        .vendor
        .clone()
        .or_else(|| registration.vendor.clone());
    let description = payload
        .description
        .clone()
        .or_else(|| registration.description.clone());
    let lock_deploying = if lock_deploying_permitted {
        payload
            .lock_deploying
            .unwrap_or(registration.lock_deploying)
    } else {
        registration.lock_deploying
    };

    sqlx::query(
        "UPDATE lti_registrations SET \
             name = $2, admin_nickname = $3, vendor = $4, description = $5, \
             workflow_state = $6, configuration = $7, overlay = $8, lock_deploying = $9, \
             updated_by_id = $10, updated_at = now() \
         WHERE id = $1",
    )
    .bind(registration.id)
    .bind(&name)
    .bind(admin_nickname.as_deref())
    .bind(vendor.as_deref())
    .bind(description.as_deref())
    .bind(registration_state.as_str())
    .bind(&new_configuration)
    .bind(&new_overlay)
    .bind(lock_deploying)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    // The account binding follows the workflow state.
    if payload.workflow_state.is_some() {
        upsert_binding(
            &state.db,
            &registration,
            &ctx,
            binding_state.as_str(),
            auth.id,
        )
        .await?;
    }

    let registration = find_active_registration(&state.db, registration.id).await?;
    let json = registration_json(
        &state.db,
        &registration,
        &ctx,
        RegIncludes {
            configuration: true,
            overlay: true,
            overlay_versions: true,
            account_binding: true,
            ..Default::default()
        },
    )
    .await?;

    Ok(Json(json).into_response())
}

/// Canvas `Lti::AccountBindingService#bind_to_registration`.
async fn upsert_binding(
    pool: &PgPool,
    registration: &RegistrationRow,
    ctx: &Ctx,
    workflow_state: &str,
    user_id: i64,
) -> Result<BindingRow, ApiError> {
    let sql = "INSERT INTO lti_registration_account_bindings \
                   (registration_id, account_id, root_account_id, workflow_state, \
                    created_by_id, updated_by_id) \
               VALUES ($1, $2, $3, $4, $5, $5) \
               ON CONFLICT (registration_id, account_id) DO UPDATE \
                   SET workflow_state = EXCLUDED.workflow_state, \
                       updated_by_id = EXCLUDED.updated_by_id, \
                       updated_at = now() \
               RETURNING id, registration_id, account_id, root_account_id, workflow_state, \
                         created_by_id, updated_by_id, created_at, updated_at";

    let row: BindingRow = sqlx::query_as(sql)
        .bind(registration.id)
        .bind(ctx.account_id)
        .bind(ctx.root_account_id)
        .bind(workflow_state)
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(row)
}

/// `PUT /accounts/{account_id}/lti_registrations/{id}/reset`.
///
/// Canvas `Lti::RegistrationsController#reset`.
pub async fn reset_registration(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;

    // require_lti_registrations_next_feature_flag
    if !feature_enabled(&state.db, ctx.root_account_id, "lti_registrations_next").await? {
        return Ok(not_found_render_error());
    }

    require_manage(&state.db, &auth, account_id).await?;

    let registration = find_active_registration(&state.db, id).await?;

    // require_manage_lti_registrations_in_registrations_account
    if registration.account_id != ctx.account_id {
        return Ok(errors_string(
            StatusCode::BAD_REQUEST,
            "registration does not belong to account",
        ));
    }

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    if let Some(response) = require_in_account_or_site_admin(&ctx, &registration) {
        return Ok(response);
    }

    // The overlay of this context is emptied.
    if registration.overlay.is_some() {
        sqlx::query(
            "UPDATE lti_registrations SET overlay = '{}'::jsonb, updated_by_id = $2, \
                    updated_at = now() WHERE id = $1",
        )
        .bind(registration.id)
        .bind(auth.id)
        .execute(&state.db)
        .await?;
    }

    let registration = find_active_registration(&state.db, registration.id).await?;
    let json = registration_json(
        &state.db,
        &registration,
        &ctx,
        RegIncludes {
            overlaid_configuration: true,
            overlay: true,
            overlay_versions: true,
            ..Default::default()
        },
    )
    .await?;

    Ok(Json(json).into_response())
}

/// `POST /accounts/{account_id}/lti_registrations/{id}/bind`.
///
/// Canvas `Lti::RegistrationsController#bind` backed by
/// `Lti::InstallTemplateRegistrationService`.
pub async fn bind(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    let payload: BindBody = parse_body(&body)?.unwrap_or_default();

    if let Some(response) = validate_workflow_state(&ctx, payload.workflow_state.as_deref()) {
        return Ok(response);
    }

    let registration = find_active_registration(&state.db, id).await?;

    if let Some(response) = require_in_account_or_site_admin(&ctx, &registration) {
        return Ok(response);
    }

    // Site-admin accounts may not hold local bindings.
    if ctx.account_id == ctx.site_admin_id {
        return Ok(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_context",
            "site admin local bindings are not allowed",
        ));
    }

    let Some(workflow_state) = payload.workflow_state.as_deref() else {
        return Ok(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_workflow_state",
            "workflow_state must be one of 'on', 'off'",
        ));
    };
    if !matches!(workflow_state, "on" | "off") {
        return Ok(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_workflow_state",
            "workflow_state must be one of 'on', 'off'",
        ));
    }

    match install_template_registration(&state.db, &ctx, &registration, workflow_state, auth.id)
        .await
    {
        Ok((_local_copy, binding)) => {
            let created_by = user_summary(&state.db, binding.created_by_id).await?;
            let updated_by = user_summary(&state.db, binding.updated_by_id).await?;
            Ok(Json(binding_json(&binding, created_by, updated_by)).into_response())
        }
        Err(response) => Ok(response),
    }
}

/// `DELETE /accounts/{account_id}/lti_registrations/{id}/bind`.
///
/// Canvas `Lti::RegistrationsController#unbind`.
pub async fn unbind(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;

    // require_lti_deactivate_registrations_flag
    if !feature_enabled(
        &state.db,
        ctx.root_account_id,
        "lti_deactivate_registrations",
    )
    .await?
    {
        return Ok(not_found_render_error());
    }

    require_manage(&state.db, &auth, account_id).await?;

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    let registration = find_active_registration(&state.db, id).await?;

    if let Some(response) = require_in_account_or_site_admin(&ctx, &registration) {
        return Ok(response);
    }

    if ctx.account_id == ctx.site_admin_id {
        return Ok(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_context",
            "site admin local bindings are not allowed",
        ));
    }

    if registration.account_id == ctx.account_id {
        return Ok(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_registration",
            "use destroy to delete local registrations",
        ));
    }

    let binding = find_binding(&state.db, registration.id, ctx.account_id).await?;
    let Some(binding) = binding else {
        return Ok(render_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "No binding found for this registration in this account.",
        ));
    };

    // The binding and the local copy are removed.
    sqlx::query("DELETE FROM lti_registration_account_bindings WHERE id = $1")
        .bind(binding.id)
        .execute(&state.db)
        .await?;

    if let Some(local_copy) = find_local_copy(&state.db, registration.id, ctx.account_id).await? {
        sqlx::query("UPDATE lti_registrations SET workflow_state = 'deleted' WHERE id = $1")
            .bind(local_copy.id)
            .execute(&state.db)
            .await?;
    }

    let created_by = user_summary(&state.db, binding.created_by_id).await?;
    let updated_by = user_summary(&state.db, binding.updated_by_id).await?;

    Ok(Json(binding_json(&binding, created_by, updated_by)).into_response())
}

/// `POST /accounts/{account_id}/lti_registrations/{id}/install_from_template`.
///
/// Canvas `Lti::RegistrationsController#install_from_template`.
pub async fn install_from_template(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = account_context(&state.db, account_id).await?;
    require_manage(&state.db, &auth, account_id).await?;

    if let Some(response) = restrict_sub_account_to_read_only(&ctx) {
        return Ok(response);
    }

    let registration = find_active_registration(&state.db, id).await?;

    match install_template_registration(&state.db, &ctx, &registration, "on", auth.id).await {
        Ok((local_copy, binding)) => {
            let mut json = registration_json(
                &state.db,
                &local_copy,
                &ctx,
                RegIncludes {
                    configuration: true,
                    overlay: true,
                    ..Default::default()
                },
            )
            .await?;
            // Canvas renders the template's account binding.
            let created_by = user_summary(&state.db, binding.created_by_id).await?;
            let updated_by = user_summary(&state.db, binding.updated_by_id).await?;
            json["account_binding"] = binding_json(&binding, created_by, updated_by);
            Ok(Json(json).into_response())
        }
        Err(response) => Ok(response),
    }
}

/// Canvas `Lti::InstallTemplateRegistrationService.call`.
///
/// Returns the local copy of the template and the account binding. The
/// `ArgumentError`s Canvas rescues become a `422 invalid_template` response.
async fn install_template_registration(
    pool: &PgPool,
    ctx: &Ctx,
    template: &RegistrationRow,
    binding_state: &str,
    user_id: i64,
) -> Result<(RegistrationRow, BindingRow), Response> {
    // The service's own validations.
    if template.workflow_state != "active" {
        return Err(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_template",
            "template registration is off",
        ));
    }
    if !ctx.is_root_account {
        return Err(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_template",
            "root account must be provided",
        ));
    }
    if template.dynamic_registration {
        return Err(render_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_template",
            "Dynamic Registrations cannot be used as templates",
        ));
    }

    // The local copy of the template.
    let local_copy = match find_local_copy(pool, template.id, ctx.account_id).await {
        Ok(Some(local_copy)) => local_copy,
        Ok(None) => {
            // The local copy starts from the overlaid template configuration.
            let configuration = apply_overlay(&template.configuration, &overlay_data(template));

            let sql = format!(
                "INSERT INTO lti_registrations \
                     (account_id, root_account_id, name, admin_nickname, vendor, description, \
                      internal_service, lti_version, dynamic_registration, lock_deploying, \
                      template_registration_id, unified_tool_id, workflow_state, configuration, \
                      overlay, created_by_id, updated_by_id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, '1.3', FALSE, $8, $9, $10, 'active', $11, \
                         '{{}}'::jsonb, $12, $12) \
                 RETURNING {REGISTRATION_COLUMNS}"
            );
            match sqlx::query_as::<_, RegistrationRow>(&sql)
                .bind(ctx.account_id)
                .bind(ctx.root_account_id)
                .bind(&template.name)
                .bind(template.admin_nickname.as_deref())
                .bind(template.vendor.as_deref())
                .bind(template.description.as_deref())
                .bind(template.internal_service)
                .bind(template.lock_deploying)
                .bind(template.id)
                .bind(template.unified_tool_id.as_deref())
                .bind(&configuration)
                .bind(user_id)
                .fetch_one(pool)
                .await
            {
                Ok(row) => {
                    // The tool is deployed to the account.
                    if let Err(e) = deploy_registration(pool, &row, ctx, user_id).await {
                        return Err(render_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "deployment_failed",
                            &e.to_string(),
                        ));
                    }
                    row
                }
                Err(e) => {
                    return Err(render_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "install_failed",
                        &e.to_string(),
                    ))
                }
            }
        }
        Err(e) => {
            return Err(render_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "install_failed",
                &e.to_string(),
            ))
        }
    };

    // The registration state follows the binding state.
    let registration_state = if binding_state == "on" {
        "active"
    } else {
        "inactive"
    };
    if local_copy.workflow_state != registration_state {
        if let Err(e) = sqlx::query(
            "UPDATE lti_registrations SET workflow_state = $2, updated_by_id = $3, \
                    updated_at = now() WHERE id = $1",
        )
        .bind(local_copy.id)
        .bind(registration_state)
        .bind(user_id)
        .execute(pool)
        .await
        {
            return Err(render_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "install_failed",
                &e.to_string(),
            ));
        }
    }

    // The account binding of the template.
    let binding = match upsert_binding(pool, template, ctx, binding_state, user_id).await {
        Ok(binding) => binding,
        Err(e) => {
            return Err(render_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "install_failed",
                &e.to_string(),
            ))
        }
    };

    Ok((local_copy, binding))
}

/// Canvas `Lti::Registration#new_external_tool(account, available: false)`.
///
/// The workspace stores the deployment in `lti_deployments`.
async fn deploy_registration(
    pool: &PgPool,
    registration: &RegistrationRow,
    ctx: &Ctx,
    user_id: i64,
) -> Result<(), ApiError> {
    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_deployments \
         WHERE registration_id = $1 AND context_type = 'Account' AND context_id = $2",
    )
    .bind(registration.id)
    .bind(ctx.account_id)
    .fetch_optional(pool)
    .await?;
    if existing.is_some() {
        return Ok(());
    }

    let context_name: Option<(String,)> = sqlx::query_as("SELECT name FROM accounts WHERE id = $1")
        .bind(ctx.account_id)
        .fetch_optional(pool)
        .await?;
    let deployment_id = format!("{}:{}", ctx.account_id, uuid::Uuid::new_v4());

    sqlx::query(
        "INSERT INTO lti_deployments \
             (registration_id, deployment_id, context_type, context_id, context_name, available, \
              workflow_state, created_by_id) \
         VALUES ($1, $2, 'Account', $3, $4, FALSE, 'active', $5)",
    )
    .bind(registration.id)
    .bind(&deployment_id)
    .bind(ctx.account_id)
    .bind(context_name.map(|(name,)| name))
    .bind(user_id)
    .execute(pool)
    .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the LTI registrations router (mounted under `/api/v1` by star-http).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{account_id}/lti_registrations/configuration/validate",
            post(validate_configuration),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/check_domain_duplicates",
            get(check_domain_duplicates),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/deployments/{deployment_id}/context_search",
            get(context_search),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{id}/overlay_history",
            get(overlay_history),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{id}/history",
            get(history),
        )
        .route(
            "/accounts/{account_id}/lti_registration_by_client_id/{client_id}",
            get(show_by_client_id),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/by_utid/{utid}",
            get(show_by_utid),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/install_status/{client_id}",
            get(install_status),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{id}/update_requests/{update_request_id}",
            get(show_update_request),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{id}/latest_update_request",
            get(latest_update_request),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{id}/update_requests/{update_request_id}/apply",
            put(apply_update_request),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{id}",
            put(update_registration),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{id}/reset",
            put(reset_registration),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{id}/bind",
            post(bind).delete(unbind),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{id}/install_from_template",
            post(install_from_template),
        )
}
