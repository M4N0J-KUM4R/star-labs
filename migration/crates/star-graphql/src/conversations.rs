//! Conversation, conversation message and conversation participant read
//! types + `User.conversationsConnection` (Canvas issue #82 conversations
//! half).
//!
//! Mirrors the conventions of the merged slices: camelCase field names,
//! relay-style connections, opaque cursors, `LIMIT first+1` ordered by id.
//!
//! Canvas shape references: `app/graphql/types/conversation_type.rb`,
//! `conversation_message_type.rb` and `conversation_participant_type.rb`
//! in canvas-lms. Fields without a backing star-lms column are omitted and
//! documented per type:
//! - `Conversation`: `canReply` (no policy column), `contextId` /
//!   `contextType` / `contextName` / `contextAssetString` (star-lms
//!   conversations have no course context, 0020 is a direct-message
//!   inbox); `isPrivate` is true — every star-lms conversation is a
//!   private DM batch like Canvas's default.
//! - `ConversationMessage`: `mediaComment` (no media link), `recipients`
//!   (participant resolution is on the participant type),
//!   `attachments` / `attachmentsConnection` (no message-attachment link).
//! - `ConversationParticipant`: `label` (no column).
//!
//! Authorization: conversations are private. `User.conversationsConnection`
//! returns only the caller's own participation rows (self; admins also see
//! only their own — Canvas's conversations are per-user scoped, there is no
//! admin override in conversation_type.rb). `Conversation` resolvers
//! require the caller to be an active participant of the conversation.
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

