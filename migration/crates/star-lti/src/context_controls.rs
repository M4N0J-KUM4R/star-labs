//! Canvas parity for the LTI context controls REST API
//! (`Lti::ContextControlsController`).
//!
//! Canvas source:
//! - `app/controllers/lti/context_controls_controller.rb`
//! - `lib/api/v1/lti/context_control.rb` (the control JSON)
//! - `lib/api/v1/lti/deployment.rb` (the deployment JSON)
//! - `app/models/lti/context_control.rb`
//! - `app/services/lti/context_control_service.rb`
//!
//! Routes:
//! - `GET    /accounts/{account_id}/lti_registrations/{registration_id}/controls` — list, grouped by deployment
//! - `POST   /accounts/{account_id}/lti_registrations/{registration_id}/controls` — create
//! - `POST   /accounts/{account_id}/lti_registrations/{registration_id}/controls/bulk` — bulk create
//! - `GET    /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}` — show
//! - `PUT    /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}` — change availability
//! - `DELETE /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}` — delete
//!
//! Canvas' `POST` route spells the account segment `:current_account_id` so
//! that the `account_id` body parameter does not clash with it; the segment
//! is the same account, so it is registered here as `{account_id}`.
//!
//! The `before_action` chain is reproduced in order: a missing account is a
//! 404, a non-root account is a 404, a missing `lti_registrations_next`
//! feature flag is a 404, and a caller without `manage_lti_registrations` is
//! a 403.
//!
//! ## Required DDL (for the coordinator)
//!
//! `lti_context_controls` already exists (migration `0224`), but it was
//! created with both `account_id` and `course_id` `NOT NULL` plus a check
//! constraint that requires exactly one of them to be set — a combination
//! that no row can satisfy. Canvas' own definition has both columns
//! nullable. The table also carried a foreign key on `deployment_id` that
//! pointed at Canvas' `context_external_tools`, while this workspace keeps
//! its LTI deployments in `lti_deployments` (the table the rest of star-lti
//! uses, and the only one with a `deployment_id` string and a
//! `context_name`). Migration `0233` repairs both; the statements it runs
//! are:
//!
//! ```sql
//! ALTER TABLE lti_context_controls
//!     DROP CONSTRAINT IF EXISTS chk_require_context_4;
//!
//! ALTER TABLE lti_context_controls
//!     ALTER COLUMN account_id DROP NOT NULL,
//!     ALTER COLUMN course_id DROP NOT NULL;
//!
//! ALTER TABLE lti_context_controls
//!     DROP CONSTRAINT IF EXISTS fk_lti_context_controls_deployment_id;
//!
//! ALTER TABLE lti_context_controls
//!     ADD CONSTRAINT fk_lti_context_controls_deployment_id
//!     FOREIGN KEY (deployment_id) REFERENCES lti_deployments (id) ON DELETE CASCADE;
//! ```
//!
//! The partial unique indexes the bulk upsert relies on
//! (`index_lti_context_controls_on_account_and_deployment` and
//! `index_lti_context_controls_on_course_and_deployment`) already exist in
//! migration `0226`.
//!
//! `lti_context_controls.created_at` / `updated_at` are `timestamp without
//! time zone` (as created by `0224`), so they are read as `NaiveDateTime`
//! and written with `timezone('UTC', now())`.

use std::collections::{HashMap, HashSet};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{can_manage_account, find_active_registration, is_root_account, RegistrationRow};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Canvas `Lti::ContextControl` workflow states.
const STATE_ACTIVE: &str = "active";
const STATE_DELETED: &str = "deleted";
const STATE_DELETED_WITH_CONTEXT: &str = "deleted_with_context";
/// Canvas `ContextExternalTool` workflow state.
const STATE_DISABLED: &str = "disabled";

/// Canvas `MAX_BULK_CREATE`.
const MAX_BULK_CREATE: usize = 100;

/// Canvas `CONTROLS_DEFAULT_LIST_PAGE_SIZE`.
const CONTROLS_DEFAULT_LIST_PAGE_SIZE: i64 = 20;

/// Canvas `CONTROLS_MAX_LIST_PAGE_SIZE`.
const CONTROLS_MAX_LIST_PAGE_SIZE: i64 = 100;

/// Columns of `lti_context_controls` read by this module.
const CONTROL_COLUMNS: &str = "id, account_id, course_id, registration_id, app_id, deployment_id, \
     available, path, workflow_state, created_by_id, updated_by_id, created_at, updated_at";

/// The same columns, qualified for the join against `lti_deployments`.
const CONTROL_COLUMNS_QUALIFIED: &str = "c.id, c.account_id, c.course_id, c.registration_id, \
     c.app_id, c.deployment_id, c.available, c.path, c.workflow_state, c.created_by_id, \
     c.updated_by_id, c.created_at, c.updated_at";

/// Columns of `lti_deployments` read by this module.
const DEPLOYMENT_COLUMNS: &str =
    "id, registration_id, deployment_id, context_type, context_id, context_name, workflow_state";

// ---------------------------------------------------------------------------
// Domain rows
// ---------------------------------------------------------------------------

/// A row of `lti_context_controls`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ContextControlRow {
    id: i64,
    account_id: Option<i64>,
    course_id: Option<i64>,
    registration_id: i64,
    app_id: Option<i64>,
    deployment_id: i64,
    available: bool,
    path: String,
    workflow_state: String,
    created_by_id: Option<i64>,
    updated_by_id: Option<i64>,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}

/// A row of `lti_deployments` — the workspace's stand-in for Canvas'
/// `ContextExternalTool`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct DeploymentRow {
    id: i64,
    registration_id: i64,
    deployment_id: String,
    context_type: String,
    context_id: i64,
    context_name: Option<String>,
    workflow_state: String,
}

/// Canvas `Lti::ContextControlService.preload_calculated_attrs` — the
/// UI-only fields of a control.
#[derive(Debug, Clone, Default)]
struct CalculatedAttrs {
    context_name: Option<String>,
    child_control_count: i64,
    subaccount_count: i64,
    course_count: i64,
    display_path: Vec<String>,
    depth: i64,
}

/// The parameters Canvas assembles before writing a control.
#[derive(Debug, Clone)]
struct ControlDraft {
    account_id: Option<i64>,
    course_id: Option<i64>,
    deployment_id: i64,
    available: bool,
    path: String,
    registration_id: i64,
    app_id: Option<i64>,
    root_account_id: i64,
    created_by_id: Option<i64>,
    updated_by_id: Option<i64>,
}

/// The course rows the bulk endpoint needs.
#[derive(Debug, Clone)]
struct CourseInfo {
    name: String,
    /// `courses.account_id` is nullable in this workspace.
    account_id: Option<i64>,
}

/// The request context of a control: the root account from the path.
#[derive(Debug, Clone, Copy)]
struct Ctx {
    account_id: i64,
}

/// The pre-loaded data `Lti::ContextControlService.build_anchor_controls`
/// needs to place the anchor controls.
#[derive(Debug, Default)]
struct AnchorContext {
    /// Account chain (leaf first) per account id.
    chains: HashMap<i64, Vec<i64>>,
    /// Account id per course id.
    course_account_ids: HashMap<i64, i64>,
    /// Context account id per deployment id (account deployments only).
    deployment_account_ids: HashMap<i64, i64>,
    /// Context course id per deployment id (course deployments only).
    deployment_course_ids: HashMap<i64, i64>,
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Canvas accepts ids as JSON numbers or as strings ("Can also be a
/// string") and casts them with `to_i`.
fn flexible_id<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IdValue {
        Int(i64),
        Str(String),
    }

    let value = Option::<IdValue>::deserialize(deserializer)?;
    Ok(match value {
        None => None,
        Some(IdValue::Int(value)) => Some(value),
        Some(IdValue::Str(value)) => Some(value.trim().parse::<i64>().unwrap_or(0)),
    })
}

/// Rails' `value_to_boolean` — the false set is
/// `[false, 0, "0", "f", "F", "false", "FALSE", "off", "OFF", ""]`.
fn flexible_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum BoolValue {
        Bool(bool),
        Int(i64),
        Str(String),
    }

    let value = Option::<BoolValue>::deserialize(deserializer)?;
    Ok(match value {
        None => None,
        Some(BoolValue::Bool(value)) => Some(value),
        Some(BoolValue::Int(value)) => Some(value != 0),
        Some(BoolValue::Str(value)) => {
            let lowered = value.trim().to_ascii_lowercase();
            Some(!matches!(
                lowered.as_str(),
                "" | "0" | "f" | "false" | "off"
            ))
        }
    })
}

/// Body of `POST /accounts/{account_id}/lti_registrations/{registration_id}/controls`.
#[derive(Debug, Default, Deserialize)]
pub struct ControlParams {
    #[serde(default, deserialize_with = "flexible_id")]
    pub account_id: Option<i64>,
    #[serde(default, deserialize_with = "flexible_id")]
    pub course_id: Option<i64>,
    #[serde(default, deserialize_with = "flexible_id")]
    pub deployment_id: Option<i64>,
    #[serde(default, deserialize_with = "flexible_bool")]
    pub available: Option<bool>,
    #[serde(default)]
    pub comment: Option<String>,
}

