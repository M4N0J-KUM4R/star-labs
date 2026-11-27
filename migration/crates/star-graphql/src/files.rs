//! File, folder read types + `Course.foldersConnection` and the root
//! `folder(id)` query (Canvas issue #80 domain).
//!
//! Mirrors the conventions of the merged slices: camelCase field names,
//! relay-style connections, opaque cursors, `LIMIT first+1` ordered by id.
//!
//! Canvas shape references: `app/graphql/types/file_type.rb` and
//! `folder_type.rb` in canvas-lms. Fields without a backing star-lms
//! column are omitted and documented per type:
//! - `File`: `wordCount` / `viewedAt` / `thumbnailUrl` /
//!   `submissionPreviewUrl` (no columns; 0005/0016/0036/0061); `mimeClass`
//!   is derived from `content_type` (first segment), like Canvas's
//!   `MimeClass` conversion; `size` is the byte size as a string
//!   (Canvas renders `size` humanized but the GraphQL field is String;
//!   star-lms emits the raw byte count for fidelity); `folder` resolves
//!   through `files.folder_id` (0036).
//! - `Folder`: `hidden` maps to `workflow_state == 'hidden'` (Canvas
//!   folders use visible/hidden states); `currentlyLocked` /
//!   `canUpload` / `rootFolder` are derived; `usageRights` is omitted
//!   until a rights column lands on folders.
//!
//! Authorization:
//! - `folder(id)` requires the caller to be enrolled in the folder's
//!   context course (any role) or an admin; unknown ids resolve to null.
//! - `Folder.files` / `subFolders` inherit the folder's visibility.
//!
//! Authentication itself is enforced before execution by the router's
//! `AuthUser` extractor (HTTP 401), so every resolver here can assume an
//! authenticated caller.

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{
    db_err, decode_cursor, encode_cursor, GraphQLContext, PageInfo, DEFAULT_PAGE_SIZE,
    MAX_PAGE_SIZE,
};

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

