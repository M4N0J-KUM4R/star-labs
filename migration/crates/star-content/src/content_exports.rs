//! content_exports — Canvas `content_exports_api_controller` parity.
//!
//! Routes:
//! - GET  /courses/{courseId}/content_exports            — list (newest first)
//! - POST /courses/{courseId}/content_exports            — start an export
//! - GET  /courses/{courseId}/content_exports/{id}       — Canvas v1 nested show
//! - GET  /content_exports/{id}                          — flat show
//! - GET  /groups/{groupId}/content_exports              — list (group context)
//! - POST /groups/{groupId}/content_exports              — start a group export
//! - GET  /groups/{groupId}/content_exports/{id}         — group show
//! - GET  /users/{userId}/content_exports                — list (user context)
//! - POST /users/{userId}/content_exports                — start a user export
//! - GET  /users/{userId}/content_exports/{id}           — user show
//! - PUT  /courses/{courseId}/content_exports/{id}/fail  — mark an export failed
//! - PUT  /groups/{groupId}/content_exports/{id}/fail    — mark an export failed
//! - PUT  /users/{userId}/content_exports/{id}/fail      — mark an export failed
//! - GET  /courses/{courseId}/content_list               — selectable content
//! - PUT  /courses/{courseId}/content_exports/{id}       — update (site admin)
//!
//! Canvas behavior mirrored here (`lib/api/v1/content_export.rb`):
//! - the export JSON is `id, user_id, created_at, workflow_state,
//!   export_type` plus `course_id` for course exports. We also expose
//!   `progress` and `updated_at` (brief parity fields).
//! - POST accepts `export_type` of `common_cartridge`, `qti`, `zip` or
//!   `quizzes2` for a course; a group or a user only accepts `zip`. Anything
//!   else is 400 `{"message": "invalid export_type"}`.
//! - `quizzes2` additionally requires a `quiz_id` that exists in the course.
//! - a newly created export is `workflow_state = 'created'`, `progress = 0`.
//!   No worker runs in this workspace, so the export stays in that state
//!   until a job runner (star-jobs) picks it up.
//! - visibility follows `ContentExport.admin/non_admin` scopes: teachers and
//!   admins see every course export, everyone else only their own
//!   `zip`/`user_data` exports. A group or a user only ever sees its own
//!   exports (`content_exports_visible_to`).
//! - `PUT .../fail` is site-admin only; `PUT /courses/{id}/content_exports/{id}`
//!   is site-admin only as well (Canvas `require_site_admin`).

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Export types Canvas accepts for a course context.
const COURSE_EXPORT_TYPES: [&str; 4] = ["common_cartridge", "qti", "zip", "quizzes2"];

/// Export types Canvas accepts for a group or a user context
/// (`valid_types = %w[zip]` plus the course-only types).
const CONTEXT_EXPORT_TYPES: [&str; 1] = ["zip"];

/// Export types that a non-admin (student) is allowed to download.
const NON_ADMIN_EXPORT_TYPES: [&str; 2] = ["zip", "user_data"];

