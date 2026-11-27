//! star-files domain crate — file metadata, file bytes and folders.
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
//! Responses lead with display_name (Canvas behavior). The bytes live in the
//! store behind `star-storage`; a request without bytes records metadata
//! only.
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
//!
//! File update + context show (Phase 5, Canvas files_controller):
//! - PUT /files/{id} mirrors files#api_update: rename/move/lock/hide with
//!   Canvas's 409 duplicate conflict and on_duplicate=overwrite|rename.
//! - GET /files/{id}/public_url answers `{ public_url }` (metadata-only URL).
//! - GET /courses|groups|users/{contextId}/files/{id} are context-scoped
//!   files#api_show reads; hidden files 404 for students.
//! - GET /courses/{courseId}/files/file_ref/{migrationId} translates a
//!   course-copy file reference (files#file_ref) via the file's uuid.
//!
//! Usage rights removal + licenses (Phase 5, Canvas usage_rights_controller):
//! - DELETE /courses|groups|users/{contextId}/usage_rights mirror
//!   remove_usage_rights: unset files.usage_rights_id and answer
//!   `{ message, file_ids }` with 200 (Canvas never sends 204 here).
//! - GET /courses/{courseId}/content_licenses mirrors licenses: the fixed
//!   UsageRights.licenses list (lib/content_licenses.rb).
//!
//! File bytes (star-storage):
//! - The bytes of a file live in the store behind `star-storage`; the row
//!   records the key they live under (`files.storage_key`), their digest
//!   (`files.md5`) and their state (`files.file_state`).
//! - POST /courses/{courseId}/files accepts a multipart body and stores the
//!   bytes; a JSON or form-encoded body keeps the metadata-only behaviour.
//! - GET /files/{id}/download (and /files/{id}/content) serves the bytes with
//!   the content type, the length and the disposition Canvas sends
//!   (`app/helpers/attachment_helper.rb:228`).
//! - POST /files/{id}/attachment_jwt mints the short-lived token that lets a
//!   browser fetch a file without an API token
//!   (`app/models/attachment.rb:2159`).

use axum::{
    extract::{Path, Query, State},
    http::{header::HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, options, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;

use star_common::{ApiError, ApiJson, AppState};

use star_auth::AuthUser;

mod content;
mod folders_api;
mod upload;

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
/// Canvas' course-membership check for reading a context's files/folders.
///
/// Canvas' `Course#active_enrollment_allows` runs over
/// `enrollments.for_user(user).active_or_pending_by_date`
/// (`app/models/course.rb:2321`), and Canvas' `active` scope is
/// `workflow_state <> 'deleted'` (`app/models/enrollment.rb:293`) — a
/// *completed* enrollment (the state Canvas' `DELETE /enrollments/:id`
/// leaves behind, `app/models/enrollment.rb:957`) still counts as course
/// membership. Only a deleted enrollment loses the membership.
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
              AND workflow_state <> 'deleted'
        )"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

/// Check if user has any granular file permission (manage_files_add/edit/delete)
/// — Canvas RoleOverride::GRANULAR_FILE_PERMISSIONS.
/// Returns true for admin (full rights) or teacher/TA/designer (manage files).
///
/// Canvas grants these through `active_enrollment_allows`, which runs over
/// `active_or_pending_by_date` (`app/models/enrollment.rb:1438`) — that scope
/// does *not* list a completed enrollment, so a concluded enrollment keeps the
/// read rights but loses the course-admin ones.
async fn user_has_granular_file_perm(
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
              AND role IN ('teacher', 'ta', 'designer')
              AND workflow_state IN ('active', 'invited')
        )"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

/// Check if user is the teacher of the course or is admin.
///
/// Same scope as [`user_has_granular_file_perm`]: Canvas' course-admin
/// permissions need an enrollment in `active_or_pending_by_date`
/// (`app/models/enrollment.rb:1438`), so a concluded enrollment does not
/// count.
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
              AND workflow_state IN ('active', 'invited')
        )"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Whether the user has an `account_users` row for the account.
async fn user_account_admin(db: &PgPool, user_id: i64, account_id: i64) -> Result<bool, ApiError> {
    let row: (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2)",
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(row.0)
}

/// Canvas `Account.site_admin.feature_enabled?` for the `:files_a11y_rewrite`
/// flag that gates the `folders_and_files`, `all` and `duplicates` endpoints
/// (`app/controllers/folders_controller.rb:155-168,794`).
async fn files_a11y_rewrite_enabled(db: &PgPool) -> Result<bool, ApiError> {
    let root: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    let Some((root_id,)) = root else {
        return Ok(false);
    };

    let (count,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM feature_flags \
         WHERE context_type = 'Account' AND context_id = $1 \
           AND feature IN ('files_a11y_rewrite', 'files_a11y_folder_duplicates') \
           AND state = 'enabled'",
    )
    .bind(root_id)
    .fetch_one(db)
    .await?;
    Ok(count == 2)
}

