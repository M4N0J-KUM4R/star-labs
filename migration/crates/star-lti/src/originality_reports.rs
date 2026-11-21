//! lti/originality_reports — Canvas `Lti::OriginalityReportsApiController`
//! parity.
//!
//! Routes (the LTI Advantage router mounts this module at the app root, so
//! the paths carry the literal `/api/lti` prefix):
//! - `POST /api/lti/assignments/{assignmentId}/submissions/{submissionId}/originality_report`
//! - `GET  /api/lti/assignments/{assignmentId}/submissions/{submissionId}/originality_report/{id}`
//! - `PUT  /api/lti/assignments/{assignmentId}/submissions/{submissionId}/originality_report/{id}`
//! - `GET  /api/lti/assignments/{assignmentId}/files/{fileId}/originality_report`
//! - `PUT  /api/lti/assignments/{assignmentId}/files/{fileId}/originality_report`
//!
//! Canvas routes (config/routes.rb, the LTI API block):
//!   post "assignments/:assignment_id/submissions/:submission_id/originality_report", action: :create
//!   put  "assignments/:assignment_id/submissions/:submission_id/originality_report/:id", action: :update
//!   put  "assignments/:assignment_id/files/:file_id/originality_report", action: :update
//!   get  "assignments/:assignment_id/submissions/:submission_id/originality_report/:id", action: :show
//!   get  "assignments/:assignment_id/files/:file_id/originality_report", action: :show
//!
//! Canvas controller (`app/controllers/lti/originality_reports_api_controller.rb`):
//! the `before_action` chain is `authorized_lti2_tool` (401), then
//! `find_originality_report` (404), `report_in_context` (404) and
//! `ensure_tool_proxy_associated` (401).
//!
//! The report JSON is the Canvas `OriginalityReport` shape: `id`, `file_id`,
//! `originality_score`, `originality_report_file_id`,
//! `originality_report_url`, `workflow_state`, `error_report` (the error
//! message), `submission_time` and `root_account_id`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_common::{ApiError, AppState};

/// An `originality_reports` row.
#[derive(Debug, sqlx::FromRow)]
struct OriginalityReport {
    id: i64,
    attachment_id: Option<i64>,
    originality_score: Option<f64>,
    originality_report_attachment_id: Option<i64>,
    originality_report_url: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    submission_id: i64,
    workflow_state: String,
    error_message: Option<String>,
    submission_time: Option<chrono::DateTime<chrono::Utc>>,
    root_account_id: i64,
}

/// The columns of an `originality_reports` row.
const REPORT_COLUMNS: &str = "id, attachment_id, originality_score, \
     originality_report_attachment_id, originality_report_url, \
     created_at::timestamptz AS created_at, updated_at::timestamptz AS updated_at, \
     submission_id, workflow_state, error_message, \
     submission_time::timestamptz AS submission_time, root_account_id";

/// The body of a create or update request.
#[derive(Debug, Default, Deserialize)]
struct ReportRequest {
    originality_report: Option<ReportFields>,
}

/// The fields of a report in a request.
#[derive(Debug, Default, Deserialize)]
struct ReportFields {
    file_id: Option<i64>,
    originality_score: Option<f64>,
    originality_report_url: Option<String>,
    originality_report_file_id: Option<i64>,
    workflow_state: Option<String>,
    error_report: Option<String>,
    tool_setting: Option<Value>,
}

/// Domain router for the Canvas originality-report routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/lti/assignments/{assignment_id}/submissions/{submission_id}/originality_report",
            axum::routing::post(create_report),
        )
        .route(
            "/api/lti/assignments/{assignment_id}/submissions/{submission_id}/originality_report/{id}",
            get(show_report).put(update_submission_report),
        )
        .route(
            "/api/lti/assignments/{assignment_id}/files/{file_id}/originality_report",
            get(show_file_report).put(update_file_report),
        )
}