/// Columns returned for a content export row (kept in one place so the
/// list/show/insert queries stay in sync).
pub(crate) const EXPORT_COLUMNS: &str = "id, course_id, user_id, context_type, \
     export_type, workflow_state, progress, new_quizzes_export_state, \
     new_quizzes_export_url, created_at, updated_at";

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct ContentExportRow {
    id: i64,
    course_id: Option<i64>,
    user_id: Option<i64>,
    context_type: Option<String>,
    export_type: String,
    workflow_state: String,
    progress: i32,
    new_quizzes_export_state: Option<String>,
    new_quizzes_export_url: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl ContentExportRow {
    /// Canvas `content_export_json` shape (datetime as RFC3339 `...Z`).
    ///
    /// `includes` carries the request's `include[]` values; Canvas only adds
    /// the new-quizzes export settings when they are asked for.
    pub(crate) fn to_json(&self, includes: &[String]) -> Value {
        fn ts(value: &DateTime<Utc>) -> String {
            value.to_rfc3339_opts(SecondsFormat::Secs, true)
        }

        let mut value = json!({
            "id": self.id,
            "user_id": self.user_id,
            "created_at": ts(&self.created_at),
            "workflow_state": self.workflow_state,
            "export_type": self.export_type,
            "progress": self.progress,
            "updated_at": ts(&self.updated_at),
        });
        let map = value.as_object_mut().expect("json object");
        // Canvas renders `course_id` only when the context is a course.
        if self.context_type.as_deref() == Some("Course") {
            map.insert("course_id".to_string(), json!(self.course_id));
        }
        if includes
            .iter()
            .any(|item| item == "new_quizzes_export_settings")
        {
            map.insert(
                "new_quizzes_export_state".to_string(),
                json!(self.new_quizzes_export_state),
            );
            map.insert(
                "new_quizzes_export_url".to_string(),
                json!(self.new_quizzes_export_url),
            );
        }
        value
    }

    /// `ContentExport.non_admin` scope: only own zip/user_data exports.
    fn visible_to(&self, auth: &AuthUser) -> bool {
        self.user_id == Some(auth.id) && NON_ADMIN_EXPORT_TYPES.contains(&self.export_type.as_str())
    }
}

/// The Canvas context a content export belongs to.
///
/// Canvas stores the context polymorphically (`belongs_to :context,
/// polymorphic: [:course, :group, :user]`), so the same actions are served
/// for each of them.
#[derive(Debug, Clone, Copy)]
enum ExportContext {
    Course(i64),
    Group(i64),
    User(i64),
}

impl ExportContext {
    /// The `context_type` Canvas stores.
    fn type_name(self) -> &'static str {
        match self {
            ExportContext::Course(_) => "Course",
            ExportContext::Group(_) => "Group",
            ExportContext::User(_) => "User",
        }
    }

    fn id(self) -> i64 {
        match self {
            ExportContext::Course(id) | ExportContext::Group(id) | ExportContext::User(id) => id,
        }
    }

    /// The course the export belongs to, when there is one.
    fn course_id(self) -> Option<i64> {
        match self {
            ExportContext::Course(course_id) => Some(course_id),
            _ => None,
        }
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

/// Canvas `require_context`: 404 when the context does not exist.
async fn ensure_context(db: &PgPool, context: ExportContext) -> Result<(), ApiError> {
    let row: Option<(i32,)> = match context {
        ExportContext::Course(course_id) => {
            sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
                .bind(course_id)
                .fetch_optional(db)
                .await?
        }
        ExportContext::Group(group_id) => {
            sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
                .bind(group_id)
                .fetch_optional(db)
                .await?
        }
        ExportContext::User(user_id) => {
            sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(db)
                .await?
        }
    };
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

/// Canvas `authorized_action(@context, @current_user, :read)`.
async fn can_read_context(
    db: &PgPool,
    auth: &AuthUser,
    context: ExportContext,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    match context {
        ExportContext::Course(course_id) => can_read_course(db, auth, course_id).await,
        ExportContext::Group(group_id) => {
            // A group member, or a teacher of the parent course.
            let member: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM group_memberships \
                 WHERE group_id = $1 AND user_id = $2 AND workflow_state = 'accepted' LIMIT 1",
            )
            .bind(group_id)
            .bind(auth.id)
            .fetch_optional(db)
            .await?;
            if member.is_some() {
                return Ok(true);
            }
            let group: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT course_id FROM groups WHERE id = $1")
                    .bind(group_id)
                    .fetch_optional(db)
                    .await?;
            match group {
                Some((Some(course_id),)) => manages_course(db, auth, course_id).await,
                _ => Ok(false),
            }
        }
        ExportContext::User(user_id) => Ok(auth.id == user_id),
    }
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

/// The `include[]` values of a request body.
fn body_includes(payload: &Value) -> Vec<String> {
    match payload.get("include") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect(),
        Some(Value::String(value)) => value
            .split(',')
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Canvas `content_exports_visible_to(user).active.not_for_copy`.
async fn visible_exports(
    db: &PgPool,
    auth: &AuthUser,
    context: ExportContext,
    per_page: i64,
    offset: i64,
) -> Result<Vec<ContentExportRow>, ApiError> {
    const ACTIVE: &str = "workflow_state <> 'deleted' \
         AND export_type NOT IN ('course_copy', 'master_course_copy')";
    match context {
        ExportContext::Course(course_id) => {
            if manages_course(db, auth, course_id).await? {
                // ContentExport.admin scope: every export of the course.
                let sql = format!(
                    "SELECT {EXPORT_COLUMNS} FROM content_exports \
                     WHERE course_id = $1 AND {ACTIVE} ORDER BY id DESC LIMIT $2 OFFSET $3"
                );
                Ok(sqlx::query_as(&sql)
                    .bind(course_id)
                    .bind(per_page)
                    .bind(offset)
                    .fetch_all(db)
                    .await?)
            } else {
                // ContentExport.non_admin scope: own zip/user_data exports only.
                let sql = format!(
                    "SELECT {EXPORT_COLUMNS} FROM content_exports \
                     WHERE course_id = $1 AND {ACTIVE} \
                       AND export_type IN ('zip', 'user_data') AND user_id = $2 \
                     ORDER BY id DESC LIMIT $3 OFFSET $4"
                );
                Ok(sqlx::query_as(&sql)
                    .bind(course_id)
                    .bind(auth.id)
                    .bind(per_page)
                    .bind(offset)
                    .fetch_all(db)
                    .await?)
            }
        }
        ExportContext::Group(_) | ExportContext::User(_) => {
            // Group and user contexts only ever see their own exports.
            let sql = format!(
                "SELECT {EXPORT_COLUMNS} FROM content_exports \
                 WHERE context_type = $1 AND context_id = $2 AND {ACTIVE} AND user_id = $3 \
                 ORDER BY id DESC LIMIT $4 OFFSET $5"
            );
            Ok(sqlx::query_as(&sql)
                .bind(context.type_name())
                .bind(context.id())
                .bind(auth.id)
                .bind(per_page)
                .bind(offset)
                .fetch_all(db)
                .await?)
        }
    }
}

/// Canvas `@context.content_exports.find(id)`.
async fn find_export(
    db: &PgPool,
    context: ExportContext,
    id: i64,
) -> Result<ContentExportRow, ApiError> {
    let row = match context {
        ExportContext::Course(course_id) => {
            let sql = format!(
                "SELECT {EXPORT_COLUMNS} FROM content_exports WHERE id = $1 AND course_id = $2"
            );
            sqlx::query_as(&sql)
                .bind(id)
                .bind(course_id)
                .fetch_optional(db)
                .await?
        }
        _ => {
            let sql = format!(
                "SELECT {EXPORT_COLUMNS} FROM content_exports \
                 WHERE id = $1 AND context_type = $2 AND context_id = $3"
            );
            sqlx::query_as(&sql)
                .bind(id)
                .bind(context.type_name())
                .bind(context.id())
                .fetch_optional(db)
                .await?
        }
    };
    row.ok_or(ApiError::NotFound)
}

/// Canvas `@context.content_exports.not_for_copy.find(id)`.
///
/// The port also keeps the `active` scope of the export list, so a deleted
/// export is not reachable.
async fn find_not_for_copy_export(
    db: &PgPool,
    context: ExportContext,
    id: i64,
) -> Result<ContentExportRow, ApiError> {
    let row = match context {
        ExportContext::Course(course_id) => {
            let sql = format!(
                "SELECT {EXPORT_COLUMNS} FROM content_exports \
                 WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted' \
                   AND export_type NOT IN ('course_copy', 'master_course_copy')"
            );
            sqlx::query_as(&sql)
                .bind(id)
                .bind(course_id)
                .fetch_optional(db)
                .await?
        }
        _ => {
            let sql = format!(
                "SELECT {EXPORT_COLUMNS} FROM content_exports \
                 WHERE id = $1 AND context_type = $2 AND context_id = $3 \
                   AND workflow_state <> 'deleted' \
                   AND export_type NOT IN ('course_copy', 'master_course_copy')"
            );
            sqlx::query_as(&sql)
                .bind(id)
                .bind(context.type_name())
                .bind(context.id())
                .fetch_optional(db)
                .await?
        }
    };
    row.ok_or(ApiError::NotFound)
}

/// Canvas `authorized_action(export, @current_user, :read)`.
async fn export_visible_to(
    db: &PgPool,
    auth: &AuthUser,
    context: ExportContext,
    row: &ContentExportRow,
) -> Result<bool, ApiError> {
    match context {
        ExportContext::Course(course_id) => {
            if manages_course(db, auth, course_id).await? {
                return Ok(true);
            }
            Ok(row.visible_to(auth))
        }
        ExportContext::Group(_) | ExportContext::User(_) => Ok(row.user_id == Some(auth.id)),
    }
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
    let rows = visible_exports(
        &state.db,
        &auth,
        ExportContext::Course(course_id),
        per_page,
        offset,
    )
    .await?;

    Ok(Json(Value::Array(
        rows.iter().map(|row| row.to_json(&[])).collect(),
    )))
}

/// GET /{groups,users}/{id}/content_exports — Canvas `index` for a group or
/// a user context.
async fn list_context_exports(
    state: &AppState,
    auth: &AuthUser,
    context: ExportContext,
    params: &HashMap<String, String>,
) -> Result<Json<Value>, ApiError> {
    ensure_context(&state.db, context).await?;
    if !can_read_context(&state.db, auth, context).await? {
        return Err(ApiError::Forbidden);
    }

    let (per_page, offset) = pagination(params);
    let rows = visible_exports(&state.db, auth, context, per_page, offset).await?;

    Ok(Json(Value::Array(
        rows.iter().map(|row| row.to_json(&[])).collect(),
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
    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    create_export_for_context(&state, &auth, ExportContext::Course(course_id), &payload).await
}

/// POST /{groups,users}/{id}/content_exports — Canvas `create` for a group or
/// a user context.
async fn create_context_export(
    state: &AppState,
    auth: &AuthUser,
    context: ExportContext,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    create_export_for_context(state, auth, context, &payload).await
}

/// The shared body of the create route.
async fn create_export_for_context(
    state: &AppState,
    auth: &AuthUser,
    context: ExportContext,
    payload: &Value,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_context(&state.db, context).await?;
    // Canvas: authorized_action(@context, @current_user, :read) runs first.
    if !can_read_context(&state.db, auth, context).await? {
        return Err(ApiError::Forbidden);
    }

    let export_type = payload
        .get("export_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");

    let valid: &[&str] = match context {
        ExportContext::Course(_) => &COURSE_EXPORT_TYPES,
        _ => &CONTEXT_EXPORT_TYPES,
    };
    if !valid.contains(&export_type) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "invalid export_type" })),
        ));
    }

    // Canvas re-authorizes the export itself: students may only export zips.
    let manager = match context {
        ExportContext::Course(course_id) => manages_course(&state.db, auth, course_id).await?,
        _ => true,
    };
    if !manager && export_type != "zip" {
        return Err(ApiError::Forbidden);
    }

    if export_type == "quizzes2" {
        let Some(course_id) = context.course_id() else {
            return Ok((
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "invalid export_type" })),
            ));
        };
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
        "INSERT INTO content_exports \
         (course_id, user_id, context_type, context_id, export_type, workflow_state, progress) \
         VALUES ($1, $2, $3, $4, $5, 'created', 0) RETURNING {EXPORT_COLUMNS}"
    );
    let row: ContentExportRow = sqlx::query_as(&sql)
        .bind(context.course_id())
        .bind(auth.id)
        .bind(context.type_name())
        .bind(context.id())
        .bind(export_type)
        .fetch_one(&state.db)
        .await?;

    Ok((StatusCode::OK, Json(row.to_json(&body_includes(payload)))))
}

