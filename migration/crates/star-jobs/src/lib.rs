//! star-jobs — Canvas-compatible background job/progress records.
//!
//! Canvas parity (`app/controllers/jobs_controller.rb`, Jobs V1 API):
//! - `GET  /api/v1/jobs`              — list jobs, JSON mode (`only`/`flavor`/`q`)
//! - `GET  /api/v1/jobs/{id}`         — show one job (`?flavor=failed`)
//! - `POST /api/v1/jobs/batch_update` — bulk update/delete/unblock jobs
//!
//! The V1 JSON-mode endpoints emit the Canvas `Delayed::Job` columns
//! (`id, tag, strand, singleton, shard_id, max_concurrent, priority,
//! attempts, max_attempts, locked_by, run_at, locked_at, handler`, with
//! `failed_at, original_job_id, requeued_job_id, last_error` on failed rows);
//! listings drop `handler`/`last_error` like Canvas' `as_json` except list.
//!
//! Canvas parity (`app/controllers/jobs_v2_controller.rb`, Jobs V2 API):
//! - `GET  /api/v1/jobs2/{id}`         — lookup by id or `original_job_id`
//! - `GET  /api/v1/jobs2/{bucket}`     — bucket listing (queued/running/future/failed)
//! - `GET  /api/v1/jobs2/{bucket}/by_{group}` — grouped counts (tag/strand/singleton)
//! - `GET  /api/v1/jobs2/{bucket}/by_{group}/search` — group text search
//! - `GET  /api/v1/jobs2/clusters`     — per-cluster job counts
//! - `POST /api/v1/jobs2/{id}/requeue` — requeue a failed job
//! - `PUT  /api/v1/jobs2/manage`       — update a strand's concurrency/priority
//! - `PUT  /api/v1/jobs2/unstuck`      — unstick a strand/singleton (202 async mode)
//! - `GET  /api/v1/jobs2/throttle/check` — throttle pre-check for a tag prefix
//!
//! Jobs V2 exposes the Delayed::Job columns added by migration 0057:
//! `id, tag, strand, singleton, shard_id, max_concurrent, priority, attempts,
//! max_attempts, locked_by, run_at, locked_at, handler` plus, for failed
//! jobs, `failed_at, original_job_id, requeued_job_id, last_error`.
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
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// New route modules (V1 JSON and V2 grouped/management).
pub mod routes_v1;
pub mod routes_v2_grouped;
pub mod routes_v2_manage;

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

/// Canvas caps job listings at 100 rows per page.
const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 100;

/// Failed state used by the Jobs V2 `failed` bucket.
const STATE_FAILED: &str = "failed";

// ---------------------------------------------------------------------------
// Jobs V2 domain types (Canvas JobsV2Controller)
// ---------------------------------------------------------------------------

/// A job row in Canvas Jobs V2 (Delayed::Job) JSON shape.
///
/// Fields Canvas only emits for failed jobs are skipped when `None` and
/// re-added as explicit `null`s for failed rows by [`job_json`].
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct JobV2 {
    pub id: i64,
    pub tag: String,
    pub strand: Option<String>,
    pub singleton: Option<String>,
    pub shard_id: Option<i64>,
    pub max_concurrent: i32,
    pub priority: i32,
    pub attempts: i32,
    pub max_attempts: i32,
    pub locked_by: Option<String>,
    pub run_at: DateTime<Utc>,
    pub locked_at: Option<DateTime<Utc>>,
    pub handler: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_job_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requeued_job_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Internal state used for bucket inference; not part of the V2 payload.
    #[serde(skip_serializing)]
    pub workflow_state: String,
}

/// Column list shared by every Jobs V2 read (order-independent for `FromRow`).
pub(crate) const JOB_V2_COLUMNS: &str =
    "id, tag, strand, singleton, shard_id, max_concurrent, priority, \
     attempts, max_attempts, locked_by, run_at, locked_at, handler, failed_at, \
     original_job_id, requeued_job_id, last_error, workflow_state";

/// Canvas Jobs V2 buckets (`JobsV2Controller::BUCKETS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bucket {
    Queued,
    Running,
    Future,
    Failed,
}