/// `POST /api/lti/assignments/:assignment_id/submissions/:submission_id/originality_report`.
async fn create_report(
    State(state): State<AppState>,
    Path((assignment_id, submission_id)): Path<(i64, i64)>,
    body: Option<Json<ReportRequest>>,
) -> Result<Response, Box<Response>> {
    require_assignment(&state.db, assignment_id).await?;
    require_submission(&state.db, assignment_id, submission_id).await?;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    let fields = body.originality_report.unwrap_or_default();
    let Some(score) = fields.originality_score else {
        return Err(bad_request("Originality score is required"));
    };

    // Canvas updates the report of the submission when there already is one.
    if let Some(report) = report_of_submission(&state.db, submission_id).await? {
        return update_report(&state, report, assignment_id, submission_id, fields).await;
    }

    let root_account_id = root_account_of_course(&state.db, assignment_id).await?;
    let workflow_state = fields
        .workflow_state
        .clone()
        .unwrap_or_else(|| "pending".to_string());

    let sql = format!(
        "INSERT INTO originality_reports \
             (attachment_id, originality_score, originality_report_attachment_id, \
              originality_report_url, created_at, updated_at, submission_id, workflow_state, \
              error_message, submission_time, root_account_id) \
         VALUES ($1, $2, $3, $4, now(), now(), $5, $6, $7, $8, $9) \
         RETURNING {REPORT_COLUMNS}"
    );
    let report: Option<OriginalityReport> = sqlx::query_as(&sql)
        .bind(fields.file_id)
        .bind(score)
        .bind(fields.originality_report_file_id)
        .bind(&fields.originality_report_url)
        .bind(submission_id)
        .bind(&workflow_state)
        .bind(&fields.error_report)
        .bind(submission_time_of(&state.db, submission_id).await?)
        .bind(root_account_id)
        .fetch_optional(&state.db)
        .await
        .map_err(server_error)?;

    match report {
        Some(report) => Ok((StatusCode::CREATED, Json(report_json(&report))).into_response()),
        None => Err(bad_request("The originality report could not be saved")),
    }
}

/// `GET /api/lti/assignments/:assignment_id/submissions/:submission_id/originality_report/:id`.
async fn show_report(
    State(state): State<AppState>,
    Path((assignment_id, submission_id, id)): Path<(i64, i64, i64)>,
) -> Result<Response, Box<Response>> {
    require_assignment(&state.db, assignment_id).await?;
    require_submission(&state.db, assignment_id, submission_id).await?;
    let report = load_report(&state.db, id).await?;
    let Some(report) = report else {
        return Err(not_found());
    };
    if report.submission_id != submission_id {
        return Err(not_found());
    }
    Ok(Json(report_json(&report)).into_response())
}

/// `PUT /api/lti/assignments/:assignment_id/submissions/:submission_id/originality_report/:id`.
async fn update_submission_report(
    State(state): State<AppState>,
    Path((assignment_id, submission_id, id)): Path<(i64, i64, i64)>,
    body: Option<Json<ReportRequest>>,
) -> Result<Response, Box<Response>> {
    require_assignment(&state.db, assignment_id).await?;
    require_submission(&state.db, assignment_id, submission_id).await?;
    let report = load_report(&state.db, id).await?;
    let Some(report) = report else {
        return Err(not_found());
    };
    if report.submission_id != submission_id {
        return Err(not_found());
    }
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let fields = body.originality_report.unwrap_or_default();
    update_report(&state, report, assignment_id, submission_id, fields).await
}

/// The shared update of a report.
async fn update_report(
    state: &AppState,
    report: OriginalityReport,
    _assignment_id: i64,
    _submission_id: i64,
    fields: ReportFields,
) -> Result<Response, Box<Response>> {
    let score = fields.originality_score.or(report.originality_score);
    let url = fields
        .originality_report_url
        .clone()
        .or(report.originality_report_url.clone());
    let report_file_id = fields
        .originality_report_file_id
        .or(report.originality_report_attachment_id);
    let workflow_state = fields
        .workflow_state
        .clone()
        .unwrap_or_else(|| report.workflow_state.clone());
    let error_message = fields.error_report.clone().or(report.error_message.clone());

    sqlx::query(
        "UPDATE originality_reports SET originality_score = $1, originality_report_url = $2, \
             originality_report_attachment_id = $3, workflow_state = $4, error_message = $5, \
             updated_at = now() WHERE id = $6",
    )
    .bind(score)
    .bind(url)
    .bind(report_file_id)
    .bind(workflow_state)
    .bind(error_message)
    .bind(report.id)
    .execute(&state.db)
    .await
    .map_err(server_error)?;

    let report = load_report(&state.db, report.id).await?;
    let Some(report) = report else {
        return Err(not_found());
    };
    Ok(Json(report_json(&report)).into_response())
}

/// `GET /api/lti/assignments/:assignment_id/files/:file_id/originality_report`.
async fn show_file_report(
    State(state): State<AppState>,
    Path((assignment_id, file_id)): Path<(i64, i64)>,
) -> Result<Response, Box<Response>> {
    require_assignment(&state.db, assignment_id).await?;
    let report = report_of_attachment(&state.db, file_id).await?;
    let Some(report) = report else {
        return Err(not_found());
    };
    Ok(Json(report_json(&report)).into_response())
}

