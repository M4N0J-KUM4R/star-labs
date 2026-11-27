//! star-lti — Canvas LTI 1.3 registrations and deployments.
//!
//! Canvas parity (`app/controllers/lti/registrations_controller.rb`,
//! `app/controllers/lti/deployments_controller.rb`, `lib/api/v1/lti/*`):
//!
//! - `GET    /api/v1/accounts/{accountId}/lti_registrations` — list
//! - `POST   /api/v1/accounts/{accountId}/lti_registrations` — create
//! - `GET    /api/v1/accounts/{accountId}/lti_registrations/{id}` — show
//! - `DELETE /api/v1/accounts/{accountId}/lti_registrations/{id}` — soft delete
//! - `GET    /api/v1/accounts/{accountId}/lti_registrations/{id}/deployments`
//! - `POST   /api/v1/accounts/{accountId}/lti_registrations/{id}/deployments`
//!
//! Issue #69 deep slices (`lti/registrations_controller.rb`,
//! `lti/deployments_controller.rb`):
//! - `PUT    /accounts/{accountId}/lti_registrations/{id}` — update
//! - `POST   /accounts/{accountId}/lti_registrations/{id}/bind` — bind
//! - `DELETE /accounts/{accountId}/lti_registrations/{id}/bind` — unbind
//! - `GET    /accounts/{accountId}/lti_registrations/{id}/history`
//! - `GET    /accounts/{accountId}/lti_registrations/{id}/overlay_history`
//! - `PUT    /accounts/{accountId}/lti_registrations/{id}/reset`
//! - `POST   /accounts/{accountId}/lti_registrations/{id}/install_from_template`
//! - `GET    /accounts/{accountId}/lti_registrations/{id}/latest_update_request`
//! - `GET    /accounts/{accountId}/lti_registrations/{id}/update_requests/{updateRequestId}`
//! - `PUT    /accounts/{accountId}/lti_registrations/{id}/update_requests/{updateRequestId}/apply`
//! - `GET    /accounts/{accountId}/lti_registrations/{registrationId}/deployments/{deploymentId}`
//! - `DELETE /accounts/{accountId}/lti_registrations/{registrationId}/deployments/{deploymentId}`
//! - `GET    /accounts/{accountId}/lti_registrations/{registrationId}/deployments/{deploymentId}/context_search`
//! - `GET    /accounts/{accountId}/lti_registration_by_client_id/{clientId}`
//! - `GET    /accounts/{accountId}/lti_registrations/by_utid/{utid}`
//! - `GET    /accounts/{accountId}/lti_registrations/check_domain_duplicates`
//! - `POST   /accounts/{accountId}/lti_registrations/configuration/validate`
//! - `GET    /accounts/{accountId}/lti_registrations/install_status/{clientId}`
//!
//! The Canvas-nested aliases
//! `/accounts/{accountId}/lti_registrations/{id}` and
//! `/accounts/{accountId}/lti_registrations/{registrationId}/deployments`
//! are served as well, so clients built against Canvas' documented paths work
//! unchanged.
//!
//! Response shapes mirror Canvas: registration JSON carries `id, account_id,
//! root_account_id, internal_service, vendor, name, admin_nickname,
//! workflow_state, created_at, updated_at, description, lock_deploying,
//! template_registration_id, inherited, lti_version, icon_url`, plus
//! `account_binding` and (for show/create) `configuration`. Deployment JSON
//! carries `id, context_id, context_type, registration_id, deployment_id,
//! context_name, workflow_state, root_account_deployment`.
//!
//! Auth: Canvas gates these endpoints behind `manage_lti_registrations`; the
//! workspace equivalent is the global `admin` role or an `account_users` row
//! for the account (or one of its ancestors), matching the star-accounts
//! permission helpers. Non-admins get `403`, anonymous callers `401`.

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

mod ags_auth;
mod ags_results;
mod ags_scores;
mod authentication;

mod context_controls;
mod deployments;
mod lti_api;
mod lti_apps;
mod originality_reports;
mod resource_links;
mod subscriptions;
mod tool_configurations;

mod data_services;
mod deep_linking;
mod dynamic_registration;
mod external_tools;
pub mod launch_cache;
mod line_items;
mod lti_authorization;
mod lti_gaps;
mod nrps;

/// Issue #95 — the opaque per-user LTI login_hint (Canvas
/// `Lti::V1p1::Asset.opaque_identifier_for`), exposed so launch-flow callers
/// and tests can construct hints the platform will accept.
pub use authentication::opaque_login_hint;
mod support_helpers;
mod tool_proxy;
mod tool_setting;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum account-hierarchy walk, mirroring star-accounts' cycle guard.
pub(crate) const MAX_ACCOUNT_TREE_DEPTH: i64 = 10;

/// Canvas lists 15 registrations per page by default.
const DEFAULT_PER_PAGE: i64 = 15;
pub(crate) const MAX_PER_PAGE: i64 = 100;

/// Canvas `Schemas::InternalLtiConfiguration.allowed_base_properties` — the
/// internal configuration keys that may be written
/// (lib/schemas/internal_lti_configuration.rb).
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

/// Canvas `Schemas::Lti::Overlay::ROOT_KEYS` — the overlay keys that
/// overwrite the root of the internal configuration.
const OVERLAY_ROOT_KEYS: &[&str] = &[
    "title",
    "description",
    "custom_fields",
    "target_link_uri",
    "oidc_initiation_url",
    "domain",
    "privacy_level",
];

/// Canvas `Schemas::Lti::Overlay::LAUNCH_SETTINGS_KEYS` — the overlay keys
/// that overwrite `launch_settings`.
const OVERLAY_LAUNCH_SETTINGS_KEYS: &[&str] = &["custom_fields", "icon_url", "message_type"];

/// Canvas `Lti::ResourcePlacement::PLACEMENTS` — the subset that matters for
/// the launch-settings filter in `Schemas::InternalLtiConfiguration
/// .from_lti_configuration` (lib/schemas/internal_lti_configuration.rb:51).
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
];

/// Canvas registration workflow states (`active | inactive | deleted`).
const STATE_ACTIVE: &str = "active";
pub(crate) const STATE_DELETED: &str = "deleted";

// ---------------------------------------------------------------------------
// Domain rows
// ---------------------------------------------------------------------------

/// A row of `lti_registrations`, in Canvas registration JSON shape (the
/// `configuration` / `overlay` JSONB blobs are serialized separately).
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct RegistrationRow {
    pub(crate) id: i64,
    pub(crate) account_id: i64,
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

/// A row of `lti_registration_account_bindings` (Canvas binding vocabulary:
/// `on | off | allow`).
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

/// A row of `lti_deployments`, in Canvas deployment JSON shape.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct DeploymentRow {
    pub(crate) id: i64,
    pub(crate) registration_id: i64,
    pub(crate) deployment_id: String,
    pub(crate) context_type: String,
    pub(crate) context_id: i64,
    pub(crate) context_name: Option<String>,
    pub(crate) workflow_state: String,
}

/// Columns selected for a [`RegistrationRow`], shared by the lookups.
pub(crate) const REGISTRATION_COLUMNS: &str =
    "id, account_id, root_account_id, name, admin_nickname, \
     vendor, description, icon_url, lti_version, dynamic_registration, internal_service, \
     lock_deploying, template_registration_id, workflow_state, configuration, overlay, \
     created_by_id, updated_by_id, created_at, updated_at";

/// Columns selected for a [`DeploymentRow`].
pub(crate) const DEPLOYMENT_COLUMNS: &str =
    "id, registration_id, deployment_id, context_type, context_id, context_name, workflow_state";

// ---------------------------------------------------------------------------
// Request / query types
// ---------------------------------------------------------------------------

/// Query params for `GET /accounts/{accountId}/lti_registrations`.
///
/// Canvas accepts `page`, `per_page`, `query`, `sort`, `dir` and
/// `include[]`. Only the subset this implementation honors is modelled;
/// `include` is accepted both as `include=configuration` and as Canvas'
/// bracket form `include[]=configuration`.
#[derive(Debug, Deserialize)]
pub struct ListRegistrationsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub query: Option<String>,
    pub include: Option<String>,
    #[serde(rename = "include[]")]
    pub include_brackets: Option<String>,
}

/// Query params shared by show/list when only `include[]` matters.
#[derive(Debug, Deserialize)]
pub struct IncludeQuery {
    pub include: Option<String>,
    #[serde(rename = "include[]")]
    pub include_brackets: Option<String>,
}

/// Which optional registration fields to embed in the response.
#[derive(Debug, Clone, Copy, Default)]
struct Includes {
    configuration: bool,
    overlay: bool,
}

impl ListRegistrationsQuery {
    /// Canvas defaults to page 1, 15 per page, capped at 100.
    fn resolve(&self) -> (i64, i64, i64) {
        let page = self.page.unwrap_or(1).max(1);
        let per_page = self
            .per_page
            .unwrap_or(DEFAULT_PER_PAGE)
            .clamp(1, MAX_PER_PAGE);
        let offset = page.saturating_sub(1).saturating_mul(per_page);
        (page, per_page, offset)
    }

    fn includes(&self) -> Includes {
        includes_from(self.include.as_deref().or(self.include_brackets.as_deref()))
    }
}

impl IncludeQuery {
    fn includes(&self) -> Includes {
        includes_from(self.include.as_deref().or(self.include_brackets.as_deref()))
    }
}

/// Parse a Canvas `include[]` value (comma separated or single value).
fn includes_from(raw: Option<&str>) -> Includes {
    let mut includes = Includes::default();
    for value in raw.unwrap_or_default().split(',') {
        match value.trim() {
            "configuration" => includes.configuration = true,
            "overlay" => includes.overlay = true,
            _ => {}
        }
    }
    includes
}

/// Body for `POST /accounts/{accountId}/lti_registrations`.
#[derive(Debug, Deserialize)]
pub struct CreateRegistrationRequest {
    pub name: Option<String>,
    pub admin_nickname: Option<String>,
    pub vendor: Option<String>,
    pub description: Option<String>,
    pub lti_version: Option<String>,
    pub unified_tool_id: Option<String>,
    pub lock_deploying: Option<bool>,
    /// Canvas binding/registration vocabulary: on | off | allow | active |
    /// inactive.
    pub workflow_state: Option<String>,
    /// Canvas `Lti::ToolConfiguration` (or legacy XML converted upstream).
    pub configuration: Option<Value>,
    /// Per-account configuration override.
    pub overlay: Option<Value>,
}

/// Body for `POST /accounts/{accountId}/lti_registrations/{id}/deployments`.
#[derive(Debug, Deserialize, Default)]
pub struct CreateDeploymentRequest {
    pub for_subaccount_id: Option<i64>,
    pub for_course_id: Option<i64>,
    pub available: Option<bool>,
}

/// Body for `PUT /accounts/{accountId}/lti_registrations/{id}` — Canvas
/// `update` permits `admin_nickname vendor name description workflow_state`
/// (registrations_controller.rb:1637-1641) plus the configuration and
/// overlay params and a change-log comment.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateRegistrationRequest {
    pub admin_nickname: Option<String>,
    pub vendor: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub workflow_state: Option<String>,
    pub lock_deploying: Option<bool>,
    pub configuration: Option<Value>,
    pub overlay: Option<Value>,
    pub comment: Option<String>,
}

/// Body for `POST .../lti_registrations/{id}/bind` — Canvas requires
/// `workflow_state` to be exactly `on` or `off` (:1747-1780).
#[derive(Debug, Deserialize)]
pub struct BindRequest {
    pub workflow_state: Option<String>,
}

/// Body for `PUT .../lti_registrations/{id}/update_requests/{id}/apply`
/// (registrations_controller.rb:2096-2148).
#[derive(Debug, Deserialize, Default)]
pub struct ApplyUpdateRequest {
    pub accepted: Option<bool>,
    pub overlay: Option<Value>,
    pub comment: Option<String>,
}

