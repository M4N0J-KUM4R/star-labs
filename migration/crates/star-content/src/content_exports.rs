//! content_exports — Canvas `content_exports_api_controller` parity.
//!
//! Routes:
//! - GET  /courses/{courseId}/content_exports       — list (newest first)
//! - POST /courses/{courseId}/content_exports       — start an export
//! - GET  /courses/{courseId}/content_exports/{id}  — Canvas v1 nested show
//! - GET  /content_exports/{id}                     — flat show
//!
//! Canvas behavior mirrored here (`lib/api/v1/content_export.rb`):
//! - the export JSON is `id, user_id, created_at, workflow_state,
//!   export_type` plus `course_id` for course exports. We also expose
//!   `progress` and `updated_at` (brief parity fields).
//! - POST accepts `export_type` of `common_cartridge`, `qti`, `zip` or
//!   `quizzes2`; anything else is 400 `{"message": "invalid export_type"}`.
//! - `quizzes2` additionally requires a `quiz_id` that exists in the course.
//! - a newly created export is `workflow_state = 'created'`, `progress = 0`.
//!   No worker runs in this workspace, so the export stays in that state
//!   until a job runner (star-jobs) picks it up.
//! - visibility follows `ContentExport.admin/non_admin` scopes: teachers and
//!   admins see every course export, everyone else only their own
//!   `zip`/`user_data` exports.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Export types Canvas accepts for a course context.
const COURSE_EXPORT_TYPES: [&str; 4] = ["common_cartridge", "qti", "zip", "quizzes2"];

/// Export types that a non-admin (student) is allowed to download.
const NON_ADMIN_EXPORT_TYPES: [&str; 2] = ["zip", "user_data"];

/// Columns returned for a content export row (kept in one place so the
/// list/show/insert queries stay in sync).
const EXPORT_COLUMNS: &str =
    "id, course_id, export_type, workflow_state, progress, user_id, created_at, updated_at";

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct ContentExportRow {
    id: i64,
    course_id: Option<i64>,
    export_type: String,
    workflow_state: String,
    progress: i32,
    user_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl ContentExportRow {
    /// Canvas `content_export_json` shape (datetime as RFC3339 `...Z`).
    fn to_json(&self) -> Value {
        fn ts(value: &DateTime<Utc>) -> String {
            value.to_rfc3339_opts(SecondsFormat::Secs, true)
        }

        json!({
            "id": self.id,
            "course_id": self.course_id,
            "export_type": self.export_type,
            "workflow_state": self.workflow_state,
            "progress": self.progress,
            "user_id": self.user_id,
            "created_at": ts(&self.created_at),
            "updated_at": ts(&self.updated_at),
        })
    }

    /// `ContentExport.non_admin` scope: only own zip/user_data exports.
    fn visible_to(&self, auth: &AuthUser) -> bool {
        self.user_id == Some(auth.id) && NON_ADMIN_EXPORT_TYPES.contains(&self.export_type.as_str())
    }
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// 404 unless the course exists (Canvas `require_context`).
async fn ensure_course(db: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Course read access: any enrollment or global admin.
async fn can_read_course(db: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
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

/// Teachers/admins see all course exports (`ContentExport.admin` scope).
async fn manages_course(db: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
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

/// Canvas pagination params (page default 1, per_page default 10, max 100).
fn pagination(params: &HashMap<String, String>) -> (i64, i64) {
    let page = params
        .get("page")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    let per_page = params
        .get("per_page")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(10)
        .clamp(1, 100);
    (per_page, (page - 1) * per_page)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/content_exports — Canvas `index`: newest first.
async fn list_exports(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let (per_page, offset) = pagination(&params);
    let rows: Vec<ContentExportRow> = if manages_course(&state.db, &auth, course_id).await? {
        let sql = format!(
            "SELECT {EXPORT_COLUMNS} FROM content_exports \
             WHERE course_id = $1 AND workflow_state <> 'deleted' \
               AND export_type NOT IN ('course_copy', 'master_course_copy') \
             ORDER BY id DESC LIMIT $2 OFFSET $3"
        );
        sqlx::query_as(&sql)
            .bind(course_id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
    } else {
        // CourseExport.non_admin scope: own zip/user_data exports only.
        let sql = format!(
            "SELECT {EXPORT_COLUMNS} FROM content_exports \
             WHERE course_id = $1 AND workflow_state <> 'deleted' \
               AND export_type IN ('zip', 'user_data') AND user_id = $2 \
             ORDER BY id DESC LIMIT $3 OFFSET $4"
        );
        sqlx::query_as(&sql)
            .bind(course_id)
            .bind(auth.id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(&state.db)
            .await?
    };

    Ok(Json(Value::Array(
        rows.iter().map(ContentExportRow::to_json).collect(),
    )))
}

/// POST /courses/{courseId}/content_exports — Canvas `create`.
///
/// Returns 200 with the export (Canvas renders JSON, not 201) and 400 with
/// Canvas' `{"message": ...}` body on validation failures.
async fn create_export(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    // Canvas: authorized_action(@context, @current_user, :read) runs first.
    if !can_read_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let export_type = payload
        .get("export_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");

    if !COURSE_EXPORT_TYPES.contains(&export_type) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "invalid export_type" })),
        ));
    }

    // Canvas re-authorizes the export itself: students may only export zips.
    let manager = manages_course(&state.db, &auth, course_id).await?;
    if !manager && export_type != "zip" {
        return Err(ApiError::Forbidden);
    }

    if export_type == "quizzes2" {
        let quiz_id = payload.get("quiz_id").and_then(|value| match value {
            Value::Number(number) => number.as_i64(),
            Value::String(text) => text.trim().parse::<i64>().ok(),
            _ => None,
        });
        let Some(quiz_id) = quiz_id.filter(|id| *id > 0) else {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "quiz_id required and must be a valid ID" })),
            ));
        };
        let quiz: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM quizzes WHERE id = $1 AND course_id = $2")
                .bind(quiz_id)
                .bind(course_id)
                .fetch_optional(&state.db)
                .await?;
        if quiz.is_none() {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "Quiz could not be found" })),
            ));
        }
    }

    let sql = format!(
        "INSERT INTO content_exports (course_id, user_id, export_type, workflow_state, progress) \
         VALUES ($1, $2, $3, 'created', 0) RETURNING {EXPORT_COLUMNS}"
    );
    let row: ContentExportRow = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(auth.id)
        .bind(export_type)
        .fetch_one(&state.db)
        .await?;

    Ok((StatusCode::OK, Json(row.to_json())))
}

