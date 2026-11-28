//! star-worker — the background job worker for star-lms.
//!
//! Canvas runs background work through `delayed_job`: rows in `delayed_jobs`
//! with a `handler`, a `run_at`, an `attempts` count and a `locked_at`. A
//! worker process claims the due rows, runs the handler and either destroys
//! the row (success) or reschedules it (failure).
//!
//! The Canvas facts this module reproduces:
//!
//! | Rule | Canvas source |
//! |---|---|
//! | the queue table and its columns | `canvas-src/db/migrate/20101216224513_create_delayed_jobs.rb:26-77` |
//! | claim order `(priority, run_at, id)` | same file, `:62-64` |
//! | batch size 20 | `canvas-src/config/initializers/delayed_job.rb:74` |
//! | max attempts 1 | `canvas-src/config/initializers/delayed_job.rb:77` |
//! | queue `canvas_queue` | `canvas-src/config/initializers/delayed_job.rb:80` |
//! | sleep delay 2 s | `canvas-src/config/initializers/delayed_job.rb:81` |
//! | retry backoff `now + attempts^4 + 5` seconds | `canvas-src/lib/turnitin/outcome_response_processor.rb:85` |
//!
//! The worker drains two queues. Canvas' `delayed_jobs` table follows Canvas'
//! semantics exactly — a row that ran successfully is destroyed, and a row
//! that exhausted its attempts keeps `failed_at` and `last_error` so it can
//! be inspected. The port's own `jobs` table (`0033_jobs.sql`, extended by
//! `0057_jobs_v2.sql`) is the port's projection of the same work: a row that
//! ran successfully is kept with `workflow_state = 'completed'`, the state
//! `0033_jobs.sql:28-31` allows and nothing else writes.

pub mod handlers;

use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use std::time::Duration as StdDuration;
use tokio::sync::watch;
use tracing::{error, info, warn};

use crate::handlers::{JobCtx, JobError};

/// Canvas' single queue (`config/initializers/delayed_job.rb:80`).
pub const CANVAS_QUEUE: &str = "canvas_queue";

/// Canvas' batch size (`config/initializers/delayed_job.rb:74`).
pub const CANVAS_FETCH_BATCH_SIZE: i64 = 20;

/// Canvas' sleep delay in seconds (`config/initializers/delayed_job.rb:81`).
pub const CANVAS_SLEEP_DELAY_SECS: u64 = 2;

/// Canvas' default max attempts (`config/initializers/delayed_job.rb:77`).
pub const CANVAS_MAX_ATTEMPTS: i32 = 1;

/// How long a claim stays valid before another worker may take the row.
///
/// Canvas has no lease — it relies on the `script/delayed_job` process
/// (`canvas-src/script/delayed_job`) staying alive. The port recovers the
/// rows of a worker that died, so a claim expires after this long.
pub const DEFAULT_LEASE_SECS: u64 = 300;

/// The worker's settings.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// The identity written to `locked_by`.
    pub worker_name: String,
    /// How long the worker sleeps when the queue is empty.
    pub poll_interval: StdDuration,
    /// How many rows one claim takes.
    pub batch_size: i64,
    /// How long a claim stays valid.
    pub lease: StdDuration,
    /// The queue the worker claims from.
    pub queue: String,
    /// The max attempts used when a row does not carry its own.
    pub max_attempts: i32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            worker_name: default_worker_name(),
            poll_interval: StdDuration::from_secs(CANVAS_SLEEP_DELAY_SECS),
            batch_size: CANVAS_FETCH_BATCH_SIZE,
            lease: StdDuration::from_secs(DEFAULT_LEASE_SECS),
            queue: CANVAS_QUEUE.to_string(),
            max_attempts: CANVAS_MAX_ATTEMPTS,
        }
    }
}

