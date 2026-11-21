//! Routes V2 — strand management, unstuck, and throttle-check.
//!
//! Canvas: `app/controllers/jobs_v2_controller.rb`.
//! Routes:
//! - `PUT /api/v1/jobs2/manage`         → manage_strand
//! - `PUT /api/v1/jobs2/unstuck`         → unstuck_strand
//! - `GET /api/v1/jobs2/throttle/check`  → throttle_check

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::require_admin;

/// Request body for `PUT /jobs2/manage`.
#[derive(Debug, Deserialize)]
pub struct ManageRequest {
    pub strand: Option<String>,
    pub max_concurrent: Option<i32>,
    pub priority: Option<i32>,
}

/// Request body for `PUT /jobs2/unstuck`.
#[derive(Debug, Deserialize)]
pub struct UnstuckRequest {
    pub strand: Option<String>,
    pub singleton: Option<String>,
    pub job_shards: Option<Vec<i64>>,
}

/// Query params for `GET /jobs2/throttle/check`.
#[derive(Debug, Deserialize)]
pub struct ThrottleCheckQuery {
    pub term: String,
    pub shard_id: Option<i64>,
}

/// Response for `PUT /jobs2/manage`.
#[derive(Debug, Serialize)]
pub struct ManageResponse {
    pub status: String,
    pub count: i64,
}

/// `PUT /api/v1/jobs2/manage` — update strand concurrency/priority.
///
/// Canvas: `JobsV2Controller#manage` — updates every job in the strand;
/// `max_concurrent` and `priority` are only written when present.
pub async fn manage_strand(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<ManageRequest>,
) -> Result<Json<ManageResponse>, ApiError> {
    require_admin(&auth)?;

    // Canvas: `render json: { message: "missing strand" }, status: :bad_request`.
    // The field is optional so a missing key is answered here (400) rather
    // than by the JSON extractor (422).
    let Some(strand) = payload
        .strand
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Err(ApiError::BadRequest("missing strand".into()));
    };

    // Canvas only includes the fields that were sent in update_all.
    let mut sets = Vec::new();
    if let Some(max_concurrent) = payload.max_concurrent {
        sets.push(format!("max_concurrent = {}", max_concurrent.max(1)));
    }
    if let Some(priority) = payload.priority {
        sets.push(format!("priority = {priority}"));
    }
    if sets.is_empty() {
        return Ok(Json(ManageResponse {
            status: "OK".to_string(),
            count: 0,
        }));
    }
    sets.push("updated_at = NOW()".to_string());

    let sql = format!("UPDATE jobs SET {} WHERE strand = $1", sets.join(", "));

    let updated = sqlx::query(&sql)
        .bind(strand)
        .execute(&state.db)
        .await?;

    Ok(Json(ManageResponse {
        status: "OK".to_string(),
        count: updated.rows_affected() as i64,
    }))
}