/// Row of the `conversations` table (migration 0020).
#[derive(Debug, Clone, sqlx::FromRow)]
struct ConversationRow {
    id: i64,
    subject: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Row of the `conversation_messages` table (migration 0020).
#[derive(Debug, Clone, sqlx::FromRow)]
struct ConversationMessageRow {
    id: i64,
    conversation_id: i64,
    author_id: i64,
    body: String,
    created_at: DateTime<Utc>,
}

/// Row of the `conversation_participants` table (migration 0020).
#[derive(Debug, Clone, sqlx::FromRow)]
struct ConversationParticipantRow {
    id: i64,
    conversation_id: i64,
    user_id: i64,
    workflow_state: String,
    last_message_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Conversation` (subset of
/// `app/graphql/types/conversation_type.rb`). Method names are camelCased
/// by the Object derive: `id`, `_id`, `subject`, `isPrivate`,
/// `conversationMessagesConnection`, `conversationMessagesCount`,
/// `conversationParticipantsConnection`, `createdAt`, `updatedAt`.
#[derive(Debug, Clone)]
pub struct Conversation {
    id: i64,
    subject: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl Conversation {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `Conversation._id`: the legacy canvas id
    /// (conversation_type.rb:28).
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn subject(&self) -> Option<&str> {
        Some(self.subject.as_str())
    }

    /// Canvas `Conversation.isPrivate` (conversation_type.rb:39); every
    /// star-lms conversation is a private DM.
    async fn is_private(&self) -> bool {
        true
    }

    /// Canvas `Conversation.conversationMessagesConnection`
    /// (conversation_type.rb:44) — participants only.
    async fn conversation_messages_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<ConversationMessageConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !caller_participates(&gctx.pool, gctx.auth_user.id, self.id).await? {
            return Ok(None);
        }
        let conn = fetch_message_page(&gctx.pool, self.id, first, after.as_deref()).await?;
        Ok(Some(conn))
    }

    async fn conversation_messages_count(
        &self,
        ctx: &Context<'_>,
    ) -> Result<i64, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = $1")
                .bind(self.id)
                .fetch_one(&gctx.pool)
                .await
                .map_err(db_err)?;
        Ok(count)
    }

    /// Canvas `Conversation.conversationParticipantsConnection`
    /// (conversation_type.rb:76) — participants only.
    async fn conversation_participants_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<ConversationParticipantConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if !caller_participates(&gctx.pool, gctx.auth_user.id, self.id).await? {
            return Ok(None);
        }
        let conn = fetch_participant_page(&gctx.pool, self.id, first, after.as_deref()).await?;
        Ok(Some(conn))
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Canvas GraphQL `ConversationMessage` (subset of
/// `app/graphql/types/conversation_message_type.rb`).
#[derive(Debug, Clone)]
pub struct ConversationMessage {
    id: i64,
    conversation_id: i64,
    author_id: i64,
    body: String,
    created_at: DateTime<Utc>,
}

#[Object]
impl ConversationMessage {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `ConversationMessage._id`
    /// (conversation_message_type.rb:26).
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn body(&self) -> &str {
        &self.body
    }

    async fn conversation_id(&self) -> ID {
        ID(self.conversation_id.to_string())
    }

    /// Canvas `ConversationMessage.author`
    /// (conversation_message_type.rb:31).
    async fn author(&self, ctx: &Context<'_>) -> Result<Option<crate::User>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        crate::fetch_user(&gctx.pool, self.author_id).await
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
}

/// Canvas GraphQL `ConversationParticipant` (subset of
/// `app/graphql/types/conversation_participant_type.rb`).
#[derive(Debug, Clone)]
pub struct ConversationParticipant {
    id: i64,
    conversation_id: i64,
    user_id: i64,
    workflow_state: String,
    last_message_at: Option<DateTime<Utc>>,
}

#[Object]
impl ConversationParticipant {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `ConversationParticipant._id`
    /// (conversation_participant_type.rb:26).
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn user_id(&self) -> ID {
        ID(self.user_id.to_string())
    }

    /// Canvas `ConversationParticipant.user`
    /// (conversation_participant_type.rb:33).
    async fn user(&self, ctx: &Context<'_>) -> Result<Option<crate::User>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        crate::fetch_user(&gctx.pool, self.user_id).await
    }

    /// Canvas `ConversationParticipant.conversation`
    /// (conversation_participant_type.rb:48).
    async fn conversation(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<Conversation>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_conversation(&gctx.pool, self.conversation_id).await
    }

    /// Canvas `ConversationParticipant.subscribed` is non-null
    /// (conversation_participant_type.rb:28); star-lms has no subscribe
    /// column yet — active participants are subscribed, like the REST
    /// inbox default.
    async fn subscribed(&self) -> bool {
        self.workflow_state == "active"
    }

    async fn workflow_state(&self) -> &str {
        &self.workflow_state
    }

    async fn updated_at(&self) -> Option<DateTime<Utc>> {
        self.last_message_at
    }
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationParticipantEdge {
    pub cursor: String,
    pub node: ConversationParticipant,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationParticipantConnection {
    pub edges: Vec<ConversationParticipantEdge>,
    pub page_info: PageInfo,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationMessageEdge {
    pub cursor: String,
    pub node: ConversationMessage,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ConversationMessageConnection {
    pub edges: Vec<ConversationMessageEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetches
// ---------------------------------------------------------------------------

/// Fetch one page of a user's conversation participations
/// (`User.conversationsConnection`, user_type.rb:431): `LIMIT first+1`
/// ordered by id.
pub(crate) async fn fetch_user_conversations_page(
    pool: &PgPool,
    user_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<ConversationParticipantConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<ConversationParticipantRow> = sqlx::query_as(
        r#"
        SELECT id, conversation_id, user_id, workflow_state, last_message_at
        FROM conversation_participants
        WHERE user_id = $1
          AND workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
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

    let edges: Vec<ConversationParticipantEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            ConversationParticipantEdge {
                cursor,
                node: ConversationParticipant {
                    id: row.id,
                    conversation_id: row.conversation_id,
                    user_id: row.user_id,
                    workflow_state: row.workflow_state,
                    last_message_at: row.last_message_at,
                },
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(ConversationParticipantConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch one page of a conversation's messages: `LIMIT first+1` by id.
async fn fetch_message_page(
    pool: &PgPool,
    conversation_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<ConversationMessageConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<ConversationMessageRow> = sqlx::query_as(
        r#"
        SELECT id, conversation_id, author_id, body, created_at
        FROM conversation_messages
        WHERE conversation_id = $1
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(conversation_id)
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
                node: ConversationMessage {
                    id: row.id,
                    conversation_id: row.conversation_id,
                    author_id: row.author_id,
                    body: row.body,
                    created_at: row.created_at,
                },
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(ConversationMessageConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch one page of a conversation's participants: `LIMIT first+1` by id.
async fn fetch_participant_page(
    pool: &PgPool,
    conversation_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<ConversationParticipantConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<ConversationParticipantRow> = sqlx::query_as(
        r#"
        SELECT id, conversation_id, user_id, workflow_state, last_message_at
        FROM conversation_participants
        WHERE conversation_id = $1
          AND workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(conversation_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<ConversationParticipantEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            ConversationParticipantEdge {
                cursor,
                node: ConversationParticipant {
                    id: row.id,
                    conversation_id: row.conversation_id,
                    user_id: row.user_id,
                    workflow_state: row.workflow_state,
                    last_message_at: row.last_message_at,
                },
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(ConversationParticipantConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch a single conversation by id.
async fn fetch_conversation(
    pool: &PgPool,
    conversation_id: i64,
) -> Result<Option<Conversation>, async_graphql::Error> {
    let row: Option<ConversationRow> = sqlx::query_as(
        r#"
        SELECT id, subject, workflow_state, created_at, updated_at
        FROM conversations
        WHERE id = $1
        "#,
    )
    .bind(conversation_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(|row| Conversation {
        id: row.id,
        subject: row.subject,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

/// Whether the user has an active participation row in the conversation.
async fn caller_participates(
    pool: &PgPool,
    user_id: i64,
    conversation_id: i64,
) -> Result<bool, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM conversation_participants
           WHERE conversation_id = $1 AND user_id = $2
             AND workflow_state != 'deleted'
           LIMIT 1"#,
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.is_some())
}
