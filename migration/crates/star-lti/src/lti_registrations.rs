//! Extension handlers for lti-2 slice:
//! - validate_lti_configuration  POST /accounts/:id/lti_registrations/configuration/validate
//! - check_domain_duplicates      GET  /accounts/:id/lti_registrations/check_domain_duplicates
//! - update_registration         PUT  /accounts/:id/lti_registrations/:id
//! - reset_registration          PUT  /accounts/:id/lti_registrations/:id/reset
//! - bind_registration           POST /accounts/:id/lti_registrations/:id/bind
//! - unbind_registration         DELETE /accounts/:id/lti_registrations/:id/unbind
//! - show_by_client_id           GET  /accounts/:id/lti_registration_by_client_id/:client_id
//! - show_by_utid                GET  /accounts/:id/lti_registrations/by_utid/:utid
//! - install_status              GET  /accounts/:id/lti_registrations/install_status/:client_id
//! - show_deployment_scoped       GET  /accounts/:id/lti_registrations/:id/deployments/:deployment_id
//! - delete_deployment_scoped    DELETE /accounts/:id/lti_registrations/:id/deployments/:deployment_id
//! - context_search              GET  /accounts/:id/lti_registrations/:id/deployments/:deployment_id/context_search
//!
//! Canvas: `app/controllers/lti/registrations_controller.rb`

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::ApiError;

use crate::{
    account_binding_json, can_manage_account, create_deployment_for_registration,
    require_account, require_registration_permission, root_account_id,
    find_active_registration, find_registration_in_account, is_root_account,
    registration_json, soft_delete_registration, AppState, BindingRow,
    CreateDeploymentRequest, DeploymentRow, Includes, RegistrationRow, STATE_ACTIVE,
    STATE_DELETED, DEPLOYMENT_COLUMNS,
};

// ---------------------------------------------------------------------------
// Request shapes
// ---------------------------------------------------------------------------

/// Body for validate_lti_configuration.
#[derive(Debug, Deserialize)]
pub struct ValidateConfigurationRequest {
    pub lti_configuration: Option<Value>,
    pub url: Option<String>,
}

/// Body for bind.
#[derive(Debug, Deserialize)]
pub struct BindRequest {
    #[serde(rename = "workflow_state")]
    pub workflow_state: String,
}

/// Body for update_registration.
#[derive(Debug, Deserialize)]
pub struct UpdateRegistrationRequest {
    pub name: Option<String>,
    pub admin_nickname: Option<String>,
    pub vendor: Option<String>,
    pub description: Option<String>,
    pub configuration: Option<Value>,
    pub overlay: Option<Value>,
    #[serde(rename = "workflow_state")]
    pub workflow_state: Option<String>,
    #[serde(rename = "lock_deploying")]
    pub lock_deploying: Option<bool>,
    pub comment: Option<String>,
}

/// Query for context_search.
#[derive(Debug, Deserialize)]
pub struct ContextSearchQuery {
    pub search_term: Option<String>,
}

// ---------------------------------------------------------------------------
// validate_lti_configuration
// ---------------------------------------------------------------------------

/// POST /accounts/:account_id/lti_registrations/configuration/validate
///
/// Validates an LTI configuration object or fetches one from a URL.
/// Requires exactly one of `lti_configuration` OR `url`.
pub async fn validate_lti_configuration(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<ValidateConfigurationRequest>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let config = match (&payload.lti_configuration, &payload.url) {
        (Some(cfg), None) => cfg.clone(),
        (None, Some(_)) => {
            // URL validation not supported in this environment —
            // validate that URL is reachable and contains JSON config
            return Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "errors": ["url validation not implemented in this environment"] })),
            )
                .into_response());
        }
        (Some(_), Some(_)) => {
            return Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "errors": ["only one of lti_configuration or url is allowed"] })),
            )
                .into_response());
        }
        (None, None) => {
            return Ok((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "errors": ["one of lti_configuration or url is required"] })),
            )
                .into_response());
        }
    };

    let errors = validate_config_schema(&config);
    if !errors.is_empty() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "errors": errors })),
        )
            .into_response());
    }

    // Normalize: ensure redirect_uris from target_link_uri if not set
    let mut normalized = config.clone();
    if !normalized.get("redirect_uris").is_some() {
        if let Some(uri) = normalized.get("target_link_uri").and_then(Value::as_str) {
            normalized["redirect_uris"] = json!([uri]);
        }
    }

    Ok((StatusCode::OK, Json(json!({ "configuration": normalized }))).into_response())
}