impl WorkerConfig {
    /// Read the worker's settings from the environment.
    ///
    /// Every setting has the Canvas default, so a worker started with no
    /// environment at all behaves like Canvas' `Delayed::Worker`.
    pub fn from_env() -> Result<Self, String> {
        let mut config = Self::default();

        if let Ok(name) = std::env::var("STAR_WORKER_NAME") {
            if !name.trim().is_empty() {
                config.worker_name = name;
            }
        }
        if let Ok(raw) = std::env::var("STAR_WORKER_POLL_INTERVAL_MS") {
            let millis: u64 = raw
                .trim()
                .parse()
                .map_err(|_| format!("STAR_WORKER_POLL_INTERVAL_MS is not a number: {raw}"))?;
            config.poll_interval = StdDuration::from_millis(millis);
        }
        if let Ok(raw) = std::env::var("STAR_WORKER_BATCH_SIZE") {
            let size: i64 = raw
                .trim()
                .parse()
                .map_err(|_| format!("STAR_WORKER_BATCH_SIZE is not a number: {raw}"))?;
            config.batch_size = size.clamp(1, 1000);
        }
        if let Ok(raw) = std::env::var("STAR_WORKER_LEASE_SECONDS") {
            let seconds: u64 = raw
                .trim()
                .parse()
                .map_err(|_| format!("STAR_WORKER_LEASE_SECONDS is not a number: {raw}"))?;
            config.lease = StdDuration::from_secs(seconds.max(1));
        }
        if let Ok(raw) = std::env::var("STAR_WORKER_QUEUE") {
            if !raw.trim().is_empty() {
                config.queue = raw;
            }
        }

        Ok(config)
    }
}

/// The identity a worker writes to `locked_by`.
///
/// Canvas uses the process name with a shard prefix
/// (`canvas-src/config/initializers/delayed_job.rb:83`).
fn default_worker_name() -> String {
    format!("star-worker-{}", std::process::id())
}

/// The queue a job row lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Queue {
    /// Canvas' `delayed_jobs` table.
    DelayedJobs,
    /// The port's own `jobs` table.
    Jobs,
}

impl Queue {
    /// The table the queue lives in.
    pub fn table(self) -> &'static str {
        match self {
            Self::DelayedJobs => "delayed_jobs",
            Self::Jobs => "jobs",
        }
    }

    /// The queues the worker drains, Canvas' first.
    pub fn all() -> [Queue; 2] {
        [Queue::DelayedJobs, Queue::Jobs]
    }
}

/// A claimed job row.
#[derive(Debug, Clone)]
pub struct ClaimedJob {
    /// The queue the row came from.
    pub queue: Queue,
    /// The row id.
    pub id: i64,
    /// The human-readable job name.
    pub tag: Option<String>,
    /// The handler that does the work.
    pub handler: Option<String>,
    /// How many times the row ran already.
    pub attempts: i32,
    /// The row's own max attempts, when it has one.
    pub max_attempts: Option<i32>,
    /// The queue priority.
    pub priority: i32,
    /// The user the job was enqueued for.
    pub user_id: Option<i64>,
    /// The Canvas context type.
    pub context_type: Option<String>,
    /// The Canvas context id.
    pub context_id: Option<i64>,
    /// When the row was due.
    pub run_at: DateTime<Utc>,
}

/// Canvas' retry backoff: the next run is `attempts^4 + 5` seconds away.
///
/// The formula is Canvas' delayed-job convention; it appears verbatim in
/// `canvas-src/lib/turnitin/outcome_response_processor.rb:85` and
/// `canvas-src/lib/basic_lti/basic_outcomes.rb:269`. Attempts 1, 2 and 3
/// give +6 s, +21 s and +86 s.
pub const fn backoff_seconds(attempts: i32) -> i64 {
    let a = attempts as i64;
    a * a * a * a + 5
}

/// What happened to a job row.
#[derive(Debug)]
pub enum Outcome {
    /// The work ran; the row is done.
    Success,
    /// The work failed but may succeed later.
    Retryable(String),
    /// The work can never succeed as requested.
    Permanent(String),
}