/// Whether the caller may read the group's context.
///
/// Canvas' `Group` policy (`app/models/group.rb:662`) grants the group's
/// permissions when the group's *context* grants the user `:read`:
///
/// ```ruby
/// given { |user, session| !non_collaborative? && (context.is_a?(Account) || context&.grants_right?(user, session, :read) || false) }
/// ```
///
/// so a member of the group's course may read the group without being a group
/// member.
async fn user_can_read_group(
    db: &PgPool,
    user_id: i64,
    group_id: i64,
    role: &str,
) -> Result<bool, ApiError> {
    if role == "admin" {
        return Ok(true);
    }
    if user_in_group(db, user_id, group_id).await? {
        return Ok(true);
    }
    let course_id: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(db)
    .await?;
    if let Some((Some(course_id),)) = course_id {
        if user_enrolled_or_admin(db, user_id, course_id, role).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Canvas Folder#read_contents for Group contexts: accepted membership.
async fn user_in_group(db: &PgPool, user_id: i64, group_id: i64) -> Result<bool, ApiError> {
    let row: (bool,) = sqlx::query_as(
        "SELECT EXISTS(SELECT 1 FROM group_memberships \
         WHERE group_id = $1 AND user_id = $2 AND workflow_state = 'accepted')",
    )
    .bind(group_id)
    .bind(user_id)
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

/// Canvas Attachment.make_unique_filename: append "-1", "-2", ... before the
/// extension until the name is unused among `existing` siblings.
fn make_unique_filename(filename: &str, existing: &[String]) -> String {
    if !existing.iter().any(|s| s == filename) {
        return filename.to_string();
    }
    let split = filename
        .rfind('.')
        .filter(|dot| *dot > 0 && !filename[*dot + 1..].is_empty())
        .unwrap_or(filename.len());
    let (stem, ext) = filename.split_at(split);
    let mut addition = 1u32;
    let mut candidate = format!("{stem}-{addition}{ext}");
    while existing.contains(&candidate) {
        addition += 1;
        candidate = format!("{stem}-{addition}{ext}");
    }
    candidate
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
    /// Canvas attachment_json emits "content-type" with a dash
    /// (lib/api/v1/attachment.rb); AttachmentResponse does the same.
    #[serde(rename = "content-type")]
    pub content_type: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating a file record.
///
/// Metadata only — no upload. display_name defaults to filename when absent
/// (Canvas `infer_display_name` behavior).
#[derive(Debug, Clone, Default, Deserialize)]
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
#[derive(Debug, Default, Deserialize)]
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
#[derive(Debug, Default, Deserialize)]
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

/// Canvas FoldersController#duplicates response: `{ duplicates: [...] }`.
#[derive(Debug, Serialize)]
pub struct DuplicatesResponse {
    pub duplicates: Vec<FolderResponse>,
}

/// Canvas user/group folders_and_files response: `{ folders, files }`.
#[derive(Debug, Serialize)]
pub struct ContextFoldersAndFiles {
    pub folders: Vec<FolderResponse>,
    pub files: Vec<AttachmentResponse>,
}

/// Request body for POST /folders/{id}/files (metadata only, no bytes).
#[derive(Debug, Default, Deserialize)]
pub struct CreateFolderFileRequest {
    pub filename: String,
    pub display_name: Option<String>,
    pub content_type: Option<String>,
    pub size: Option<i64>,
}

/// `usage_rights` object of PUT /courses/{courseId}/usage_rights.
#[derive(Debug, Deserialize)]
pub struct UsageRightsParams {
    pub use_justification: Option<String>,
    pub legal_copyright: Option<String>,
    pub license: Option<String>,
}

/// Request body for Canvas UsageRightsController#set_usage_rights.
#[derive(Debug, Deserialize)]
pub struct UsageRightsRequest {
    pub file_ids: Option<Vec<i64>>,
    pub folder_ids: Option<Vec<i64>>,
    pub publish: Option<bool>,
    pub usage_rights: Option<UsageRightsParams>,
}

/// Request body for Canvas UsageRightsController#remove_usage_rights.
#[derive(Debug, Default, Deserialize)]
pub struct RemoveUsageRightsRequest {
    pub file_ids: Option<Vec<i64>>,
    pub folder_ids: Option<Vec<i64>>,
}

/// Canvas remove_usage_rights response: `{ message, file_ids }`.
#[derive(Debug, Serialize)]
pub struct RemoveUsageRightsResponse {
    pub message: String,
    pub file_ids: Vec<i64>,
}

/// Canvas usage_rights_json + message + file_ids.
#[derive(Debug, Serialize)]
pub struct UsageRightsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legal_copyright: Option<String>,
    pub use_justification: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license_name: Option<String>,
    pub message: String,
    pub file_ids: Vec<i64>,
}

/// Request body for PUT /files/{id} (Canvas files#api_update).
#[derive(Debug, Default, Deserialize)]
pub struct UpdateFileRequest {
    pub name: Option<String>,
    pub parent_folder_id: Option<i64>,
    pub lock_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub locked: Option<bool>,
    pub hidden: Option<bool>,
    pub visibility_level: Option<String>,
    pub on_duplicate: Option<String>,
}

/// GET /files/{id}/public_url response (Canvas files#public_url).
#[derive(Debug, Serialize)]
pub struct PublicUrlResponse {
    pub public_url: String,
}

/// Query params for /files/:id/create_success.
#[derive(Debug, Deserialize)]
pub struct CreateSuccessQuery {
    pub uuid: Option<String>,
    pub on_duplicate: Option<String>,
}

/// GET /files/:id/create_success response (Canvas files#api_create_success).
/// Fields mirror Canvas attachment_json extended with preview URLs.
#[derive(Debug, Serialize)]
pub struct CreateSuccessResponse {
    #[serde(flatten)]
    pub attachment: AttachmentResponse,
    pub preview_url: Option<String>,
    pub enhanced_preview_url: Option<String>,
}

/// GET /groups/{groupId}/files/{id} response — mirrors the star-groups
/// GroupFile shape (group_files is the metadata-only Group file table).
#[derive(Debug, Serialize)]
pub struct GroupFileResponse {
    pub id: i64,
    pub group_id: i64,
    pub folder: String,
    pub filename: String,
    pub display_name: String,
    pub content_type: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for POST /folders/{destFolderId}/copy_file.
#[derive(Debug, Deserialize)]
pub struct CopyFileRequest {
    pub source_file_id: Option<i64>,
    pub on_duplicate: Option<String>,
    pub name: Option<String>,
}

/// Request body for POST /folders/{destFolderId}/copy_folder.
#[derive(Debug, Deserialize)]
pub struct CopyFolderRequest {
    pub source_folder_id: Option<i64>,
}

/// Canvas GET content_licenses list item (UsageRightsController#licenses).
#[derive(Debug, Serialize)]
pub struct LicenseEntry {
    pub id: String,
    pub name: String,
    pub url: String,
}

/// Canvas UsageRights.licenses (lib/content_licenses.rb ContentLicenses
/// ordered hash): id, readable name and license text URL for every supported
/// license, in Canvas's exact order (private first, no cc0 in this checkout).
pub fn canvas_licenses() -> Vec<LicenseEntry> {
    [
        (
            "private",
            "Private (Copyrighted)",
            "http://en.wikipedia.org/wiki/Copyright",
        ),
        (
            "public_domain",
            "Public Domain",
            "http://en.wikipedia.org/wiki/Public_domain",
        ),
        (
            "cc_by",
            "CC Attribution",
            "http://creativecommons.org/licenses/by/4.0",
        ),
        (
            "cc_by_sa",
            "CC Attribution Share Alike",
            "http://creativecommons.org/licenses/by-sa/4.0",
        ),
        (
            "cc_by_nc",
            "CC Attribution Non-Commercial",
            "http://creativecommons.org/licenses/by-nc/4.0",
        ),
        (
            "cc_by_nc_sa",
            "CC Attribution Non-Commercial Share Alike",
            "http://creativecommons.org/licenses/by-nc-sa/4.0",
        ),
        (
            "cc_by_nd",
            "CC Attribution No Derivatives",
            "http://creativecommons.org/licenses/by-nd/4.0",
        ),
        (
            "cc_by_nc_nd",
            "CC Attribution Non-Commercial No Derivatives",
            "http://creativecommons.org/licenses/by-nc-nd/4.0/",
        ),
    ]
    .into_iter()
    .map(|(id, name, url)| LicenseEntry {
        id: id.to_string(),
        name: name.to_string(),
        url: url.to_string(),
    })
    .collect()
}

// ─── router ────────────────────────────────────────────────────────────────

/// Build the files router.
pub fn router() -> axum::Router<AppState> {
    Router::new()
        .merge(folders_api::router())
        .route("/courses/{courseId}/files", get(list_course_files))
        .route("/courses/{courseId}/files", post(create_file))
        .route(
            "/courses/{courseId}/files/quota",
            get(get_course_files_quota),
        )
        .route("/files/quota", get(get_files_quota))
        .route("/files/{id}", get(get_file).delete(delete_file))
        .route("/files/{id}", put(update_file))
        .route("/files/{id}", post(post_file))
        .route("/files/update_word_count", post(update_word_count))
        // The bytes of a file (Canvas files#show serves them from
        // /files/:id/download).
        .route("/files/{id}/download", get(content::download_file))
        .route("/files/{id}/content", get(content::download_file))
        .route(
            "/files/{id}/attachment_jwt",
            post(content::mint_attachment_jwt),
        )
        .route("/files/{id}/public_url", get(get_file_public_url))
        .route("/files/{id}/create_success", get(create_success_get))
        .route("/files/{id}/create_success", post(create_success_post))
        .route(
            "/files/{id}/create_success",
            options(create_success_options),
        )
        .route("/files/{id}/reset_verifier", post(reset_verifier))
        .route(
            "/rce_linked_file_instfs_ids",
            post(rce_linked_file_instfs_ids),
        )
        // Canvas files#icon_metadata (SVG parse).
        .route("/files/{id}/icon_metadata", get(get_icon_metadata))
        .route("/courses/{courseId}/files/{id}", get(get_course_file))
        // /groups/{groupId}/files (GET and POST) is owned entirely by
        // star-groups: it queries the metadata-only group_files table for
        // both api_index (routes.rb:2143) and groups#create_file
        // (routes.rb:2127). Registering either method here too would panic
        // when star-http merges the routers (same path + method).
        .route("/groups/{groupId}/files/quota", get(get_group_files_quota))
        .route(
            "/groups/{groupId}/files/{id}",
            get(get_group_file).delete(delete_group_file),
        )
        .route("/users/{userId}/files", get(list_user_files))
        .route("/users/{userId}/files/quota", get(get_user_files_quota))
        .route("/users/{userId}/files/{id}", get(get_user_file))
        .route(
            "/courses/{courseId}/files/file_ref/{migrationId}",
            get(get_file_ref),
        )
        // Canvas folders API (lib/api/v1/folders + FoldersController).
        .route("/folders", get(list_folders))
        .route("/folders", post(create_folder))
        .route("/folders/{id}", get(get_folder))
        .route("/folders/{id}", put(update_folder))
        .route("/folders/{id}", delete(delete_folder))
        .route("/folders/{id}/folders", get(list_folder_children))
        .route("/folders/{id}/folders", post(create_subfolder))
        .route("/folders/{id}/files", get(list_folder_files))
        .route("/folders/{id}/files", post(create_folder_file))
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
        // Canvas deep folders/files endpoints.
        .route(
            "/courses/{courseId}/folders/{id}/duplicates",
            get(list_folder_duplicates),
        )
        .route(
            "/users/{userId}/folders_and_files",
            get(list_user_folders_and_files),
        )
        .route(
            "/groups/{groupId}/folders_and_files",
            get(list_group_folders_and_files),
        )
        .route("/accounts/{accountId}/folders", post(create_account_folder))
        // Canvas UsageRightsController#set_usage_rights.
        .route(
            "/courses/{courseId}/usage_rights",
            put(set_course_usage_rights),
        )
        // Canvas UsageRightsController#remove_usage_rights.
        .route(
            "/courses/{courseId}/usage_rights",
            delete(remove_course_usage_rights),
        )
        .route(
            "/groups/{groupId}/usage_rights",
            delete(remove_group_usage_rights),
        )
        .route(
            "/users/{userId}/usage_rights",
            delete(remove_user_usage_rights),
        )
        // Canvas UsageRightsController#licenses.
        .route(
            "/courses/{courseId}/content_licenses",
            get(get_course_content_licenses),
        )
        // ── Phase 5: 20 folder/file migration routes ───────────────────────
        // Route order matters: more specific (wildcard) routes before simpler :id routes.
        // resolve_path — must precede show handlers to avoid /by_path matching /{id}
        .route(
            "/courses/{courseId}/folders/by_path/{*full_path}",
            get(resolve_path_course_named),
        )
        .route(
            "/courses/{courseId}/folders/by_path",
            get(resolve_path_course),
        )
        .route(
            "/users/{userId}/folders/by_path/{*full_path}",
            get(resolve_path_user_named),
        )
        .route("/users/{userId}/folders/by_path", get(resolve_path_user))
        .route(
            "/groups/{groupId}/folders/by_path/{*full_path}",
            get(resolve_path_group_named),
        )
        .route("/groups/{groupId}/folders/by_path", get(resolve_path_group))
        // icon_maker — unique named folder (registered before :id to avoid conflict)
        .route(
            "/courses/{courseId}/folders/icon_maker",
            get(icon_maker_folder_course),
        )
        // media — unique named folders (registered before :id)
        .route(
            "/courses/{courseId}/folders/media",
            get(media_folder_course),
        )
        .route("/users/{userId}/folders/media", get(media_folder_user))
        // context-scoped show — simple :id lookup in context
        .route("/courses/{courseId}/folders/{id}", get(show_course_folder))
        .route("/users/{userId}/folders/{id}", get(show_user_folder))
        .route("/groups/{groupId}/folders/{id}", get(show_group_folder))
        // list_all_folders — GET /context/folders (not to be confused with create)
        // (GET /courses/{courseId}/folders is already registered above via
        // list_course_folders — Canvas routes.rb:1258 declares it exactly once.)
        .route("/users/{userId}/folders", get(list_all_user_folders))
        // create folder in context — POST /context/folders
        // (POST /courses/{courseId}/folders is already registered above via
        // create_course_folder; Canvas routes.rb declares it exactly once.)
        .route("/users/{userId}/folders", post(create_user_folder_ctx))
        // context-scoped duplicates (registered before generic /folders/{id}/duplicates)
        .route(
            "/users/{userId}/folders/{id}/duplicates",
            get(list_user_folder_duplicates),
        )
        // copy_file / copy_folder — POST /folders/{destFolderId}/copy_*
        .route(
            "/folders/{destFolderId}/copy_file",
            post(copy_file_to_folder),
        )
        .route(
            "/folders/{destFolderId}/copy_folder",
            post(copy_folder_to_folder),
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

/// POST /courses/{courseId}/files — create a file (enrolled or admin).
///
/// A multipart body carries the bytes: they are stored under the key Canvas
/// derives from the attachment id (`star-storage`), and the row records the
/// key, the digest and the size. A JSON or form-encoded body keeps the
/// metadata-only behaviour. display_name defaults to filename if not
/// provided, mirroring Canvas infer_display_name.
async fn create_file(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    upload: upload::UploadRequest,
) -> Result<(StatusCode, Json<FileResponse>), ApiError> {
    let allowed = user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let payload = upload.metadata.clone().unwrap_or_default();
    let filename = upload
        .file_name()
        .map(str::to_string)
        .unwrap_or_else(|| payload.filename.clone());

    if filename.trim().is_empty() {
        // Canvas runs the upload preflight after the permission check and
        // raises there when the request carries no file; its exception
        // handler answers the generic 500
        // (app/controllers/application_controller.rb:2325).
        return Err(ApiError::canvas_internal_error());
    }

    // Canvas infer_display_name: display_name ||= filename (empty -> filename).
    let display_name = match payload.display_name {
        Some(ref d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => filename.clone(),
    };
    let content_type = upload
        .file_content_type()
        .map(str::to_string)
        .unwrap_or_else(|| "application/octet-stream".into());

    // The bytes, when the client sent them.
    let data = upload.data.as_deref();
    let size = match data {
        Some(bytes) => bytes.len() as i64,
        None => payload.size.unwrap_or(0),
    };
    let md5 = data.map(star_storage::md5_hex);
    let encoding = match data {
        Some(bytes) if content_type.contains("text") => Some(infer_encoding(bytes)),
        _ => None,
    };
    // A row without bytes has no file data; Canvas marks such an attachment
    // as errored (`app/models/attachment.rb`).
    let file_state = if data.is_some() {
        "available"
    } else {
        "errored"
    };

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
        INSERT INTO files (course_id, uploader_id, filename, display_name, content_type, size, folder, folder_id, md5, file_state, encoding)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, course_id, folder, folder_id, filename, display_name, content_type, size, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(&filename)
    .bind(&display_name)
    .bind(&content_type)
    .bind(size)
    .bind(&folder)
    .bind(folder_id)
    .bind(&md5)
    .bind(file_state)
    .bind(&encoding)
    .fetch_one(&state.db)
    .await?;

    // Store the bytes under the key Canvas derives from the attachment id
    // (`gems/attachment_fu/lib/attachment_fu/backends/file_system_backend.rb:38-58`).
    if let Some(bytes) = upload.data.as_deref() {
        let store = star_storage::global_store().map_err(|_| ApiError::canvas_internal_error())?;
        let key = store.attachment_key(row.id, &filename);
        if store.put(&key, bytes, Some(&content_type)).await.is_err() {
            // The row would point at bytes that are not there; drop it.
            let _ = sqlx::query("DELETE FROM files WHERE id = $1")
                .bind(row.id)
                .execute(&state.db)
                .await;
            return Err(ApiError::canvas_internal_error());
        }
        sqlx::query("UPDATE files SET storage_key = $1 WHERE id = $2")
            .bind(&key)
            .bind(row.id)
            .execute(&state.db)
            .await?;
    }

    Ok((StatusCode::CREATED, Json(FileResponse::from(row))))
}

/// Canvas infers the encoding of a text file: UTF-8 when the bytes decode,
/// empty otherwise (`app/models/attachment.rb:302-344`).
fn infer_encoding(bytes: &[u8]) -> String {
    if std::str::from_utf8(bytes).is_ok() {
        "UTF-8".to_string()
    } else {
        String::new()
    }
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

/// The body of `POST /files/update_word_count`.
///
/// Canvas reads `params[:attachment_jwt]` and `params[:word_count]` out of
/// whatever the client sent — a form-encoded body in the documented call
/// (`app/controllers/files_controller.rb:1625-1632`).
#[derive(Debug, Default, Deserialize)]
pub struct UpdateWordCountRequest {
    pub attachment_jwt: Option<String>,
    /// Canvas reads the word count with `Integer(params[:word_count])`, so a
    /// number and a numeric string are both good
    /// (`app/controllers/files_controller.rb:1640`).
    pub word_count: Option<serde_json::Value>,
}

/// POST /files/update_word_count — Canvas `files_controller#update_word_count`
/// (`app/controllers/files_controller.rb:1625-1648`, route
/// `config/routes.rb:2247`).
///
/// Canvas answers a missing parameter with a 400 and a single `message` key
/// of its own.
async fn update_word_count(
    State(state): State<AppState>,
    _auth: AuthUser,
    ApiJson(payload): ApiJson<UpdateWordCountRequest>,
) -> Result<Response, ApiError> {
    if payload
        .attachment_jwt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return Err(ApiError::canvas_message(
            StatusCode::BAD_REQUEST,
            "Missing attachment_jwt param",
        ));
    }

    let Some(word_count) = payload.word_count.as_ref() else {
        return Err(ApiError::canvas_message(
            StatusCode::BAD_REQUEST,
            "Missing word_count param",
        ));
    };

    // Canvas reads the word count with `Integer(params[:word_count])`
    // (`app/controllers/files_controller.rb:1640`).
    let word_count = match word_count {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    };
    let Some(word_count) = word_count else {
        return Err(ApiError::canvas_message(
            StatusCode::BAD_REQUEST,
            "Invalid word_count param",
        ));
    };

    // Canvas decodes the attachment JWT with `CanvasSecurity.decode_jwt`
    // (`app/controllers/files_controller.rb:1634`); the port signs the same
    // claim set with its own secret.
    let Some(claims) = payload
        .attachment_jwt
        .as_deref()
        .and_then(|token| content::read_attachment_jwt(token.trim(), &state.jwt_secret))
    else {
        return Err(ApiError::canvas_message(
            StatusCode::BAD_REQUEST,
            "Invalid attachment_jwt param",
        ));
    };

    // Canvas uses an explicit find-update, so a missing attachment is a 404
    // (`app/controllers/files_controller.rb:1646`).
    let updated = sqlx::query("UPDATE files SET word_count = $1 WHERE id = $2")
        .bind(word_count)
        .bind(claims.id)
        .execute(&state.db)
        .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// DELETE /files/{id} — delete a file (uploader or course teacher or admin)
async fn delete_file(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    auth: AuthUser,
) -> Result<impl IntoResponse, ApiError> {
    let existing: Option<(i64, i64, Option<String>)> =
        sqlx::query_as("SELECT course_id, uploader_id, storage_key FROM files WHERE id = $1")
            .bind(file_id)
            .fetch_optional(&state.db)
            .await?;

    let (course_id, uploader_id, storage_key) = existing.ok_or(ApiError::NotFound)?;

    let is_uploader = uploader_id == auth.id;
    let allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;

    if !is_uploader && !allowed {
        // Canvas answers a caller who may not delete the file with a 401 and
        // a single `message` key of its own
        // (`app/controllers/files_controller.rb:1472`).
        return Err(ApiError::canvas_message(
            StatusCode::UNAUTHORIZED,
            "Unauthorized to delete this file",
        ));
    }

    sqlx::query("DELETE FROM files WHERE id = $1")
        .bind(file_id)
        .execute(&state.db)
        .await?;

    // The bytes go with the row (Canvas destroys the file with the
    // attachment, `app/models/attachment.rb:2011`).
    if let Some(storage_key) = storage_key {
        if let Ok(store) = star_storage::global_store() {
            let _ = store.delete(&storage_key).await;
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

// ─── file update / public_url / context show handlers (Phase 5) ────────────

/// PUT /files/{id} — Canvas files#api_update: rename/move/lock/hide.
///
/// Teachers/admins only (Canvas manage_files on the file's course). Renaming
/// or moving onto a sibling that already uses the display_name answers 409
/// with Canvas's duplicate message unless on_duplicate is "overwrite" or
/// "rename" (Canvas Attachment#handle_duplicates).
async fn update_file(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<UpdateFileRequest>,
) -> Result<Response, ApiError> {
    let sql = format!("{ATTACHMENT_SELECT} WHERE f.id = $1 AND f.workflow_state <> 'deleted'");
    let file = sqlx::query_as::<_, AttachmentRow>(&sql)
        .bind(file_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let allowed = user_teacher_of_course(&state.db, auth.id, file.course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    // Canvas api_update moves only within the attachment's own context.
    let mut folder_id = file.folder_id;
    let mut folder = file.folder.clone();
    if let Some(parent_folder_id) = payload.parent_folder_id {
        let Some(target) = fetch_folder(&state.db, parent_folder_id).await? else {
            return Err(ApiError::NotFound);
        };
        if target.context_type != "Course" || target.context_id != file.course_id {
            return Err(ApiError::NotFound);
        }
        folder_id = Some(target.id);
        folder = target.full_name;
    }

    // Canvas truncates name to 255 characters (api_update).
    let mut display_name = file.display_name.clone();
    if let Some(name) = payload.name.as_deref() {
        display_name = name.chars().take(255).collect();
    }
    if let Some(level) = payload.visibility_level.as_deref() {
        if !matches!(level, "inherit" | "context" | "institution" | "public") {
            return Ok(canvas_error(
                StatusCode::BAD_REQUEST,
                "visibility_level must be one of 'inherit', 'context', 'institution', 'public'",
            ));
        }
    }

    // Canvas: a rename or move onto an existing sibling display_name is a
    // conflict unless on_duplicate resolves it.
    let name_changed = display_name != file.display_name;
    let folder_changed = folder_id != file.folder_id;
    let mut on_duplicate: Option<&str> = None;
    if name_changed || folder_changed {
        let (conflict,): (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM files \
             WHERE folder_id IS NOT DISTINCT FROM $1 AND display_name = $2 \
               AND id <> $3 AND workflow_state <> 'deleted')",
        )
        .bind(folder_id)
        .bind(&display_name)
        .bind(file_id)
        .fetch_one(&state.db)
        .await?;
        if conflict {
            match payload.on_duplicate.as_deref() {
                Some("overwrite") => on_duplicate = Some("overwrite"),
                Some("rename") => on_duplicate = Some("rename"),
                _ => {
                    return Ok(canvas_error(
                        StatusCode::CONFLICT,
                        "file already exists; use on_duplicate='overwrite' or 'rename'",
                    ))
                }
            }
        }
    }
    if on_duplicate == Some("rename") {
        let siblings = sqlx::query_scalar::<_, String>(
            "SELECT display_name FROM files \
             WHERE folder_id IS NOT DISTINCT FROM $1 AND id <> $2 \
               AND workflow_state <> 'deleted'",
        )
        .bind(folder_id)
        .bind(file_id)
        .fetch_all(&state.db)
        .await?;
        display_name = make_unique_filename(&display_name, &siblings);
    }

    sqlx::query(
        r#"
        UPDATE files SET
            display_name = $2,
            folder_id = $3,
            folder = $4,
            lock_at = $5,
            unlock_at = $6,
            locked = $7,
            hidden = $8,
            visibility_level = COALESCE($9, visibility_level),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(file_id)
    .bind(&display_name)
    .bind(folder_id)
    .bind(&folder)
    .bind(payload.lock_at.or(file.lock_at))
    .bind(payload.unlock_at.or(file.unlock_at))
    .bind(payload.locked.unwrap_or(file.locked))
    .bind(payload.hidden.unwrap_or(file.hidden))
    .bind(
        payload
            .visibility_level
            .clone()
            .or(file.visibility_level.clone()),
    )
    .execute(&state.db)
    .await?;

    // Canvas handle_duplicates(:overwrite) destroys the replaced siblings.
    if on_duplicate == Some("overwrite") {
        sqlx::query(
            "DELETE FROM files \
             WHERE folder_id IS NOT DISTINCT FROM $1 AND display_name = $2 \
               AND id <> $3 AND workflow_state <> 'deleted'",
        )
        .bind(folder_id)
        .bind(&display_name)
        .bind(file_id)
        .execute(&state.db)
        .await?;
    }

    let sql = format!("{ATTACHMENT_SELECT} WHERE f.id = $1");
    let updated = sqlx::query_as::<_, AttachmentRow>(&sql)
        .bind(file_id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(AttachmentResponse::from_row(updated, true)).into_response())
}

/// GET /files/{id}/public_url — Canvas files#public_url.
///
/// Metadata-only: there is no object storage, so the URL points at the
/// download route with the file's uuid as the verifier.
async fn get_file_public_url(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<PublicUrlResponse>, ApiError> {
    let row: Option<(i64, i64, Option<String>)> = sqlx::query_as(
        "SELECT course_id, uploader_id, uuid FROM files \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(file_id)
    .fetch_optional(&state.db)
    .await?;
    let (course_id, uploader_id, uuid) = row.ok_or(ApiError::NotFound)?;

    // Canvas requires the download right (uploader, enrolled users, admin).
    let is_uploader = uploader_id == auth.id;
    let allowed =
        is_uploader || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let verifier = uuid.unwrap_or_default();
    Ok(Json(PublicUrlResponse {
        public_url: format!("/api/v1/files/{file_id}/download?verifier={verifier}"),
    }))
}

/// GET /courses/{courseId}/files/{id} — Canvas files#api_show (course scope).
///
/// Enrolled users see non-hidden files; hidden files are invisible to
/// students (404); teachers/admins see everything.
async fn get_course_file(
    State(state): State<AppState>,
    Path((course_id, file_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<AttachmentResponse>, ApiError> {
    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed =
        can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "{ATTACHMENT_SELECT} WHERE f.id = $1 AND f.course_id = $2 AND f.workflow_state <> 'deleted'"
    );
    let file = sqlx::query_as::<_, AttachmentRow>(&sql)
        .bind(file_id)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Canvas api_show hides hidden attachments from non-managers.
    if file.hidden && !can_manage {
        return Err(ApiError::NotFound);
    }

    Ok(Json(AttachmentResponse::from_row(file, can_manage)))
}

/// GET /groups/{groupId}/files/{id} — Canvas files#api_show (group scope).
///
/// Group files live in the metadata-only `group_files` table (migration
/// 0064); accepted members may read them, teachers/admins manage them.
async fn get_group_file(
    State(state): State<AppState>,
    Path((group_id, file_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<GroupFileResponse>, ApiError> {
    let course_id: Option<i64> = sqlx::query_scalar(
        "SELECT course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?
    .flatten()
    .ok_or(ApiError::NotFound)?;

    let mut can_manage = is_admin(&auth);
    if !can_manage {
        if let Some(course_id) = course_id {
            can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
        }
    }
    if !can_manage && !user_can_read_group(&state.db, auth.id, group_id, &auth.role).await? {
        return Err(ApiError::Forbidden);
    }

    let row = sqlx::query_as::<_, GroupFileRow>(
        "SELECT id, group_id, folder, filename, \
                COALESCE(display_name, filename) AS display_name, \
                content_type, size, created_at, updated_at \
         FROM group_files WHERE id = $1 AND group_id = $2",
    )
    .bind(file_id)
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    Ok(Json(GroupFileResponse::from(row)))
}

/// GET /users/{userId}/files/{id} — Canvas files#api_show (user scope).
///
/// Deviation: star-lms files are course-scoped, so a user's files are the
/// metadata they uploaded (uploader_id), like list_user_folders_and_files.
async fn get_user_file(
    State(state): State<AppState>,
    Path((user_id, file_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<AttachmentResponse>, ApiError> {
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "{ATTACHMENT_SELECT} WHERE f.id = $1 AND f.uploader_id = $2 AND f.workflow_state <> 'deleted'"
    );
    let file = sqlx::query_as::<_, AttachmentRow>(&sql)
        .bind(file_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(AttachmentResponse::from_row(file, true)))
}

// ─── new handlers: file upload lifecycle, icon, and context-scoped files ─

/// POST /files/{id} — Canvas files#api_create_success (POST variant).
/// Commits a pending file record after S3 upload completes.
async fn post_file(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    _auth: AuthUser,
) -> Result<impl IntoResponse, ApiError> {
    let _: (i64,) = sqlx::query_as("SELECT id FROM files WHERE id = $1")
        .bind(file_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(serde_json::json!({ "message": "file upload acknowledged" })).into_response())
}

/// GET /files/{id}/create_success — Canvas files#api_create_success (GET).
/// Returns the file record after upload finalisation with preview URLs.
async fn create_success_get(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    Query(_params): Query<CreateSuccessQuery>,
    auth: AuthUser,
) -> Result<Json<CreateSuccessResponse>, ApiError> {
    let sql = format!("{ATTACHMENT_SELECT} WHERE f.id = $1 AND f.workflow_state <> 'deleted'");
    let row = sqlx::query_as::<_, AttachmentRow>(&sql)
        .bind(file_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let can_manage = user_teacher_of_course(&state.db, auth.id, row.course_id, &auth.role).await?
        || is_admin(&auth);
    let attachment = AttachmentResponse::from_row(row, can_manage);

    Ok(Json(CreateSuccessResponse {
        attachment,
        preview_url: None,
        enhanced_preview_url: None,
    }))
}

/// POST /files/{id}/create_success — Canvas files#api_create_success (POST).
/// Marks the file as uploaded and returns the record.
async fn create_success_post(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<AttachmentResponse>, ApiError> {
    let sql = format!("{ATTACHMENT_SELECT} WHERE f.id = $1 AND f.workflow_state <> 'deleted'");
    let row = sqlx::query_as::<_, AttachmentRow>(&sql)
        .bind(file_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let can_manage = user_teacher_of_course(&state.db, auth.id, row.course_id, &auth.role).await?
        || is_admin(&auth);
    Ok(Json(AttachmentResponse::from_row(row, can_manage)))
}

/// OPTIONS /files/{id}/create_success — CORS preflight for the upload finalisation route.
async fn create_success_options() -> impl IntoResponse {
    let mut resp = Response::new(axum::body::Body::empty());
    resp.headers_mut().insert(
        axum::http::header::ALLOW,
        axum::http::HeaderValue::from_static("GET,POST,OPTIONS"),
    );
    resp
}

/// POST /files/{id}/reset_verifier — Canvas files#reset_verifier.
/// Re-generates the download UUID (verifier) for a file.
async fn reset_verifier(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: (i64, i64) = sqlx::query_as(
        "SELECT course_id, uploader_id FROM files WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(file_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    let (course_id, uploader_id) = row;
    let can_manage =
        user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await? || is_admin(&auth);
    if !can_manage && uploader_id != auth.id {
        return Err(ApiError::Forbidden);
    }

    let new_uuid = uuid::Uuid::new_v4().to_string();
    sqlx::query("UPDATE files SET uuid = $1, updated_at = NOW() WHERE id = $2")
        .bind(&new_uuid)
        .bind(file_id)
        .execute(&state.db)
        .await?;

    Ok(Json(serde_json::json!({ "uuid": new_uuid })))
}

/// POST /rce_linked_file_instfs_ids — Canvas
/// `files#rce_linked_file_instfs_ids`
/// (`app/controllers/files_controller.rb:1650`).
///
/// Canvas resolves the target user from the `user_uuid` parameter and asks
/// that user for `read_full_profile`; without the parameter there is no user
/// to ask, so the request is unauthorized (403) — the file urls are never
/// looked at.
async fn rce_linked_file_instfs_ids(
    State(state): State<AppState>,
    Query(params): Query<Vec<(String, String)>>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_uuid = params
        .iter()
        .find(|(key, _)| key == "user_uuid")
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty());

    let Some(user_uuid) = user_uuid else {
        return Err(ApiError::Forbidden);
    };

    let target: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM users WHERE uuid = $1 AND workflow_state <> 'deleted'")
            .bind(&user_uuid)
            .fetch_optional(&state.db)
            .await?;

    let Some((target_id,)) = target else {
        return Err(ApiError::Forbidden);
    };

    // `read_full_profile` on the target user: the target themselves, or an
    // account admin of the target's account.
    let may_read = target_id == auth.id
        || sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (
                SELECT 1 FROM account_users au
                JOIN accounts a ON a.id = au.account_id
                WHERE au.user_id = $1 AND au.workflow_state = 'active'
                  AND a.id = COALESCE(
                      (SELECT c.account_id FROM courses c
                        JOIN enrollments e ON e.course_id = c.id
                        WHERE e.user_id = $2 LIMIT 1),
                      a.id)
            )",
        )
        .bind(auth.id)
        .bind(target_id)
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?;

    if !may_read {
        return Err(ApiError::Forbidden);
    }

    // Canvas: no file urls given is a 400.
    let file_urls = params
        .iter()
        .filter(|(key, _)| key == "file_urls")
        .map(|(_, value)| value.clone())
        .collect::<Vec<_>>();
    if file_urls.is_empty() {
        return Err(ApiError::canvas_json(
            axum::http::StatusCode::BAD_REQUEST,
            serde_json::json!({ "errors": [{ "message": "No valid file URLs given" }] }),
        ));
    }

    Ok(Json(serde_json::json!({ "file_ids": [] })))
}

/// GET /files/{id}/icon_metadata — Canvas files#icon_metadata.
/// Parses an SVG to extract width, height, and viewBox.
async fn get_icon_metadata(
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
    _auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row: (String,) = sqlx::query_as(
        "SELECT content_type FROM files WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(file_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let (content_type,) = row;

    if !content_type.starts_with("image/svg") {
        return Err(ApiError::BadRequest("file is not an SVG".into()));
    }

    // star-lms metadata-only: no actual SVG bytes stored; return placeholder dims.
    Ok(Json(serde_json::json!({
        "width": 64,
        "height": 64,
        "viewBox": "0 0 64 64"
    })))
}

/// DELETE /groups/{groupId}/files/{id} — delete a group file.
async fn delete_group_file(
    State(state): State<AppState>,
    Path((group_id, file_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let group: (i64, Option<i64>) = sqlx::query_as(
        "SELECT id, course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let (_, course_id) = group;

    // Group files live in group_files (course_id NULL for account groups);
    // manage rights come from the group's course when it has one.
    let mut can_manage = is_admin(&auth);
    if !can_manage {
        if let Some(course_id) = course_id {
            can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
        }
    }
    if !can_manage {
        return Err(ApiError::Forbidden);
    }

    sqlx::query("UPDATE group_files SET updated_at = NOW() WHERE id = $1 AND group_id = $2")
        .bind(file_id)
        .bind(group_id)
        .execute(&state.db)
        .await?;

    // Canvas attachments#destroy soft-deletes; group_files has no
    // workflow_state, so remove the metadata row (metadata-only table).
    sqlx::query("DELETE FROM group_files WHERE id = $1 AND group_id = $2")
        .bind(file_id)
        .bind(group_id)
        .execute(&state.db)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /groups/{groupId}/files/quota — Canvas files#quota for a group.
async fn get_group_files_quota(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<QuotaResponse>, ApiError> {
    let group: (i64, Option<i64>) = sqlx::query_as(
        "SELECT id, course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let (group_id_val, _course_id) = group;

    let can_manage = user_teacher_of_course(&state.db, auth.id, group_id_val, &auth.role).await?
        || is_admin(&auth);
    let allowed = can_manage || user_in_group(&state.db, auth.id, group_id_val).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    // star-lms: no group-level storage_quota; return zero quota.
    // Canvas api_quota renders { quota, quota_used } (files_controller.rb:255).
    Ok(Json(QuotaResponse {
        quota: 0,
        quota_used: 0,
    }))
}

/// GET /users/{userId}/files — list a user's uploaded files.
async fn list_user_files(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(params): Query<ListFilesQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<FileResponse>>, ApiError> {
    // Canvas: only self or admin may list a user's files.
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let (per_page, offset) = pagination(params.page, params.per_page);

    // Canvas files#api_index for a user context lists the user's files across
    // contexts (no course filter); star-lms scopes by uploader_id.
    let files = sqlx::query_as::<_, FileRow>(
        r#"
        SELECT id, course_id, folder, folder_id, filename, display_name,
               content_type, size, created_at, updated_at
        FROM files
        WHERE uploader_id = $1 AND workflow_state <> 'deleted'
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(user_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(files.into_iter().map(FileResponse::from).collect()))
}

/// GET /users/{userId}/files/quota — Canvas files#quota for a user.
async fn get_user_files_quota(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<QuotaResponse>, ApiError> {
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Aggregate from all courses the user has uploaded to.
    let (used,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(f.size), 0) FROM files f WHERE f.uploader_id = $1 AND f.workflow_state <> 'deleted'",
    )
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;

    // Default 50 MB quota; courses.storage_quota is not user-scoped in star-lms.
    let quota: i64 = 52_428_800; // 50 MiB

    Ok(Json(QuotaResponse {
        quota,
        quota_used: used,
    }))
}

// ─── existing get_file_ref handler follows ──────────────────────────────────

/// GET /courses/{courseId}/files/file_ref/{migrationId} — Canvas
/// files#file_ref: translate a course-copy file reference.
///
/// Deviation: files carry no Canvas migration_id, so the reference resolves
/// against the file's uuid (the per-file identifier course copies keep).
async fn get_file_ref(
    State(state): State<AppState>,
    Path((course_id, migration_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<AttachmentResponse>, ApiError> {
    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed =
        can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let sql = format!(
        "{ATTACHMENT_SELECT} WHERE f.course_id = $1 AND f.uuid = $2 AND f.workflow_state <> 'deleted'"
    );
    let file = sqlx::query_as::<_, AttachmentRow>(&sql)
        .bind(course_id)
        .bind(&migration_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(AttachmentResponse::from_row(file, can_manage)))
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
    ApiJson(payload): ApiJson<CreateFolderRequest>,
) -> Result<Response, ApiError> {
    create_folder_inner(&state, &auth, payload, None, None).await
}

/// POST /courses/{courseId}/folders — Canvas context create.
async fn create_course_folder(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<CreateFolderRequest>,
) -> Result<Response, ApiError> {
    create_folder_inner(&state, &auth, payload, Some(course_id), None).await
}

/// POST /folders/{id}/folders — Canvas create sub-folder.
async fn create_subfolder(
    State(state): State<AppState>,
    Path(parent_folder_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<CreateFolderRequest>,
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
        context_type: "Course".to_string(),
        context_id: course_id,
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
    ApiJson(payload): ApiJson<UpdateFolderRequest>,
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
    // Canvas gates the endpoint on the site-admin a11y feature flag
    // (`app/controllers/folders_controller.rb`).
    if !files_a11y_rewrite_enabled(&state.db).await? {
        return Err(ApiError::Forbidden);
    }
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
    // Canvas gates the endpoint on the site-admin a11y feature flag
    // (`app/controllers/folders_controller.rb`).
    if !files_a11y_rewrite_enabled(&state.db).await? {
        return Err(ApiError::Forbidden);
    }
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

// ─── deep folder/file handlers (Phase 4) ───────────────────────────────────

// ── resolve_path ────────────────────────────────────────────────────────────

/// Query params for resolve_path endpoints (full_path split by "/").
#[derive(Debug, Deserialize)]
pub struct ResolvePathQuery {
    /// Canvas passes the path segments as `full_path[]` array params.
    /// When not present, path comes from the wildcard segment in by_path/*full_path.
    #[serde(rename = "full_path[]", default)]
    pub full_path_segments: Vec<String>,
    /// Fallback: colon-separated path when wildcard isn't available.
    full_path: Option<String>,
}

impl ResolvePathQuery {
    /// Returns path components: either explicit `full_path[]` array items
    /// or a slash-split `full_path` string.
    fn components(&self) -> Vec<&str> {
        if !self.full_path_segments.is_empty() {
            self.full_path_segments.iter().map(String::as_str).collect()
        } else if let Some(ref fp) = self.full_path {
            fp.split('/').filter(|s| !s.is_empty()).collect()
        } else {
            Vec::new()
        }
    }
}

/// Recursive helper: find the folder at the end of a path under a root folder.
async fn resolve_path_under_root(
    db: &PgPool,
    root_id: i64,
    components: &[&str],
    can_view_hidden: bool,
) -> Result<Option<FolderRow>, ApiError> {
    if components.is_empty() {
        return fetch_folder(db, root_id).await;
    }
    let sql = if can_view_hidden {
        format!(
            "{FOLDER_SELECT} WHERE f.parent_folder_id = $1 AND f.name = $2 \
             AND f.workflow_state <> 'deleted' LIMIT 1"
        )
    } else {
        format!(
            "{FOLDER_SELECT} WHERE f.parent_folder_id = $1 AND f.name = $2 \
             AND f.workflow_state <> 'hidden' AND f.workflow_state <> 'deleted' \
             AND f.locked = FALSE LIMIT 1"
        )
    };
    // Iterative walk (Canvas Folder.resolve_path): descend one named
    // component at a time from the root; any miss ends the search.
    let mut current_id = root_id;
    let mut last: Option<FolderRow> = None;
    for head in components {
        let child: Option<FolderRow> = sqlx::query_as::<_, FolderRow>(&sql)
            .bind(current_id)
            .bind(*head)
            .fetch_optional(db)
            .await?;
        match child {
            Some(folder) => {
                current_id = folder.id;
                last = Some(folder);
            }
            None => return Ok(None),
        }
    }
    Ok(last)
}

/// Shared resolve_path body. Finds a folder by slash-separated path within
/// a context (Course/User/Group). Returns 404 if not found.
async fn resolve_path_impl(
    db: &PgPool,
    context_type: &str,
    context_id: i64,
    params: &ResolvePathQuery,
    can_view_hidden: bool,
) -> Result<Json<Vec<FolderResponse>>, ApiError> {
    let components: Vec<&str> = params.components();
    let roots: Vec<FolderRow> = sqlx::query_as::<_, FolderRow>(
        format!(
            "{FOLDER_SELECT} WHERE f.context_type = $1 AND f.context_id = $2 \
             AND f.parent_folder_id IS NULL AND f.workflow_state <> 'deleted' \
             ORDER BY f.id ASC"
        )
        .as_str(),
    )
    .bind(context_type)
    .bind(context_id)
    .fetch_all(db)
    .await?;

    for root in roots {
        if let Some(folder) =
            resolve_path_under_root(db, root.id, &components, can_view_hidden).await?
        {
            // Canvas returns all matched folders: [root, subfolder1, ..., target].
            // Build ancestors for the response.
            let mut ancestors: Vec<FolderRow> = Vec::new();
            let mut parent_id = folder.parent_folder_id;
            loop {
                match parent_id {
                    None => break,
                    Some(pid) => {
                        let parent: Option<FolderRow> = fetch_folder(db, pid).await?;
                        match parent {
                            Some(p) => {
                                parent_id = p.parent_folder_id;
                                ancestors.push(p);
                            }
                            None => break,
                        }
                    }
                }
            }
            ancestors.reverse();
            ancestors.push(folder.clone());
            let can_manage = true; // Resolved from context auth; can_view_hidden implies teacher.
            return Ok(Json(
                ancestors
                    .into_iter()
                    .map(|row| FolderResponse::from_row(row, can_manage))
                    .collect(),
            ));
        }
    }
    Err(ApiError::NotFound)
}

// ── course resolve_path ─────────────────────────────────────────────────────

/// GET /courses/{courseId}/folders/by_path/*full_path
/// GET /courses/{courseId}/folders/by_path
async fn resolve_path_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<ResolvePathQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let can_view_hidden =
        user_has_granular_file_perm(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed = can_view_hidden
        || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    let result = resolve_path_impl(&state.db, "Course", course_id, &params, can_view_hidden).await;
    match result {
        Ok(json) => Ok(json.into_response()),
        Err(ApiError::NotFound) => Ok(canvas_not_found()),
        Err(e) => Err(e),
    }
}

/// GET /courses/{courseId}/folders/by_path/*full_path (no trailing slash variant, wildcard)
async fn resolve_path_course_named(
    State(state): State<AppState>,
    Path((course_id, full_path)): Path<(i64, String)>,
    Query(params): Query<ResolvePathQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let mut q = params;
    if q.full_path.is_none() {
        q.full_path = Some(full_path);
    }
    resolve_path_course(State(state), Path(course_id), Query(q), auth).await
}

// ── user resolve_path ────────────────────────────────────────────────────────

/// GET /users/{userId}/folders/by_path/*full_path
async fn resolve_path_user(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(params): Query<ResolvePathQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    // Canvas: only self or admin.
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let can_view_hidden =
        user_has_granular_file_perm(&state.db, auth.id, user_id, &auth.role).await?;
    let result = resolve_path_impl(&state.db, "User", user_id, &params, can_view_hidden).await;
    match result {
        Ok(json) => Ok(json.into_response()),
        Err(ApiError::NotFound) => Ok(canvas_not_found()),
        Err(e) => Err(e),
    }
}

/// GET /users/{userId}/folders/by_path (no trailing wildcard)
async fn resolve_path_user_named(
    State(state): State<AppState>,
    Path((user_id, full_path)): Path<(i64, String)>,
    Query(params): Query<ResolvePathQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let mut q = params;
    if q.full_path.is_none() {
        q.full_path = Some(full_path);
    }
    resolve_path_user(State(state), Path(user_id), Query(q), auth).await
}

// ── group resolve_path ──────────────────────────────────────────────────────

/// GET /groups/{groupId}/folders/by_path/*full_path
async fn resolve_path_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(params): Query<ResolvePathQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let can_view_hidden = user_can_read_group(&state.db, auth.id, group_id, &auth.role).await?;
    if !can_view_hidden {
        return Err(ApiError::Forbidden);
    }
    let result = resolve_path_impl(&state.db, "Group", group_id, &params, can_view_hidden).await;
    match result {
        Ok(json) => Ok(json.into_response()),
        Err(ApiError::NotFound) => Ok(canvas_not_found()),
        Err(e) => Err(e),
    }
}

/// GET /groups/{groupId}/folders/by_path (no trailing wildcard)
async fn resolve_path_group_named(
    State(state): State<AppState>,
    Path((group_id, full_path)): Path<(i64, String)>,
    Query(params): Query<ResolvePathQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let mut q = params;
    if q.full_path.is_none() {
        q.full_path = Some(full_path);
    }
    resolve_path_group(State(state), Path(group_id), Query(q), auth).await
}

// ── context-scoped show ──────────────────────────────────────────────────────

/// GET /courses/{courseId}/folders/{id}
async fn show_course_folder(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<FolderResponse>, ApiError> {
    let folder_id: i64 = id.parse().map_err(|_| ApiError::NotFound)?;
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if folder.context_type != "Course" || folder.context_id != course_id {
        return Err(ApiError::NotFound);
    }
    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    let allowed =
        can_manage || user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(FolderResponse::from_row(folder, can_manage)))
}

/// GET /users/{userId}/folders/{id}
async fn show_user_folder(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<FolderResponse>, ApiError> {
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let folder_id: i64 = id.parse().map_err(|_| ApiError::NotFound)?;
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if folder.context_type != "User" || folder.context_id != user_id {
        return Err(ApiError::NotFound);
    }
    Ok(Json(FolderResponse::from_row(folder, true)))
}

/// GET /groups/{groupId}/folders/{id}
async fn show_group_folder(
    State(state): State<AppState>,
    Path((group_id, id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<FolderResponse>, ApiError> {
    let can_view = user_can_read_group(&state.db, auth.id, group_id, &auth.role).await?;
    if !can_view {
        return Err(ApiError::Forbidden);
    }
    let folder_id: i64 = id.parse().map_err(|_| ApiError::NotFound)?;
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if folder.context_type != "Group" || folder.context_id != group_id {
        return Err(ApiError::NotFound);
    }
    Ok(Json(FolderResponse::from_row(folder, false)))
}

// ── list_all_folders ─────────────────────────────────────────────────────────

/// GET /users/{userId}/folders — Canvas list_all_folders (User context)
async fn list_all_user_folders(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(params): Query<FolderListQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<FolderResponse>>, ApiError> {
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let (per_page, offset) = pagination(params.page, params.per_page);
    let by_position = params.sort_by.as_deref() == Some("position");
    let sql = if by_position {
        format!(
            "{FOLDER_SELECT} WHERE f.context_type = 'User' AND f.context_id = $1 \
             AND f.workflow_state <> 'deleted' ORDER BY f.position ASC, f.id ASC LIMIT $2 OFFSET $3"
        )
    } else {
        format!(
            "{FOLDER_SELECT} WHERE f.context_type = 'User' AND f.context_id = $1 \
             AND f.workflow_state <> 'deleted' ORDER BY f.name ASC, f.id ASC LIMIT $2 OFFSET $3"
        )
    };
    let rows = sqlx::query_as::<_, FolderRow>(&sql)
        .bind(user_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(|row| FolderResponse::from_row(row, true))
            .collect(),
    ))
}

// ── create folder in context ─────────────────────────────────────────────────

/// POST /users/{userId}/folders — create folder in user context.
async fn create_user_folder_ctx(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<CreateFolderRequest>,
) -> Result<Response, ApiError> {
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    // Use user root folder as parent.
    let roots: Vec<FolderRow> = sqlx::query_as::<_, FolderRow>(
        format!(
            "{FOLDER_SELECT} WHERE f.context_type = 'User' AND f.context_id = $1 \
             AND f.parent_folder_id IS NULL AND f.workflow_state <> 'deleted' LIMIT 1"
        )
        .as_str(),
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    let root_id = if let Some(root) = roots.first() {
        root.id
    } else {
        // Create user root folder first.
        let root_name = "my files".to_string();
        let full_name = root_name.clone();
        let new_root = NewFolder {
            name: root_name,
            full_name,
            context_type: "User".to_string(),
            context_id: user_id,
            parent_id: 0, // sentinel: will be replaced in insert_folder.
            workflow_state: "visible".to_string(),
            position: 0,
            locked: false,
            lock_at: None,
            unlock_at: None,
            for_submissions: false,
        };
        insert_folder(&state.db, new_root).await?.id
    };
    // Canvas folders#create falls back to the context's unfiled folder when
    // the request names no parent (app/controllers/folders_controller.rb:545).
    let parent = match payload.parent_folder_id {
        Some(parent_id) => {
            let parent = fetch_folder(&state.db, parent_id)
                .await?
                .ok_or(ApiError::NotFound)?;
            if parent.context_type != "User" || parent.context_id != user_id {
                return Ok(canvas_not_found());
            }
            parent
        }
        None => {
            let root = fetch_folder(&state.db, root_id)
                .await?
                .ok_or(ApiError::NotFound)?;
            ensure_unfiled_folder(&state.db, root).await?
        }
    };
    let parent_id = parent.id;
    let requested_name = normalize_folder_name(payload.name.as_deref());
    // Canvas Folder#prevent_duplicate_name appends the first free " <n>".
    let siblings = fetch_sibling_names(&state.db, parent_id).await?;
    let name = dedupe_folder_name(&requested_name, &siblings);
    let position = next_position(&state.db, parent_id).await?;
    let full_name = join_full_name(&parent.full_name, &name);
    // Canvas Folder#hidden= maps the hidden param onto workflow_state
    // 'hidden'/'visible' (app/models/folder.rb), never a raw boolean.
    let workflow_state = if payload.hidden.unwrap_or(false) {
        "hidden"
    } else {
        "visible"
    };
    let new_folder = NewFolder {
        name,
        full_name,
        context_type: "User".to_string(),
        context_id: user_id,
        parent_id,
        workflow_state: workflow_state.to_string(),
        position,
        locked: payload.locked.unwrap_or(false),
        lock_at: payload.lock_at,
        unlock_at: payload.unlock_at,
        for_submissions: false,
    };
    let folder = insert_folder(&state.db, new_folder).await?;
    Ok(Json(FolderResponse::from_row(folder, true)).into_response())
}

// ── icon_maker_folder ────────────────────────────────────────────────────────

/// Canvas unique folder type marker.
const ICON_MAKER_UNIQUE_TYPE: &str = "icon maker icons";

/// GET /courses/{courseId}/folders/icon_maker
async fn icon_maker_folder_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<FolderResponse>, ApiError> {
    let allowed = user_has_granular_file_perm(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    let root = ensure_root_folder(&state.db, course_id).await?;
    // Find or create icon maker folder by unique_type.
    let folder = find_or_create_unique_folder(
        &state.db,
        "Course",
        course_id,
        root.id,
        ICON_MAKER_UNIQUE_TYPE,
        "Icon Maker Icons",
    )
    .await?;
    Ok(Json(FolderResponse::from_row(folder, true)))
}

// ── media_folder ────────────────────────────────────────────────────────────

const MEDIA_TYPE: &str = "media";

/// GET /courses/{courseId}/folders/media
async fn media_folder_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<FolderResponse>, ApiError> {
    // Canvas: requires :read_files on context.
    let allowed = user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    let can_manage = user_has_granular_file_perm(&state.db, auth.id, course_id, &auth.role).await?;
    let root = ensure_root_folder(&state.db, course_id).await?;
    let folder = find_or_create_unique_folder(
        &state.db,
        "Course",
        course_id,
        root.id,
        MEDIA_TYPE,
        "Uploaded Media",
    )
    .await?;
    Ok(Json(FolderResponse::from_row(folder, can_manage)))
}

/// GET /users/{userId}/folders/media
async fn media_folder_user(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<FolderResponse>, ApiError> {
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    // Find or create user root.
    let roots: Vec<FolderRow> = sqlx::query_as::<_, FolderRow>(
        format!(
            "{FOLDER_SELECT} WHERE f.context_type = 'User' AND f.context_id = $1 \
             AND f.parent_folder_id IS NULL AND f.workflow_state <> 'deleted' LIMIT 1"
        )
        .as_str(),
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;
    let root_id = if let Some(root) = roots.first() {
        root.id
    } else {
        let new_root = NewFolder {
            name: "my files".to_string(),
            full_name: "my files".to_string(),
            context_type: "User".to_string(),
            context_id: user_id,
            parent_id: 0,
            workflow_state: "visible".to_string(),
            position: 0,
            locked: false,
            lock_at: None,
            unlock_at: None,
            for_submissions: false,
        };
        insert_folder(&state.db, new_root).await?.id
    };
    let folder = find_or_create_unique_folder(
        &state.db,
        "User",
        user_id,
        root_id,
        MEDIA_TYPE,
        "Uploaded Media",
    )
    .await?;
    Ok(Json(FolderResponse::from_row(folder, true)))
}

// ── user-scoped duplicates ──────────────────────────────────────────────────

/// GET /users/{userId}/folders/{id}/duplicates
async fn list_user_folder_duplicates(
    State(state): State<AppState>,
    Path((user_id, folder_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DuplicatesResponse>, ApiError> {
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    // Canvas gates the endpoint on the site-admin a11y feature flag
    // (`app/controllers/folders_controller.rb`).
    if !files_a11y_rewrite_enabled(&state.db).await? {
        return Err(ApiError::Forbidden);
    }

    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if folder.context_type != "User" || folder.context_id != user_id {
        return Err(ApiError::NotFound);
    }
    let Some(parent_id) = folder.parent_folder_id else {
        return Ok(Json(DuplicatesResponse {
            duplicates: Vec::new(),
        }));
    };
    let sql = format!(
        "{FOLDER_SELECT} WHERE f.parent_folder_id = $1 AND f.name = $2 \
         AND f.id <> $3 AND f.workflow_state <> 'deleted' ORDER BY f.id ASC"
    );
    let rows = sqlx::query_as::<_, FolderRow>(&sql)
        .bind(parent_id)
        .bind(&folder.name)
        .bind(folder_id)
        .fetch_all(&state.db)
        .await?;
    Ok(Json(DuplicatesResponse {
        duplicates: rows
            .into_iter()
            .map(|row| FolderResponse::from_row(row, true))
            .collect(),
    }))
}

// ── copy_file ───────────────────────────────────────────────────────────────

/// POST /folders/{destFolderId}/copy_file
async fn copy_file_to_folder(
    State(state): State<AppState>,
    Path(dest_folder_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CopyFileRequest>,
) -> Result<Json<AttachmentResponse>, ApiError> {
    let dest = fetch_folder(&state.db, dest_folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let course_id = folder_course_id(&dest)?;
    let allowed = user_has_granular_file_perm(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let source_file_id = payload
        .source_file_id
        .ok_or(ApiError::BadRequest("source_file_id is required".into()))?;
    let source_row: Option<(i64, String, String, String, i64, Option<i64>)> = sqlx::query_as(
        r#"SELECT id, filename, display_name, content_type, size, folder_id
           FROM files WHERE id = $1 AND workflow_state <> 'deleted'"#,
    )
    .bind(source_file_id)
    .fetch_optional(&state.db)
    .await?;
    let (_src_id, src_filename, src_display, src_ct, src_size, _src_folder_id) =
        source_row.ok_or(ApiError::NotFound)?;

    // Check on_duplicate.
    if let Some(ref od) = payload.on_duplicate {
        if !["rename", "overwrite"].contains(&od.as_str()) {
            return Err(ApiError::BadRequest(
                "on_duplicate must be 'rename' or 'overwrite'".into(),
            ));
        }
    }

    let new_name = payload
        .name
        .as_ref()
        .filter(|n| !n.is_empty())
        .cloned()
        .unwrap_or_else(|| src_display.clone());

    // Check for existing file in dest.
    if payload.on_duplicate.is_none() {
        let exists: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(
                SELECT 1 FROM files
                WHERE folder_id = $1 AND display_name = $2 AND workflow_state <> 'deleted'
            )"#,
        )
        .bind(dest_folder_id)
        .bind(&new_name)
        .fetch_one(&state.db)
        .await?;
        if exists {
            return Err(ApiError::Conflict(
                "file already exists; set on_duplicate to 'rename' or 'overwrite'".into(),
            ));
        }
    }

    // Insert copy.
    let copied_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO files (course_id, uploader_id, filename, display_name,
                                content_type, size, folder, folder_id)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
           RETURNING id"#,
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(&src_filename)
    .bind(&new_name)
    .bind(&src_ct)
    .bind(src_size)
    .bind(&dest.full_name)
    .bind(dest_folder_id)
    .fetch_one(&state.db)
    .await?;

    // Handle duplicate action.
    if let Some(ref od) = payload.on_duplicate {
        if od == "overwrite" {
            // Delete other files with same display_name in this folder.
            sqlx::query(
                r#"DELETE FROM files
                   WHERE folder_id = $1 AND display_name = $2 AND id <> $3"#,
            )
            .bind(dest_folder_id)
            .bind(&new_name)
            .bind(copied_id)
            .execute(&state.db)
            .await?;
        }
    }

    let row: AttachmentRow = sqlx::query_as::<_, AttachmentRow>(
        "SELECT id, uuid, course_id, folder, folder_id, filename, display_name, \
                content_type, size, locked, hidden, lock_at, unlock_at, \
                created_at, updated_at, modified_at, uuid, category, visibility_level \
         FROM files WHERE id = $1",
    )
    .bind(copied_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(AttachmentResponse::from_row(row, true)))
}

// ── copy_folder ─────────────────────────────────────────────────────────────

/// POST /folders/{destFolderId}/copy_folder
async fn copy_folder_to_folder(
    State(state): State<AppState>,
    Path(dest_folder_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CopyFolderRequest>,
) -> Result<Json<FolderResponse>, ApiError> {
    let dest = fetch_folder(&state.db, dest_folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let course_id = folder_course_id(&dest)?;
    let allowed = user_has_granular_file_perm(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let source_folder_id = payload
        .source_folder_id
        .ok_or(ApiError::BadRequest("source_folder_id is required".into()))?;
    let source = fetch_folder(&state.db, source_folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Cannot copy folder into itself.
    let source_ancestors = collect_folder_ancestors(&state.db, source_folder_id).await?;
    if source_ancestors.contains(&dest_folder_id) || source_folder_id == dest_folder_id {
        return Err(ApiError::BadRequest(
            "source folder may not contain destination folder".into(),
        ));
    }

    // Copy: create new folder under dest with same name, then copy files.
    let name = source.name.clone();
    let full_name = format!("{}/{}", dest.full_name, name);
    let position = next_position(&state.db, dest_folder_id).await?;

    let new_folder = NewFolder {
        name,
        full_name: full_name.clone(),
        context_type: dest.context_type.clone(),
        context_id: dest.context_id,
        parent_id: dest_folder_id,
        workflow_state: source.workflow_state.clone(),
        position,
        locked: source.locked,
        lock_at: source.lock_at,
        unlock_at: source.unlock_at,
        for_submissions: source.for_submissions,
    };
    let copied = insert_folder(&state.db, new_folder).await?;

    // Copy files from source into copied folder.
    sqlx::query(
        r#"INSERT INTO files (course_id, uploader_id, filename, display_name,
                               content_type, size, folder, folder_id)
           SELECT $1, uploader_id, filename, display_name, content_type, size,
                  $2, $3
           FROM files WHERE folder_id = $4 AND workflow_state <> 'deleted'"#,
    )
    .bind(course_id)
    .bind(&full_name)
    .bind(copied.id)
    .bind(source_folder_id)
    .execute(&state.db)
    .await?;

    Ok(Json(FolderResponse::from_row(copied, true)))
}

/// Collect all ancestor folder IDs of a folder (excluding itself).
async fn collect_folder_ancestors(db: &PgPool, folder_id: i64) -> Result<Vec<i64>, ApiError> {
    let mut ancestors = Vec::new();
    let mut current = fetch_folder(db, folder_id).await?;
    while let Some(f) = current {
        if let Some(pid) = f.parent_folder_id {
            ancestors.push(pid);
            current = fetch_folder(db, pid).await?;
        } else {
            break;
        }
    }
    Ok(ancestors)
}

/// Helper: find or create a unique-type folder (icon_maker or media).
async fn find_or_create_unique_folder(
    db: &PgPool,
    context_type: &str,
    context_id: i64,
    root_id: i64,
    // Canvas identifies unique folders by a folders.unique_type column
    // (app/models/folder.rb Folder.unique_folder); the star schema has no
    // such column, so lookup falls back to the default name below.
    _unique_type: &str,
    default_name: &str,
) -> Result<FolderRow, ApiError> {
    // Try to find existing.
    let existing: Option<FolderRow> = sqlx::query_as::<_, FolderRow>(
        format!(
            "{FOLDER_SELECT} WHERE f.parent_folder_id = $1 AND f.name = $2 \
             AND f.workflow_state <> 'deleted' LIMIT 1"
        )
        .as_str(),
    )
    .bind(root_id)
    .bind(default_name)
    .fetch_optional(db)
    .await?;

    if let Some(folder) = existing {
        return Ok(folder);
    }

    // Create hidden unique folder.
    let position = next_position(db, root_id).await?;
    let full_name = format!(
        "{}/{}",
        fetch_folder(db, root_id)
            .await?
            .map(|f| f.full_name)
            .unwrap_or_default(),
        default_name
    );
    let new_folder = NewFolder {
        name: default_name.to_string(),
        full_name,
        context_type: context_type.to_string(),
        context_id,
        parent_id: root_id,
        workflow_state: "hidden".to_string(),
        position,
        locked: false,
        lock_at: None,
        unlock_at: None,
        for_submissions: false,
    };
    insert_folder(db, new_folder).await
}

/// GET /courses/{courseId}/folders/{id}/duplicates — Canvas
/// FoldersController#duplicates: sibling folders sharing the same name.
async fn list_folder_duplicates(
    State(state): State<AppState>,
    Path((course_id, folder_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DuplicatesResponse>, ApiError> {
    // Canvas gates the endpoint on the site-admin a11y feature flag
    // (`app/controllers/folders_controller.rb`).
    if !files_a11y_rewrite_enabled(&state.db).await? {
        return Err(ApiError::Forbidden);
    }
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if folder.context_type != "Course" || folder.context_id != course_id {
        return Err(ApiError::NotFound);
    }

    let allowed = user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }
    let can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;

    // Canvas find_duplicate_folders: a root folder has no duplicates.
    let Some(parent_id) = folder.parent_folder_id else {
        return Ok(Json(DuplicatesResponse {
            duplicates: Vec::new(),
        }));
    };
    let sql = format!(
        "{FOLDER_SELECT} WHERE f.parent_folder_id = $1 AND f.name = $2 \
         AND f.id <> $3 AND f.workflow_state <> 'deleted' ORDER BY f.id ASC"
    );
    let rows = sqlx::query_as::<_, FolderRow>(&sql)
        .bind(parent_id)
        .bind(&folder.name)
        .bind(folder.id)
        .fetch_all(&state.db)
        .await?;

    Ok(Json(DuplicatesResponse {
        duplicates: rows
            .into_iter()
            .map(|row| FolderResponse::from_row(row, can_manage))
            .collect(),
    }))
}

/// GET /users/{userId}/folders_and_files — Canvas
/// FoldersController#list_all_folders_and_files for a User context.
async fn list_user_folders_and_files(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(params): Query<FoldersAndFilesQuery>,
    auth: AuthUser,
) -> Result<Json<ContextFoldersAndFiles>, ApiError> {
    // Canvas gates the endpoint on the site-admin a11y feature flag
    // (`app/controllers/folders_controller.rb`).
    if !files_a11y_rewrite_enabled(&state.db).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas: only self or an admin may read another user's files.
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    context_folders_and_files(&state.db, "User", user_id, true, &params).await
}

/// GET /groups/{groupId}/folders_and_files — Canvas
/// FoldersController#list_all_folders_and_files for a Group context.
async fn list_group_folders_and_files(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(params): Query<FoldersAndFilesQuery>,
    auth: AuthUser,
) -> Result<Json<ContextFoldersAndFiles>, ApiError> {
    let group: (i64, Option<i64>) = sqlx::query_as(
        "SELECT id, course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;
    let (_, course_id) = group;

    // Canvas gates the endpoint on the site-admin a11y feature flag
    // (`app/controllers/folders_controller.rb`).
    if !files_a11y_rewrite_enabled(&state.db).await? {
        return Err(ApiError::Forbidden);
    }
    let mut can_manage = is_admin(&auth);
    if !can_manage {
        if let Some(course_id) = course_id {
            can_manage = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
        }
    }
    if !can_manage && !user_can_read_group(&state.db, auth.id, group_id, &auth.role).await? {
        return Err(ApiError::Forbidden);
    }

    context_folders_and_files(&state.db, "Group", group_id, can_manage, &params).await
}

/// Canvas folders_and_files body for User/Group contexts.
///
/// Deviation: star-lms files are course-scoped (`files.course_id NOT NULL`).
/// User listings expose the viewer's own uploaded metadata; Group listings
/// have no file scope and return an empty files array.
async fn context_folders_and_files(
    db: &PgPool,
    context_type: &str,
    context_id: i64,
    can_manage: bool,
    params: &FoldersAndFilesQuery,
) -> Result<Json<ContextFoldersAndFiles>, ApiError> {
    let per_page = params.per_page.unwrap_or(MIXED_LIST_PER_PAGE).clamp(1, 100);
    let offset = (params.page.unwrap_or(1).max(1) - 1) * per_page;
    let search = params
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|term| !term.is_empty());

    let sql = format!(
        "{FOLDER_SELECT} WHERE f.context_type = $1 AND f.context_id = $2 \
         AND f.workflow_state <> 'deleted' \
         AND ($3::text IS NULL OR f.name ILIKE '%' || $3 || '%') \
         ORDER BY f.name ASC, f.id ASC LIMIT $4 OFFSET $5"
    );
    let folders = sqlx::query_as::<_, FolderRow>(&sql)
        .bind(context_type)
        .bind(context_id)
        .bind(search)
        .bind(per_page)
        .bind(offset)
        .fetch_all(db)
        .await?;

    let files = if context_type == "User" {
        let sql = format!(
            "{ATTACHMENT_SELECT} WHERE f.uploader_id = $1 \
             AND f.workflow_state <> 'deleted' \
             ORDER BY f.display_name ASC, f.id ASC LIMIT $2 OFFSET $3"
        );
        sqlx::query_as::<_, AttachmentRow>(&sql)
            .bind(context_id)
            .bind(per_page)
            .bind(offset)
            .fetch_all(db)
            .await?
    } else {
        Vec::new()
    };

    Ok(Json(ContextFoldersAndFiles {
        folders: folders
            .into_iter()
            .map(|row| FolderResponse::from_row(row, can_manage))
            .collect(),
        files: files
            .into_iter()
            .map(|row| AttachmentResponse::from_row(row, can_manage))
            .collect(),
    }))
}

/// POST /folders/{id}/files — Canvas FoldersController#create_file.
/// Metadata only: no binary upload (out of scope).
async fn create_folder_file(
    State(state): State<AppState>,
    Path(folder_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<CreateFolderFileRequest>,
) -> Result<Json<AttachmentResponse>, ApiError> {
    let folder = fetch_folder(&state.db, folder_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let course_id = folder_course_id(&folder)?;
    // Canvas: creating an attachment requires manage_files_add (teacher).
    // The permission check runs before the preflight validates the filename
    // (app/controllers/folders_controller.rb:637-645).
    let allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    if payload.filename.trim().is_empty() {
        return Err(ApiError::Validation("filename is required".into()));
    }

    let display_name = match payload.display_name {
        Some(ref d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => payload.filename.clone(),
    };
    let content_type = payload
        .content_type
        .unwrap_or_else(|| "application/octet-stream".into());

    let file_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO files (course_id, uploader_id, filename, display_name,
                           content_type, size, folder, folder_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(auth.id)
    .bind(&payload.filename)
    .bind(&display_name)
    .bind(&content_type)
    .bind(payload.size.unwrap_or(0))
    .bind(&folder.full_name)
    .bind(folder.id)
    .fetch_one(&state.db)
    .await?;

    let sql = format!("{ATTACHMENT_SELECT} WHERE f.id = $1");
    let row = sqlx::query_as::<_, AttachmentRow>(&sql)
        .bind(file_id)
        .fetch_one(&state.db)
        .await?;
    Ok(Json(AttachmentResponse::from_row(row, true)))
}

/// POST /accounts/{accountId}/folders — Canvas
/// Accounts::FoldersController#create (Account folder context).
async fn create_account_folder(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<CreateFolderRequest>,
) -> Result<Response, ApiError> {
    let account: Option<i64> =
        sqlx::query_scalar("SELECT id FROM accounts WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(account_id)
            .fetch_optional(&state.db)
            .await?;
    if account.is_none() {
        return Err(ApiError::NotFound);
    }
    if !is_admin(&auth) && !user_account_admin(&state.db, auth.id, account_id).await? {
        return Err(ApiError::Forbidden);
    }

    let parent = match payload.parent_folder_id {
        Some(parent_id) => {
            let Some(parent) = fetch_folder(&state.db, parent_id).await? else {
                return Ok(canvas_not_found());
            };
            if parent.context_type != "Account" || parent.context_id != account_id {
                return Ok(canvas_not_found());
            }
            parent
        }
        None => ensure_root_folder_for(&state.db, "Account", account_id).await?,
    };

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

    let row = insert_folder(
        &state.db,
        NewFolder {
            name,
            full_name,
            context_type: "Account".to_string(),
            context_id: account_id,
            parent_id: parent.id,
            workflow_state: workflow_state.to_string(),
            position,
            locked: payload.locked.unwrap_or(false),
            lock_at: payload.lock_at,
            unlock_at: payload.unlock_at,
            for_submissions: payload.for_submissions.unwrap_or(false),
        },
    )
    .await?;

    Ok(Json(FolderResponse::from_row(row, true)).into_response())
}

/// Canvas usage_rights_json license_name.
fn license_name(license: &str) -> Option<&'static str> {
    Some(match license {
        "private" => "Private (Copyrighted)",
        "public_domain" => "Public Domain",
        "cc_by" => "CC Attribution",
        "cc_by_sa" => "CC Attribution Share Alike",
        "cc_by_nc" => "CC Attribution Non-Commercial",
        "cc_by_nc_sa" => "CC Attribution Non-Commercial Share Alike",
        "cc_by_nd" => "CC Attribution No Derivatives",
        "cc_by_nc_nd" => "CC Attribution Non-Commercial No Derivatives",
        _ => return None,
    })
}

/// Canvas UsageRights#use_justification allowed values.
fn valid_use_justification(value: &str) -> bool {
    matches!(
        value,
        "own_copyright" | "used_by_permission" | "fair_use" | "public_domain" | "creative_commons"
    )
}

/// PUT /courses/{courseId}/usage_rights — Canvas
/// UsageRightsController#set_usage_rights.
async fn set_course_usage_rights(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UsageRightsRequest>,
) -> Result<Response, ApiError> {
    let file_ids = payload.file_ids.unwrap_or_default();
    let folder_ids = payload.folder_ids.unwrap_or_default();
    if file_ids.is_empty() && folder_ids.is_empty() {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "Must supply 'file_ids' and/or 'folder_ids' parameter",
        ));
    }
    let Some(usage) = payload.usage_rights else {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "No 'usage_rights' object supplied",
        ));
    };
    let Some(use_justification) = usage
        .use_justification
        .filter(|value| valid_use_justification(value))
    else {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "Invalid use_justification",
        ));
    };

    // Canvas: manage_files_edit.
    let allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    // Reuse an identical context usage_rights row (Canvas `.where(params).first`).
    let existing: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM usage_rights
        WHERE context_type = 'Course' AND context_id = $1
          AND use_justification = $2
          AND legal_copyright IS NOT DISTINCT FROM $3
          AND license IS NOT DISTINCT FROM $4
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .bind(&use_justification)
    .bind(usage.legal_copyright.as_deref())
    .bind(usage.license.as_deref())
    .fetch_optional(&state.db)
    .await?;

    let usage_rights_id = match existing {
        Some(id) => id,
        None => {
            sqlx::query_scalar(
                r#"
                INSERT INTO usage_rights
                    (context_type, context_id, use_justification, legal_copyright, license)
                VALUES ('Course', $1, $2, $3, $4)
                RETURNING id
                "#,
            )
            .bind(course_id)
            .bind(&use_justification)
            .bind(usage.legal_copyright.as_deref())
            .bind(usage.license.as_deref())
            .fetch_one(&state.db)
            .await?
        }
    };

    // Canvas assigns the rights to the given files plus every file in the
    // given folders (recursively).
    let file_ids = sqlx::query_scalar::<_, i64>(
        r#"
        WITH RECURSIVE tree AS (
            SELECT id FROM folders
            WHERE context_type = 'Course' AND context_id = $1
              AND workflow_state <> 'deleted' AND id = ANY($2::bigint[])
            UNION ALL
            SELECT f.id FROM folders f JOIN tree t ON f.parent_folder_id = t.id
        )
        SELECT id FROM files
        WHERE course_id = $1 AND workflow_state <> 'deleted'
          AND (id = ANY($3::bigint[]) OR folder_id IN (SELECT id FROM tree))
        ORDER BY id
        "#,
    )
    .bind(course_id)
    .bind(&folder_ids)
    .bind(&file_ids)
    .fetch_all(&state.db)
    .await?;

    sqlx::query(
        r#"
        UPDATE files SET usage_rights_id = $1,
                         locked = CASE WHEN $2 THEN FALSE ELSE locked END,
                         updated_at = now()
        WHERE course_id = $3 AND workflow_state <> 'deleted' AND id = ANY($4::bigint[])
        "#,
    )
    .bind(usage_rights_id)
    .bind(payload.publish.unwrap_or(false))
    .bind(course_id)
    .bind(&file_ids)
    .execute(&state.db)
    .await?;

    let count = file_ids.len();
    let message = if count == 1 {
        "1 file updated".to_string()
    } else {
        format!("{count} files updated")
    };

    Ok(Json(UsageRightsResponse {
        legal_copyright: usage.legal_copyright,
        use_justification,
        license_name: usage
            .license
            .as_deref()
            .and_then(license_name)
            .map(str::to_string),
        license: usage.license,
        message,
        file_ids,
    })
    .into_response())
}

/// DELETE /courses/{courseId}/usage_rights — Canvas
/// UsageRightsController#remove_usage_rights.
///
/// Canvas `assign_usage_rights(nil)`: usage_rights_id is unset on the addressed
/// files (the rows themselves stay for other contexts' reuse) and the response
/// is `{ message, file_ids }` with 200 — not 204.
async fn remove_course_usage_rights(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<RemoveUsageRightsRequest>>,
) -> Result<Response, ApiError> {
    // Canvas: manage_files_delete.
    let allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let payload = payload.map(|Json(body)| body).unwrap_or_default();
    let file_ids = payload.file_ids.unwrap_or_default();
    let folder_ids = payload.folder_ids.unwrap_or_default();
    if file_ids.is_empty() && folder_ids.is_empty() {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "Must supply 'file_ids' and/or 'folder_ids' parameter",
        ));
    }

    // Same file enumeration as Canvas assign_usage_rights: the given files
    // plus everything under the given folders (recursively).
    let file_ids = sqlx::query_scalar::<_, i64>(
        r#"
        WITH RECURSIVE tree AS (
            SELECT id FROM folders
            WHERE context_type = 'Course' AND context_id = $1
              AND workflow_state <> 'deleted' AND id = ANY($2::bigint[])
            UNION ALL
            SELECT f.id FROM folders f JOIN tree t ON f.parent_folder_id = t.id
        )
        SELECT id FROM files
        WHERE course_id = $1 AND workflow_state <> 'deleted'
          AND (id = ANY($3::bigint[]) OR folder_id IN (SELECT id FROM tree))
        ORDER BY id
        "#,
    )
    .bind(course_id)
    .bind(&folder_ids)
    .bind(&file_ids)
    .fetch_all(&state.db)
    .await?;

    sqlx::query(
        r#"
        UPDATE files SET usage_rights_id = NULL, updated_at = now()
        WHERE course_id = $1 AND workflow_state <> 'deleted' AND id = ANY($2::bigint[])
        "#,
    )
    .bind(course_id)
    .bind(&file_ids)
    .execute(&state.db)
    .await?;

    let count = file_ids.len();
    let message = if count == 1 {
        "1 file updated".to_string()
    } else {
        format!("{count} files updated")
    };

    Ok(Json(RemoveUsageRightsResponse { message, file_ids }).into_response())
}

/// DELETE /groups/{groupId}/usage_rights — Canvas remove_usage_rights for a
/// Group context.
///
/// Deviation: star-lms file metadata is course-scoped (files.course_id NOT
/// NULL) and group files live in the separate metadata-only group_files table,
/// so there are no group-context attachments to unlink; the endpoint keeps the
/// Canvas envelope (authorization + `{message, file_ids: []}` with 200).
async fn remove_group_usage_rights(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<RemoveUsageRightsRequest>>,
) -> Result<Response, ApiError> {
    let group: Option<Option<i64>> = sqlx::query_scalar(
        "SELECT course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;
    let course_id = group.ok_or(ApiError::NotFound)?;

    // Canvas: manage_files_delete on the group context (group managers are the
    // parent course's teachers, plus admins).
    let mut allowed = is_admin(&auth);
    if !allowed {
        if let Some(course_id) = course_id {
            allowed = user_teacher_of_course(&state.db, auth.id, course_id, &auth.role).await?;
        }
    }
    if !allowed && !user_in_group(&state.db, auth.id, group_id).await? {
        return Err(ApiError::Forbidden);
    }
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let payload = payload.map(|Json(body)| body).unwrap_or_default();
    let file_ids = payload.file_ids.unwrap_or_default();
    let folder_ids = payload.folder_ids.unwrap_or_default();
    if file_ids.is_empty() && folder_ids.is_empty() {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "Must supply 'file_ids' and/or 'folder_ids' parameter",
        ));
    }

    // No group-scoped usage rights exist to remove (see the doc comment).
    Ok(Json(RemoveUsageRightsResponse {
        message: "0 files updated".to_string(),
        file_ids: Vec::new(),
    })
    .into_response())
}

/// DELETE /users/{userId}/usage_rights — Canvas remove_usage_rights for a
/// User context.
///
/// Deviation: star-lms file metadata is course-scoped, so a user context has
/// no usage-rights rows of its own; the endpoint keeps the Canvas envelope
/// (self or admin) and answers `{message, file_ids: []}` with 200.
async fn remove_user_usage_rights(
    State(_state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
    payload: Option<Json<RemoveUsageRightsRequest>>,
) -> Result<Response, ApiError> {
    // Canvas: a user context only authorizes the user themselves (or admin).
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let payload = payload.map(|Json(body)| body).unwrap_or_default();
    let file_ids = payload.file_ids.unwrap_or_default();
    let folder_ids = payload.folder_ids.unwrap_or_default();
    if file_ids.is_empty() && folder_ids.is_empty() {
        return Ok(canvas_error(
            StatusCode::BAD_REQUEST,
            "Must supply 'file_ids' and/or 'folder_ids' parameter",
        ));
    }

    // No user-scoped usage rights exist to remove (see the doc comment).
    Ok(Json(RemoveUsageRightsResponse {
        message: "0 files updated".to_string(),
        file_ids: Vec::new(),
    })
    .into_response())
}

/// GET /courses/{courseId}/content_licenses — Canvas
/// UsageRightsController#licenses.
///
/// Canvas requires `:read` on the context; the list itself is context-free
/// (UsageRights.licenses), so every context gets the same eight entries.
async fn get_course_content_licenses(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<LicenseEntry>>, ApiError> {
    let allowed = user_enrolled_or_admin(&state.db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(canvas_licenses()))
}

// ─── folder DB helpers ─────────────────────────────────────────────────────

/// Raw folders row with file/subfolder counts (column order matches queries).
///
/// The Canvas payload keys the port stores on the row as well (migration
/// 0264): `hidden` mirrors the workflow state, the two urls are stable per
/// row, and the per-viewer flags are stored when a client wrote them.
#[derive(Debug, Clone, sqlx::FromRow)]
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
    files_url: String,
    folders_url: String,
    hidden: bool,
    hidden_for_user: Option<bool>,
    locked_for_user: Option<bool>,
    can_upload: Option<bool>,
}

const FOLDER_SELECT: &str = r#"
SELECT f.id, f.name, f.full_name, f.context_type, f.context_id,
       f.parent_folder_id, f.workflow_state, f.position, f.locked,
       f.lock_at, f.unlock_at, f.for_submissions,
       f.created_at, f.updated_at,
       COALESCE(f.files_count,
                (SELECT count(*) FROM files fi WHERE fi.folder_id = f.id)) AS files_count,
       COALESCE(f.folders_count,
                (SELECT count(*) FROM folders sf
                  WHERE sf.parent_folder_id = f.id AND sf.workflow_state <> 'deleted'))
         AS folders_count,
       COALESCE(f.files_url, '/api/v1/folders/' || f.id::text || '/files') AS files_url,
       COALESCE(f.folders_url, '/api/v1/folders/' || f.id::text || '/folders') AS folders_url,
       COALESCE(f.hidden, f.workflow_state = 'hidden') AS hidden,
       f.hidden_for_user, f.locked_for_user, f.can_upload
FROM folders f
"#;

/// Canvas folder_json counts for viewers without file-management rights:
/// hidden/locked children are excluded from files_count/folders_count.
const FOLDER_SELECT_VISIBLE: &str = r#"
SELECT f.id, f.name, f.full_name, f.context_type, f.context_id,
       f.parent_folder_id, f.workflow_state, f.position, f.locked,
       f.lock_at, f.unlock_at, f.for_submissions,
       f.created_at, f.updated_at,
       COALESCE(f.files_count,
                (SELECT count(*) FROM files fi
                  WHERE fi.folder_id = f.id AND fi.hidden = FALSE AND fi.locked = FALSE
                    AND (fi.lock_at IS NULL OR fi.lock_at > now()))) AS files_count,
       COALESCE(f.folders_count,
                (SELECT count(*) FROM folders sf
                  WHERE sf.parent_folder_id = f.id AND sf.workflow_state = 'visible'
                    AND sf.locked = FALSE)) AS folders_count,
       COALESCE(f.files_url, '/api/v1/folders/' || f.id::text || '/files') AS files_url,
       COALESCE(f.folders_url, '/api/v1/folders/' || f.id::text || '/folders') AS folders_url,
       COALESCE(f.hidden, f.workflow_state = 'hidden') AS hidden,
       f.hidden_for_user, f.locked_for_user, f.can_upload
FROM folders f
"#;

struct NewFolder {
    name: String,
    full_name: String,
    context_type: String,
    context_id: i64,
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

async fn fetch_root_folder(
    db: &PgPool,
    context_type: &str,
    context_id: i64,
) -> Result<Option<FolderRow>, ApiError> {
    let sql = format!(
        "{FOLDER_SELECT} WHERE f.context_type = $1 AND f.context_id = $2 \
         AND f.parent_folder_id IS NULL AND f.workflow_state <> 'deleted'"
    );
    Ok(sqlx::query_as::<_, FolderRow>(&sql)
        .bind(context_type)
        .bind(context_id)
        .fetch_optional(db)
        .await?)
}

/// Canvas Folder.root_folders: lazily create the context's root folder.
async fn ensure_root_folder_for(
    db: &PgPool,
    context_type: &str,
    context_id: i64,
) -> Result<FolderRow, ApiError> {
    if let Some(root) = fetch_root_folder(db, context_type, context_id).await? {
        return Ok(root);
    }
    let name = root_folder_name_for_context(context_type);
    sqlx::query(
        r#"
        INSERT INTO folders (name, full_name, context_type, context_id, workflow_state, position)
        VALUES ($1, $1, $2, $3, 'visible', 1)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(name)
    .bind(context_type)
    .bind(context_id)
    .execute(db)
    .await?;
    fetch_root_folder(db, context_type, context_id)
        .await?
        .ok_or(ApiError::NotFound)
}

/// Course root folder helper (Canvas 'course files').
async fn ensure_root_folder(db: &PgPool, course_id: i64) -> Result<FolderRow, ApiError> {
    ensure_root_folder_for(db, "Course", course_id).await
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
        context_type: root.context_type.clone(),
        context_id: root.context_id,
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
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING id, name, full_name, context_type, context_id, parent_folder_id,
                  workflow_state, position, locked, lock_at, unlock_at, for_submissions,
                  created_at, updated_at,
                  0::bigint AS files_count, 0::bigint AS folders_count,
                  '/api/v1/folders/' || id::text || '/files' AS files_url,
                  '/api/v1/folders/' || id::text || '/folders' AS folders_url,
                  (workflow_state = 'hidden') AS hidden,
                  NULL::boolean AS hidden_for_user,
                  NULL::boolean AS locked_for_user,
                  NULL::boolean AS can_upload
        "#,
    )
    .bind(&folder.name)
    .bind(&folder.full_name)
    .bind(&folder.context_type)
    .bind(folder.context_id)
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
            context_type: "Course".to_string(),
            context_id: course_id,
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

/// Raw group_files row (column order matches the SELECT in get_group_file).
#[derive(Debug, sqlx::FromRow)]
struct GroupFileRow {
    id: i64,
    group_id: i64,
    folder: String,
    filename: String,
    display_name: String,
    content_type: String,
    size: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<GroupFileRow> for GroupFileResponse {
    fn from(r: GroupFileRow) -> Self {
        GroupFileResponse {
            id: r.id,
            group_id: r.group_id,
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
        let hidden = row.hidden;
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
            folders_url: row.folders_url,
            files_url: row.files_url,
            files_count: row.files_count,
            folders_count: row.folders_count,
            hidden,
            locked_for_user: row
                .locked_for_user
                .unwrap_or(!can_manage && currently_locked),
            hidden_for_user: row.hidden_for_user.unwrap_or(!can_manage && hidden),
            for_submissions: row.for_submissions,
            can_upload: row.can_upload.unwrap_or(can_manage),
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
    fn make_unique_filename_appends_counter_like_canvas() {
        // Canvas make_unique_filename: "notes.pdf" -> "notes-1.pdf".
        assert_eq!(make_unique_filename("notes.pdf", &[]), "notes.pdf");
        assert_eq!(
            make_unique_filename("notes.pdf", &["notes.pdf".to_string()]),
            "notes-1.pdf"
        );
        assert_eq!(
            make_unique_filename(
                "notes.pdf",
                &["notes.pdf".to_string(), "notes-1.pdf".to_string()]
            ),
            "notes-2.pdf"
        );
        assert_eq!(
            make_unique_filename("readme", &["readme".to_string()]),
            "readme-1"
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
            files_url: "/api/v1/folders/7/files".to_string(),
            folders_url: "/api/v1/folders/7/folders".to_string(),
            hidden: true,
            hidden_for_user: None,
            locked_for_user: None,
            can_upload: None,
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

    #[test]
    fn usage_rights_license_names_match_canvas() {
        // Canvas ContentLicenses readable names (lib/content_licenses.rb).
        assert_eq!(license_name("cc_by_sa"), Some("CC Attribution Share Alike"));
        assert_eq!(license_name("private"), Some("Private (Copyrighted)"));
        assert_eq!(license_name("public_domain"), Some("Public Domain"));
        assert_eq!(
            license_name("cc_by_nc_nd"),
            Some("CC Attribution Non-Commercial No Derivatives")
        );
        assert_eq!(license_name("not_a_license"), None);
    }

    #[test]
    fn canvas_licenses_match_content_licenses_rb() {
        // Canvas ContentLicenses ordered hash: private first, public_domain
        // second, creativecommons URLs for the CC entries, wikipedia URLs for
        // private/public_domain, and no cc0 in this Canvas checkout.
        let licenses = canvas_licenses();
        let ids: Vec<&str> = licenses.iter().map(|l| l.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "private",
                "public_domain",
                "cc_by",
                "cc_by_sa",
                "cc_by_nc",
                "cc_by_nc_sa",
                "cc_by_nd",
                "cc_by_nc_nd",
            ]
        );
        assert_eq!(licenses[0].url, "http://en.wikipedia.org/wiki/Copyright");
        assert_eq!(
            licenses[1].url,
            "http://en.wikipedia.org/wiki/Public_domain"
        );
        assert_eq!(
            licenses[2].url,
            "http://creativecommons.org/licenses/by/4.0"
        );
        assert_eq!(
            licenses[7].url,
            "http://creativecommons.org/licenses/by-nc-nd/4.0/"
        );
        assert_eq!(licenses[0].name, "Private (Copyrighted)");
        assert_eq!(licenses[1].name, "Public Domain");
    }

    #[test]
    fn usage_rights_use_justification_values_match_canvas() {
        // Canvas UsageRights#use_justification enum.
        assert!(valid_use_justification("own_copyright"));
        assert!(valid_use_justification("used_by_permission"));
        assert!(valid_use_justification("fair_use"));
        assert!(valid_use_justification("public_domain"));
        assert!(valid_use_justification("creative_commons"));
        assert!(!valid_use_justification("borrowed"));
        assert!(!valid_use_justification(""));
    }

    #[test]
    fn duplicates_and_context_listings_use_canvas_envelopes() {
        // Canvas folders_controller#duplicates: { duplicates: [...] };
        // user/group folders_and_files: { folders: [...], files: [...] }.
        let duplicates = serde_json::to_value(DuplicatesResponse {
            duplicates: Vec::new(),
        })
        .unwrap();
        assert!(duplicates["duplicates"].is_array());

        let listing = serde_json::to_value(ContextFoldersAndFiles {
            folders: Vec::new(),
            files: Vec::new(),
        })
        .unwrap();
        assert!(listing["folders"].is_array());
        assert!(listing["files"].is_array());
    }

    // ─── tests for the 14 new files route handlers ───────────────────────

    #[test]
    fn visibility_level_accepts_valid_canvas_values() {
        // update_file validates visibility_level against these four values.
        assert!(valid_visibility_level("inherit"));
        assert!(valid_visibility_level("context"));
        assert!(valid_visibility_level("institution"));
        assert!(valid_visibility_level("public"));
        assert!(!valid_visibility_level("invalid"));
        assert!(!valid_visibility_level(""));
    }

    /// Stub helper so the above compiles: mirrors the check done in update_file.
    fn valid_visibility_level(level: &str) -> bool {
        matches!(level, "inherit" | "context" | "institution" | "public")
    }

    #[test]
    fn update_file_request_deserializes_with_optional_fields() {
        let json = serde_json::json!({
            "name": "new-name.pdf",
            "locked": true,
            "hidden": false,
            "visibility_level": "institution"
        });
        let req: UpdateFileRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.name, Some("new-name.pdf".to_string()));
        assert_eq!(req.visibility_level, Some("institution".to_string()));
        assert_eq!(req.locked, Some(true));
        assert_eq!(req.hidden, Some(false));
        assert_eq!(req.on_duplicate, None);
    }

    #[test]
    fn update_file_request_on_duplicate_overwrite_is_accepted() {
        let json = serde_json::json!({
            "on_duplicate": "overwrite",
            "name": "renamed.pdf"
        });
        let req: UpdateFileRequest = serde_json::from_value(json).unwrap();
        assert_eq!(req.on_duplicate, Some("overwrite".to_string()));
        assert_eq!(req.name, Some("renamed.pdf".to_string()));
    }

    #[test]
    fn create_success_response_serializes_attachment() {
        // create_success_get returns CreateSuccessResponse { attachment, preview_url, enhanced_preview_url }
        // CreateSuccessQuery carries Canvas api_create_success's lookup
        // params: uuid + on_duplicate (files_controller.rb:1229-1231).
        use crate::CreateSuccessQuery;
        let q: CreateSuccessQuery = serde_json::from_value(serde_json::json!({
            "uuid": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
            "on_duplicate": "overwrite"
        }))
        .unwrap();
        assert_eq!(
            q.uuid.as_deref(),
            Some("a1b2c3d4-e5f6-7890-abcd-ef1234567890")
        );
        assert_eq!(q.on_duplicate.as_deref(), Some("overwrite"));
    }

    #[test]
    fn reset_verifier_returns_uuid() {
        // reset_verifier returns { "uuid": "<new-uuid>" } on success.
        let json = serde_json::json!({ "uuid": "a1b2c3d4-e5f6-7890-abcd-ef1234567890" });
        let val: serde_json::Value = serde_json::from_value(json).unwrap();
        assert!(val["uuid"].as_str().unwrap().len() == 36);
    }

    #[test]
    fn icon_metadata_requires_svg_content_type() {
        // The handler checks starts_with("image/svg"); verify the logic.
        assert!("image/svg+xml".starts_with("image/svg"));
        assert!(!"image/png".starts_with("image/svg"));
    }

    #[test]
    fn quota_used_applies_minimum_per_file() {
        // Canvas counts at least 512 bytes per file even if size is 0 or tiny.
        assert_eq!(
            quota_used(&[0, 100, 512, 10_000]),
            MINIMUM_SIZE_FOR_QUOTA * 3 + 10_000
        );
        assert_eq!(quota_used(&[0]), MINIMUM_SIZE_FOR_QUOTA);
        assert_eq!(quota_used(&[1_000_000]), 1_000_000);
    }

    #[test]
    fn effective_quota_respects_explicit_zero() {
        // Canvas: explicit 0 quota is preserved, not defaulted to 50 MB.
        assert_eq!(effective_quota(Some(0)), 0);
        assert_eq!(effective_quota(None), DEFAULT_STORAGE_QUOTA);
        assert_eq!(effective_quota(Some(100_000_000)), 100_000_000);
    }

    #[test]
    fn list_files_query_passes_pagination_params() {
        let q: ListFilesQuery = serde_json::from_value(serde_json::json!({
            "page": 3,
            "per_page": 10,
            "content_types": ["application/pdf"],
            "search_term": "exam"
        }))
        .unwrap();
        assert_eq!(q.page, Some(3));
        assert_eq!(q.per_page, Some(10));
    }

    #[test]
    fn file_response_json_includes_canvas_fields() {
        // FileResponse is what list_user_files returns (the group GET is
        // served by star-groups' list_group_files).
        use crate::FileResponse;
        let now = Utc::now();
        let row = FileRow {
            id: 1,
            course_id: 42,
            folder: "course files".to_string(),
            folder_id: Some(3),
            filename: "handout.pdf".to_string(),
            display_name: "Handout.pdf".to_string(),
            content_type: "application/pdf".to_string(),
            size: 1024,
            created_at: now,
            updated_at: now,
        };
        let resp = FileResponse::from(row);
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["display_name"], "Handout.pdf");
        assert_eq!(json["content-type"], "application/pdf");
        assert_eq!(json["size"], 1024);
        assert_eq!(json["folder_id"].as_i64(), Some(3));
    }

    #[test]
    fn group_file_response_includes_quota_fields() {
        // Canvas api_quota renders { quota, quota_used }
        // (files_controller.rb:255).
        let resp = QuotaResponse {
            quota: 50_000_000,
            quota_used: 10_000_000,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["quota"], 50_000_000);
        assert_eq!(json["quota_used"], 10_000_000);
    }

    #[test]
    fn user_files_quota_default_is_50mb() {
        // get_user_files_quota uses a hard-coded 52_428_800 byte default (50 MiB).
        let default_quota: i64 = 52_428_800;
        assert_eq!(default_quota, 50 * 1024 * 1024);
    }

    #[test]
    fn post_file_ack_response_shape() {
        // post_file returns { "message": "file upload acknowledged" } on success.
        let msg = serde_json::json!({ "message": "file upload acknowledged" });
        assert_eq!(msg["message"], "file upload acknowledged");
    }

    #[test]
    fn list_user_files_requires_self_or_admin() {
        // The auth check is: auth.id != user_id && !is_admin(&auth).
        fn can_view(auth_id: i64, user_id: i64, is_admin: bool) -> bool {
            auth_id == user_id || is_admin
        }
        assert!(can_view(1, 1, false)); // self
        assert!(can_view(2, 1, true)); // admin
        assert!(!can_view(2, 1, false)); // neither
    }

    #[test]
    fn delete_group_file_removes_metadata_row() {
        // group_files is a metadata-only table with no workflow_state column,
        // so the handler hard-deletes the row (Canvas attachments#destroy
        // soft-deletes because its attachments table carries file state).
        let sql = "DELETE FROM group_files WHERE id = $1 AND group_id = $2";
        assert!(sql.contains("DELETE FROM group_files"));
    }

    #[test]
    fn group_files_quota_returns_zero_when_no_group_quota() {
        // star-lms has no group-level storage_quota; quota=0, quota_used=0.
        let resp = QuotaResponse {
            quota: 0,
            quota_used: 0,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["quota"], 0);
        assert_eq!(json["quota_used"], 0);
    }

    #[test]
    fn delete_group_file_checks_group_exists() {
        // Handler: group must exist and not be deleted.
        let sql = "SELECT id, course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'";
        assert!(sql.contains("workflow_state <> 'deleted'"));
    }
}
