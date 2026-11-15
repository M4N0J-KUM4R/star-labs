//! Conversations (inbox), course calendar events, and the account overview
//! read types (issue #82).
//!
//! Mirrors the conventions of the foundation slice in `lib.rs` and the
//! merged `assignment.rs` / `terms_sections.rs` slices: camelCase field
//! names via the `#[Object]`/`SimpleObject` derives, relay-style connections
//! with `edges { cursor node }` + `pageInfo { hasNextPage endCursor }`,
//! opaque hex cursors, `LIMIT first+1` ordered by id ascending, `state` as
//! the Canvas renaming of the DB `workflow_state` column.
//!
//! Canvas shape references: `app/graphql/types/conversation_type.rb` and
//! `account_type.rb` in canvas-lms. `calendar_event_type.rb` was NOT found
//! in the reference checkout, so the `CalendarEvent` contract here is the
//! migration 0019 column set plus the REST calendar semantics
//! (`star-calendar`: soft-deleted events are excluded, course events are
//! listed for enrolled users) — documented deviation, to be revisited when
//! a Canvas calendar GraphQL type lands in the reference.
//!
//! Authorization:
//! - Conversations are strictly per-user: `User.conversationsConnection`
//!   returns the connection only when the caller is that user (null for
//!   everyone else, admins included — Canvas mounts conversations under the
//!   legacy inbox of the *current* user, so there is no other-user inbox
//!   view; documented deviation from the usual self-or-admin rule). The
//!   page query only returns conversations where the caller is an ACTIVE
//!   participant and the conversation itself is active (REST parity with
//!   star-conversations' list handler). `conversationMessagesConnection`
//!   omits messages the viewer removed from their copy of the thread
//!   (REST parity with `conversation_message_removals`).
//! - `Course.calendarEventsConnection` is enrolled-only (any role) or
//!   admin: the connection inherits the `course(id)` gate and re-checks
//!   enrollment as defense in depth, returning null otherwise.
//! - `account(id)` mirrors `course(id)` visibility: global admins and
//!   account members (an `account_users` grant) see the account, everyone
//!   else gets null. The roster/content counts (`memberCount`,
//!   `courseCount`) are admin-gated: null for non-admin members.
//!
//! The account type is deliberately named `AccountOverview` (not `Account`)
//! because the pending issue #28 slice adds `Course.account` with its own
//! `Account` type in `course_user_fields.rs`; this avoids the collision.

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{
    db_err, decode_cursor, encode_cursor, GraphQLContext, PageInfo, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};