/// One entry of the bulk body.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BulkControlItem {
    #[serde(default, deserialize_with = "flexible_id")]
    pub account_id: Option<i64>,
    #[serde(default, deserialize_with = "flexible_id")]
    pub course_id: Option<i64>,
    #[serde(default, deserialize_with = "flexible_id")]
    pub deployment_id: Option<i64>,
    #[serde(default, deserialize_with = "flexible_bool")]
    pub available: Option<bool>,
}

/// The bulk body is a JSON array; Rails also accepts it wrapped in `_json`.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum BulkControlBody {
    List(Vec<BulkControlItem>),
    Wrapped {
        #[serde(rename = "_json")]
        items: Vec<BulkControlItem>,
    },
}

impl BulkControlBody {
    fn items(&self) -> &[BulkControlItem] {
        match self {
            BulkControlBody::List(items) => items,
            BulkControlBody::Wrapped { items } => items,
        }
    }
}

/// Body of `PUT /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}`.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateControlParams {
    #[serde(default, deserialize_with = "flexible_bool")]
    pub available: Option<bool>,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Query params of the list endpoint.
#[derive(Debug, Default, Deserialize)]
pub struct ListControlsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl ListControlsQuery {
    /// Canvas `Api.paginate` — page 1, `per_page` capped at 100.
    fn resolve(&self) -> (i64, i64, i64) {
        let page = self.page.unwrap_or(1).max(1);
        let per_page = self
            .per_page
            .unwrap_or(CONTROLS_DEFAULT_LIST_PAGE_SIZE)
            .clamp(1, CONTROLS_MAX_LIST_PAGE_SIZE);
        let offset = page.saturating_sub(1).saturating_mul(per_page);
        (page, per_page, offset)
    }
}

/// The endpoints without a request body take their `comment` from the query
/// string.
#[derive(Debug, Default, Deserialize)]
pub struct CommentQuery {
    pub comment: Option<String>,
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

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

/// Canvas `root_account.feature_enabled?(:lti_registrations_templates)` — when
/// it is on, the controls point at the local copy of the registration
/// (`app_id`) instead of the cross-shard one.
async fn templates_enabled(pool: &PgPool, root_account_id: i64) -> Result<bool, ApiError> {
    feature_enabled(pool, root_account_id, "lti_registrations_templates").await
}

/// Canvas' `before_action` chain, in order: `require_account_context`,
/// `require_root_account`, `require_feature_flag` and
/// `require_manage_lti_registrations`. A missing account is a 404 before the
/// permission check can turn into a 403.
async fn authorize(pool: &PgPool, auth: &AuthUser, account_id: i64) -> Result<Ctx, Response> {
    let account: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(pool)
            .await
            .map_err(|error| ApiError::from(error).into_response())?;

    // `require_account_context`
    let Some((parent_account_id,)) = account else {
        return Err(ApiError::NotFound.into_response());
    };
    // `require_root_account` — a sub-account is a 404 here.
    if parent_account_id.is_some() {
        return Err(ApiError::NotFound.into_response());
    }
    // `require_feature_flag`
    if !feature_enabled(pool, account_id, "lti_registrations_next")
        .await
        .map_err(|error| ApiError::from(error).into_response())?
    {
        return Err(feature_flag_not_found());
    }
    // `require_manage_lti_registrations`
    if !can_manage_account(pool, auth, account_id)
        .await
        .map_err(|error| ApiError::from(error).into_response())?
    {
        return Err(ApiError::Forbidden.into_response());
    }

    Ok(Ctx { account_id })
}

/// Canvas `require_feature_flag` — `404 { "error": "..." }`.
fn feature_flag_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "The specified resource does not exist." })),
    )
        .into_response()
}

/// Canvas `render_errors(errors, status: :unprocessable_entity)` with a list.
fn render_errors(messages: Vec<String>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "errors": messages })),
    )
        .into_response()
}

/// Canvas `render_errors("message")` — a single message is a bare string.
fn render_error_message(message: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "errors": message })),
    )
        .into_response()
}

/// Canvas `api_error_json` for `ActionController::ParameterMissing`.
fn parameter_missing(param: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "errors": [{ "message": format!("{param} is missing") }] })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

