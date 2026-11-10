//! star-lti — Canvas LTI 1.3 registrations and deployments.
//!
//! Canvas parity (`app/controllers/lti/registrations_controller.rb`,
//! `app/controllers/lti/deployments_controller.rb`, `lib/api/v1/lti/*`):
//!
//! - `GET    /api/v1/accounts/{accountId}/lti_registrations` — list
//! - `POST   /api/v1/accounts/{accountId}/lti_registrations` — create
//! - `GET    /api/v1/lti_registrations/{id}` — show
//! - `DELETE /api/v1/lti_registrations/{id}` — soft delete
//! - `GET    /api/v1/lti_registrations/{id}/deployments` — list deployments
//! - `POST   /api/v1/lti_registrations/{id}/deployments` — create deployment
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
    routing::get,
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

/// Canvas lists 15 registrations per page by default.
const DEFAULT_PER_PAGE: i64 = 15;
const MAX_PER_PAGE: i64 = 100;

/// Canvas registration workflow states (`active | inactive | deleted`).
const STATE_ACTIVE: &str = "active";
const STATE_DELETED: &str = "deleted";

// ---------------------------------------------------------------------------
// Domain rows
// ---------------------------------------------------------------------------

/// A row of `lti_registrations`, in Canvas registration JSON shape (the
/// `configuration` / `overlay` JSONB blobs are serialized separately).
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
struct DeploymentRow {
    id: i64,
    registration_id: i64,
    deployment_id: String,
    context_type: String,
    context_id: i64,
    context_name: Option<String>,
    workflow_state: String,
}

/// Columns selected for a [`RegistrationRow`], shared by the lookups.
const REGISTRATION_COLUMNS: &str = "id, account_id, root_account_id, name, admin_nickname, \
     vendor, description, icon_url, lti_version, dynamic_registration, internal_service, \
     lock_deploying, template_registration_id, workflow_state, configuration, overlay, \
     created_by_id, updated_by_id, created_at, updated_at";

/// Columns selected for a [`DeploymentRow`].
const DEPLOYMENT_COLUMNS: &str =
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

/// Body for `POST /lti_registrations/{id}/deployments`.
#[derive(Debug, Deserialize, Default)]
pub struct CreateDeploymentRequest {
    pub for_subaccount_id: Option<i64>,
    pub for_course_id: Option<i64>,
    pub available: Option<bool>,
}

// ---------------------------------------------------------------------------
// Permission helpers (mirror star-accounts)
// ---------------------------------------------------------------------------

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

/// Global admin OR account admin for the account or any ancestor, matching
/// `can_access_account` in star-accounts.
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

/// 404 when the account does not exist (Canvas `require_account_context`).
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

/// True when the account has no parent.
async fn is_root_account(pool: &PgPool, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(pool)
            .await?;
    Ok(matches!(row, Some((None,))))
}

/// Require manage rights on the account that owns a registration.
async fn require_registration_permission(
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
async fn find_active_registration(pool: &PgPool, id: i64) -> Result<RegistrationRow, ApiError> {
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
async fn find_registration_in_account(
    pool: &PgPool,
    id: i64,
    account_id: i64,
) -> Result<RegistrationRow, ApiError> {
    let row = find_active_registration(pool, id).await?;
    if row.account_id != account_id {
        return Err(ApiError::BadRequest(
            "registration does not belong to account".to_string(),
        ));
    }
    Ok(row)
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
fn deployment_json(deployment: &DeploymentRow, root_account_deployment: bool) -> Value {
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
        .route(
            "/accounts/{account_id}/lti_registrations",
            get(list_registrations).post(create_registration),
        )
        // Canvas-nested registration paths.
        .route(
            "/accounts/{account_id}/lti_registrations/{id}",
            get(show_registration_scoped).delete(delete_registration_scoped),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/deployments",
            get(list_deployments_scoped).post(create_deployment_scoped),
        )
        // Brief-compatible flat paths.
        .route(
            "/lti_registrations/{id}",
            get(show_registration).delete(delete_registration),
        )
        .route(
            "/lti_registrations/{id}/deployments",
            get(list_deployments).post(create_deployment),
        )
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

/// `GET /lti_registrations/{id}` — show one registration. Canvas always
/// embeds the account binding and configuration.
async fn show_registration(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<IncludeQuery>,
) -> Result<Json<Value>, ApiError> {
    let registration = find_active_registration(&state.db, id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    let includes = Includes {
        configuration: true,
        overlay: params.includes().overlay,
    };
    Ok(Json(
        registration_json(&state.db, &registration, includes).await?,
    ))
}

/// Canvas path alias: `GET /accounts/{accountId}/lti_registrations/{id}`.
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

/// `DELETE /lti_registrations/{id}` — Canvas soft-deletes the registration
/// and returns its JSON with `workflow_state: "deleted"`.
async fn delete_registration(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let registration = find_active_registration(&state.db, id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;
    soft_delete_registration(&state.db, id, auth.id).await
}

/// Canvas path alias: `DELETE /accounts/{accountId}/lti_registrations/{id}`.
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

/// `GET /lti_registrations/{id}/deployments` — Canvas returns a bare array.
async fn list_deployments(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    let registration = find_active_registration(&state.db, id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;
    Ok(Json(
        deployments_for_registration(&state.db, registration.id).await?,
    ))
}

/// Canvas path alias:
/// `GET /accounts/{accountId}/lti_registrations/{registrationId}/deployments`.
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

/// `POST /lti_registrations/{id}/deployments` — deploy into an account (the
/// registration's own by default) or a course. Canvas answers `200` with the
/// deployment JSON.
async fn create_deployment(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateDeploymentRequest>,
) -> Result<Response, ApiError> {
    let registration = find_active_registration(&state.db, id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;
    create_deployment_for_registration(&state.db, &registration, &auth, payload).await
}

/// Canvas path alias:
/// `POST /accounts/{accountId}/lti_registrations/{registrationId}/deployments`.
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
