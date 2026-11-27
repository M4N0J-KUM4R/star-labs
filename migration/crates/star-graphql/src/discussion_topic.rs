//! Discussion object type + `Course.discussionsConnection` (issue #48).
//!
//! Mirrors the conventions of the merged foundation slice in `lib.rs` and the
//! assignment slice in `assignment.rs`: camelCase field names via the
//! Object impl (`contextId`, `contextType`, `published`,
//! `discussionType`, `createdAt`), relay-style connection with
//! `edges { cursor node }` + `pageInfo { hasNextPage endCursor }`, opaque
//! cursors, `LIMIT first+1` ordered by id.
//!
//! Canvas shape reference: `app/graphql/types/discussion_type.rb` (Canvas's
//! `graphql_name` is "Discussion"; star-lms exposes "Discussion" per the
//! issue terminology — field shapes are unaffected). Omitted for missing
//! columns and documented in `specs/gql/submissions-discussions.md`:
//! `isAnnouncement` (no announcement/type column), `author` / `editor`
//! (no `user_id` column), `anonymousState` / `isAnonymousAuthor` (no anonymous
//! columns), `delayedPostAt` / `lockAt` / `editedAt` / `lastReplyAt` /
//! `postedAt` / `position` / … (no columns). The `pinned` column (0105) is
//! deliberately not exposed: Canvas's Discussion type has no `pinned` field —
//! REST-only in star-lms. Discussion entries and their connection are a later
//! slice.
//!
//! Authorization: the connection is only reachable through `course(id)`
//! (enrolled users of any role, or admins; everyone else gets a null course).
//! Inside, `workflow_state <> 'deleted'` is returned to every viewer —
//! identical to Canvas's `Discussion.active` scope and to the
//! star-discussions REST list handler. Canvas's extra per-viewer
//! section/availability scoping (`ScopedToUser`) and the published/unpublished
//! split for students are documented deviations in the spec.

use async_graphql::{Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, decode_cursor, encode_cursor, PageInfo, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

// ---------------------------------------------------------------------------
// Database row (runtime sqlx::query_as + FromRow, like every other crate)
// ---------------------------------------------------------------------------

/// Row of the `discussion_topics` table (migrations 0017 + 0105). `pinned` is
/// not selected: it has no Canvas GraphQL counterpart (REST-only field).
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct TopicRow {
    id: i64,
    course_id: i64,
    title: String,
    message: Option<String>,
    workflow_state: String,
    discussion_type: String,
    locked: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Discussion` (subset of `discussion_type.rb`), exposed as
/// `Discussion`. Field names are camelCased by the Object impl below:
/// `id`, `contextId`, `contextType`, `title`, `message`, `published`,
/// `locked`, `discussionType`, `createdAt`, `updatedAt`,
/// `entriesConnection` (Canvas `discussionEntriesConnection` — the
/// entry list nested under the topic, `DiscussionEntry` in the type
/// inventory).
#[derive(Debug, Clone)]
pub struct Discussion {
    pub id: ID,
    /// Canvas `contextId` — the discussion's context. star-lms topics are
    /// always course-scoped, so this is the course id.
    pub context_id: ID,
    /// Canvas `contextType` — the literal "Course": the table has no Group or
    /// Account contexts (Canvas allows Course | Group).
    pub context_type: String,
    pub title: String,
    pub message: Option<String>,
    /// Canvas `published` — derived from the DB `workflow_state`, which the
    /// REST handlers already map published⇄active/unpublished.
    pub published: bool,
    pub locked: bool,
    /// Canvas `discussionType` (enum `side_comment|threaded|flat|not_threaded`
    /// — the same values the table's CHECK constraint allows).
    pub discussion_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<TopicRow> for Discussion {
    fn from(row: TopicRow) -> Self {
        Discussion {
            id: ID(row.id.to_string()),
            context_id: ID(row.course_id.to_string()),
            context_type: "Course".to_string(),
            title: row.title,
            message: row.message,
            published: row.workflow_state == "active",
            locked: row.locked,
            discussion_type: row.discussion_type,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Manual Object impl (the type used to be a SimpleObject; the nested
/// `entriesConnection` field needs Context, so the derive became an impl).
/// Field names camelCase exactly like the derive did.
#[Object]
impl Discussion {
    async fn id(&self) -> &ID {
        &self.id
    }

    /// Canvas `Discussion._id` — the legacy Canvas id (the type implements
    /// `LegacyIDInterface`).
    #[graphql(name = "_id")]
    async fn legacy_id(&self) -> &ID {
        &self.id
    }

    async fn context_id(&self) -> &ID {
        &self.context_id
    }

    async fn context_type(&self) -> &str {
        &self.context_type
    }

    async fn title(&self) -> &str {
        &self.title
    }

    async fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    async fn published(&self) -> bool {
        self.published
    }

    async fn locked(&self) -> bool {
        self.locked
    }

    async fn discussion_type(&self) -> &str {
        &self.discussion_type
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// The topic's non-deleted entries, paginated (LIMIT first+1, ORDER BY
    /// id). Visibility is inherited from whoever resolved the topic.
    async fn entries_connection(
        &self,
        ctx: &async_graphql::Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<crate::discussion_entries::DiscussionEntryConnection>, async_graphql::Error>
    {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        let raw = self
            .id
            .0
            .parse::<i64>()
            .map_err(|_| async_graphql::Error::new("invalid topic id"))?;
        Ok(Some(
            crate::discussion_entries::fetch_topic_entries_page(
                &gctx.pool,
                raw,
                first,
                after.as_deref(),
            )
            .await?,
        ))
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionEdge {
    pub cursor: String,
    pub node: Discussion,
}

/// Relay connection for discussion topics.
///
/// Canvas `DiscussionConnection`: `edges: [DiscussionEdge]`,
/// `nodes: [Discussion]`, `pageInfo: PageInfo!`.
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionConnection {
    pub edges: Vec<DiscussionEdge>,
    pub nodes: Vec<Discussion>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetch (same pagination shape as fetch_assignment_page)
// ---------------------------------------------------------------------------

/// Fetch one page of a course's discussion topics: `LIMIT first+1` ordered by
/// id ascending (Canvas orders regular discussions by `id: asc`), optionally
/// resuming after a cursor. The extra row drives `hasNextPage`. Deleted
/// topics are excluded (`workflow_state <> 'deleted'`, Canvas's `active`
/// scope); unpublished ones are kept — no per-viewer scoping in this slice.
pub(crate) async fn fetch_discussion_topic_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<DiscussionConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<TopicRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, discussion_type,
               locked, created_at, updated_at
        FROM discussion_topics
        WHERE course_id = $1
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

    let edges: Vec<DiscussionEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            DiscussionEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let nodes: Vec<Discussion> = edges.iter().map(|edge| edge.node.clone()).collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(DiscussionConnection {
        edges,
        nodes,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Canvas `Query.node(type: Discussion)` — a discussion topic by legacy id.
pub(crate) async fn fetch_discussion_topic(
    pool: &PgPool,
    id: i64,
) -> Result<Option<Discussion>, async_graphql::Error> {
    let row: Option<TopicRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, discussion_type,
               locked, created_at, updated_at
        FROM discussion_topics
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(Into::into))
}