// ---------------------------------------------------------------------------
// Database rows (runtime sqlx::query_as + FromRow, like every other crate)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct ConversationRow {
    id: i64,
    subject: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct MessageRow {
    id: i64,
    conversation_id: i64,
    author_id: i64,
    body: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct CalendarEventRow {
    id: i64,
    course_id: i64,
    title: String,
    description: Option<String>,
    start_at: DateTime<Utc>,
    end_at: Option<DateTime<Utc>>,
    workflow_state: String,
    location_name: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct AccountRow {
    id: i64,
    name: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types — conversations
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Conversation` (subset of
/// `app/graphql/types/conversation_type.rb`). Method names are camelCased by
/// the Object derive: `id`, `_id`, `subject`, `state`, `createdAt`,
/// `updatedAt`, `conversationMessagesConnection`.
///
/// `viewer_id` is the user whose connection produced this node; messages are
/// filtered per that viewer (removals), matching the REST detail handler.
#[derive(Debug, Clone)]
pub struct Conversation {
    id: i64,
    subject: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    viewer_id: i64,
}

#[Object]
impl Conversation {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `Conversation._id` — the legacy numeric id. star-lms has no
    /// global relay ids yet, so `id` and `_id` carry the same value.
    #[graphql(name = "_id")]
    async fn legacy_id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn subject(&self) -> &str {
        &self.subject
    }

    /// Canvas renames the DB `workflow_state` to `state`.
    async fn state(&self) -> &str {
        &self.workflow_state
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Per-viewer message list: `LIMIT first+1` ordered by id ascending,
    /// omitting messages the viewer removed from their copy of the thread
    /// (REST parity with star-conversations).
    async fn conversation_messages_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<ConversationMessageConnection, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_conversation_message_page(&gctx.pool, self.id, self.viewer_id, first, after.as_deref())
            .await
    }
}

/// Canvas GraphQL `ConversationMessage` (subset of
/// `app/graphql/types/conversation_message_type.rb`). Field names are
/// camelCased by the SimpleObject derive: `id`, `conversationId`, `authorId`,
/// `body`, `createdAt`.
#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationMessage {
    pub id: ID,
    pub conversation_id: ID,
    pub author_id: ID,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

impl From<MessageRow> for ConversationMessage {
    fn from(row: MessageRow) -> Self {
        ConversationMessage {
            id: ID(row.id.to_string()),
            conversation_id: ID(row.conversation_id.to_string()),
            author_id: ID(row.author_id.to_string()),
            body: row.body,
            created_at: row.created_at,
        }
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationEdge {
    pub cursor: String,
    pub node: Conversation,
}

/// Relay connection for the user's conversations.
#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationConnection {
    pub edges: Vec<ConversationEdge>,
    pub page_info: PageInfo,
}

/// Relay connection edge for messages.
#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationMessageEdge {
    pub cursor: String,
    pub node: ConversationMessage,
}

/// Relay connection for a conversation's messages.
#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationMessageConnection {
    pub edges: Vec<ConversationMessageEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// GraphQL types — calendar events
// ---------------------------------------------------------------------------

/// Course-scoped calendar event. Canvas has no `calendar_event_type.rb` in
/// the reference checkout; the contract is the migration 0019 column set
/// plus the REST calendar semantics (soft-deleted events excluded).
/// Field names are camelCased by the SimpleObject derive: `id`, `courseId`,
/// `title`, `description`, `startAt`, `endAt`, `state`, `locationName`,
/// `createdAt`, `updatedAt`.
#[derive(Debug, Clone, SimpleObject)]
pub struct CalendarEvent {
    pub id: ID,
    pub course_id: ID,
    pub title: String,
    pub description: Option<String>,
    pub start_at: DateTime<Utc>,
    pub end_at: Option<DateTime<Utc>>,
    /// Canvas renames the DB `workflow_state` to `state`.
    pub state: String,
    pub location_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<CalendarEventRow> for CalendarEvent {
    fn from(row: CalendarEventRow) -> Self {
        CalendarEvent {
            id: ID(row.id.to_string()),
            course_id: ID(row.course_id.to_string()),
            title: row.title,
            description: row.description,
            start_at: row.start_at,
            end_at: row.end_at,
            state: row.workflow_state,
            location_name: row.location_name,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Relay connection edge for calendar events.
#[derive(Debug, Clone, SimpleObject)]
pub struct CalendarEventEdge {
    pub cursor: String,
    pub node: CalendarEvent,
}

/// Relay connection for a course's calendar events.
#[derive(Debug, Clone, SimpleObject)]
pub struct CalendarEventConnection {
    pub edges: Vec<CalendarEventEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// GraphQL types — account overview
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Account` overview (subset of
/// `app/graphql/types/account_type.rb`): `id`, `name`, `workflowState`,
/// `createdAt`, `updatedAt`, plus the admin-gated roster/content counts.
/// Named `AccountOverview` — NOT `Account` — because the pending issue #28
/// slice adds `Course.account` with its own `Account` type; this avoids the
/// GraphQL name collision.
///
/// `memberCount` (number of `account_users` grants) and `courseCount`
/// (number of courses in the account) are admin-gated: they are null for
/// non-admin viewers (Canvas gates account roster/analytics data behind
/// admin permissions; plain members only see the account shell).
#[derive(Debug, Clone, SimpleObject)]
pub struct AccountOverview {
    pub id: ID,
    pub name: String,
    pub workflow_state: String,
    pub member_count: Option<i32>,
    pub course_count: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Connection fetches (same pagination shape as fetch_enrollment_page)
// ---------------------------------------------------------------------------

/// Resolve `first`/`after` into a clamped page size and a decoded cursor id.
fn page_params(
    first: Option<i32>,
    after: Option<&str>,
) -> Result<(usize, Option<i64>), async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };
    Ok((page_size, after_id))
}

/// Fetch one page of a user's conversations: every conversation where the
/// user is an ACTIVE participant and the conversation is active (REST parity
/// with star-conversations' list handler), `LIMIT first+1` ordered by id
/// ascending. Ordered by id (not `last_message_at`) so the opaque id cursor
/// is stable — documented deviation, consistent with the foundation slice.
pub(crate) async fn fetch_conversation_page(
    pool: &PgPool,
    user_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<ConversationConnection, async_graphql::Error> {
    let (page_size, after_id) = page_params(first, after)?;

    let mut rows: Vec<ConversationRow> = sqlx::query_as(
        r#"
        SELECT c.id, c.subject, c.workflow_state, c.created_at, c.updated_at
        FROM conversations c
        JOIN conversation_participants cp ON cp.conversation_id = c.id
        WHERE cp.user_id = $1
          AND cp.workflow_state = 'active'
          AND c.workflow_state = 'active'
          AND ($2::bigint IS NULL OR c.id > $2)
        ORDER BY c.id
        LIMIT $3
        "#,
    )
    .bind(user_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<ConversationEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            ConversationEdge {
                cursor,
                node: Conversation {
                    id: row.id,
                    subject: row.subject,
                    workflow_state: row.workflow_state,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                    viewer_id: user_id,
                },
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(ConversationConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch one page of a conversation's messages for one viewer: messages the
/// viewer removed from their copy of the thread are omitted (REST parity
/// with star-conversations' detail handler), `LIMIT first+1` ordered by id
/// ascending.
async fn fetch_conversation_message_page(
    pool: &PgPool,
    conversation_id: i64,
    viewer_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<ConversationMessageConnection, async_graphql::Error> {
    let (page_size, after_id) = page_params(first, after)?;

    let mut rows: Vec<MessageRow> = sqlx::query_as(
        r#"
        SELECT cm.id, cm.conversation_id, cm.author_id, cm.body, cm.created_at
        FROM conversation_messages cm
        WHERE cm.conversation_id = $1
          AND NOT EXISTS (
              SELECT 1 FROM conversation_message_removals r
              WHERE r.message_id = cm.id AND r.user_id = $2
          )
          AND ($3::bigint IS NULL OR cm.id > $3)
        ORDER BY cm.id
        LIMIT $4
        "#,
    )
    .bind(conversation_id)
    .bind(viewer_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<ConversationMessageEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            ConversationMessageEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(ConversationMessageConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// `Course.calendarEventsConnection` — enrolled-only (any role) or admin.
/// The connection inherits the `course(id)` gate; this re-check is defense
/// in depth and returns null for anyone who is neither enrolled nor admin.
/// Soft-deleted events are excluded (REST parity); ordered by id ascending
/// for a stable opaque cursor — the REST list orders by `start_at`, a
/// documented deviation consistent with the foundation slice.
pub(crate) async fn course_events_connection(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<Option<CalendarEventConnection>, async_graphql::Error> {
    if !is_admin && !user_enrolled_in_course(pool, user_id, course_id).await? {
        return Ok(None);
    }
    let (page_size, after_id) = page_params(first, after)?;

    let mut rows: Vec<CalendarEventRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, start_at, end_at,
               workflow_state, location_name, created_at, updated_at
        FROM calendar_events
        WHERE course_id = $1
          AND workflow_state <> 'deleted'
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

    let edges: Vec<CalendarEventEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            CalendarEventEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(Some(CalendarEventConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    }))
}

/// `account(id: ID!)` — mirrors `course(id)` visibility: global admins and
/// account members (an `account_users` grant) see the account, everyone
/// else gets null. Soft-deleted accounts are excluded. The admin-gated
/// counts (`memberCount`, `courseCount`) are only populated for admins.
pub(crate) async fn fetch_account(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    id: i64,
) -> Result<Option<AccountOverview>, async_graphql::Error> {
    let row: Option<AccountRow> = sqlx::query_as(
        r#"
        SELECT id, name, workflow_state, created_at, updated_at
        FROM accounts
        WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Ok(None);
    };

    // Gate: admin or account member (account_users grant).
    if !is_admin {
        let member: Option<(i32,)> = sqlx::query_as(
            r#"SELECT 1 FROM account_users
               WHERE account_id = $1 AND user_id = $2
               LIMIT 1"#,
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(db_err)?;
        if member.is_none() {
            return Ok(None);
        }
    }

    // Admin-gated roster/content counts: null for non-admin members.
    let (member_count, course_count) = if is_admin {
        let members: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM account_users WHERE account_id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .map_err(db_err)?;
        let courses: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM courses WHERE account_id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .map_err(db_err)?;
        (Some(members.0 as i32), Some(courses.0 as i32))
    } else {
        (None, None)
    };

    Ok(Some(AccountOverview {
        id: ID(row.id.to_string()),
        name: row.name,
        workflow_state: row.workflow_state,
        member_count,
        course_count,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

// ---------------------------------------------------------------------------
// Permission helper (private copy — see terms_sections.rs for the rationale)
// ---------------------------------------------------------------------------

// Copied verbatim from lib.rs (user_enrolled_in_course) to keep the lib.rs
// diff of this slice minimal across the parallel gql-foundation branches.
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