/// Row of the `files` table (migrations 0005 + 0016 + 0036 + 0061).
#[derive(Debug, Clone, sqlx::FromRow)]
struct FileRow {
    id: i64,
    filename: String,
    display_name: String,
    content_type: String,
    size: i64,
    folder_id: Option<i64>,
    workflow_state: String,
    locked: bool,
    lock_at: Option<DateTime<Utc>>,
    unlock_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Row of the `folders` table (migration 0036).
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
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `File` (subset of `app/graphql/types/file_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `_id`,
/// `displayName`, `contentType`, `mimeClass`, `size`, `sizeBytes`,
/// `fileState`, `locked`, `lockAt`, `unlockAt`, `folder`, `createdAt`,
/// `updatedAt`. Omitted Canvas fields without backing columns: see the
/// module doc above.
#[derive(Debug, Clone)]
pub struct File {
    pub(crate) id: i64,
    pub(crate) filename: String,
    pub(crate) display_name: String,
    pub(crate) content_type: String,
    pub(crate) size: i64,
    pub(crate) folder_id: Option<i64>,
    pub(crate) workflow_state: String,
    pub(crate) locked: bool,
    pub(crate) lock_at: Option<DateTime<Utc>>,
    pub(crate) unlock_at: Option<DateTime<Utc>>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl File {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `File._id`: the legacy canvas id (file_type.rb:40).
    #[graphql(name = "_id")]
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Canvas `File.title` (schema.graphql): the item title, like every
    /// ModuleItemInterface member; files use the filename.
    async fn title(&self) -> &str {
        &self.filename
    }

    async fn content_type(&self) -> &str {
        &self.content_type
    }

    /// Canvas `File.mimeClass` (file_type.rb:46): the coarse class of the
    /// content type (image, video, document, ...), derived from the first
    /// segment like Canvas's MimeClass conversion.
    async fn mime_class(&self) -> &str {
        match self.content_type.split('/').next().unwrap_or("file") {
            "image" => "image",
            "video" => "video",
            "audio" => "audio",
            "text" => "text",
            "application" if self.content_type.contains("pdf") => "pdf",
            "application" if self.content_type.contains("zip") => "zip",
            _ => "file",
        }
    }

    /// Canvas `File.size` is a String (file_type.rb:60); star-lms emits the
    /// raw byte count, like the REST files API.
    async fn size(&self) -> String {
        self.size.to_string()
    }

    async fn size_bytes(&self) -> i64 {
        self.size
    }

    async fn file_state(&self) -> &str {
        &self.workflow_state
    }

    async fn locked(&self) -> bool {
        self.locked
    }

    async fn lock_at(&self) -> Option<DateTime<Utc>> {
        self.lock_at
    }

    async fn unlock_at(&self) -> Option<DateTime<Utc>> {
        self.unlock_at
    }

    /// Canvas `File.folder` (file_type.rb:82).
    async fn folder(&self, ctx: &Context<'_>) -> Result<Option<Folder>, async_graphql::Error> {
        let Some(folder_id) = self.folder_id else {
            return Ok(None);
        };
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_folder(&gctx.pool, folder_id).await
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Canvas GraphQL `Folder` (subset of `app/graphql/types/folder_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `_id`,
/// `contextId`, `contextType`, `name`, `fullName`, `parentFolderId`,
/// `parentFolder`, `position`, `hidden`, `locked`, `lockAt`, `unlockAt`,
/// `currentlyLocked`, `rootFolder`, `filesCount`, `foldersCount`, `files`,
/// `subFolders`, `createdAt`, `updatedAt`.
#[derive(Debug, Clone)]
pub struct Folder {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) full_name: String,
    pub(crate) context_type: String,
    pub(crate) context_id: i64,
    pub(crate) parent_folder_id: Option<i64>,
    pub(crate) workflow_state: String,
    pub(crate) position: i32,
    pub(crate) locked: bool,
    pub(crate) lock_at: Option<DateTime<Utc>>,
    pub(crate) unlock_at: Option<DateTime<Utc>>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl Folder {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `Folder._id`: the legacy canvas id (folder_type.rb:29).
    #[graphql(name = "_id")]
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn context_id(&self) -> ID {
        ID(self.context_id.to_string())
    }

    async fn context_type(&self) -> &str {
        &self.context_type
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn full_name(&self) -> &str {
        &self.full_name
    }

    async fn parent_folder_id(&self) -> Option<ID> {
        self.parent_folder_id.map(|id| ID(id.to_string()))
    }

    /// Canvas `Folder.parentFolder` (schema.graphql).
    async fn parent_folder(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<Folder>, async_graphql::Error> {
        let Some(parent_id) = self.parent_folder_id else {
            return Ok(None);
        };
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_folder(&gctx.pool, parent_id).await
    }

    async fn position(&self) -> i32 {
        self.position
    }

    /// Canvas `Folder.hidden` — star-lms folders use the Canvas folder
    /// states (visible/hidden), not a boolean column.
    async fn hidden(&self) -> bool {
        self.workflow_state == "hidden"
    }

    async fn locked(&self) -> bool {
        self.locked
    }

    async fn lock_at(&self) -> Option<DateTime<Utc>> {
        self.lock_at
    }

    async fn unlock_at(&self) -> Option<DateTime<Utc>> {
        self.unlock_at
    }

    /// Canvas `Folder.currentlyLocked`: locked now or scheduled to lock
    /// (schema.graphql, non-null).
    async fn currently_locked(&self) -> bool {
        self.locked || self.lock_at.map(|t| t <= Utc::now()).unwrap_or(false)
    }

    /// Canvas `Folder.rootFolder`: no parent folder (schema.graphql).
    async fn root_folder(&self) -> bool {
        self.parent_folder_id.is_none()
    }

    async fn files_count(&self, ctx: &Context<'_>) -> Result<i64, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let (count,): (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*) FROM files
               WHERE folder_id = $1 AND workflow_state != 'deleted'"#,
        )
        .bind(self.id)
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(count)
    }

    async fn folders_count(&self, ctx: &Context<'_>) -> Result<i64, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let (count,): (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*) FROM folders
               WHERE parent_folder_id = $1 AND workflow_state != 'deleted'"#,
        )
        .bind(self.id)
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(count)
    }

    /// Canvas `Folder.files` (folder_type.rb:52) — a plain list, ordered
    /// by id for determinism.
    async fn files(&self, ctx: &Context<'_>) -> Result<Vec<File>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let rows: Vec<FileRow> = sqlx::query_as(
            r#"
            SELECT id, filename, display_name, content_type, size, folder_id,
                   workflow_state, locked, lock_at, unlock_at, created_at, updated_at
            FROM files
            WHERE folder_id = $1 AND workflow_state != 'deleted'
            ORDER BY id
            "#,
        )
        .bind(self.id)
        .fetch_all(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(file_from_row).collect())
    }

    /// Canvas `Folder.subFolders` (folder_type.rb:59).
    async fn sub_folders(&self, ctx: &Context<'_>) -> Result<Vec<Folder>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let rows: Vec<FolderRow> = sqlx::query_as(
            r#"
            SELECT id, name, full_name, context_type, context_id, parent_folder_id,
                   workflow_state, position, locked, lock_at, unlock_at, created_at, updated_at
            FROM folders
            WHERE parent_folder_id = $1 AND workflow_state != 'deleted'
            ORDER BY id
            "#,
        )
        .bind(self.id)
        .fetch_all(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(folder_from_row).collect())
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

#[derive(Debug, Clone, SimpleObject)]
pub struct FolderEdge {
    pub cursor: String,
    pub node: Folder,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct FolderConnection {
    pub edges: Vec<FolderEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn file_from_row(row: FileRow) -> File {
    File {
        id: row.id,
        filename: row.filename,
        display_name: row.display_name,
        content_type: row.content_type,
        size: row.size,
        folder_id: row.folder_id,
        workflow_state: row.workflow_state,
        locked: row.locked,
        lock_at: row.lock_at,
        unlock_at: row.unlock_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn folder_from_row(row: FolderRow) -> Folder {
    Folder {
        id: row.id,
        name: row.name,
        full_name: row.full_name,
        context_type: row.context_type,
        context_id: row.context_id,
        parent_folder_id: row.parent_folder_id,
        workflow_state: row.workflow_state,
        position: row.position,
        locked: row.locked,
        lock_at: row.lock_at,
        unlock_at: row.unlock_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

// ---------------------------------------------------------------------------
// Connection fetches
// ---------------------------------------------------------------------------

/// Fetch one page of a course's root folders: `LIMIT first+1` ordered by
/// id ascending. The extra row drives `hasNextPage`.
pub(crate) async fn fetch_folder_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<FolderConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<FolderRow> = sqlx::query_as(
        r#"
        SELECT id, name, full_name, context_type, context_id, parent_folder_id,
               workflow_state, position, locked, lock_at, unlock_at, created_at, updated_at
        FROM folders
        WHERE context_type = 'Course' AND context_id = $1
          AND parent_folder_id IS NULL
          AND workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(course_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<FolderEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            FolderEdge {
                cursor,
                node: folder_from_row(row),
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(FolderConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch a single folder by id, deleted folders excluded.
pub(crate) async fn fetch_folder(
    pool: &PgPool,
    folder_id: i64,
) -> Result<Option<Folder>, async_graphql::Error> {
    let row: Option<FolderRow> = sqlx::query_as(
        r#"
        SELECT id, name, full_name, context_type, context_id, parent_folder_id,
               workflow_state, position, locked, lock_at, unlock_at, created_at, updated_at
        FROM folders
        WHERE id = $1 AND workflow_state != 'deleted'
        "#,
    )
    .bind(folder_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(folder_from_row))
}

/// `folder(id: ID!)` — Canvas `Query.folder` (query_type.rb:410). The
/// folder's context gates visibility: enrolled users of the context course
/// (any role) and admins; everyone else gets null.
pub(crate) async fn fetch_folder_authorized(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    folder_id: i64,
) -> Result<Option<Folder>, async_graphql::Error> {
    let Some(folder) = fetch_folder(pool, folder_id).await? else {
        return Ok(None);
    };

    if folder.context_type == "Course" {
        if !is_admin && !user_enrolled_in_course(pool, user_id, folder.context_id).await? {
            return Ok(None);
        }
    } else if !is_admin {
        return Ok(None);
    }
    Ok(Some(folder))
}

// ---------------------------------------------------------------------------
// Permission helper (copied from lib.rs to keep the slice self-contained)
// ---------------------------------------------------------------------------

async fn user_enrolled_in_course(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<bool, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND workflow_state != 'deleted'
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Single-record fetch (Canvas `Query.node` / `Query.legacyNode`)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Single-record fetch (Canvas `Query.node` / `Query.legacyNode`)
// ---------------------------------------------------------------------------

/// Canvas `Query.node(type: File)` — a file by legacy id.
pub(crate) async fn fetch_file(
    pool: &PgPool,
    id: i64,
) -> Result<Option<File>, async_graphql::Error> {
    let row: Option<FileRow> = sqlx::query_as(
        r#"
        SELECT id, filename, display_name, content_type, size, folder_id,
               workflow_state, locked, lock_at, unlock_at, created_at, updated_at
        FROM files
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(file_from_row))
}
