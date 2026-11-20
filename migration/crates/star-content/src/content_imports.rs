//! content_imports — Canvas `content_imports_controller` parity.
//!
//! Routes:
//! - POST /courses/{courseId}/course_copy      — start a course copy
//! - GET  /courses/{courseId}/course_copy/{id} — the status of a course copy
//!
//! This is Canvas' deprecated course-copy API.  A copy is a content migration
//! of the type `course_copy_importer`, so the rows live in
//! `content_migrations` and the copy is created here.
//!
//! Canvas behavior mirrored here (`lib/api/v1/course.rb`):
//! - the body is `copy_status_json`: `id`, `progress`, `created_at`,
//!   `workflow_state`, `integration_id` and `status_url`.
//! - the legacy API remaps the workflow state: `imported` becomes
//!   `completed`, and every state that is neither `created` nor `failed`
//!   becomes `started`.
//! - `status_url` points at the status route of the same copy.
//! - the caller needs `:manage_course_content_add` on the course and both
//!   `read` and `read_as_admin` on the source course.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// The content types Canvas accepts for a selective copy.
const COPY_CONTENT_TYPES: [&str; 10] = [
    "course_settings",
    "assignments",
    "external_tools",
    "files",
    "topics",
    "calendar_events",
    "quizzes",
    "wiki_pages",
    "modules",
    "outcomes",
];

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct CopyRow {
    id: i64,
    workflow_state: String,
    created_at: DateTime<Utc>,
}

impl CopyRow {
    /// Canvas `copy_status_json`.
    fn to_json(&self, course_id: i64) -> Value {
        fn ts(value: &DateTime<Utc>) -> String {
            value.to_rfc3339_opts(SecondsFormat::Secs, true)
        }

        // The legacy API remaps the migration workflow state.
        let workflow_state = match self.workflow_state.as_str() {
            "imported" => "completed",
            "created" | "failed" => self.workflow_state.as_str(),
            _ => "started",
        };

        json!({
            "id": self.id,
            "progress": 0,
            "created_at": ts(&self.created_at),
            "workflow_state": workflow_state,
            "integration_id": Value::Null,
            "status_url": format!("/api/v1/courses/{course_id}/course_copy/{}", self.id),
        })
    }
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas `api_find(Course, params[:course_id])`: 404 when the course does not
/// exist.
async fn ensure_course(db: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `authorized_action(@context, @current_user, :manage_course_content_add)`.
async fn can_manage_content(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta', 'designer') LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Canvas `authorized_action(@source_course, @current_user, [:read, :read_as_admin],
/// all_rights: true)` — the caller needs both rights on the source course.
async fn can_read_source_course(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta', 'designer') LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// The source course of a copy: the body or query `source_course` is the id
/// of the source course.
///
/// Canvas also accepts a SIS id here; the port stores no `sis_source_id` on
/// a course, so only the numeric id resolves.
async fn find_source_course(db: &PgPool, source: &str) -> Result<Option<i64>, ApiError> {
    let source = source.trim();
    if source.is_empty() {
        return Ok(None);
    }
    let Ok(id) = source.parse::<i64>() else {
        return Ok(None);
    };
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(id)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|_| id))
}

/// The selective copy options of the request (`only[]` / `except[]`).
fn copy_options(payload: &Value, params: &HashMap<String, String>) -> (Vec<String>, Vec<String>) {
    let collect = |value: Option<&Value>| -> Vec<String> {
        match value {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect(),
            Some(Value::String(value)) => value
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect(),
            _ => Vec::new(),
        }
    };
    let from_params = |key: &str| -> Vec<String> {
        params
            .get(key)
            .map(|value| {
                value
                    .split(',')
                    .map(|item| item.trim().to_string())
                    .filter(|item| !item.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let only = {
        let values = collect(payload.get("only"));
        if values.is_empty() {
            from_params("only[]")
        } else {
            values
        }
    };
    let except = {
        let values = collect(payload.get("except"));
        if values.is_empty() {
            from_params("except[]")
        } else {
            values
        }
    };
    (only, except)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/course_copy — Canvas `copy_course_content`.
async fn copy_course_content(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !can_manage_content(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let source = payload
        .get("source_course")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| params.get("source_course").cloned())
        .unwrap_or_default();

    let (only, except) = copy_options(&payload, &params);
    if !only.is_empty() && !except.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "errors": "You can not use \"only\" and \"except\" options at the same time."
            })),
        ));
    }

    let Some(source_course_id) = find_source_course(&state.db, &source).await? else {
        return Err(ApiError::NotFound);
    };
    if !can_read_source_course(&state.db, &auth, source_course_id).await? {
        return Err(ApiError::Forbidden);
    }

    // Only the content types Canvas knows are accepted.
    for content_type in only.iter().chain(except.iter()) {
        if !COPY_CONTENT_TYPES.contains(&content_type.as_str()) {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "errors": format!("Invalid content type {content_type}")
                })),
            ));
        }
    }

    let mut settings = json!({
        "source_course_id": source_course_id,
        "import_immediately": true,
    });
    if !only.is_empty() {
        settings
            .as_object_mut()
            .expect("json object")
            .insert("only".to_string(), json!(only));
    }
    if !except.is_empty() {
        settings
            .as_object_mut()
            .expect("json object")
            .insert("except".to_string(), json!(except));
    }
    let settings = serde_json::to_string(&settings).unwrap_or_else(|_| "{}".to_string());

    let sql = "INSERT INTO content_migrations \
         (course_id, user_id, context_type, context_id, migration_type, workflow_state, \
          migration_settings, source_course_id) \
         VALUES ($1, $2, 'Course', $1, 'course_copy_importer', 'created', $3, $4) \
         RETURNING id, workflow_state, created_at";
    let row: CopyRow = sqlx::query_as(sql)
        .bind(course_id)
        .bind(auth.id)
        .bind(settings)
        .bind(source_course_id)
        .fetch_one(&state.db)
        .await?;

    Ok((StatusCode::OK, Json(row.to_json(course_id))))
}