/// GET /courses/{courseId}/content_exports/{id} — Canvas v1 nested `show`.
async fn show_course_export(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_course(&state.db, course_id).await?;
    let row = find_not_for_copy_export(&state.db, ExportContext::Course(course_id), id).await?;
    authorize_export(&state.db, &auth, &row).await?;
    Ok(Json(row.to_json(&[])))
}

/// GET /{groups,users}/{id}/content_exports/{id} — Canvas `show` for a group
/// or a user context.
async fn show_context_export(
    state: &AppState,
    auth: &AuthUser,
    context: ExportContext,
    id: i64,
) -> Result<Json<Value>, ApiError> {
    ensure_context(&state.db, context).await?;
    let row = find_not_for_copy_export(&state.db, context, id).await?;
    if !can_read_context(&state.db, auth, context).await? {
        return Err(ApiError::Forbidden);
    }
    if !export_visible_to(&state.db, auth, context, &row).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(row.to_json(&[])))
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
    Ok(Json(row.to_json(&[])))
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

/// PUT /{courses,groups,users}/{id}/content_exports/{id}/fail — Canvas `fail`.
///
/// Site-admin only: Canvas authorizes `Account.site_admin` before the export
/// is looked up.
async fn fail_export(
    state: &AppState,
    auth: &AuthUser,
    context: ExportContext,
    id: i64,
) -> Result<Json<Value>, ApiError> {
    ensure_context(&state.db, context).await?;
    if !is_admin(auth) {
        return Err(ApiError::Forbidden);
    }
    let row = find_export(&state.db, context, id).await?;
    sqlx::query(
        "UPDATE content_exports SET workflow_state = 'failed', updated_at = now() WHERE id = $1",
    )
    .bind(row.id)
    .execute(&state.db)
    .await?;
    let row = find_export(&state.db, context, row.id).await?;
    Ok(Json(row.to_json(&[])))
}

