//! Canvas Jobs V2 — grouped listings, search, strand management, throttling
//! and stuck-strand reporting.
//!
//! Parity target: `app/controllers/jobs_v2_controller.rb` (Canvas), the
//! `switchman-inst-jobs` gem (`SwitchmanInstJobs::JobsMigrator`) and the
//! `inst-jobs` delayed_job backend.
//!
//! Routes (mounted under `/api/v1` by star-http):
//! - `GET /jobs2/{bucket}/by_tag`       — grouped info, grouped by tag
//! - `GET /jobs2/{bucket}/by_strand`    — grouped info, grouped by strand
//! - `GET /jobs2/{bucket}/by_singleton` — grouped info, grouped by singleton
//! - `GET /jobs2/{bucket}/by_tag/search`       — search tags
//! - `GET /jobs2/{bucket}/by_strand/search`    — search strands
//! - `GET /jobs2/{bucket}/by_singleton/search` — search singletons
//! - `PUT /jobs2/manage`                — manage a strand
//! - `PUT /jobs2/unstuck`               — unstick strands / singletons
//! - `GET /jobs2/throttle/check`        — count the jobs a throttle would match
//! - `PUT /jobs2/throttle`              — throttle jobs by tag prefix
//! - `GET /jobs2/stuck/strands`         — blocked strands
//! - `GET /jobs2/stuck/singletons`      — blocked singletons
//!
//! Canvas spells the group inside the path segment (`by_:group`). axum's
//! router (matchit 0.8) only accepts whole-segment parameters, so the three
//! group values are registered as literal paths instead of one `{group}` path.
//!
//! Permission order follows Canvas: `view_jobs` for the read endpoints and
//! `manage_jobs` for `manage` / `unstuck` / `throttle`. This workspace models
//! both rights as the global `admin` role, so non-admins get `403` and
//! anonymous callers `401`.