/// Canvas `ContextExternalTool.find(id)` — a missing deployment is a 404.
async fn find_deployment(pool: &PgPool, deployment_id: i64) -> Result<DeploymentRow, ApiError> {
    let sql = format!("SELECT {DEPLOYMENT_COLUMNS} FROM lti_deployments WHERE id = $1");
    let row: Option<DeploymentRow> = sqlx::query_as(&sql)
        .bind(deployment_id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// Canvas `root_account_deployment` — the active deployment of the
/// registration in the root account.
async fn root_account_deployment(
    pool: &PgPool,
    registration: &RegistrationRow,
    ctx: &Ctx,
) -> Result<Option<DeploymentRow>, ApiError> {
    let sql = format!(
        "SELECT {DEPLOYMENT_COLUMNS} FROM lti_deployments \
         WHERE registration_id = $1 AND context_type = 'Account' AND context_id = $2 \
           AND workflow_state <> '{STATE_DELETED}' AND workflow_state <> '{STATE_DISABLED}' \
         ORDER BY id ASC LIMIT 1"
    );
    let row: Option<DeploymentRow> = sqlx::query_as(&sql)
        .bind(registration.id)
        .bind(ctx.account_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// Canvas `Lti::ContextControl.active.for_registration(registration, @account)
/// .find_by(id: params[:id], root_account_id: @account.id)`.
async fn find_control(
    pool: &PgPool,
    registration: &RegistrationRow,
    ctx: &Ctx,
    templates: bool,
    id: i64,
) -> Result<ContextControlRow, ApiError> {
    let registration_column = if templates {
        "app_id"
    } else {
        "registration_id"
    };
    let sql = format!(
        "SELECT {CONTROL_COLUMNS} FROM lti_context_controls \
         WHERE id = $1 AND root_account_id = $2 AND {registration_column} = $3 \
           AND workflow_state <> '{STATE_DELETED}' \
           AND workflow_state <> '{STATE_DELETED_WITH_CONTEXT}'"
    );
    let row: Option<ContextControlRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(ctx.account_id)
        .bind(registration.id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// Canvas `Lti::ContextControl.find_or_initialize_by(unique_checks)` — the
/// unique check attributes are `account_id`, `course_id`, `deployment_id` and
/// `registration_id`, compacted.
async fn find_control_by_unique(
    pool: &PgPool,
    account_id: Option<i64>,
    course_id: Option<i64>,
    deployment_id: i64,
    registration_id: i64,
) -> Result<Option<ContextControlRow>, ApiError> {
    let (column, value) = match account_id {
        Some(account_id) => ("account_id", account_id),
        None => ("course_id", course_id.unwrap_or_default()),
    };
    let sql = format!(
        "SELECT {CONTROL_COLUMNS} FROM lti_context_controls \
         WHERE {column} = $1 AND deployment_id = $2 AND registration_id = $3 LIMIT 1"
    );
    let row: Option<ContextControlRow> = sqlx::query_as(&sql)
        .bind(value)
        .bind(deployment_id)
        .bind(registration_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// Canvas `Lti::ContextControl.for_registration(registration, @account).active
/// .exists?(unique_checks)` — the "already exists" guard of the create
/// endpoint.
async fn control_exists_for_registration(
    pool: &PgPool,
    registration: &RegistrationRow,
    templates: bool,
    account_id: Option<i64>,
    course_id: Option<i64>,
    deployment_id: i64,
) -> Result<bool, ApiError> {
    let registration_column = if templates {
        "app_id"
    } else {
        "registration_id"
    };
    let (context_column, context_value) = match account_id {
        Some(account_id) => ("account_id", account_id),
        None => ("course_id", course_id.unwrap_or_default()),
    };
    let sql = format!(
        "SELECT 1 FROM lti_context_controls \
         WHERE {registration_column} = $1 \
           AND {context_column} = $2 AND deployment_id = $3 \
           AND workflow_state <> '{STATE_DELETED}' \
           AND workflow_state <> '{STATE_DELETED_WITH_CONTEXT}' LIMIT 1"
    );
    let row: Option<(i32,)> = sqlx::query_as(&sql)
        .bind(registration.id)
        .bind(context_value)
        .bind(deployment_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Canvas `validates :deployment_id, uniqueness: { scope: %i[account_id course_id] }`.
async fn control_exists_for_deployment(
    pool: &PgPool,
    account_id: Option<i64>,
    course_id: Option<i64>,
    deployment_id: i64,
) -> Result<bool, ApiError> {
    let (column, value) = match account_id {
        Some(account_id) => ("account_id", account_id),
        None => ("course_id", course_id.unwrap_or_default()),
    };
    let sql = format!(
        "SELECT 1 FROM lti_context_controls \
         WHERE {column} = $1 AND deployment_id = $2 LIMIT 1"
    );
    let row: Option<(i32,)> = sqlx::query_as(&sql)
        .bind(value)
        .bind(deployment_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Canvas `Lti::Registration#local_copy_for(account)` — the local copy of a
/// cross-shard registration, or the registration itself.
async fn local_copy_for(
    pool: &PgPool,
    registration: &RegistrationRow,
    account_id: i64,
) -> Result<Option<i64>, ApiError> {
    if registration.account_id == account_id {
        return Ok(Some(registration.id));
    }
    let sql = "SELECT id FROM lti_registrations \
               WHERE template_registration_id = $1 AND account_id = $2 \
                 AND workflow_state <> 'deleted' LIMIT 1";
    let row: Option<(i64,)> = sqlx::query_as(sql)
        .bind(registration.id)
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(id,)| id))
}

/// Canvas `Account.find(ids)` — a missing row is a 404.
async fn find_accounts(pool: &PgPool, ids: &[i64]) -> Result<HashMap<i64, i64>, ApiError> {
    let mut accounts = HashMap::new();
    for id in ids {
        let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        let Some((id,)) = row else {
            return Err(ApiError::NotFound);
        };
        let root_account_id = root_account_of_account(pool, id).await?;
        accounts.insert(id, root_account_id);
    }
    Ok(accounts)
}

/// Canvas `Course.find(ids)` — a missing row is a 404.
async fn find_courses(pool: &PgPool, ids: &[i64]) -> Result<HashMap<i64, CourseInfo>, ApiError> {
    let mut courses = HashMap::new();
    for id in ids {
        let row: Option<(i64, String, Option<i64>)> =
            sqlx::query_as("SELECT id, name, account_id FROM courses WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let Some((id, name, account_id)) = row else {
            return Err(ApiError::NotFound);
        };
        courses.insert(id, CourseInfo { name, account_id });
    }
    Ok(courses)
}

/// The courses that do exist — Canvas' `Course.where(id: ...)` never raises.
async fn course_rows(pool: &PgPool, ids: &[i64]) -> Result<HashMap<i64, CourseInfo>, ApiError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(i64, String, Option<i64>)> =
        sqlx::query_as("SELECT id, name, account_id FROM courses WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, account_id)| (id, CourseInfo { name, account_id }))
        .collect())
}

// ---------------------------------------------------------------------------
// Account hierarchy
// ---------------------------------------------------------------------------

/// Walk up to the root of the account hierarchy (cycles are bounded).
async fn root_account_of_account(pool: &PgPool, account_id: i64) -> Result<i64, ApiError> {
    let mut current = account_id;
    let mut depth: i64 = 0;
    while depth < crate::MAX_ACCOUNT_TREE_DEPTH {
        depth += 1;
        let row: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(current)
                .fetch_optional(pool)
                .await?;
        match row {
            Some((Some(parent),)) => current = parent,
            _ => break,
        }
    }
    Ok(current)
}

/// Canvas `Account.account_chain_ids` — the account and its ancestors, leaf
/// first.
async fn account_chain_ids(pool: &PgPool, account_id: i64) -> Result<Vec<i64>, ApiError> {
    let mut chain = Vec::new();
    let mut current = Some(account_id);
    let mut depth: i64 = 0;
    while depth < crate::MAX_ACCOUNT_TREE_DEPTH {
        depth += 1;
        let id = match current {
            Some(id) => id,
            None => break,
        };
        let row: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        let Some((parent,)) = row else {
            break;
        };
        chain.push(id);
        current = parent;
    }
    Ok(chain)
}

/// Canvas `Lti::ContextControl.calculate_path_for_account_ids` — the account
/// chain rendered root first, e.g. `"a1.a2."`.
fn calculate_path_for_account_ids(account_ids: &[i64]) -> String {
    let mut path = String::new();
    for id in account_ids.iter().rev() {
        path.push_str(&format!("a{id}."));
    }
    path
}

/// Canvas `Lti::ContextControl.calculate_path_for_course_id` — e.g.
/// `"a1.a2.c3."`.
fn calculate_path_for_course_id(course_id: i64, account_ids: &[i64]) -> String {
    format!(
        "{}c{}.",
        calculate_path_for_account_ids(account_ids),
        course_id
    )
}

/// Canvas `Account.sub_account_ids_recursive` — the descendants of an
/// account, at any depth.
async fn sub_account_ids_recursive(pool: &PgPool, account_id: i64) -> Result<Vec<i64>, ApiError> {
    let sql = "WITH RECURSIVE t AS ( \
                 SELECT id FROM accounts \
                 WHERE parent_account_id = $1 AND workflow_state <> 'deleted' \
                 UNION \
                 SELECT a.id FROM accounts a INNER JOIN t ON a.parent_account_id = t.id \
                 WHERE a.workflow_state <> 'deleted' \
               ) SELECT id FROM t";
    let rows: Vec<(i64,)> = sqlx::query_as(sql).bind(account_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

// ---------------------------------------------------------------------------
// Calculated attributes
// ---------------------------------------------------------------------------

/// Canvas `Lti::ContextControl.self_and_all_parent_paths` — the path of the
/// context and of each of its ancestors.
fn self_and_all_parent_paths(path: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut current = String::new();
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        current.push_str(segment);
        current.push('.');
        paths.push(current.clone());
    }
    paths
}

/// Canvas `Lti::ContextControlService.preload_calculated_attrs`.
async fn preload_calculated_attrs(
    pool: &PgPool,
    controls: &[ContextControlRow],
) -> Result<HashMap<i64, CalculatedAttrs>, ApiError> {
    let mut attrs: HashMap<i64, CalculatedAttrs> = HashMap::new();
    if controls.is_empty() {
        return Ok(attrs);
    }

    let account_controls: Vec<&ContextControlRow> =
        controls.iter().filter(|c| c.account_id.is_some()).collect();
    let course_controls: Vec<&ContextControlRow> =
        controls.iter().filter(|c| c.course_id.is_some()).collect();

    // --- subaccounts and course counts (account controls only) ---
    let account_ids: Vec<i64> = account_controls
        .iter()
        .filter_map(|c| c.account_id)
        .collect::<HashSet<i64>>()
        .into_iter()
        .collect();
    let mut subaccount_ids: HashMap<i64, Vec<i64>> = HashMap::new();
    for id in &account_ids {
        subaccount_ids.insert(*id, sub_account_ids_recursive(pool, *id).await?);
    }
    let mut all_account_ids: Vec<i64> = account_ids.clone();
    for descendants in subaccount_ids.values() {
        all_account_ids.extend(descendants.iter().copied());
    }
    all_account_ids.sort_unstable();
    all_account_ids.dedup();

    let course_counts: HashMap<i64, i64> = if all_account_ids.is_empty() {
        HashMap::new()
    } else {
        let rows: Vec<(Option<i64>, i64)> = sqlx::query_as(
            "SELECT account_id, COUNT(*) FROM courses \
             WHERE account_id = ANY($1) AND workflow_state <> 'deleted' GROUP BY account_id",
        )
        .bind(&all_account_ids)
        .fetch_all(pool)
        .await?;
        rows.into_iter()
            .filter_map(|(account_id, count)| account_id.map(|account_id| (account_id, count)))
            .collect()
    };

    // --- the paths of every active control of the involved deployments ---
    let deployment_ids: Vec<i64> = controls
        .iter()
        .map(|c| c.deployment_id)
        .collect::<HashSet<i64>>()
        .into_iter()
        .collect();
    let sibling_rows: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT id, deployment_id, path FROM lti_context_controls \
         WHERE deployment_id = ANY($1) \
           AND workflow_state <> '{STATE_DELETED}' \
           AND workflow_state <> '{STATE_DELETED_WITH_CONTEXT}'",
    )
    .bind(&deployment_ids)
    .fetch_all(pool)
    .await?;

    let possible_parent_paths: HashSet<String> = controls
        .iter()
        .flat_map(|c| self_and_all_parent_paths(&c.path))
        .collect();

    // --- names ---
    let course_ids: Vec<i64> = course_controls
        .iter()
        .filter_map(|c| c.course_id)
        .collect::<HashSet<i64>>()
        .into_iter()
        .collect();
    let courses = course_rows(pool, &course_ids).await?;
    let mut name_account_ids: Vec<i64> = account_ids.clone();
    for course in courses.values() {
        if let Some(account_id) = course.account_id {
            name_account_ids.push(account_id);
        }
    }
    for control in controls {
        for segment in control.path.split('.') {
            if let Some(id) = segment
                .strip_prefix('a')
                .and_then(|id| id.parse::<i64>().ok())
            {
                name_account_ids.push(id);
            }
        }
    }
    name_account_ids.sort_unstable();
    name_account_ids.dedup();
    let names = account_names(pool, &name_account_ids).await?;

    for control in controls {
        let context_name = match control.course_id {
            Some(course_id) => courses.get(&course_id).map(|course| course.name.clone()),
            None => control.account_id.and_then(|id| names.get(&id).cloned()),
        };

        let display_path = display_path_for(&control.path, &names);

        let depth = sibling_rows
            .iter()
            .filter(|(_, deployment_id, path)| {
                *deployment_id == control.deployment_id
                    && possible_parent_paths.contains(path)
                    && control.path.starts_with(path.as_str())
                    && *path != control.path
            })
            .count() as i64;

        let (child_control_count, subaccount_count, course_count) = match control.account_id {
            Some(account_id) => {
                let child_control_count = sibling_rows
                    .iter()
                    .filter(|(_, deployment_id, path)| {
                        *deployment_id == control.deployment_id
                            && path.starts_with(&control.path)
                            && *path != control.path
                    })
                    .count() as i64;
                let descendants = subaccount_ids.get(&account_id).cloned().unwrap_or_default();
                let course_count = std::iter::once(&account_id)
                    .chain(descendants.iter())
                    .filter_map(|id| course_counts.get(id).copied())
                    .sum();
                (child_control_count, descendants.len() as i64, course_count)
            }
            None => (0, 0, 0),
        };

        attrs.insert(
            control.id,
            CalculatedAttrs {
                context_name,
                child_control_count,
                subaccount_count,
                course_count,
                display_path,
                depth,
            },
        );
    }

    Ok(attrs)
}

/// Canvas `Lti::ContextControlService.preload_calculated_attrs` — the names
/// of the accounts in the hierarchy, without the root and without the
/// control's own context.
fn display_path_for(path: &str, account_names: &HashMap<i64, String>) -> Vec<String> {
    let segments: Vec<&str> = path.split('.').collect();
    let inner: &[&str] = if segments.len() > 2 {
        &segments[1..segments.len() - 1]
    } else {
        &[]
    };
    inner
        .iter()
        .filter_map(|segment| segment.strip_prefix('a'))
        .filter_map(|id| id.parse::<i64>().ok())
        .filter_map(|id| account_names.get(&id).cloned())
        .collect()
}

/// The names of the given accounts.
async fn account_names(pool: &PgPool, ids: &[i64]) -> Result<HashMap<i64, String>, ApiError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM accounts WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().collect())
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Canvas `user_json` for the `created_by` / `updated_by` fields.
async fn user_summaries(pool: &PgPool, ids: &[i64]) -> Result<HashMap<i64, Value>, ApiError> {
    let mut summaries = HashMap::new();
    if ids.is_empty() {
        return Ok(summaries);
    }
    let rows: Vec<(i64, String, Option<String>)> =
        sqlx::query_as("SELECT id, name, sortable_name FROM users WHERE id = ANY($1)")
            .bind(ids)
            .fetch_all(pool)
            .await?;
    for (id, name, sortable_name) in rows {
        let sortable = sortable_name.unwrap_or_else(|| name.clone());
        summaries.insert(
            id,
            json!({ "id": id, "name": name, "sortable_name": sortable }),
        );
    }
    Ok(summaries)
}

/// The user ids referenced by the `created_by` / `updated_by` fields.
fn control_user_ids(controls: &[ContextControlRow]) -> Vec<i64> {
    controls
        .iter()
        .flat_map(|c| [c.created_by_id, c.updated_by_id])
        .flatten()
        .collect::<HashSet<i64>>()
        .into_iter()
        .collect()
}

/// Canvas `lti_context_control_json`.
fn context_control_json(
    control: &ContextControlRow,
    attrs: &CalculatedAttrs,
    users: &HashMap<i64, Value>,
    templates: bool,
) -> Value {
    json!({
        "id": control.id,
        "account_id": control.account_id,
        "course_id": control.course_id,
        "registration_id": if templates { control.app_id } else { Some(control.registration_id) },
        "deployment_id": control.deployment_id,
        "workflow_state": control.workflow_state,
        "created_at": DateTime::<Utc>::from_naive_utc_and_offset(control.created_at, Utc),
        "updated_at": DateTime::<Utc>::from_naive_utc_and_offset(control.updated_at, Utc),
        "available": control.available,
        "path": control.path,
        "context_name": attrs.context_name,
        "child_control_count": attrs.child_control_count,
        "subaccount_count": attrs.subaccount_count,
        "course_count": attrs.course_count,
        "display_path": attrs.display_path,
        "depth": attrs.depth,
        "created_by": control.created_by_id.and_then(|id| users.get(&id).cloned()),
        "updated_by": control.updated_by_id.and_then(|id| users.get(&id).cloned()),
    })
}

/// Canvas `lti_deployment_json`.
fn deployment_json(
    deployment: &DeploymentRow,
    root_account_deployment: bool,
    context_controls: Option<Vec<Value>>,
) -> Value {
    let workflow_state = if deployment.workflow_state == STATE_DELETED
        || deployment.workflow_state == STATE_DISABLED
    {
        deployment.workflow_state.clone()
    } else {
        STATE_ACTIVE.to_string()
    };
    let mut body = json!({
        "id": deployment.id,
        "context_id": deployment.context_id,
        "context_type": deployment.context_type,
        "registration_id": deployment.registration_id,
        "deployment_id": deployment.deployment_id,
        "context_name": deployment.context_name,
        "workflow_state": workflow_state,
        "root_account_deployment": root_account_deployment,
    });
    if let Some(context_controls) = context_controls {
        body["context_controls"] = Value::Array(context_controls);
    }
    body
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the LTI context controls router (mounted under `/api/v1` by
/// star-http).
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/controls",
            get(index).post(create),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/controls/bulk",
            post(create_many),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}",
            get(show).put(update).delete(delete),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /accounts/{account_id}/lti_registrations/{registration_id}/controls` —
/// the active controls of the registration, grouped by deployment.
async fn index(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    Query(params): Query<ListControlsQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = match authorize(&state.db, &auth, account_id).await {
        Ok(ctx) => ctx,
        Err(response) => return Ok(response),
    };
    let registration = find_active_registration(&state.db, registration_id).await?;
    let templates = templates_enabled(&state.db, ctx.account_id).await?;

    let registration_column = if templates {
        "c.app_id"
    } else {
        "c.registration_id"
    };
    let sql = format!(
        "SELECT {CONTROL_COLUMNS_QUALIFIED} FROM lti_context_controls c \
         JOIN lti_deployments d ON d.id = c.deployment_id \
         WHERE {registration_column} = $1 AND c.root_account_id = $2 \
           AND c.workflow_state <> '{STATE_DELETED}' \
           AND c.workflow_state <> '{STATE_DELETED_WITH_CONTEXT}' \
           AND d.workflow_state <> '{STATE_DELETED}' \
         ORDER BY CASE \
             WHEN d.context_type = 'Account' AND d.context_id = c.root_account_id THEN 0 \
             WHEN d.context_type = 'Account' THEN 1 \
             ELSE 2 \
         END, c.deployment_id, REPLACE(REPLACE(c.path, 'a', '1'), 'c', '0'), c.path \
         LIMIT $3 OFFSET $4"
    );
    let (_, per_page, offset) = params.resolve();
    let controls: Vec<ContextControlRow> = sqlx::query_as(&sql)
        .bind(registration.id)
        .bind(ctx.account_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    let attrs = preload_calculated_attrs(&state.db, &controls).await?;
    let users = user_summaries(&state.db, &control_user_ids(&controls)).await?;

    // Canvas groups the paginated controls by their deployment, keeping the
    // order in which the deployments first appear.
    let mut groups: Vec<(i64, Vec<&ContextControlRow>)> = Vec::new();
    for control in &controls {
        let position = groups
            .iter()
            .position(|(deployment_id, _)| *deployment_id == control.deployment_id);
        match position {
            Some(position) => groups[position].1.push(control),
            None => groups.push((control.deployment_id, vec![control])),
        }
    }

    let mut body = Vec::with_capacity(groups.len());
    for (deployment_id, members) in groups {
        let deployment = find_deployment(&state.db, deployment_id).await?;
        let root = deployment.context_type == "Account"
            && is_root_account(&state.db, deployment.context_id).await?;
        let mut control_json = Vec::with_capacity(members.len());
        for member in members {
            let empty = CalculatedAttrs::default();
            let attrs = attrs.get(&member.id).unwrap_or(&empty);
            control_json.push(context_control_json(member, attrs, &users, templates));
        }
        body.push(deployment_json(&deployment, root, Some(control_json)));
    }

    Ok(Json(body).into_response())
}

/// `GET /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}`.
async fn show(
    State(state): State<AppState>,
    Path((account_id, registration_id, id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = match authorize(&state.db, &auth, account_id).await {
        Ok(ctx) => ctx,
        Err(response) => return Ok(response),
    };
    let registration = find_active_registration(&state.db, registration_id).await?;
    let templates = templates_enabled(&state.db, ctx.account_id).await?;
    let control = find_control(&state.db, &registration, &ctx, templates, id).await?;

    let attrs = preload_calculated_attrs(&state.db, std::slice::from_ref(&control)).await?;
    let empty = CalculatedAttrs::default();
    let attrs = attrs.get(&control.id).unwrap_or(&empty);
    let users =
        user_summaries(&state.db, &control_user_ids(std::slice::from_ref(&control))).await?;

    Ok(Json(context_control_json(&control, attrs, &users, templates)).into_response())
}

/// `POST /accounts/{account_id}/lti_registrations/{registration_id}/controls` —
/// create one control.
async fn create(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<ControlParams>,
) -> Result<Response, ApiError> {
    let ctx = match authorize(&state.db, &auth, account_id).await {
        Ok(ctx) => ctx,
        Err(response) => return Ok(response),
    };
    let registration = find_active_registration(&state.db, registration_id).await?;
    let templates = templates_enabled(&state.db, ctx.account_id).await?;

    // Canvas `params_for_control`
    let deployment_id = match payload.deployment_id {
        Some(deployment_id) => Some(deployment_id),
        None => root_account_deployment(&state.db, &registration, &ctx)
            .await?
            .map(|deployment| deployment.id),
    };
    let available = payload.available.unwrap_or(true);

    let Some(deployment_id) = deployment_id else {
        return Ok(render_error_message(
            "No active deployment found for the root account.",
        ));
    };
    if payload.account_id.is_none() && payload.course_id.is_none() {
        return Ok(render_error_message(
            "Either account_id or course_id must be present.",
        ));
    }

    if control_exists_for_registration(
        &state.db,
        &registration,
        templates,
        payload.account_id,
        payload.course_id,
        deployment_id,
    )
    .await?
    {
        return Ok(render_error_message(
            "A context control for this deployment and context already exists.",
        ));
    }

    // Canvas `ContextExternalTool.find(deployment_id)` — a missing deployment
    // is a 404.
    let deployment = find_deployment(&state.db, deployment_id).await?;

    // The context of the control has to exist.
    let Some(context_account_id) =
        resolve_context_account_id(&state.db, payload.account_id, payload.course_id).await?
    else {
        return Ok(render_errors(vec!["Context must exist".to_string()]));
    };

    let app_id = local_copy_for(&state.db, &registration, ctx.account_id).await?;
    let path = control_path(&state.db, payload.account_id, payload.course_id).await?;
    let root_account_id = root_account_of_account(&state.db, context_account_id).await?;
    let draft = ControlDraft {
        account_id: payload.account_id,
        course_id: payload.course_id,
        deployment_id,
        available,
        path,
        registration_id: registration.id,
        app_id,
        root_account_id,
        created_by_id: Some(auth.id),
        updated_by_id: Some(auth.id),
    };

    let existing = find_control_by_unique(
        &state.db,
        payload.account_id,
        payload.course_id,
        deployment_id,
        registration.id,
    )
    .await?;

    let old_controls = snapshot_controls(&state.db, &[draft.clone()]).await?;

    let control = match existing {
        // Canvas `Lti::ContextControlService.restore_deleted_control`.
        Some(control) => restore_control(&state.db, control.id, available, auth.id).await?,
        None => {
            if control_exists_for_deployment(
                &state.db,
                payload.account_id,
                payload.course_id,
                deployment_id,
            )
            .await?
            {
                return Ok(render_errors(vec![
                    "Deployment has already been taken".to_string()
                ]));
            }
            if let Some(response) =
                validate_context_belongs_to_deployment(&state.db, &deployment, context_account_id)
                    .await?
            {
                return Ok(response);
            }

            let control_id = insert_control(&state.db, &draft).await?;
            find_control_by_id(&state.db, control_id).await?
        }
    };

    // Canvas `Lti::ContextControlService.build_anchor_control`.
    let mut written = vec![draft.clone()];
    if let Some(anchor) = build_anchor_control(&state.db, &draft).await? {
        insert_control(&state.db, &anchor).await?;
        written.push(anchor);
    }

    let new_controls = snapshot_controls(&state.db, &written).await?;
    write_history_entry(
        &state.db,
        &registration,
        &old_controls,
        &new_controls,
        ctx.account_id,
        auth.id,
        payload.comment.as_deref(),
    )
    .await?;

    let attrs = preload_calculated_attrs(&state.db, std::slice::from_ref(&control)).await?;
    let empty = CalculatedAttrs::default();
    let attrs = attrs.get(&control.id).unwrap_or(&empty);
    let users =
        user_summaries(&state.db, &control_user_ids(std::slice::from_ref(&control))).await?;

    Ok((
        StatusCode::CREATED,
        Json(context_control_json(&control, attrs, &users, templates)),
    )
        .into_response())
}

/// `POST /accounts/{account_id}/lti_registrations/{registration_id}/controls/bulk`
/// — create up to 100 controls at once.
async fn create_many(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    Query(query): Query<CommentQuery>,
    auth: AuthUser,
    Json(payload): Json<BulkControlBody>,
) -> Result<Response, ApiError> {
    let ctx = match authorize(&state.db, &auth, account_id).await {
        Ok(ctx) => ctx,
        Err(response) => return Ok(response),
    };
    let registration = find_active_registration(&state.db, registration_id).await?;
    let templates = templates_enabled(&state.db, ctx.account_id).await?;
    let items = payload.items().to_vec();

    // Canvas `validate_bulk_params`
    if items.is_empty() {
        return Ok(render_error_message(
            "Invalid parameters. Expected an array of context control parameters.",
        ));
    }
    if items.len() > MAX_BULK_CREATE {
        return Ok(render_error_message(&format!(
            "Cannot create more than {MAX_BULK_CREATE} context controls at once"
        )));
    }
    if items
        .iter()
        .any(|item| item.account_id.is_none() && item.course_id.is_none())
    {
        return Ok(render_error_message(
            "Either account_id or course_id must be present for each context control.",
        ));
    }
    if items
        .iter()
        .any(|item| item.account_id.is_some() && item.course_id.is_some())
    {
        return Ok(render_error_message(
            "Either account_id or course_id must be present for each context control, but not both.",
        ));
    }
    let root_deployment = root_account_deployment(&state.db, &registration, &ctx).await?;
    if items.iter().any(|item| item.deployment_id.is_none()) && root_deployment.is_none() {
        return Ok(render_error_message(
            "No active deployment found for the root account. Please specify a deployment_id for each control.",
        ));
    }
    let mut contexts: Vec<(Option<i64>, Option<i64>)> = items
        .iter()
        .map(|item| (item.account_id, item.course_id))
        .collect();
    contexts.sort_unstable();
    contexts.dedup();
    if contexts.len() != items.len() {
        return Ok(render_error_message(
            "Cannot create multiple context controls for the same context. Please specify unique account_id or course_id for each context control.",
        ));
    }

    // Canvas `Account.find(...)` / `Course.find(...)`
    let account_ids: Vec<i64> = items.iter().filter_map(|item| item.account_id).collect();
    let course_ids: Vec<i64> = items.iter().filter_map(|item| item.course_id).collect();
    let accounts = find_accounts(&state.db, &account_ids).await?;
    let courses = find_courses(&state.db, &course_ids).await?;

    // The account chains of every context.
    let mut chain_account_ids: Vec<i64> = account_ids.clone();
    for course in courses.values() {
        if let Some(account_id) = course.account_id {
            chain_account_ids.push(account_id);
        }
    }
    chain_account_ids.sort_unstable();
    chain_account_ids.dedup();
    let mut chains: HashMap<i64, Vec<i64>> = HashMap::new();
    for id in chain_account_ids {
        chains.insert(id, account_chain_ids(&state.db, id).await?);
    }

    let app_id = local_copy_for(&state.db, &registration, ctx.account_id).await?;

    let mut drafts: Vec<ControlDraft> = Vec::with_capacity(items.len());
    for item in &items {
        let deployment_id = match item.deployment_id {
            Some(deployment_id) => deployment_id,
            None => root_deployment
                .as_ref()
                .map(|deployment| deployment.id)
                .unwrap_or(0),
        };
        let (account_id, course_id) = (item.account_id, item.course_id);
        let (root_account_id, path) = match account_id {
            Some(account_id) => {
                let root_account_id = accounts.get(&account_id).copied().unwrap_or(ctx.account_id);
                let path = calculate_path_for_account_ids(
                    chains.get(&account_id).map(Vec::as_slice).unwrap_or(&[]),
                );
                (root_account_id, path)
            }
            None => {
                let course_id = course_id.unwrap_or_default();
                let chain = courses
                    .get(&course_id)
                    .and_then(|course| course.account_id)
                    .and_then(|account_id| chains.get(&account_id))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let root_account_id = chain.last().copied().unwrap_or(ctx.account_id);
                let path = calculate_path_for_course_id(course_id, chain);
                (root_account_id, path)
            }
        };
        drafts.push(ControlDraft {
            account_id,
            course_id,
            deployment_id,
            available: item.available.unwrap_or(true),
            path,
            registration_id: registration.id,
            app_id,
            root_account_id,
            created_by_id: Some(auth.id),
            updated_by_id: Some(auth.id),
        });
    }

    // Canvas checks that every context belongs to the deployment's context.
    let deployment_ids: Vec<i64> = drafts
        .iter()
        .map(|draft| draft.deployment_id)
        .collect::<HashSet<i64>>()
        .into_iter()
        .collect();
    let mut deployment_course_ids: HashMap<i64, i64> = HashMap::new();
    let mut deployment_account_ids: HashMap<i64, i64> = HashMap::new();
    // Canvas reads the deployments with `ContextExternalTool.where(id: ...)`:
    // a deployment that does not exist is not an error here, it only makes
    // the controls that use it invalid.
    let deployment_sql =
        format!("SELECT {DEPLOYMENT_COLUMNS} FROM lti_deployments WHERE id = ANY($1)");
    let deployments: Vec<DeploymentRow> = sqlx::query_as(&deployment_sql)
        .bind(&deployment_ids)
        .fetch_all(&state.db)
        .await?;
    for deployment in deployments {
        if deployment.context_type == "Course" {
            deployment_course_ids.insert(deployment.id, deployment.context_id);
        } else if deployment.context_type == "Account" {
            deployment_account_ids.insert(deployment.id, deployment.context_id);
        }
    }

    let mut invalid_controls: Vec<Value> = Vec::new();
    for draft in &drafts {
        let valid = if let Some(course_id) = deployment_course_ids.get(&draft.deployment_id) {
            Some(*course_id) == draft.course_id
        } else {
            let account_id = draft.account_id.or_else(|| {
                draft.course_id.and_then(|course_id| {
                    courses.get(&course_id).and_then(|course| course.account_id)
                })
            });
            match (
                account_id,
                deployment_account_ids.get(&draft.deployment_id).copied(),
            ) {
                (Some(account_id), Some(deployment_account_id)) => {
                    if deployment_account_id == account_id {
                        true
                    } else {
                        sub_account_ids_recursive(&state.db, deployment_account_id)
                            .await?
                            .contains(&account_id)
                    }
                }
                _ => false,
            }
        };
        if !valid {
            let mut error = json!({ "message": "Context must belong to the deployment's context" });
            if let Some(account_id) = draft.account_id {
                error["account_id"] = json!(account_id);
            }
            if let Some(course_id) = draft.course_id {
                error["course_id"] = json!(course_id);
            }
            error["deployment_id"] = json!(draft.deployment_id);
            invalid_controls.push(error);
        }
    }
    if !invalid_controls.is_empty() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "errors": invalid_controls })),
        )
            .into_response());
    }

    // Canvas `Lti::ContextControlService.build_anchor_controls`.
    let anchor_context = AnchorContext {
        chains: chains.clone(),
        course_account_ids: courses
            .iter()
            .filter_map(|(course_id, course)| {
                course.account_id.map(|account_id| (*course_id, account_id))
            })
            .collect(),
        deployment_account_ids: deployment_account_ids.clone(),
        deployment_course_ids: deployment_course_ids.clone(),
    };
    let anchors =
        build_anchor_controls(&state.db, &drafts, &anchor_context, &deployment_ids).await?;
    drafts.extend(anchors);

    let old_controls = snapshot_controls(&state.db, &drafts).await?;

    let ids = upsert_controls(&state.db, &drafts).await?;

    let mut controls: Vec<ContextControlRow> = Vec::with_capacity(ids.len());
    for id in &ids {
        controls.push(find_control_by_id(&state.db, *id).await?);
    }
    controls.sort_by_key(|control| control.id);

    let new_controls = snapshot_controls(&state.db, &drafts).await?;
    write_history_entry(
        &state.db,
        &registration,
        &old_controls,
        &new_controls,
        ctx.account_id,
        auth.id,
        query.comment.as_deref(),
    )
    .await?;

    let attrs = preload_calculated_attrs(&state.db, &controls).await?;
    let users = user_summaries(&state.db, &control_user_ids(&controls)).await?;

    let empty = CalculatedAttrs::default();
    let body: Vec<Value> = controls
        .iter()
        .map(|control| {
            let attrs = attrs.get(&control.id).unwrap_or(&empty);
            context_control_json(control, attrs, &users, templates)
        })
        .collect();

    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// `PUT /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}`
/// — change the availability of a control.
async fn update(
    State(state): State<AppState>,
    Path((account_id, registration_id, id)): Path<(i64, i64, i64)>,
    Query(query): Query<CommentQuery>,
    auth: AuthUser,
    Json(payload): Json<UpdateControlParams>,
) -> Result<Response, ApiError> {
    let ctx = match authorize(&state.db, &auth, account_id).await {
        Ok(ctx) => ctx,
        Err(response) => return Ok(response),
    };

    // Canvas `params.require(:available)`.
    let Some(available) = payload.available else {
        return Ok(parameter_missing("available"));
    };

    let registration = find_active_registration(&state.db, registration_id).await?;
    let templates = templates_enabled(&state.db, ctx.account_id).await?;

    let control = find_control(&state.db, &registration, &ctx, templates, id).await?;
    let updated = update_control_available(&state.db, control.id, available).await?;

    // Canvas `Lti::RegistrationHistoryEntry.track_control_changes`.
    let old_controls = tracked_controls(std::slice::from_ref(&control));
    let new_controls = tracked_controls(std::slice::from_ref(&updated));
    write_history_entry(
        &state.db,
        &registration,
        &old_controls,
        &new_controls,
        ctx.account_id,
        auth.id,
        payload.comment.as_deref().or(query.comment.as_deref()),
    )
    .await?;

    let attrs = preload_calculated_attrs(&state.db, std::slice::from_ref(&updated)).await?;
    let empty = CalculatedAttrs::default();
    let attrs = attrs.get(&updated.id).unwrap_or(&empty);
    let users =
        user_summaries(&state.db, &control_user_ids(std::slice::from_ref(&updated))).await?;

    Ok(Json(context_control_json(&updated, attrs, &users, templates)).into_response())
}

/// `DELETE /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}`
/// — delete a control.
async fn delete(
    State(state): State<AppState>,
    Path((account_id, registration_id, id)): Path<(i64, i64, i64)>,
    Query(query): Query<CommentQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let ctx = match authorize(&state.db, &auth, account_id).await {
        Ok(ctx) => ctx,
        Err(response) => return Ok(response),
    };
    let registration = find_active_registration(&state.db, registration_id).await?;
    let templates = templates_enabled(&state.db, ctx.account_id).await?;

    let control = find_control(&state.db, &registration, &ctx, templates, id).await?;
    let deployment = find_deployment(&state.db, control.deployment_id).await?;

    // Canvas `prevent_primary_deletion` — the control of the context the
    // deployment lives in may not be deleted.
    let context_id = control.account_id.or(control.course_id);
    let context_type = if control.account_id.is_some() {
        "Account"
    } else {
        "Course"
    };
    if Some(deployment.context_id) == context_id && deployment.context_type == context_type {
        return Ok(render_errors(vec![
            "Cannot delete primary control for deployment".to_string(),
        ]));
    }

    let deleted = soft_delete_control(&state.db, control.id).await?;

    // Canvas `soft_delete_child_controls`.
    soft_delete_child_controls(&state.db, &control, auth.id).await?;

    let old_controls = tracked_controls(std::slice::from_ref(&control));
    let new_controls = tracked_controls(std::slice::from_ref(&deleted));
    write_history_entry(
        &state.db,
        &registration,
        &old_controls,
        &new_controls,
        ctx.account_id,
        auth.id,
        query.comment.as_deref(),
    )
    .await?;

    let attrs = preload_calculated_attrs(&state.db, std::slice::from_ref(&deleted)).await?;
    let empty = CalculatedAttrs::default();
    let attrs = attrs.get(&deleted.id).unwrap_or(&empty);
    let users =
        user_summaries(&state.db, &control_user_ids(std::slice::from_ref(&deleted))).await?;

    Ok(Json(context_control_json(&deleted, attrs, &users, templates)).into_response())
}

// ---------------------------------------------------------------------------
// Control persistence
// ---------------------------------------------------------------------------

/// Canvas `Lti::ContextControl.find(id)`.
async fn find_control_by_id(pool: &PgPool, id: i64) -> Result<ContextControlRow, ApiError> {
    let sql = format!("SELECT {CONTROL_COLUMNS} FROM lti_context_controls WHERE id = $1");
    let row: Option<ContextControlRow> = sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
    row.ok_or(ApiError::NotFound)
}

/// Canvas `Lti::ContextControl.create!`.
async fn insert_control(pool: &PgPool, draft: &ControlDraft) -> Result<i64, ApiError> {
    let sql = format!(
        "INSERT INTO lti_context_controls \
         (account_id, course_id, registration_id, app_id, deployment_id, available, path, \
          workflow_state, created_by_id, updated_by_id, root_account_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, '{STATE_ACTIVE}', $8, $9, $10, \
                 timezone('UTC', now()), timezone('UTC', now())) \
         RETURNING id"
    );
    let id: i64 = sqlx::query_scalar(&sql)
        .bind(draft.account_id)
        .bind(draft.course_id)
        .bind(draft.registration_id)
        .bind(draft.app_id)
        .bind(draft.deployment_id)
        .bind(draft.available)
        .bind(&draft.path)
        .bind(draft.created_by_id)
        .bind(draft.updated_by_id)
        .bind(draft.root_account_id)
        .fetch_one(pool)
        .await?;
    Ok(id)
}

/// Canvas `Lti::ContextControlService.restore_deleted_control`.
async fn restore_control(
    pool: &PgPool,
    id: i64,
    available: bool,
    user_id: i64,
) -> Result<ContextControlRow, ApiError> {
    let sql = format!(
        "UPDATE lti_context_controls \
         SET available = $2, workflow_state = '{STATE_ACTIVE}', updated_by_id = $3, \
             updated_at = timezone('UTC', now()) \
         WHERE id = $1 RETURNING {CONTROL_COLUMNS}"
    );
    let row: Option<ContextControlRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(available)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// Canvas `control.update!(available:)`.
async fn update_control_available(
    pool: &PgPool,
    id: i64,
    available: bool,
) -> Result<ContextControlRow, ApiError> {
    let sql = format!(
        "UPDATE lti_context_controls \
         SET available = $2, updated_at = timezone('UTC', now()) \
         WHERE id = $1 RETURNING {CONTROL_COLUMNS}"
    );
    let row: Option<ContextControlRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(available)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// Canvas `Canvas::SoftDeletable#destroy` — the control is soft deleted.
async fn soft_delete_control(pool: &PgPool, id: i64) -> Result<ContextControlRow, ApiError> {
    let sql = format!(
        "UPDATE lti_context_controls \
         SET workflow_state = '{STATE_DELETED}', updated_at = timezone('UTC', now()) \
         WHERE id = $1 RETURNING {CONTROL_COLUMNS}"
    );
    let row: Option<ContextControlRow> = sqlx::query_as(&sql).bind(id).fetch_optional(pool).await?;
    row.ok_or(ApiError::NotFound)
}

/// Canvas `soft_delete_child_controls`.
async fn soft_delete_child_controls(
    pool: &PgPool,
    control: &ContextControlRow,
    user_id: i64,
) -> Result<(), ApiError> {
    let sql = format!(
        "UPDATE lti_context_controls \
         SET workflow_state = '{STATE_DELETED}', updated_at = timezone('UTC', now()), \
             updated_by_id = $4 \
         WHERE deployment_id = $1 AND registration_id = $2 AND id <> $3 \
           AND path LIKE $5 \
           AND workflow_state <> '{STATE_DELETED}' \
           AND workflow_state <> '{STATE_DELETED_WITH_CONTEXT}'"
    );
    sqlx::query(&sql)
        .bind(control.deployment_id)
        .bind(control.registration_id)
        .bind(control.id)
        .bind(user_id)
        .bind(format!("{}%", control.path))
        .execute(pool)
        .await?;
    Ok(())
}

/// Canvas `Lti::ContextControl.upsert_all` — the two upserts are split
/// because Postgres can only name one unique index per conflict target.
async fn upsert_controls(pool: &PgPool, drafts: &[ControlDraft]) -> Result<Vec<i64>, ApiError> {
    let mut ids = Vec::with_capacity(drafts.len());

    for draft in drafts.iter().filter(|draft| draft.course_id.is_some()) {
        let sql = format!(
            "INSERT INTO lti_context_controls \
             (account_id, course_id, registration_id, app_id, deployment_id, available, path, \
              workflow_state, created_by_id, updated_by_id, root_account_id, created_at, updated_at) \
             VALUES (NULL, $1, $2, $3, $4, $5, $6, '{STATE_ACTIVE}', $7, $8, $9, \
                     timezone('UTC', now()), timezone('UTC', now())) \
             ON CONFLICT (course_id, deployment_id) WHERE course_id IS NOT NULL \
             DO UPDATE SET account_id = EXCLUDED.account_id, \
                 registration_id = EXCLUDED.registration_id, app_id = EXCLUDED.app_id, \
                 available = EXCLUDED.available, path = EXCLUDED.path, \
                 workflow_state = EXCLUDED.workflow_state, \
                 created_by_id = EXCLUDED.created_by_id, updated_by_id = EXCLUDED.updated_by_id, \
                 root_account_id = EXCLUDED.root_account_id, \
                 updated_at = timezone('UTC', now()) \
             RETURNING id"
        );
        let id: i64 = sqlx::query_scalar(&sql)
            .bind(draft.course_id)
            .bind(draft.registration_id)
            .bind(draft.app_id)
            .bind(draft.deployment_id)
            .bind(draft.available)
            .bind(&draft.path)
            .bind(draft.created_by_id)
            .bind(draft.updated_by_id)
            .bind(draft.root_account_id)
            .fetch_one(pool)
            .await?;
        ids.push(id);
    }

    for draft in drafts.iter().filter(|draft| draft.account_id.is_some()) {
        let sql = format!(
            "INSERT INTO lti_context_controls \
             (account_id, course_id, registration_id, app_id, deployment_id, available, path, \
              workflow_state, created_by_id, updated_by_id, root_account_id, created_at, updated_at) \
             VALUES ($1, NULL, $2, $3, $4, $5, $6, '{STATE_ACTIVE}', $7, $8, $9, \
                     timezone('UTC', now()), timezone('UTC', now())) \
             ON CONFLICT (account_id, deployment_id) WHERE account_id IS NOT NULL \
             DO UPDATE SET course_id = EXCLUDED.course_id, \
                 registration_id = EXCLUDED.registration_id, app_id = EXCLUDED.app_id, \
                 available = EXCLUDED.available, path = EXCLUDED.path, \
                 workflow_state = EXCLUDED.workflow_state, \
                 created_by_id = EXCLUDED.created_by_id, updated_by_id = EXCLUDED.updated_by_id, \
                 root_account_id = EXCLUDED.root_account_id, \
                 updated_at = timezone('UTC', now()) \
             RETURNING id"
        );
        let id: i64 = sqlx::query_scalar(&sql)
            .bind(draft.account_id)
            .bind(draft.registration_id)
            .bind(draft.app_id)
            .bind(draft.deployment_id)
            .bind(draft.available)
            .bind(&draft.path)
            .bind(draft.created_by_id)
            .bind(draft.updated_by_id)
            .bind(draft.root_account_id)
            .fetch_one(pool)
            .await?;
        ids.push(id);
    }

    Ok(ids)
}

// ---------------------------------------------------------------------------
// Context helpers
// ---------------------------------------------------------------------------

/// The account of the control's context — the account itself for an account
/// control, the course's account for a course control.
async fn resolve_context_account_id(
    pool: &PgPool,
    account_id: Option<i64>,
    course_id: Option<i64>,
) -> Result<Option<i64>, ApiError> {
    if let Some(account_id) = account_id {
        let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(pool)
            .await?;
        return Ok(row.map(|(id,)| id));
    }
    if let Some(course_id) = course_id {
        let row: Option<(Option<i64>,)> =
            sqlx::query_as::<_, (Option<i64>,)>("SELECT account_id FROM courses WHERE id = $1")
                .bind(course_id)
                .fetch_optional(pool)
                .await?;
        return Ok(row.and_then(|(account_id,)| account_id));
    }
    Ok(None)
}

/// The path of the control's context.
async fn control_path(
    pool: &PgPool,
    account_id: Option<i64>,
    course_id: Option<i64>,
) -> Result<String, ApiError> {
    if let Some(account_id) = account_id {
        let chain = account_chain_ids(pool, account_id).await?;
        return Ok(calculate_path_for_account_ids(&chain));
    }
    if let Some(course_id) = course_id {
        let row: Option<(Option<i64>,)> =
            sqlx::query_as::<_, (Option<i64>,)>("SELECT account_id FROM courses WHERE id = $1")
                .bind(course_id)
                .fetch_optional(pool)
                .await?;
        let Some((Some(account_id))) = row else {
            return Ok(String::new());
        };
        let chain = account_chain_ids(pool, account_id).await?;
        return Ok(calculate_path_for_course_id(course_id, &chain));
    }
    Ok(String::new())
}

/// Canvas `context_belongs_to_deployment` — the context of the control has to
/// be the deployment's context or one of its descendants.
async fn validate_context_belongs_to_deployment(
    pool: &PgPool,
    deployment: &DeploymentRow,
    context_account_id: i64,
) -> Result<Option<Response>, ApiError> {
    if deployment.context_type == "Course" {
        return Ok(None);
    }
    let mut valid_account_ids = sub_account_ids_recursive(pool, deployment.context_id).await?;
    valid_account_ids.push(deployment.context_id);
    if valid_account_ids.contains(&context_account_id) {
        Ok(None)
    } else {
        Ok(Some(render_errors(vec![
            "Context must belong to the deployment's context".to_string(),
        ])))
    }
}

// ---------------------------------------------------------------------------
// Anchor controls
// ---------------------------------------------------------------------------

/// Canvas `Lti::ContextControlService.build_anchor_control` — the anchor of
/// a single control.
async fn build_anchor_control(
    pool: &PgPool,
    draft: &ControlDraft,
) -> Result<Option<ControlDraft>, ApiError> {
    let deployment = find_deployment(pool, draft.deployment_id).await?;

    let mut deployment_account_ids = HashMap::new();
    let mut deployment_course_ids = HashMap::new();
    if deployment.context_type == "Course" {
        deployment_course_ids.insert(deployment.id, deployment.context_id);
    } else if deployment.context_type == "Account" {
        deployment_account_ids.insert(deployment.id, deployment.context_id);
    }

    let mut chains: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut course_account_ids: HashMap<i64, i64> = HashMap::new();
    // A control has either an account or a course as its context, never both.
    if let Some(course_id) = draft.course_id {
        let row: Option<(Option<i64>,)> =
            sqlx::query_as::<_, (Option<i64>,)>("SELECT account_id FROM courses WHERE id = $1")
                .bind(course_id)
                .fetch_optional(pool)
                .await?;
        if let Some((Some(account_id))) = row {
            course_account_ids.insert(course_id, account_id);
            chains.insert(account_id, account_chain_ids(pool, account_id).await?);
        }
    }
    if let Some(account_id) = draft.account_id {
        chains.insert(account_id, account_chain_ids(pool, account_id).await?);
    }

    let anchor_context = AnchorContext {
        chains,
        course_account_ids,
        deployment_account_ids,
        deployment_course_ids,
    };
    let deployment_ids = vec![draft.deployment_id];
    let anchors =
        build_anchor_controls(pool, &[draft.clone()], &anchor_context, &deployment_ids).await?;
    let Some(anchor) = anchors.into_iter().next() else {
        return Ok(None);
    };

    // Canvas skips the anchor when a control for it already exists.
    let sql = "SELECT 1 FROM lti_context_controls \
               WHERE account_id = $1 AND deployment_id = $2 AND course_id IS NULL LIMIT 1";
    let existing: Option<(i32,)> = sqlx::query_as(sql)
        .bind(anchor.account_id)
        .bind(anchor.deployment_id)
        .fetch_optional(pool)
        .await?;
    if existing.is_some() {
        return Ok(None);
    }

    Ok(Some(anchor))
}

/// Canvas `Lti::ContextControlService.build_anchor_controls` — the anchor
/// control sits one sub-account level below the account the deployment lives
/// in.
async fn build_anchor_controls(
    pool: &PgPool,
    drafts: &[ControlDraft],
    context: &AnchorContext,
    deployment_ids: &[i64],
) -> Result<Vec<ControlDraft>, ApiError> {
    if drafts.is_empty() {
        return Ok(Vec::new());
    }

    // Canvas `Lti::ContextControl.primary_controls_for(deployments:)`.
    let mut deployment_availabilities: HashMap<i64, bool> = if deployment_ids.is_empty() {
        HashMap::new()
    } else {
        let rows: Vec<(i64, bool)> = sqlx::query_as(
            "SELECT c.deployment_id, c.available FROM lti_context_controls c \
             JOIN lti_deployments d ON d.id = c.deployment_id \
             WHERE c.deployment_id = ANY($1) \
               AND c.workflow_state <> '{STATE_DELETED}' \
               AND c.workflow_state <> '{STATE_DELETED_WITH_CONTEXT}' \
               AND ((d.context_id = c.account_id AND d.context_type = 'Account') \
                 OR (d.context_id = c.course_id AND d.context_type = 'Course'))",
        )
        .bind(deployment_ids)
        .fetch_all(pool)
        .await?;
        rows.into_iter().collect()
    };

    for draft in drafts {
        if context.deployment_account_ids.get(&draft.deployment_id) == draft.account_id.as_ref() {
            deployment_availabilities.insert(draft.deployment_id, draft.available);
        }
    }

    let possible_account_controls: HashSet<(i64, i64)> = drafts
        .iter()
        .filter_map(|draft| draft.account_id.map(|id| (id, draft.deployment_id)))
        .collect();

    let mut anchors: Vec<ControlDraft> = Vec::new();
    let mut seen: HashSet<(i64, i64)> = HashSet::new();
    for draft in drafts {
        if context
            .deployment_course_ids
            .contains_key(&draft.deployment_id)
        {
            continue;
        }
        let Some(&deployment_account_id) = context.deployment_account_ids.get(&draft.deployment_id)
        else {
            continue;
        };
        if Some(deployment_account_id) == draft.account_id {
            continue;
        }
        let course_account_id = draft
            .course_id
            .and_then(|course_id| context.course_account_ids.get(&course_id).copied());
        if Some(deployment_account_id) == course_account_id {
            continue;
        }
        let account_id = draft.account_id.or(course_account_id);
        let Some(account_id) = account_id else {
            continue;
        };
        let Some(chain) = context.chains.get(&account_id) else {
            continue;
        };
        let Some(deployment_index) = chain.iter().position(|id| *id == deployment_account_id)
        else {
            continue;
        };
        if deployment_index == 0 {
            continue;
        }
        let anchor_account_id = chain[deployment_index - 1];
        if draft.account_id == Some(anchor_account_id) {
            continue;
        }
        if possible_account_controls.contains(&(anchor_account_id, draft.deployment_id)) {
            continue;
        }
        let available = deployment_availabilities
            .get(&draft.deployment_id)
            .copied()
            .unwrap_or(true);
        let path = match context.chains.get(&anchor_account_id) {
            Some(chain) => calculate_path_for_account_ids(chain),
            None => {
                calculate_path_for_account_ids(&account_chain_ids(pool, anchor_account_id).await?)
            }
        };
        let root_account_id = root_account_of_account(pool, anchor_account_id).await?;
        if !seen.insert((anchor_account_id, draft.deployment_id)) {
            continue;
        }
        anchors.push(ControlDraft {
            account_id: Some(anchor_account_id),
            course_id: None,
            deployment_id: draft.deployment_id,
            available,
            path,
            registration_id: draft.registration_id,
            app_id: draft.app_id,
            root_account_id,
            created_by_id: draft.created_by_id,
            updated_by_id: draft.updated_by_id,
        });
    }

    // Canvas drops the anchors that already exist.
    if !anchors.is_empty() {
        let account_ids: Vec<i64> = anchors.iter().filter_map(|a| a.account_id).collect();
        let anchor_deployment_ids: Vec<i64> = anchors
            .iter()
            .map(|a| a.deployment_id)
            .collect::<HashSet<i64>>()
            .into_iter()
            .collect();
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT account_id, deployment_id FROM lti_context_controls \
             WHERE account_id = ANY($1) AND deployment_id = ANY($2) \
               AND workflow_state <> '{STATE_DELETED}' \
               AND workflow_state <> '{STATE_DELETED_WITH_CONTEXT}'",
        )
        .bind(&account_ids)
        .bind(&anchor_deployment_ids)
        .fetch_all(pool)
        .await?;
        let existing: HashSet<(i64, i64)> = rows.into_iter().collect();
        anchors.retain(|anchor| match anchor.account_id {
            Some(account_id) => !existing.contains(&(account_id, anchor.deployment_id)),
            None => true,
        });
    }

    Ok(anchors)
}

// ---------------------------------------------------------------------------
// Change log
// ---------------------------------------------------------------------------

/// Canvas `Lti::ContextControl::TRACKED_ATTRIBUTES`.
fn tracked_control(control: &ContextControlRow) -> Value {
    json!({
        "id": control.id,
        "account_id": control.account_id,
        "course_id": control.course_id,
        "deployment_id": control.deployment_id,
        "available": control.available,
        "workflow_state": control.workflow_state,
    })
}

/// The tracked attributes of the given controls, keyed by control id.
fn tracked_controls(controls: &[ContextControlRow]) -> HashMap<i64, Value> {
    controls
        .iter()
        .map(|control| (control.id, tracked_control(control)))
        .collect()
}

/// Canvas `Lti::RegistrationHistoryEntry.track_bulk_control_changes` — the
/// state of the controls a request touches, before it runs.
async fn snapshot_controls(
    pool: &PgPool,
    drafts: &[ControlDraft],
) -> Result<HashMap<i64, Value>, ApiError> {
    let deployment_ids: Vec<i64> = drafts
        .iter()
        .map(|draft| draft.deployment_id)
        .collect::<HashSet<i64>>()
        .into_iter()
        .collect();
    if deployment_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let sql =
        format!("SELECT {CONTROL_COLUMNS} FROM lti_context_controls WHERE deployment_id = ANY($1)");
    let rows: Vec<ContextControlRow> = sqlx::query_as(&sql)
        .bind(&deployment_ids)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .iter()
        .filter(|row| {
            drafts.iter().any(|draft| {
                (draft.account_id == row.account_id && draft.deployment_id == row.deployment_id)
                    || (draft.course_id == row.course_id
                        && draft.deployment_id == row.deployment_id)
            })
        })
        .map(|row| (row.id, tracked_control(row)))
        .collect())
}

/// Canvas `Lti::RegistrationHistoryEntry.track_control_changes` /
/// `track_bulk_control_changes` — the change-log entry of a control change.
async fn write_history_entry(
    pool: &PgPool,
    registration: &RegistrationRow,
    old_controls: &HashMap<i64, Value>,
    new_controls: &HashMap<i64, Value>,
    root_account_id: i64,
    user_id: i64,
    comment: Option<&str>,
) -> Result<(), ApiError> {
    if !table_exists(pool, "lti_registration_history_entries").await? {
        return Ok(());
    }

    let mut diff: Map<String, Value> = Map::new();
    for (id, old) in old_controls {
        let new = new_controls.get(id).cloned().unwrap_or(Value::Null);
        if old != &new {
            diff.insert(id.to_string(), json!({ "old": old, "new": new }));
        }
    }
    for (id, new) in new_controls {
        if !old_controls.contains_key(id) {
            diff.insert(id.to_string(), json!({ "old": Value::Null, "new": new }));
        }
    }
    if diff.is_empty() {
        return Ok(());
    }

    let old_context_controls: Map<String, Value> = old_controls
        .iter()
        .map(|(id, value)| (id.to_string(), value.clone()))
        .collect();
    let new_context_controls: Map<String, Value> = new_controls
        .iter()
        .map(|(id, value)| (id.to_string(), value.clone()))
        .collect();

    sqlx::query(
        "INSERT INTO lti_registration_history_entries \
         (root_account_id, lti_registration_id, created_by_id, diff, update_type, comment, \
          old_context_controls, new_context_controls) \
         VALUES ($1, $2, $3, $4, 'control_edit', $5, $6, $7)",
    )
    .bind(root_account_id)
    .bind(registration.id)
    .bind(user_id)
    .bind(json!({ "context_controls": diff }))
    .bind(comment)
    .bind(json!(old_context_controls))
    .bind(json!(new_context_controls))
    .execute(pool)
    .await?;
    Ok(())
}

/// Whether a table exists in the public schema.
async fn table_exists(pool: &PgPool, name: &str) -> Result<bool, ApiError> {
    let row: Option<(bool,)> = sqlx::query_as("SELECT to_regclass($1) IS NOT NULL")
        .bind(format!("public.{name}"))
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(exists,)| exists).unwrap_or(false))
}