fn validate_config_schema(config: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    if !config
        .get("title")
        .and_then(Value::as_str)
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        errors.push("title is required".to_string());
    }
    if !config
        .get("target_link_uri")
        .and_then(Value::as_str)
        .map(|s| !s.is_empty())
        .unwrap_or(false)
    {
        errors.push("target_link_uri is required".to_string());
    }
    errors
}

// ---------------------------------------------------------------------------
// check_domain_duplicates
// ---------------------------------------------------------------------------

/// GET /accounts/:account_id/lti_registrations/check_domain_duplicates?domain=...
///
/// Returns registrations in the account hierarchy that share the given domain
/// in their target_link_uri.
pub async fn check_domain_duplicates(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<DomainQuery>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let domain = params.domain.as_deref().unwrap_or_default();
    if domain.is_empty() {
        return Ok(Json(json!({ "duplicates": [] })));
    }

    let root_id = root_account_id(&state.db, account_id).await?;

    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        r#"SELECT r.id, r.name, r.admin_nickname
           FROM lti_registrations r
           WHERE r.workflow_state <> 'deleted'
             AND r.account_id IN ($1, $2)
             AND r.configuration->>'target_link_uri' LIKE '%' || $3 || '%'"#,
    )
    .bind(account_id)
    .bind(root_id)
    .bind(domain)
    .fetch_all(&state.db)
    .await?;

    let duplicates: Vec<Value> = rows
        .into_iter()
        .map(|(id, name, admin_nickname)| json!({
            "id": id.to_string(),
            "name": name,
            "admin_nickname": admin_nickname
        }))
        .collect();

    Ok(Json(json!({ "duplicates": duplicates })))
}

#[derive(Debug, Deserialize)]
struct DomainQuery {
    domain: Option<String>,
}

// ---------------------------------------------------------------------------
// update_registration
// ---------------------------------------------------------------------------

/// PUT /accounts/:account_id/lti_registrations/:id
///
/// Update editable fields on a registration including configuration and overlay.
pub async fn update_registration(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateRegistrationRequest>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    let mut set_clauses = vec!["updated_by_id = $2".to_string()];
    let mut p: Vec<String> = Vec::new();

    if let Some(ref name) = payload.name {
        p.push(format!("name = '${}'", name));
    }
    if let Some(ref admin_nickname) = payload.admin_nickname {
        p.push(format!("admin_nickname = '${}'", admin_nickname));
    }
    if let Some(ref vendor) = payload.vendor {
        p.push(format!("vendor = '${}'", vendor));
    }
    if let Some(ref description) = payload.description {
        p.push(format!("description = '${}'", description));
    }
    if let Some(lock_deploying) = payload.lock_deploying {
        p.push(format!("lock_deploying = {}", lock_deploying));
    }

    // Merge configuration if provided
    if let Some(ref config_patch) = payload.configuration {
        let current: Option<(Value,)> =
            sqlx::query_as("SELECT configuration FROM lti_registrations WHERE id = $1")
                .bind(id)
                .fetch_optional(&state.db)
                .await?;
        if let Some((current_config,)) = current {
            let merged = merge_config(&current_config, config_patch);
            let merged_str = serde_json::to_string(&merged).unwrap_or_default();
            p.push(format!("configuration = '{}'", merged_str));
        }
    }

    // Set overlay if provided (overwrite, not merge)
    if payload.overlay.is_some() {
        let overlay_str =
            serde_json::to_string(&payload.overlay).unwrap_or_else(|_| "{}".to_string());
        p.push(format!("overlay = '{}'", overlay_str));
    }

    // Handle workflow_state: maps to binding state for account binding
    if let Some(ref ws) = payload.workflow_state {
        p.push(format!("workflow_state = '{}'", ws));
    }

    if p.is_empty() {
        let includes = Includes { configuration: true, overlay: true };
        return Ok(Json(registration_json(&state.db, &registration, includes).await?));
    }

    set_clauses.push(p.join(", "));
    let sql = format!(
        "UPDATE lti_registrations SET {} WHERE id = $1 AND workflow_state <> 'deleted' RETURNING {}",
        set_clauses.join(" "),
        "id, account_id, root_account_id, name, admin_nickname, vendor, description, icon_url, lti_version, dynamic_registration, internal_service, lock_deploying, template_registration_id, workflow_state, configuration, overlay, created_by_id, updated_by_id, created_at, updated_at"
    );

    let updated: RegistrationRow = sqlx::query_as(&sql)
        .bind(id)
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;

    let includes = Includes { configuration: true, overlay: true };
    Ok(Json(registration_json(&state.db, &updated, includes).await?))
}