use axum::{
    body::Bytes,
    extract::{FromRequest, Path, Query, Request, State},
    http::{header::CONTENT_TYPE, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Postgres, Transaction};

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Canvas `Api::PER_PAGE` — default page size for `Api.paginate`.
const DEFAULT_PER_PAGE: i64 = 10;
/// Canvas `Api::MAX_PER_PAGE` — largest page Canvas serves.
const MAX_PER_PAGE: i64 = 100;
/// Canvas `JobsV2Controller::SEARCH_LIMIT`.
const SEARCH_LIMIT: i64 = 100;
/// Canvas `Delayed::Backend::Base::ON_HOLD_BLOCKER` — the migrator's blocker
/// job marker (the port has no `source` column, see the module docs).
const ON_HOLD_BLOCKER: &str = "blocker job";
/// Upper bound for request bodies read by [`ActionParams`].
const MAX_BODY_BYTES: usize = 64 * 1024;
/// Canvas `JobsV2Controller#run_unstucker!` tag (progress row + queued job).
const UNSTUCKER_TAG: &str = "JobsV2Controller::run_unstucker!";

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Canvas `JobsV2Controller::BUCKETS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Queued,
    Running,
    Future,
    Failed,
}

impl Bucket {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "future" => Some(Self::Future),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// Canvas `jobs_scope`: the predicate selecting this bucket.
    fn where_clause(self) -> &'static str {
        match self {
            // Canvas `queued_scope`: due jobs that no worker holds. Jobs held
            // by "on hold" are still queued.
            Self::Queued => "run_at <= now() AND (locked_by IS NULL OR locked_by = 'on hold')",
            // Canvas `Delayed::Job.running`.
            Self::Running => "locked_at IS NOT NULL AND locked_by <> 'on hold'",
            // Canvas `Delayed::Job.future`.
            Self::Future => "run_at > now()",
            // Canvas `Delayed::Job::Failed` (same table in this port).
            Self::Failed => "workflow_state = 'failed' OR failed_at IS NOT NULL",
        }
    }

    /// Column the date range filters on (`failed_at` / `locked_at` / `run_at`).
    fn date_column(self) -> &'static str {
        match self {
            Self::Failed => "failed_at",
            Self::Running => "locked_at",
            Self::Queued | Self::Future => "run_at",
        }
    }

    /// Canvas `grouped_info_select`: the aggregate behind the `info` field.
    fn grouped_info_select(self) -> &'static str {
        match self {
            Self::Queued => "MIN(run_at) AS info_value",
            Self::Running => "MIN(locked_at) AS info_value",
            Self::Future => "MIN(run_at) AS info_value",
            Self::Failed => "MAX(failed_at) AS info_value",
        }
    }

    /// Canvas `grouped_order_clause` default: order by the bucket's info
    /// column (oldest first, newest failure first).
    fn default_order(self) -> &'static str {
        match self {
            Self::Queued | Self::Running | Self::Future => "info_value ASC",
            Self::Failed => "info_value DESC",
        }
    }
}

/// The column Canvas groups by (`@group`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Tag,
    Strand,
    Singleton,
}

impl Group {
    fn column(self) -> &'static str {
        match self {
            Self::Tag => "tag",
            Self::Strand => "strand",
            Self::Singleton => "singleton",
        }
    }
}

/// Query params for `GET /jobs2/{bucket}/by_{group}`.
#[derive(Debug, Default, Deserialize)]
pub struct GroupedQuery {
    /// Canvas `order` argument: `count`, `tag`, `strand`, `singleton`, `group`.
    pub order: Option<String>,
    /// Canvas `start_date` argument (ISO8601).
    pub start_date: Option<String>,
    /// Canvas `end_date` argument (ISO8601).
    pub end_date: Option<String>,
    /// Canvas `Api.paginate` page size.
    pub per_page: Option<i64>,
    /// Canvas `Api.paginate` page number (1-based).
    pub page: Option<i64>,
    /// Workspace alias for `per_page`.
    pub limit: Option<i64>,
    /// Workspace alias for the computed offset.
    pub offset: Option<i64>,
}

/// Query params for `GET /jobs2/{bucket}/by_{group}/search`.
#[derive(Debug, Default, Deserialize)]
pub struct SearchQuery {
    /// Canvas `term` argument (required).
    pub term: Option<String>,
    /// Canvas `start_date` argument (ISO8601).
    pub start_date: Option<String>,
    /// Canvas `end_date` argument (ISO8601).
    pub end_date: Option<String>,
}

/// Query params for `GET /jobs2/throttle/check`.
#[derive(Debug, Default, Deserialize)]
pub struct ThrottleCheckQuery {
    /// Canvas `term` argument (required).
    pub term: Option<String>,
    /// Canvas `shard_id` argument.
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

/// One grouped row (Canvas `grouped_info`).
#[derive(Debug, sqlx::FromRow)]
struct GroupedRow {
    group_count: i64,
    group_value: Option<String>,
    info_value: Option<DateTime<Utc>>,
}

/// One search hit (Canvas `search`).
#[derive(Debug, sqlx::FromRow)]
struct SearchRow {
    group_value: Option<String>,
    group_count: i64,
}

/// One blocked strand / singleton (Canvas `stuck_strands`).
#[derive(Debug, sqlx::FromRow)]
struct StuckRow {
    name: Option<String>,
    stuck_count: i64,
}

/// A progress row (Canvas `Api::V1::Progress#progress_json`).
#[derive(Debug, sqlx::FromRow)]
struct ProgressRow {
    id: i64,
    tag: String,
    workflow_state: String,
    completion: i32,
    context_type: Option<String>,
    context_id: Option<i64>,
    user_id: Option<i64>,
    message: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the Jobs V2 router (mounted under `/api/v1` by star-http).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs2/{bucket}/by_tag", get(grouped_info_by_tag))
        .route("/jobs2/{bucket}/by_strand", get(grouped_info_by_strand))
        .route(
            "/jobs2/{bucket}/by_singleton",
            get(grouped_info_by_singleton),
        )
        .route("/jobs2/{bucket}/by_tag/search", get(search_by_tag))
        .route("/jobs2/{bucket}/by_strand/search", get(search_by_strand))
        .route(
            "/jobs2/{bucket}/by_singleton/search",
            get(search_by_singleton),
        )
        .route("/jobs2/manage", put(manage))
        .route("/jobs2/unstuck", put(unstuck))
        .route("/jobs2/throttle/check", get(throttle_check))
        .route("/jobs2/throttle", put(throttle))
        .route("/jobs2/stuck/strands", get(stuck_strands))
        .route("/jobs2/stuck/singletons", get(stuck_singletons))
}

// ---------------------------------------------------------------------------
// Grouped info
// ---------------------------------------------------------------------------

/// `GET /jobs2/{bucket}/by_tag` — Canvas `grouped_info` grouped by tag.
async fn grouped_info_by_tag(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(bucket): Path<String>,
    Query(params): Query<GroupedQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    crate::require_admin(&auth)?;
    grouped_info(&state, &bucket, Group::Tag, &params).await
}

/// `GET /jobs2/{bucket}/by_strand` — Canvas `grouped_info` grouped by strand.
async fn grouped_info_by_strand(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(bucket): Path<String>,
    Query(params): Query<GroupedQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    crate::require_admin(&auth)?;
    grouped_info(&state, &bucket, Group::Strand, &params).await
}

/// `GET /jobs2/{bucket}/by_singleton` — Canvas `grouped_info` by singleton.
async fn grouped_info_by_singleton(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(bucket): Path<String>,
    Query(params): Query<GroupedQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    crate::require_admin(&auth)?;
    grouped_info(&state, &bucket, Group::Singleton, &params).await
}

/// Canvas `JobsV2Controller#grouped_info`.
async fn grouped_info(
    state: &AppState,
    bucket_raw: &str,
    group: Group,
    params: &GroupedQuery,
) -> Result<Json<Vec<Value>>, ApiError> {
    let bucket = parse_bucket(bucket_raw)?;
    let (per_page, offset) = page_bounds(params.per_page, params.page, params.limit, params.offset);
    let start_date = parse_iso8601(params.start_date.as_deref());
    let end_date = parse_iso8601(params.end_date.as_deref());

    let column = group.column();
    let sql = format!(
        "SELECT COUNT(*) AS group_count, {column} AS group_value, {info_select} AS info_value \
         FROM jobs \
         WHERE ({bucket_clause}) AND {column} IS NOT NULL \
           AND ($1::timestamptz IS NULL OR {date_column} >= $1) \
           AND ($2::timestamptz IS NULL OR {date_column} <= $2) \
         GROUP BY {column} \
         ORDER BY {order_clause} \
         LIMIT $3 OFFSET $4",
        column = column,
        info_select = bucket.grouped_info_select(),
        bucket_clause = bucket.where_clause(),
        date_column = bucket.date_column(),
        order_clause = grouped_order_clause(bucket, group, params.order.as_deref()),
    );

    let rows: Vec<GroupedRow> = sqlx::query_as(&sql)
        .bind(start_date)
        .bind(end_date)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    // Canvas `get_group_statuses`: only strand/singleton groups in the
    // queued and future buckets report whether they are orphaned.
    let statuses = if matches!(group, Group::Strand | Group::Singleton)
        && matches!(bucket, Bucket::Queued | Bucket::Future)
    {
        let values: Vec<String> = rows
            .iter()
            .filter_map(|row| row.group_value.clone())
            .collect();
        group_statuses(&state.db, group, &values).await?
    } else {
        Default::default()
    };

    let mut body = Vec::with_capacity(rows.len());
    for row in rows {
        let mut value = json!({
            "count": row.group_count,
            column: row.group_value.clone(),
            "info": grouped_info_data(bucket, row.info_value),
        });
        if let Some(name) = row.group_value.as_ref() {
            if let Some(orphaned) = statuses.get(name) {
                value["orphaned"] = json!(orphaned);
            }
        }
        body.push(value);
    }

    Ok(Json(body))
}

/// Canvas `grouped_order_clause`.
fn grouped_order_clause(bucket: Bucket, group: Group, order: Option<&str>) -> String {
    match order {
        // Canvas orders by `LOWER(<order>)`; the column has to be part of the
        // grouping, otherwise Canvas raises (Postgres rejects an ungrouped
        // column) — the port falls back to the bucket default instead.
        Some("tag") if group == Group::Tag => "LOWER(tag)".to_string(),
        Some("strand") if group == Group::Strand => "LOWER(strand)".to_string(),
        Some("singleton") if group == Group::Singleton => "LOWER(singleton)".to_string(),
        Some("count") => "group_count DESC".to_string(),
        Some("group") => format!("LOWER({})", group.column()),
        _ => bucket.default_order().to_string(),
    }
}

/// Canvas `grouped_info_data`: seconds for queued/running, timestamps for
/// future/failed.
fn grouped_info_data(bucket: Bucket, info: Option<DateTime<Utc>>) -> Value {
    match (bucket, info) {
        (Bucket::Queued | Bucket::Running, Some(at)) => json!(seconds_since(at)),
        (Bucket::Future | Bucket::Failed, Some(at)) => json!(timestamp(at)),
        _ => Value::Null,
    }
}

/// Canvas `get_group_statuses`: a group is orphaned when none of its jobs has
/// `next_in_strand` set.
async fn group_statuses(
    db: &PgPool,
    group: Group,
    values: &[String],
) -> Result<std::collections::HashMap<String, bool>, ApiError> {
    let mut statuses = std::collections::HashMap::new();
    if values.is_empty() {
        return Ok(statuses);
    }

    let sql = match group {
        Group::Strand => {
            "SELECT strand AS group_value, NOT BOOL_OR(next_in_strand) AS orphaned \
             FROM jobs WHERE strand = ANY($1) GROUP BY strand"
        }
        Group::Singleton => {
            "SELECT singleton AS group_value, NOT BOOL_OR(next_in_strand) AS orphaned \
             FROM jobs WHERE strand IS NULL AND singleton = ANY($1) GROUP BY singleton"
        }
        Group::Tag => return Ok(statuses),
    };

    let rows: Vec<(String, bool)> = sqlx::query_as(sql)
        .bind(values.to_vec())
        .fetch_all(db)
        .await?;

    for (name, orphaned) in rows {
        statuses.insert(name, orphaned);
    }

    Ok(statuses)
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

/// `GET /jobs2/{bucket}/by_tag/search` — Canvas `search` on the tag column.
async fn search_by_tag(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(bucket): Path<String>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    crate::require_admin(&auth)?;
    search(&state, &bucket, Group::Tag, &params).await
}

/// `GET /jobs2/{bucket}/by_strand/search` — Canvas `search` on strand.
async fn search_by_strand(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(bucket): Path<String>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    crate::require_admin(&auth)?;
    search(&state, &bucket, Group::Strand, &params).await
}

/// `GET /jobs2/{bucket}/by_singleton/search` — Canvas `search` on singleton.
async fn search_by_singleton(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(bucket): Path<String>,
    Query(params): Query<SearchQuery>,
) -> Result<Json<Value>, ApiError> {
    crate::require_admin(&auth)?;
    search(&state, &bucket, Group::Singleton, &params).await
}

/// Canvas `JobsV2Controller#search`: group → count for every group whose
/// value matches the search term.
async fn search(
    state: &AppState,
    bucket_raw: &str,
    group: Group,
    params: &SearchQuery,
) -> Result<Json<Value>, ApiError> {
    let bucket = parse_bucket(bucket_raw)?;
    let term = params
        .term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing term".into()))?;

    let start_date = parse_iso8601(params.start_date.as_deref());
    let end_date = parse_iso8601(params.end_date.as_deref());

    let column = group.column();
    let sql = format!(
        "SELECT {column} AS group_value, COUNT(*) AS group_count \
         FROM jobs \
         WHERE ({bucket_clause}) AND {column} ILIKE $1 ESCAPE '\\' \
           AND ($2::timestamptz IS NULL OR {date_column} >= $2) \
           AND ($3::timestamptz IS NULL OR {date_column} <= $3) \
         GROUP BY {column} \
         ORDER BY group_count DESC, {column} ASC \
         LIMIT {limit}",
        column = column,
        bucket_clause = bucket.where_clause(),
        date_column = bucket.date_column(),
        limit = SEARCH_LIMIT,
    );

    let rows: Vec<SearchRow> = sqlx::query_as(&sql)
        .bind(wildcard_full(term))
        .bind(start_date)
        .bind(end_date)
        .fetch_all(&state.db)
        .await?;

    let mut counts = serde_json::Map::new();
    for row in rows {
        if let Some(name) = row.group_value {
            counts.insert(name, json!(row.group_count));
        }
    }

    Ok(Json(Value::Object(counts)))
}

// ---------------------------------------------------------------------------
// Manage
// ---------------------------------------------------------------------------

/// `PUT /jobs2/manage` — Canvas `JobsV2Controller#manage`.
async fn manage(
    State(state): State<AppState>,
    auth: AuthUser,
    params: ActionParams,
) -> Result<Json<Value>, ApiError> {
    crate::require_admin(&auth)?;

    let strand = params
        .strand
        .as_deref()
        .map(str::trim)
        .filter(|strand| !strand.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing strand".into()))?;

    let max_concurrent = params
        .max_concurrent
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(ruby_to_i);
    let priority = params
        .priority
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(ruby_to_i);

    let mut tx = state.db.begin().await?;

    // Canvas `Delayed::Job.advisory_lock(strand)`.
    sqlx::query("SELECT pg_advisory_xact_lock($1::bigint)")
        .bind(advisory_lock_key("strand", strand))
        .execute(&mut *tx)
        .await?;

    // Canvas runs the update first and the unblock second; the unblock raises
    // `JobsBlockedError` for a blocked strand, which rolls the update back.
    if strand_blocked(&mut tx, strand).await? {
        let _ = tx.rollback().await;
        return Err(ApiError::Conflict(
            "strand is blocked by the jobs migrator".into(),
        ));
    }

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE strand = $1")
        .bind(strand)
        .fetch_one(&mut *tx)
        .await?;

    sqlx::query(
        "UPDATE jobs \
         SET max_concurrent = COALESCE($2::int, max_concurrent), \
             priority = COALESCE($3::int, priority) \
         WHERE strand = $1",
    )
    .bind(strand)
    .bind(max_concurrent)
    .bind(priority)
    .execute(&mut *tx)
    .await?;

    // Canvas calls `unblock_strand!` when `max_concurrent` was given.
    if let Some(max_concurrent) = max_concurrent {
        unblock_strand(&mut tx, strand, Some(max_concurrent)).await?;
    }

    tx.commit().await?;

    Ok(Json(json!({ "status": "OK", "count": count })))
}

// ---------------------------------------------------------------------------
// Unstuck
// ---------------------------------------------------------------------------

/// `PUT /jobs2/unstuck` — Canvas `JobsV2Controller#unstuck`.
async fn unstuck(
    State(state): State<AppState>,
    auth: AuthUser,
    params: ActionParams,
) -> Result<Response, ApiError> {
    crate::require_admin(&auth)?;

    let strand = params
        .strand
        .as_deref()
        .map(str::trim)
        .filter(|strand| !strand.is_empty());
    let singleton = params
        .singleton
        .as_deref()
        .map(str::trim)
        .filter(|singleton| !singleton.is_empty());

    if let Some(strand) = strand {
        let mut tx = state.db.begin().await?;

        if strand_blocked(&mut tx, strand).await? {
            let _ = tx.rollback().await;
            return Ok(Json(json!({ "status": "blocked" })).into_response());
        }

        // Canvas `acquire_advisory_lock(:strand, name)`.
        sqlx::query("SELECT pg_advisory_xact_lock($1::bigint)")
            .bind(advisory_lock_key("strand", strand))
            .execute(&mut *tx)
            .await?;

        let count = match unblock_strand(&mut tx, strand, None).await {
            Ok(count) => count,
            Err(err) => {
                let _ = tx.rollback().await;
                return Err(err);
            }
        };

        tx.commit().await?;

        return Ok(Json(json!({ "status": "OK", "count": count })).into_response());
    }

    if let Some(singleton) = singleton {
        let mut tx = state.db.begin().await?;

        if singleton_blocked(&mut tx, singleton).await? {
            let _ = tx.rollback().await;
            return Ok(Json(json!({ "status": "blocked" })).into_response());
        }

        // Canvas `acquire_advisory_lock(:singleton, name)`.
        sqlx::query("SELECT pg_advisory_xact_lock($1::bigint)")
            .bind(advisory_lock_key("singleton", singleton))
            .execute(&mut *tx)
            .await?;

        let count = match unblock_singleton(&mut tx, singleton).await {
            Ok(count) => count,
            Err(err) => {
                let _ = tx.rollback().await;
                return Err(err);
            }
        };

        tx.commit().await?;

        return Ok(Json(json!({ "status": "OK", "count": count })).into_response());
    }

    // No strand and no singleton: Canvas queues the unstucker job and answers
    // with the progress record.
    let progress = create_unstucker_progress(&state, &auth, params.job_shards.as_deref()).await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "status": "pending", "progress": progress })),
    )
        .into_response())
}