/// `PUT /api/lti/assignments/:assignment_id/files/:file_id/originality_report`.
async fn update_file_report(
    State(state): State<AppState>,
    Path((assignment_id, file_id)): Path<(i64, i64)>,
    body: Option<Json<ReportRequest>>,
) -> Result<Response, Box<Response>> {
    require_assignment(&state.db, assignment_id).await?;
    let report = report_of_attachment(&state.db, file_id).await?;
    let Some(report) = report else {
        return Err(not_found());
    };
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let fields = body.originality_report.unwrap_or_default();
    update_report(&state, report, assignment_id, 0, fields).await
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn not_found() -> Box<Response> {
    Box::new((StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response())
}

fn bad_request(message: &str) -> Box<Response> {
    Box::new((StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response())
}

fn server_error(error: sqlx::Error) -> Box<Response> {
    Box::new(
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    )
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// The assignment of a route has to exist — 404.
async fn require_assignment(db: &PgPool, assignment_id: i64) -> Result<(), Box<Response>> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM assignments WHERE id = $1")
        .bind(assignment_id)
        .fetch_optional(db)
        .await
        .map_err(server_error)?;
    if row.is_some() {
        Ok(())
    } else {
        Err(not_found())
    }
}

/// The submission has to belong to the assignment — 404.
async fn require_submission(
    db: &PgPool,
    assignment_id: i64,
    submission_id: i64,
) -> Result<(), Box<Response>> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM submissions WHERE id = $1 AND assignment_id = $2")
            .bind(submission_id)
            .bind(assignment_id)
            .fetch_optional(db)
            .await
            .map_err(server_error)?;
    if row.is_some() {
        Ok(())
    } else {
        Err(not_found())
    }
}

/// The report of an id.
async fn load_report(db: &PgPool, id: i64) -> Result<Option<OriginalityReport>, Box<Response>> {
    let sql = format!("SELECT {REPORT_COLUMNS} FROM originality_reports WHERE id = $1");
    sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(db)
        .await
        .map_err(server_error)
}

/// The report of a submission.
async fn report_of_submission(
    db: &PgPool,
    submission_id: i64,
) -> Result<Option<OriginalityReport>, Box<Response>> {
    let sql = format!(
        "SELECT {REPORT_COLUMNS} FROM originality_reports \
         WHERE submission_id = $1 ORDER BY id DESC LIMIT 1"
    );
    sqlx::query_as(&sql)
        .bind(submission_id)
        .fetch_optional(db)
        .await
        .map_err(server_error)
}

/// The report of a file.
async fn report_of_attachment(
    db: &PgPool,
    file_id: i64,
) -> Result<Option<OriginalityReport>, Box<Response>> {
    let sql = format!(
        "SELECT {REPORT_COLUMNS} FROM originality_reports \
         WHERE attachment_id = $1 ORDER BY id DESC LIMIT 1"
    );
    sqlx::query_as(&sql)
        .bind(file_id)
        .fetch_optional(db)
        .await
        .map_err(server_error)
}

/// The root account of the course of an assignment.
async fn root_account_of_course(db: &PgPool, assignment_id: i64) -> Result<i64, Box<Response>> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT c.account_id FROM assignments a JOIN courses c ON c.id = a.course_id \
         WHERE a.id = $1",
    )
    .bind(assignment_id)
    .fetch_optional(db)
    .await
    .map_err(server_error)?;
    row.map(|row| row.0).ok_or_else(not_found)
}

/// The time the submission was made.
async fn submission_time_of(
    db: &PgPool,
    submission_id: i64,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, Box<Response>> {
    let row: Option<(chrono::DateTime<chrono::Utc>,)> =
        sqlx::query_as("SELECT submitted_at FROM submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_optional(db)
            .await
            .map_err(server_error)?;
    Ok(row.map(|row| row.0))
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// The Canvas `OriginalityReport` JSON.
fn report_json(report: &OriginalityReport) -> Value {
    json!({
        "id": report.id,
        "file_id": report.attachment_id,
        "originality_score": report.originality_score,
        "originality_report_file_id": report.originality_report_attachment_id,
        "originality_report_url": report.originality_report_url,
        "workflow_state": report.workflow_state,
        "error_report": report.error_message,
        "submission_time": report.submission_time.map(|value| value.to_rfc3339()),
        "root_account_id": report.root_account_id,
        "created_at": report.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "updated_at": report.updated_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    })
}