/// PUT /courses/{courseId}/content_exports/{id} — Canvas `update`.
///
/// Site-admin only (`require_site_admin` runs before the action). The export
/// has to be a common-cartridge export of the course; the body carries the
/// new-quizzes export settings.
async fn update_course_export(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Canvas: @context.content_exports.common_cartridge.find(params[:id])
    let row = find_export(&state.db, ExportContext::Course(course_id), id).await?;
    if row.export_type != "common_cartridge" {
        return Err(ApiError::NotFound);
    }

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let Some(attributes) = payload.get("content_export") else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "param is missing or the value is empty: content_export"
            })),
        ));
    };
    let new_state = attributes
        .get("new_quizzes_export_state")
        .and_then(Value::as_str);
    let new_url = attributes
        .get("new_quizzes_export_url")
        .and_then(Value::as_str);

    sqlx::query(
        "UPDATE content_exports \
         SET new_quizzes_export_state = $1, new_quizzes_export_url = $2, updated_at = now() \
         WHERE id = $3",
    )
    .bind(new_state)
    .bind(new_url)
    .bind(row.id)
    .execute(&state.db)
    .await?;

    let row = find_export(&state.db, ExportContext::Course(course_id), row.id).await?;
    Ok((
        StatusCode::OK,
        Json(row.to_json(&["new_quizzes_export_settings".to_string()])),
    ))
}