/// Port of `SwitchmanInstJobs::JobsMigrator.unblock_strand!`.
///
/// Returns the number of jobs that were unblocked; `404` when the strand has
/// no jobs at all (Canvas raises `ActiveRecord::RecordNotFound`).
async fn unblock_strand(
    tx: &mut Transaction<'_, Postgres>,
    strand: &str,
    new_parallelism: Option<i32>,
) -> Result<i64, ApiError> {
    let max_concurrent: Option<i32> =
        sqlx::query_scalar("SELECT MAX(max_concurrent) FROM jobs WHERE strand = $1")
            .bind(strand)
            .fetch_one(&mut **tx)
            .await?;

    let Some(new_parallelism) = new_parallelism.or(max_concurrent) else {
        // Canvas returns nil here and the controller turns it into a 404.
        return Err(ApiError::NotFound);
    };

    let running: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE strand = $1 AND next_in_strand = TRUE")
            .bind(strand)
            .fetch_one(&mut **tx)
            .await?;

    let needed = i64::from(new_parallelism) - running;
    if needed <= 0 {
        return Ok(0);
    }

    // Canvas orders by `strand_order_override, id`; the port has no
    // `strand_order_override` column and orders by id only.
    let updated = sqlx::query(
        "UPDATE jobs SET next_in_strand = TRUE \
         WHERE strand = $1 AND next_in_strand = FALSE \
           AND locked_by IS NULL AND singleton IS NULL \
         ORDER BY id LIMIT $2",
    )
    .bind(strand)
    .bind(needed)
    .execute(&mut **tx)
    .await?;

    Ok(updated.rows_affected() as i64)
}