/// Query params for `GET .../lti_registrations/{id}/history` (Canvas
/// paginates with `page` / `per_page`, default page size 10).
#[derive(Debug, Deserialize, Default)]
pub struct HistoryQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// Query params for `GET .../lti_registrations/{id}/overlay_history` —
/// Canvas `limit` defaults to 10, max 100 (:1968-1983).
#[derive(Debug, Deserialize, Default)]
pub struct OverlayHistoryQuery {
    pub limit: Option<i64>,
}

/// Query params for the deployment `context_search` endpoint
/// (registrations_controller.rb:1864-1933).
#[derive(Debug, Deserialize, Default)]
pub struct ContextSearchQuery {
    pub only_children_of: Option<i64>,
    pub search_term: Option<String>,
}

/// Query params for `GET .../lti_registrations/check_domain_duplicates`
/// — Canvas requires `domain` (:1312-1338).
#[derive(Debug, Deserialize, Default)]
pub struct DomainQuery {
    pub domain: Option<String>,
}

/// Body for `POST .../lti_registrations/configuration/validate` — exactly one
/// of `lti_configuration` or `url` (registrations_controller.rb:1244-1260).
#[derive(Debug, Deserialize, Default)]
pub struct ValidateConfigurationRequest {
    pub lti_configuration: Option<Value>,
    pub url: Option<String>,
}

// ---------------------------------------------------------------------------
// Permission helpers (mirror star-accounts)
// ---------------------------------------------------------------------------

pub(crate) fn is_admin(auth: &AuthUser) -> bool {
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

/// Global admin OR account admin for the account or any ancestor, matching
/// `can_access_account` in star-accounts.
pub(crate) async fn can_manage_account(
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

/// 404 when the account does not exist (Canvas `require_account_context`).
pub(crate) async fn require_account(pool: &PgPool, account_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `Account#feature_enabled?` against the `feature_flags` table —
/// the state is `enabled` (or `allowed`, Canvas' default-on spelling).
pub(crate) async fn feature_enabled(
    pool: &PgPool,
    root_account_id: i64,
    feature: &str,
) -> Result<bool, ApiError> {
    let state: Option<(String,)> = sqlx::query_as(
        "SELECT state FROM feature_flags \
         WHERE context_type = 'Account' AND context_id = $1 AND feature = $2",
    )
    .bind(root_account_id)
    .bind(feature)
    .fetch_optional(pool)
    .await?;
    Ok(matches!(state, Some((ref state,)) if state == "enabled" || state == "allowed"))
}

/// Canvas `render_error(:not_found, "The specified resource does not exist.",
/// status: :not_found)` (registrations_controller.rb:2238-2240, 2275-2282).
pub(crate) fn render_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "errors": [{
            "code": "not_found",
            "message": "The specified resource does not exist."
        }] })),
    )
        .into_response()
}

/// Canvas `require_root_account_instrumented_or_sub_account_and_feature_flag`
/// (registrations_controller.rb:2246-2263): a sub-account whose root does
/// not carry `canvas_apps_sub_account_access` is a 404. `Some(response)` is
/// the rendered 404.
pub(crate) async fn require_root_or_sub_account_feature(
    pool: &PgPool,
    account_id: i64,
) -> Result<Option<Response>, ApiError> {
    if is_root_account(pool, account_id).await? {
        return Ok(None);
    }
    let root_id = root_account_id(pool, account_id).await?;
    if feature_enabled(pool, root_id, "canvas_apps_sub_account_access").await? {
        return Ok(None);
    }
    Ok(Some(render_not_found()))
}

/// Canvas `restrict_sub_account_to_read_only`
/// (registrations_controller.rb:2317-2321): sub-accounts only ever see
/// `403 { "errors": "sub-accounts can only view registrations" }` on the
/// write routes.
pub(crate) async fn restrict_sub_account_to_read_only(
    pool: &PgPool,
    account_id: i64,
) -> Result<Option<Response>, ApiError> {
    if is_root_account(pool, account_id).await? {
        return Ok(None);
    }
    Ok(Some(
        (
            StatusCode::FORBIDDEN,
            Json(json!({ "errors": "sub-accounts can only view registrations" })),
        )
            .into_response(),
    ))
}

