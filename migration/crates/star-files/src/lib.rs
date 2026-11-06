//! star-files domain crate — file metadata tracking (no S3/upload).
//!
//! Canvas parity (app/models/attachment.rb, lib/api/v1/attachment.rb):
//! - filename: stored name on disk (unencoded)
//! - display_name: user-facing name; defaults to filename if not provided
//!   (Canvas `infer_display_name`: `self.display_name ||= unencoded_filename`)
//! - content_type: MIME type
//! - size: file size in bytes (Canvas column name; ours was `size_bytes`,
//!   renamed by migration 0016)
//! - folder: full folder path, e.g. 'course files/subfolder'
//!   (Canvas course root folder is named 'course files')
//!
//! Responses lead with display_name (Canvas behavior). Actual upload/S3
//! storage is explicitly out of scope — POST records metadata only.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use star_common::{ApiError, AppState};

use star_auth::AuthUser;

// ─── auth helpers ───────────────────────────────────────────────────────────

/// Check if user is enrolled in the course or is admin.
async fn user_enrolled_or_admin(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    role: &str,
) -> Result<bool, ApiError> {
    if role == "admin" {
        return Ok(true);
    }
    let row: (bool,) = sqlx::query_as(
        r#"SELECT EXISTS(
            SELECT 1 FROM enrollments
            WHERE user_id = $1 AND course_id = $2
        )"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

/// Check if user is the teacher of the course or is admin.
async fn user_teacher_of_course(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    role: &str,
) -> Result<bool, ApiError> {
    if role == "admin" {
        return Ok(true);
    }
    let row: (bool,) = sqlx::query_as(
        r#"SELECT EXISTS(
            SELECT 1 FROM enrollments
            WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
        )"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

// ─── DTOs ───────────────────────────────────────────────────────────────────

/// Query params for listing files.
#[derive(Debug, Deserialize)]
pub struct ListFilesQuery {
    folder: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

/// Response shape for a file — mirrors Canvas attachment_json field order:
/// id, then display_name/filename, then content-type/size/timestamps.
/// (`size` is the Canvas field name; migration 0016 renamed size_bytes -> size.)
#[derive(Debug, Serialize)]
pub struct FileResponse {
    pub id: i64,
    pub course_id: i64,
    pub folder: String,
    pub filename: String,
    pub display_name: String,
    pub content_type: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating a file record.
///
/// Metadata only — no upload. display_name defaults to filename when absent
/// (Canvas `infer_display_name` behavior).
#[derive(Debug, Deserialize)]
pub struct CreateFileRequest {
    pub filename: String,
    pub display_name: Option<String>,
    pub content_type: Option<String>,
    pub size: Option<i64>,
    pub folder: Option<String>,
}

// ─── router ────────────────────────────────────────────────────────────────

/// Build the files router.
pub fn router() -> axum::Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/files", get(list_course_files))
        .route("/courses/{courseId}/files", post(create_file))
        .route("/files/{id}", get(get_file))
        .route("/files/{id}", delete(delete_file))
}

// ─── handlers ─────────────────────────────────────────────────────────────

/// GET /courses/{courseId}/files — list files for a course (enrolled or admin)
async fn list_course_files(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<ListFilesQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<FileResponse>>, ApiError> {
    let allowed = user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    // Canvas lists are ordered by display_name (display_name_order_by_clause);
    // we keep created_at DESC (recency) as before — display_name ordering is
    // a UI concern here.
    let files = if let Some(ref folder) = params.folder {
        sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, course_id, folder, filename, display_name, content_type, size, created_at, updated_at
            FROM files
            WHERE course_id = $1 AND folder = $2
            ORDER BY created_at DESC
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(course_id)
        .bind(folder)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, course_id, folder, filename, display_name, content_type, size, created_at, updated_at
            FROM files
            WHERE course_id = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(course_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?
    };

    Ok(Json(files.into_iter().map(FileResponse::from).collect()))
}

/// POST /courses/{courseId}/files — create file metadata (enrolled or admin).
///
/// Metadata only: no bytes are stored (upload/S3 out of scope). display_name
/// defaults to filename if not provided, mirroring Canvas infer_display_name.
async fn create_file(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateFileRequest>,
) -> Result<(StatusCode, Json<FileResponse>), ApiError> {
    if payload.filename.trim().is_empty() {
        return Err(ApiError::Validation("filename is required".into()));
    }

    let allowed = user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    // Canvas infer_display_name: display_name ||= filename (empty -> filename).
    let display_name = match payload.display_name {
        Some(ref d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => payload.filename.clone(),
    };
    let content_type = payload
        .content_type
        .unwrap_or_else(|| "application/octet-stream".into());
    let size = payload.size.unwrap_or(0);
    let folder = payload.folder.unwrap_or_else(|| "unfiled".into());

    let row = sqlx::query_as::<_, FileRow>(
        r#"
        INSERT INTO files (course_id, uploader_id, filename, display_name, content_type, size, folder)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, course_id, folder, filename, display_name, content_type, size, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(&payload.filename)
    .bind(&display_name)
    .bind(&content_type)
    .bind(size)
    .bind(&folder)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(FileResponse::from(row))))
}

/// GET /files/{id} — get a single file (uploader or enrolled or admin)
async fn get_file(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<FileResponse>, ApiError> {
    let file: Option<FileRow> = sqlx::query_as(
        "SELECT id, course_id, folder, filename, display_name, content_type, size, created_at, updated_at FROM files WHERE id = $1",
    )
    .bind(file_id)
    .fetch_optional(&state.db)
    .await?;

    let file = file.ok_or(ApiError::NotFound)?;

    let is_uploader = file.id == auth.id;
    let allowed = user_enrolled_or_admin(&state.db, auth.id, file.course_id, &auth.role).await?;

    if !is_uploader && !allowed {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(FileResponse::from(file)))
}

/// DELETE /files/{id} — delete a file (uploader or course teacher or admin)
async fn delete_file(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    auth: AuthUser,
) -> Result<impl IntoResponse, ApiError> {
    let existing: Option<(i64, i64)> =
        sqlx::query_as("SELECT course_id, uploader_id FROM files WHERE id = $1")
            .bind(file_id)
            .fetch_optional(&state.db)
            .await?;

    let (course_id, uploader_id) = existing.ok_or(ApiError::NotFound)?;

    let is_uploader = uploader_id == auth.id;
    let allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;

    if !is_uploader && !allowed {
        return Err(ApiError::Forbidden);
    }

    sqlx::query("DELETE FROM files WHERE id = $1")
        .bind(file_id)
        .execute(&state.db)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ─── row type ─────────────────────────────────────────────────────────────

/// Raw files row as selected from the DB (column order matches every SELECT above).
#[derive(Debug, sqlx::FromRow)]
struct FileRow {
    id: i64,
    course_id: i64,
    folder: String,
    filename: String,
    display_name: String,
    content_type: String,
    size: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<FileRow> for FileResponse {
    fn from(r: FileRow) -> Self {
        FileResponse {
            id: r.id,
            course_id: r.course_id,
            folder: r.folder,
            filename: r.filename,
            display_name: r.display_name,
            content_type: r.content_type,
            size: r.size,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_router_builds() {
        // Verify router builds without panicking.
        let _ = super::router();
    }
}