/// Port of `SwitchmanInstJobs::JobsMigrator.unblock_singleton!`.
async fn unblock_singleton(
    tx: &mut Transaction<'_, Postgres>,
    singleton: &str,
) -> Result<i64, ApiError> {
    let row: (Option<i64>, Option<bool>) = sqlx::query_as(
        "SELECT MIN(id), BOOL_OR(next_in_strand) \
         FROM jobs WHERE strand IS NULL AND singleton = $1",
    )
    .bind(singleton)
    .fetch_one(&mut **tx)
    .await?;

    if row.1.unwrap_or(false) {
        // The singleton is not stuck.
        return Ok(0);
    }

    match row.0 {
        Some(id) => {
            sqlx::query("UPDATE jobs SET next_in_strand = TRUE WHERE id = $1")
                .bind(id)
                .execute(&mut **tx)
                .await?;
            Ok(1)
        }
        // Canvas returns nil and the controller turns it into a 404.
        None => Err(ApiError::NotFound),
    }
}

/// Canvas `JobsMigrator.blocked_by_migrator?` for a strand.
///
/// Canvas looks for a blocker job (`source = 'JobsMigrator::StrandBlocker'`)
/// or a shard that blocks stranded jobs. The port has no `source` column, so
/// the blocker job is recognised by its `blocker job` lock and the shard flags
/// live on `job_clusters`.
async fn strand_blocked(
    tx: &mut Transaction<'_, Postgres>,
    strand: &str,
) -> Result<bool, ApiError> {
    let blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
            SELECT 1 FROM jobs j \
            WHERE j.strand = $1 \
              AND (j.locked_by = $2 \
                   OR EXISTS (SELECT 1 FROM job_clusters c \
                              WHERE c.id = j.shard_id \
                                AND (c.block_stranded OR c.jobs_held))) \
        )",
    )
    .bind(strand)
    .bind(ON_HOLD_BLOCKER)
    .fetch_one(&mut **tx)
    .await?;

    Ok(blocked)
}