/// GET /courses/{courseId}/course_copy/{id} — Canvas `copy_course_status`.
async fn copy_course_status(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !can_manage_content(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let sql = "SELECT id, workflow_state, created_at FROM content_migrations \
               WHERE id = $1 AND course_id = $2 AND context_type = 'Course' AND context_id = $2";
    let row: CopyRow = sqlx::query_as(sql)
        .bind(id)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(row.to_json(course_id)))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the content_imports router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/course_copy", post(copy_course_content))
        .route(
            "/courses/{courseId}/course_copy/{id}",
            get(copy_course_status),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(state: &str) -> CopyRow {
        CopyRow {
            id: 12,
            workflow_state: state.to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn the_legacy_api_remaps_the_workflow_state() {
        assert_eq!(copy("created").to_json(7)["workflow_state"], "created");
        assert_eq!(copy("failed").to_json(7)["workflow_state"], "failed");
        assert_eq!(copy("imported").to_json(7)["workflow_state"], "completed");
        assert_eq!(copy("exporting").to_json(7)["workflow_state"], "started");
    }

    #[test]
    fn the_status_url_points_at_the_status_route() {
        let value = copy("created").to_json(7);
        assert_eq!(value["status_url"], "/api/v1/courses/7/course_copy/12");
        assert_eq!(value["id"], 12);
        assert!(value.get("integration_id").is_some());
    }

    #[test]
    fn only_and_except_are_read_from_the_body_and_the_query() {
        let payload = json!({ "only": ["assignments", "files"] });
        let params = HashMap::new();
        let (only, except) = copy_options(&payload, &params);
        assert_eq!(only, vec!["assignments", "files"]);
        assert!(except.is_empty());

        let mut params = HashMap::new();
        params.insert("except[]".to_string(), "quizzes".to_string());
        let (only, except) = copy_options(&json!({}), &params);
        assert!(only.is_empty());
        assert_eq!(except, vec!["quizzes"]);
    }
}
