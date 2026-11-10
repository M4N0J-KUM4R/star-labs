//! star-jobs — Canvas-compatible background job/progress records.
//!
//! Canvas parity (`app/controllers/jobs_controller.rb`, Jobs V1 API):
//! - `GET  /api/v1/jobs`             — list jobs (site admin only)
//! - `GET  /api/v1/jobs/{id}`        — show one job (site admin only)
//! - `POST /api/v1/jobs/{id}/cancel` — cancel a queued/running job
//!
//! Job JSON shape: `id, tag, workflow_state, progress, created_at,
//! updated_at, user_id, context_type, context_id`.
//!
//! No worker runs in this workspace: [`enqueue_job`] records the durable row
//! and an external worker (or a later phase) is expected to advance
//! `workflow_state` / `progress`.
//!
//! Auth: Canvas gates the Jobs API behind the site-admin `view_jobs` /
//! `manage_jobs` permissions; the workspace equivalent is the global `admin`
//! role, so non-admins get `403` and anonymous callers `401`.

use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// A background job row in Canvas Jobs API shape.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub tag: String,
    pub workflow_state: String,
    pub progress: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub user_id: Option<i64>,
    pub context_type: Option<String>,
    pub context_id: Option<i64>,
}

/// Query params for `GET /jobs`.
///
/// Canvas accepts `limit`/`offset` (`JobsController::LIMIT = 100`, offset 0)
/// and filters by workflow state; the same names are kept here.
#[derive(Debug, Deserialize)]
pub struct ListJobsQuery {
    pub workflow_state: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Canvas caps job listings at 100 rows per page.
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 100;

/// Terminal state written by `POST /jobs/{id}/cancel`.
const STATE_CANCELLED: &str = "cancelled";

/// Next state when cancelling a job currently in `current`.
///
/// Cancelling an already-cancelled job is idempotent (retries must not fail);
/// jobs that already reached another terminal state (`completed`, `failed`)
/// conflict instead of being rewritten.
fn cancelled_state(current: &str) -> Result<&'static str, ApiError> {
    match current {
        "queued" | "running" => Ok(STATE_CANCELLED),
        "cancelled" => Ok(STATE_CANCELLED),
        other => Err(ApiError::Conflict(format!(
            "job in state '{other}' cannot be cancelled"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas restricts the Jobs API to site admins (`view_jobs` / `manage_jobs`);
/// the workspace equivalent is the global `admin` role.
fn require_admin(auth: &AuthUser) -> Result<(), ApiError> {
    if is_admin(auth) {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the jobs router (mounted under `/api/v1` by star-http).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(list_jobs))
        .route("/jobs/{id}", get(show_job))
        .route("/jobs/{id}/cancel", post(cancel_job))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /jobs` — list jobs, newest first (site admin only).
async fn list_jobs(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ListJobsQuery>,
) -> Result<Json<Vec<Job>>, ApiError> {
    require_admin(&auth)?;

    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);

    let rows: Vec<Job> = sqlx::query_as(
        r#"
        SELECT id, tag, workflow_state, progress, created_at, updated_at,
               user_id, context_type, context_id
        FROM jobs
        WHERE ($1::text IS NULL OR workflow_state = $1)
        ORDER BY id DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(params.workflow_state.as_deref())
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// `GET /jobs/{id}` — show one job (site admin only).
async fn show_job(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Job>, ApiError> {
    require_admin(&auth)?;
    Ok(Json(fetch_job(&state.db, id).await?))
}

/// `POST /jobs/{id}/cancel` — move a queued/running job to `cancelled`.
async fn cancel_job(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Job>, ApiError> {
    require_admin(&auth)?;

    let existing = fetch_job(&state.db, id).await?;
    let next = cancelled_state(&existing.workflow_state)?;

    // Already cancelled: return the row unchanged so retries do not bump
    // updated_at.
    if next == existing.workflow_state.as_str() {
        return Ok(Json(existing));
    }

    let job: Job = sqlx::query_as(
        r#"
        UPDATE jobs
        SET workflow_state = $1, updated_at = now()
        WHERE id = $2
        RETURNING id, tag, workflow_state, progress, created_at, updated_at,
                  user_id, context_type, context_id
        "#,
    )
    .bind(next)
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(job))
}

// ---------------------------------------------------------------------------
// Enqueue helper (no HTTP route: internal callers only)
// ---------------------------------------------------------------------------

/// Enqueue an internal job row for a request.
///
/// No worker is started and nothing is scheduled: the returned row is the
/// durable record that a background task is pending. Callers that enqueue on
/// behalf of a user should pass that user's id and the Canvas context
/// (`context_type` + `context_id`) the job belongs to.
pub async fn enqueue_job(
    db: &PgPool,
    user_id: Option<i64>,
    tag: &str,
    context_type: Option<&str>,
    context_id: Option<i64>,
) -> Result<Job, ApiError> {
    if tag.trim().is_empty() {
        return Err(ApiError::Validation("tag cannot be empty".into()));
    }

    let job: Job = sqlx::query_as(
        r#"
        INSERT INTO jobs (tag, workflow_state, progress, user_id, context_type, context_id)
        VALUES ($1, 'queued', 0, $2, $3, $4)
        RETURNING id, tag, workflow_state, progress, created_at, updated_at,
                  user_id, context_type, context_id
        "#,
    )
    .bind(tag)
    .bind(user_id)
    .bind(context_type)
    .bind(context_id)
    .fetch_one(db)
    .await?;

    Ok(job)
}

/// Load one job by id, mapping a missing row to 404.
async fn fetch_job(db: &PgPool, id: i64) -> Result<Job, ApiError> {
    sqlx::query_as(
        r#"
        SELECT id, tag, workflow_state, progress, created_at, updated_at,
               user_id, context_type, context_id
        FROM jobs
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Unit tests (no database required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn test_cancelled_state_transitions() {
        // Queued and running jobs can be cancelled.
        assert_eq!(cancelled_state("queued").unwrap(), STATE_CANCELLED);
        assert_eq!(cancelled_state("running").unwrap(), STATE_CANCELLED);
        // Cancelling twice stays cancelled (idempotent retry).
        assert_eq!(cancelled_state("cancelled").unwrap(), STATE_CANCELLED);

        // Other terminal states conflict rather than being rewritten.
        for terminal in ["completed", "failed", "unknown"] {
            let err = cancelled_state(terminal).unwrap_err();
            assert_eq!(err.status(), StatusCode::CONFLICT);
        }
    }

    #[test]
    fn test_require_admin() {
        let admin = AuthUser {
            id: 1,
            role: "admin".to_string(),
        };
        let teacher = AuthUser {
            id: 2,
            role: "teacher".to_string(),
        };
        assert!(is_admin(&admin));
        assert!(require_admin(&admin).is_ok());
        assert_eq!(
            require_admin(&teacher).unwrap_err().status(),
            StatusCode::FORBIDDEN
        );
    }
}
