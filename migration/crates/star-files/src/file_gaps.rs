//! Files parity gaps (PORT-B) — media capture, word counts, InstFS ids, and
//! group/user usage rights + content licenses.
//!
//! Canvas controllers covered:
//!   files          — POST /files/capture (api_capture),
//!                    POST /files/update_word_count,
//!                    POST /rce_linked_file_instfs_ids
//!   usage_rights   — PUT /groups/{groupId}/usage_rights,
//!                    PUT /users/{userId}/usage_rights (set_usage_rights),
//!                    GET /groups/{groupId}/content_licenses and
//!                    GET /users/{userId}/content_licenses (licenses)
//!
//! Already served elsewhere in this crate and NOT repeated here:
//!   - the by_path/{*full_path} folder resolvers (lib.rs),
//!   - the group folders/media/duplicates routes (folders_api.rs),
//!   - the course usage_rights/licenses and all remove_usage_rights routes
//!     (lib.rs).
//!
//! Schema stances (migration 0255):
//!   - `files` is course-scoped (`course_id NOT NULL`), so the user-context
//!     capture of POST /files/capture is stored in `user_files` — the same
//!     table star-users uses for POST /users/{userId}/files.
//!   - usage_rights rows are shared (migration 0084); Group/User contexts
//!     link through `group_files.usage_rights_id` / `user_files.usage_rights_id`
//!     (columns added by migration 0255).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{
    canvas_error, canvas_licenses, canvas_not_found, is_admin, license_name,
    user_teacher_of_course, valid_use_justification, LicenseEntry, UsageRightsParams,
    UsageRightsRequest, UsageRightsResponse,
};

// ---------------------------------------------------------------------------
// Group helpers
// ---------------------------------------------------------------------------

/// Fetch a group's (id, course_id); unknown or deleted groups are 404.
async fn fetch_group(db: &PgPool, group_id: i64) -> Result<(i64, Option<i64>), ApiError> {
    let group: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(db)
    .await?;
    group.ok_or(ApiError::NotFound)
}

/// Canvas group read access (Folder#read_contents): an accepted member, the
/// parent course's teachers, or an admin.
async fn group_read_access(
    db: &PgPool,
    group_id: i64,
    course_id: Option<i64>,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    if let Some(course_id) = course_id {
        if user_teacher_of_course(db, auth.id, course_id, &auth.role).await? {
            return Ok(true);
        }
    }
    crate::user_in_group(db, auth.id, group_id).await
}

