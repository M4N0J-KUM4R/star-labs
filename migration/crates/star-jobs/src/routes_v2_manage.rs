//! Routes V2 — strand management, unstuck, and throttling.
//!
//! Canvas: `app/controllers/jobs_v2_controller.rb`.
//! Routes:
//! - `PUT /api/v1/jobs2/manage`         → manage_strand
//! - `PUT /api/v1/jobs2/unstuck`         → unstuck_strand
//! - `GET /api/v1/jobs2/throttle/check`  → throttle_check
//! - `PUT /api/v1/jobs2/throttle`        → throttle
//! - `GET /api/v1/jobs2/stuck/strands`   → stuck_strands
//! - `GET /api/v1/jobs2/stuck/singletons` → stuck_singletons

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::require_admin;

/// Request body for `PUT /jobs2/manage`.
#[derive(Debug, Default, Deserialize)]
pub struct ManageRequest {
    pub strand: Option<String>,
    pub max_concurrent: Option<i32>,
    pub priority: Option<i32>,
}

impl ManageRequest {
    /// Rails `params`: the query string and the request body merge, the body
    /// wins.
    fn merge(self, other: Self) -> Self {
        Self {
            strand: self.strand.or(other.strand),
            max_concurrent: self.max_concurrent.or(other.max_concurrent),
            priority: self.priority.or(other.priority),
        }
    }
}

/// Request body for `PUT /jobs2/unstuck`.
#[derive(Debug, Default, Deserialize)]
pub struct UnstuckRequest {
    pub strand: Option<String>,
    pub singleton: Option<String>,
    pub job_shards: Option<Vec<i64>>,
}