/// Canvas `JobsMigrator.blocked_by_migrator?` for a singleton.
async fn singleton_blocked(
    tx: &mut Transaction<'_, Postgres>,
    singleton: &str,
) -> Result<bool, ApiError> {
    let blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
            SELECT 1 FROM jobs j \
            WHERE j.strand IS NULL AND j.singleton = $1 \
              AND (j.locked_by = $2 \
                   OR EXISTS (SELECT 1 FROM job_clusters c \
                              WHERE c.id = j.shard_id \
                                AND (c.block_stranded OR c.jobs_held))) \
        )",
    )
    .bind(singleton)
    .bind(ON_HOLD_BLOCKER)
    .fetch_one(&mut **tx)
    .await?;

    Ok(blocked)
}

/// Canvas `Progress.create(context: @current_user, tag: ...)` plus the queued
/// unstucker job.
///
/// Canvas hands the requested job shard ids to the unstucker job. The port
/// has no payload column for that job, so the ids are recorded in the
/// progress message instead.
async fn create_unstucker_progress(
    state: &AppState,
    auth: &AuthUser,
    job_shards: Option<&str>,
) -> Result<Value, ApiError> {
    let message = job_shards
        .map(str::trim)
        .filter(|shards| !shards.is_empty())
        .map(|shards| format!("job_shards={shards}"));

    let progress: ProgressRow = sqlx::query_as(
        "INSERT INTO progress (tag, workflow_state, completion, context_type, context_id, user_id, message) \
         VALUES ($1, 'queued', 0, 'User', $2, $2, $3) \
         RETURNING id, tag, workflow_state, completion, context_type, context_id, user_id, \
                   message, created_at, updated_at",
    )
    .bind(UNSTUCKER_TAG)
    .bind(auth.id)
    .bind(message)
    .fetch_one(&state.db)
    .await?;

    // Canvas enqueues `JobsV2Controller#run_unstucker!`; the port records the
    // durable job row (no worker runs in this workspace).
    crate::enqueue_job(
        &state.db,
        Some(auth.id),
        UNSTUCKER_TAG,
        Some("User"),
        Some(auth.id),
    )
    .await?;

    Ok(json!({
        "id": progress.id,
        "context_id": progress.context_id,
        "context_type": progress.context_type,
        "user_id": progress.user_id,
        "tag": progress.tag,
        "completion": progress.completion,
        "workflow_state": progress.workflow_state,
        "created_at": timestamp(progress.created_at),
        "updated_at": timestamp(progress.updated_at),
        "message": progress.message,
        "url": format!("http://localhost:3000/api/v1/progress/{}", progress.id),
    }))
}