fn merge_config(current: &Value, patch: &Value) -> Value {
    let mut result = current.clone();
    if let (Some(curr_obj), Some(patch_obj)) = (result.as_object_mut(), patch.as_object()) {
        for (key, value) in patch_obj {
            curr_obj.insert(key.clone(), value.clone());
        }
    }
    result
}

// ---------------------------------------------------------------------------
// reset_registration
// ---------------------------------------------------------------------------

/// PUT /accounts/:account_id/lti_registrations/:id/reset
///
/// Clears the per-account overlay, resetting to base configuration defaults.
pub async fn reset_registration(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    sqlx::query(
        "UPDATE lti_registrations SET overlay = '{}', updated_by_id = $1 WHERE id = $2",
    )
    .bind(auth.id)
    .bind(id)
    .execute(&state.db)
    .await?;

    let updated = find_active_registration(&state.db, id).await?;
    let includes = Includes { configuration: true, overlay: true };
    Ok(Json(registration_json(&state.db, &updated, includes).await?))
}

// ---------------------------------------------------------------------------
// destroy_registration (account-scoped)
// ---------------------------------------------------------------------------

/// DELETE /accounts/:account_id/lti_registrations/:id
pub async fn destroy_registration(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;
    soft_delete_registration(&state.db, id, auth.id).await
}

// ---------------------------------------------------------------------------
// bind / unbind
// ---------------------------------------------------------------------------

/// POST /accounts/:account_id/lti_registrations/:id/bind
///
/// Set binding workflow_state to `on` or `off`. Returns binding JSON.
pub async fn bind_registration(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<BindRequest>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    // Site admin does not allow local bindings
    let is_site_admin: bool = sqlx::query_scalar(
        "SELECT id = 1 FROM accounts WHERE id = $1 AND parent_account_id IS NULL",
    )
    .bind(account_id)
    .fetch_optional(&state.db)
    .await?
    .unwrap_or(false);
    if is_site_admin {
        return Err(ApiError::BadRequest(
            "site admin local bindings are not allowed".to_string(),
        ));
    }

    let binding_state = match payload.workflow_state.as_str() {
        "on" => "on",
        "off" => "off",
        _ => {
            return Err(ApiError::BadRequest(
                "workflow_state must be one of 'on', 'off'".to_string(),
            ));
        }
    };

    let root_id = root_account_id(&state.db, account_id).await?;

    sqlx::query(
        r#"INSERT INTO lti_registration_account_bindings
           (registration_id, account_id, root_account_id, workflow_state, created_by_id, updated_by_id)
           VALUES ($1, $2, $3, $4, $5, $5)
           ON CONFLICT (registration_id, account_id) DO UPDATE
           SET workflow_state = EXCLUDED.workflow_state, updated_at = now()"#,
    )
    .bind(id)
    .bind(account_id)
    .bind(root_id)
    .bind(binding_state)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    account_binding_json(&state.db, id, account_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound)
}