impl UnstuckRequest {
    /// Rails `params`: the query string and the request body merge, the body
    /// wins.
    fn merge(self, other: Self) -> Self {
        Self {
            strand: self.strand.or(other.strand),
            singleton: self.singleton.or(other.singleton),
            job_shards: self.job_shards.or(other.job_shards),
        }
    }
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
/// Canvas: `JobsV2Controller#manage` — updates every job in the strand
/// (`max_concurrent` and `priority` are only written when present) and then
/// unblocks the strand up to the new parallelism
/// (`SwitchmanInstJobs::JobsMigrator.unblock_strand!`).
pub async fn manage_strand(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ManageRequest>,
    body: Option<Json<ManageRequest>>,
) -> Result<Json<ManageResponse>, ApiError> {
    require_admin(&auth)?;

    let payload = query.merge(body.map(|Json(body)| body).unwrap_or_default());

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
    let mut new_parallelism: Option<i64> = None;
    if let Some(max_concurrent) = payload.max_concurrent {
        sets.push(format!("max_concurrent = {}", max_concurrent.max(1)));
        new_parallelism = Some(i64::from(max_concurrent.max(1)));
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

    let updated = sqlx::query(&sql).bind(strand).execute(&state.db).await?;

    // Canvas unblocks the strand up to the new parallelism (only when
    // `max_concurrent` was among the update args).
    if let Some(parallelism) = new_parallelism {
        unblock_strand(&state.db, strand, parallelism).await?;
    }

    Ok(Json(ManageResponse {
        status: "OK".to_string(),
        count: updated.rows_affected() as i64,
    }))
}

/// `PUT /api/v1/jobs2/unstuck` — unstick a strand, singleton, or all shards.
///
/// Canvas: `JobsV2Controller#unstuck` →
/// `SwitchmanInstJobs::JobsMigrator.unblock_strand!` / `unblock_singleton!`.
/// Three modes:
/// 1. `strand` provided → unblock the strand up to its parallelism
///    (`MAX(max_concurrent)`); the number of jobs that became runnable is
///    the count. An unknown strand is a 404 (Canvas' `count.nil?`).
/// 2. `singleton` provided → unblock the singleton's oldest job unless one
///    is already runnable.
/// 3. Neither → create a progress record and return 202 (no background worker).
pub async fn unstuck_strand(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<UnstuckRequest>,
    body: Option<Json<UnstuckRequest>>,
) -> Result<Response, ApiError> {
    require_admin(&auth)?;

    let payload = query.merge(body.map(|Json(body)| body).unwrap_or_default());

    if let Some(ref strand) = payload.strand {
        // Mode 1: unstick by strand (Canvas `unblock_strand!`).
        // `MAX(max_concurrent)` over an empty strand is NULL — Canvas then
        // answers `ActiveRecord::RecordNotFound`.
        let (parallelism,): (Option<i32>,) =
            sqlx::query_as("SELECT MAX(max_concurrent) FROM jobs WHERE strand = $1")
                .bind(strand)
                .fetch_one(&state.db)
                .await?;
        let Some(parallelism) = parallelism else {
            return Err(ApiError::NotFound);
        };
        let count = unblock_strand(&state.db, strand, i64::from(parallelism)).await?;
        return Ok(Json(json!({ "status": "OK", "count": count })).into_response());
    }

    if let Some(ref singleton) = payload.singleton {
        // Mode 2: unstick by singleton (Canvas `unblock_singleton!`): nothing
        // to do when a job is already next in strand, otherwise the oldest
        // job of the singleton becomes runnable; an unknown singleton is a
        // 404 (Canvas' `count.nil?`).
        let (oldest_id, any_next): (Option<i64>, Option<bool>) = sqlx::query_as(
            "SELECT MIN(id), BOOL_OR(next_in_strand) FROM jobs \
             WHERE strand IS NULL AND singleton = $1",
        )
        .bind(singleton)
        .fetch_one(&state.db)
        .await?;
        let count = match (any_next, oldest_id) {
            (Some(true), _) => 0,
            (_, Some(id)) => sqlx::query("UPDATE jobs SET next_in_strand = TRUE WHERE id = $1")
                .bind(id)
                .execute(&state.db)
                .await?
                .rows_affected() as i64,
            (Some(false) | None, None) => return Err(ApiError::NotFound),
        };
        return Ok(Json(json!({ "status": "OK", "count": count })).into_response());
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

// ---------------------------------------------------------------------------
// Canvas `SwitchmanInstJobs::JobsMigrator`
// ---------------------------------------------------------------------------

/// Canvas `unblock_strand!(strand, new_parallelism:)`: set `next_in_strand`
/// on the oldest blocked, unlocked jobs of the strand until the parallelism
/// is reached; returns how many jobs became runnable.
async fn unblock_strand(db: &PgPool, strand: &str, parallelism: i64) -> Result<i64, ApiError> {
    let (running,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM jobs WHERE strand = $1 AND next_in_strand = TRUE")
            .bind(strand)
            .fetch_one(db)
            .await?;
    let needed = parallelism - running;
    if needed <= 0 {
        return Ok(0);
    }
    let updated = sqlx::query(
        r#"
        UPDATE jobs SET next_in_strand = TRUE
        WHERE id IN (
            SELECT id FROM jobs
            WHERE strand = $1
              AND next_in_strand = FALSE
              AND locked_by IS NULL
              AND singleton IS NULL
            ORDER BY id ASC
            LIMIT $2
        )
        "#,
    )
    .bind(strand)
    .bind(needed)
    .execute(db)
    .await?;
    Ok(updated.rows_affected() as i64)
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
        .route("/jobs2/throttle", put(throttle))
        .route("/jobs2/stuck/strands", get(stuck_strands))
        .route("/jobs2/stuck/singletons", get(stuck_singletons))
}

/// Request body for `PUT /jobs2/throttle`.
#[derive(Debug, Default, Deserialize)]
pub struct ThrottleRequest {
    pub term: Option<String>,
    pub max_concurrent: Option<i32>,
    pub shard_id: Option<i64>,
}

impl ThrottleRequest {
    /// Rails `params`: the query string and the request body merge, the body
    /// wins.
    fn merge(self, other: Self) -> Self {
        Self {
            term: self.term.or(other.term),
            max_concurrent: self.max_concurrent.or(other.max_concurrent),
            shard_id: self.shard_id.or(other.shard_id),
        }
    }
}

/// Query params for `GET /jobs2/stuck/{strands,singletons}`.
#[derive(Debug, Default, Deserialize)]
pub struct StuckQuery {
    /// Canvas `job_shard` argument (a job cluster id in this port).
    pub job_shard: Option<i64>,
    /// Canvas `Api.paginate` page size.
    pub per_page: Option<i64>,
    /// Canvas `Api.paginate` page number (1-based).
    pub page: Option<i64>,
    /// Workspace alias for `per_page`.
    pub limit: Option<i64>,
    /// Workspace alias for the computed offset.
    pub offset: Option<i64>,
}

/// One stuck strand or singleton.
#[derive(Debug, sqlx::FromRow)]
struct StuckRow {
    name: Option<String>,
    stuck_count: i64,
}

/// `PUT /api/v1/jobs2/throttle` — Canvas `JobsV2Controller#throttle`.
///
/// The matching jobs move to a fresh temporary strand, so they run one at a
/// time (Canvas `Delayed::Job.apply_temp_strand!`). Like Rails' `params`, the
/// arguments come from the query string or the request body.
pub async fn throttle(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ThrottleRequest>,
    body: Option<Json<ThrottleRequest>>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&auth)?;

    let params = query.merge(body.map(|Json(body)| body).unwrap_or_default());

    let term = params
        .term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing term".into()))?;

    // Canvas: `params[:max_concurrent]&.to_i || 1`.
    let max_concurrent = params.max_concurrent.unwrap_or(1).max(0);
    let new_strand = format!("tmp_strand_{}", random_alphanumeric(16));
    let pattern = wildcard_right(term);

    let mut tx = state.db.begin().await?;

    // Canvas `Delayed::Job.apply_temp_strand!`.
    let updated = sqlx::query(
        "UPDATE jobs \
         SET strand = $3, max_concurrent = $4, next_in_strand = FALSE \
         WHERE strand IS NULL AND singleton IS NULL AND locked_by IS NULL \
           AND tag LIKE $1 ESCAPE '\\' \
           AND ($2::bigint IS NULL OR shard_id = $2)",
    )
    .bind(&pattern)
    .bind(params.shard_id)
    .bind(&new_strand)
    .bind(max_concurrent)
    .execute(&mut *tx)
    .await?;

    let job_count = updated.rows_affected() as i64;

    // Canvas `apply_temp_strand!`: the `max_concurrent` oldest jobs of the
    // new strand may run.
    sqlx::query(
        "UPDATE jobs SET next_in_strand = TRUE \
         WHERE id IN (SELECT id FROM jobs WHERE strand = $1 ORDER BY id ASC LIMIT $2)",
    )
    .bind(&new_strand)
    .bind(i64::from(max_concurrent))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(json!({
        "job_count": job_count,
        "new_strand": new_strand,
    })))
}

/// `GET /api/v1/jobs2/stuck/strands` — Canvas `JobsV2Controller#stuck_strands`.
pub async fn stuck_strands(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<StuckQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&auth)?;
    stuck(&state, &params, true).await
}

/// `GET /api/v1/jobs2/stuck/singletons` — Canvas
/// `JobsV2Controller#stuck_singletons`.
pub async fn stuck_singletons(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<StuckQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&auth)?;
    stuck(&state, &params, false).await
}

/// Canvas `stuck_strands` / `stuck_singletons` — the strands and the
/// singletons that hold a job that is not ready to run.
async fn stuck(
    state: &AppState,
    params: &StuckQuery,
    strands: bool,
) -> Result<Json<Vec<Value>>, ApiError> {
    let (per_page, offset) = page_bounds(params.per_page, params.page, params.limit, params.offset);

    // Canvas activates the requested job shard; the port stores the shard id
    // on the job row and knows the clusters through `job_clusters`.
    let shard_id = match params.job_shard {
        Some(shard_id) => {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM job_clusters WHERE id = $1)")
                    .bind(shard_id)
                    .fetch_one(&state.db)
                    .await?;
            if !exists {
                // Canvas raises `ActiveRecord::RecordNotFound` for an unknown
                // shard.
                return Err(ApiError::NotFound);
            }
            Some(shard_id)
        }
        None => None,
    };

