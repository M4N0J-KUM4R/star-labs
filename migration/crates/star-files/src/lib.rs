//! star-files domain crate — file metadata tracking (no S3/upload) and folders.
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
//! - folder_id: folder row the file belongs to (migration 0036)
//!
//! Responses lead with display_name (Canvas behavior). Actual upload/S3
//! storage is explicitly out of scope — POST records metadata only.
//!
//! Folders mirror Canvas folders_controller.rb / app/models/folder.rb:
//! - course root folder is created on demand and named 'course files'
//! - `full_name` is the path including the root name
//! - names are normalized ("/" -> "_", blank -> "New Folder") and duplicates
//!   get a " 2", " 3", ... suffix (Canvas prevent_duplicate_name)
//! - DELETE soft-deletes (workflow_state = 'deleted') and refuses the root
//!   folder or a non-empty folder unless force=true
//!
//! Mixed listings (migration 0061):
//! - GET /courses/{courseId}/folders_and_files and GET /folders/{id}/all mirror
//!   Canvas list_all_folders_and_files / list_folders_and_files: folders first,
//!   then files, one page window across both, X-Total-Items header, Canvas
//!   attachment_json/folder_json item shapes.
//! - GET /courses/{courseId}/files/quota (and /files/quota?course_id=) mirror
//!   Canvas files#api_quota: `{ quota, quota_used }` from courses.storage_quota
//!   with the 50 MB default and 512-byte per-file minimum.

use axum::{
    extract::{Path, Query, State},
    http::{header::HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;

use star_common::{ApiError, AppState};

use star_auth::AuthUser;

// ─── folder constants ──────────────────────────────────────────────────────

/// Canvas Folder::ROOT_FOLDER_NAME.
const COURSE_ROOT_FOLDER_NAME: &str = "course files";
/// Default folder for files created without a folder (Canvas unfiled_folder).
const UNFILED_FOLDER_NAME: &str = "unfiled";
/// Canvas fallback when the folder name is blank (infer_full_name).
const DEFAULT_FOLDER_NAME: &str = "New Folder";
/// Canvas Attachment::CONTEXT_DEFAULT_QUOTA (50 decimal megabytes).
const DEFAULT_STORAGE_QUOTA: i64 = 50_000_000;
/// Canvas Attachment::MINIMUM_SIZE_FOR_QUOTA.
const MINIMUM_SIZE_FOR_QUOTA: i64 = 512;
/// Canvas folders_and_files pagination default (Api.per_page default: 25).
const MIXED_LIST_PER_PAGE: i64 = 25;

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

// ─── folder helpers ────────────────────────────────────────────────────────

/// Canvas Folder.root_folder_name_for_context.
fn root_folder_name_for_context(context_type: &str) -> &'static str {
    match context_type {
        "Course" => COURSE_ROOT_FOLDER_NAME,
        "User" => "my files",
        _ => "files",
    }
}

/// Folder names are stripped, blank names become "New Folder", and slashes are
/// replaced with underscores (Canvas Folder#infer_full_name).
fn normalize_folder_name(name: Option<&str>) -> String {
    let raw = name.unwrap_or("").trim();
    if raw.is_empty() {
        DEFAULT_FOLDER_NAME.to_string()
    } else {
        raw.replace('/', "_")
    }
}

/// Canvas Folder#prevent_duplicate_name: if a sibling already uses `name`,
/// append the first free " <n>" suffix starting at 2.
fn dedupe_folder_name(name: &str, sibling_names: &[String]) -> String {
    if !sibling_names.iter().any(|s| s == name) {
        return name.to_string();
    }
    let prefix = format!("{name} ");
    let mut used: Vec<u32> = sibling_names
        .iter()
        .filter_map(|s| s.strip_prefix(&prefix).and_then(|n| n.parse::<u32>().ok()))
        .filter(|n| *n > 1)
        .collect();
    used.sort_unstable();
    let mut candidate = 2u32;
    for i in used {
        if candidate < i {
            break;
        }
        if candidate == i {
            candidate += 1;
        }
    }
    format!("{name} {candidate}")
}

/// Join a parent folder's full name with a child name (Canvas `full_name`).
fn join_full_name(parent_full_name: &str, name: &str) -> String {
    let parent = parent_full_name.trim_end_matches('/');
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

/// Folder responses are only served for Course contexts.
fn folder_course_id(folder: &FolderRow) -> Result<i64, ApiError> {
    if folder.context_type != "Course" {
        return Err(ApiError::Forbidden);
    }
    Ok(folder.context_id)
}

fn pagination(page: Option<i64>, per_page: Option<i64>) -> (i64, i64) {
    let page = page.unwrap_or(1).max(1);
    let per_page = per_page.unwrap_or(20).clamp(1, 100);
    (per_page, (page - 1) * per_page)
}

// ─── folders_and_files (mixed listing) helpers ─────────────────────────────

/// Sortable columns accepted by the Canvas folders_and_files listing
/// (`sort`/`sort_by` + `order` params).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortField {
    Name,
    CreatedAt,
    UpdatedAt,
    Size,
}

fn parse_sort_field(raw: Option<&str>) -> SortField {
    match raw.unwrap_or("").to_ascii_lowercase().as_str() {
        "created_at" => SortField::CreatedAt,
        "updated_at" => SortField::UpdatedAt,
        "size" => SortField::Size,
        _ => SortField::Name,
    }
}

fn sort_is_desc(raw: Option<&str>) -> bool {
    raw.map(|order| order.eq_ignore_ascii_case("desc"))
        .unwrap_or(false)
}

/// Folder ORDER BY clause. Folders have no size, so `size` falls back to name
/// (Canvas folder_column_map has no size column either).
fn folder_order_clause(sort: SortField, desc: bool) -> String {
    let dir = if desc { "DESC" } else { "ASC" };
    let column = match sort {
        SortField::CreatedAt => "f.created_at",
        SortField::UpdatedAt => "f.updated_at",
        SortField::Name | SortField::Size => "f.name",
    };
    format!(" ORDER BY {column} {dir}, f.id {dir}")
}

/// File ORDER BY clause (Canvas file_column_map: display_name is "name").
fn file_order_clause(sort: SortField, desc: bool) -> String {
    let dir = if desc { "DESC" } else { "ASC" };
    let column = match sort {
        SortField::CreatedAt => "f.created_at",
        SortField::UpdatedAt => "f.updated_at",
        SortField::Size => "f.size",
        SortField::Name => "f.display_name",
    };
    format!(" ORDER BY {column} {dir}, f.id {dir}")
}

/// Page window over the concatenated folders-then-files collection
/// (Canvas paginated_folders_and_files_collection).
#[derive(Debug, PartialEq, Eq)]
struct PageWindow {
    folder_limit: i64,
    folder_offset: i64,
    file_limit: i64,
    file_offset: i64,
}

fn combined_page_window(folder_total: i64, per_page: i64, offset: i64) -> PageWindow {
    let folder_total = folder_total.max(0);
    let per_page = per_page.max(0);
    let offset = offset.max(0);
    if offset >= folder_total {
        PageWindow {
            folder_limit: 0,
            folder_offset: 0,
            file_limit: per_page,
            file_offset: offset - folder_total,
        }
    } else {
        let folder_limit = per_page.min(folder_total - offset);
        PageWindow {
            folder_limit,
            folder_offset: offset,
            file_limit: per_page - folder_limit,
            file_offset: 0,
        }
    }
}

// ─── attachment_json helpers ───────────────────────────────────────────────

/// Canvas Attachment.mime_class (app/models/attachment.rb): strip MIME
/// parameters, look up the Canvas type table, then fall back to content-type
/// prefixes and finally "file".
fn mime_class(content_type: &str) -> &'static str {
    let base = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "text/html" => "html",
        "text/x-csharp" | "text/xml" | "text/css" | "application/xml" => "code",
        "text" | "text/plain" => "text",
        "application/rtf"
        | "text/rtf"
        | "application/vnd.oasis.opendocument.text"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.apple.pages"
        | "application/x-docx"
        | "application/msword" => "doc",
        "application/pdf" => "pdf",
        "application/vnd.ms-powerpoint"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "applicatoin/vnd.apple.key" => "ppt",
        "application/vnd.ms-excel"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "application/vnd.apple.numbers"
        | "application/vnd.oasis.opendocument.spreadsheet" => "xls",
        "image/jpeg"
        | "image/pjpeg"
        | "image/png"
        | "image/gif"
        | "image/bmp"
        | "image/svg+xml"
        | "image/webp"
        | "image/vnd.microsoft.icon" => "image",
        "application/x-rar"
        | "application/x-rar-compressed"
        | "application/x-zip"
        | "application/x-zip-compressed"
        | "application/zip" => "zip",
        _ if base.starts_with("audio/") => "audio",
        _ if base.starts_with("video/") => "video",
        _ if base.starts_with("image/") => "image",
        _ if base.starts_with("text/") => "text",
        _ => "file",
    }
}

