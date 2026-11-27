//! Outcome parity gaps — P5 port.
//!
//! - `outcome_groups_api`: the account-context root redirect and the account
//!   subgroup create (`OutcomeGroupsApiController#redirect` / `#create`).
//!   The course-context and global siblings already live in `lib.rs`; these
//!   reuse the same helpers (`ensure_root_outcome_group`,
//!   `fetch_account_outcome_group`, `group_json`).
//! - `outcomes_academic_benchmark_import_api`
//!   (`OutcomesAcademicBenchmarkImportApiController`): the Academic
//!   Benchmarks import family. Canvas fronts AB migrations; the sync stance
//!   here records a completed `outcome_ab_migrations` row (migration 0254)
//!   and answers with its id / status.

use axum::extract::{Path, State};
use axum::http::{header::LOCATION, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{
    account_exists, can_access_account, ensure_root_outcome_group, fetch_account_outcome_group,
    group_json, is_admin, CreateOutcomeGroup, LearningOutcomeGroup,
};

// ---------------------------------------------------------------------------
// outcome_groups_api — account root redirect
// ---------------------------------------------------------------------------

/// GET /accounts/{accountId}/root_outcome_group — Canvas:
/// outcome_groups_api_controller#redirect —
/// GET /api/v1/accounts/:account_id/root_outcome_group.
///
/// 302 to the account root group's canonical URL (the group JSON rides in
/// the body for clients that do not follow redirects). Like the course and
/// global redirects in `lib.rs`, the root group is created on first access,
/// titled after the account (Canvas `context.root_outcome_group`).
async fn account_root_outcome_group(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !can_access_account(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let name: Option<String> = sqlx::query_scalar("SELECT name FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(&state.db)
        .await?;
    let name = name.ok_or(ApiError::NotFound)?;

    let group = ensure_root_outcome_group(&state.db, "Account", account_id, &name).await?;
    let can_edit = can_access_account(&state.db, auth.id, account_id, &auth).await?;
    let location = format!("/api/v1/accounts/{account_id}/outcome_groups/{}", group.id);

    Ok((
        StatusCode::FOUND,
        [(LOCATION, location)],
        Json(group_json(&group, can_edit)),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// outcome_groups_api — account subgroup create
// ---------------------------------------------------------------------------

/// POST /accounts/{accountId}/outcome_groups/{id}/subgroups — Canvas:
/// outcome_groups_api_controller#create —
/// POST /api/v1/accounts/:account_id/outcome_groups/:id/subgroups.
///
/// Mirrors the course subgroup create (`lib.rs`) for the account context:
/// child of the addressed group, owned by the account, `can_edit` for the
/// account admins that may manage it. Registered on the existing subgroups
/// route in `lib.rs` (next to the GET).
pub(crate) async fn create_account_outcome_subgroup(
    State(state): State<AppState>,
    Path((account_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateOutcomeGroup>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    let can_manage = can_access_account(&state.db, auth.id, account_id, &auth).await?;
    if !can_manage {
        return Err(ApiError::Forbidden);
    }

    fetch_account_outcome_group(&state.db, account_id, group_id).await?;
    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }

    let row: LearningOutcomeGroup = sqlx::query_as(
        r#"
        INSERT INTO outcome_groups
            (course_id, context_type, context_id, parent_outcome_group_id,
             title, description, vendor_guid)
        VALUES (NULL, 'Account', $1, $2, $3, $4, $5)
        RETURNING id, course_id, context_type, context_id, parent_outcome_group_id,
                  title, description, vendor_guid, workflow_state, created_at, updated_at
        "#,
    )
    .bind(account_id)
    .bind(group_id)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.vendor_guid)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(group_json(&row, can_manage))))
}

// ---------------------------------------------------------------------------
// outcomes_academic_benchmark_import_api
// ---------------------------------------------------------------------------

/// GET /global/outcomes_import/available — Canvas:
/// outcomes_academic_benchmark_import_api_controller#available —
/// GET /api/v1/global/outcomes_import/available.
///
/// Canvas fronts Academic Benchmarks; the workspace reports the importer as
/// available (the sync migration stance below always succeeds).
async fn outcomes_import_available(auth: AuthUser) -> Result<Json<Value>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(json!({ "available": true })))
}

/// POST /global/outcomes_import — Canvas:
/// outcomes_academic_benchmark_import_api_controller#create —
/// POST /api/v1/global/outcomes_import (admin).
///
/// Canvas starts an async AB migration; the sync stance records a completed
/// migration row (`outcome_ab_migrations`, migration 0254) and answers with
/// its id.
async fn create_outcomes_import(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO outcome_ab_migrations (workflow_state) VALUES ('completed') RETURNING id",
    )
    .fetch_one(&state.db)
    .await?;
    Ok(Json(json!({ "migration_id": id })))
}

/// GET /global/outcomes_import/migration_status/{migrationId} — Canvas:
/// outcomes_academic_benchmark_import_api_controller#migration_status —
/// GET /api/v1/global/outcomes_import/migration_status/:migration_id.
async fn outcomes_import_migration_status(
    State(state): State<AppState>,
    Path(migration_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let row: Option<(String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT workflow_state, created_at FROM outcome_ab_migrations WHERE id = $1",
    )
    .bind(migration_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((workflow_state, created_at)) = row else {
        return Err(ApiError::NotFound);
    };
    Ok(Json(json!({
        "migration_status": {
            "id": migration_id,
            "workflow_state": workflow_state,
            "created_at": created_at,
        }
    })))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Mount the P5 outcome gap routes (merged into the crate router in
/// `lib.rs`; the account subgroup POST is added to the existing subgroups
/// route there).
pub fn outcome_gaps_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{accountId}/root_outcome_group",
            get(account_root_outcome_group),
        )
        .route(
            "/global/outcomes_import/available",
            get(outcomes_import_available),
        )
        .route("/global/outcomes_import", post(create_outcomes_import))
        .route(
            "/global/outcomes_import/migration_status/{migrationId}",
            get(outcomes_import_migration_status),
        )
}