    let sql = if strands {
        "SELECT strand AS name, COUNT(*) AS stuck_count \
         FROM jobs \
         WHERE strand IS NOT NULL AND ($1::bigint IS NULL OR shard_id = $1) \
         GROUP BY strand \
         HAVING NOT BOOL_OR(next_in_strand) \
         ORDER BY strand \
         LIMIT $2 OFFSET $3"
    } else {
        "SELECT singleton AS name, COUNT(*) AS stuck_count \
         FROM jobs \
         WHERE strand IS NULL AND singleton IS NOT NULL \
           AND ($1::bigint IS NULL OR shard_id = $1) \
         GROUP BY singleton \
         HAVING NOT BOOL_OR(next_in_strand) \
         ORDER BY singleton \
         LIMIT $2 OFFSET $3"
    };

    let rows: Vec<StuckRow> = sqlx::query_as(sql)
        .bind(shard_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    let body: Vec<Value> = rows
        .into_iter()
        .map(|row| json!({ "name": row.name, "count": row.stuck_count }))
        .collect();

    Ok(Json(body))
}

/// Canvas `Api.paginate`: `per_page` (default 10, max 100) and `page`.
fn page_bounds(
    per_page: Option<i64>,
    page: Option<i64>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> (i64, i64) {
    let per_page = per_page
        .or(limit)
        .unwrap_or(crate::DEFAULT_LIMIT)
        .clamp(1, crate::MAX_LIMIT);
    let page = page.unwrap_or(1).max(1);
    let offset = offset
        .unwrap_or_else(|| (page - 1).saturating_mul(per_page))
        .max(0);
    (per_page, offset)
}

/// Canvas `ActiveRecord::Base.wildcard_pattern(value, type: :right)`:
/// a case-sensitive prefix pattern.
fn wildcard_right(value: &str) -> String {
    format!("{}%", escape_like(value))
}

/// Escape the LIKE wildcards (`%`, `_`) and the escape character itself.
fn escape_like(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '%' => out.push_str("\\%"),
            '_' => out.push_str("\\_"),
            other => out.push(other),
        }
    }
    out
}

/// Random suffix for the temporary strand Canvas creates when throttling.
fn random_alphanumeric(length: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

    let mut state = random_seed();
    let mut out = String::with_capacity(length);
    for _ in 0..length {
        state = xorshift64(state);
        let index = (state % ALPHABET.len() as u64) as usize;
        out.push(ALPHABET[index] as char);
    }
    out
}

/// Seed for the strand suffix generator.
fn random_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15);
    let seed = nanos ^ (u64::from(std::process::id()) << 32) ^ 0x9e37_79b9_7f4a_7c15;
    if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    }
}

/// The xorshift64 generator the suffix uses.
fn xorshift64(mut state: u64) -> u64 {
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}