/// Canvas Attachment.get_quota: a context quota of `nil` becomes the 50 MB
/// default; an explicit 0 stays 0 (Ruby truthiness).
fn effective_quota(storage_quota: Option<i64>) -> i64 {
    storage_quota.unwrap_or(DEFAULT_STORAGE_QUOTA)
}

/// Canvas Attachment.get_quota sums `max(size, MINIMUM_SIZE_FOR_QUOTA)`.
#[cfg(test)]
fn quota_used(sizes: &[i64]) -> i64 {
    sizes
        .iter()
        .map(|size| (*size).max(MINIMUM_SIZE_FOR_QUOTA))
        .sum()
}

fn item_json<T: Serialize>(value: T) -> Result<serde_json::Value, ApiError> {
    serde_json::to_value(value).map_err(|e| ApiError::Db(format!("serialize: {e}")))
}

/// Canvas-shaped error body for folder API conflicts (`{"message": ...}`).
fn canvas_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "message": message }))).into_response()
}

/// Canvas-shaped 404 for a missing parent folder.
fn canvas_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "errors": [{ "message": "The specified resource does not exist." }] })),
    )
        .into_response()
}

// ─── DTOs ───────────────────────────────────────────────────────────────────

/// Query params for listing files.
#[derive(Debug, Deserialize)]
pub struct ListFilesQuery {
    folder: Option<String>,
    folder_id: Option<i64>,
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
    pub folder_id: Option<i64>,
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
    pub folder_id: Option<i64>,
}

/// Query params for GET /folders (list children or all context folders).
#[derive(Debug, Deserialize)]
pub struct ListFoldersQuery {
    folder_id: Option<i64>,
    course_id: Option<i64>,
    context_id: Option<i64>,
    context_type: Option<String>,
    sort_by: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

/// Query params for folder list endpoints.
#[derive(Debug, Deserialize)]
pub struct FolderListQuery {
    sort_by: Option<String>,
    page: Option<i64>,
    per_page: Option<i64>,
}

/// Query params for GET /folders/{id} (needed only for id="root").
#[derive(Debug, Deserialize)]
pub struct FolderLookupQuery {
    course_id: Option<i64>,
    context_id: Option<i64>,
}

/// Query params for GET /folders/{id}/files.
#[derive(Debug, Deserialize)]
pub struct FolderFilesQuery {
    page: Option<i64>,
    per_page: Option<i64>,
}

/// Query params for DELETE /folders/{id}.
#[derive(Debug, Deserialize)]
pub struct DeleteFolderQuery {
    force: Option<String>,
}

/// Request body for creating a folder (Canvas folders#create).
#[derive(Debug, Deserialize)]
pub struct CreateFolderRequest {
    pub name: Option<String>,
    pub parent_folder_id: Option<i64>,
    pub parent_folder_path: Option<String>,
    /// Canvas POST /folders/{folder_id}/folders also accepts it in the body.
    pub folder_id: Option<i64>,
    pub course_id: Option<i64>,
    pub context_type: Option<String>,
    pub context_id: Option<i64>,
    pub lock_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub locked: Option<bool>,
    pub hidden: Option<bool>,
    pub position: Option<i32>,
    pub for_submissions: Option<bool>,
}

/// Request body for updating a folder (Canvas folders#update).
#[derive(Debug, Deserialize)]
pub struct UpdateFolderRequest {
    pub name: Option<String>,
    pub parent_folder_id: Option<i64>,
    pub lock_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub locked: Option<bool>,
    pub hidden: Option<bool>,
    pub position: Option<i32>,
    /// Canvas legacy UI flag: "1" hides and unlocks the folder.
    pub just_hide: Option<String>,
}

/// Canvas folder_json response shape (lib/api/v1/folders.rb).
#[derive(Debug, Serialize)]
pub struct FolderResponse {
    pub id: i64,
    pub name: String,
    pub full_name: String,
    pub position: i32,
    pub parent_folder_id: Option<i64>,
    pub context_type: String,
    pub context_id: i64,
    pub unlock_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub category: Option<String>,
    pub locked: bool,
    pub all_url: String,
    pub folders_url: String,
    pub files_url: String,
    pub files_count: i64,
    pub folders_count: i64,
    pub hidden: bool,
    pub locked_for_user: bool,
    pub hidden_for_user: bool,
    pub for_submissions: bool,
    pub can_upload: bool,
}

/// Query params for the Canvas folders_and_files listings
/// (GET /courses/{courseId}/folders_and_files and GET /folders/{id}/all).
#[derive(Debug, Deserialize)]
pub struct FoldersAndFilesQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    /// Canvas uses `sort`; accept `sort_by` too for consistency with folders.
    sort: Option<String>,
    sort_by: Option<String>,
    order: Option<String>,
    search_term: Option<String>,
}

/// Canvas attachment_json shape (lib/api/v1/attachment.rb).
///
/// Metadata only — `url`/`thumbnail_url` are empty because no binary storage
/// exists. Both `content-type` (Canvas key) and `content_type` (star-lms
/// convention) are emitted so existing clients keep working.
#[derive(Debug, Serialize)]
pub struct AttachmentResponse {
    pub id: i64,
    pub uuid: Option<String>,
    pub folder_id: Option<i64>,
    pub display_name: String,
    pub filename: String,
    pub upload_status: String,
    #[serde(rename = "content-type")]
    pub content_type: String,
    #[serde(rename = "content_type")]
    pub content_type_compat: String,
    pub url: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub locked: bool,
    pub hidden: bool,
    pub lock_at: Option<DateTime<Utc>>,
    pub hidden_for_user: bool,
    pub locked_for_user: bool,
    pub thumbnail_url: String,
    pub modified_at: DateTime<Utc>,
    pub mime_class: String,
    pub media_entry_id: Option<String>,
    pub category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility_level: Option<String>,
    /// star-lms extensions kept for parity with the existing files API.
    pub course_id: i64,
    pub folder: String,
}

/// Query params for GET /files/quota (shorthand; course_id in the query).
#[derive(Debug, Deserialize)]
pub struct QuotaQuery {
    course_id: Option<i64>,
}

/// Canvas files#api_quota response: `{ quota, quota_used }`.
#[derive(Debug, Serialize)]
pub struct QuotaResponse {
    pub quota: i64,
    pub quota_used: i64,
}

// ─── router ────────────────────────────────────────────────────────────────

/// Build the files router.
pub fn router() -> axum::Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/files", get(list_course_files))
        .route("/courses/{courseId}/files", post(create_file))
        .route(
            "/courses/{courseId}/files/quota",
            get(get_course_files_quota),
        )
        .route("/files/quota", get(get_files_quota))
        .route("/files/{id}", get(get_file))
        .route("/files/{id}", delete(delete_file))
        // Canvas folders API (lib/api/v1/folders + FoldersController).
        .route("/folders", get(list_folders))
        .route("/folders", post(create_folder))
        .route("/folders/{id}", get(get_folder))
        .route("/folders/{id}", put(update_folder))
        .route("/folders/{id}", delete(delete_folder))
        .route("/folders/{id}/folders", get(list_folder_children))
        .route("/folders/{id}/folders", post(create_subfolder))
        .route("/folders/{id}/files", get(list_folder_files))
        .route("/folders/{id}/all", get(list_folder_all))
        .route(
            "/courses/{courseId}/folders_and_files",
            get(list_course_folders_and_files),
        )
        .route("/courses/{courseId}/folders", get(list_course_folders))
        .route("/courses/{courseId}/folders", post(create_course_folder))
        .route(
            "/courses/{courseId}/folders/root",
            get(get_course_root_folder),
        )
}

