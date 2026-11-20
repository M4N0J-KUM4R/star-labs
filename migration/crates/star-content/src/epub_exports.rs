//! epub_exports — Canvas `epub_exports_controller` parity.
//!
//! Routes:
//! - GET  /epub_exports                    — courses with their latest export
//! - POST /courses/{courseId}/epub_exports  — start an ePub export
//! - GET  /courses/{courseId}/epub_exports/{id} — show one export
//!
//! Canvas behavior mirrored here (`lib/api/v1/epub_export.rb`):
//! - the body of a course is `{"id": ..., "name": ..., "epub_export": {...}}`;
//!   the export is only present when the course has one.
//! - the export itself carries the model attributes (`id`,
//!   `content_export_id`, `course_id`, `user_id`, `workflow_state`, `type`,
//!   `created_at`, `updated_at`) plus `progress_id` and `progress_url`, which
//!   point at the job progress of the export.
//! - `check_feature_enabled` answers 404 when the root account does not allow
//!   the `epub_export` feature.  The port has no account feature machinery
//!   wired for it, so the feature counts as allowed unless the root account
//!   explicitly disabled it.
//! - the web zip exports live in the same table with `type = 'WebZipExport'`
//!   (Canvas single table inheritance); see `web_zip_exports.rs`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Columns of an ePub export row.
const EPUB_COLUMNS: &str = "id, content_export_id, course_id, user_id, workflow_state, \
     type AS export_type, created_at, updated_at";

/// The id of the progress row that belongs to an export, if there is one.
const PROGRESS_ID: &str = "(SELECT p.id FROM progress p \
     WHERE p.context_type = 'EpubExport' AND p.context_id = e.id \
     ORDER BY p.id DESC LIMIT 1) AS progress_id";

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct EpubExportRow {
    id: i64,
    content_export_id: Option<i64>,
    course_id: i64,
    user_id: Option<i64>,
    workflow_state: String,
    export_type: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    progress_id: Option<i64>,
}

impl EpubExportRow {
    /// Canvas `epub_export_json`.
    fn to_json(&self) -> Value {
        fn ts(value: &DateTime<Utc>) -> String {
            value.to_rfc3339_opts(SecondsFormat::Secs, true)
        }

        json!({
            "id": self.id,
            "content_export_id": self.content_export_id,
            "course_id": self.course_id,
            "user_id": self.user_id,
            "workflow_state": self.workflow_state,
            "type": self.export_type,
            "created_at": ts(&self.created_at),
            "updated_at": ts(&self.updated_at),
            "progress_id": self.progress_id,
            "progress_url": self
                .progress_id
                .map(|id| format!("/api/v1/progress/{id}")),
        })
    }
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas `check_feature_enabled`: 404 when the root account explicitly
/// disabled the `epub_export` feature.
async fn feature_enabled(db: &PgPool) -> Result<bool, ApiError> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT state FROM feature_flags \
         WHERE context_type = 'Account' AND context_id = 1 AND feature = 'epub_export' \
         LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    Ok(match row {
        Some((state,)) => state == "enabled",
        None => true,
    })
}