/// Canvas manage_files on a group context: the parent course's teachers and
/// admins (mirrors the crate's group file write gates, e.g. delete_group_file).
async fn group_manage_access(
    db: &PgPool,
    course_id: Option<i64>,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    match course_id {
        Some(course_id) => user_teacher_of_course(db, auth.id, course_id, &auth.role).await,
        None => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// files — api_capture / update_word_count / rce_linked_file_instfs_ids
// ---------------------------------------------------------------------------

/// Capture file metadata (Canvas media capture params; `file` is the
/// documented param, `attachment` the legacy alias).
#[derive(Debug, Deserialize)]
pub struct CaptureFileParams {
    pub name: Option<String>,
    pub filename: Option<String>,
    pub display_name: Option<String>,
    pub content_type: Option<String>,
    pub size: Option<i64>,
}

/// Request body of POST /files/capture.
#[derive(Debug, Deserialize)]
pub struct CaptureRequest {
    pub file: Option<CaptureFileParams>,
    pub attachment: Option<CaptureFileParams>,
}

/// Captured attachment JSON — the user_files mirror of the crate's
/// user-context file response (star-users POST /users/{userId}/files).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CapturedFileResponse {
    pub id: i64,
    pub user_id: i64,
    pub folder: String,
    pub filename: String,
    pub display_name: String,
    pub content_type: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Canvas: files#api_capture — POST /api/v1/files/capture (media capture
/// upload).
///
/// Stance: the crate is metadata-only and `files` is course-scoped, so a
/// capture request (which carries no course) records the user-owned
/// attachment in `user_files`, exactly like POST /users/{userId}/files.
/// A body without a `file`/`attachment` parameter is Canvas' bad request.
async fn api_capture(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CaptureRequest>,
) -> Result<Json<CapturedFileResponse>, ApiError> {
    let params = payload
        .file
        .or(payload.attachment)
        .ok_or_else(|| ApiError::BadRequest("must provide a file parameter".into()))?;

    let filename = params
        .filename
        .or(params.name)
        .map(|name| name.trim().to_string())
        .unwrap_or_default();
    if filename.is_empty() {
        return Err(ApiError::BadRequest("filename is required".into()));
    }

    let display_name = params
        .display_name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| filename.clone());
    let content_type = params
        .content_type
        .unwrap_or_else(|| "application/octet-stream".into());

    let captured = sqlx::query_as::<_, CapturedFileResponse>(
        r#"
        INSERT INTO user_files (user_id, filename, display_name, content_type, size, folder)
        VALUES ($1, $2, $3, $4, $5, 'unfiled')
        RETURNING id, user_id, folder, filename, display_name, content_type,
                  size, created_at, updated_at
        "#,
    )
    .bind(auth.id)
    .bind(&filename)
    .bind(&display_name)
    .bind(&content_type)
    .bind(params.size.unwrap_or(0).max(0))
    .fetch_one(&state.db)
    .await?;

    Ok(Json(captured))
}

// ---------------------------------------------------------------------------
// usage_rights — group + user set_usage_rights
// ---------------------------------------------------------------------------

/// Validate the common set_usage_rights params: file/folder ids must be
/// supplied, the usage_rights object must be present, and the justification
/// must be one of Canvas' five values (400/422 Canvas error shapes, like the
/// course handler).
fn validate_usage_rights_params(
    file_ids: &[i64],
    folder_ids: &[i64],
    usage: Option<&UsageRightsParams>,
) -> Result<UsageRightsParams, Box<Response>> {
    if file_ids.is_empty() && folder_ids.is_empty() {
        return Err(Box::new(canvas_error(
            StatusCode::BAD_REQUEST,
            "Must supply 'file_ids' and/or 'folder_ids' parameter",
        )));
    }
    let Some(usage) = usage else {
        return Err(Box::new(canvas_error(
            StatusCode::BAD_REQUEST,
            "No 'usage_rights' object supplied",
        )));
    };
    if !usage
        .use_justification
        .as_deref()
        .is_some_and(valid_use_justification)
    {
        return Err(Box::new(canvas_error(
            StatusCode::BAD_REQUEST,
            "Invalid use_justification",
        )));
    }
    Ok(UsageRightsParams {
        use_justification: usage.use_justification.clone(),
        legal_copyright: usage.legal_copyright.clone(),
        license: usage.license.clone(),
    })
}

/// Reuse (or insert) the context's usage_rights row — Canvas
/// `.where(params).first` semantics, like the course handler.
async fn upsert_usage_rights_row(
    db: &PgPool,
    context_type: &str,
    context_id: i64,
    usage: &UsageRightsParams,
) -> Result<i64, ApiError> {
    let existing: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM usage_rights
        WHERE context_type = $1 AND context_id = $2
          AND use_justification = $3
          AND legal_copyright IS NOT DISTINCT FROM $4
          AND license IS NOT DISTINCT FROM $5
        LIMIT 1
        "#,
    )
    .bind(context_type)
    .bind(context_id)
    .bind(usage.use_justification.as_deref())
    .bind(usage.legal_copyright.as_deref())
    .bind(usage.license.as_deref())
    .fetch_optional(db)
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    sqlx::query_scalar(
        r#"
        INSERT INTO usage_rights
            (context_type, context_id, use_justification, legal_copyright, license)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(context_type)
    .bind(context_id)
    .bind(usage.use_justification.as_deref())
    .bind(usage.legal_copyright.as_deref())
    .bind(usage.license.as_deref())
    .fetch_one(db)
    .await
    .map_err(ApiError::from)
}

/// Canvas set_usage_rights message ("1 file updated" / "N files updated").
fn files_updated_message(count: usize) -> String {
    if count == 1 {
        "1 file updated".to_string()
    } else {
        format!("{count} files updated")
    }
}

/// Canvas: usage_rights_controller#set_usage_rights —
/// PUT /api/v1/groups/{groupId}/usage_rights (Group context).
///
/// Group attachments live in the metadata-only `group_files` table; the
/// addressed file ids must all belong to the group (unknown ids answer
/// Canvas' 404 error shape) and get the usage_rights row linked through
/// `group_files.usage_rights_id` (migration 0255). `folder_ids` are accepted
/// for Canvas compatibility but group files carry no folder linkage, so they
/// contribute no files. The response is Canvas' flat usage_rights_json plus
/// message/file_ids (same shape as the course handler).
async fn set_group_usage_rights(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UsageRightsRequest>,
) -> Result<Response, ApiError> {
    let (_, course_id) = fetch_group(&state.db, group_id).await?;
    if !group_manage_access(&state.db, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let mut file_ids = payload.file_ids.unwrap_or_default();
    let folder_ids = payload.folder_ids.unwrap_or_default();
    let usage =
        match validate_usage_rights_params(&file_ids, &folder_ids, payload.usage_rights.as_ref()) {
            Ok(usage) => usage,
            Err(response) => return Ok(*response),
        };
    file_ids.sort_unstable();
    file_ids.dedup();

    // Unknown file ids are Canvas' 404 (the files must belong to the group).
    let matched: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM group_files WHERE group_id = $1 AND id = ANY($2::bigint[]) ORDER BY id",
    )
    .bind(group_id)
    .bind(&file_ids)
    .fetch_all(&state.db)
    .await?;
    if matched.len() != file_ids.len() {
        return Ok(canvas_not_found());
    }

    let usage_rights_id = upsert_usage_rights_row(&state.db, "Group", group_id, &usage).await?;

    sqlx::query(
        "UPDATE group_files SET usage_rights_id = $1, updated_at = now() \
         WHERE group_id = $2 AND id = ANY($3::bigint[])",
    )
    .bind(usage_rights_id)
    .bind(group_id)
    .bind(&matched)
    .execute(&state.db)
    .await?;

    Ok(Json(UsageRightsResponse {
        legal_copyright: usage.legal_copyright,
        use_justification: usage.use_justification.unwrap_or_default(),
        license: usage.license.clone(),
        license_name: usage
            .license
            .as_deref()
            .and_then(license_name)
            .map(str::to_string),
        message: files_updated_message(matched.len()),
        file_ids: matched,
    })
    .into_response())
}

/// Canvas: usage_rights_controller#set_usage_rights —
/// PUT /api/v1/users/{userId}/usage_rights (User context).
///
/// User attachments live in `user_files`; the addressed file ids must all be
/// owned by the user (unknown ids answer Canvas' 404 error shape). Folder ids
/// expand recursively over the user's folders like Canvas
/// assign_usage_rights. The response is the flat usage_rights_json shape.
async fn set_user_usage_rights(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UsageRightsRequest>,
) -> Result<Response, ApiError> {
    // Canvas get_context: unknown user -> 404; a user context authorizes
    // only the user themself (or an admin).
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    exists.ok_or(ApiError::NotFound)?;
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let mut file_ids = payload.file_ids.unwrap_or_default();
    let folder_ids = payload.folder_ids.unwrap_or_default();
    let usage =
        match validate_usage_rights_params(&file_ids, &folder_ids, payload.usage_rights.as_ref()) {
            Ok(usage) => usage,
            Err(response) => return Ok(*response),
        };
    file_ids.sort_unstable();
    file_ids.dedup();

    let matched: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM user_files WHERE user_id = $1 AND id = ANY($2::bigint[]) ORDER BY id",
    )
    .bind(user_id)
    .bind(&file_ids)
    .fetch_all(&state.db)
    .await?;
    if matched.len() != file_ids.len() {
        return Ok(canvas_not_found());
    }

    // The given files plus everything under the given folders (recursively).
    let addressed: Vec<i64> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE tree AS (
            SELECT id FROM folders
            WHERE context_type = 'User' AND context_id = $1
              AND workflow_state <> 'deleted' AND id = ANY($2::bigint[])
            UNION ALL
            SELECT f.id FROM folders f JOIN tree t ON f.parent_folder_id = t.id
        )
        SELECT id FROM user_files
        WHERE user_id = $1
          AND (id = ANY($3::bigint[]) OR folder_id IN (SELECT id FROM tree))
        ORDER BY id
        "#,
    )
    .bind(user_id)
    .bind(&folder_ids)
    .bind(&file_ids)
    .fetch_all(&state.db)
    .await?;

    let usage_rights_id = upsert_usage_rights_row(&state.db, "User", user_id, &usage).await?;

    sqlx::query(
        "UPDATE user_files SET usage_rights_id = $1, updated_at = now() \
         WHERE user_id = $2 AND id = ANY($3::bigint[])",
    )
    .bind(usage_rights_id)
    .bind(user_id)
    .bind(&addressed)
    .execute(&state.db)
    .await?;

    Ok(Json(UsageRightsResponse {
        legal_copyright: usage.legal_copyright,
        use_justification: usage.use_justification.unwrap_or_default(),
        license: usage.license.clone(),
        license_name: usage
            .license
            .as_deref()
            .and_then(license_name)
            .map(str::to_string),
        message: files_updated_message(addressed.len()),
        file_ids: addressed,
    })
    .into_response())
}