// ─── file handlers ─────────────────────────────────────────────────────────

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

    let (per_page, offset) = pagination(params.page, params.per_page);

    // Canvas lists are ordered by display_name (display_name_order_by_clause);
    // we keep created_at DESC (recency) as before — display_name ordering is
    // a UI concern here.
    let files = if let Some(folder_id) = params.folder_id {
        sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, course_id, folder, folder_id, filename, display_name, content_type, size, created_at, updated_at
            FROM files
            WHERE course_id = $1 AND folder_id = $2
            ORDER BY created_at DESC
            LIMIT $3 OFFSET $4
            "#,
        )
        .bind(course_id)
        .bind(folder_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?
    } else if let Some(ref folder) = params.folder {
        sqlx::query_as::<_, FileRow>(
            r#"
            SELECT id, course_id, folder, folder_id, filename, display_name, content_type, size, created_at, updated_at
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
            SELECT id, course_id, folder, folder_id, filename, display_name, content_type, size, created_at, updated_at
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

    // A folder_id, when given, must be an active folder in this course. The
    // legacy `folder` text path is derived from the folder's full_name.
    let (folder, folder_id) = if let Some(folder_id) = payload.folder_id {
        let found: Option<(i64, String, String, i64)> = sqlx::query_as(
            "SELECT id, full_name, context_type, context_id FROM folders \
             WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(folder_id)
        .fetch_optional(&state.db)
        .await?;
        let (id, full_name, context_type, context_id) = found.ok_or(ApiError::NotFound)?;
        if context_type != "Course" || context_id != course_id {
            return Err(ApiError::Validation(
                "folder does not belong to this course".into(),
            ));
        }
        (full_name, Some(id))
    } else {
        (
            payload.folder.unwrap_or_else(|| UNFILED_FOLDER_NAME.into()),
            None,
        )
    };

    let row = sqlx::query_as::<_, FileRow>(
        r#"
        INSERT INTO files (course_id, uploader_id, filename, display_name, content_type, size, folder, folder_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, course_id, folder, folder_id, filename, display_name, content_type, size, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(&payload.filename)
    .bind(&display_name)
    .bind(&content_type)
    .bind(size)
    .bind(&folder)
    .bind(folder_id)
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
        "SELECT id, course_id, folder, folder_id, filename, display_name, content_type, size, created_at, updated_at FROM files WHERE id = $1",
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

// ─── folder handlers ───────────────────────────────────────────────────────

/// GET /folders — list child folders (`?folder_id=`) or all folders for a
/// context (`?course_id=` / `?context_id=`). Canvas list_all_folders/api_index.
async fn list_folders(
    State(state): State<AppState>,
    Query(params): Query<ListFoldersQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<FolderResponse>>, ApiError> {
    if let Some(context_type) = params.context_type.as_deref() {
        if context_type != "Course" {
            return Err(ApiError::BadRequest(
                "only Course folder contexts are supported".into(),
            ));
        }
    }

    let (per_page, offset) = pagination(params.page, params.per_page);
    let by_position = params.sort_by.as_deref() == Some("position");

    let (rows, can_manage) = if let Some(folder_id) = params.folder_id {
        let folder = fetch_folder(&state.db, folder_id)
            .await?
            .ok_or(ApiError::NotFound)?;
        let course_id = folder_course_id(&folder)?;
        let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
        let allowed =
            can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
        if !allowed {
            return Err(ApiError::Forbidden);
        }
        let rows = fetch_child_folders(
            &state.db,
            folder_id,
            can_manage,
            by_position,
            per_page,
            offset,
        )
        .await?;
        (rows, can_manage)
    } else {
        let course_id = params
            .course_id
            .or(params.context_id)
            .ok_or_else(|| ApiError::BadRequest("course_id or folder_id is required".into()))?;
        let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
        let allowed =
            can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
        if !allowed {
            return Err(ApiError::Forbidden);
        }
        ensure_root_folder(&state.db, course_id).await?;
        let rows = fetch_context_folders(
            &state.db,
            course_id,
            can_manage,
            by_position,
            per_page,
            offset,
        )
        .await?;
        (rows, can_manage)
    };

    Ok(Json(
        rows.into_iter()
            .map(|row| FolderResponse::from_row(row, can_manage))
            .collect(),
    ))
}

/// GET /courses/{courseId}/folders — Canvas list_all_folders (flat list).
async fn list_course_folders(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<FolderListQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<FolderResponse>>, ApiError> {
    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed =
        can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    ensure_root_folder(&state.db, course_id).await?;

    let (per_page, offset) = pagination(params.page, params.per_page);
    let by_position = params.sort_by.as_deref() == Some("position");
    let rows = fetch_context_folders(
        &state.db,
        course_id,
        can_manage,
        by_position,
        per_page,
        offset,
    )
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|row| FolderResponse::from_row(row, can_manage))
            .collect(),
    ))
}

/// GET /folders/{id}/folders — Canvas api_index (children of one folder).
async fn list_folder_children(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
    Query(params): Query<FolderListQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<FolderResponse>>, ApiError> {
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let course_id = folder_course_id(&folder)?;

    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed =
        can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let (per_page, offset) = pagination(params.page, params.per_page);
    let by_position = params.sort_by.as_deref() == Some("position");
    let rows = fetch_child_folders(
        &state.db,
        folder_id,
        can_manage,
        by_position,
        per_page,
        offset,
    )
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|row| FolderResponse::from_row(row, can_manage))
            .collect(),
    ))
}

/// GET /folders/{id} — Canvas folders#show. `id` may be "root" together with
/// `?course_id=` to resolve a course root folder.
async fn get_folder(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<FolderLookupQuery>,
    auth: AuthUser,
) -> Result<Json<FolderResponse>, ApiError> {
    let folder = if id == "root" {
        let course_id = params
            .course_id
            .or(params.context_id)
            .ok_or_else(|| ApiError::BadRequest("course_id is required for root".into()))?;
        ensure_root_folder(&state.db, course_id).await?
    } else {
        let folder_id: i64 = id.parse().map_err(|_| ApiError::NotFound)?;
        fetch_folder(&state.db, folder_id)
            .await?
            .ok_or(ApiError::NotFound)?
    };

    let course_id = folder_course_id(&folder)?;
    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed =
        can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(FolderResponse::from_row(folder, can_manage)))
}

/// GET /courses/{courseId}/folders/root — Canvas folders#show with id=root.
async fn get_course_root_folder(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<FolderResponse>, ApiError> {
    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed =
        can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    let folder = ensure_root_folder(&state.db, course_id).await?;
    Ok(Json(FolderResponse::from_row(folder, can_manage)))
}

/// GET /folders/{id}/files — Canvas files#api_index for one folder.
async fn list_folder_files(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
    Query(params): Query<FolderFilesQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<FileResponse>>, ApiError> {
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let course_id = folder_course_id(&folder)?;

    let allowed = user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let (per_page, offset) = pagination(params.page, params.per_page);
    let files = sqlx::query_as::<_, FileRow>(
        r#"
        SELECT id, course_id, folder, folder_id, filename, display_name, content_type, size, created_at, updated_at
        FROM files
        WHERE folder_id = $1
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(folder_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(files.into_iter().map(FileResponse::from).collect()))
}

/// POST /folders — Canvas folders#create with context in the body.
async fn create_folder(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateFolderRequest>,
) -> Result<Response, ApiError> {
    create_folder_inner(&state, &auth, payload, None, None).await
}

/// POST /courses/{courseId}/folders — Canvas context create.
async fn create_course_folder(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateFolderRequest>,
) -> Result<Response, ApiError> {
    create_folder_inner(&state, &auth, payload, Some(course_id), None).await
}

/// POST /folders/{id}/folders — Canvas create sub-folder.
async fn create_subfolder(
    State(state): State<AppState>,
    Path(parent_folder_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateFolderRequest>,
) -> Result<Response, ApiError> {
    create_folder_inner(&state, &auth, payload, None, Some(parent_folder_id)).await
}

async fn create_folder_inner(
    state: &AppState,
    auth: &AuthUser,
    payload: CreateFolderRequest,
    forced_course_id: Option<i64>,
    forced_parent_id: Option<i64>,
) -> Result<Response, ApiError> {
    let body_parent_id = payload.folder_id.or(payload.parent_folder_id);
    let parent_id_param = forced_parent_id.or(body_parent_id);

    // Canvas rejects mixing parent_folder_path with an explicit parent id.
    if payload.parent_folder_path.is_some()
        && (forced_parent_id.is_some() || body_parent_id.is_some())
    {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "Can't set folder path and folder id",
        ));
    }

    if let Some(context_type) = payload.context_type.as_deref() {
        if context_type != "Course" {
            return Ok(canvas_error(
                StatusCode::BAD_REQUEST,
                "only Course folder contexts are supported",
            ));
        }
    }

    let mut parent: Option<FolderRow> = None;
    let mut course_id = forced_course_id
        .or(payload.course_id)
        .or(payload.context_id);

    if let Some(parent_id) = parent_id_param {
        let Some(folder) = fetch_folder(&state.db, parent_id).await? else {
            return Ok(canvas_not_found());
        };
        let folder_course = folder_course_id(&folder)?;
        if let Some(requested_course) = course_id {
            if requested_course != folder_course {
                return Ok(canvas_not_found());
            }
        }
        course_id = Some(folder_course);
        parent = Some(folder);
    }

    let Some(course_id) = course_id else {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "course_id is required",
        ));
    };

    // Authorize before creating any parent folders via parent_folder_path.
    let allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    if parent.is_none() {
        if let Some(path) = payload.parent_folder_path.as_deref() {
            parent = Some(ensure_path(&state.db, course_id, path).await?);
        }
    }

    if parent.is_none() {
        let root = ensure_root_folder(&state.db, course_id).await?;
        parent = Some(ensure_unfiled_folder(&state.db, root).await?);
    }
    let parent = parent.expect("parent folder resolved above");
    if parent.context_id != course_id {
        return Ok(canvas_not_found());
    }

    let requested_name = normalize_folder_name(payload.name.as_deref());
    let siblings = fetch_sibling_names(&state.db, parent.id).await?;
    let name = dedupe_folder_name(&requested_name, &siblings);
    let full_name = join_full_name(&parent.full_name, &name);
    let position = match payload.position {
        Some(position) => position,
        None => next_position(&state.db, parent.id).await?,
    };
    let workflow_state = if payload.hidden.unwrap_or(false) {
        "hidden"
    } else {
        "visible"
    };

    let new_folder = NewFolder {
        name,
        full_name,
        course_id,
        parent_id: parent.id,
        workflow_state: workflow_state.to_string(),
        position,
        locked: payload.locked.unwrap_or(false),
        lock_at: payload.lock_at,
        unlock_at: payload.unlock_at,
        for_submissions: payload.for_submissions.unwrap_or(false),
    };
    let row = insert_folder(&state.db, new_folder).await?;

    Ok((StatusCode::OK, Json(FolderResponse::from_row(row, true))).into_response())
}

/// PUT /folders/{id} — Canvas folders#update.
async fn update_folder(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateFolderRequest>,
) -> Result<Response, ApiError> {
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let course_id = folder_course_id(&folder)?;

    let allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let mut name = folder.name.clone();
    if let Some(requested_name) = payload.name.as_deref() {
        let normalized = normalize_folder_name(Some(requested_name));
        if folder.parent_folder_id.is_none() && normalized != folder.name {
            return Ok(canvas_error(
                StatusCode::BAD_REQUEST,
                "Root folder name cannot be changed",
            ));
        }
        name = normalized;
    }

    // Canvas prevent_duplicate_name runs on every save.
    let siblings: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT name FROM folders \
         WHERE parent_folder_id IS NOT DISTINCT FROM $1 AND id <> $2 AND workflow_state <> 'deleted'",
    )
    .bind(folder.parent_folder_id)
    .bind(folder.id)
    .fetch_all(&state.db)
    .await?;
    let name = dedupe_folder_name(&name, &siblings);

    let mut parent: Option<FolderRow> = None;
    if let Some(new_parent_id) = payload.parent_folder_id {
        if new_parent_id == folder.id {
            return Ok(canvas_error(
                StatusCode::BAD_REQUEST,
                "A folder cannot be the parent of itself",
            ));
        }
        let Some(target) = fetch_folder(&state.db, new_parent_id).await? else {
            return Ok(canvas_not_found());
        };
        if target.context_type != folder.context_type || target.context_id != folder.context_id {
            return Ok(canvas_error(
                StatusCode::BAD_REQUEST,
                "The new folder must be in the same context as the original parent folder",
            ));
        }
        if is_descendant_of(&state.db, folder.id, target.id).await? {
            return Ok(canvas_error(
                StatusCode::BAD_REQUEST,
                "A folder cannot be the parent of itself",
            ));
        }
        parent = Some(target);
    } else if let Some(parent_id) = folder.parent_folder_id {
        parent = fetch_folder(&state.db, parent_id).await?;
    }

    let parent_id = parent.as_ref().map(|p| p.id).or(folder.parent_folder_id);
    let full_name = match parent.as_ref() {
        Some(parent) => join_full_name(&parent.full_name, &name),
        None if folder.parent_folder_id.is_none() => name.clone(),
        None => folder.full_name.clone(),
    };

    let mut workflow_state = folder.workflow_state.clone();
    if let Some(hidden) = payload.hidden {
        workflow_state = if hidden { "hidden" } else { "visible" }.to_string();
    }
    if payload.just_hide.as_deref() == Some("1") {
        workflow_state = "hidden".to_string();
    }
    let locked = if payload.just_hide.as_deref() == Some("1") {
        false
    } else {
        payload.locked.unwrap_or(folder.locked)
    };

    sqlx::query(
        r#"
        UPDATE folders SET
            name = $2,
            full_name = $3,
            parent_folder_id = $4,
            workflow_state = $5,
            position = COALESCE($6, position),
            locked = $7,
            lock_at = $8,
            unlock_at = $9,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(folder.id)
    .bind(&name)
    .bind(&full_name)
    .bind(parent_id)
    .bind(&workflow_state)
    .bind(payload.position)
    .bind(locked)
    .bind(payload.lock_at.or(folder.lock_at))
    .bind(payload.unlock_at.or(folder.unlock_at))
    .execute(&state.db)
    .await?;

    // A rename or move changes the full_name of every descendant.
    recompute_descendant_full_names(&state.db, folder.id).await?;

    let updated = fetch_folder(&state.db, folder.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(FolderResponse::from_row(updated, true)).into_response())
}

/// DELETE /folders/{id} — Canvas folders#api_destroy (soft delete).
async fn delete_folder(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
    Query(params): Query<DeleteFolderQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let course_id = folder_course_id(&folder)?;

    let allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    if folder.parent_folder_id.is_none() {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "Can't delete the root folder",
        ));
    }

    let (has_subfolders,): (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM folders WHERE parent_folder_id = $1 AND workflow_state <> 'deleted')",
    )
    .bind(folder.id)
    .fetch_one(&state.db)
    .await?;
    let (has_files,): (bool,) =
        sqlx::query_as("SELECT EXISTS(SELECT 1 FROM files WHERE folder_id = $1)")
            .bind(folder.id)
            .fetch_one(&state.db)
            .await?;
    let force = params.force.as_deref() == Some("true");
    if (has_subfolders || has_files) && !force {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "Can't delete a folder with content",
        ));
    }

    soft_delete_folder_tree(&state.db, folder.id).await?;

    Ok(Json(FolderResponse::from_row(folder, true)).into_response())
}