// ---------------------------------------------------------------------------
// Throttling
// ---------------------------------------------------------------------------

/// `GET /jobs2/throttle/check` — Canvas `JobsV2Controller#throttle_check`.
async fn throttle_check(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<ThrottleCheckQuery>,
) -> Result<Json<Value>, ApiError> {
    crate::require_admin(&auth)?;

    let term = params
        .term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing term".into()))?;

    let (matched_jobs, matched_tags): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*) AS matched_jobs, COUNT(DISTINCT tag) AS matched_tags \
         FROM jobs \
         WHERE strand IS NULL AND singleton IS NULL AND locked_by IS NULL \
           AND tag LIKE $1 ESCAPE '\\' \
           AND ($2::bigint IS NULL OR shard_id = $2)",
    )
    .bind(wildcard_right(term))
    .bind(params.shard_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({
        "matched_jobs": matched_jobs,
        "matched_tags": matched_tags,
    })))
}

/// `PUT /jobs2/throttle` — Canvas `JobsV2Controller#throttle`.
async fn throttle(
    State(state): State<AppState>,
    auth: AuthUser,
    params: ActionParams,
) -> Result<Json<Value>, ApiError> {
    crate::require_admin(&auth)?;

    let term = params
        .term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing term".into()))?;

    // Canvas: `params[:max_concurrent]&.to_i || 1`.
    let max_concurrent = params
        .max_concurrent
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(ruby_to_i)
        .unwrap_or(1);

    let shard_id = params
        .shard_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<i64>().ok());

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
    .bind(shard_id)
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