/// Load one export scoped to a course (404 when missing or soft-deleted).
async fn find_course_export(
    db: &PgPool,
    course_id: i64,
    id: i64,
) -> Result<ContentExportRow, ApiError> {
    let sql = format!(
        "SELECT {EXPORT_COLUMNS} FROM content_exports \
         WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'"
    );
    sqlx::query_as(&sql)
        .bind(id)
        .bind(course_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Read authorization for a single export: course read access plus the
/// Canvas visibility scope (manager, or own zip/user_data export).
async fn authorize_export(
    db: &PgPool,
    auth: &AuthUser,
    row: &ContentExportRow,
) -> Result<(), ApiError> {
    if let Some(course_id) = row.course_id {
        if !can_read_course(db, auth, course_id).await? {
            return Err(ApiError::Forbidden);
        }
        if manages_course(db, auth, course_id).await? {
            return Ok(());
        }
    } else if is_admin(auth) {
        return Ok(());
    }

    if row.visible_to(auth) {
        return Ok(());
    }
    Err(ApiError::Forbidden)
}

/// GET /courses/{courseId}/content_exports/{id} — Canvas v1 nested `show`.
async fn show_course_export(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    let row = find_course_export(&state.db, course_id, id).await?;
    authorize_export(&state.db, &auth, &row).await?;
    Ok(Json(row.to_json()))
}

/// GET /content_exports/{id} — flat show alias.
async fn show_export(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let sql = format!(
        "SELECT {EXPORT_COLUMNS} FROM content_exports \
         WHERE id = $1 AND workflow_state <> 'deleted'"
    );
    let row: ContentExportRow = sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    authorize_export(&state.db, &auth, &row).await?;
    Ok(Json(row.to_json()))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the content_exports router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/content_exports",
            get(list_exports).post(create_export),
        )
        .route(
            "/courses/{courseId}/content_exports/{id}",
            get(show_course_export),
        )
        .route("/content_exports/{id}", get(show_export))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn row(export_type: &str, user_id: i64) -> ContentExportRow {
        ContentExportRow {
            id: 1,
            course_id: Some(7),
            export_type: export_type.to_string(),
            workflow_state: "created".to_string(),
            progress: 0,
            user_id: Some(user_id),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn auth(id: i64, role: &str) -> AuthUser {
        AuthUser {
            id,
            role: role.to_string(),
        }
    }

    #[test]
    fn json_matches_canvas_content_export_shape() {
        let value = row("common_cartridge", 4).to_json();
        for field in [
            "id",
            "course_id",
            "export_type",
            "workflow_state",
            "progress",
            "user_id",
            "created_at",
            "updated_at",
        ] {
            assert!(value.get(field).is_some(), "missing field {field}");
        }
        assert_eq!(value["export_type"], "common_cartridge");
        assert_eq!(value["workflow_state"], "created");
        assert_eq!(value["progress"], 0);
        // Canvas renders datetimes as RFC3339 UTC with a trailing Z.
        assert!(value["created_at"]
            .as_str()
            .expect("created_at is a string")
            .ends_with('Z'));
    }

    #[test]
    fn non_admin_visibility_only_own_zip_or_user_data() {
        let student = auth(4, "student");
        assert!(row("zip", 4).visible_to(&student));
        assert!(row("user_data", 4).visible_to(&student));
        assert!(!row("common_cartridge", 4).visible_to(&student));
        assert!(!row("qti", 4).visible_to(&student));
        // Other users' exports are never visible.
        assert!(!row("zip", 5).visible_to(&student));
    }

    #[test]
    fn pagination_defaults_and_clamps() {
        let empty = HashMap::new();
        assert_eq!(pagination(&empty), (10, 0));

        let mut params = HashMap::new();
        params.insert("page".to_string(), "3".to_string());
        params.insert("per_page".to_string(), "5".to_string());
        assert_eq!(pagination(&params), (5, 10));

        params.insert("page".to_string(), "0".to_string());
        params.insert("per_page".to_string(), "500".to_string());
        assert_eq!(pagination(&params), (100, 0));
    }

    #[test]
    fn course_export_types_match_canvas_allow_list() {
        assert!(COURSE_EXPORT_TYPES.contains(&"common_cartridge"));
        assert!(COURSE_EXPORT_TYPES.contains(&"qti"));
        assert!(COURSE_EXPORT_TYPES.contains(&"zip"));
        assert!(COURSE_EXPORT_TYPES.contains(&"quizzes2"));
        assert!(!COURSE_EXPORT_TYPES.contains(&"course_copy"));
    }
}
