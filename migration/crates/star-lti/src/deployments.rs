//! lti/deployments — Canvas `Lti::DeploymentsController` parity.
//!
//! Route (the crate router mounts this module under `/api/v1`):
//! - `GET    /accounts/{accountId}/lti_registrations/{registrationId}/deployments/{id}/controls`
//!
//! The `show` and `destroy` actions of the controller are served by the
//! scoped deployment handlers of `lib.rs` (`show_deployment_scoped` /
//! `delete_deployment_scoped`), so only `list_controls` lives here.
//!
//! Canvas routes (config/routes.rb, the account API block):
//!   get    "accounts/:account_id/lti_registrations/:registration_id/deployments/:id", action: :show
//!   delete "accounts/:account_id/lti_registrations/:registration_id/deployments/:id", action: :destroy
//!   get    "accounts/:account_id/lti_registrations/:registration_id/deployments/:id/controls", action: :list_controls
//!
//! Canvas controller (`app/controllers/lti/deployments_controller.rb`): the
//! `before_action` chain is `require_account_context`, `require_root_account`
//! (404), `require_feature_flag` (404) and `require_manage_lti_registrations`
//! (403). The deployment itself is looked up in the registration and in the
//! account — a deployment that does not belong answers 404.
//!
//! The deployment JSON is the Canvas `Api::V1::Lti::Deployment` shape: the
//! model attributes plus `registration_id`, `deployment_id`, `context_name`,
//! `workflow_state` and `root_account_deployment`. A disabled deployment is
//! reported as "disabled", a deleted one as "deleted" and every other state
//! as "active".

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// The columns of an `lti_deployments` row that the controls routes read.
const DEPLOYMENT_COLUMNS: &str = "id";

/// A deployment row.
#[derive(Debug, sqlx::FromRow)]
struct DeploymentRow {
    id: i64,
}

/// Domain router for the Canvas deployment routes.
pub fn router() -> Router<AppState> {
    Router::new().route(
        "/accounts/{account_id}/lti_registrations/{registration_id}/deployments/{deployment_id}/controls",
        get(list_controls),
    )
}

/// Canvas `Lti::DeploymentsController#list_controls`.
async fn list_controls(
    State(state): State<AppState>,
    Path((account_id, registration_id, deployment_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let row = load_deployment(&state.db, account_id, registration_id, deployment_id).await?;
    authorize(&state.db, &auth, account_id).await?;

    // The context controls of the deployment.
    let controls: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM lti_context_controls \
         WHERE deployment_id = $1 AND workflow_state = 'active' ORDER BY id ASC",
    )
    .bind(row.id)
    .fetch_all(&state.db)
    .await?;

    let mut rendered = Vec::with_capacity(controls.len());
    for (control_id,) in controls {
        let control = context_control_json(&state.db, control_id).await?;
        rendered.push(control);
    }
    Ok(Json(rendered).into_response())
}

/// The deployment of a registration in an account — 404 when it does not
/// belong.
async fn load_deployment(
    db: &PgPool,
    account_id: i64,
    registration_id: i64,
    deployment_id: i64,
) -> Result<DeploymentRow, ApiError> {
    // The account has to be a root account.
    let root: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(db)
            .await?;
    match root {
        Some((None,)) => {}
        _ => return Err(ApiError::NotFound),
    }

    // The registration has to belong to the account.
    let registration: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM lti_registrations WHERE id = $1 AND account_id = $2 LIMIT 1")
            .bind(registration_id)
            .bind(account_id)
            .fetch_optional(db)
            .await?;
    if registration.is_none() {
        return Err(ApiError::NotFound);
    }

    let sql = format!(
        "SELECT {DEPLOYMENT_COLUMNS} FROM lti_deployments \
         WHERE id = $1 AND registration_id = $2 LIMIT 1"
    );
    let row: Option<DeploymentRow> = sqlx::query_as(&sql)
        .bind(deployment_id)
        .bind(registration_id)
        .fetch_optional(db)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// Canvas `require_manage_lti_registrations`.
async fn authorize(db: &PgPool, auth: &AuthUser, account_id: i64) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM account_users \
         WHERE account_id = $1 AND user_id = $2 AND role <> 'student' LIMIT 1",
    )
    .bind(account_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// A row of the `lti_context_controls` table.
#[derive(Debug, sqlx::FromRow)]
struct ContextControlRow {
    id: i64,
    account_id: Option<i64>,
    course_id: Option<i64>,
    registration_id: i64,
    deployment_id: i64,
    workflow_state: String,
    available: bool,
}

/// The context control JSON of a control id.
async fn context_control_json(db: &PgPool, id: i64) -> Result<Value, ApiError> {
    let row: Option<ContextControlRow> = sqlx::query_as(
        "SELECT id, account_id, course_id, registration_id, deployment_id, workflow_state, available \
         FROM lti_context_controls WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Ok(Value::Null);
    };

    Ok(json!({
        "id": row.id,
        "account_id": row.account_id,
        "course_id": row.course_id,
        "registration_id": row.registration_id,
        "deployment_id": row.deployment_id,
        "workflow_state": row.workflow_state,
        "available": row.available,
        "path": "",
        "context_name": "",
        "child_control_count": 0,
        "subaccount_count": 0,
        "course_count": 0,
        "display_path": "",
        "depth": 0,
        "created_by": Value::Null,
        "updated_by": Value::Null,
    }))
}