/// Walk to the root of the account hierarchy (cycles are bounded).
pub(crate) async fn root_account_id(pool: &PgPool, account_id: i64) -> Result<i64, ApiError> {
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

/// True when the account has no parent.
pub(crate) async fn is_root_account(pool: &PgPool, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(pool)
            .await?;
    Ok(matches!(row, Some((None,))))
}

/// Require manage rights on the account that owns a registration.
pub(crate) async fn require_registration_permission(
    pool: &PgPool,
    auth: &AuthUser,
    registration: &RegistrationRow,
) -> Result<(), ApiError> {
    if can_manage_account(pool, auth, registration.account_id).await? {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

/// Fetch a registration that has not been soft-deleted (Canvas `.active`).
pub(crate) async fn find_active_registration(
    pool: &PgPool,
    id: i64,
) -> Result<RegistrationRow, ApiError> {
    let sql = format!(
        "SELECT {REGISTRATION_COLUMNS} FROM lti_registrations \
         WHERE id = $1 AND workflow_state <> '{STATE_DELETED}'"
    );
    let row: Option<RegistrationRow> = sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
    row.ok_or(ApiError::NotFound)
}

/// Fetch a registration scoped to the Canvas account path segment. Canvas
/// renders `400 {errors: "registration does not belong to account"}` when the
/// registration lives in a different account.
pub(crate) async fn find_registration_in_account(
    pool: &PgPool,
    id: i64,
    account_id: i64,
) -> Result<RegistrationRow, ApiError> {
    let row = find_active_registration(pool, id).await?;
    // Canvas `require_in_account_or_site_admin`
    // (registrations_controller.rb:2299-2303): a registration of the Site
    // Admin account is visible from every account. The workspace's Site
    // Admin is the first parent-less account.
    if row.account_id != account_id {
        let site_admin: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
        )
        .fetch_optional(pool)
        .await?;
        if site_admin != Some(row.account_id) {
            return Err(ApiError::BadRequest(
                "registration does not belong to account".to_string(),
            ));
        }
    }
    Ok(row)
}

// ---------------------------------------------------------------------------
// JSON serialization
// ---------------------------------------------------------------------------

/// Canvas `user_json` subset used for `created_by` / `updated_by`.
pub(crate) async fn user_summary(
    pool: &PgPool,
    user_id: Option<i64>,
) -> Result<Option<Value>, ApiError> {
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
async fn account_binding_json(
    pool: &PgPool,
    registration_id: i64,
    account_id: i64,
) -> Result<Option<Value>, ApiError> {
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

    let Some(binding) = row else {
        return Ok(None);
    };

    let mut json = json!({
        "id": binding.id,
        "account_id": binding.account_id,
        "root_account_id": binding.root_account_id,
        "registration_id": binding.registration_id,
        "workflow_state": binding.workflow_state,
        "created_at": binding.created_at,
        "updated_at": binding.updated_at,
    });
    if let Some(created_by) = user_summary(pool, binding.created_by_id).await? {
        json["created_by"] = created_by;
    }
    if let Some(updated_by) = user_summary(pool, binding.updated_by_id).await? {
        json["updated_by"] = updated_by;
    }
    Ok(Some(json))
}

/// Canvas `lti_registration_json` shape.
async fn registration_json(
    pool: &PgPool,
    registration: &RegistrationRow,
    includes: Includes,
) -> Result<Value, ApiError> {
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
        "inherited": false,
        "lti_version": registration.lti_version,
        "icon_url": registration.icon_url,
        "developer_key_id": Value::Null,
        "ims_registration_id": Value::Null,
        "dynamic_registration_url": Value::Null,
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

    // Canvas always emits `developer_key_id` / `ims_registration_id`
    // (lib/api/v1/lti/registration.rb:56-58: `registration.developer_key&.global_id`,
    // `registration.ims_registration&.id`); the workspace links an IMS
    // registration to its parent registration through
    // `lti_ims_registrations.lti_registration_id` (migration 0115).
    let linked: Option<(Option<i64>, i64)> = sqlx::query_as(
        "SELECT developer_key_id, id FROM lti_ims_registrations \
         WHERE lti_registration_id = $1 LIMIT 1",
    )
    .bind(registration.id)
    .fetch_optional(pool)
    .await?;
    if let Some((developer_key_id, ims_registration_id)) = linked {
        json["developer_key_id"] = json!(developer_key_id);
        json["ims_registration_id"] = json!(ims_registration_id);
    }

    if includes.configuration {
        json["configuration"] = registration.configuration.clone();
    }
    if includes.overlay {
        if let Some(overlay) = &registration.overlay {
            json["overlay"] = overlay.clone();
        }
    }

    if let Some(binding) =
        account_binding_json(pool, registration.id, registration.account_id).await?
    {
        json["account_binding"] = binding;
    }

    Ok(json)
}

/// Canvas `lti_deployment_json` shape.
pub(crate) fn deployment_json(deployment: &DeploymentRow, root_account_deployment: bool) -> Value {
    // Canvas hides "disabled"/"deleted" as-is and normalizes everything else
    // to "active".
    let workflow_state = match deployment.workflow_state.as_str() {
        "deleted" | "disabled" => deployment.workflow_state.clone(),
        _ => STATE_ACTIVE.to_string(),
    };

    json!({
        "id": deployment.id,
        "context_id": deployment.context_id,
        "context_type": deployment.context_type,
        "registration_id": deployment.registration_id,
        "deployment_id": deployment.deployment_id,
        "context_name": deployment.context_name,
        "workflow_state": workflow_state,
        "root_account_deployment": root_account_deployment,
    })
}

/// Validate a registration body the way Canvas' `render_configuration_errors`
/// does: `422 { "errors": [...] }`.
fn configuration_errors(errors: Vec<String>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "errors": errors })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Workflow-state mapping
// ---------------------------------------------------------------------------

/// Translate Canvas' five create/update values into the registration and
/// account-binding states. `on`/`off`/`allow` only move the binding;
/// `active`/`inactive` only move the registration.
fn resolve_workflow_state(value: Option<&str>) -> Result<(&'static str, &'static str), String> {
    match value.unwrap_or("on") {
        "on" => Ok((STATE_ACTIVE, "on")),
        "off" => Ok((STATE_ACTIVE, "off")),
        "allow" => Ok((STATE_ACTIVE, "allow")),
        "active" => Ok((STATE_ACTIVE, "on")),
        "inactive" => Ok(("inactive", "off")),
        other => Err(format!("invalid workflow_state: {other}")),
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the LTI router (mounted under `/api/v1` by star-http).
pub fn router() -> Router<AppState> {
    Router::new()
        // The /api/v1-served gap routes (feature flags, public JWK,
        // asset_processors/tii_migrations, support_helpers/*).
        .merge(lti_gaps::lti_gaps_routes())
        .route(
            "/accounts/{account_id}/lti_registrations",
            get(list_registrations).post(create_registration),
        )
        // Canvas declares the whole LTI-registration surface under an account
        // scope only (`config/routes.rb:2168-2186`, plus the deployments scope
        // at `config/routes.rb:2190-2195`), so these nested paths are the
        // complete surface — there is no flat `/lti_registrations/{id}`.
        .route(
            "/accounts/{account_id}/lti_registrations/{id}",
            get(show_registration_scoped)
                .put(update_registration_scoped)
                .delete(delete_registration_scoped),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/deployments",
            get(list_deployments_scoped).post(create_deployment_scoped),
        )
        // Issue #69 — registration lifecycle (registrations_controller.rb).
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/bind",
            post(bind_registration).delete(unbind_registration),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/history",
            get(registration_history),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/overlay_history",
            get(overlay_history),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/reset",
            put(reset_registration),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/install_from_template",
            post(install_from_template),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/latest_update_request",
            get(latest_update_request),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/update_requests/{update_request_id}",
            get(show_update_request),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/update_requests/{update_request_id}/apply",
            put(apply_update_request),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/deployments/{deployment_id}",
            get(show_deployment_scoped).delete(delete_deployment_scoped),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/deployments/{deployment_id}/context_search",
            get(deployment_context_search),
        )
        // Static siblings of {id} at the same tree position — matchit prefers
        // the literal segments (same pattern as the `uuid` route in
        // dynamic_registration.rs).
        .route(
            "/accounts/{account_id}/lti_registration_by_client_id/{client_id}",
            get(show_registration_by_client_id),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/by_utid/{utid}",
            get(show_registration_by_utid),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/check_domain_duplicates",
            get(check_domain_duplicates),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/configuration/validate",
            post(validate_configuration),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/install_status/{client_id}",
            get(install_status),
        )
        .merge(context_controls::router())
        .merge(lti_apps::router())
        .merge(external_tools::router())
        .merge(data_services::router())
        .merge(tool_proxy::router())
        .merge(support_helpers::router())
        // Issue #95 — LTI resource links API (resource_links_controller.rb),
        // served under /api/v1 like the other course-scoped routes.
        .merge(resource_links::router())
        // The deployment controls of the batch-3 lane (registrations paths
        // under /api/v1).
        .merge(deployments::router())
}

/// Router for the Canvas `/api/lti` family (LTI Advantage services).
///
/// Canvas serves these routes outside `/api/v1` (`ApiRouteSet.draw(self,
/// "/api/lti")`), so the paths carry the `/api/lti` prefix and the router is
/// mounted at the application root — see `star-http/src/main.rs`.
/// Test/dev helper: mint an LTI Advantage service token (see `ags_auth`).
pub use ags_auth::mint_service_token as mint_service_token_for_tests;

pub fn lti_advantage_router() -> Router<AppState> {
    Router::new()
        .merge(dynamic_registration::advantage_router())
        .merge(line_items::router())
        .merge(tool_proxy::advantage_router())
        .merge(tool_setting::router())
        .merge(external_tools::advantage_router())
        // Issue #94 — LTI AGS scores/results and NRPS memberships.
        .merge(ags_scores::router())
        .merge(ags_results::router())
        .merge(nrps::router())
        // Issue #95 — LTI 1.3 launch flows: OIDC login initiation and
        // authentication request (authentication_controller.rb), the LTI 2
        // authorization server (authorization_controller.rb) and the deep
        // linking content selection (deep_linking_controller.rb, served at
        // the root like Canvas' default route set).
        .merge(authentication::advantage_router())
        .merge(lti_authorization::advantage_router())
        .merge(deep_linking::advantage_router())
        // The webhook subscriptions of a tool proxy.
        .merge(subscriptions::router())
        // The originality reports, the LTI 1.1 outcome/analytics services
        // and the tool configurations of the batch-3 lane — Canvas serves
        // them through the "/api/lti" route set (config/routes.rb:3109),
        // and their routers carry the full /api/lti paths, so they belong
        // HERE (the root-mounted advantage router), not under /api/v1.
        .merge(originality_reports::router())
        .merge(lti_api::router())
        .merge(tool_configurations::router())
        // The /api/lti-served gap routes (Token, UsersApi,
        // MembershipService, ToolConsumerProfile, NoticeHandlers,
        // Progress, asset processors) — same route set, same root mount.
        .merge(lti_gaps::lti_gaps_advantage_routes())
}

// ---------------------------------------------------------------------------
// Registration handlers
// ---------------------------------------------------------------------------

/// `GET /accounts/{account_id}/lti_registrations` — list registrations owned
/// by the account. Canvas returns `{ total, data }` (not page metadata).
async fn list_registrations(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<ListRegistrationsQuery>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let (_page, per_page, offset) = params.resolve();
    let search = params
        .query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(|query| format!("%{query}%"));

    let (total,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM lti_registrations \
         WHERE account_id = $1 AND workflow_state <> $2 \
         AND ($3::text IS NULL OR name ILIKE $3 OR COALESCE(admin_nickname, '') ILIKE $3)",
    )
    .bind(account_id)
    .bind(STATE_DELETED)
    .bind(search.as_deref())
    .fetch_one(&state.db)
    .await?;

    let sql = format!(
        "SELECT {REGISTRATION_COLUMNS} FROM lti_registrations \
         WHERE account_id = $1 AND workflow_state <> $2 \
         AND ($3::text IS NULL OR name ILIKE $3 OR COALESCE(admin_nickname, '') ILIKE $3) \
         ORDER BY created_at DESC, id DESC LIMIT $4 OFFSET $5"
    );
    let rows: Vec<RegistrationRow> = sqlx::query_as(&sql)
        .bind(account_id)
        .bind(STATE_DELETED)
        .bind(search.as_deref())
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    let includes = params.includes();
    let mut data = Vec::with_capacity(rows.len());
    for row in &rows {
        data.push(registration_json(&state.db, row, includes).await?);
    }

    Ok(Json(json!({ "total": total, "data": data })))
}

/// `POST /accounts/{account_id}/lti_registrations` — create a registration,
/// its account binding, and return Canvas' `201` registration JSON.
async fn create_registration(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateRegistrationRequest>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let Some(configuration) = payload.configuration else {
        return Ok(configuration_errors(vec![
            "configuration is required".to_string()
        ]));
    };
    if !configuration.is_object() {
        return Ok(configuration_errors(vec![
            "configuration must be an object".to_string(),
        ]));
    }
    if let Some(overlay) = &payload.overlay {
        if !overlay.is_object() {
            return Ok(configuration_errors(vec![
                "overlay must be an object".to_string()
            ]));
        }
    }

    // Canvas derives the name from `configuration.title` when the request
    // does not carry one.
    let name = payload
        .name
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            configuration
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            payload
                .admin_nickname
                .clone()
                .filter(|name| !name.trim().is_empty())
        });
    let Some(name) = name else {
        return Ok(configuration_errors(vec![
            "name or configuration.title is required".to_string(),
        ]));
    };

    let (registration_state, binding_state) =
        match resolve_workflow_state(payload.workflow_state.as_deref()) {
            Ok(states) => states,
            Err(message) => return Ok(configuration_errors(vec![message])),
        };

    let lti_version = payload
        .lti_version
        .or_else(|| {
            configuration
                .get("lti_version")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "1.3".to_string());

    let icon_url = configuration
        .pointer("/launch_settings/icon_url")
        .and_then(Value::as_str)
        .or_else(|| configuration.get("icon_url").and_then(Value::as_str))
        .map(str::to_string);

    let root_id = root_account_id(&state.db, account_id).await?;

    let sql = format!(
        "INSERT INTO lti_registrations \
         (account_id, root_account_id, name, admin_nickname, vendor, description, icon_url, \
          lti_version, dynamic_registration, lock_deploying, unified_tool_id, workflow_state, \
          configuration, overlay, created_by_id, updated_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, FALSE, $9, $10, $11, $12, $13, $14, $14) \
         RETURNING {REGISTRATION_COLUMNS}"
    );
    let registration: RegistrationRow = sqlx::query_as(&sql)
        .bind(account_id)
        .bind(root_id)
        .bind(name)
        .bind(payload.admin_nickname)
        .bind(payload.vendor)
        .bind(payload.description)
        .bind(icon_url)
        .bind(lti_version)
        .bind(payload.lock_deploying.unwrap_or(false))
        .bind(payload.unified_tool_id)
        .bind(registration_state)
        .bind(configuration)
        .bind(payload.overlay)
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;

    sqlx::query(
        "INSERT INTO lti_registration_account_bindings \
         (registration_id, account_id, root_account_id, workflow_state, created_by_id, updated_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (registration_id, account_id) DO UPDATE \
         SET workflow_state = EXCLUDED.workflow_state, updated_at = now()",
    )
    .bind(registration.id)
    .bind(account_id)
    .bind(root_id)
    .bind(binding_state)
    .bind(auth.id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    let includes = Includes {
        configuration: true,
        overlay: true,
    };
    let body = registration_json(&state.db, &registration, includes).await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// Canvas path: `GET /accounts/{accountId}/lti_registrations/{id}`. Canvas
/// always embeds the account binding and configuration.
async fn show_registration_scoped(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<IncludeQuery>,
) -> Result<Json<Value>, ApiError> {
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    let includes = Includes {
        configuration: true,
        overlay: params.includes().overlay,
    };
    Ok(Json(
        registration_json(&state.db, &registration, includes).await?,
    ))
}

/// Canvas path: `DELETE /accounts/{accountId}/lti_registrations/{id}` — Canvas
/// soft-deletes the registration and returns its JSON with
/// `workflow_state: "deleted"`.
async fn delete_registration_scoped(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;
    soft_delete_registration(&state.db, id, auth.id).await
}

/// Mark the registration and its deployments deleted; bindings are kept for
/// history (Canvas destroys the registration, not the binding row).
async fn soft_delete_registration(
    pool: &PgPool,
    id: i64,
    user_id: i64,
) -> Result<Json<Value>, ApiError> {
    let sql = format!(
        "UPDATE lti_registrations \
         SET workflow_state = $2, updated_by_id = $3, updated_at = now() \
         WHERE id = $1 AND workflow_state <> $2 \
         RETURNING {REGISTRATION_COLUMNS}"
    );
    let registration: Option<RegistrationRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(STATE_DELETED)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    let Some(registration) = registration else {
        return Err(ApiError::NotFound);
    };

    sqlx::query(
        "UPDATE lti_deployments SET workflow_state = $2, updated_at = now() \
         WHERE registration_id = $1 AND workflow_state <> $2",
    )
    .bind(id)
    .bind(STATE_DELETED)
    .execute(pool)
    .await?;

    let includes = Includes {
        configuration: true,
        overlay: true,
    };
    Ok(Json(
        registration_json(pool, &registration, includes).await?,
    ))
}

// ---------------------------------------------------------------------------
// Deployment handlers
// ---------------------------------------------------------------------------

/// Canvas path: `GET /accounts/{accountId}/lti_registrations/{registrationId}/deployments`
/// — Canvas returns a bare array.
async fn list_deployments_scoped(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    let registration = find_registration_in_account(&state.db, registration_id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;
    Ok(Json(
        deployments_for_registration(&state.db, registration.id).await?,
    ))
}

/// Load the active deployments of a registration in Canvas JSON shape.
async fn deployments_for_registration(
    pool: &PgPool,
    registration_id: i64,
) -> Result<Vec<Value>, ApiError> {
    let sql = format!(
        "SELECT {DEPLOYMENT_COLUMNS} FROM lti_deployments \
         WHERE registration_id = $1 AND workflow_state <> $2 ORDER BY id ASC"
    );
    let rows: Vec<DeploymentRow> = sqlx::query_as(&sql)
        .bind(registration_id)
        .bind(STATE_DELETED)
        .fetch_all(pool)
        .await?;

    let mut deployments = Vec::with_capacity(rows.len());
    for row in &rows {
        let root = row.context_type == "Account" && is_root_account(pool, row.context_id).await?;
        deployments.push(deployment_json(row, root));
    }
    Ok(deployments)
}

/// Canvas path:
/// `POST /accounts/{accountId}/lti_registrations/{registrationId}/deployments`
/// — deploy into an account (the registration's own by default) or a course.
/// Canvas answers `200` with the deployment JSON.
async fn create_deployment_scoped(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateDeploymentRequest>,
) -> Result<Response, ApiError> {
    let registration = find_registration_in_account(&state.db, registration_id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;
    create_deployment_for_registration(&state.db, &registration, &auth, payload).await
}

/// Shared deployment creation.
async fn create_deployment_for_registration(
    pool: &PgPool,
    registration: &RegistrationRow,
    auth: &AuthUser,
    payload: CreateDeploymentRequest,
) -> Result<Response, ApiError> {
    if payload.for_subaccount_id.is_some() && payload.for_course_id.is_some() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "error": "Cannot specify both for_subaccount_id and for_course_id" })),
        )
            .into_response());
    }

    let (context_type, context_id, context_name) = if let Some(course_id) = payload.for_course_id {
        let row: Option<(String,)> = sqlx::query_as("SELECT name FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
        let Some((name,)) = row else {
            return Err(ApiError::NotFound);
        };
        ("Course", course_id, name)
    } else if let Some(subaccount_id) = payload.for_subaccount_id {
        let row: Option<(String,)> = sqlx::query_as("SELECT name FROM accounts WHERE id = $1")
            .bind(subaccount_id)
            .fetch_optional(pool)
            .await?;
        let Some((name,)) = row else {
            return Err(ApiError::NotFound);
        };
        ("Account", subaccount_id, name)
    } else {
        let row: (String,) = sqlx::query_as("SELECT name FROM accounts WHERE id = $1")
            .bind(registration.account_id)
            .fetch_one(pool)
            .await?;
        ("Account", registration.account_id, row.0)
    };

    let available = payload.available.unwrap_or(true);
    let workflow_state = if available { STATE_ACTIVE } else { "disabled" };
    let root_id = registration
        .root_account_id
        .unwrap_or(registration.account_id);
    let deployment_id = generate_deployment_id(root_id);

    let sql = format!(
        "INSERT INTO lti_deployments \
         (registration_id, deployment_id, context_type, context_id, context_name, available, \
          workflow_state, created_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING {DEPLOYMENT_COLUMNS}"
    );
    let deployment: DeploymentRow = sqlx::query_as(&sql)
        .bind(registration.id)
        .bind(deployment_id)
        .bind(context_type)
        .bind(context_id)
        .bind(context_name)
        .bind(available)
        .bind(workflow_state)
        .bind(auth.id)
        .fetch_one(pool)
        .await?;

    let root_deployment = deployment.context_type == "Account"
        && is_root_account(pool, deployment.context_id).await?;
    Ok((
        StatusCode::OK,
        Json(deployment_json(&deployment, root_deployment)),
    )
        .into_response())
}

/// Canvas deployment ids look like `<root_account_id>:<opaque token>`. The
/// token only has to be unique (enforced by the database index), so a
/// timestamp + pid mix is sufficient here.
fn generate_deployment_id(root_account_id: i64) -> String {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let pid = std::process::id();
    format!("{root_account_id}:{nanos:x}{pid:x}")
}

// ---------------------------------------------------------------------------
// Issue #69 — registration lifecycle handlers (registrations_controller.rb)
// ---------------------------------------------------------------------------

/// Canvas `render_error` (registrations_controller.rb:2238-2240):
/// `422 { "errors": [{ "code", "message" }] }`.
pub(crate) fn render_error(code: &str, message: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "errors": [{ "code": code, "message": message }] })),
    )
        .into_response()
}

