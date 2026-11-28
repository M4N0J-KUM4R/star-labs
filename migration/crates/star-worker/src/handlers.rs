//! The handler registry — the `handler` string names the work to do.
//!
//! Canvas stores the object that performs the work in the `handler` column
//! (`canvas-src/db/migrate/20101216224513_create_delayed_jobs.rb:31-32`) and
//! the human-readable `Class#method` name in `tag` (`:49`). The port writes
//! the `Class#method` string into both columns, so the worker's tag is the
//! `handler` string when it is set and the `tag` otherwise.
//!
//! A handler that the port cannot run yet must still exist and must report
//! the work as **not implemented** — a job that silently succeeds is a lie
//! the Jobs API then shows as a completed export.

use serde_json::Value;
use sqlx::PgPool;

/// Why a handler failed.
#[derive(Debug)]
pub enum JobError {
    /// The work failed but may succeed on a later attempt.
    Retryable(String),
    /// The work can never succeed as it was requested.
    Permanent(String),
}

impl JobError {
    /// The message recorded in `last_error`.
    pub fn message(&self) -> &str {
        match self {
            Self::Retryable(message) | Self::Permanent(message) => message,
        }
    }
}

/// The context a handler runs in.
pub struct JobCtx<'a> {
    /// The database the handler works against.
    pub db: &'a PgPool,
    /// The id of the job row.
    pub job_id: i64,
    /// The queue table the row came from.
    pub queue: &'static str,
    /// The resolved job tag, e.g. `AccountReport#generate`.
    pub tag: String,
    /// The user the job was enqueued for.
    pub user_id: Option<i64>,
    /// The Canvas context type the job belongs to (`AccountReport`, …).
    pub context_type: Option<String>,
    /// The Canvas context id the job belongs to.
    pub context_id: Option<i64>,
    /// The handler payload, parsed from the arguments after the tag.
    pub payload: Value,
}

impl JobCtx<'_> {
    /// The id the handler works on: the job's context id, or the `id` in the
    /// payload.
    pub fn target_id(&self) -> Option<i64> {
        self.context_id
            .or_else(|| self.payload.get("id").and_then(Value::as_i64))
    }
}

/// Run the handler the tag names.
///
/// The registry is an explicit match rather than a map so that every tag the
/// port knows about — runnable or not — is visible in one place.
pub async fn dispatch(ctx: &JobCtx<'_>) -> Result<(), JobError> {
    match ctx.tag.as_str() {
        "AccountReport#delete_account_report_rows" => delete_account_report_rows(ctx).await,
        "OutcomeImport::run" => outcome_import_run(ctx).await,
        other => Err(not_implemented(other)),
    }
}

/// The failure a tag that the port cannot run produces.
///
/// The message is stored in `last_error`, so the Jobs API shows exactly what
/// is missing instead of a job that looks finished.
pub fn not_implemented(tag: &str) -> JobError {
    let detail = match tag {
        "AccountReport#generate" => {
            "star-lms has no account report runner (Canvas runs AccountReportRunner)"
        }
        "ContentMigration#import_content" => {
            "star-lms has no course copy worker (Canvas runs Canvas::Migration::Worker)"
        }
        "ContentExport#export_content" => {
            "star-lms has no export archive writer (Canvas runs ContentMigration)"
        }
        "SisBatch#process_instructure_csv_zip" => {
            "star-lms has no SIS CSV importer and no file byte storage"
        }
        "AccountNotification#create_alert" => {
            "star-lms has no notification delivery (no mailer)"
        }
        "BounceNotificationProcessor#process" => {
            "star-lms has no notification delivery (no mailer)"
        }
        "Delayed::Job" => {
            "the Delayed::Job queue record is not executable work; star-lms has no periodic job runner"
        }
        "PeriodicJobs" => "star-lms has no periodic job scheduler",
        _ => "no handler is registered for this job tag",
    };

    JobError::Permanent(format!("not implemented: {tag} ({detail})"))
}

/// `AccountReport#delete_account_report_rows`
///
/// Canvas (`canvas-src/app/models/account_report.rb:94-96`) deletes the rows
/// of the report in batches of 10 000. The port has the `account_report_rows`
/// table, so this handler runs for real.
async fn delete_account_report_rows(ctx: &JobCtx<'_>) -> Result<(), JobError> {
    let report_id = ctx.target_id().ok_or_else(|| {
        JobError::Permanent(
            "AccountReport#delete_account_report_rows has no account report id in the job row"
                .to_string(),
        )
    })?;

    sqlx::query("DELETE FROM account_report_rows WHERE account_report_id = $1")
        .bind(report_id)
        .execute(ctx.db)
        .await
        .map_err(|error| {
            JobError::Retryable(format!("deleting account report rows failed: {error}"))
        })?;

    Ok(())
}

/// `OutcomeImport::run`
///
/// The port processes an outcome import synchronously when it is created
/// (`crates/star-outcomes/src/outcomes_api.rs:824` writes the row with
/// `workflow_state = 'processed'`), so the job row is the record of work that
/// is already done. The handler verifies that and completes the job; an
/// import that is not processed yet fails as not implemented rather than
/// claiming success.
async fn outcome_import_run(ctx: &JobCtx<'_>) -> Result<(), JobError> {
    let import_id = ctx.target_id().ok_or_else(|| {
        JobError::Permanent(
            "OutcomeImport::run has no outcome import id in the job row".to_string(),
        )
    })?;

    let row: Option<(String,)> =
        sqlx::query_as("SELECT workflow_state FROM outcome_imports WHERE id = $1")
            .bind(import_id)
            .fetch_optional(ctx.db)
            .await
            .map_err(|error| {
                JobError::Retryable(format!(
                    "reading outcome import {import_id} failed: {error}"
                ))
            })?;

    match row {
        Some((state,)) if state == "processed" => Ok(()),
        Some((state,)) => Err(JobError::Permanent(format!(
            "not implemented: OutcomeImport::run (outcome import {import_id} is '{state}'; \
             star-lms imports outcomes synchronously at request time and has no background \
             outcome importer)"
        ))),
        None => Err(JobError::Permanent(format!(
            "not implemented: OutcomeImport::run (outcome import {import_id} does not exist)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tags_report_not_implemented() {
        let error = not_implemented("AccountReport#generate");
        assert!(matches!(error, JobError::Permanent(_)));
        assert!(error
            .message()
            .starts_with("not implemented: AccountReport#generate"));
    }

    #[test]
    fn unknown_tags_report_not_implemented() {
        let error = not_implemented("Some::Unknown#method");
        assert!(matches!(error, JobError::Permanent(_)));
        assert!(error.message().contains("no handler is registered"));
    }
}