impl Bucket {
    /// Parse the Canvas `bucket` parameter; anything else is a 400.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "future" => Some(Self::Future),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Future => "future",
            Self::Failed => "failed",
        }
    }

    /// SQL predicate selecting this bucket from the jobs table.
    pub(crate) fn where_clause(self) -> &'static str {
        match self {
            // Canvas `queued_scope`: due jobs that no worker has locked.
            Self::Queued => {
                "workflow_state = 'queued' AND run_at <= now() \
                 AND locked_by IS NULL AND locked_at IS NULL"
            }
            // Canvas `Delayed::Job.running`.
            Self::Running => "workflow_state = 'running' OR locked_at IS NOT NULL",
            // Canvas `Delayed::Job.future`.
            Self::Future => "workflow_state = 'queued' AND run_at > now() AND locked_at IS NULL",
            // Canvas `Delayed::Job::Failed` (same table here).
            Self::Failed => "workflow_state = 'failed' OR failed_at IS NOT NULL",
        }
    }
}

/// Query params for `GET /jobs2/{bucket}` (Canvas list arguments).
#[derive(Debug, Deserialize)]
pub struct ListJobs2Query {
    pub tag: Option<String>,
    pub strand: Option<String>,
    pub singleton: Option<String>,
    pub shard_id: Option<i64>,
    pub order: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Query params for `GET /jobs2/clusters`.
///
/// Canvas sends `job_shards[]=1&job_shards[]=2`; the workspace extractor
/// takes the same ids comma-separated (`job_shards=1,2`).
#[derive(Debug, Deserialize)]
pub struct ClustersQuery {
    pub job_shards: Option<String>,
    pub per_page: Option<i64>,
    pub limit: Option<i64>,
}

/// A delayed-jobs cluster row (Canvas job shard).
#[derive(Debug, Clone, sqlx::FromRow)]
struct JobCluster {
    id: i64,
    database_server_id: String,
    domain: String,
    block_stranded: bool,
    jobs_held: bool,
}

/// Canvas `list_order_clause`: whitelisted ORDER BY per `order`/bucket.
fn list_order_clause(bucket: Option<Bucket>, order: Option<&str>) -> &'static str {
    match order {
        Some("tag") => "LOWER(tag) ASC",
        Some("strand_singleton") => "LOWER(strand) ASC, LOWER(singleton) ASC",
        Some("id") => "id DESC",
        _ => match bucket {
            Some(Bucket::Queued | Bucket::Future) => "run_at ASC",
            Some(Bucket::Running) => "locked_at ASC",
            Some(Bucket::Failed) => "failed_at DESC",
            None => "id DESC",
        },
    }
}

/// Canvas `infer_bucket` for lookups/requeues that have no bucket argument.
fn infer_bucket(job: &JobV2) -> &'static str {
    if is_failed(job) {
        Bucket::Failed.as_str()
    } else if job.locked_at.is_some() || job.workflow_state == "running" {
        Bucket::Running.as_str()
    } else if job.run_at <= Utc::now() {
        Bucket::Queued.as_str()
    } else {
        Bucket::Future.as_str()
    }
}

fn is_failed(job: &JobV2) -> bool {
    job.failed_at.is_some() || job.workflow_state == STATE_FAILED
}

/// Canvas `list_info_data`: seconds waited for queued/running buckets,
/// timestamps for future/failed buckets.
fn bucket_info(bucket: Bucket, job: &JobV2) -> Value {
    match bucket {
        Bucket::Queued => json!(seconds_since(job.run_at)),
        Bucket::Running => json!(job.locked_at.map(seconds_since).unwrap_or(0)),
        Bucket::Future => json!(timestamp(job.run_at)),
        Bucket::Failed => json!(timestamp(job.failed_at.unwrap_or(job.run_at))),
    }
}

fn seconds_since(at: DateTime<Utc>) -> i64 {
    (Utc::now() - at).num_seconds().max(0)
}

fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Canvas `job_json`: bucket-shaped listing or inferred bucket for lookups.
fn job_json(job: &JobV2, bucket: Option<Bucket>) -> Value {
    let mut value = serde_json::to_value(job).unwrap_or_else(|_| json!({}));

    if let Some(obj) = value.as_object_mut() {
        match bucket {
            Some(bucket) => {
                obj.insert("info".to_string(), bucket_info(bucket, job));
            }
            None => {
                obj.insert("bucket".to_string(), json!(infer_bucket(job)));
            }
        }

        // Canvas emits the failed-only columns (as nulls) on failed rows.
        if is_failed(job) {
            for key in [
                "failed_at",
                "original_job_id",
                "requeued_job_id",
                "last_error",
            ] {
                obj.entry(key.to_string()).or_insert(Value::Null);
            }
        }
    }

    value
}

/// Parse `job_shards=1,2` into ids (bad entries are a 400).
fn parse_id_list(raw: &str) -> Result<Vec<i64>, ApiError> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| {
            part.parse::<i64>()
                .map_err(|_| ApiError::BadRequest(format!("invalid job shard id: {part}")))
        })
        .collect()
}

/// Load one job for the V2 surface, mapping a missing row to 404.
async fn fetch_job_v2(db: &PgPool, id: i64) -> Result<JobV2, ApiError> {
    let sql = format!("SELECT {JOB_V2_COLUMNS} FROM jobs WHERE id = $1");
    sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas restricts the Jobs API to site admins (`view_jobs` / `manage_jobs`);
/// the workspace equivalent is the global `admin` role.
pub(crate) fn require_admin(auth: &AuthUser) -> Result<(), ApiError> {
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
///
/// The V1 JSON-mode routes (`/jobs`, `/jobs/{id}`, `/jobs/batch_update`)
/// live in [`routes_v1`]; the V2 grouped and management routes live in
/// [`routes_v2_grouped`] and [`routes_v2_manage`].
pub fn router() -> Router<AppState> {
    Router::new()
        // Canvas Jobs V1 (JSON mode).
        .merge(routes_v1::router())
        // Canvas Jobs V2 (`/api/v1/jobs2/...`).
        .route("/jobs2/clusters", get(list_job_clusters))
        .route("/jobs2/{id}", get(lookup_or_list_v2))
        .route("/jobs2/{id}/requeue", post(requeue_job_v2))
        .merge(routes_v2_grouped::router())
        .merge(routes_v2_manage::router())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Jobs V2 handlers (Canvas JobsV2Controller)
// ---------------------------------------------------------------------------

/// Shared listing core for `GET /jobs2?bucket=…` and `GET /jobs2/{bucket}`.
async fn list_jobs_v2_rows(
    db: &PgPool,
    bucket: Option<Bucket>,
    params: &ListJobs2Query,
) -> Result<Vec<Value>, ApiError> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);

    let mut sql = format!("SELECT {JOB_V2_COLUMNS} FROM jobs WHERE 1 = 1");
    if let Some(bucket) = bucket {
        sql.push_str(" AND (");
        sql.push_str(bucket.where_clause());
        sql.push(')');
    }
    sql.push_str(" AND ($1::text IS NULL OR tag = $1)");
    sql.push_str(" AND ($2::text IS NULL OR strand = $2)");
    sql.push_str(" AND ($3::text IS NULL OR singleton = $3)");
    sql.push_str(" AND ($4::bigint IS NULL OR shard_id = $4)");
    sql.push_str(&format!(
        " ORDER BY {} LIMIT $5 OFFSET $6",
        list_order_clause(bucket, params.order.as_deref())
    ));

    let rows: Vec<JobV2> = sqlx::query_as(&sql)
        .bind(params.tag.as_deref())
        .bind(params.strand.as_deref())
        .bind(params.singleton.as_deref())
        .bind(params.shard_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await?;

    Ok(rows.iter().map(|job| job_json(job, bucket)).collect())
}

/// `GET /jobs2/{id}` — Canvas lookup by job id or `original_job_id`, or a
/// bucket listing when the path segment names a bucket.
///
/// Canvas routes these as two constrained routes (`jobs2/:bucket` and
/// `jobs2/:id`); axum cannot express Rails' segment constraints, so one
/// handler dispatches. A segment that is neither a bucket name nor numeric
/// is a 404, matching Canvas' unmatched-route behavior. Canvas returns an
/// array for lookups (a failed row and the job it superseded can both
/// match); an unknown id yields an empty array rather than a 404.
async fn lookup_or_list_v2(
    State(state): State<AppState>,
    Path(id_or_bucket): Path<String>,
    auth: AuthUser,
    Query(params): Query<ListJobs2Query>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&auth)?;

    if let Some(bucket) = Bucket::parse(&id_or_bucket) {
        let rows = list_jobs_v2_rows(&state.db, Some(bucket), &params).await?;
        return Ok(Json(rows));
    }

    let id: i64 = id_or_bucket.parse().map_err(|_| ApiError::NotFound)?;

    let sql = format!(
        "SELECT {JOB_V2_COLUMNS} FROM jobs WHERE id = $1 OR original_job_id = $1 ORDER BY id"
    );
    let rows: Vec<JobV2> = sqlx::query_as(&sql).bind(id).fetch_all(&state.db).await?;

    Ok(Json(rows.iter().map(|job| job_json(job, None)).collect()))
}

/// `GET /jobs2/clusters` — Canvas per-cluster job dashboard.
///
/// Each cluster reports its identity, the shard ids it has blocked or held,
/// its domain, and the queued/running/future/blocked job counts.
async fn list_job_clusters(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ClustersQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_admin(&auth)?;

    let shard_ids = match params.job_shards.as_deref() {
        Some(raw) => Some(parse_id_list(raw)?),
        None => None,
    };
    let limit = params
        .per_page
        .or(params.limit)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);

    let clusters: Vec<JobCluster> = sqlx::query_as(
        r#"
        SELECT id, database_server_id, domain, block_stranded, jobs_held
        FROM job_clusters
        WHERE ($1::bigint[] IS NULL OR id = ANY($1))
        ORDER BY id
        LIMIT $2
        "#,
    )
    .bind(shard_ids)
    .bind(limit)
    .fetch_all(&state.db)
    .await?;

    let mut body = Vec::with_capacity(clusters.len());
    for cluster in clusters {
        let (queued, running, future, blocked): (i64, i64, i64, i64) = sqlx::query_as(
            r#"
            SELECT
                COUNT(*) FILTER (
                    WHERE workflow_state = 'queued' AND run_at <= now()
                      AND locked_by IS NULL AND locked_at IS NULL
                      AND next_in_strand
                ) AS queued,
                COUNT(*) FILTER (
                    WHERE workflow_state = 'running' OR locked_at IS NOT NULL
                ) AS running,
                COUNT(*) FILTER (
                    WHERE workflow_state = 'queued' AND run_at > now()
                      AND locked_at IS NULL
                ) AS future,
                COUNT(*) FILTER (
                    WHERE workflow_state = 'queued'
                      AND (next_in_strand = FALSE OR $2)
                ) AS blocked
            FROM jobs
            WHERE shard_id = $1
            "#,
        )
        .bind(cluster.id)
        .bind(cluster.block_stranded)
        .fetch_one(&state.db)
        .await?;

        let blocked_shard_ids = if cluster.block_stranded {
            vec![cluster.id]
        } else {
            Vec::new()
        };
        let held_shard_ids = if cluster.jobs_held {
            vec![cluster.id]
        } else {
            Vec::new()
        };

        body.push(json!({
            "id": cluster.id,
            "database_server_id": cluster.database_server_id,
            "block_stranded_shard_ids": blocked_shard_ids,
            "jobs_held_shard_ids": held_shard_ids,
            "domain": cluster.domain,
            "counts": {
                "queued": queued,
                "running": running,
                "future": future,
                "blocked": blocked,
            },
        }));
    }

    Ok(Json(body))
}