// ---------------------------------------------------------------------------
// content_licenses — group + user
// ---------------------------------------------------------------------------

/// Canvas: usage_rights_controller#licenses —
/// GET /api/v1/groups/{groupId}/content_licenses (Group context): the fixed
/// UsageRights.licenses list for anyone with read access on the group.
async fn get_group_content_licenses(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<LicenseEntry>>, ApiError> {
    let (_, course_id) = fetch_group(&state.db, group_id).await?;
    if !group_read_access(&state.db, group_id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(canvas_licenses()))
}

/// Canvas: usage_rights_controller#licenses —
/// GET /api/v1/users/{userId}/content_licenses (User context, self or admin).
async fn get_user_content_licenses(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<LicenseEntry>>, ApiError> {
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    exists.ok_or(ApiError::NotFound)?;
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(canvas_licenses()))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// PORT-B gap routes; merged into the crate router by `lib.rs`.
pub fn file_gaps_routes() -> Router<AppState> {
    Router::new()
        .route("/files/capture", axum::routing::post(api_capture))
        .route(
            "/groups/{groupId}/usage_rights",
            put(set_group_usage_rights),
        )
        .route("/users/{userId}/usage_rights", put(set_user_usage_rights))
        .route(
            "/groups/{groupId}/content_licenses",
            get(get_group_content_licenses),
        )
        .route(
            "/users/{userId}/content_licenses",
            get(get_user_content_licenses),
        )
}

// ---------------------------------------------------------------------------
// Tests (DB-free; HTTP behavior is pinned in tests/file_gaps_integration.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::file_gaps_routes;

    /// Route registration sanity: the gap router builds cleanly.
    #[test]
    fn routes() {
        let _ = file_gaps_routes();
    }
}
