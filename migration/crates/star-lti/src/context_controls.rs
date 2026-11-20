//! Canvas parity for LTI context controls (`Lti::ContextControl`).
//!
//! Canvas source: `app/controllers/lti/context_controls_controller.rb`,
//! `app/controllers/lti/deployments_controller.rb#list_controls`,
//! `lib/api/v1/lti/context_control.rb`.
//!
//! Routes (mounted under `/api/v1` by star-http):
//!
//! - `GET    /accounts/{account_id}/lti_registrations/{registration_id}/controls`
//! - `POST   /accounts/{account_id}/lti_registrations/{registration_id}/controls`
//! - `POST   /accounts/{account_id}/lti_registrations/{registration_id}/controls/bulk`
//! - `GET    /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}`
//! - `PUT    /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}`
//! - `DELETE /accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}`
//! - `GET    /accounts/{account_id}/lti_registrations/{registration_id}/deployments/{deployment_id}/controls`
//!
//! Context controls gate a registration deployment's availability per account
//! or course. Canvas stores them on `lti_context_controls` (migration 0223);
//! the JSON carries the calculated attrs the Canvas serializer computes
//! (`context_name`, `display_path`, `depth`, `subaccount_count`,
//! `course_count`, `child_control_count`).

use axum::{
    extract::{Path, State},
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

use crate::{
    can_manage_account, deployment_json, find_active_deployment, find_active_registration,
    record_history_entry, require_account, root_account_id, user_summary, DeploymentRow,
    STATE_DELETED,
};

/// Canvas MAX_BULK_CREATE (context_controls_controller.rb).
const MAX_BULK_CREATE: usize = 100;

/// A row of `lti_context_controls`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ContextControlRow {
    id: i64,
    registration_id: i64,
    deployment_id: i64,
    account_id: Option<i64>,
    course_id: Option<i64>,
    root_account_id: Option<i64>,
    available: bool,
    workflow_state: String,
    created_by_id: Option<i64>,
    updated_by_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const CONTROL_COLUMNS: &str = "id, registration_id, deployment_id, account_id, course_id, \
     root_account_id, available, workflow_state, created_by_id, updated_by_id, created_at, \
     updated_at";

/// Body for `POST .../controls` — one of `account_id` / `course_id`, an
/// optional `deployment_id` (defaults to the registration's root-account
/// deployment) and `available` (context_controls_controller.rb:304-337).
#[derive(Debug, Deserialize, Default)]
pub struct CreateControlRequest {
    pub account_id: Option<i64>,
    pub course_id: Option<i64>,
    pub deployment_id: Option<i64>,
    pub available: Option<bool>,
    pub comment: Option<String>,
}

/// One element of the `POST .../controls/bulk` array
/// (context_controls_controller.rb:359-434).
#[derive(Debug, Deserialize, Default)]
pub struct BulkControlItem {
    pub account_id: Option<i64>,
    pub course_id: Option<i64>,
    pub deployment_id: Option<i64>,
    pub available: Option<bool>,
}

/// Body for `PUT .../controls/{id}` — Canvas requires `available`
/// (context_controls_controller.rb:503-516).
#[derive(Debug, Deserialize, Default)]
pub struct UpdateControlRequest {
    pub available: Option<bool>,
    pub comment: Option<String>,
}

/// Build the context controls router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/controls",
            get(index_controls).post(create_control),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/controls/bulk",
            post(create_controls_bulk),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/controls/{id}",
            get(show_control).put(update_control).delete(delete_control),
        )
        .route(
            "/accounts/{account_id}/lti_registrations/{registration_id}/deployments/{deployment_id}/controls",
            get(list_deployment_controls),
        )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Canvas `render_errors` (context_controls_controller.rb:550-554):
/// `422 { "errors": [...] }`.
fn render_errors(errors: Vec<String>) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "errors": errors })),
    )
        .into_response()
}

