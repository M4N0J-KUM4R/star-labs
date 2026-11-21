//! web_zip_exports — Canvas `web_zip_exports_controller` parity.
//!
//! Routes:
//! - GET /courses/{courseId}/web_zip_exports      — list the web zip exports
//! - GET /courses/{courseId}/web_zip_exports/{id} — show one web zip export
//!
//! Canvas behavior mirrored here (`lib/api/v1/web_zip_export.rb`):
//! - the body carries the model attributes (`id`, `created_at`, `updated_at`,
//!   `user_id`, `course_id`, `content_export_id`, `workflow_state`, `type`)
//!   plus `progress_id` and `progress_url`, which point at the job progress of
//!   the export.
//! - Canvas stores these rows in the `epub_exports` table with
//!   `type = 'WebZipExport'` (single table inheritance), so this module reads
//!   the table that `epub_exports.rs` writes.
//! - the list only contains the exports of the caller
//!   (`EpubExport.visible_to(user)`), newest first.
//! - `check_feature_enabled` answers 404 when the course does not allow the
//!   offline web export.  The port has no account setting for it, so the
//!   download counts as allowed unless the root account explicitly disabled
//!   it.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Columns of a web zip export row.
const WEB_ZIP_COLUMNS: &str = "id, content_export_id, course_id, user_id, workflow_state, \
     type AS export_type, created_at, updated_at";

/// The id of the progress row that belongs to an export, if there is one.
const PROGRESS_ID: &str = "(SELECT p.id FROM progress p \
     WHERE p.context_type = 'EpubExport' AND p.context_id = e.id \
     ORDER BY p.id DESC LIMIT 1) AS progress_id";

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct WebZipExportRow {
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

impl WebZipExportRow {
    /// Canvas `web_zip_export_json`.
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
/// disabled the offline web export.
/// Canvas `WebZipExportHelper#course_allow_web_export_download?`: the account
/// of the course and the course itself have to enable the offline web
/// export.
async fn feature_enabled(db: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    let course: Option<(Option<i64>, Option<Value>)> =
        sqlx::query_as("SELECT account_id, settings FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    let Some((account_id, course_settings)) = course else {
        return Ok(false);
    };

    let account: Option<(Value,)> = sqlx::query_as("SELECT settings FROM accounts WHERE id = $1")
        .bind(account_id.unwrap_or(1))
        .fetch_optional(db)
        .await?;
    let account_settings = account.map(|(settings,)| settings).unwrap_or(Value::Null);

    let enabled = |settings: &Value| {
        settings
            .get("enable_offline_web_export")
            .and_then(|value| value.get("value"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };

    let course_settings = course_settings.unwrap_or(Value::Null);
    Ok(enabled(&account_settings) && enabled(&course_settings))
}

/// Canvas `require_context`: 404 when the course does not exist.
async fn ensure_course(db: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `allow_web_export_for_course_user?`: the caller has to be
/// enrolled in the course or a teacher of it.
async fn can_download(db: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1")
            .bind(auth.id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/web_zip_exports — Canvas `index`.
async fn list_web_zip_exports(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !feature_enabled(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !can_download(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
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

    let sql = format!(
        "SELECT {WEB_ZIP_COLUMNS}, {PROGRESS_ID} FROM epub_exports e \
         WHERE e.course_id = $1 AND e.type = 'WebZipExport' AND e.user_id = $2 \
         ORDER BY e.created_at DESC, e.id DESC LIMIT $3 OFFSET $4"
    );
    let rows: Vec<WebZipExportRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(auth.id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

    Ok((
        StatusCode::OK,
        Json(Value::Array(
            rows.iter().map(WebZipExportRow::to_json).collect(),
        )),
    ))
}

/// GET /courses/{courseId}/web_zip_exports/{id} — Canvas `show`.
async fn show_web_zip_export(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !feature_enabled(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }

    let sql = format!(
        "SELECT {WEB_ZIP_COLUMNS}, {PROGRESS_ID} FROM epub_exports e \
         WHERE e.id = $1 AND e.course_id = $2 AND e.type = 'WebZipExport'"
    );
    let row: WebZipExportRow = sqlx::query_as(&sql)
        .bind(id)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Canvas `EpubExport.visible_to(user)`.
    if !is_admin(&auth) && row.user_id != Some(auth.id) {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(row.to_json()))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the web_zip_exports router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/web_zip_exports",
            get(list_web_zip_exports),
        )
        .route(
            "/courses/{courseId}/web_zip_exports/{id}",
            get(show_web_zip_export),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn export() -> WebZipExportRow {
        WebZipExportRow {
            id: 5,
            content_export_id: Some(8),
            course_id: 7,
            user_id: Some(4),
            workflow_state: "generated".to_string(),
            export_type: Some("WebZipExport".to_string()),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            progress_id: Some(12),
        }
    }

    #[test]
    fn json_matches_canvas_web_zip_export_shape() {
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
        assert_eq!(value["type"], "WebZipExport");
        assert_eq!(value["progress_url"], "/api/v1/progress/12");
    }
}
