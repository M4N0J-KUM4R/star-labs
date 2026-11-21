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

    let updated = sqlx::query(&sql).bind(strand).execute(&state.db).await?;

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
/// time.
pub async fn throttle(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ThrottleRequest>,
) -> Result<Json<Value>, ApiError> {
    require_admin(&auth)?;

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

    sqlx::query("UPDATE jobs SET next_in_strand = TRUE WHERE strand = $1 ORDER BY id LIMIT $2")
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