// ---------------------------------------------------------------------------
// /content_list — Canvas `ContentExportsApiController#content_list`
// ---------------------------------------------------------------------------

/// One entry of Canvas `SelectiveContentFormatter::SELECTIVE_CONTENT_TYPES`.
struct ContentType {
    /// The `type` value of the request.
    kind: &'static str,
    /// The human title Canvas renders.
    title: &'static str,
    /// SQL returning `(id, title)` for the type in a course.
    sql: &'static str,
    /// The Canvas asset-string prefix (`assignment_42`).
    asset: &'static str,
}

/// The content types `/content_list` can enumerate.
const CONTENT_TYPES: [ContentType; 14] = [
    ContentType {
        kind: "context_modules",
        title: "Modules",
        sql: "SELECT id, name FROM modules WHERE course_id = $1 ORDER BY position, id",
        asset: "context_module",
    },
    ContentType {
        kind: "assignments",
        title: "Assignments",
        sql: "SELECT id, title FROM assignments WHERE course_id = $1 ORDER BY id",
        asset: "assignment",
    },
    ContentType {
        kind: "quizzes",
        title: "Quizzes",
        sql: "SELECT id, title FROM quizzes WHERE course_id = $1 ORDER BY id",
        asset: "quiz",
    },
    ContentType {
        kind: "assessment_question_banks",
        title: "Question Banks",
        sql: "SELECT id, title FROM question_banks \
              WHERE context_type = 'Course' AND context_id = $1 ORDER BY id",
        asset: "assessment_question_bank",
    },
    ContentType {
        kind: "discussion_topics",
        title: "Discussion Topics",
        sql: "SELECT id, title FROM discussion_topics \
              WHERE course_id = $1 AND workflow_state = 'active' ORDER BY id",
        asset: "discussion_topic",
    },
    ContentType {
        kind: "wiki_pages",
        title: "Pages",
        sql: "SELECT id, title FROM pages WHERE course_id = $1 ORDER BY id",
        asset: "wiki_page",
    },
    ContentType {
        kind: "context_external_tools",
        title: "External Tools",
        sql: "SELECT id, name FROM external_tools \
              WHERE course_id = $1 AND workflow_state <> 'deleted' ORDER BY id",
        asset: "context_external_tool",
    },
    ContentType {
        kind: "tool_profiles",
        title: "Tool Profiles",
        sql: "SELECT id, COALESCE(raw_data->>'name', guid) FROM lti_tool_proxies \
              WHERE context_type = 'Course' AND context_id = $1 AND workflow_state = 'active' \
              ORDER BY id",
        asset: "tool_profile",
    },
    ContentType {
        kind: "announcements",
        title: "Announcements",
        sql: "SELECT id, title FROM announcements WHERE course_id = $1 ORDER BY id",
        asset: "announcement",
    },
    ContentType {
        kind: "calendar_events",
        title: "Calendar Events",
        sql: "SELECT id, title FROM calendar_events \
              WHERE course_id = $1 AND workflow_state = 'active' ORDER BY id",
        asset: "calendar_event",
    },
    ContentType {
        kind: "rubrics",
        title: "Rubrics",
        sql: "SELECT id, title FROM rubrics WHERE course_id = $1 AND workflow_state <> 'deleted' \
              ORDER BY id",
        asset: "rubric",
    },
    ContentType {
        kind: "groups",
        title: "Student Groups",
        sql: "SELECT id, name FROM groups WHERE course_id = $1 AND workflow_state <> 'deleted' \
              ORDER BY id",
        asset: "group",
    },
    ContentType {
        kind: "learning_outcomes",
        title: "Learning Outcomes",
        sql: "SELECT lo.id, lo.title FROM learning_outcomes lo \
              JOIN learning_outcome_links lol ON lol.outcome_id = lo.id \
              JOIN learning_outcome_groups g ON g.id = lol.outcome_group_id \
              WHERE g.course_id = $1 ORDER BY lo.id",
        asset: "learning_outcome",
    },
    ContentType {
        kind: "attachments",
        title: "Files",
        sql: "SELECT id, filename FROM files WHERE course_id = $1 ORDER BY id",
        asset: "attachment",
    },
];