/// Canvas `render_configuration_errors` (:2149-2151): `422 { "errors": [...] }`.
fn configuration_error_list(errors: Vec<String>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "errors": errors })),
    )
        .into_response()
}

/// The account chain from `account_id` up to (excluding) the root, farthest
/// first — the display path Canvas builds for context_search
/// (registrations_controller.rb:1922-1931, `tap(&:pop).reverse`).
async fn display_path_names(
    pool: &PgPool,
    account_id: i64,
    include_self: bool,
) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "WITH RECURSIVE up AS ( \
             SELECT id, name, parent_account_id FROM accounts WHERE id = $1 \
             UNION ALL \
             SELECT a.id, a.name, a.parent_account_id FROM accounts a \
             JOIN up ON a.id = up.parent_account_id \
         ) SELECT id, name FROM up",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await?;
    // rows are [self, parent, ..., root]; drop the root (last), drop self
    // unless include_self, then reverse so the highest level comes first.
    let mut chain: Vec<(i64, String)> = rows;
    chain.pop(); // root
    if !include_self && !chain.is_empty() {
        chain.remove(0);
    }
    chain.reverse();
    Ok(chain.into_iter().map(|(_, name)| name).collect())
}

/// Record an overlay version row (Canvas `Lti::OverlayVersion`).
async fn record_overlay_version(
    pool: &PgPool,
    registration_id: i64,
    account_id: i64,
    root_account_id: i64,
    overlay: &Value,
    caused_by_reset: bool,
    user_id: i64,
) -> Result<(), ApiError> {
    // The Canvas schema (migration 0224) keeps one Lti::Overlay row per
    // registration+account with the live data, and versions reference it
    // through the lti_overlay_id foreign key. The workspace keeps the live
    // overlay on lti_registrations.overlay; the lti_overlays row is
    // maintained in sync so the FK holds. diff stores the full new overlay
    // snapshot (Canvas stores a JSON patch; the column is NOT NULL from the
    // Canvas schema dump).
    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_overlays \
         WHERE registration_id = $1 AND account_id = $2 AND workflow_state <> 'deleted' \
         LIMIT 1",
    )
    .bind(registration_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    let lti_overlay_id: i64 = match existing {
        Some((overlay_row_id,)) => {
            sqlx::query("UPDATE lti_overlays SET data = $3, updated_at = now() WHERE id = $2")
                .bind(registration_id)
                .bind(overlay_row_id)
                .bind(overlay)
                .execute(pool)
                .await?;
            overlay_row_id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO lti_overlays \
             (registration_id, account_id, root_account_id, workflow_state, data, \
              created_at, updated_at) \
             VALUES ($1, $2, $3, 'active', $4, now(), now()) RETURNING id",
            )
            .bind(registration_id)
            .bind(account_id)
            .bind(root_account_id)
            .bind(overlay)
            .fetch_one(pool)
            .await?
        }
    };

    sqlx::query(
        "INSERT INTO lti_overlay_versions \
         (registration_id, account_id, root_account_id, lti_overlay_id, overlay, \
          diff, caused_by_reset, created_by_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $5, $6, $7, now(), now())",
    )
    .bind(registration_id)
    .bind(account_id)
    .bind(root_account_id)
    .bind(lti_overlay_id)
    .bind(overlay)
    .bind(caused_by_reset)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a registration history entry (Canvas
/// `Lti::RegistrationHistoryEntry`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_history_entry(
    pool: &PgPool,
    registration_id: i64,
    root_account_id: i64,
    update_type: &str,
    diff: Value,
    comment: Option<&str>,
    old_configuration: Option<&Value>,
    new_configuration: Option<&Value>,
    user_id: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO lti_registration_history_entries \
         (lti_registration_id, root_account_id, update_type, diff, comment, \
          old_configuration, new_configuration, created_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(registration_id)
    .bind(root_account_id)
    .bind(update_type)
    .bind(diff)
    .bind(comment)
    .bind(old_configuration)
    .bind(new_configuration)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Canvas `lti_overlay_version_json` shape (lib/api/v1/lti/
/// overlay_version.rb) — `lti_overlay_id` is null because the workspace
/// keeps the overlay on the registration row instead of a separate
/// `Lti::Overlay`.
/// A row of `lti_overlay_versions`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct OverlayVersionRow {
    id: i64,
    account_id: i64,
    root_account_id: Option<i64>,
    lti_overlay_id: i64,
    overlay: Value,
    diff: Option<Value>,
    caused_by_reset: bool,
    created_by_id: Option<i64>,
    created_at: DateTime<Utc>,
}

async fn overlay_version_json(pool: &PgPool, version_id: i64) -> Result<Option<Value>, ApiError> {
    let row: Option<OverlayVersionRow> = sqlx::query_as(
        "SELECT id, account_id, root_account_id, lti_overlay_id, overlay, diff, \
                caused_by_reset, created_by_id, created_at \
         FROM lti_overlay_versions WHERE id = $1",
    )
    .bind(version_id)
    .fetch_optional(pool)
    .await?;
    let Some(version) = row else {
        return Ok(None);
    };
    let mut json = json!({
        "id": version.id,
        "account_id": version.account_id,
        "root_account_id": version.root_account_id,
        "lti_overlay_id": version.lti_overlay_id,
        "overlay": version.overlay,
        "diff": version.diff,
        "caused_by_reset": version.caused_by_reset,
        "created_at": version.created_at,
        "updated_at": version.created_at,
    });
    if let Some(user) = user_summary(pool, version.created_by_id).await? {
        json["created_by"] = user;
    }
    Ok(Some(json))
}

/// Overlay versions for a registration in an account, newest first.
async fn overlay_versions_json(
    pool: &PgPool,
    registration_id: i64,
    account_id: i64,
    limit: i64,
) -> Result<Vec<Value>, ApiError> {
    let ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_overlay_versions \
         WHERE registration_id = $1 AND account_id = $2 \
         ORDER BY id DESC LIMIT $3",
    )
    .bind(registration_id)
    .bind(account_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let mut versions = Vec::with_capacity(ids.len());
    for (id,) in ids {
        if let Some(version) = overlay_version_json(pool, id).await? {
            versions.push(version);
        }
    }
    Ok(versions)
}

/// Canvas `Lti::Overlay.apply_to` — the overlay root keys overwrite the
/// base configuration, the launch-settings keys merge into
/// `launch_settings`, `disabled_scopes` filters the scopes and the
/// per-placement overlay merges into the matching placement
/// (`app/models/lti/overlay.rb`).
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

    // `disabled_scopes` takes precedence over `scopes`.
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

    // `disabled_placements` takes precedence over `placements`.
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

/// Canvas `overlaid_configuration` — the configuration with the overlay
/// applied. Canvas resolves this through `Lti::Overlay` placement merging;
/// the workspace applies a shallow top-level merge of the overlay keys.
fn overlaid_configuration(configuration: &Value, overlay: Option<&Value>) -> Value {
    let mut merged = configuration.clone();
    if let Some(Value::Object(keys)) = overlay {
        for (key, value) in keys {
            merged[key] = value.clone();
        }
    }
    merged
}

/// `registration_json` plus the overlay/version includes Canvas's update,
/// reset and apply endpoints carry (`overlaid_configuration`, `overlay`,
/// `overlay_versions`).
async fn registration_json_with_versions(
    pool: &PgPool,
    registration: &RegistrationRow,
    account_id: i64,
) -> Result<Value, ApiError> {
    let includes = Includes {
        configuration: true,
        overlay: true,
    };
    let mut json = registration_json(pool, registration, includes).await?;
    let overlay = registration.overlay.clone().unwrap_or_else(|| json!({}));
    json["overlaid_configuration"] =
        overlaid_configuration(&registration.configuration, Some(&overlay));
    json["overlay_versions"] =
        json!(overlay_versions_json(pool, registration.id, account_id, MAX_PER_PAGE).await?);
    Ok(json)
}

/// `PUT /accounts/{account_id}/lti_registrations/{id}` — update the permitted
/// registration fields, configuration and overlay
/// (registrations_controller.rb:1637-1682).
async fn update_registration_scoped(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateRegistrationRequest>,
) -> Result<Response, ApiError> {
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    // validate_workflow_state (:2202-2213): allow is site-admin only.
    if let Some(workflow_state) = payload.workflow_state.as_deref() {
        if !matches!(
            workflow_state,
            "on" | "off" | "allow" | "active" | "inactive"
        ) {
            return Ok(render_error(
                "invalid_workflow_state",
                "workflow_state must be one of 'on', 'off', 'allow', 'active', or 'inactive'",
            ));
        }
        if workflow_state == "allow" {
            return Ok(render_error(
                "invalid_workflow_state",
                "only site admin registrations can have a state of 'allow'",
            ));
        }
    }

    // restrict_dynamic_registration_updates (:2231-2236): the base
    // configuration of a dynamic registration is immutable.
    if payload.configuration.is_some() && registration.dynamic_registration {
        return Ok(render_error(
            "tool_configuration_required",
            "Only manual configurations can be updated. Please create a new registration if you need to update the base tool configuration of a Dynamic Registration.",
        ));
    }

    let (registration_state, binding_state) = match payload.workflow_state.as_deref() {
        None | Some("on") | Some("active") => (None, None),
        Some("off") => (None, Some("off")),
        Some("inactive") => (Some("inactive"), Some("off")),
        Some(_) => (None, None),
    };

    if let Some(configuration) = &payload.configuration {
        if !configuration.is_object() {
            return Ok(configuration_error_list(vec![
                "configuration must be an object".to_string(),
            ]));
        }
    }
    if let Some(overlay) = &payload.overlay {
        if !overlay.is_object() {
            return Ok(configuration_error_list(vec![
                "overlay must be an object".to_string()
            ]));
        }
    }

    let old_configuration = registration.configuration.clone();

    // Canvas `UpdateRegistrationService#apply_overlay_to_manual_configuration!`
    // (app/services/lti/update_registration_service.rb:100-121): for a
    // MANUAL registration an overlay param is merged into the base
    // configuration (the overlay wins, the configuration params apply
    // underneath it) and the stored overlay is cleared; only dynamic
    // registrations keep an overlay.
    let previous_overlay = registration.overlay.clone();
    let (new_configuration, new_overlay, manual_overlay_merged) =
        if let Some(overlay) = payload.overlay.clone() {
            if registration.dynamic_registration {
                (payload.configuration.clone(), Some(overlay), false)
            } else {
                let mut merged = registration
                    .configuration
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                if let Some(map) = payload.configuration.as_ref().and_then(Value::as_object) {
                    for (key, value) in map {
                        merged.insert(key.clone(), value.clone());
                    }
                }
                let mut overlaid = apply_overlay(&Value::Object(merged), &overlay);
                if let Some(map) = overlaid.as_object_mut() {
                    map.retain(|key, _| INTERNAL_CONFIG_KEYS.contains(&key.as_str()));
                }
                (Some(overlaid), Some(json!({})), true)
            }
        } else {
            (payload.configuration.clone(), None, false)
        };

    let sql = format!(
        "UPDATE lti_registrations SET \
         name = COALESCE($2, name), \
         admin_nickname = COALESCE($3, admin_nickname), \
         vendor = COALESCE($4, vendor), \
         description = COALESCE($5, description), \
         lock_deploying = COALESCE($6, lock_deploying), \
         workflow_state = COALESCE($7, workflow_state), \
         configuration = COALESCE($8, configuration), \
         overlay = COALESCE($9, overlay), \
         updated_by_id = $10, updated_at = now() \
         WHERE id = $1 \
         RETURNING {REGISTRATION_COLUMNS}"
    );
    let updated: Option<RegistrationRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(payload.name)
        .bind(payload.admin_nickname)
        .bind(payload.vendor)
        .bind(payload.description)
        .bind(payload.lock_deploying)
        .bind(registration_state)
        .bind(&new_configuration)
        .bind(&new_overlay)
        .bind(auth.id)
        .fetch_optional(&state.db)
        .await?;
    let Some(updated) = updated else {
        return Err(ApiError::NotFound);
    };

    // A workflow_state of on/off moves the account binding (Canvas
    // UpdateRegistrationService resolves both, like create).
    if let Some(binding_state) = binding_state {
        let root_id = root_account_id(&state.db, account_id).await?;
        sqlx::query(
            "INSERT INTO lti_registration_account_bindings \
             (registration_id, account_id, root_account_id, workflow_state, created_by_id, updated_by_id) \
             VALUES ($1, $2, $3, $4, $5, $5) \
             ON CONFLICT (registration_id, account_id) DO UPDATE \
             SET workflow_state = EXCLUDED.workflow_state, updated_at = now()",
        )
        .bind(id)
        .bind(account_id)
        .bind(root_id)
        .bind(binding_state)
        .bind(auth.id)
        .execute(&state.db)
        .await?;
    }

    if let Some(overlay) = &new_overlay {
        if manual_overlay_merged {
            // Canvas clears an existing overlay row when the overlay is
            // merged into a manual configuration (update_registration_
            // service.rb:118-121) — a version is only recorded when an
            // overlay existed before.
            if previous_overlay.is_some() {
                let root_id = root_account_id(&state.db, account_id).await?;
                record_overlay_version(
                    &state.db,
                    id,
                    account_id,
                    root_id,
                    &json!({}),
                    false,
                    auth.id,
                )
                .await?;
            }
        } else {
            let root_id = root_account_id(&state.db, account_id).await?;
            record_overlay_version(&state.db, id, account_id, root_id, overlay, false, auth.id)
                .await?;
        }
    }
    let root_id = root_account_id(&state.db, account_id).await?;
    record_history_entry(
        &state.db,
        id,
        root_id,
        "update",
        json!({ "fields": ["name", "admin_nickname", "vendor", "description", "workflow_state", "lock_deploying", "configuration", "overlay"] }),
        payload.comment.as_deref(),
        Some(&old_configuration),
        Some(&updated.configuration),
        auth.id,
    )
    .await?;

    Ok(
        Json(registration_json_with_versions(&state.db, &updated, account_id).await?)
            .into_response(),
    )
}

/// `POST /accounts/{account_id}/lti_registrations/{id}/bind` — enable or
/// disable a registration for the account by upserting the account binding
/// (registrations_controller.rb:1747-1784).
async fn bind_registration(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<BindRequest>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let registration = find_active_registration(&state.db, registration_id).await?;

    // Canvas `bind` refuses the Site Admin account itself
    // (registrations_controller.rb:1747-1750: "site admin local bindings
    // are not allowed"). The workspace's Site Admin is the first
    // parent-less account.
    let site_admin: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(&state.db)
    .await?;
    if account_id == site_admin {
        return Ok(render_error(
            "invalid_context",
            "site admin local bindings are not allowed",
        ));
    }

    let Some(workflow_state) = payload.workflow_state.as_deref() else {
        return Ok(render_error(
            "invalid_workflow_state",
            "workflow_state must be one of 'on', 'off'",
        ));
    };
    if !matches!(workflow_state, "on" | "off") {
        return Ok(render_error(
            "invalid_workflow_state",
            "workflow_state must be one of 'on', 'off'",
        ));
    }

    let root_id = root_account_id(&state.db, account_id).await?;

    // Canvas `Lti::InstallTemplateRegistrationService.call`
    // (registrations_controller.rb:1764-1772): binding a template
    // registration installs a local copy of it for the account first —
    // the same install the install_from_template route performs.
    install_template_for_account(&state.db, &registration, account_id, root_id, auth.id).await?;

    sqlx::query(
        "INSERT INTO lti_registration_account_bindings \
         (registration_id, account_id, root_account_id, workflow_state, created_by_id, updated_by_id) \
         VALUES ($1, $2, $3, $4, $5, $5) \
         ON CONFLICT (registration_id, account_id) DO UPDATE \
         SET workflow_state = EXCLUDED.workflow_state, updated_by_id = EXCLUDED.updated_by_id, \
             updated_at = now()",
    )
    .bind(registration_id)
    .bind(account_id)
    .bind(root_id)
    .bind(workflow_state)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    let binding = account_binding_json(&state.db, registration_id, account_id).await?;
    Ok(Json(binding.unwrap_or(Value::Null)).into_response())
}

/// Canvas `Lti::InstallTemplateRegistrationService` — a local copy of the
/// template for the account, unless one exists already. The copy is only
/// created for a registration that lives in the Site Admin account (an
/// inherited template); a registration of another root account is only
/// bound, never copied.
async fn install_template_for_account(
    db: &PgPool,
    template: &RegistrationRow,
    account_id: i64,
    root_id: i64,
    user_id: i64,
) -> Result<(), ApiError> {
    let site_admin: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    if site_admin != Some(template.account_id) {
        return Ok(());
    }
    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_registrations \
         WHERE template_registration_id = $1 AND account_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(template.id)
    .bind(account_id)
    .fetch_optional(db)
    .await?;
    if existing.is_some() {
        return Ok(());
    }
    let sql = "INSERT INTO lti_registrations \
         (account_id, root_account_id, name, admin_nickname, vendor, description, icon_url, \
          lti_version, dynamic_registration, lock_deploying, template_registration_id, \
          workflow_state, configuration, created_by_id, updated_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'active', $12, $13, $13)";
    sqlx::query(sql)
        .bind(account_id)
        .bind(root_id)
        .bind(&template.name)
        .bind(template.admin_nickname.clone())
        .bind(template.vendor.clone())
        .bind(template.description.clone())
        .bind(template.icon_url.clone())
        .bind(&template.lti_version)
        .bind(template.dynamic_registration)
        .bind(template.lock_deploying)
        .bind(template.id)
        .bind(&template.configuration)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(())
}

/// `DELETE /accounts/{account_id}/lti_registrations/{id}/bind` — remove an
/// inherited registration's binding for the account
/// (registrations_controller.rb:1786-1820).
async fn unbind_registration(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas `require_lti_deactivate_registrations_flag`
    // (registrations_controller.rb:2284-2292) — 404 without the flag.
    let unbind_root = root_account_id(&state.db, account_id).await?;
    if !feature_enabled(&state.db, unbind_root, "lti_deactivate_registrations").await? {
        return Ok(render_not_found());
    }
    let registration = find_active_registration(&state.db, registration_id).await?;

    // Canvas `unbind` refuses locally-owned registrations
    // (registrations_controller.rb:1790-1791).
    if registration.account_id == account_id {
        return Ok(render_error(
            "invalid_registration",
            "use destroy to delete local registrations",
        ));
    }

    let binding = account_binding_json(&state.db, registration_id, account_id).await?;
    let Some(binding) = binding else {
        // Canvas renders this one with status :not_found (:1801).
        return Ok(
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "errors": [{ "code": "not_found", "message": "No binding found for this registration in this account." }] })),
            )
                .into_response(),
        );
    };

    // Canvas destroys the binding and the local copy (if any) — real
    // destroys, not soft deletes (registrations_controller.rb:1796-1802).
    sqlx::query(
        "DELETE FROM lti_registration_account_bindings \
         WHERE registration_id = $1 AND account_id = $2",
    )
    .bind(registration_id)
    .bind(account_id)
    .execute(&state.db)
    .await?;
    sqlx::query(
        "DELETE FROM lti_registrations \
         WHERE template_registration_id = $1 AND account_id = $2",
    )
    .bind(registration_id)
    .bind(account_id)
    .execute(&state.db)
    .await?;

    Ok(Json(binding).into_response())
}

