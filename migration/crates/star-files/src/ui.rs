//! Pub data loaders for the server-rendered UI shell (star-ui, issue #110).
//!
//! Mirrors the `star_discussions::ui` extraction pattern: loaders apply the
//! SAME authorization gates as the REST handlers above and are the single
//! data path for the HTML pages.
//!
//! Canvas parity notes:
//! - Files in this workspace are metadata records (`files`: filename,
//!   display_name, content_type, size); there is no byte storage, so the UI
//!   pages are read-only. Canvas' web `files#create` / `folders#create`
//!   actions do not exist either (`config/routes.rb:114-129` routes them, but
//!   `FilesController` and `FoldersController` define no create) — uploads go
//!   through the API with a multipart attachment.
//! - Hidden / locked / usage-rights state is not modeled in the schema, so
//!   those Canvas columns are omitted (documented deviation).

use sqlx::PgPool;
use star_common::ApiError;

use crate::{user_enrolled_or_admin, AuthUser, DEFAULT_STORAGE_QUOTA, MINIMUM_SIZE_FOR_QUOTA};

/// A folder entry for the UI tree / breadcrumb.
#[derive(Debug, sqlx::FromRow)]
pub struct UiFolder {
    pub id: i64,
    pub name: String,
    pub full_name: String,
}

/// A file entry for the UI table / show page.
#[derive(Debug, sqlx::FromRow)]
pub struct UiFile {
    pub id: i64,
    pub filename: String,
    pub display_name: String,
    pub content_type: String,
    pub size: i64,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// The course root folder (the one `course_files` root for this course).
/// Enrolled-or-admin gated, like the REST course-files list.
pub async fn load_root_folder_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<UiFolder, ApiError> {
    let allowed = user_enrolled_or_admin(db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let row: Option<UiFolder> = sqlx::query_as(
        r#"
        SELECT id, name, full_name FROM folders
        WHERE context_type = 'Course' AND context_id = $1
          AND parent_folder_id IS NULL AND workflow_state <> 'deleted'
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    // Canvas auto-creates a course's root folder the first time Files is
    // opened; mirror that instead of 404ing on a fresh course.
    if row.is_none() {
        sqlx::query(
            r#"
            INSERT INTO folders (name, full_name, context_type, context_id, workflow_state)
            SELECT 'course files', '', 'Course', $1, 'visible'
            WHERE NOT EXISTS (
                SELECT 1 FROM folders
                WHERE context_type = 'Course' AND context_id = $1
                  AND parent_folder_id IS NULL AND workflow_state <> 'deleted'
            )
            "#,
        )
        .bind(course_id)
        .execute(db)
        .await?;
        return sqlx::query_as(
            r#"
            SELECT id, name, full_name FROM folders
            WHERE context_type = 'Course' AND context_id = $1
              AND parent_folder_id IS NULL AND workflow_state <> 'deleted'
            LIMIT 1
            "#,
        )
        .bind(course_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound);
    }
    row.ok_or(ApiError::NotFound)
}

/// Subfolders of a folder, name-ordered (Canvas folders API default sort).
/// Enrolled-or-admin gated via the parent folder's course.
pub async fn load_subfolders_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    folder_id: i64,
) -> Result<Vec<UiFolder>, ApiError> {
    let allowed = user_enrolled_or_admin(db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let rows = sqlx::query_as::<_, UiFolder>(
        r#"
        SELECT id, name, full_name FROM folders
        WHERE parent_folder_id = $1 AND workflow_state <> 'deleted'
        ORDER BY name, id
        "#,
    )
    .bind(folder_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Files of a folder, display-name-ordered. The folder must belong to this
/// course (Canvas scopes the folder-files endpoint to the context).
pub async fn load_files_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    folder_id: i64,
) -> Result<Vec<UiFile>, ApiError> {
    let allowed = user_enrolled_or_admin(db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let folder_ok: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM folders WHERE id = $1 AND context_type = 'Course' \
         AND context_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(folder_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    if folder_ok.is_none() {
        return Err(ApiError::NotFound);
    }

    let rows = sqlx::query_as::<_, UiFile>(
        r#"
        SELECT id, filename, display_name, content_type, size, updated_at FROM files
        WHERE folder_id = $1
        ORDER BY display_name, id
        "#,
    )
    .bind(folder_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Breadcrumb of a folder (root first). Each hop re-checks the course gate.
pub async fn load_breadcrumb_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    folder_id: i64,
) -> Result<Vec<UiFolder>, ApiError> {
    let allowed = user_enrolled_or_admin(db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let mut trail: Vec<UiFolder> = Vec::new();
    let mut current = folder_id;
    loop {
        let row: Option<(i64, String, String, Option<i64>)> = sqlx::query_as(
            "SELECT id, name, full_name, parent_folder_id FROM folders \
             WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(current)
        .fetch_optional(db)
        .await?;
        let (id, name, full_name, parent) = match row {
            Some(r) => r,
            None => break,
        };
        trail.push(UiFolder {
            id,
            name,
            full_name,
        });
        match parent {
            Some(p) => current = p,
            None => break,
        }
    }
    trail.reverse();
    Ok(trail)
}

/// One file for the UI show page; must belong to this course.
pub async fn load_file_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    file_id: i64,
) -> Result<UiFile, ApiError> {
    let allowed = user_enrolled_or_admin(db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let row: Option<UiFile> = sqlx::query_as(
        r#"
        SELECT id, filename, display_name, content_type, size, updated_at FROM files
        WHERE id = $1 AND course_id = $2
        "#,
    )
    .bind(file_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    row.ok_or(ApiError::NotFound)
}

/// Course usage quota for the files page usage bar (Canvas files#api_quota:
/// `courses.storage_quota` with the 50 MB default, usage summing
/// `max(size, MINIMUM_SIZE_FOR_QUOTA)` per file). Enrolled-or-admin gated.
///
/// Returns `(quota, used)` in bytes.
pub async fn load_quota_for_ui(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<(i64, i64), ApiError> {
    let allowed = user_enrolled_or_admin(db, auth.id, course_id, &auth.role).await?;
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let quota_row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT storage_quota FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    let storage_quota = quota_row.ok_or(ApiError::NotFound)?.0;

    let sizes: Vec<i64> = sqlx::query_scalar(
        "SELECT COALESCE(size, 0) FROM files WHERE course_id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_all(db)
    .await?;

    let quota = storage_quota.unwrap_or(DEFAULT_STORAGE_QUOTA);
    let used: i64 = sizes
        .iter()
        .map(|s| std::cmp::max(*s, MINIMUM_SIZE_FOR_QUOTA))
        .sum();
    Ok((quota, used))
}