/// Canvas `SelectiveContentFormatter#valid_type?`.
fn valid_content_type(kind: &str) -> bool {
    CONTENT_TYPES.iter().any(|item| item.kind == kind)
        || kind.starts_with("submodules_")
        || kind.starts_with("learning_outcome_groups_")
}

/// The count query of a content type (`SELECT count(*) FROM ...`).
fn count_query(items_sql: &str) -> String {
    let sql = match items_sql.find(" FROM ") {
        Some(index) => format!("SELECT count(*){}", &items_sql[index..]),
        None => items_sql.to_string(),
    };
    // `ORDER BY` is not allowed in an aggregate query.
    match sql.find(" ORDER BY ") {
        Some(index) => sql[..index].to_string(),
        None => sql,
    }
}

/// GET /courses/{courseId}/content_list — Canvas `content_list`.
///
/// Without a `type` the response is the list of content buckets; with a known
/// `type` it is that type's items.
async fn content_list(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_course(&state.db, course_id).await?;
    // Canvas: authorized_action(@context, @current_user, :read_as_admin)
    if !manages_course(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    let base_url = format!("/api/v1/courses/{course_id}/content_list");
    let requested = params
        .get("type")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());

    let Some(requested) = requested else {
        let mut list = vec![
            json!({
                "type": "course_settings",
                "property": "select[all_course_settings]",
                "title": "Course Settings",
            }),
            json!({
                "type": "syllabus_body",
                "property": "select[all_syllabus_body]",
                "title": "Syllabus Body",
            }),
        ];
        for content_type in CONTENT_TYPES.iter() {
            // Canvas never lists the groups bucket.
            if content_type.kind == "groups" {
                continue;
            }
            let count: i64 = sqlx::query_scalar(&count_query(content_type.sql))
                .bind(course_id)
                .fetch_one(&state.db)
                .await?;
            if count == 0 {
                continue;
            }
            list.push(json!({
                "type": content_type.kind,
                "property": format!("select[all_{}]", content_type.kind),
                "title": content_type.title,
                "count": count,
                "sub_items_url": format!("{base_url}?type={}", content_type.kind),
            }));
        }
        return Ok((StatusCode::OK, Json(Value::Array(list))));
    };

    if !valid_content_type(requested) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "unsupported migration type" })),
        ));
    }

    let Some(content_type) = CONTENT_TYPES.iter().find(|item| item.kind == requested) else {
        // `submodules_*` and `learning_outcome_groups_*` are accepted but the
        // port stores no such hierarchy, so the list is empty.
        return Ok((StatusCode::OK, Json(Value::Array(Vec::new()))));
    };

    let rows: Vec<(i64, String)> = sqlx::query_as(content_type.sql)
        .bind(course_id)
        .fetch_all(&state.db)
        .await?;
    let list: Vec<Value> = rows
        .iter()
        .map(|(id, title)| {
            json!({
                "type": content_type.kind,
                "title": title,
                "id": format!("{}_{}", content_type.asset, id),
            })
        })
        .collect();
    Ok((StatusCode::OK, Json(Value::Array(list))))
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
            get(show_course_export).put(update_course_export),
        )
        .route(
            "/courses/{courseId}/content_exports/{id}/fail",
            put(fail_course_export),
        )
        .route("/courses/{courseId}/content_list", get(content_list))
        .route(
            "/groups/{groupId}/content_exports",
            get(list_group_exports).post(create_group_export),
        )
        .route(
            "/groups/{groupId}/content_exports/{id}",
            get(show_group_export),
        )
        .route(
            "/groups/{groupId}/content_exports/{id}/fail",
            put(fail_group_export),
        )
        .route(
            "/users/{userId}/content_exports",
            get(list_user_exports).post(create_user_export),
        )
        .route(
            "/users/{userId}/content_exports/{id}",
            get(show_user_export),
        )
        .route(
            "/users/{userId}/content_exports/{id}/fail",
            put(fail_user_export),
        )
        .route("/content_exports/{id}", get(show_export))
}