/// A row of `lti_registration_history_entries`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct HistoryEntryRow {
    id: i64,
    root_account_id: Option<i64>,
    update_type: String,
    diff: Value,
    comment: Option<String>,
    old_configuration: Option<Value>,
    new_configuration: Option<Value>,
    created_by_id: Option<i64>,
    created_at: DateTime<Utc>,
}

/// `GET /accounts/{account_id}/lti_registrations/{id}/history` — the
/// registration changelog, newest first
/// (registrations_controller.rb:2000-2026).
async fn registration_history(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<HistoryQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas `require_lti_registrations_history_feature_flag`
    // (registrations_controller.rb:1115, 2284-2292) — 404 without the flag.
    let history_root = root_account_id(&state.db, account_id).await?;
    if !feature_enabled(&state.db, history_root, "lti_registrations_history").await? {
        return Err(ApiError::NotFound);
    }
    let _ = find_active_registration(&state.db, registration_id).await?;

    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(10).clamp(1, MAX_PER_PAGE);
    let offset = page.saturating_sub(1).saturating_mul(per_page);
    let root_id = history_root;

    let rows: Vec<HistoryEntryRow> = sqlx::query_as(
        "SELECT id, root_account_id, update_type, diff, comment, old_configuration, \
                new_configuration, created_by_id, created_at \
         FROM lti_registration_history_entries \
         WHERE lti_registration_id = $1 AND root_account_id = $2 \
         ORDER BY created_at DESC, id DESC LIMIT $3 OFFSET $4",
    )
    .bind(registration_id)
    .bind(root_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let mut entries = Vec::with_capacity(rows.len());
    for entry in rows {
        let mut json = json!({
            "id": entry.id,
            "root_account_id": entry.root_account_id,
            "lti_registration_id": registration_id,
            "created_at": entry.created_at,
            "updated_at": entry.created_at,
            "diff": entry.diff,
            "update_type": entry.update_type,
            "comment": entry.comment,
            "old_configuration": entry.old_configuration,
            "new_configuration": entry.new_configuration,
            "old_controls_by_deployment": json!({}),
            "new_controls_by_deployment": json!({}),
        });
        if let Some(user) = user_summary(&state.db, entry.created_by_id).await? {
            json["created_by"] = user;
        }
        entries.push(json);
    }
    Ok(Json(entries))
}

/// `GET /accounts/{account_id}/lti_registrations/{id}/overlay_history` —
/// overlay edit history, newest first (:1968-1998).
async fn overlay_history(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<OverlayHistoryQuery>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas `require_lti_registrations_next_feature_flag`
    // (registrations_controller.rb:1114, 2275-2282) — 404 without the flag.
    let overlay_root = root_account_id(&state.db, account_id).await?;
    if !feature_enabled(&state.db, overlay_root, "lti_registrations_next").await? {
        return Err(ApiError::NotFound);
    }
    let registration = find_active_registration(&state.db, registration_id).await?;

    // Canvas renders [] when the registration has no overlay at all — the
    // limit is only validated for a registration that carries an overlay
    // (registrations_controller.rb:1968-1985).
    if registration.overlay.is_none() {
        return Ok(Json(Vec::<Value>::new()).into_response());
    }

    // Canvas `validate_limit_param` (:2372-2388) — the limit must be a
    // positive integer of at most `Api::MAX_PER_PAGE`.
    let limit = match params.limit {
        None => 10,
        Some(limit) if limit <= 0 => {
            return Ok(render_error(
                "invalid_limit",
                "limit must be a positive integer",
            ));
        }
        Some(limit) if limit > MAX_PER_PAGE => {
            return Ok(render_error("invalid_limit", "limit cannot exceed 100"));
        }
        Some(limit) => limit,
    };
    Ok(
        Json(overlay_versions_json(&state.db, registration_id, account_id, limit).await?)
            .into_response(),
    )
}

/// `PUT /accounts/{account_id}/lti_registrations/{id}/reset` — reset the
/// overlay to defaults (registrations_controller.rb:1684-1706).
async fn reset_registration(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    // Canvas `require_lti_registrations_next_feature_flag`
    // (registrations_controller.rb:1114, 2275-2282) — 404 without the flag.
    let reset_root = root_account_id(&state.db, account_id).await?;
    if !feature_enabled(&state.db, reset_root, "lti_registrations_next").await? {
        return Err(ApiError::NotFound);
    }
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    let sql = format!(
        "UPDATE lti_registrations SET overlay = '{{}}'::jsonb, \
         updated_by_id = $2, updated_at = now() \
         WHERE id = $1 \
         RETURNING {REGISTRATION_COLUMNS}"
    );
    let updated: Option<RegistrationRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(auth.id)
        .fetch_optional(&state.db)
        .await?;
    let Some(updated) = updated else {
        return Err(ApiError::NotFound);
    };

    let root_id = root_account_id(&state.db, account_id).await?;
    record_overlay_version(
        &state.db,
        id,
        account_id,
        root_id,
        &json!({}),
        true,
        auth.id,
    )
    .await?;
    record_history_entry(
        &state.db,
        id,
        root_id,
        "overlay_reset",
        json!({ "overlay": "reset" }),
        None,
        Some(&registration.overlay.clone().unwrap_or_else(|| json!({}))),
        Some(&json!({})),
        auth.id,
    )
    .await?;

    Ok(Json(
        registration_json_with_versions(&state.db, &updated, account_id).await?,
    ))
}

/// `POST /accounts/{account_id}/lti_registrations/{id}/install_from_template`
/// — install a local copy of a template registration
/// (registrations_controller.rb:1822-1862).
async fn install_from_template(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let template = find_active_registration(&state.db, id).await?;

    // Canvas' InstallTemplateRegistrationService reuses an existing local
    // copy instead of refusing (install_template_registration_service.rb:
    // 38-48, `find_by` + `create_local_copy unless local_copy.present?`).
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM lti_registrations \
         WHERE template_registration_id = $1 AND account_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(id)
    .bind(account_id)
    .fetch_optional(&state.db)
    .await?
    .flatten();

    let root_id = root_account_id(&state.db, account_id).await?;
    let local_copy: RegistrationRow = match existing {
        Some(local_copy_id) => find_active_registration(&state.db, local_copy_id).await?,
        None => {
            let sql = format!(
                "INSERT INTO lti_registrations \
                 (account_id, root_account_id, name, admin_nickname, vendor, description, icon_url, \
                  lti_version, dynamic_registration, lock_deploying, template_registration_id, \
                  workflow_state, configuration, created_by_id, updated_by_id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'active', $12, $13, $13) \
                 RETURNING {REGISTRATION_COLUMNS}"
            );
            sqlx::query_as(&sql)
                .bind(account_id)
                .bind(root_id)
                .bind(&template.name)
                .bind(template.admin_nickname.clone())
                .bind(template.vendor.clone())
                .bind(template.description.clone())
                .bind(template.icon_url.clone())
                .bind(&template.lti_version)
                .bind(template.dynamic_registration)
                .bind(template.lock_deploying)
                .bind(id)
                .bind(&template.configuration)
                .bind(auth.id)
                .fetch_one(&state.db)
                .await?
        }
    };

    sqlx::query(
        "INSERT INTO lti_registration_account_bindings \
         (registration_id, account_id, root_account_id, workflow_state, created_by_id, updated_by_id) \
         VALUES ($1, $2, $3, 'on', $4, $4) \
         ON CONFLICT (registration_id, account_id) DO UPDATE \
         SET workflow_state = 'on', updated_at = now()",
    )
    .bind(local_copy.id)
    .bind(account_id)
    .bind(root_id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    record_history_entry(
        &state.db,
        local_copy.id,
        root_id,
        "installed_from_template",
        json!({ "template_registration_id": id }),
        None,
        None,
        Some(&local_copy.configuration),
        auth.id,
    )
    .await?;

    let includes = Includes {
        configuration: true,
        overlay: true,
    };
    let body = registration_json(&state.db, &local_copy, includes).await?;
    Ok(Json(body).into_response())
}

/// A row of `lti_registration_update_requests` (Canvas
/// `lti_registration_update_request_json` shape, lib/api/v1/lti/
/// registration_update_request.rb).
#[derive(Debug, Clone, sqlx::FromRow)]
struct UpdateRequestRow {
    id: i64,
    root_account_id: Option<i64>,
    lti_registration_id: i64,
    uuid: String,
    reinstall: bool,
    tool_initiated: bool,
    comment: Option<String>,
    created_by_id: Option<i64>,
    accepted_at: Option<DateTime<Utc>>,
    rejected_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

async fn update_request_json(pool: &PgPool, rur_id: i64) -> Result<Option<Value>, ApiError> {
    let row: Option<UpdateRequestRow> = sqlx::query_as(
        "SELECT id, root_account_id, lti_registration_id, uuid, reinstall, \
                tool_initiated, comment, created_by_id, \
                accepted_at, rejected_at, created_at, updated_at \
         FROM lti_registration_update_requests WHERE id = $1",
    )
    .bind(rur_id)
    .fetch_optional(pool)
    .await?;
    let Some(rur) = row else {
        return Ok(None);
    };
    let status = if rur.accepted_at.is_some() {
        "applied"
    } else if rur.rejected_at.is_some() {
        "rejected"
    } else {
        "pending"
    };
    let mut json = json!({
        "id": rur.id,
        "root_account_id": rur.root_account_id,
        "lti_registration_id": rur.lti_registration_id,
        "uuid": rur.uuid,
        "reinstall": rur.reinstall,
        "tool_initiated": rur.tool_initiated,
        "comment": rur.comment,
        "created_at": rur.created_at,
        "updated_at": rur.updated_at,
        "accepted_at": rur.accepted_at,
        "rejected_at": rur.rejected_at,
        "status": status,
    });
    if let Some(user) = user_summary(pool, rur.created_by_id).await? {
        json["created_by"] = user;
    }
    // The workspace's lti_registration_update_requests (migration 0115)
    // carries no updated_by column, so Canvas' updated_by is always null.
    json["updated_by"] = Value::Null;
    Ok(Some(json))
}

/// `GET .../lti_registrations/{id}/latest_update_request`
/// (registrations_controller.rb:2058-2094).
async fn latest_update_request(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    // Canvas `require_in_account_or_site_admin` renders this one as a JSON
    // body (registrations_controller.rb:2299-2303); the plain-text
    // ApiError::BadRequest shape would not match the API.
    let registration = match find_registration_in_account(&state.db, id, account_id).await {
        Ok(registration) => registration,
        Err(ApiError::BadRequest(message)) => {
            return Ok(
                (StatusCode::BAD_REQUEST, Json(json!({ "errors": message }))).into_response(),
            )
        }
        Err(error) => return Err(error),
    };
    require_registration_permission(&state.db, &auth, &registration).await?;

    let latest: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_registration_update_requests \
         WHERE lti_registration_id = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some((rur_id,)) = latest else {
        return Err(ApiError::NotFound);
    };
    let json = update_request_json(&state.db, rur_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(json).into_response())
}

/// `GET .../lti_registrations/{id}/update_requests/{update_request_id}`
/// (registrations_controller.rb:2028-2056).
async fn show_update_request(
    State(state): State<AppState>,
    Path((account_id, id, update_request_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    let found: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_registration_update_requests \
         WHERE id = $1 AND lti_registration_id = $2",
    )
    .bind(update_request_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some((rur_id,)) = found else {
        return Err(ApiError::NotFound);
    };
    let json = update_request_json(&state.db, rur_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(json))
}

/// `PUT .../lti_registrations/{id}/update_requests/{update_request_id}/apply`
/// — accept (and apply) or reject a registration update request
/// (registrations_controller.rb:2096-2148).
async fn apply_update_request(
    State(state): State<AppState>,
    Path((account_id, id, update_request_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<ApplyUpdateRequest>,
) -> Result<Response, ApiError> {
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    // Canvas scopes to Lti::RegistrationUpdateRequest.active — already
    // applied or rejected requests 404.
    let rur: Option<(i64, Value, Option<i64>)> = sqlx::query_as(
        "SELECT id, lti_ims_registration, created_by_id \
         FROM lti_registration_update_requests \
         WHERE id = $1 AND lti_registration_id = $2 \
           AND accepted_at IS NULL AND rejected_at IS NULL",
    )
    .bind(update_request_id)
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some((rur_id, ims_registration, _rur_author)) = rur else {
        return Err(ApiError::NotFound);
    };

    let Some(accepted) = payload.accepted else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "errors": "accepted parameter is required" })),
        )
            .into_response());
    };

    // most_recent? — a newer pending request makes this one outdated.
    let newer: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_registration_update_requests \
         WHERE lti_registration_id = $1 AND id <> $2 \
           AND accepted_at IS NULL AND rejected_at IS NULL \
           AND created_at > (SELECT created_at FROM lti_registration_update_requests WHERE id = $2) \
         LIMIT 1",
    )
    .bind(id)
    .bind(rur_id)
    .fetch_optional(&state.db)
    .await?;
    if newer.is_some() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "errors": "Cannot apply an outdated registration update request. A newer update request exists." })),
        )
            .into_response());
    }

    let root_id = root_account_id(&state.db, account_id).await?;
    let old_configuration = registration.configuration.clone();

    if accepted {
        // ApplyRegistrationUpdateRequestService replaces the configuration
        // with the update request's stored registration state. The workspace
        // stores the IMS registration payload in
        // lti_registration_update_requests.lti_ims_registration; its
        // `lti_tool_configuration` (or the object itself) is the new base
        // configuration, its `client_name` the new name.
        let new_configuration = ims_registration
            .get("lti_tool_configuration")
            .filter(|config| config.is_object())
            .cloned()
            .unwrap_or(ims_registration.clone());
        let new_name = ims_registration
            .get("client_name")
            .and_then(Value::as_str)
            .map(str::to_string);

        if let Some(overlay) = &payload.overlay {
            if !overlay.is_object() {
                return Ok(configuration_error_list(vec![
                    "overlay must be an object".to_string()
                ]));
            }
        }

        let sql = format!(
            "UPDATE lti_registrations SET \
             name = COALESCE($2, name), \
             configuration = $3, \
             overlay = COALESCE($4, overlay), \
             updated_by_id = $5, updated_at = now() \
             WHERE id = $1 \
             RETURNING {REGISTRATION_COLUMNS}"
        );
        let updated: Option<RegistrationRow> = sqlx::query_as(&sql)
            .bind(id)
            .bind(new_name)
            .bind(&new_configuration)
            .bind(&payload.overlay)
            .bind(auth.id)
            .fetch_optional(&state.db)
            .await?;
        let Some(updated) = updated else {
            return Err(ApiError::NotFound);
        };

        if let Some(overlay) = &payload.overlay {
            record_overlay_version(&state.db, id, account_id, root_id, overlay, false, auth.id)
                .await?;
        }
        sqlx::query(
            "UPDATE lti_registration_update_requests SET accepted_at = now(), \
             updated_at = now() WHERE id = $1",
        )
        .bind(rur_id)
        .execute(&state.db)
        .await?;
        record_history_entry(
            &state.db,
            id,
            root_id,
            "update_request_applied",
            json!({ "update_request_id": rur_id }),
            payload.comment.as_deref(),
            Some(&old_configuration),
            Some(&updated.configuration),
            auth.id,
        )
        .await?;

        Ok(
            Json(registration_json_with_versions(&state.db, &updated, account_id).await?)
                .into_response(),
        )
    } else {
        sqlx::query(
            "UPDATE lti_registration_update_requests SET rejected_at = now(), \
             updated_at = now() WHERE id = $1",
        )
        .bind(rur_id)
        .execute(&state.db)
        .await?;
        record_history_entry(
            &state.db,
            id,
            root_id,
            "update_request_rejected",
            json!({ "update_request_id": rur_id }),
            payload.comment.as_deref(),
            None,
            None,
            auth.id,
        )
        .await?;
        Ok(
            Json(registration_json_with_versions(&state.db, &registration, account_id).await?)
                .into_response(),
        )
    }
}

