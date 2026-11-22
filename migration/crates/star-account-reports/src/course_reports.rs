//! Course reports — Canvas `CourseReportsController` (parity gap).
//!
//! Routes:
//!   - GET  /courses/{courseId}/reports/{reportType}         (last)
//!   - GET  /courses/{courseId}/reports/{reportType}/{id}    (show)
//!   - POST /courses/{courseId}/reports/{reportType}         (create)
//!
//! Canvas semantics:
//!   - `last`: the caller's most recent active report of the type — `{}`
//!     when none exists.
//!   - `show`: active report by id (404 when deleted/missing); read right.
//!   - `create`: course context only, `report_type` must be one of the
//!     available reports (`course_pace_docx`), `read_reports` right; the
//!     row is created `created` state with a Progress at 0 — no background
//!     job runs in this workspace, so progress stays 0 until an external
//!     worker flips it (Canvas queues report.run_report on a strand).
//!
//! Serializer (`course_report_json`): id, parameters, status
//! (workflow_state), report_type, course_id, created_at, started_at,
//! ended_at, file_url/attachment (none here), progress (completion;
//! complete → 100).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `available_reports`.
const AVAILABLE_REPORTS: &[&str] = &["course_pace_docx"];

#[derive(Debug, Clone, sqlx::FromRow)]
struct CourseReportRow {
    id: i64,
    course_id: i64,
    /// Kept for row completeness; the serializer does not emit it.
    #[allow(dead_code)]
    user_id: i64,
    workflow_state: String,
    report_type: String,
    parameters: Option<String>,
    start_at: Option<chrono::DateTime<chrono::Utc>>,
    end_at: Option<chrono::DateTime<chrono::Utc>>,
    created_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn report_json(r: &CourseReportRow) -> Value {
    json!({
        "id": r.id,
        "parameters": r
            .parameters
            .as_deref()
            .and_then(|p| serde_json::from_str::<Value>(p).ok())
            .unwrap_or(Value::Null),
        "status": r.workflow_state,
        "report_type": r.report_type,
        "course_id": r.course_id,
        "created_at": r.created_at.map(|d| d.to_rfc3339()),
        "started_at": r.start_at.map(|d| d.to_rfc3339()),
        "ended_at": r.end_at.map(|d| d.to_rfc3339()),
        "file_url": Value::Null,
        "progress": if r.workflow_state == "complete" { 100 } else { 0 },
    })
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateReport {
    pub parameters: Option<CreateParameters>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CreateParameters {
    pub enrollment_ids: Option<Vec<i64>>,
    pub section_ids: Option<Vec<i64>>,
}

/// get_context: the course must exist.
async fn load_course(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(course_id)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// `read_reports` on the course: teacher/ta or admin.
async fn can_read_reports(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
           AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

async fn fetch_active_report(pool: &PgPool, report_id: i64) -> Result<CourseReportRow, ApiError> {
    sqlx::query_as(
        "SELECT id, course_id, user_id, workflow_state, report_type, parameters, \
                start_at, end_at, created_at \
         FROM course_reports WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(report_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/reports/{reportType} — `last`
async fn last_report(
    State(state): State<AppState>,
    Path((course_id, report_type)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    load_course(&state.db, course_id).await?;
    if !can_read_reports(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let row: Option<CourseReportRow> = sqlx::query_as(
        "SELECT id, course_id, user_id, workflow_state, report_type, parameters, \
                start_at, end_at, created_at \
         FROM course_reports \
         WHERE course_id = $1 AND user_id = $2 AND report_type = $3 \
           AND workflow_state <> 'deleted' \
         ORDER BY created_at DESC NULLS LAST, id DESC LIMIT 1",
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(&report_type)
    .fetch_optional(&state.db)
    .await?;
    // Canvas renders {} when no report exists.
    Ok(Json(
        row.as_ref().map(report_json).unwrap_or_else(|| json!({})),
    ))
}

/// GET /courses/{courseId}/reports/{reportType}/{id} — `show`
async fn show_report(
    State(state): State<AppState>,
    Path((course_id, _report_type, report_id)): Path<(i64, String, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    load_course(&state.db, course_id).await?;
    if !can_read_reports(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let report = fetch_active_report(&state.db, report_id).await?;
    if report.course_id != course_id {
        return Err(ApiError::NotFound);
    }
    Ok(Json(report_json(&report)))
}

/// POST /courses/{courseId}/reports/{reportType} — `create`
async fn create_report(
    State(state): State<AppState>,
    Path((course_id, report_type)): Path<(i64, String)>,
    auth: AuthUser,
    payload: Option<Json<CreateReport>>,
) -> Result<Json<Value>, ApiError> {
    load_course(&state.db, course_id).await?;
    if !AVAILABLE_REPORTS.contains(&report_type.as_str()) {
        return Err(ApiError::BadRequest(format!(
            "invalid report type {report_type}"
        )));
    }
    if !can_read_reports(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    // parameters: enrollment_ids/section_ids arrays, JSON-encoded into the
    // text column the way Canvas stores the hash.
    let parameters = payload
        .and_then(|Json(p)| p.parameters)
        .map(|p| {
            json!({
                "enrollment_ids": p.enrollment_ids.unwrap_or_default(),
                "section_ids": p.section_ids.unwrap_or_default(),
            })
            .to_string()
        })
        .unwrap_or_else(|| json!({"enrollment_ids": [], "section_ids": []}).to_string());

    let root_account: Option<i64> = sqlx::query_scalar(
        "WITH RECURSIVE chain AS ( \
            SELECT id, parent_account_id FROM accounts WHERE id = \
              (SELECT account_id FROM courses WHERE id = $1) \
            UNION ALL \
            SELECT a.id, a.parent_account_id FROM accounts a \
              JOIN chain c ON a.id = c.parent_account_id) \
         SELECT id FROM chain WHERE parent_account_id IS NULL LIMIT 1",
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let root_account = root_account.ok_or(ApiError::NotFound)?;

    let row: CourseReportRow = sqlx::query_as(
        "INSERT INTO course_reports \
            (root_account_id, course_id, user_id, workflow_state, report_type, parameters, \
             job_ids, created_at, updated_at) \
         VALUES ($1, $2, $3, 'created', $4, $5, '{}', now(), now()) \
         RETURNING id, course_id, user_id, workflow_state, report_type, parameters, \
                   start_at, end_at, created_at",
    )
    .bind(root_account)
    .bind(course_id)
    .bind(auth.id)
    .bind(&report_type)
    .bind(&parameters)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(report_json(&row)))
}

/// Mount the course-report routes.
pub fn course_reports_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/reports/{reportType}",
            get(last_report).post(create_report),
        )
        .route(
            "/courses/{courseId}/reports/{reportType}/{id}",
            get(show_report),
        )
}