/// GET /groups/{groupId}/content_exports — Canvas `index` (group context).
async fn list_group_exports(
    State(state): State<AppState>,
    Path(context_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    list_context_exports(&state, &auth, ExportContext::Group(context_id), &params).await
}

/// POST /groups/{groupId}/content_exports — Canvas `create` (group context).
async fn create_group_export(
    State(state): State<AppState>,
    Path(context_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    create_context_export(&state, &auth, ExportContext::Group(context_id), body).await
}

/// GET /groups/{groupId}/content_exports/{id} — Canvas `show` (group context).
async fn show_group_export(
    State(state): State<AppState>,
    Path((context_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    show_context_export(&state, &auth, ExportContext::Group(context_id), id).await
}

/// PUT /groups/{groupId}/content_exports/{id}/fail — Canvas `fail`.
async fn fail_group_export(
    State(state): State<AppState>,
    Path((context_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    fail_export(&state, &auth, ExportContext::Group(context_id), id).await
}

/// GET /users/{userId}/content_exports — Canvas `index` (user context).
async fn list_user_exports(
    State(state): State<AppState>,
    Path(context_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    list_context_exports(&state, &auth, ExportContext::User(context_id), &params).await
}

/// POST /users/{userId}/content_exports — Canvas `create` (user context).
async fn create_user_export(
    State(state): State<AppState>,
    Path(context_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    create_context_export(&state, &auth, ExportContext::User(context_id), body).await
}

/// GET /users/{userId}/content_exports/{id} — Canvas `show` (user context).
async fn show_user_export(
    State(state): State<AppState>,
    Path((context_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    show_context_export(&state, &auth, ExportContext::User(context_id), id).await
}

/// PUT /users/{userId}/content_exports/{id}/fail — Canvas `fail`.
async fn fail_user_export(
    State(state): State<AppState>,
    Path((context_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    fail_export(&state, &auth, ExportContext::User(context_id), id).await
}

/// PUT /courses/{courseId}/content_exports/{id}/fail — Canvas `fail`.
async fn fail_course_export(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    fail_export(&state, &auth, ExportContext::Course(course_id), id).await
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
            user_id: Some(user_id),
            context_type: Some("Course".to_string()),
            export_type: export_type.to_string(),
            workflow_state: "created".to_string(),
            progress: 0,
            new_quizzes_export_state: None,
            new_quizzes_export_url: None,
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
        let value = row("common_cartridge", 4).to_json(&[]);
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
    fn course_id_is_only_rendered_for_a_course_context() {
        let mut group = row("zip", 4);
        group.context_type = Some("Group".to_string());
        let value = group.to_json(&[]);
        assert!(value.get("course_id").is_none(), "{value}");
    }

    #[test]
    fn new_quizzes_settings_need_the_include() {
        let mut export = row("common_cartridge", 4);
        export.new_quizzes_export_state = Some("completed".to_string());
        export.new_quizzes_export_url = Some("https://example.test/q".to_string());
        assert!(export
            .to_json(&[])
            .get("new_quizzes_export_state")
            .is_none());
        let value = export.to_json(&["new_quizzes_export_settings".to_string()]);
        assert_eq!(value["new_quizzes_export_state"], "completed");
        assert_eq!(value["new_quizzes_export_url"], "https://example.test/q");
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
        // A group or a user context only accepts a zip export.
        assert_eq!(CONTEXT_EXPORT_TYPES, ["zip"]);
    }

    #[test]
    fn content_list_accepts_the_canvas_types() {
        assert!(valid_content_type("assignments"));
        assert!(valid_content_type("context_modules"));
        assert!(valid_content_type("submodules_3"));
        assert!(valid_content_type("learning_outcome_groups_4"));
        assert!(!valid_content_type("nonsense"));
    }

    #[test]
    fn count_query_replaces_the_select_list() {
        let sql = "SELECT id, title FROM assignments WHERE course_id = $1 ORDER BY id";
        assert_eq!(
            count_query(sql),
            "SELECT count(*) FROM assignments WHERE course_id = $1"
        );
    }
}