/// `GET .../deployments/{deployment_id}` — show one deployment
/// (deployments_controller.rb:103-110).
async fn show_deployment_scoped(
    State(state): State<AppState>,
    Path((account_id, registration_id, deployment_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let registration = find_registration_in_account(&state.db, registration_id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;
    let deployment = find_active_deployment(&state.db, registration_id, deployment_id).await?;
    let root = deployment.context_type == "Account"
        && is_root_account(&state.db, deployment.context_id).await?;
    Ok(Json(deployment_json(&deployment, root)))
}

/// `DELETE .../deployments/{deployment_id}` — soft-delete a deployment
/// (deployments_controller.rb:158-175); Canvas renders the destroyed
/// deployment's JSON.
async fn delete_deployment_scoped(
    State(state): State<AppState>,
    Path((account_id, registration_id, deployment_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let registration = find_registration_in_account(&state.db, registration_id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;
    let deployment = find_active_deployment(&state.db, registration_id, deployment_id).await?;

    sqlx::query(
        "UPDATE lti_deployments SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(deployment_id)
    .execute(&state.db)
    .await?;

    // Canvas renders the destroyed deployment's JSON (deployments_
    // controller.rb:164-170), with workflow_state 'deleted' on the returned
    // record — the same shape delete_registration_scoped uses.
    let mut deleted = deployment;
    deleted.workflow_state = STATE_DELETED.to_string();
    let root =
        deleted.context_type == "Account" && is_root_account(&state.db, deleted.context_id).await?;
    Ok(Json(deployment_json(&deleted, root)))
}

/// An active deployment of a registration (Canvas
/// `ContextExternalTool.for_lti_registration`).
pub(crate) async fn find_active_deployment(
    pool: &PgPool,
    registration_id: i64,
    deployment_id: i64,
) -> Result<DeploymentRow, ApiError> {
    let sql = format!(
        "SELECT {DEPLOYMENT_COLUMNS} FROM lti_deployments \
         WHERE id = $1 AND registration_id = $2 AND workflow_state <> '{STATE_DELETED}'"
    );
    let row: Option<DeploymentRow> = sqlx::query_as(&sql)
        .bind(deployment_id)
        .bind(registration_id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// `GET .../deployments/{deployment_id}/context_search` — accounts and
/// courses usable as deployment contexts (registrations_controller.rb:
/// 1864-1966).
async fn deployment_context_search(
    State(state): State<AppState>,
    Path((account_id, registration_id, deployment_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Query(params): Query<ContextSearchQuery>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    // Canvas `require_lti_registrations_next_feature_flag`
    // (registrations_controller.rb:1114, 2275-2282) — 404 without the flag;
    // it runs before the manage check in the before_action chain.
    let search_root = root_account_id(&state.db, account_id).await?;
    if !feature_enabled(&state.db, search_root, "lti_registrations_next").await? {
        return Ok(render_not_found());
    }
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = find_active_registration(&state.db, registration_id).await?;
    let deployment = find_active_deployment(&state.db, registration_id, deployment_id).await?;

    if deployment.context_type != "Account" {
        return Ok(Json(json!({ "accounts": [], "courses": [] })).into_response());
    }

    // The deployment's account subtree.
    let subtree: Vec<(i64, String)> = sqlx::query_as(
        "WITH RECURSIVE down AS ( \
             SELECT id, name, parent_account_id, workflow_state FROM accounts WHERE id = $1 \
             UNION ALL \
             SELECT a.id, a.name, a.parent_account_id, a.workflow_state FROM accounts a \
             JOIN down ON a.parent_account_id = down.id \
         ) SELECT id, name FROM down WHERE workflow_state <> 'deleted'",
    )
    .bind(deployment.context_id)
    .fetch_all(&state.db)
    .await?;
    let subtree_ids: Vec<i64> = subtree.iter().map(|(id, _)| *id).collect();

    // Canvas lists the SUB-accounts of the deployment account
    // (`Account.sub_account_ids_recursive`, registrations_controller.rb:1881)
    // — the deployment account itself is not among the accounts.
    let account_scope_ids: Vec<i64> = subtree_ids
        .iter()
        .copied()
        .filter(|id| *id != deployment.context_id)
        .collect();

    let only_children_of = params.only_children_of;
    if let Some(filter_id) = only_children_of {
        if !subtree_ids.contains(&filter_id) {
            return Ok(render_error(
                "invalid_account_id",
                "only_children_of account ID must be or belong to deployment's account",
            ));
        }
    }

    let search = params
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| format!("%{term}%"));

    // Canvas also matches ids when the search term parses as a positive
    // integer (:1914-1918).
    let search_id = params
        .search_term
        .as_deref()
        .map(str::trim)
        .and_then(|term| term.parse::<i64>().ok())
        .filter(|parsed| *parsed > 0);

    let account_rows: Vec<(i64, String)> = if let Some(filter_id) = only_children_of {
        sqlx::query_as(
            "SELECT id, name FROM accounts \
             WHERE parent_account_id = $1 AND workflow_state <> 'deleted' \
               AND ($2::text IS NULL OR name ILIKE $2) \
             ORDER BY name LIMIT 20",
        )
        .bind(filter_id)
        .bind(search.as_deref())
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, name FROM accounts \
             WHERE id = ANY($1) AND workflow_state <> 'deleted' \
               AND ($2::text IS NULL OR name ILIKE $2) \
             ORDER BY name LIMIT 20",
        )
        .bind(&account_scope_ids)
        .bind(search.as_deref())
        .fetch_all(&state.db)
        .await?
    };
    let mut account_rows = account_rows;
    if let (Some(search_id), Some(pattern)) = (search_id, search.as_deref()) {
        let extra: Option<(i64, String)> = sqlx::query_as(
            "SELECT id, name FROM accounts \
             WHERE id = $1 AND workflow_state <> 'deleted' \
               AND NOT ($2::text IS NULL OR name ILIKE $2) \
             LIMIT 1",
        )
        .bind(search_id)
        .bind(pattern)
        .fetch_optional(&state.db)
        .await?;
        if let Some(extra) = extra {
            account_rows.push(extra);
        }
    }

    let course_rows: Vec<(i64, String, String)> = if let Some(filter_id) = only_children_of {
        sqlx::query_as(
            "SELECT id, name, course_code FROM courses \
             WHERE account_id = $1 AND workflow_state <> 'deleted' \
               AND ($2::text IS NULL OR name ILIKE $2 OR course_code ILIKE $2) \
             ORDER BY name LIMIT 20",
        )
        .bind(filter_id)
        .bind(search.as_deref())
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, name, course_code FROM courses \
             WHERE (account_id = ANY($1) OR account_id = $3) AND workflow_state <> 'deleted' \
               AND ($2::text IS NULL OR name ILIKE $2 OR course_code ILIKE $2) \
             ORDER BY name LIMIT 20",
        )
        .bind(&subtree_ids)
        .bind(search.as_deref())
        .bind(deployment.context_id)
        .fetch_all(&state.db)
        .await?
    };
    let mut course_rows = course_rows;
    if let (Some(search_id), Some(pattern)) = (search_id, search.as_deref()) {
        let extra: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT id, name, course_code FROM courses \
             WHERE id = $1 AND workflow_state <> 'deleted' \
               AND NOT ($2::text IS NULL OR name ILIKE $2 OR course_code ILIKE $2) \
             LIMIT 1",
        )
        .bind(search_id)
        .bind(pattern)
        .fetch_optional(&state.db)
        .await?;
        if let Some(extra) = extra {
            course_rows.push(extra);
        }
    }

    // Canvas limits each list to 20 after the id OR-merge.
    account_rows.truncate(20);
    course_rows.truncate(20);

    // The workspace schema carries no SIS columns, so sis_id is always
    // null (Canvas nulls it for callers without read_sis).
    let mut accounts_json = Vec::with_capacity(account_rows.len());
    for (id, name) in &account_rows {
        let display_path = display_path_names(&state.db, *id, false).await?;
        accounts_json.push(json!({
            "id": id.to_string(),
            "name": name,
            "sis_id": Value::Null,
            "display_path": display_path,
        }));
    }

    let mut courses_json = Vec::with_capacity(course_rows.len());
    for (id, name, course_code) in &course_rows {
        let account_of_course: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
                .bind(id)
                .fetch_optional(&state.db)
                .await?;
        let display_path = match account_of_course {
            Some((Some(course_account),)) => {
                display_path_names(&state.db, course_account, true).await?
            }
            _ => vec![],
        };
        courses_json.push(json!({
            "id": id.to_string(),
            "name": name,
            "sis_id": Value::Null,
            "course_code": course_code,
            "display_path": display_path,
        }));
    }

    Ok(Json(json!({ "accounts": accounts_json, "courses": courses_json })).into_response())
}

/// Find an active registration by client id. Canvas resolves
/// `DeveloperKey#lti_registration` (registrations_controller.rb:1462-1480):
/// the client id IS the developer key id and the registration is linked
/// through the developer key's IMS registration. The workspace keeps that
/// link in `lti_ims_registrations` (migration 0115); registrations created
/// without a developer key carry the client id inside their configuration
/// JSON, so that spelling resolves as well.
async fn find_registration_by_client_id(
    pool: &PgPool,
    client_id: &str,
) -> Result<Option<RegistrationRow>, ApiError> {
    if let Ok(key_id) = client_id.parse::<i64>() {
        let linked: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT lti_registration_id FROM lti_ims_registrations \
             WHERE developer_key_id = $1 LIMIT 1",
        )
        .bind(key_id)
        .fetch_optional(pool)
        .await?;
        if let Some((Some(registration_id),)) = linked {
            if let Ok(row) = find_active_registration(pool, registration_id).await {
                return Ok(Some(row));
            }
        }
    }
    let sql = format!(
        "SELECT {REGISTRATION_COLUMNS} FROM lti_registrations \
         WHERE configuration->>'client_id' = $1 AND workflow_state <> '{STATE_DELETED}'"
    );
    let row: Option<RegistrationRow> = sqlx::query_as(&sql)
        .bind(client_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// `GET /accounts/{account_id}/lti_registration_by_client_id/{client_id}`
/// (registrations_controller.rb:1462-1480).
async fn show_registration_by_client_id(
    State(state): State<AppState>,
    Path((account_id, client_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "errors": "LTI registration not found" })),
        )
            .into_response()
    };
    let Some(registration) = find_registration_by_client_id(&state.db, &client_id).await? else {
        return Ok(not_found());
    };
    if registration.account_id != account_id {
        return Ok(not_found());
    }
    let includes = Includes {
        configuration: true,
        overlay: true,
    };
    let mut json = registration_json(&state.db, &registration, includes).await?;
    let overlay = registration.overlay.clone().unwrap_or_else(|| json!({}));
    json["overlaid_configuration"] =
        overlaid_configuration(&registration.configuration, Some(&overlay));
    Ok(Json(json).into_response())
}

/// `GET /accounts/{account_id}/lti_registrations/by_utid/{utid}`
/// (registrations_controller.rb:1502-1541) — searches manual registrations
/// and IMS registrations by unified tool id.
async fn show_registration_by_utid(
    State(state): State<AppState>,
    Path((account_id, utid)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "errors": "LTI registration not found" })),
        )
            .into_response()
    };

    let sql = format!(
        "SELECT {REGISTRATION_COLUMNS} FROM lti_registrations \
         WHERE unified_tool_id = $1 AND workflow_state <> '{STATE_DELETED}'"
    );
    let mut registration: Option<RegistrationRow> = sqlx::query_as(&sql)
        .bind(&utid)
        .fetch_optional(&state.db)
        .await?;
    if registration.is_none() {
        let via_ims: Option<(i64,)> = sqlx::query_as(
            "SELECT lti_registration_id FROM lti_ims_registrations \
             WHERE unified_tool_id = $1 AND lti_registration_id IS NOT NULL",
        )
        .bind(&utid)
        .fetch_optional(&state.db)
        .await?;
        if let Some((registration_id,)) = via_ims {
            registration = find_active_registration(&state.db, registration_id)
                .await
                .ok();
        }
    }
    let Some(registration) = registration else {
        return Ok(not_found());
    };
    if registration.account_id != account_id {
        return Ok(not_found());
    }
    let includes = Includes {
        configuration: true,
        overlay: false,
    };
    Ok(Json(registration_json(&state.db, &registration, includes).await?).into_response())
}

/// `GET .../lti_registrations/check_domain_duplicates?domain=`
/// (registrations_controller.rb:1312-1338) backed by
/// `Lti::CheckDomainDuplicatesService` (app/services/lti/
/// check_domain_duplicates_service.rb): the domain match is CASE
/// INSENSITIVE (`LOWER(...)`), the ids are integers (the service plucks
/// `:id`), and without the `lti_registrations_templates` feature the
/// Site Admin registrations forced on and the inherited registrations
/// that are enabled count as well.
async fn check_domain_duplicates(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<DomainQuery>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let domain = params.domain.unwrap_or_default();
    let trimmed = domain.trim().to_lowercase();
    if trimmed.is_empty() {
        return Ok(Json(json!({ "duplicates": [] })));
    }

    // The workspace's Site Admin is the first parent-less account.
    let site_admin: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;
    let root_id = root_account_id(&state.db, account_id).await?;
    let templates_enabled =
        feature_enabled(&state.db, root_id, "lti_registrations_templates").await?;

    let mut scope = if templates_enabled {
        // With templates only the account's own registrations count (the
        // local copies carry the domain of their template).
        "r.account_id = $1 AND r.workflow_state <> 'deleted'".to_string()
    } else {
        // Without templates the local copies are hidden.
        "r.account_id = $1 AND r.workflow_state <> 'deleted' \
         AND r.template_registration_id IS NULL"
            .to_string()
    };
    let mut duplicates =
        registrations_with_domain(&state.db, &scope, account_id, None, &trimmed).await?;
    if !templates_enabled {
        if let Some(site_admin) = site_admin {
            // The Site Admin registrations that are forced on...
            scope = "r.account_id = $2 AND r.workflow_state <> 'deleted' \
                 AND EXISTS (SELECT 1 FROM lti_registration_account_bindings b \
                             WHERE b.registration_id = r.id AND b.account_id = $2 \
                               AND b.workflow_state = 'on')"
                .to_string();
            let forced_on = registrations_with_domain(
                &state.db,
                &scope,
                account_id,
                Some(site_admin),
                &trimmed,
            )
            .await?;
            let forced_on_ids: Vec<i64> = forced_on
                .iter()
                .filter_map(|entry| entry.get("id").and_then(Value::as_i64))
                .collect();
            duplicates.extend(forced_on);

            // ...and the inherited registrations that are enabled.
            scope = "r.account_id = $2 AND r.workflow_state <> 'deleted' \
                 AND EXISTS (SELECT 1 FROM lti_registration_account_bindings b \
                             WHERE b.registration_id = r.id AND b.account_id = $1 \
                               AND b.workflow_state = 'on')"
                .to_string();
            let inherited = registrations_with_domain(
                &state.db,
                &scope,
                account_id,
                Some(site_admin),
                &trimmed,
            )
            .await?;
            duplicates.extend(inherited.into_iter().filter(|entry| {
                !forced_on_ids.contains(&entry.get("id").and_then(Value::as_i64).unwrap_or(-1))
            }));
        }
    }

    Ok(Json(json!({ "duplicates": duplicates })))
}

/// The registrations of a scope whose domain matches — the configuration
/// domain or the IMS registration's tool configuration domain, compared
/// case insensitively, limited to three like the Canvas service.
async fn registrations_with_domain(
    pool: &PgPool,
    scope: &str,
    account_id: i64,
    site_admin_id: Option<i64>,
    normalized_domain: &str,
) -> Result<Vec<Value>, ApiError> {
    let sql = format!(
        "SELECT r.id, r.name, r.admin_nickname FROM lti_registrations r \
         WHERE {scope} \
           AND ($2::bigint IS NULL OR $2::bigint IS NOT NULL) \
           AND (LOWER(r.configuration->>'domain') = $3 \
                OR LOWER((SELECT i.lti_tool_configuration->>'domain' \
                          FROM lti_ims_registrations i \
                          WHERE i.lti_registration_id = r.id LIMIT 1)) = $3) \
         LIMIT 3"
    );
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(&sql)
        .bind(account_id)
        .bind(site_admin_id)
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

/// `POST .../lti_registrations/configuration/validate`
/// (registrations_controller.rb:1244-1310). Validates an LTI 1.3 tool
/// configuration — inline or fetched from a URL — against the Canvas schema
/// (lib/schemas/lti_configuration.rb: title, description, target_link_uri,
/// oidc_initiation_url) and returns the internal configuration.
async fn validate_configuration(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    payload: Result<Json<ValidateConfigurationRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    // Canvas `require_root_account_instrumented_or_sub_account_and_feature_flag`
    // (registrations_controller.rb:1113, 2246-2263): a sub-account whose root
    // does not carry `canvas_apps_sub_account_access` is a 404.
    if let Some(response) = require_root_or_sub_account_feature(&state.db, account_id).await? {
        return Ok(response);
    }
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas `restrict_sub_account_to_read_only` (:1119, 2317-2321) —
    // `validate_lti_configuration` is not in the except list, so sub-accounts
    // may not use it.
    if let Some(response) = restrict_sub_account_to_read_only(&state.db, account_id).await? {
        return Ok(response);
    }
    let payload = match payload {
        Ok(Json(payload)) => payload,
        Err(_) => {
            return Ok(configuration_error_list(vec![
                "one of lti_configuration or url is required".to_string(),
            ]))
        }
    };

    let has_config = payload.lti_configuration.is_some();
    let has_url = payload
        .url
        .as_deref()
        .is_some_and(|url| !url.trim().is_empty());
    if !has_config && !has_url {
        return Ok(configuration_error_list(vec![
            "one of lti_configuration or url is required".to_string(),
        ]));
    }
    if has_config && has_url {
        return Ok(configuration_error_list(vec![
            "only one of lti_configuration or url is allowed".to_string(),
        ]));
    }

    let config = if let Some(config) = payload.lti_configuration {
        config
    } else {
        // Canvas fetches the URL and parses the body as JSON
        // (registrations_controller.rb:1264-1284).
        let url = payload.url.unwrap_or_default();
        let response = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| ApiError::Db(e.to_string()))?
            .get(&url)
            .send()
            .await;
        let response = match response {
            Ok(response) if response.status().is_success() => response,
            _ => {
                return Ok(configuration_error_list(vec![
                    "invalid configuration url".to_string()
                ]))
            }
        };
        let text = response
            .text()
            .await
            .map_err(|_| ApiError::Db("configuration url read failed".to_string()))?;
        match serde_json::from_str(&text) {
            Ok(config) => config,
            Err(_) => {
                return Ok(configuration_error_list(vec![
                    "url does not return JSON".to_string()
                ]))
            }
        }
    };

    if !config.is_object() {
        return Ok(configuration_error_list(vec![
            "lti_configuration must be an object".to_string(),
        ]));
    }
    let mut errors = Vec::new();
    for field in [
        "title",
        "description",
        "target_link_uri",
        "oidc_initiation_url",
    ] {
        if !config.get(field).is_some_and(Value::is_string) {
            errors.push(format!("{field} is missing"));
        }
    }
    if !errors.is_empty() {
        return Ok(configuration_error_list(errors));
    }

    // Schemas::InternalLtiConfiguration.from_lti_configuration + the
    // redirect_uris defaulting the controller performs (:1294-1302).
    let mut configuration = from_lti_configuration(&config);
    let target_link_uri = config
        .get("target_link_uri")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let redirect_uris = match configuration.get("redirect_uris") {
        Some(Value::Array(uris)) => json!(uris),
        Some(Value::String(uri)) => json!([uri]),
        _ => json!([target_link_uri]),
    };
    configuration["redirect_uris"] = redirect_uris;

    Ok(Json(json!({ "configuration": configuration })).into_response())
}

/// Canvas `Schemas::InternalLtiConfiguration.from_lti_configuration`
/// (lib/schemas/internal_lti_configuration.rb:37-59): the Canvas extension
/// of the `extensions` array folds its `settings` into `launch_settings`
/// (placements are pulled out of them), `scopes` defaults to `[]`, and the
/// result always carries `launch_settings` and `placements`.
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

/// `GET .../lti_registrations/install_status/{client_id}`
/// (registrations_controller.rb:1543-1635) — the local installation of a
/// registration: the registration itself when it lives in this account, or
/// its local copy (template install) otherwise.
async fn install_status(
    State(state): State<AppState>,
    Path((account_id, client_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "errors": "LTI registration not found" })),
        )
            .into_response()
    };
    let Some(registration) = find_registration_by_client_id(&state.db, &client_id).await? else {
        return Ok(not_found());
    };

    let installed = if registration.account_id == account_id {
        registration
    } else {
        let sql = format!(
            "SELECT {REGISTRATION_COLUMNS} FROM lti_registrations \
             WHERE template_registration_id = $1 AND account_id = $2 \
               AND workflow_state <> '{STATE_DELETED}'"
        );
        let local: Option<RegistrationRow> = sqlx::query_as(&sql)
            .bind(registration.id)
            .bind(account_id)
            .fetch_optional(&state.db)
            .await?;
        let Some(local) = local else {
            return Ok(not_found());
        };
        local
    };

    let includes = Includes {
        configuration: true,
        overlay: true,
    };
    Ok(Json(registration_json(&state.db, &installed, includes).await?).into_response())
}