/// DELETE /accounts/:account_id/lti_registrations/:id/unbind
///
/// Remove the account binding for an inherited registration.
pub async fn unbind_registration(
    State(state): State<AppState>,
    Path((account_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    let registration = find_registration_in_account(&state.db, id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    // Only allowed for inherited registrations
    if registration.account_id == account_id {
        return Err(ApiError::BadRequest(
            "use destroy to delete local registrations".to_string(),
        ));
    }

    sqlx::query(
        r#"UPDATE lti_registration_account_bindings
           SET workflow_state = 'deleted', updated_at = now()
           WHERE registration_id = $1 AND account_id = $2 AND workflow_state <> 'deleted'"#,
    )
    .bind(id)
    .bind(account_id)
    .execute(&state.db)
    .await?;

    account_binding_json(&state.db, id, account_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Alternate lookups
// ---------------------------------------------------------------------------

/// GET /accounts/:account_id/lti_registration_by_client_id/:client_id
///
/// Lookup by developer key (client_id). In our schema, registration id IS the
/// unified_tool_id used as client_id.
pub async fn show_by_client_id(
    State(state): State<AppState>,
    Path((account_id, client_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<RegistrationRow> = sqlx::query_as(
        format!(
            "SELECT {} FROM lti_registrations WHERE id = $1 AND workflow_state <> 'deleted'",
            "id, account_id, root_account_id, name, admin_nickname, vendor, description, \
             icon_url, lti_version, dynamic_registration, internal_service, lock_deploying, \
             template_registration_id, workflow_state, configuration, overlay, \
             created_by_id, updated_by_id, created_at, updated_at"
        )
        .as_str(),
    )
    .bind(client_id)
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some(reg) if reg.account_id == account_id || reg.account_id == 1 => {
            let includes = Includes { configuration: true, overlay: true };
            Ok(Json(registration_json(&state.db, &reg, includes).await?))
        }
        _ => Err(ApiError::NotFound),
    }
}

/// GET /accounts/:account_id/lti_registrations/by_utid/:utid
///
/// Lookup by unified_tool_id stored in configuration JSONB.
pub async fn show_by_utid(
    State(state): State<AppState>,
    Path((account_id, utid)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<RegistrationRow> = sqlx::query_as(
        format!(
            "SELECT {} FROM lti_registrations \
             WHERE configuration->>'unified_tool_id' = $1 AND workflow_state <> 'deleted'",
            "id, account_id, root_account_id, name, admin_nickname, vendor, description, \
             icon_url, lti_version, dynamic_registration, internal_service, lock_deploying, \
             template_registration_id, workflow_state, configuration, overlay, \
             created_by_id, updated_by_id, created_at, updated_at"
        )
        .as_str(),
    )
    .bind(&utid)
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some(reg) if reg.account_id == account_id => {
            let includes = Includes { configuration: true, overlay: true };
            Ok(Json(registration_json(&state.db, &reg, includes).await?))
        }
        _ => Err(ApiError::NotFound),
    }
}

/// GET /accounts/:account_id/lti_registrations/install_status/:client_id
///
/// Check if a Site Admin registration has been installed locally.
pub async fn install_status(
    State(state): State<AppState>,
    Path((account_id, client_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<RegistrationRow> = sqlx::query_as(
        format!(
            "SELECT {} FROM lti_registrations WHERE id = $1 AND workflow_state <> 'deleted'",
            "id, account_id, root_account_id, name, admin_nickname, vendor, description, \
             icon_url, lti_version, dynamic_registration, internal_service, lock_deploying, \
             template_registration_id, workflow_state, configuration, overlay, \
             created_by_id, updated_by_id, created_at, updated_at"
        )
        .as_str(),
    )
    .bind(client_id)
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some(reg) if reg.account_id == account_id || reg.account_id == 1 => {
            let includes = Includes { configuration: true, overlay: true };
            Ok(Json(registration_json(&state.db, &reg, includes).await?))
        }
        _ => Err(ApiError::NotFound),
    }
}

// ---------------------------------------------------------------------------
// Deployment show / destroy (account-scoped)
// ---------------------------------------------------------------------------

/// GET /accounts/:account_id/lti_registrations/:id/deployments/:deployment_id
pub async fn show_deployment_scoped(
    State(state): State<AppState>,
    Path((account_id, registration_id, deployment_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    let registration = find_registration_in_account(&state.db, registration_id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    let sql = format!(
        "SELECT {} FROM lti_deployments WHERE id = $1 AND registration_id = $2 AND workflow_state <> $3",
        DEPLOYMENT_COLUMNS
    );
    let row: Option<DeploymentRow> = sqlx::query_as(&sql)
        .bind(deployment_id)
        .bind(registration_id)
        .bind(STATE_DELETED)
        .fetch_optional(&state.db)
        .await?;

    let row = row.ok_or(ApiError::NotFound)?;
    let root_deployment = row.context_type == "Account"
        && is_root_account(&state.db, row.context_id).await?;
    Ok(Json(crate::deployment_json(&row, root_deployment)))
}

/// DELETE /accounts/:account_id/lti_registrations/:id/deployments/:deployment_id
pub async fn delete_deployment_scoped(
    State(state): State<AppState>,
    Path((account_id, registration_id, deployment_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    let registration = find_registration_in_account(&state.db, registration_id, account_id).await?;
    require_registration_permission(&state.db, &auth, &registration).await?;

    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_deployments WHERE id = $1 AND registration_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(deployment_id)
    .bind(registration_id)
    .fetch_optional(&state.db)
    .await?;

    row.map(|_| ()).ok_or(ApiError::NotFound)?;

    sqlx::query(
        "UPDATE lti_deployments SET workflow_state = 'deleted', updated_at = now() \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(deployment_id)
    .execute(&state.db)
    .await?;

    Err(ApiError::NotFound) // Canvas returns 404 after delete
}

// ---------------------------------------------------------------------------
// context_search
// ---------------------------------------------------------------------------

/// GET /accounts/:account_id/lti_registrations/:registration_id/deployments/:deployment_id/context_search
///
/// Search accounts and courses by name/sis_id/course_code, used by the
/// registration UI to show deployment availability targets.
pub async fn context_search(
    State(state): State<AppState>,
    Path((account_id, registration_id, deployment_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Query(params): Query<ContextSearchQuery>,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify registration and deployment exist
    let _ = find_active_registration(&state.db, registration_id).await?;
    let _: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_deployments WHERE id = $1 AND registration_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(deployment_id)
    .bind(registration_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    let term = params.search_term.as_deref().unwrap_or_default();
    let limit = 20;

    let accounts: Vec<Value> = sqlx::query_as(
        "SELECT id, name FROM accounts WHERE name ILIKE $1 AND (id = $2 OR parent_account_id = $2) LIMIT $3",
    )
    .bind(format!("%{}%", term))
    .bind(account_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(id, name): (i64, String)| json!({ "id": id, "name": name, "type": "account" }))
    .collect();

    let courses: Vec<Value> = sqlx::query_as(
        "SELECT id, name FROM courses WHERE name ILIKE $1 AND account_id = $2 LIMIT $3",
    )
    .bind(format!("%{}%", term))
    .bind(account_id)
    .bind(limit)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(id, name): (i64, String)| json!({ "id": id, "name": name, "type": "course" }))
    .collect();

    let mut results = accounts;
    results.extend(courses);
    Ok(Json(json!({ "results": results })))
}