/// `POST /jobs2/{id}/requeue` — re-enqueue a failed job.
///
/// Canvas copies the failed job's payload into a fresh queued row and records
/// the relationship on the failed row via `requeued_job_id`.
async fn requeue_job_v2(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_admin(&auth)?;

    let failed = fetch_job_v2(&state.db, id).await?;
    if !is_failed(&failed) {
        // Canvas only requeues `Delayed::Job::Failed` rows.
        return Err(ApiError::NotFound);
    }

    let mut tx = state.db.begin().await?;

    let insert_sql = format!(
        r#"
        INSERT INTO jobs (
            tag, workflow_state, progress, user_id, context_type, context_id,
            strand, singleton, shard_id, max_concurrent, priority,
            attempts, max_attempts, handler, run_at, next_in_strand, queue
        )
        SELECT tag, 'queued', 0, user_id, context_type, context_id,
               strand, singleton, shard_id, max_concurrent, priority,
               0, max_attempts, handler, now(), TRUE, queue
        FROM jobs
        WHERE id = $1
        RETURNING {JOB_V2_COLUMNS}
        "#
    );
    let job: JobV2 = sqlx::query_as(&insert_sql)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;

    sqlx::query("UPDATE jobs SET requeued_job_id = $1, updated_at = now() WHERE id = $2")
        .bind(job.id)
        .bind(id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    Ok(Json(job_json(&job, None)))
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

// ---------------------------------------------------------------------------
// Unit tests (no database required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

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

    fn sample_job(workflow_state: &str, run_at: DateTime<Utc>) -> JobV2 {
        JobV2 {
            id: 7,
            tag: "Kernel.pp".to_string(),
            strand: None,
            singleton: None,
            shard_id: Some(3),
            max_concurrent: 1,
            priority: 0,
            attempts: 0,
            max_attempts: 1,
            locked_by: None,
            run_at,
            locked_at: None,
            handler: Some("--- !ruby/object:Proc {}".to_string()),
            failed_at: None,
            original_job_id: None,
            requeued_job_id: None,
            last_error: None,
            workflow_state: workflow_state.to_string(),
        }
    }

    #[test]
    fn test_bucket_parse_and_ordering() {
        for (raw, bucket) in [
            ("queued", Bucket::Queued),
            ("running", Bucket::Running),
            ("future", Bucket::Future),
            ("failed", Bucket::Failed),
        ] {
            assert_eq!(Bucket::parse(raw), Some(bucket));
            assert_eq!(bucket.as_str(), raw);
        }
        // Unknown / missing buckets are rejected (Canvas requires a bucket).
        assert_eq!(Bucket::parse("completed"), None);

        // Canvas orders by the bucket's info column unless told otherwise.
        assert_eq!(list_order_clause(Some(Bucket::Queued), None), "run_at ASC");
        assert_eq!(
            list_order_clause(Some(Bucket::Failed), None),
            "failed_at DESC"
        );
        assert_eq!(
            list_order_clause(Some(Bucket::Running), Some("tag")),
            "LOWER(tag) ASC"
        );
        assert_eq!(list_order_clause(None, Some("id")), "id DESC");
        assert_eq!(list_order_clause(None, Some("bogus")), "id DESC");
    }

    #[test]
    fn test_job_json_matches_canvas_shapes() {
        let now = Utc::now();

        // Lookup (no bucket argument): inferred bucket, no `info`.
        let queued = sample_job("queued", now);
        let value = job_json(&queued, None);
        assert_eq!(value["id"], 7);
        assert_eq!(value["tag"], "Kernel.pp");
        assert_eq!(value["bucket"], "queued");
        assert!(value.get("info").is_none());
        assert!(value.get("workflow_state").is_none());
        assert!(value.get("failed_at").is_none());

        // Future jobs infer `future`; failed jobs infer `failed` and expose
        // the failed-only columns (as nulls when unset).
        let future = sample_job("queued", now + chrono::Duration::hours(1));
        assert_eq!(job_json(&future, None)["bucket"], "future");
        let mut failed = sample_job("failed", now);
        failed.failed_at = Some(now);
        failed.original_job_id = Some(99);
        let value = job_json(&failed, None);
        assert_eq!(value["bucket"], "failed");
        assert_eq!(value["original_job_id"], 99);
        assert!(value.get("requeued_job_id").is_some());
        assert!(value["requeued_job_id"].is_null());

        // Bucket listings carry `info`: seconds for queued, timestamp for future.
        let value = job_json(&queued, Some(Bucket::Queued));
        assert!(value.get("bucket").is_none());
        let info = value["info"].as_i64().expect("queued info is seconds");
        assert!((0..5).contains(&info), "queued info: {info}");
        let value = job_json(&future, Some(Bucket::Future));
        assert!(
            value["info"].as_str().is_some_and(|s| s.contains('T')),
            "future info is an ISO timestamp: {}",
            value["info"]
        );
    }
}