/// Resolve the tag of a job row.
///
/// The `handler` column names the work and the `tag` column repeats it as
/// `Class#method`; the port's enqueue paths write the tag only, so the tag is
/// the fallback.
pub fn resolve_tag(job: &ClaimedJob) -> Option<String> {
    job.handler
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            job.tag
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .map(|value| value.to_string())
}

/// Split a handler string into its tag and its payload.
///
/// The payload is the JSON object that follows the tag, e.g.
/// `AccountReport#delete_account_report_rows {"id": 12}`. A handler that
/// holds Canvas' YAML of a Ruby object (`--- !ruby/object:Proc {}`) is never
/// executable in Rust and fails permanently.
pub fn parse_handler(raw: &str) -> Result<(String, Value), String> {
    let trimmed = raw.trim();

    if trimmed.starts_with("--- !ruby") {
        return Err(
            "not executable: the handler holds a Canvas YAML payload of a Ruby object".to_string(),
        );
    }

    let (tag, rest) = match trimmed.split_once(' ') {
        Some((tag, rest)) => (tag.trim(), rest.trim()),
        None => (trimmed, ""),
    };

    if tag.is_empty() {
        return Err("the handler names no job".to_string());
    }

    if rest.is_empty() {
        return Ok((tag.to_string(), Value::Null));
    }

    let payload: Value = serde_json::from_str(rest)
        .map_err(|error| format!("the handler payload is not valid JSON: {error}"))?;

    Ok((tag.to_string(), payload))
}

/// Claim the due rows of one queue.
///
/// The claim is Canvas' shape: due (`run_at <= now()`), unlocked or holding a
/// stale lock, next in its strand, ordered by `(priority, run_at, id)` and
/// limited to the batch size. `FOR UPDATE SKIP LOCKED` is what keeps two
/// workers off the same row.
pub async fn claim(
    db: &PgPool,
    queue: Queue,
    config: &WorkerConfig,
) -> Result<Vec<ClaimedJob>, sqlx::Error> {
    match queue {
        Queue::DelayedJobs => claim_delayed_jobs(db, config).await,
        Queue::Jobs => claim_jobs(db, config).await,
    }
}

/// Claim from Canvas' `delayed_jobs` table.
///
/// The table stores naive timestamps (Canvas' `t.timestamp`,
/// `20101216224513_create_delayed_jobs.rb:39`), so every comparison uses
/// `now()::timestamp` — the same frame the port writes those columns in.
async fn claim_delayed_jobs(
    db: &PgPool,
    config: &WorkerConfig,
) -> Result<Vec<ClaimedJob>, sqlx::Error> {
    let sql = r#"
        UPDATE delayed_jobs AS d
           SET locked_at = now()::timestamp,
               locked_by = $1,
               workflow_state = 'running',
               updated_at = now()::timestamp
         WHERE d.id IN (
               SELECT id
                 FROM delayed_jobs
                WHERE queue = $2
                  AND run_at <= now()::timestamp
                  AND next_in_strand
                  AND failed_at IS NULL
                  AND (locked_at IS NULL
                       OR locked_at < now()::timestamp - make_interval(secs => $3))
                ORDER BY priority ASC, run_at ASC, id ASC
                LIMIT $4
                FOR UPDATE SKIP LOCKED
         )
        RETURNING d.id, d.tag, d.handler, d.attempts, d.max_attempts, d.priority,
                  d.user_id, d.context_type, d.context_id, d.run_at
    "#;

    let rows: Vec<DelayedJobRow> = sqlx::query_as(sql)
        .bind(&config.worker_name)
        .bind(&config.queue)
        .bind(config.lease.as_secs_f64())
        .bind(config.batch_size)
        .fetch_all(db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| ClaimedJob {
            queue: Queue::DelayedJobs,
            id: row.id,
            tag: row.tag,
            handler: row.handler,
            attempts: row.attempts,
            max_attempts: row.max_attempts,
            priority: row.priority,
            user_id: row.user_id,
            context_type: row.context_type,
            context_id: row.context_id,
            run_at: DateTime::from_naive_utc_and_offset(row.run_at, Utc),
        })
        .collect())
}