// ─── folders_and_files + quota handlers ────────────────────────────────────

/// GET /courses/{courseId}/folders_and_files — Canvas internal API
/// (FoldersController#list_all_folders_and_files): a flat, paginated listing
/// of every course folder followed by every course file, with X-Total-Items.
async fn list_course_folders_and_files(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<FoldersAndFilesQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed =
        can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    folders_and_files_response(&state.db, course_id, None, can_manage, &params).await
}

/// GET /folders/{id}/all — Canvas FoldersController#list_folders_and_files:
/// mixed listing of one folder's subfolders (then its files).
async fn list_folder_all(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
    Query(params): Query<FoldersAndFilesQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let course_id = folder_course_id(&folder)?;

    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed =
        can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    folders_and_files_response(&state.db, course_id, Some(folder.id), can_manage, &params).await
}

/// Build the Canvas folders_and_files body: folders sort before files, the
/// page window spans the concatenation, and X-Total-Items reports both counts.
async fn folders_and_files_response(
    db: &PgPool,
    course_id: i64,
    folder_id: Option<i64>,
    can_manage: bool,
    params: &FoldersAndFilesQuery,
) -> Result<Response, ApiError> {
    let per_page = params.per_page.unwrap_or(MIXED_LIST_PER_PAGE).clamp(1, 100);
    let offset = (params.page.unwrap_or(1).max(1) - 1) * per_page;
    let sort = parse_sort_field(params.sort.as_deref().or(params.sort_by.as_deref()));
    let desc = sort_is_desc(params.order.as_deref());
    let search = params
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty());

    let folder_filter = FolderFilter {
        course_id,
        parent_id: folder_id,
        can_manage,
        search,
    };
    let attachment_filter = AttachmentFilter {
        course_id,
        folder_id,
        can_manage,
        search,
    };

    let folder_total = count_folders_for_listing(db, &folder_filter).await?;
    let file_total = count_attachments_for_listing(db, &attachment_filter).await?;
    let window = combined_page_window(folder_total, per_page, offset);

    let mut items: Vec<serde_json::Value> = Vec::new();
    if window.folder_limit > 0 {
        let rows = fetch_folders_for_listing(
            db,
            &folder_filter,
            sort,
            desc,
            window.folder_limit,
            window.folder_offset,
        )
        .await?;
        for row in rows {
            items.push(item_json(FolderResponse::from_row(row, can_manage))?);
        }
    }
    if window.file_limit > 0 {
        let rows = fetch_attachments_for_listing(
            db,
            &attachment_filter,
            sort,
            desc,
            window.file_limit,
            window.file_offset,
        )
        .await?;
        for row in rows {
            items.push(item_json(AttachmentResponse::from_row(row, can_manage))?);
        }
    }

    let total = folder_total + file_total;
    let mut response = Json(items).into_response();
    if let Ok(value) = HeaderValue::from_bytes(total.to_string().as_bytes()) {
        response
            .headers_mut()
            .insert(HeaderName::from_static("x-total-items"), value);
    }
    Ok(response)
}