/// The registration's deployment on the (root) account — Canvas
/// `root_account_deployment` (:566-569).
async fn root_account_deployment(
    pool: &PgPool,
    registration_id: i64,
    account_id: i64,
) -> Result<Option<DeploymentRow>, ApiError> {
    let sql = format!(
        "SELECT {crate::DEPLOYMENT_COLUMNS} FROM lti_deployments \
         WHERE registration_id = $1 AND context_type = 'Account' AND context_id = $2 \
           AND workflow_state <> '{STATE_DELETED}' \
         ORDER BY id ASC LIMIT 1"
    );
    let row: Option<DeploymentRow> = sqlx::query_as(&sql)
        .bind(registration_id)
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// Fetch one active control scoped to the registration
/// (Canvas `control`, :572-577).
async fn find_control(
    pool: &PgPool,
    registration_id: i64,
    root_account_id: i64,
    control_id: i64,
) -> Result<ContextControlRow, ApiError> {
    let sql = format!(
        "SELECT {CONTROL_COLUMNS} FROM lti_context_controls \
         WHERE id = $1 AND registration_id = $2 AND root_account_id = $3 \
           AND workflow_state <> 'deleted'"
    );
    let row: Option<ContextControlRow> = sqlx::query_as(&sql)
        .bind(control_id)
        .bind(registration_id)
        .bind(root_account_id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// The name of the context a control targets.
async fn context_name(pool: &PgPool, control: &ContextControlRow) -> Result<Value, ApiError> {
    if let Some(account_id) = control.account_id {
        let row: Option<(String,)> = sqlx::query_as("SELECT name FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(pool)
            .await?;
        return Ok(row.map(|(name,)| json!(name)).unwrap_or(Value::Null));
    }
    if let Some(course_id) = control.course_id {
        let row: Option<(String,)> = sqlx::query_as("SELECT name FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
        return Ok(row.map(|(name,)| json!(name)).unwrap_or(Value::Null));
    }
    Ok(Value::Null)
}

/// Account chain names between the root and the context — the Canvas
/// `display_path` calculation, scoped to names the workspace stores.
async fn display_path(pool: &PgPool, account_id: i64) -> Result<Vec<String>, ApiError> {
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
    let mut chain = rows;
    chain.pop(); // drop the root
    chain.reverse(); // farthest-from-root first
    Ok(chain.into_iter().map(|(_, name)| name).collect())
}

/// The `path` column Canvas materializes (`calculate_path_for_account_ids`)
/// — the account chain ids joined with underscores.
async fn control_path(pool: &PgPool, account_id: i64) -> Result<String, ApiError> {
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
    Ok(rows
        .into_iter()
        .rev()
        .map(|(id,)| id.to_string())
        .collect::<Vec<_>>()
        .join("_"))
}

/// Canvas `lti_context_control_json` (lib/api/v1/lti/context_control.rb)
/// with the calculated attrs.
async fn control_json(pool: &PgPool, control: &ContextControlRow) -> Result<Value, ApiError> {
    let (context_name, display_path, path, subaccount_count, course_count, child_control_count) =
        if let Some(account_id) = control.account_id {
            let name = sqlx::query_as::<_, (String,)>("SELECT name FROM accounts WHERE id = $1")
                .bind(account_id)
                .fetch_optional(pool)
                .await?
                .map(|(name,)| name);
            let display = display_path(pool, account_id).await?;
            let path = control_path(pool, account_id).await?;
            let (subaccounts,): (i64,) = sqlx::query_as(
                "WITH RECURSIVE down AS ( \
                     SELECT id FROM accounts WHERE id = $1 \
                     UNION ALL \
                     SELECT a.id FROM accounts a JOIN down ON a.parent_account_id = down.id \
                 ) SELECT COUNT(*) - 1 FROM down",
            )
            .bind(account_id)
            .fetch_one(pool)
            .await?;
            let (courses,): (i64,) = sqlx::query_as(
                "WITH RECURSIVE down AS ( \
                     SELECT id FROM accounts WHERE id = $1 \
                     UNION ALL \
                     SELECT a.id FROM accounts a JOIN down ON a.parent_account_id = down.id \
                 ) SELECT COUNT(*) FROM courses WHERE account_id IN (SELECT id FROM down) \
                   AND workflow_state <> 'deleted'",
            )
            .bind(account_id)
            .fetch_one(pool)
            .await?;
            let (children,): (i64,) = sqlx::query_as(
                "WITH RECURSIVE down AS ( \
                     SELECT id FROM accounts WHERE id = $1 \
                     UNION ALL \
                     SELECT a.id FROM accounts a JOIN down ON a.parent_account_id = down.id \
                 ) SELECT COUNT(*) FROM lti_context_controls c \
                 JOIN accounts a ON c.account_id = a.id \
                 WHERE a.id IN (SELECT id FROM down) AND c.id <> $2 \
                   AND c.deployment_id = $3 AND c.workflow_state <> 'deleted'",
            )
            .bind(account_id)
            .bind(control.id)
            .bind(control.deployment_id)
            .fetch_one(pool)
            .await?;
            (
                name.map(|n| json!(n)).unwrap_or(Value::Null),
                display,
                path,
                subaccounts,
                courses,
                children,
            )
        } else {
            let name = sqlx::query_as::<_, (String,)>("SELECT name FROM courses WHERE id = $1")
                .bind(control.course_id)
                .fetch_optional(pool)
                .await?
                .map(|(name,)| name);
            (
                name.map(|n| json!(n)).unwrap_or(Value::Null),
                vec![],
                String::new(),
                0,
                0,
                0,
            )
        };

    let mut json = json!({
        "id": control.id,
        "account_id": control.account_id,
        "course_id": control.course_id,
        "registration_id": control.registration_id,
        "deployment_id": control.deployment_id,
        "workflow_state": control.workflow_state,
        "created_at": control.created_at,
        "updated_at": control.updated_at,
        "available": control.available,
        "path": path,
        "context_name": context_name,
        "child_control_count": child_control_count,
        "subaccount_count": subaccount_count,
        "course_count": course_count,
        "display_path": display_path,
        "depth": if control.account_id.is_some() { 1 } else { 0 },
        "created_by": Value::Null,
        "updated_by": Value::Null,
    });
    if let Some(user) = user_summary(pool, control.created_by_id).await? {
        json["created_by"] = user;
    }
    if let Some(user) = user_summary(pool, control.updated_by_id).await? {
        json["updated_by"] = user;
    }
    Ok(json)
}

/// Insert (or reactivate) a control — Canvas
/// `Lti::ContextControlService.create_or_update`.
#[allow(clippy::too_many_arguments)]
async fn upsert_control(
    pool: &PgPool,
    registration_id: i64,
    deployment_id: i64,
    account_id: Option<i64>,
    course_id: Option<i64>,
    root_account_id: i64,
    available: bool,
    user_id: i64,
) -> Result<ContextControlRow, ApiError> {
    // Canvas' create_or_update semantics: an existing active control for the
    // same deployment and context is updated rather than duplicated. The
    // select-then-write keeps the semantics without relying on partial
    // unique-index inference.
    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_context_controls \
         WHERE deployment_id = $1 AND account_id IS NOT DISTINCT FROM $2 \
           AND course_id IS NOT DISTINCT FROM $3 AND workflow_state <> 'deleted'",
    )
    .bind(deployment_id)
    .bind(account_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    if let Some((existing_id,)) = existing {
        let sql = format!(
            "UPDATE lti_context_controls \
             SET available = $2, updated_by_id = $3, updated_at = now() \
             WHERE id = $1 \
             RETURNING {CONTROL_COLUMNS}"
        );
        let row: ContextControlRow = sqlx::query_as(&sql)
            .bind(existing_id)
            .bind(available)
            .bind(user_id)
            .fetch_one(pool)
            .await?;
        return Ok(row);
    }

    let sql = format!(
        "INSERT INTO lti_context_controls \
         (registration_id, deployment_id, account_id, course_id, root_account_id, available, \
          workflow_state, created_by_id, updated_by_id) \
         VALUES ($1, $2, $3, $4, $5, $6, 'active', $7, $7) \
         RETURNING {CONTROL_COLUMNS}"
    );
    let row: ContextControlRow = sqlx::query_as(&sql)
        .bind(registration_id)
        .bind(deployment_id)
        .bind(account_id)
        .bind(course_id)
        .bind(root_account_id)
        .bind(available)
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET .../controls` — Canvas groups the registration's controls by
/// deployment (context_controls_controller.rb:232-270).
async fn index_controls(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let registration = find_active_registration(&state.db, registration_id).await?;
    let root_id = root_account_id(&state.db, account_id).await?;

    let controls: Vec<ContextControlRow> = sqlx::query_as(&format!(
        "SELECT {CONTROL_COLUMNS} FROM lti_context_controls \
         WHERE registration_id = $1 AND root_account_id = $2 AND workflow_state <> 'deleted' \
         ORDER BY id ASC"
    ))
    .bind(registration.id)
    .bind(root_id)
    .fetch_all(&state.db)
    .await?;

    let mut by_deployment: Vec<(i64, Vec<&ContextControlRow>)> = Vec::new();
    for control in &controls {
        match by_deployment
            .iter_mut()
            .find(|(id, _)| *id == control.deployment_id)
        {
            Some((_, list)) => list.push(control),
            None => by_deployment.push((control.deployment_id, vec![control])),
        }
    }

    let mut deployments = Vec::with_capacity(by_deployment.len());
    for (deployment_id, list) in &by_deployment {
        let sql = format!(
            "SELECT {crate::DEPLOYMENT_COLUMNS} FROM lti_deployments \
             WHERE id = $1 AND workflow_state <> '{STATE_DELETED}'"
        );
        let deployment: Option<DeploymentRow> = sqlx::query_as(&sql)
            .bind(deployment_id)
            .fetch_optional(&state.db)
            .await?;
        let Some(deployment) = deployment else {
            continue;
        };
        let mut json = deployment_json(&deployment, false);
        let mut control_jsons = Vec::with_capacity(list.len());
        for control in list {
            control_jsons.push(control_json(&state.db, control).await?);
        }
        json["context_controls"] = json!(control_jsons);
        deployments.push(json);
    }
    Ok(Json(deployments))
}

/// `POST .../controls` — create a control
/// (context_controls_controller.rb:304-357).
async fn create_control(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateControlRequest>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let registration = find_active_registration(&state.db, registration_id).await?;
    let root_id = root_account_id(&state.db, account_id).await?;

    let deployment = match payload.deployment_id {
        Some(deployment_id) => {
            let Some(deployment) =
                find_active_deployment(&state.db, registration.id, deployment_id)
                    .await
                    .ok()
            else {
                return Ok(render_errors(vec!["deployment_id is invalid".to_string()]));
            };
            deployment
        }
        None => {
            let deployment = root_account_deployment(&state.db, registration.id, root_id).await?;
            let Some(deployment) = deployment else {
                return Ok(render_errors(vec![
                    "No active deployment found for the root account.".to_string(),
                ]));
            };
            deployment
        }
    };

    if payload.account_id.is_none() && payload.course_id.is_none() {
        return Ok(render_errors(vec![
            "Either account_id or course_id must be present.".to_string(),
        ]));
    }
    if payload.account_id.is_some() && payload.course_id.is_some() {
        return Ok(render_errors(vec![
            "Either account_id or course_id must be present.".to_string(),
        ]));
    }

    // The context must exist (Canvas validates against real contexts).
    if let Some(control_account_id) = payload.account_id {
        let exists: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM accounts WHERE id = $1 AND workflow_state <> 'deleted'")
                .bind(control_account_id)
                .fetch_optional(&state.db)
                .await?;
        if exists.is_none() {
            return Ok(render_errors(vec!["account_id is invalid".to_string()]));
        }
    }
    if let Some(course_id) = payload.course_id {
        let exists: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
                .bind(course_id)
                .fetch_optional(&state.db)
                .await?;
        if exists.is_none() {
            return Ok(render_errors(vec!["course_id is invalid".to_string()]));
        }
    }

    let duplicate: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_context_controls \
         WHERE deployment_id = $1 AND account_id IS NOT DISTINCT FROM $2 \
           AND course_id IS NOT DISTINCT FROM $3 AND workflow_state <> 'deleted'",
    )
    .bind(deployment.id)
    .bind(payload.account_id)
    .bind(payload.course_id)
    .fetch_optional(&state.db)
    .await?;
    if duplicate.is_some() {
        return Ok(render_errors(vec![
            "A context control for this deployment and context already exists.".to_string(),
        ]));
    }

    let control = upsert_control(
        &state.db,
        registration.id,
        deployment.id,
        payload.account_id,
        payload.course_id,
        root_id,
        payload.available.unwrap_or(true),
        auth.id,
    )
    .await?;

    record_history_entry(
        &state.db,
        registration.id,
        root_id,
        "control_created",
        json!({ "control_id": control.id, "available": control.available }),
        payload.comment.as_deref(),
        None,
        None,
        auth.id,
    )
    .await?;

    let body = control_json(&state.db, &control).await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

/// `POST .../controls/bulk` — create up to 100 controls at once
/// (context_controls_controller.rb:359-434); existing controls are updated.
async fn create_controls_bulk(
    State(state): State<AppState>,
    Path((account_id, registration_id)): Path<(i64, i64)>,
    auth: AuthUser,
    payload: Result<Json<Vec<BulkControlItem>>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let registration = find_active_registration(&state.db, registration_id).await?;
    let root_id = root_account_id(&state.db, account_id).await?;

    let items = match payload {
        Ok(Json(items)) => items,
        Err(_) => {
            return Ok(render_errors(vec![
                "Invalid parameters. Expected an array of context control parameters.".to_string(),
            ]))
        }
    };
    if items.is_empty() {
        return Ok(render_errors(vec![
            "Invalid parameters. Expected an array of context control parameters.".to_string(),
        ]));
    }
    if items.len() > MAX_BULK_CREATE {
        return Ok(render_errors(vec![format!(
            "Cannot create more than {MAX_BULK_CREATE} context controls at once"
        )]));
    }
    if items
        .iter()
        .any(|item| item.account_id.is_none() && item.course_id.is_none())
    {
        return Ok(render_errors(vec![
            "Either account_id or course_id must be present for each context control.".to_string(),
        ]));
    }
    if items
        .iter()
        .any(|item| item.account_id.is_some() && item.course_id.is_some())
    {
        return Ok(render_errors(vec![
            "Either account_id or course_id must be present for each context control, but not both."
                .to_string(),
        ]));
    }

    let root_deployment = root_account_deployment(&state.db, registration.id, root_id).await?;
    if items.iter().any(|item| item.deployment_id.is_none()) && root_deployment.is_none() {
        return Ok(render_errors(vec![
            "No active deployment found for the root account. Please specify a deployment_id for each control."
                .to_string(),
        ]));
    }

    // Duplicate contexts inside the payload are rejected (Canvas
    // validate_bulk_params, :617-620).
    let mut seen: Vec<(Option<i64>, Option<i64>)> = Vec::new();
    for item in &items {
        let key = (item.account_id, item.course_id);
        if seen.contains(&key) {
            return Ok(render_errors(vec![
                "Cannot create multiple context controls for the same context. Please specify unique account_id or course_id for each context control."
                    .to_string(),
            ]));
        }
        seen.push(key);
    }

    let mut created = Vec::with_capacity(items.len());
    for item in items {
        let deployment_id = item
            .deployment_id
            .unwrap_or_else(|| root_deployment.as_ref().map(|d| d.id).unwrap_or_default());
        let Some(deployment) = find_active_deployment(&state.db, registration.id, deployment_id)
            .await
            .ok()
        else {
            return Ok(render_errors(vec!["deployment_id is invalid".to_string()]));
        };
        let control = upsert_control(
            &state.db,
            registration.id,
            deployment.id,
            item.account_id,
            item.course_id,
            root_id,
            item.available.unwrap_or(true),
            auth.id,
        )
        .await?;
        created.push(control_json(&state.db, &control).await?);
    }

    record_history_entry(
        &state.db,
        registration.id,
        root_id,
        "controls_bulk_created",
        json!({ "count": created.len() }),
        None,
        None,
        None,
        auth.id,
    )
    .await?;

    Ok((StatusCode::CREATED, Json(created)).into_response())
}

/// `GET .../controls/{id}` (context_controls_controller.rb:273-285).
async fn show_control(
    State(state): State<AppState>,
    Path((account_id, registration_id, id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let registration = find_active_registration(&state.db, registration_id).await?;
    let root_id = root_account_id(&state.db, account_id).await?;
    let control = find_control(&state.db, registration.id, root_id, id).await?;
    Ok(Json(control_json(&state.db, &control).await?))
}

/// `PUT .../controls/{id}` — flip availability
/// (context_controls_controller.rb:503-516).
async fn update_control(
    State(state): State<AppState>,
    Path((account_id, registration_id, id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateControlRequest>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let registration = find_active_registration(&state.db, registration_id).await?;
    let root_id = root_account_id(&state.db, account_id).await?;
    let control = find_control(&state.db, registration.id, root_id, id).await?;

    // Canvas `params.require(:available)` answers 400 when missing.
    let Some(available) = payload.available else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "errors": ["available is required"] })),
        )
            .into_response());
    };

    let sql = format!(
        "UPDATE lti_context_controls SET available = $2, updated_by_id = $3, updated_at = now() \
         WHERE id = $1 \
         RETURNING {CONTROL_COLUMNS}"
    );
    let updated: Option<ContextControlRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(available)
        .bind(auth.id)
        .fetch_optional(&state.db)
        .await?;
    let Some(updated) = updated else {
        return Err(ApiError::NotFound);
    };

    record_history_entry(
        &state.db,
        registration.id,
        root_id,
        "control_updated",
        json!({ "control_id": id, "available": available }),
        payload.comment.as_deref(),
        None,
        None,
        auth.id,
    )
    .await?;

    Ok(Json(control_json(&state.db, &updated).await?).into_response())
}

/// `DELETE .../controls/{id}` — delete a control; the "primary" control (the
/// one on the deployment's own context) may not be deleted
/// (context_controls_controller.rb:533-548).
async fn delete_control(
    State(state): State<AppState>,
    Path((account_id, registration_id, id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Query(params): Query<CommentQuery>,
) -> Result<Response, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let registration = find_active_registration(&state.db, registration_id).await?;
    let root_id = root_account_id(&state.db, account_id).await?;
    let control = find_control(&state.db, registration.id, root_id, id).await?;

    let deployment_sql =
        format!("SELECT {crate::DEPLOYMENT_COLUMNS} FROM lti_deployments WHERE id = $1");
    let deployment: Option<DeploymentRow> = sqlx::query_as(&deployment_sql)
        .bind(control.deployment_id)
        .fetch_optional(&state.db)
        .await?;
    if let Some(deployment) = deployment {
        let is_primary = deployment.context_type == "Account"
            && control.account_id == Some(deployment.context_id);
        if is_primary {
            return Ok(render_errors(vec![
                "Cannot delete the primary context control for a deployment.".to_string(),
            ]));
        }
    }

    sqlx::query(
        "UPDATE lti_context_controls SET workflow_state = 'deleted', updated_at = now() \
         WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    record_history_entry(
        &state.db,
        registration.id,
        root_id,
        "control_deleted",
        json!({ "control_id": id }),
        params.comment.as_deref(),
        None,
        None,
        auth.id,
    )
    .await?;

    Ok(Json(control_json(&state.db, &control).await?).into_response())
}

/// Query params carrying only a change-log comment.
#[derive(Debug, Deserialize, Default)]
pub struct CommentQuery {
    pub comment: Option<String>,
}

/// `GET .../deployments/{deployment_id}/controls` — the controls of one
/// deployment (deployments_controller.rb:197-210).
async fn list_deployment_controls(
    State(state): State<AppState>,
    Path((account_id, registration_id, deployment_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_account(&state.db, account_id).await?;
    if !can_manage_account(&state.db, &auth, account_id).await? {
        return Err(ApiError::Forbidden);
    }
    let registration = find_active_registration(&state.db, registration_id).await?;
    let _ = find_active_deployment(&state.db, registration.id, deployment_id).await?;
    let root_id = root_account_id(&state.db, account_id).await?;

    let controls: Vec<ContextControlRow> = sqlx::query_as(&format!(
        "SELECT {CONTROL_COLUMNS} FROM lti_context_controls \
         WHERE deployment_id = $1 AND root_account_id = $2 AND workflow_state <> 'deleted' \
         ORDER BY id ASC"
    ))
    .bind(deployment_id)
    .bind(root_id)
    .fetch_all(&state.db)
    .await?;

    let mut jsons = Vec::with_capacity(controls.len());
    for control in &controls {
        jsons.push(control_json(&state.db, control).await?);
    }
    Ok(Json(jsons))
}