/// `PUT /api/v1/jobs2/unstuck` — unstick a strand, singleton, or all shards.
///
/// Canvas: `JobsV2Controller#unstuck`.
/// Three modes:
/// 1. `strand` provided → unblock the strand (set next_in_strand = true).
/// 2. `singleton` provided → unblock the singleton.
/// 3. Neither → create a progress record and return 202 (no background worker).
pub async fn unstuck_strand(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<UnstuckRequest>,
) -> Result<Response, ApiError> {
    require_admin(&auth)?;

    if let Some(ref strand) = payload.strand {
        // Mode 1: unstick by strand.
        let updated = sqlx::query(
            r#"
            UPDATE jobs
            SET next_in_strand = TRUE,
                locked_by = NULL,
                locked_at = NULL,
                updated_at = NOW()
            WHERE strand = $1
              AND workflow_state = 'queued'
            "#,
        )
        .bind(strand)
        .execute(&state.db)
        .await?;

        let count = updated.rows_affected();
        return Ok(Json(json!({
            "status": if count > 0 { "OK" } else { "blocked" },
            "count": count
        }))
        .into_response());
    }

    if let Some(ref singleton) = payload.singleton {
        // Mode 2: unstick by singleton.
        let updated = sqlx::query(
            r#"
            UPDATE jobs
            SET next_in_strand = TRUE,
                locked_by = NULL,
                locked_at = NULL,
                updated_at = NOW()
            WHERE singleton = $1
              AND workflow_state = 'queued'
            "#,
        )
        .bind(singleton)
        .execute(&state.db)
        .await?;

        let count = updated.rows_affected();
        return Ok(Json(json!({
            "status": if count > 0 { "OK" } else { "blocked" },
            "count": count
        }))
        .into_response());
    }

    // Mode 3: neither strand nor singleton — create a progress record (no worker).
    let progress_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO progress (context_id, context_type, tag, workflow_state, completion, created_at, updated_at)
        VALUES ($1, 'User', 'JobsV2Controller::run_unstucker!', 'queued', 0, NOW(), NOW())
        RETURNING id
        "#,
    )
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    let progress = sqlx::query_as::<_, ProgressRow>(
        "SELECT id, context_id, context_type, tag, workflow_state, completion, created_at, updated_at FROM progress WHERE id = $1"
    )
    .bind(progress_id)
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "pending",
            "progress": {
                "id": progress.id,
                "context_id": progress.context_id,
                "context_type": progress.context_type,
                "tag": progress.tag,
                "workflow_state": progress.workflow_state,
                "completion": progress.completion,
                "created_at": progress.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "updated_at": progress.updated_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            }
        })),
    )
    .into_response())
}

#[derive(Debug, sqlx::FromRow)]
struct ProgressRow {
    id: i64,
    context_id: i64,
    context_type: String,
    tag: String,
    workflow_state: String,
    completion: i32,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// `GET /api/v1/jobs2/throttle/check` — check throttle for a tag prefix.
///
/// Canvas: `JobsV2Controller#throttle_check`.
/// Counts queued, unstranded, unsingletoned, unlocked jobs whose tag starts with `term`.
pub async fn throttle_check(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ThrottleCheckQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&auth)?;

    if params.term.trim().is_empty() {
        return Err(ApiError::BadRequest("term is required".into()));
    }

    // Count jobs matching: queued, no strand, no singleton, not locked, tag LIKE 'term%'.
    let term_pattern = format!("{}%", params.term.replace('\'', "''"));

    let sql = if params.shard_id.is_some() {
        r#"
        SELECT COUNT(*) AS matched_jobs,
               COUNT(DISTINCT tag) AS matched_tags
        FROM jobs
        WHERE workflow_state = 'queued'
          AND run_at <= now()
          AND locked_by IS NULL
          AND locked_at IS NULL
          AND (strand IS NULL OR strand = '')
          AND (singleton IS NULL OR singleton = '')
          AND tag LIKE $1
          AND ($2::bigint IS NULL OR shard_id = $2)
        "#
    } else {
        r#"
        SELECT COUNT(*) AS matched_jobs,
               COUNT(DISTINCT tag) AS matched_tags
        FROM jobs
        WHERE workflow_state = 'queued'
          AND run_at <= now()
          AND locked_by IS NULL
          AND locked_at IS NULL
          AND (strand IS NULL OR strand = '')
          AND (singleton IS NULL OR singleton = '')
          AND tag LIKE $1
        "#
    };

    let mut qb = sqlx::query_as::<_, ThrottleRow>(sql).bind(&term_pattern);
    if let Some(shard) = params.shard_id {
        qb = qb.bind(shard);
    }

    let row: ThrottleRow = qb.fetch_one(&state.db).await?;

    Ok(Json(json!({
        "matched_jobs": row.matched_jobs,
        "matched_tags": row.matched_tags
    })))
}

#[derive(Debug, sqlx::FromRow)]
struct ThrottleRow {
    matched_jobs: i64,
    matched_tags: i64,
}

/// Build the V2 management routes sub-router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs2/manage", put(manage_strand))
        .route("/jobs2/unstuck", put(unstuck_strand))
        .route("/jobs2/throttle/check", get(throttle_check))
}