/// Claim from the port's `jobs` table.
async fn claim_jobs(db: &PgPool, config: &WorkerConfig) -> Result<Vec<ClaimedJob>, sqlx::Error> {
    let sql = r#"
        UPDATE jobs AS j
           SET locked_at = now(),
               locked_by = $1,
               workflow_state = 'running',
               updated_at = now()
         WHERE j.id IN (
               SELECT id
                 FROM jobs
                WHERE workflow_state = 'queued'
                  AND run_at <= now()
                  AND next_in_strand
                  AND failed_at IS NULL
                  AND (locked_at IS NULL
                       OR locked_at < now() - make_interval(secs => $2))
                ORDER BY priority ASC, run_at ASC, id ASC
                LIMIT $3
                FOR UPDATE SKIP LOCKED
         )
        RETURNING j.id, j.tag, j.handler, j.attempts, j.max_attempts, j.priority,
                  j.user_id, j.context_type, j.context_id, j.run_at
    "#;

    let rows: Vec<PortJobRow> = sqlx::query_as(sql)
        .bind(&config.worker_name)
        .bind(config.lease.as_secs_f64())
        .bind(config.batch_size)
        .fetch_all(db)
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| ClaimedJob {
            queue: Queue::Jobs,
            id: row.id,
            tag: row.tag,
            handler: row.handler,
            attempts: row.attempts,
            max_attempts: row.max_attempts,
            priority: row.priority,
            user_id: row.user_id,
            context_type: row.context_type,
            context_id: row.context_id,
            run_at: row.run_at,
        })
        .collect())
}

/// A row of Canvas' `delayed_jobs` table.
#[derive(Debug, sqlx::FromRow)]
struct DelayedJobRow {
    id: i64,
    tag: Option<String>,
    handler: Option<String>,
    attempts: i32,
    max_attempts: Option<i32>,
    priority: i32,
    user_id: Option<i64>,
    context_type: Option<String>,
    context_id: Option<i64>,
    run_at: NaiveDateTime,
}

/// A row of the port's `jobs` table.
#[derive(Debug, sqlx::FromRow)]
struct PortJobRow {
    id: i64,
    tag: Option<String>,
    handler: Option<String>,
    attempts: i32,
    max_attempts: Option<i32>,
    priority: i32,
    user_id: Option<i64>,
    context_type: Option<String>,
    context_id: Option<i64>,
    run_at: DateTime<Utc>,
}

/// Run the handler a job row names.
pub async fn run_job(db: &PgPool, job: &ClaimedJob) -> Outcome {
    let raw = match resolve_tag(job) {
        Some(raw) => raw,
        None => {
            return Outcome::Permanent(
                "not implemented: the job row names no handler and no tag".to_string(),
            )
        }
    };

    let (tag, payload) = match parse_handler(&raw) {
        Ok(parsed) => parsed,
        Err(message) => return Outcome::Permanent(message),
    };

    let ctx = JobCtx {
        db,
        job_id: job.id,
        queue: job.queue.table(),
        tag: tag.clone(),
        user_id: job.user_id,
        context_type: job.context_type.clone(),
        context_id: job.context_id,
        payload,
    };

    match handlers::dispatch(&ctx).await {
        Ok(()) => Outcome::Success,
        Err(JobError::Permanent(message)) => Outcome::Permanent(message),
        Err(JobError::Retryable(message)) => Outcome::Retryable(message),
    }
}

