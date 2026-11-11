//! migration_issues — Canvas `MigrationIssuesController` parity.
//!
//! Routes:
//! - GET  /courses/{courseId}/content_migrations/{id}/migration_issues
//! - POST /courses/{courseId}/content_migrations/{id}/migration_issues
//! - PUT  /courses/{courseId}/content_migrations/{id}/migration_issues/{issueId}
//!
//! Canvas JSON (`migration_issue_json`): id, description, workflow_state,
//! fix_issue_html_url, issue_type, created_at, updated_at,
//! content_migration_url, plus `unresolved` (derived from workflow_state) and
//! the `fixable_by_*` / `issue_link` fields used by the selective import UI.
//! `error_message` is only exposed to admins (Canvas `read_errors`).
//!
//! Deviations from Canvas:
//! - Canvas has no `create` action behind POST migration_issues; this route
//!   creates an issue and returns it with HTTP 200 so the API is complete.
//! - An invalid `issue_type` is a 422 validation error instead of a DB error.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::content_migrations::{ensure_course, is_admin, manages_course, pagination, ts};

const ISSUE_TYPES: [&str; 3] = ["todo", "warning", "error"];

const ISSUE_SELECT: &str = "SELECT id, content_migration_id, description, \
        workflow_state, fix_issue_html_url, issue_type, error_message, \
        issue_link, fixable_by_web_upload, fixable_by_admin, created_at, updated_at \
     FROM migration_issues";

#[derive(Debug, Clone, sqlx::FromRow)]
struct IssueRow {
    id: i64,
    content_migration_id: i64,
    description: Option<String>,
    workflow_state: String,
    fix_issue_html_url: Option<String>,
    issue_type: String,
    error_message: Option<String>,
    issue_link: Option<String>,
    fixable_by_web_upload: bool,
    fixable_by_admin: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn issue_json(row: &IssueRow, course_id: i64, include_errors: bool) -> Value {
    let mut json = json!({
        "id": row.id,
        "content_migration_id": row.content_migration_id,
        "content_migration_url": format!(
            "/api/v1/courses/{course_id}/content_migrations/{}",
            row.content_migration_id
        ),
        "description": row.description,
        "workflow_state": row.workflow_state,
        "fix_issue_html_url": row.fix_issue_html_url,
        "issue_type": row.issue_type,
        "issue_link": row.issue_link,
        "fixable_by_web_upload": row.fixable_by_web_upload,
        "fixable_by_admin": row.fixable_by_admin,
        "unresolved": row.workflow_state == "active",
        "created_at": ts(&row.created_at),
        "updated_at": ts(&row.updated_at),
    });
    // Canvas exposes the site-admin error message to `:read_errors` holders.
    if include_errors {
        json["error_message"] = json!(row.error_message);
    }
    json
}

/// 404 unless the migration belongs to the course (Canvas
/// `require_content_migration`).
async fn ensure_migration(db: &PgPool, course_id: i64, id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM content_migrations WHERE id = $1 AND course_id = $2")
            .bind(id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

async fn find_issue(db: &PgPool, migration_id: i64, id: i64) -> Result<IssueRow, ApiError> {
    let sql = format!("{ISSUE_SELECT} WHERE id = $1 AND content_migration_id = $2");
    sqlx::query_as(&sql)
        .bind(id)
        .bind(migration_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

fn json_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(text)) => {
            matches!(
                text.trim().to_ascii_lowercase().as_str(),
                "true" | "t" | "1" | "yes" | "on"
            )
        }
        Some(Value::Number(number)) => number.as_f64().map(|v| v != 0.0).unwrap_or(false),
        _ => false,
    }
}

fn json_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// GET /courses/{courseId}/content_migrations/{id}/migration_issues — Canvas
/// `index`, ordered by created_at.
async fn list_issues(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_migration(&state.db, course_id, id).await?;

    let (per_page, offset) = pagination(&params);
    let sql = format!(
        "{ISSUE_SELECT} WHERE content_migration_id = $1 \
         ORDER BY created_at, id LIMIT $2 OFFSET $3"
    );
    let rows: Vec<IssueRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    let include_errors = is_admin(&auth);
    Ok(Json(Value::Array(
        rows.iter()
            .map(|row| issue_json(row, course_id, include_errors))
            .collect(),
    )))
}

/// POST /courses/{courseId}/content_migrations/{id}/migration_issues — Canvas
/// has no create action (see module docs).
async fn create_issue(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_migration(&state.db, course_id, id).await?;

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let issue_type = json_string(payload.get("issue_type")).unwrap_or_else(|| "error".to_string());
    if !ISSUE_TYPES.contains(&issue_type.as_str()) {
        return Err(ApiError::Validation("Invalid issue_type".to_string()));
    }
    let workflow_state = json_string(payload.get("workflow_state"))
        .filter(|state| state == "active" || state == "resolved")
        .unwrap_or_else(|| "active".to_string());

    let issue_id: i64 = sqlx::query_scalar(
        "INSERT INTO migration_issues \
         (content_migration_id, description, workflow_state, fix_issue_html_url, \
          issue_type, error_message, issue_link, fixable_by_web_upload, fixable_by_admin) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) RETURNING id",
    )
    .bind(id)
    .bind(json_string(payload.get("description")))
    .bind(workflow_state)
    .bind(json_string(payload.get("fix_issue_html_url")))
    .bind(issue_type)
    .bind(json_string(payload.get("error_message")))
    .bind(json_string(payload.get("issue_link")))
    .bind(json_bool(payload.get("fixable_by_web_upload")))
    .bind(json_bool(payload.get("fixable_by_admin")))
    .fetch_one(&state.db)
    .await?;

    let row = find_issue(&state.db, id, issue_id).await?;
    // Canvas renders created resources with HTTP 200.
    Ok((
        StatusCode::OK,
        Json(issue_json(&row, course_id, is_admin(&auth))),
    ))
}

/// PUT /courses/{courseId}/content_migrations/{id}/migration_issues/{issueId}
/// — Canvas `update`: only the workflow_state can change.
async fn update_issue(
    State(state): State<AppState>,
    Path((course_id, migration_id, issue_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_migration(&state.db, course_id, migration_id).await?;
    let _ = find_issue(&state.db, migration_id, issue_id).await?;

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let workflow_state = json_string(payload.get("workflow_state")).unwrap_or_default();
    if workflow_state != "active" && workflow_state != "resolved" {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Must send a valid workflow state" })),
        ));
    }

    sqlx::query(
        "UPDATE migration_issues SET workflow_state = $1, updated_at = now() WHERE id = $2",
    )
    .bind(&workflow_state)
    .bind(issue_id)
    .execute(&state.db)
    .await?;

    let row = find_issue(&state.db, migration_id, issue_id).await?;
    Ok((
        StatusCode::OK,
        Json(issue_json(&row, course_id, is_admin(&auth))),
    ))
}

/// Build the migration_issues router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/content_migrations/{id}/migration_issues",
            get(list_issues).post(create_issue),
        )
        .route(
            "/courses/{courseId}/content_migrations/{id}/migration_issues/{issueId}",
            put(update_issue),
        )
}