/// Canvas `require_context`: 404 when the course does not exist.
async fn ensure_course(db: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `authorized_action(EpubExport.new(course:), @current_user, :create)`.
async fn can_manage_course(db: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
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

/// The course name, for the `course_epub_export_json` wrapper.
async fn course_name(db: &PgPool, course_id: i64) -> Result<String, ApiError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT name FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    row.map(|(name,)| name).ok_or(ApiError::NotFound)
}

/// Canvas `course_epub_export_json`: the course plus its latest export.
fn course_export_json(course_id: i64, name: &str, export: Option<&EpubExportRow>) -> Value {
    let mut value = json!({ "id": course_id, "name": name });
    if let Some(export) = export {
        if let Some(map) = value.as_object_mut() {
            map.insert("epub_export".to_string(), export.to_json());
        }
    }
    value
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /epub_exports — Canvas `index`.
///
/// The courses the caller is actively participating in, each with the
/// caller's own latest ePub export.
async fn list_epub_exports(
    State(state): State<AppState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !feature_enabled(&state.db).await? {
        return Err(ApiError::NotFound);
    }

    let page = params
        .get("page")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    let per_page = params
        .get("per_page")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(10)
        .clamp(1, 100);
    let offset = (page - 1) * per_page;

    // Canvas `CourseEpubExportsPresenter`: courses with a current or
    // concluded enrollment of the caller.
    let courses: Vec<(i64, String)> = sqlx::query_as(
        "SELECT c.id, c.name FROM courses c \
         JOIN enrollments e ON e.course_id = c.id \
         WHERE e.user_id = $1 AND e.workflow_state IN ('active', 'completed') \
         ORDER BY c.id LIMIT $2 OFFSET $3",
    )
    .bind(auth.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let mut list = Vec::with_capacity(courses.len());
    for (course_id, name) in courses {
        let export = latest_export(&state.db, course_id, auth.id).await?;
        list.push(course_export_json(course_id, &name, export.as_ref()));
    }

    Ok(Json(json!({ "courses": list })))
}

/// The caller's latest plain ePub export of a course.
async fn latest_export(
    db: &PgPool,
    course_id: i64,
    user_id: i64,
) -> Result<Option<EpubExportRow>, ApiError> {
    let sql = format!(
        "SELECT {EPUB_COLUMNS}, {PROGRESS_ID} FROM epub_exports e \
         WHERE e.course_id = $1 AND e.user_id = $2 AND e.type IS NULL \
         ORDER BY e.created_at DESC, e.id DESC LIMIT 1"
    );
    Ok(sqlx::query_as(&sql)
        .bind(course_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?)
}

/// POST /courses/{courseId}/epub_exports — Canvas `create`.
async fn create_epub_export(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if !feature_enabled(&state.db).await? {
        return Err(ApiError::NotFound);
    }
    ensure_course(&state.db, course_id).await?;
    if !can_manage_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let export_id: i64 = sqlx::query_scalar(
        "INSERT INTO epub_exports (course_id, user_id, workflow_state) \
         VALUES ($1, $2, 'created') RETURNING id",
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    // Canvas `export.initialize_job_progress`: the export is tracked by a
    // progress row the API links to.
    sqlx::query(
        "INSERT INTO progress (tag, workflow_state, completion, context_type, context_id, user_id) \
         VALUES ($1, 'running', 0, 'EpubExport', $2, $3)",
    )
    .bind(format!("epub_export_{}", export_id))
    .bind(export_id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    // The progress row is now part of the export.
    let row = find_export(&state.db, export_id).await?;
    let name = course_name(&state.db, course_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(course_export_json(course_id, &name, Some(&row))),
    ))
}

/// GET /courses/{courseId}/epub_exports/{id} — Canvas `show`.
async fn show_epub_export(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !feature_enabled(&state.db).await? {
        return Err(ApiError::NotFound);
    }
    ensure_course(&state.db, course_id).await?;

    // Canvas: `@course.epub_exports.where(id: params[:id]).first` — a missing
    // export fails the authorization check.
    let row = find_export(&state.db, id).await?;
    if row.course_id != course_id || !export_visible_to(&row, &auth) {
        return Err(ApiError::Forbidden);
    }

    let name = course_name(&state.db, course_id).await?;
    Ok(Json(course_export_json(course_id, &name, Some(&row))))
}

/// Load one export by id.
async fn find_export(db: &PgPool, id: i64) -> Result<EpubExportRow, ApiError> {
    let sql = format!("SELECT {EPUB_COLUMNS}, {PROGRESS_ID} FROM epub_exports e WHERE e.id = $1");
    sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Canvas `EpubExport.visible_to(user)`: only the owner sees an export.
fn export_visible_to(row: &EpubExportRow, auth: &AuthUser) -> bool {
    is_admin(auth) || row.user_id == Some(auth.id)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the epub_exports router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/epub_exports", get(list_epub_exports))
        .route("/courses/{courseId}/epub_exports", post(create_epub_export))
        .route(
            "/courses/{courseId}/epub_exports/{id}",
            get(show_epub_export),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn export() -> EpubExportRow {
        EpubExportRow {
            id: 3,
            content_export_id: Some(9),
            course_id: 7,
            user_id: Some(4),
            workflow_state: "created".to_string(),
            export_type: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            progress_id: Some(11),
        }
    }

    #[test]
    fn json_matches_canvas_epub_export_shape() {
        let value = export().to_json();
        for field in [
            "id",
            "content_export_id",
            "course_id",
            "user_id",
            "workflow_state",
            "type",
            "created_at",
            "updated_at",
            "progress_id",
            "progress_url",
        ] {
            assert!(value.get(field).is_some(), "missing field {field}");
        }
        assert_eq!(value["progress_url"], "/api/v1/progress/11");
    }

    #[test]
    fn a_course_without_an_export_has_no_export_key() {
        let value = course_export_json(7, "Course", None);
        assert!(value.get("epub_export").is_none(), "{value}");
        assert_eq!(value["id"], 7);
        assert_eq!(value["name"], "Course");
    }
}