/// Write the result of a run back to the job row.
///
/// Success destroys the row in Canvas' queue (Canvas' `Delayed::Job` is
/// destroyed when the work ran) and completes the port's projection row. A
/// failure increments `attempts`, records `last_error`, clears the lock and
/// moves `run_at` forward with Canvas' backoff; once the row used up its
/// attempts it gets `failed_at` instead.
pub async fn finish(
    db: &PgPool,
    job: &ClaimedJob,
    outcome: Outcome,
    config: &WorkerConfig,
) -> Result<(), sqlx::Error> {
    match outcome {
        Outcome::Success => finish_success(db, job).await,
        Outcome::Permanent(message) => finish_failure(db, job, &message, config, false).await,
        Outcome::Retryable(message) => finish_failure(db, job, &message, config, true).await,
    }
}

/// The row ran; it is done.
async fn finish_success(db: &PgPool, job: &ClaimedJob) -> Result<(), sqlx::Error> {
    match job.queue {
        // Canvas destroys the row when the work ran: the queue row is the
        // pending-work record, not a history. The `Delayed::Job` model
        // itself is the `switchman-inst-jobs` gem's (not vendored in
        // `canvas-src/`), but the table is Canvas'
        // (`db/migrate/20101216224513_create_delayed_jobs.rb:26`).
        Queue::DelayedJobs => {
            sqlx::query("DELETE FROM delayed_jobs WHERE id = $1")
                .bind(job.id)
                .execute(db)
                .await?;
        }
        // The port keeps the record with the state `0033_jobs.sql:28-31`
        // allows and nothing else writes.
        Queue::Jobs => {
            sqlx::query(
                "UPDATE jobs SET workflow_state = 'completed', progress = 100, \
                 locked_by = NULL, locked_at = NULL, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(job.id)
            .execute(db)
            .await?;
        }
    }

    Ok(())
}

/// The row failed.
///
/// `retry` says whether the work may succeed later. A failure that may
/// succeed is rescheduled with Canvas' backoff; one that may not — and one
/// that used up the row's attempts — is given `failed_at` so the Jobs API
/// reports it as failed.
async fn finish_failure(
    db: &PgPool,
    job: &ClaimedJob,
    message: &str,
    config: &WorkerConfig,
    retry: bool,
) -> Result<(), sqlx::Error> {
    let attempts = job.attempts + 1;
    let max_attempts = job.max_attempts.unwrap_or(config.max_attempts).max(1);
    let give_up = !retry || attempts >= max_attempts;

    match job.queue {
        Queue::DelayedJobs => {
            if give_up {
                sqlx::query(
                    "UPDATE delayed_jobs \
                     SET attempts = $2, last_error = $3, locked_by = NULL, locked_at = NULL, \
                         failed_at = now()::timestamp, workflow_state = 'failed', \
                         updated_at = now()::timestamp \
                     WHERE id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(message)
                .execute(db)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE delayed_jobs \
                     SET attempts = $2, last_error = $3, locked_by = NULL, locked_at = NULL, \
                         run_at = now()::timestamp + make_interval(secs => $4), \
                         workflow_state = 'queued', updated_at = now()::timestamp \
                     WHERE id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(message)
                .bind(backoff_seconds(attempts) as f64)
                .execute(db)
                .await?;
            }
        }
        Queue::Jobs => {
            if give_up {
                sqlx::query(
                    "UPDATE jobs \
                     SET attempts = $2, last_error = $3, locked_by = NULL, locked_at = NULL, \
                         failed_at = now(), workflow_state = 'failed', updated_at = now() \
                     WHERE id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(message)
                .execute(db)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE jobs \
                     SET attempts = $2, last_error = $3, locked_by = NULL, locked_at = NULL, \
                         run_at = now() + make_interval(secs => $4), \
                         workflow_state = 'queued', updated_at = now() \
                     WHERE id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(message)
                .bind(backoff_seconds(attempts) as f64)
                .execute(db)
                .await?;
            }
        }
    }

    Ok(())
}

/// The worker's main loop.
///
/// Each iteration claims the due rows of both queues and runs them. When the
/// queues are empty the worker sleeps for `poll_interval`. The loop stops
/// claiming as soon as the shutdown channel is signalled; the job that is
/// running when the signal arrives is finished first, so its row is never
/// left locked.
pub async fn run_loop(
    db: PgPool,
    config: WorkerConfig,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), sqlx::Error> {
    info!(
        worker = %config.worker_name,
        queue = %config.queue,
        batch_size = config.batch_size,
        poll_interval_ms = config.poll_interval.as_millis() as u64,
        "worker started"
    );

    loop {
        if *shutdown.borrow() {
            break;
        }

        let mut worked = false;

        for queue in Queue::all() {
            let claimed = match claim(&db, queue, &config).await {
                Ok(claimed) => claimed,
                Err(error) => {
                    error!(queue = queue.table(), %error, "claiming jobs failed");
                    continue;
                }
            };

            for job in claimed {
                worked = true;
                info!(
                    job_id = job.id,
                    queue = job.queue.table(),
                    attempts = job.attempts,
                    priority = job.priority,
                    "claimed job"
                );

                let outcome = run_job(&db, &job).await;

                match &outcome {
                    Outcome::Success => info!(job_id = job.id, "job succeeded"),
                    Outcome::Retryable(message) => {
                        warn!(job_id = job.id, %message, "job failed and will be retried")
                    }
                    Outcome::Permanent(message) => {
                        error!(job_id = job.id, %message, "job failed permanently")
                    }
                }

                if let Err(error) = finish(&db, &job, outcome, &config).await {
                    error!(job_id = job.id, %error, "recording the job result failed");
                }
            }
        }

        if !worked {
            tokio::select! {
                _ = tokio::time::sleep(config.poll_interval) => {}
                result = shutdown.changed() => {
                    // The sender is gone: stop the loop instead of spinning.
                    if result.is_err() {
                        break;
                    }
                }
            }
        }
    }

    info!(worker = %config.worker_name, "worker stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_follows_canvas() {
        // attempts 1, 2 and 3 give +6 s, +21 s and +86 s.
        assert_eq!(backoff_seconds(1), 6);
        assert_eq!(backoff_seconds(2), 21);
        assert_eq!(backoff_seconds(3), 86);
    }

    #[test]
    fn handler_parsing_splits_tag_and_payload() {
        let (tag, payload) = parse_handler("AccountReport#delete_account_report_rows").unwrap();
        assert_eq!(tag, "AccountReport#delete_account_report_rows");
        assert!(payload.is_null());

        let (tag, payload) =
            parse_handler("AccountReport#delete_account_report_rows {\"id\": 12}").unwrap();
        assert_eq!(tag, "AccountReport#delete_account_report_rows");
        assert_eq!(payload["id"], 12);
    }

    #[test]
    fn canvas_yaml_handlers_are_not_executable() {
        let error = parse_handler("--- !ruby/object:Proc {}").unwrap_err();
        assert!(error.contains("not executable"));
    }

    #[test]
    fn the_tag_is_the_fallback_for_the_handler() {
        let job = ClaimedJob {
            queue: Queue::Jobs,
            id: 1,
            tag: Some("AccountReport#generate".to_string()),
            handler: None,
            attempts: 0,
            max_attempts: None,
            priority: 0,
            user_id: None,
            context_type: None,
            context_id: None,
            run_at: Utc::now(),
        };
        assert_eq!(resolve_tag(&job).as_deref(), Some("AccountReport#generate"));

        let job = ClaimedJob {
            handler: Some("SisBatch#process_instructure_csv_zip".to_string()),
            ..job
        };
        assert_eq!(
            resolve_tag(&job).as_deref(),
            Some("SisBatch#process_instructure_csv_zip")
        );
    }
}