// ---------------------------------------------------------------------------
// Stuck strands / singletons
// ---------------------------------------------------------------------------

/// `GET /jobs2/stuck/strands` — Canvas `JobsV2Controller#stuck_strands`.
async fn stuck_strands(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<StuckQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    crate::require_admin(&auth)?;
    stuck(&state, &params, true).await
}

/// `GET /jobs2/stuck/singletons` — Canvas `JobsV2Controller#stuck_singletons`.
async fn stuck_singletons(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(params): Query<StuckQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    crate::require_admin(&auth)?;
    stuck(&state, &params, false).await
}

/// Canvas `stuck_strands` / `stuck_singletons`.
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

// ---------------------------------------------------------------------------
// Request parameters
// ---------------------------------------------------------------------------

/// Parameters for the Jobs V2 write endpoints.
///
/// Canvas reads them from the query string or from the request body, so the
/// extractor accepts both and merges them (the body wins).
#[derive(Debug, Clone, Default)]
pub struct ActionParams {
    pub strand: Option<String>,
    pub singleton: Option<String>,
    pub term: Option<String>,
    pub shard_id: Option<String>,
    pub max_concurrent: Option<String>,
    pub priority: Option<String>,
    pub job_shards: Option<String>,
}

impl ActionParams {
    /// Read a form-encoded / query-string payload.
    fn absorb_form(&mut self, raw: &str) {
        for (key, value) in parse_form_pairs(raw) {
            self.set(&key, &value);
        }
    }

