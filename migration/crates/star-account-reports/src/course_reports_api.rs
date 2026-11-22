//! course_reports — Canvas `CourseReportsController` parity.
//!
//! Routes (the crate router mounts this module under the `/api/v1` nest):
//! - `GET  /courses/{course_id}/reports/{report_type}` — `last`: the most
//!   recent report of the type **by the caller** (`{}` when there is none)
//! - `POST /courses/{course_id}/reports/{report_type}` — `create`: a new
//!   report of the single configured type, run through a `course_report`
//!   job
//! - `GET  /courses/{course_id}/reports/{report_type}/{report_id}` —
//!   `show`
//!
//! Canvas controller (`app/controllers/course_reports_controller.rb`):
//! - `last` scopes `course_reports.active.where(user: @current_user,
//!   report_type:).by_recency.take` and renders `{}` when nothing matches;
//! - `create` answers `{"error": "invalid report type <t>"}` with 400 for
//!   every type outside `available_reports` (this checkout: exactly
//!   `course_pace_docx`), then guards `:read_reports` on the course
//!   context, creates the report with the permitted parameters
//!   (`enrollment_ids[]`, `section_ids[]`) and a `course_report` progress
//!   job (strand `course_report:{course_id}`), and renders
//!   `course_report_json`;
//! - `show` finds the course's active report and guards `:read` on the
//!   record itself.
//!
//! `course_report_json` (`lib/api/v1/course_report.rb`): `id`,
//! `parameters`, `status` (the workflow state), `report_type`,
//! `course_id`, `created_at`, `started_at`, `ended_at`, `file_url` and
//! `progress` (100 when complete, else 0).

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::account_reports_api::ReportError;

/// A `course_reports` row.
#[derive(Debug, sqlx::FromRow)]
struct CourseReportRow {
    id: i64,
    course_id: i64,
    user_id: i64,
    report_type: String,
    workflow_state: String,
    parameters: Option<String>,
    start_at: Option<chrono::NaiveDateTime>,
    end_at: Option<chrono::NaiveDateTime>,
    created_at: chrono::NaiveDateTime,
}

/// The single report type of this Canvas checkout
/// (`CourseReportsController#available_reports`).
const COURSE_REPORT_TYPES: [&str; 1] = ["course_pace_docx"];

/// Domain router for the Canvas course-report routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{course_id}/reports/{report_type}",
            get(last_report).post(create_report),
        )
        .route(
            "/courses/{course_id}/reports/{report_type}/{report_id}",
            get(show_report),
        )
}

/// A missing course is a 404, checked before the permission of the caller
/// (Canvas `get_context`).
async fn load_course(pool: &PgPool, raw_course_id: &str) -> Result<i64, ReportError> {
    let course_id: i64 = raw_course_id.parse().map_err(|_| ApiError::NotFound)?;
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    row.map(|(id,)| id).ok_or(ApiError::NotFound.into())
}

/// Canvas `:read_reports` on a course context — the teachers and TAs of
/// the course (and site admins) hold it.
async fn authorize_read_reports(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<(), ReportError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE course_id = $1 AND user_id = $2 AND role IN ('teacher', 'ta') \
           AND workflow_state <> 'deleted' LIMIT 1",
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden.into())
    }
}

/// `GET /courses/{course_id}/reports/{report_type}` — `course_reports#last`:
/// the caller's most recent report of the type, `{}` when there is none.
async fn last_report(
    State(state): State<AppState>,
    Path((course_id, report_type)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Response, ReportError> {
    let course_id = load_course(&state.db, &course_id).await?;

    let row: Option<CourseReportRow> = sqlx::query_as(
        "SELECT id, course_id, user_id, report_type, workflow_state, parameters, \
                start_at, end_at, created_at \
         FROM course_reports \
         WHERE course_id = $1 AND user_id = $2 AND report_type = $3 \
           AND workflow_state <> 'deleted' \
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(&report_type)
    .fetch_optional(&state.db)
    .await?;

    match row {
        Some(row) => Ok(Json(course_report_json(&row)).into_response()),
        None => Ok(Json(json!({})).into_response()),
    }
}

/// `POST /courses/{course_id}/reports/{report_type}` —
/// `course_reports#create`: 400 for an unknown type, then the
/// `:read_reports` gate, then the report + its `course_report` job.
async fn create_report(
    State(state): State<AppState>,
    Path((course_id, report_type)): Path<(String, String)>,
    auth: AuthUser,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ReportError> {
    let course_id = load_course(&state.db, &course_id).await?;

    // Canvas answers `{"error": "invalid report type <t>"}` with 400 for
    // every type outside the registry.
    if !COURSE_REPORT_TYPES.contains(&report_type.as_str()) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!("invalid report type {report_type}")
            })),
        )
            .into_response());
    }

    authorize_read_reports(&state.db, &auth, course_id).await?;

    // The permitted parameters (`enrollment_ids[]`, `section_ids[]`) ride
    // on the body exactly like the account-report routes.
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let parameters = parse_parameters(content_type, &body);
    let parameters_text = parameters.as_ref().map(|value| value.to_string());

    let row: CourseReportRow = sqlx::query_as(
        "INSERT INTO course_reports \
             (course_id, user_id, report_type, workflow_state, parameters, progress) \
         VALUES ($1, $2, $3, 'created', $4, 0) \
         RETURNING id, course_id, user_id, report_type, workflow_state, parameters, \
                   start_at, end_at, created_at",
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(&report_type)
    .bind(parameters_text.as_deref())
    .fetch_one(&state.db)
    .await?;

    // Canvas enqueues a `course_report` progress job on the report's strand
    // (`course_report:{course_id}`); star-lms queues it in the `jobs` table.
    sqlx::query(
        "INSERT INTO jobs (tag, workflow_state, progress, user_id, context_type, context_id) \
         VALUES ('course_report#run_report', 'queued', 0, $1, 'CourseReport', $2)",
    )
    .bind(auth.id)
    .bind(row.id)
    .execute(&state.db)
    .await?;

    Ok(Json(course_report_json(&row)).into_response())
}