/// GET /courses/{courseId}/files/quota — Canvas files#api_quota.
async fn get_course_files_quota(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<QuotaResponse>, ApiError> {
    files_quota(&state, course_id, &auth).await
}

/// GET /files/quota?course_id=... — shorthand for the Canvas context route.
async fn get_files_quota(
    State(state): State<AppState>,
    Query(params): Query<QuotaQuery>,
    auth: AuthUser,
) -> Result<Json<QuotaResponse>, ApiError> {
    let course_id = params
        .course_id
        .ok_or_else(|| ApiError::BadRequest("course_id is required".into()))?;
    files_quota(&state, course_id, &auth).await
}

/// Canvas Attachment.get_quota: quota = course.storage_quota (50 MB default),
/// quota_used = sum(max(size, 512)) over active files.
async fn files_quota(
    state: &AppState,
    course_id: i64,
    auth: &AuthUser,
) -> Result<Json<QuotaResponse>, ApiError> {
    let storage_quota: Option<Option<i64>> =
        sqlx::query_scalar("SELECT storage_quota FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    let storage_quota = storage_quota.ok_or(ApiError::NotFound)?;

    // Canvas api_quota requires the file create/update/delete rights.
    let allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let (used,): (i64,) = sqlx::query_as(
        r#"
        SELECT COALESCE(SUM(GREATEST(size, $2)), 0)::bigint
        FROM files
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(course_id)
    .bind(MINIMUM_SIZE_FOR_QUOTA)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(QuotaResponse {
        quota: effective_quota(storage_quota),
        quota_used: used,
    }))
}

// ─── folder DB helpers ─────────────────────────────────────────────────────

/// Raw folders row with file/subfolder counts (column order matches queries).
#[derive(Debug, sqlx::FromRow)]
struct FolderRow {
    id: i64,
    name: String,
    full_name: String,
    context_type: String,
    context_id: i64,
    parent_folder_id: Option<i64>,
    workflow_state: String,
    position: i32,
    locked: bool,
    lock_at: Option<DateTime<Utc>>,
    unlock_at: Option<DateTime<Utc>>,
    for_submissions: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    files_count: i64,
    folders_count: i64,
}

const FOLDER_SELECT: &str = r#"
SELECT f.id, f.name, f.full_name, f.context_type, f.context_id,
       f.parent_folder_id, f.workflow_state, f.position, f.locked,
       f.lock_at, f.unlock_at, f.for_submissions,
       f.created_at, f.updated_at,
       (SELECT count(*) FROM files fi WHERE fi.folder_id = f.id) AS files_count,
       (SELECT count(*) FROM folders sf
         WHERE sf.parent_folder_id = f.id AND sf.workflow_state <> 'deleted') AS folders_count
FROM folders f
"#;

/// Canvas folder_json counts for viewers without file-management rights:
/// hidden/locked children are excluded from files_count/folders_count.
const FOLDER_SELECT_VISIBLE: &str = r#"
SELECT f.id, f.name, f.full_name, f.context_type, f.context_id,
       f.parent_folder_id, f.workflow_state, f.position, f.locked,
       f.lock_at, f.unlock_at, f.for_submissions,
       f.created_at, f.updated_at,
       (SELECT count(*) FROM files fi
         WHERE fi.folder_id = f.id AND fi.hidden = FALSE AND fi.locked = FALSE
           AND (fi.lock_at IS NULL OR fi.lock_at > now())) AS files_count,
       (SELECT count(*) FROM folders sf
         WHERE sf.parent_folder_id = f.id AND sf.workflow_state = 'visible'
           AND sf.locked = FALSE) AS folders_count
FROM folders f
"#;

struct NewFolder {
    name: String,
    full_name: String,
    course_id: i64,
    parent_id: i64,
    workflow_state: String,
    position: i32,
    locked: bool,
    lock_at: Option<DateTime<Utc>>,
    unlock_at: Option<DateTime<Utc>>,
    for_submissions: bool,
}

async fn fetch_folder(db: &PgPool, folder_id: i64) -> Result<Option<FolderRow>, ApiError> {
    let sql = format!("{FOLDER_SELECT} WHERE f.id = $1 AND f.workflow_state <> 'deleted'");
    Ok(sqlx::query_as::<_, FolderRow>(&sql)
        .bind(folder_id)
        .fetch_optional(db)
        .await?)
}

async fn fetch_root_folder(db: &PgPool, course_id: i64) -> Result<Option<FolderRow>, ApiError> {
    let sql = format!(
        "{FOLDER_SELECT} WHERE f.context_type = 'Course' AND f.context_id = $1 \
         AND f.parent_folder_id IS NULL AND f.workflow_state <> 'deleted'"
    );
    Ok(sqlx::query_as::<_, FolderRow>(&sql)
        .bind(course_id)
        .fetch_optional(db)
        .await?)
}

/// Canvas Folder.root_folders: lazily create the context's root folder.
async fn ensure_root_folder(db: &PgPool, course_id: i64) -> Result<FolderRow, ApiError> {
    if let Some(root) = fetch_root_folder(db, course_id).await? {
        return Ok(root);
    }
    let name = root_folder_name_for_context("Course");
    sqlx::query(
        r#"
        INSERT INTO folders (name, full_name, context_type, context_id, workflow_state, position)
        VALUES ($1, $1, 'Course', $2, 'visible', 1)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(name)
    .bind(course_id)
    .execute(db)
    .await?;
    fetch_root_folder(db, course_id)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Canvas Folder.unfiled_folder: default parent for folders created at the
/// context level without an explicit parent.
async fn ensure_unfiled_folder(db: &PgPool, root: FolderRow) -> Result<FolderRow, ApiError> {
    if let Some(folder) = fetch_child_by_name(db, root.id, UNFILED_FOLDER_NAME).await? {
        return Ok(folder);
    }
    let position = next_position(db, root.id).await?;
    let new_folder = NewFolder {
        name: UNFILED_FOLDER_NAME.to_string(),
        full_name: join_full_name(&root.full_name, UNFILED_FOLDER_NAME),
        course_id: root.context_id,
        parent_id: root.id,
        workflow_state: "visible".to_string(),
        position,
        locked: false,
        lock_at: None,
        unlock_at: None,
        for_submissions: false,
    };
    insert_folder(db, new_folder).await
}

async fn insert_folder(db: &PgPool, folder: NewFolder) -> Result<FolderRow, ApiError> {
    let row = sqlx::query_as::<_, FolderRow>(
        r#"
        INSERT INTO folders
            (name, full_name, context_type, context_id, parent_folder_id,
             workflow_state, position, locked, lock_at, unlock_at, for_submissions)
        VALUES ($1, $2, 'Course', $3, $4, $5, $6, $7, $8, $9, $10)
        RETURNING id, name, full_name, context_type, context_id, parent_folder_id,
                  workflow_state, position, locked, lock_at, unlock_at, for_submissions,
                  created_at, updated_at,
                  0::bigint AS files_count, 0::bigint AS folders_count
        "#,
    )
    .bind(&folder.name)
    .bind(&folder.full_name)
    .bind(folder.course_id)
    .bind(folder.parent_id)
    .bind(&folder.workflow_state)
    .bind(folder.position)
    .bind(folder.locked)
    .bind(folder.lock_at)
    .bind(folder.unlock_at)
    .bind(folder.for_submissions)
    .fetch_one(db)
    .await?;
    Ok(row)
}

async fn fetch_child_folders(
    db: &PgPool,
    parent_id: i64,
    include_hidden: bool,
    by_position: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<FolderRow>, ApiError> {
    let mut sql =
        format!("{FOLDER_SELECT} WHERE f.parent_folder_id = $1 AND f.workflow_state <> 'deleted'");
    if !include_hidden {
        sql.push_str(
            " AND f.workflow_state <> 'hidden' AND f.locked = FALSE \
             AND (f.lock_at IS NULL OR f.lock_at > now() \
                  OR (f.unlock_at IS NOT NULL AND f.unlock_at < now()))",
        );
    }
    sql.push_str(if by_position {
        " ORDER BY f.position ASC, f.id ASC"
    } else {
        " ORDER BY f.name ASC, f.id ASC"
    });
    sql.push_str(" LIMIT $2 OFFSET $3");
    Ok(sqlx::query_as::<_, FolderRow>(&sql)
        .bind(parent_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await?)
}

async fn fetch_context_folders(
    db: &PgPool,
    course_id: i64,
    include_hidden: bool,
    by_position: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<FolderRow>, ApiError> {
    let mut sql = format!(
        "{FOLDER_SELECT} WHERE f.context_type = 'Course' AND f.context_id = $1 \
         AND f.workflow_state <> 'deleted'"
    );
    if !include_hidden {
        sql.push_str(
            " AND f.workflow_state <> 'hidden' AND f.locked = FALSE \
             AND (f.lock_at IS NULL OR f.lock_at > now() \
                  OR (f.unlock_at IS NOT NULL AND f.unlock_at < now()))",
        );
    }
    sql.push_str(if by_position {
        " ORDER BY f.position ASC, f.id ASC"
    } else {
        " ORDER BY f.name ASC, f.id ASC"
    });
    sql.push_str(" LIMIT $2 OFFSET $3");
    Ok(sqlx::query_as::<_, FolderRow>(&sql)
        .bind(course_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(db)
        .await?)
}

async fn fetch_child_by_name(
    db: &PgPool,
    parent_id: i64,
    name: &str,
) -> Result<Option<FolderRow>, ApiError> {
    let sql = format!(
        "{FOLDER_SELECT} WHERE f.parent_folder_id = $1 AND f.name = $2 \
         AND f.workflow_state <> 'deleted'"
    );
    Ok(sqlx::query_as::<_, FolderRow>(&sql)
        .bind(parent_id)
        .bind(name)
        .fetch_optional(db)
        .await?)
}

async fn fetch_sibling_names(db: &PgPool, parent_id: i64) -> Result<Vec<String>, ApiError> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT name FROM folders WHERE parent_folder_id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(parent_id)
    .fetch_all(db)
    .await?)
}

async fn next_position(db: &PgPool, parent_id: i64) -> Result<i32, ApiError> {
    let (position,): (i32,) = sqlx::query_as(
        "SELECT COALESCE(MAX(position), 0) + 1 FROM folders \
         WHERE parent_folder_id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(parent_id)
    .fetch_one(db)
    .await?;
    Ok(position)
}

/// Canvas Folder.assert_path: resolve/create a slash-separated folder path.
async fn ensure_path(db: &PgPool, course_id: i64, path: &str) -> Result<FolderRow, ApiError> {
    let mut current = ensure_root_folder(db, course_id).await?;
    let components: Vec<String> = path
        .split('/')
        .map(str::trim)
        .filter(|component| !component.is_empty() && *component != ".")
        .map(|component| component.replace('/', "_"))
        .collect();

    for component in components {
        if let Some(child) = fetch_child_by_name(db, current.id, &component).await? {
            current = child;
            continue;
        }
        let position = next_position(db, current.id).await?;
        let new_folder = NewFolder {
            name: component.clone(),
            full_name: join_full_name(&current.full_name, &component),
            course_id,
            parent_id: current.id,
            workflow_state: "visible".to_string(),
            position,
            locked: false,
            lock_at: None,
            unlock_at: None,
            for_submissions: false,
        };
        current = insert_folder(db, new_folder).await?;
    }

    Ok(current)
}

async fn is_descendant_of(
    db: &PgPool,
    ancestor_id: i64,
    candidate_id: i64,
) -> Result<bool, ApiError> {
    let (found,): (bool,) = sqlx::query_as(
        r#"
        WITH RECURSIVE tree AS (
            SELECT id FROM folders WHERE id = $1
            UNION ALL
            SELECT f.id FROM folders f JOIN tree t ON f.parent_folder_id = t.id
        )
        SELECT EXISTS(SELECT 1 FROM tree WHERE id = $2)
        "#,
    )
    .bind(ancestor_id)
    .bind(candidate_id)
    .fetch_one(db)
    .await?;
    Ok(found)
}

/// Recompute full_name for a folder's descendants after a rename/move.
async fn recompute_descendant_full_names(db: &PgPool, folder_id: i64) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        WITH RECURSIVE tree AS (
            SELECT id, full_name FROM folders WHERE id = $1
            UNION ALL
            SELECT f.id, t.full_name || '/' || f.name
            FROM folders f JOIN tree t ON f.parent_folder_id = t.id
        )
        UPDATE folders SET full_name = tree.full_name, updated_at = now()
        FROM tree
        WHERE folders.id = tree.id AND folders.full_name <> tree.full_name
        "#,
    )
    .bind(folder_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Canvas Folder#destroy: soft-delete the folder and all of its descendants.
async fn soft_delete_folder_tree(db: &PgPool, folder_id: i64) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        WITH RECURSIVE tree AS (
            SELECT id FROM folders WHERE id = $1
            UNION ALL
            SELECT f.id FROM folders f JOIN tree t ON f.parent_folder_id = t.id
        )
        UPDATE folders
        SET workflow_state = 'deleted', deleted_at = now(), updated_at = now()
        WHERE id IN (SELECT id FROM tree)
        "#,
    )
    .bind(folder_id)
    .execute(db)
    .await?;
    Ok(())
}

// ─── folders_and_files DB helpers ──────────────────────────────────────────

/// Scope for the folders side of a folders_and_files listing.
struct FolderFilter<'a> {
    course_id: i64,
    parent_id: Option<i64>,
    can_manage: bool,
    search: Option<&'a str>,
}

/// Scope for the files side of a folders_and_files listing.
struct AttachmentFilter<'a> {
    course_id: i64,
    folder_id: Option<i64>,
    can_manage: bool,
    search: Option<&'a str>,
}

/// Canvas folder_index_scope: active folders in the context (or under one
/// parent), with hidden/locked folders excluded for viewers who cannot manage
/// files. Returns the WHERE clause plus the next free bind index.
fn folder_filter_where(filter: &FolderFilter, start_index: u32) -> (String, u32) {
    let mut index = start_index;
    let mut sql =
        String::from(" WHERE f.context_type = 'Course' AND f.workflow_state <> 'deleted'");
    sql.push_str(&format!(" AND f.context_id = ${index}"));
    index += 1;
    if filter.parent_id.is_some() {
        sql.push_str(&format!(" AND f.parent_folder_id = ${index}"));
        index += 1;
    }
    if !filter.can_manage {
        sql.push_str(
            " AND f.workflow_state <> 'hidden' AND f.locked = FALSE \
             AND (f.lock_at IS NULL OR f.lock_at > now() \
                  OR (f.unlock_at IS NOT NULL AND f.unlock_at < now()))",
        );
    }
    if filter.search.is_some() {
        sql.push_str(&format!(" AND f.name ILIKE '%' || ${index} || '%'"));
        index += 1;
    }
    (sql, index)
}

/// Canvas file_index_scope: active files, with hidden files excluded for
/// viewers who cannot manage files, plus an optional display_name search.
fn attachment_filter_where(filter: &AttachmentFilter, start_index: u32) -> (String, u32) {
    let mut index = start_index;
    let mut sql = String::from(" WHERE f.workflow_state <> 'deleted'");
    sql.push_str(&format!(" AND f.course_id = ${index}"));
    index += 1;
    if filter.folder_id.is_some() {
        sql.push_str(&format!(" AND f.folder_id = ${index}"));
        index += 1;
    }
    if !filter.can_manage {
        sql.push_str(" AND f.hidden = FALSE");
    }
    if filter.search.is_some() {
        sql.push_str(&format!(" AND f.display_name ILIKE '%' || ${index} || '%'"));
        index += 1;
    }
    (sql, index)
}

async fn fetch_folders_for_listing(
    db: &PgPool,
    filter: &FolderFilter<'_>,
    sort: SortField,
    desc: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<FolderRow>, ApiError> {
    let (mut where_clause, next) = folder_filter_where(filter, 1);
    where_clause.push_str(&folder_order_clause(sort, desc));
    where_clause.push_str(&format!(" LIMIT ${next} OFFSET ${}", next + 1));
    let select = if filter.can_manage {
        FOLDER_SELECT
    } else {
        FOLDER_SELECT_VISIBLE
    };
    let sql = format!("{select}{where_clause}");

    let mut query = sqlx::query_as::<_, FolderRow>(&sql).bind(filter.course_id);
    if let Some(parent_id) = filter.parent_id {
        query = query.bind(parent_id);
    }
    if let Some(search) = filter.search {
        query = query.bind(search);
    }
    Ok(query.bind(limit).bind(offset).fetch_all(db).await?)
}

async fn count_folders_for_listing(
    db: &PgPool,
    filter: &FolderFilter<'_>,
) -> Result<i64, ApiError> {
    let (where_clause, _) = folder_filter_where(filter, 1);
    let sql = format!("SELECT count(*) FROM folders f{where_clause}");

    let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(filter.course_id);
    if let Some(parent_id) = filter.parent_id {
        query = query.bind(parent_id);
    }
    if let Some(search) = filter.search {
        query = query.bind(search);
    }
    Ok(query.fetch_one(db).await?)
}

async fn fetch_attachments_for_listing(
    db: &PgPool,
    filter: &AttachmentFilter<'_>,
    sort: SortField,
    desc: bool,
    limit: i64,
    offset: i64,
) -> Result<Vec<AttachmentRow>, ApiError> {
    let (mut where_clause, next) = attachment_filter_where(filter, 1);
    where_clause.push_str(&file_order_clause(sort, desc));
    where_clause.push_str(&format!(" LIMIT ${next} OFFSET ${}", next + 1));
    let sql = format!("{ATTACHMENT_SELECT}{where_clause}");

    let mut query = sqlx::query_as::<_, AttachmentRow>(&sql).bind(filter.course_id);
    if let Some(folder_id) = filter.folder_id {
        query = query.bind(folder_id);
    }
    if let Some(search) = filter.search {
        query = query.bind(search);
    }
    Ok(query.bind(limit).bind(offset).fetch_all(db).await?)
}

async fn count_attachments_for_listing(
    db: &PgPool,
    filter: &AttachmentFilter<'_>,
) -> Result<i64, ApiError> {
    let (where_clause, _) = attachment_filter_where(filter, 1);
    let sql = format!("SELECT count(*) FROM files f{where_clause}");

    let mut query = sqlx::query_scalar::<_, i64>(&sql).bind(filter.course_id);
    if let Some(folder_id) = filter.folder_id {
        query = query.bind(folder_id);
    }
    if let Some(search) = filter.search {
        query = query.bind(search);
    }
    Ok(query.fetch_one(db).await?)
}

// ─── row type ─────────────────────────────────────────────────────────────

/// Raw files row as selected from the DB (column order matches every SELECT above).
#[derive(Debug, sqlx::FromRow)]
struct FileRow {
    id: i64,
    course_id: i64,
    folder: String,
    folder_id: Option<i64>,
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
            folder_id: r.folder_id,
            filename: r.filename,
            display_name: r.display_name,
            content_type: r.content_type,
            size: r.size,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Raw files row for the attachment_json listing (column order must match
/// ATTACHMENT_SELECT).
#[derive(Debug, sqlx::FromRow)]
struct AttachmentRow {
    id: i64,
    course_id: i64,
    folder_id: Option<i64>,
    folder: String,
    filename: String,
    display_name: String,
    content_type: String,
    size: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    uuid: Option<String>,
    hidden: bool,
    locked: bool,
    lock_at: Option<DateTime<Utc>>,
    unlock_at: Option<DateTime<Utc>>,
    modified_at: DateTime<Utc>,
    category: Option<String>,
    visibility_level: Option<String>,
}

const ATTACHMENT_SELECT: &str = r#"
SELECT f.id, f.course_id, f.folder_id, f.folder, f.filename, f.display_name,
       f.content_type, f.size, f.created_at, f.updated_at,
       f.uuid, f.hidden, f.locked, f.lock_at, f.unlock_at,
       COALESCE(f.modified_at, f.updated_at) AS modified_at,
       f.category, f.visibility_level
FROM files f
"#;

impl AttachmentResponse {
    fn from_row(row: AttachmentRow, can_manage: bool) -> Self {
        let now = Utc::now();
        let currently_locked = row.locked
            || row.lock_at.map(|at| at <= now).unwrap_or(false)
            || row.unlock_at.map(|at| at > now).unwrap_or(false)
            || row.hidden;
        let mime_class = mime_class(&row.content_type).to_string();
        AttachmentResponse {
            id: row.id,
            uuid: row.uuid,
            folder_id: row.folder_id,
            display_name: row.display_name,
            filename: row.filename,
            upload_status: "success".to_string(),
            content_type: row.content_type.clone(),
            content_type_compat: row.content_type,
            // Metadata-only: no binary storage, so no download/thumbnail URL.
            url: String::new(),
            size: row.size,
            created_at: row.created_at,
            updated_at: row.updated_at,
            unlock_at: row.unlock_at,
            locked: row.locked,
            hidden: row.hidden,
            lock_at: row.lock_at,
            hidden_for_user: !can_manage && row.hidden,
            locked_for_user: !can_manage && currently_locked,
            thumbnail_url: String::new(),
            modified_at: row.modified_at,
            mime_class,
            media_entry_id: None,
            category: row.category,
            visibility_level: row.visibility_level,
            course_id: row.course_id,
            folder: row.folder,
        }
    }
}

impl FolderResponse {
    fn from_row(row: FolderRow, can_manage: bool) -> Self {
        let now = Utc::now();
        let hidden = row.workflow_state == "hidden";
        let currently_locked = row.locked
            || row.lock_at.map(|at| at <= now).unwrap_or(false)
            || row.unlock_at.map(|at| at > now).unwrap_or(false)
            || hidden;

        FolderResponse {
            id: row.id,
            name: row.name,
            full_name: row.full_name,
            position: row.position,
            parent_folder_id: row.parent_folder_id,
            context_type: row.context_type,
            context_id: row.context_id,
            unlock_at: row.unlock_at,
            lock_at: row.lock_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            category: None,
            locked: row.locked,
            all_url: format!("/api/v1/folders/{}/all", row.id),
            folders_url: format!("/api/v1/folders/{}/folders", row.id),
            files_url: format!("/api/v1/folders/{}/files", row.id),
            files_count: row.files_count,
            folders_count: row.folders_count,
            hidden,
            locked_for_user: !can_manage && currently_locked,
            hidden_for_user: !can_manage && hidden,
            for_submissions: row.for_submissions,
            can_upload: can_manage,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_builds() {
        // Verify router builds without panicking.
        let _ = super::router();
    }

    #[test]
    fn normalize_folder_name_matches_canvas() {
        // Canvas infer_full_name: blank -> "New Folder", "/" -> "_".
        assert_eq!(normalize_folder_name(None), "New Folder");
        assert_eq!(normalize_folder_name(Some("   ")), "New Folder");
        assert_eq!(normalize_folder_name(Some(" week/1 ")), "week_1");
    }

    #[test]
    fn dedupe_folder_name_uses_first_free_suffix() {
        assert_eq!(dedupe_folder_name("notes", &[]), "notes");
        assert_eq!(
            dedupe_folder_name("notes", &["notes".to_string()]),
            "notes 2"
        );
        assert_eq!(
            dedupe_folder_name("notes", &["notes".to_string(), "notes 2".to_string()]),
            "notes 3"
        );
        assert_eq!(
            dedupe_folder_name("notes", &["notes".to_string(), "notes 3".to_string()]),
            "notes 2"
        );
    }

    #[test]
    fn folder_json_hides_locked_folders_from_students() {
        // Canvas folder_json: hidden/locked_for_user/can_upload depend on the
        // viewer; students never get can_upload.
        let now = Utc::now();
        let row = FolderRow {
            id: 7,
            name: "locked".to_string(),
            full_name: "course files/locked".to_string(),
            context_type: "Course".to_string(),
            context_id: 42,
            parent_folder_id: Some(1),
            workflow_state: "hidden".to_string(),
            position: 2,
            locked: true,
            lock_at: None,
            unlock_at: None,
            for_submissions: false,
            created_at: now,
            updated_at: now,
            files_count: 1,
            folders_count: 0,
        };

        let student = FolderResponse::from_row(row, false);
        assert!(student.hidden);
        assert!(student.hidden_for_user);
        assert!(student.locked_for_user);
        assert!(!student.can_upload);
        assert_eq!(student.all_url, "/api/v1/folders/7/all");
        assert_eq!(student.folders_url, "/api/v1/folders/7/folders");
        assert_eq!(student.files_url, "/api/v1/folders/7/files");
    }

    #[test]
    fn full_names_and_context_names_match_canvas() {
        assert_eq!(
            join_full_name("course files", "lectures"),
            "course files/lectures"
        );
        assert_eq!(
            join_full_name("course files/lectures", "week 1"),
            "course files/lectures/week 1"
        );
        assert_eq!(root_folder_name_for_context("Course"), "course files");
        assert_eq!(root_folder_name_for_context("User"), "my files");
        assert_eq!(root_folder_name_for_context("Group"), "files");
    }

    #[test]
    fn mime_class_matches_canvas_mapping() {
        // Canvas Attachment.mime_class: strip parameters, map known types,
        // fall back to content-type prefix and finally "file".
        assert_eq!(mime_class("image/png"), "image");
        assert_eq!(mime_class("application/pdf"), "pdf");
        assert_eq!(mime_class("text/plain; charset=utf-8"), "text");
        assert_eq!(mime_class("audio/mpeg"), "audio");
        assert_eq!(mime_class("video/mp4"), "video");
        assert_eq!(mime_class("application/zip"), "zip");
        assert_eq!(mime_class("application/octet-stream"), "file");
    }

    #[test]
    fn combined_page_window_orders_folders_before_files() {
        // Folders first, files after; the page window spans the concatenation
        // (Canvas paginated_folders_and_files_collection).
        assert_eq!(
            combined_page_window(2, 5, 0),
            PageWindow {
                folder_limit: 2,
                folder_offset: 0,
                file_limit: 3,
                file_offset: 0,
            }
        );
        assert_eq!(
            combined_page_window(2, 5, 5),
            PageWindow {
                folder_limit: 0,
                folder_offset: 0,
                file_limit: 5,
                file_offset: 3,
            }
        );
        assert_eq!(
            combined_page_window(10, 4, 2),
            PageWindow {
                folder_limit: 4,
                folder_offset: 2,
                file_limit: 0,
                file_offset: 0,
            }
        );
        assert_eq!(
            combined_page_window(0, 5, 0),
            PageWindow {
                folder_limit: 0,
                folder_offset: 0,
                file_limit: 5,
                file_offset: 0,
            }
        );
    }

    #[test]
    fn quota_defaults_and_minimum_file_size_match_canvas() {
        // Attachment::CONTEXT_DEFAULT_QUOTA is 50 MB; an explicit nil falls
        // back, but Ruby truthiness keeps an explicit 0 at 0.
        assert_eq!(effective_quota(None), 50_000_000);
        assert_eq!(effective_quota(Some(0)), 0);
        assert_eq!(effective_quota(Some(1_048_576)), 1_048_576);
        // Attachment.get_quota counts at least 512 bytes per file.
        assert_eq!(
            quota_used(&[0, 100, 512, 10_000]),
            MINIMUM_SIZE_FOR_QUOTA * 3 + 10_000
        );
    }

    #[test]
    fn attachment_json_uses_canvas_field_names() {
        let now = Utc::now();
        let row = AttachmentRow {
            id: 5,
            course_id: 42,
            folder_id: Some(9),
            folder: "course files/lectures".to_string(),
            filename: "lecture-1.pdf".to_string(),
            display_name: "Lecture 1.pdf".to_string(),
            content_type: "application/pdf".to_string(),
            size: 2048,
            created_at: now,
            updated_at: now,
            uuid: Some("abc123".to_string()),
            hidden: true,
            locked: false,
            lock_at: None,
            unlock_at: None,
            modified_at: now,
            category: Some("teaching_materials".to_string()),
            visibility_level: None,
        };

        let json = serde_json::to_value(AttachmentResponse::from_row(row, false)).unwrap();
        assert_eq!(json["display_name"], "Lecture 1.pdf");
        // Canvas key is hyphenated; the star-lms alias is kept beside it.
        assert_eq!(json["content-type"], "application/pdf");
        assert_eq!(json["content_type"], "application/pdf");
        assert_eq!(json["mime_class"], "pdf");
        assert_eq!(json["folder_id"].as_i64(), Some(9));
        assert_eq!(json["upload_status"], "success");
        assert_eq!(json["hidden_for_user"].as_bool(), Some(true));
        assert_eq!(json["visibility_level"], serde_json::Value::Null);
    }
}