    /// Read a JSON payload (numbers and booleans are accepted as strings).
    fn absorb_json(&mut self, body: &Bytes) -> Result<(), ApiError> {
        let value: Value = serde_json::from_slice(body)
            .map_err(|err| ApiError::BadRequest(format!("invalid JSON body: {err}")))?;

        let Some(object) = value.as_object() else {
            return Ok(());
        };

        for (key, value) in object {
            if let Some(text) = json_scalar_to_string(value) {
                self.set(key, &text);
            }
        }

        Ok(())
    }

    fn set(&mut self, key: &str, value: &str) {
        let value = value.to_string();
        match key {
            "strand" => self.strand = Some(value),
            "singleton" => self.singleton = Some(value),
            "term" => self.term = Some(value),
            "shard_id" => self.shard_id = Some(value),
            "max_concurrent" => self.max_concurrent = Some(value),
            "priority" => self.priority = Some(value),
            "job_shards" | "job_shards[]" => self.job_shards = Some(value),
            _ => {}
        }
    }
}

impl FromRequest<AppState> for ActionParams {
    type Rejection = ApiError;

    async fn from_request(req: Request, _state: &AppState) -> Result<Self, Self::Rejection> {
        let content_type = req
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let query = req.uri().query().map(str::to_owned);

        let body = axum::body::to_bytes(req.into_body(), MAX_BODY_BYTES)
            .await
            .map_err(|err| ApiError::BadRequest(format!("invalid request body: {err}")))?;

        let mut params = ActionParams::default();

        if let Some(query) = query {
            params.absorb_form(&query);
        }

        if !body.is_empty() {
            if content_type.contains("json") {
                params.absorb_json(&body)?;
            } else {
                params.absorb_form(&String::from_utf8_lossy(&body));
            }
        }

        Ok(params)
    }
}

/// Split a form-encoded / query-string payload into key/value pairs.
fn parse_form_pairs(raw: &str) -> Vec<(String, String)> {
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or_default();
            let value = parts.next().unwrap_or_default();
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

/// Percent-decode a form value (`+` means space).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        out.push((high * 16 + low) as u8);
                        index += 3;
                    }
                    _ => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// JSON scalars as strings (numbers and booleans included).
fn json_scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Canvas `require_bucket`: only the four known buckets are served.
fn parse_bucket(raw: &str) -> Result<Bucket, ApiError> {
    Bucket::parse(raw).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "invalid bucket '{raw}'; expected queued, running, future, or failed"
        ))
    })
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
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = page.unwrap_or(1).max(1);
    let offset = offset
        .unwrap_or_else(|| (page - 1).saturating_mul(per_page))
        .max(0);
    (per_page, offset)
}

/// Canvas `set_date_range`: only ISO8601 timestamps are applied.
fn parse_iso8601(raw: Option<&str>) -> Option<DateTime<Utc>> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// Canvas `ActiveRecord::Base.wildcard_pattern(value, type: :full)`:
/// a case-insensitive "contains" pattern.
fn wildcard_full(value: &str) -> String {
    format!("%{}%", escape_like(value))
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

/// Ruby `String#to_i`: the leading integer, `0` when there is none.
fn ruby_to_i(value: &str) -> i32 {
    let trimmed = value.trim();
    let mut digits = String::new();
    for (index, character) in trimmed.chars().enumerate() {
        if index == 0 && (character == '-' || character == '+') {
            digits.push(character);
            continue;
        }
        if character.is_ascii_digit() {
            digits.push(character);
        } else {
            break;
        }
    }
    digits.parse::<i32>().unwrap_or(0)
}

/// Advisory-lock key for a strand / singleton.
///
/// Canvas locks on `half_md5_as_bigint(name)`; the port hashes the name
/// itself because the Canvas helper function does not exist here.
fn advisory_lock_key(kind: &str, name: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in kind
        .bytes()
        .chain(std::iter::once(b':'))
        .chain(name.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash as i64
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

/// xorshift64 — small, dependency free pseudo random generator.
fn xorshift64(mut state: u64) -> u64 {
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    state
}

/// Seconds since a timestamp (Canvas `base_time - row.run_at`).
fn seconds_since(at: DateTime<Utc>) -> i64 {
    (Utc::now() - at).num_seconds().max(0)
}

/// RFC3339 timestamp with milliseconds (the house timestamp format).
fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}