/// `GET /courses/{course_id}/reports/{report_type}/{report_id}` —
/// `course_reports#show`: the course's active report, guarded by `:read`
/// on the record (the owner and the course's report readers may see it).
async fn show_report(
    State(state): State<AppState>,
    Path((course_id, report_type, report_id)): Path<(String, String, String)>,
    auth: AuthUser,
) -> Result<Response, ReportError> {
    let course_id = load_course(&state.db, &course_id).await?;

    let row: Option<CourseReportRow> = sqlx::query_as(
        "SELECT id, course_id, user_id, report_type, workflow_state, parameters, \
                start_at, end_at, created_at \
         FROM course_reports \
         WHERE course_id = $1 AND id = $2 AND report_type = $3 \
           AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .bind(report_id.parse::<i64>().map_err(|_| ApiError::NotFound)?)
    .bind(&report_type)
    .fetch_optional(&state.db)
    .await?;
    let row = row.ok_or(ApiError::NotFound)?;

    // `:read` on the record: the owner or a `:read_reports` holder.
    if row.user_id != auth.id {
        authorize_read_reports(&state.db, &auth, course_id).await?;
    }

    Ok(Json(course_report_json(&row)).into_response())
}

/// `Api::V1::CourseReport#course_report_json`.
fn course_report_json(row: &CourseReportRow) -> Value {
    let mut map = Map::new();
    map.insert("id".into(), Value::from(row.id));
    map.insert(
        "parameters".into(),
        row.parameters
            .as_deref()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .unwrap_or_else(|| json!({})),
    );
    map.insert("status".into(), Value::String(row.workflow_state.clone()));
    map.insert("report_type".into(), Value::String(row.report_type.clone()));
    map.insert("course_id".into(), Value::from(row.course_id));
    map.insert("created_at".into(), iso8601(row.created_at));
    map.insert("started_at".into(), iso8601_opt(row.start_at));
    map.insert("ended_at".into(), iso8601_opt(row.end_at));
    // `file_url` is null until an attachment exists (star has no file
    // backend for these reports).
    map.insert("file_url".into(), Value::Null);
    map.insert(
        "progress".into(),
        Value::from(if row.workflow_state == "complete" {
            100
        } else {
            0
        }),
    );
    Value::Object(map)
}

/// The `Time#iso8601` rendering of a naive timestamp (UTC).
fn iso8601(value: chrono::NaiveDateTime) -> Value {
    iso8601_opt(Some(value))
}

fn iso8601_opt(value: Option<chrono::NaiveDateTime>) -> Value {
    value.map_or(Value::Null, |at| {
        Value::String(at.and_local_timezone(chrono::Utc).unwrap().to_rfc3339())
    })
}

/// Read the `parameters` of the request body — the same parsing the
/// account-report routes use (JSON body or form parameters).
fn parse_parameters(content_type: &str, body: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(body);
    if content_type.starts_with("application/json") || text.trim_start().starts_with('{') {
        serde_json::from_str::<Value>(&text).ok()
    } else {
        // Form parameters — `parameters[section_ids][]=1&...` style — are
        // folded into a JSON object of arrays.
        let mut map = Map::new();
        for pair in text.split('&') {
            let Some((key, value)) = pair.split_once('=') else {
                continue;
            };
            let key = key.replace("%5B%5D", "[]").replace("%5D", "]");
            if let Some(name) = key.strip_prefix("parameters[") {
                let name = name.trim_end_matches(']').trim_end_matches("[]");
                if name.is_empty() {
                    continue;
                }
                let entry = map
                    .entry(name.to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Value::Array(items) = entry {
                    items.push(Value::String(value.to_string()));
                }
            }
        }
        (!map.is_empty()).then_some(Value::Object(map))
    }
}
