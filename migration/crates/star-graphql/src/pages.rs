//! Page (wiki page) read type + `Course.pagesConnection`-style access and
//! the `Progress` type with its state enum.
//!
//! Mirrors the conventions of the merged slices: camelCase field names,
//! relay-style connections, opaque cursors, `LIMIT first+1` ordered by id.
//!
//! Canvas shape references: `app/graphql/types/page_type.rb` (fields come
//! from the ModuleItemInterface: `title`, plus TimestampInterface
//! `createdAt`/`updatedAt`, LegacyIDInterface `_id`, Node `id`) and
//! `progress_type.rb`. Canvas fields without a backing star-lms column are
//! omitted and documented per type:
//! - `Page`: `todoDate`, `canDuplicate` / `canUnpublish` /
//!   `canManageAssignTo` (no columns), `graded` / `pointsPossible` /
//!   `type` (pages are not assignments here), `modules` (no content-tag
//!   reverse index exposed yet), `isLockedByMasterCourse` (no master
//!   courses in this phase), `published` maps to
//!   `workflow_state == 'published'` (0013), `body` / `url` /
//!   `frontPage` come from 0001/0013/0075.
//! - `Progress`: `context` union (the raw context_type/context_id pair is
//!   exposed instead), `state` maps workflow_state (0127: queued |
//!   running | completed | failed), `tag`, `completion`, `message` —
//!   all backed by 0127.
//!
//! Authorization: `Course.pagesConnection` inherits `course(id)`
//! visibility (enrolled users, any role, and admins). `Progress` is only
//! reachable through its owning flow (REST progress endpoints); the type
//! is defined here for schema parity and resolved by id.
//!
//! Authentication itself is enforced before execution by the router's
//! `AuthUser` extractor (HTTP 401), so every resolver here can assume an
//! authenticated caller.

use async_graphql::{Enum, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, decode_cursor, encode_cursor, PageInfo, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

/// Row of the `pages` table (migrations 0001 + 0013 + 0075 + 0087).
#[derive(Debug, Clone, sqlx::FromRow)]
struct PageRow {
    id: i64,
    title: String,
    body: Option<String>,
    url: Option<String>,
    workflow_state: String,
    front_page: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Row of the `progress` table (migration 0127).
#[derive(Debug, Clone, sqlx::FromRow)]
struct ProgressRow {
    id: i64,
    tag: String,
    workflow_state: String,
    completion: i32,
    context_type: Option<String>,
    context_id: Option<i64>,
    user_id: Option<i64>,
    message: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas `ProgressState` (progress_type.rb:23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ProgressState {
    #[graphql(name = "queued")]
    Queued,
    #[graphql(name = "running")]
    Running,
    #[graphql(name = "completed")]
    Completed,
    #[graphql(name = "failed")]
    Failed,
}

/// Canvas GraphQL `Page` (subset of `app/graphql/types/page_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `_id`, `title`,
/// `body`, `url`, `published`, `frontPage`, `createdAt`, `updatedAt`.
#[derive(Debug, Clone)]
pub struct Page {
    id: i64,
    title: String,
    body: Option<String>,
    url: Option<String>,
    workflow_state: String,
    front_page: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl Page {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `Page._id`: the legacy canvas id (LegacyIDInterface).
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn title(&self) -> Option<&str> {
        Some(self.title.as_str())
    }

    async fn body(&self) -> Option<&str> {
        self.body.as_deref()
    }

    async fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// Canvas `Page.published`; star-lms stores the publish state in
    /// `workflow_state` ('published', 0013).
    async fn published(&self) -> bool {
        self.workflow_state == "published"
    }

    async fn front_page(&self) -> bool {
        self.front_page
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Canvas GraphQL `Progress` (subset of
/// `app/graphql/types/progress_type.rb`). Method names are camelCased by
/// the Object derive: `id`, `_id`, `tag`, `completion`, `state`,
/// `message`, `contextType`, `contextId`, `createdAt`, `updatedAt`. The
/// Canvas `context` union is flattened to the raw pair — star-lms progress
/// rows are polymorphic (0127) and the union members have no resolvers.
#[derive(Debug, Clone)]
pub struct Progress {
    id: i64,
    user_id: Option<i64>,
    tag: String,
    workflow_state: String,
    completion: i32,
    context_type: Option<String>,
    context_id: Option<i64>,
    message: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl Progress {
    /// Whether the given user owns this progress row (0127 user_id).
    pub(crate) fn owner_is(&self, user_id: i64) -> bool {
        self.user_id == Some(user_id)
    }
}

#[Object]
impl Progress {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `Progress._id`: the legacy canvas id (progress_type.rb:46).
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `Progress.tag`: the type of operation
    /// (progress_type.rb:63).
    async fn tag(&self) -> &str {
        &self.tag
    }

    /// Canvas `Progress.completion`: percent completed
    /// (progress_type.rb:65).
    async fn completion(&self) -> Option<i32> {
        Some(self.completion)
    }

    /// Canvas `Progress.state` renames workflow_state
    /// (progress_type.rb:67).
    async fn state(&self) -> ProgressState {
        match self.workflow_state.as_str() {
            "queued" => ProgressState::Queued,
            "running" => ProgressState::Running,
            "completed" => ProgressState::Completed,
            "failed" => ProgressState::Failed,
            _ => ProgressState::Queued,
        }
    }

    /// Canvas `Progress.message`: details about the job
    /// (progress_type.rb:69).
    async fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    async fn context_type(&self) -> Option<&str> {
        self.context_type.as_deref()
    }

    async fn context_id(&self) -> Option<ID> {
        self.context_id.map(|id| ID(id.to_string()))
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

#[derive(Debug, Clone, SimpleObject)]
pub struct PageEdge {
    pub cursor: String,
    pub node: Page,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct PageConnection {
    pub edges: Vec<PageEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Progress fetch
// ---------------------------------------------------------------------------

/// Fetch a progress row by id. Exposure follows Canvas: `Progress` is an
/// orphan type reached through mutation payloads, never a root query; the
/// REST progress endpoints keep their own handlers. This fetch is the
/// shared resolver body for future mutation payloads (issue #29 batches
/// B–E) so the type is wired into the schema rather than dead code.
pub(crate) async fn fetch_progress(
    pool: &PgPool,
    progress_id: i64,
) -> Result<Option<Progress>, async_graphql::Error> {
    let row: Option<ProgressRow> = sqlx::query_as(
        r#"
        SELECT id, tag, workflow_state, completion, context_type, context_id,
               user_id, message, created_at, updated_at
        FROM progress
        WHERE id = $1
        "#,
    )
    .bind(progress_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| Progress {
        id: row.id,
        user_id: row.user_id,
        tag: row.tag,
        workflow_state: row.workflow_state,
        completion: row.completion,
        context_type: row.context_type,
        context_id: row.context_id,
        message: row.message,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

// ---------------------------------------------------------------------------
// Connection fetches
// ---------------------------------------------------------------------------

/// Fetch one page of a course's wiki pages: `LIMIT first+1` ordered by id
/// ascending. The extra row drives `hasNextPage`.
pub(crate) async fn fetch_page_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<PageConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<PageRow> = sqlx::query_as(
        r#"
        SELECT id, title, body, url, workflow_state, front_page,
               created_at, updated_at
        FROM pages
        WHERE course_id = $1
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

    let edges: Vec<PageEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            PageEdge {
                cursor,
                node: Page {
                    id: row.id,
                    title: row.title,
                    body: row.body,
                    url: row.url,
                    workflow_state: row.workflow_state,
                    front_page: row.front_page,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(PageConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}
