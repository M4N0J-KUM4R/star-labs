//! Routes V1 — JobsController JSON-mode endpoints.
//!
//! Canvas: `app/controllers/jobs_controller.rb`.
//! Routes: `GET /api/v1/jobs`, `GET /api/v1/jobs/:id`, `POST /api/v1/jobs/batch_update`.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{require_admin, JobV2, JOB_V2_COLUMNS};

/// Query params for `GET /api/v1/jobs`.
#[derive(Debug, Deserialize)]
pub struct ListJobsJsonQuery {
    /// One of `running`, `tags`, `jobs`. Default returns jobs.
    pub only: Option<String>,
    /// One of `current`, `future`, `failed`, `id`, or free-text query.
    pub flavor: Option<String>,
    /// Max rows to return (default 100, max 100).
    pub limit: Option<i64>,
    /// Row offset (default 0).
    pub offset: Option<i64>,
    /// Search query when `flavor` is not a named bucket.
    pub q: Option<String>,
}

/// Top-level response shapes for the V1 JSON surface.
#[derive(Debug, Serialize)]
pub struct JobsResponse {
    pub jobs: Vec<Value>,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct RunningResponse {
    pub running: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub struct TagsResponse {
    pub tags: Vec<TagCount>,
}

#[derive(Debug, Serialize)]
pub struct TagCount {
    pub tag: String,
    pub count: i64,
}

/// Response for `POST /jobs/batch_update`.
#[derive(Debug, Serialize)]
pub struct BatchUpdateResponse {
    pub status: String,
    pub count: i64,
}

/// Request body for `POST /jobs/batch_update`.
#[derive(Debug, Deserialize)]
pub struct BatchUpdateRequest {
    pub update_action: String,
    pub job_ids: Option<Vec<i64>>,
    pub flavor: Option<String>,
    pub q: Option<String>,
}

/// `GET /api/v1/jobs` — list jobs with `only`/`flavor` modes (site admin only).
///
/// Canvas: `JobsController#index` (JSON mode).
pub async fn list_jobs_json(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListJobsJsonQuery>,
) -> Result<Response, ApiError> {
    require_admin(&auth)?;

    let only = params.only.as_deref().unwrap_or("jobs");
    let flavor = params.flavor.as_deref().unwrap_or("current");
    let limit = params.limit.unwrap_or(100).clamp(1, 100);
    let offset = params.offset.unwrap_or(0).max(0);

    match only {
        "running" => list_running(&state.db, flavor)
            .await
            .map(IntoResponse::into_response),
        "tags" => list_tags(&state.db, flavor, limit)
            .await
            .map(IntoResponse::into_response),
        _ => list_jobs(&state.db, flavor, &params.q, limit, offset)
            .await
            .map(IntoResponse::into_response),
    }
}

/// Fetch running jobs (flavor-scope aware).
async fn list_running(db: &PgPool, flavor: &str) -> Result<impl IntoResponse, ApiError> {
    let where_clause = running_where_clause(flavor);

    let sql = format!("SELECT {JOB_V2_COLUMNS} FROM jobs WHERE {where_clause} ORDER BY id DESC");
    let rows: Vec<JobV2> = sqlx::query_as(&sql).fetch_all(db).await?;

    let jobs: Vec<Value> = rows.iter().map(|j| job_v2_to_json(j, false)).collect();

    Ok(Json(RunningResponse { running: jobs }))
}

/// Build the WHERE clause for running jobs filtered by flavor.
fn running_where_clause(flavor: &str) -> String {
    let base = "workflow_state = 'running' OR locked_at IS NOT NULL";
    match flavor {
        "failed" => "workflow_state = 'failed' OR failed_at IS NOT NULL".to_string(),
        "future" => {
            format!("{base} AND run_at > now()")
        }
        "current" | "id" | _ => base.to_string(),
    }
    .to_string()
}

/// Fetch tags grouped by count (top 12 by default).
async fn list_tags(db: &PgPool, flavor: &str, limit: i64) -> Result<impl IntoResponse, ApiError> {
    let where_clause = flavor_where_clause(flavor);

    let sql = format!(
        r#"
        SELECT tag, COUNT(*) AS count
        FROM jobs
        WHERE {where_clause}
        GROUP BY tag
        ORDER BY count DESC
        LIMIT $1
        "#
    );
    let rows: Vec<TagCountRow> = sqlx::query_as(&sql).bind(limit).fetch_all(db).await?;

    let tags: Vec<TagCount> = rows
        .into_iter()
        .map(|r| TagCount {
            tag: r.tag,
            count: r.count,
        })
        .collect();

    Ok(Json(TagsResponse { tags }))
}

#[derive(Debug, sqlx::FromRow)]
struct TagCountRow {
    tag: String,
    count: i64,
}

/// Fetch jobs with optional flavor/q filtering.
async fn list_jobs(
    db: &PgPool,
    flavor: &str,
    q: &Option<String>,
    limit: i64,
    offset: i64,
) -> Result<impl IntoResponse, ApiError> {
    let where_clause = flavor_where_clause(flavor);
    let q_filter = q
        .as_ref()
        .map(|s| format!("AND tag ILIKE '%{}'", s.replace('\'', "''")));

    let mut sql = format!(
        r#"
        SELECT {JOB_V2_COLUMNS}
        FROM jobs
        WHERE {where_clause}
        "#
    );
    if let Some(ref f) = q_filter {
        sql.push_str(f);
    }
    sql.push_str(&format!(
        " ORDER BY id DESC LIMIT {} OFFSET {}",
        limit, offset
    ));

    let rows: Vec<JobV2> = sqlx::query_as(&sql).fetch_all(db).await?;

    // Count total for pagination.
    let mut count_sql = format!("SELECT COUNT(*) FROM jobs WHERE {where_clause}");
    if let Some(ref f) = q_filter {
        count_sql.push_str(f);
    }
    let total: i64 = sqlx::query_scalar(&count_sql).fetch_one(db).await?;

    let jobs: Vec<Value> = rows.iter().map(|j| job_v2_to_json(j, false)).collect();

    Ok(Json(JobsResponse { jobs, total }))
}

/// Build the flavor WHERE clause for jobs queries.
fn flavor_where_clause(flavor: &str) -> String {
    match flavor {
        "failed" => "workflow_state = 'failed' OR failed_at IS NOT NULL".to_string(),
        "future" => {
            "workflow_state = 'queued' AND run_at > now() AND locked_at IS NULL".to_string()
        }
        "current" | _ => {
            "workflow_state NOT IN ('failed') AND (workflow_state != 'failed' OR failed_at IS NULL)"
                .to_string()
        }
    }
}

/// Convert a JobV2 to a JSON Value, optionally including handler/last_error.
fn job_v2_to_json(job: &JobV2, include_extras: bool) -> Value {
    let mut value = serde_json::to_value(job).unwrap_or_else(|_| json!({}));

    if let Some(obj) = value.as_object_mut() {
        if !include_extras {
            obj.remove("handler");
            obj.remove("last_error");
        }
        obj.remove("workflow_state");
    }

    value
}

/// `GET /api/v1/jobs/:id` — show one job by id (site admin only).
///
/// Canvas: `JobsController#show`.
/// Looks in `Delayed::Job` by default, or `Delayed::Job::Failed` (same table)
/// when `?flavor=failed`.
pub async fn show_job_v1(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Query(params): Query<ShowJobQuery>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&auth)?;

    let sql = if params.flavor.as_deref() == Some("failed") {
        format!(
            "SELECT {JOB_V2_COLUMNS} FROM jobs WHERE id = $1 AND (workflow_state = 'failed' OR failed_at IS NOT NULL)"
        )
    } else {
        format!(
            "SELECT {JOB_V2_COLUMNS} FROM jobs WHERE id = $1 AND (workflow_state != 'failed' AND failed_at IS NULL)"
        )
    };

    let row: Option<JobV2> = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?;

    match row {
        Some(job) => Ok(Json(job_v2_to_json(&job, true))),
        None => Err(ApiError::NotFound),
    }
}

#[derive(Debug, Deserialize)]
pub struct ShowJobQuery {
    pub flavor: Option<String>,
}

/// `POST /api/v1/jobs/batch_update` — bulk update/delete/unblock jobs.
///
/// Canvas: `JobsController#batch_update`.
pub async fn batch_update(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<BatchUpdateRequest>,
) -> Result<Json<BatchUpdateResponse>, ApiError> {
    require_admin(&auth)?;

    let action = payload.update_action.as_str();
    let job_ids = payload.job_ids;
    let flavor = payload.flavor.as_deref();
    let q = payload.q.as_deref();

    let count = match action {
        "force_delete" => {
            if let Some(ids) = job_ids {
                sqlx::query_scalar::<_, i64>("DELETE FROM jobs WHERE id = ANY($1)")
                    .bind(&ids)
                    .fetch_optional(&state.db)
                    .await?;
                ids.len() as i64
            } else {
                return Err(ApiError::BadRequest(
                    "job_ids required for force_delete".into(),
                ));
            }
        }
        "destroy" => {
            if let Some(ids) = job_ids {
                sqlx::query("UPDATE jobs SET workflow_state = 'deleted' WHERE id = ANY($1)")
                    .bind(&ids)
                    .execute(&state.db)
                    .await?;
                ids.len() as i64
            } else {
                return Err(ApiError::BadRequest("job_ids required for destroy".into()));
            }
        }
        "unblock" => {
            // Set next_in_strand = true for all jobs in the given strand.
            if let Some(strand_name) = q {
                let updated = sqlx::query(
                    "UPDATE jobs SET next_in_strand = TRUE WHERE strand = $1 AND workflow_state = 'queued'"
                )
                .bind(strand_name)
                .execute(&state.db)
                .await?;
                updated.rows_affected() as i64
            } else {
                return Err(ApiError::BadRequest(
                    "q (strand name) required for unblock".into(),
                ));
            }
        }
        "resize" => {
            // Resize sets max_concurrent on all jobs in a strand (passed via q).
            if let Some(strand_name) = q {
                let updated = sqlx::query(
                    "UPDATE jobs SET max_concurrent = COALESCE(max_concurrent, 1) WHERE strand = $1"
                )
                .bind(strand_name)
                .execute(&state.db)
                .await?;
                updated.rows_affected() as i64
            } else {
                return Err(ApiError::BadRequest(
                    "q (strand name) required for resize".into(),
                ));
            }
        }
        "strand" => {
            // Rename strand: update strand column for jobs matching q (old name).
            if let Some(strand_name) = q {
                let updated = sqlx::query("UPDATE jobs SET strand = $1 WHERE strand = $2")
                    .bind(&payload.q)
                    .bind(strand_name)
                    .execute(&state.db)
                    .await?;
                updated.rows_affected() as i64
            } else {
                return Err(ApiError::BadRequest(
                    "q (old strand name) required for strand action".into(),
                ));
            }
        }
        "update" => {
            // Generic: update priority/max_concurrent for matching job_ids.
            if let Some(ids) = job_ids {
                sqlx::query(
                    "UPDATE jobs SET priority = COALESCE(priority, 0), max_concurrent = COALESCE(max_concurrent, 1) WHERE id = ANY($1)"
                )
                .bind(&ids)
                .execute(&state.db)
                .await?;
                ids.len() as i64
            } else {
                return Err(ApiError::BadRequest("job_ids required for update".into()));
            }
        }
        _ => {
            return Err(ApiError::BadRequest(format!(
                "unknown update_action: {action}"
            )));
        }
    };

    Ok(Json(BatchUpdateResponse {
        status: "OK".to_string(),
        count,
    }))
}

/// Build the jobs V1 JSON-mode router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list_jobs_json))
        .route("/jobs/batch_update", post(batch_update))
        .route("/jobs/{id}", get(show_job_v1))
}
