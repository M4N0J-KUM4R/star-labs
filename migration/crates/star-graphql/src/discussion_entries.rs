//! DiscussionEntry object type + `DiscussionTopic.entriesConnection`
//! (GraphQL parity gap completion).
//!
//! Mirrors Canvas `app/graphql/types/discussion_entry_type.rb` and the
//! `discussionEntriesConnection` / entries surface on
//! `discussion_topic_type.rb` (Canvas nests entries under the topic; the
//! type inventory name is `DiscussionEntry`). Schema backing is
//! migration 0017 `discussion_entries`: `topic_id` FK, nullable
//! `parent_id` (NULL for root entries), `user_id` (nullable — users are
//! deleted to NULL), `message`, `workflow_state` active|deleted.
//!
//! Canvas shape (subset, camelCase): `id`, `discussionTopicId`,
//! `parentId` (null for roots), `userId` (null when the author was
//! deleted), `message`, `createdAt`, `updatedAt`.
//!
//! Authorization: visibility is inherited from the course /
//! discussionsConnection gate that resolves the topic — anyone who can
//! see the topic sees its non-deleted entries (parity with the
//! star-discussions REST entries listing).

use async_graphql::{SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, decode_cursor, encode_cursor, PageInfo, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

// ---------------------------------------------------------------------------
// Database row
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct EntryRow {
    id: i64,
    topic_id: i64,
    parent_id: Option<i64>,
    user_id: Option<i64>,
    message: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `DiscussionEntry`
/// (`app/graphql/types/discussion_entry_type.rb`, subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionEntry {
    pub id: ID,
    /// Canvas `discussionTopicId` — the owning topic.
    pub discussion_topic_id: ID,
    /// Canvas `parentId` — null for root entries (Canvas root entries
    /// carry a null parent in the flat storage model).
    pub parent_id: Option<ID>,
    /// Canvas `userId` — null when the author was deleted
    /// (`user_id ... ON DELETE SET NULL`).
    pub user_id: Option<ID>,
    pub message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionEntryEdge {
    pub cursor: String,
    pub node: DiscussionEntry,
}

/// Relay connection for discussion entries.
#[derive(Debug, Clone, SimpleObject)]
pub struct DiscussionEntryConnection {
    pub edges: Vec<DiscussionEntryEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetch
// ---------------------------------------------------------------------------

/// Fetch one page of a topic's non-deleted entries: `LIMIT first+1`
/// ordered by id (Canvas orders the flat entry list by creation; id is
/// the stable proxy), optionally resuming after a cursor. The extra row
/// drives `hasNextPage`.
pub(crate) async fn fetch_topic_entries_page(
    pool: &PgPool,
    topic_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<DiscussionEntryConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<EntryRow> = sqlx::query_as(
        r#"
        SELECT id, topic_id, parent_id, user_id, message, created_at, updated_at
        FROM discussion_entries
        WHERE topic_id = $1
          AND workflow_state <> 'deleted'
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(topic_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<DiscussionEntryEdge> = rows
        .into_iter()
        .map(|row| {
            let id = row.id;
            DiscussionEntryEdge {
                cursor: encode_cursor(id),
                node: DiscussionEntry {
                    id: ID(row.id.to_string()),
                    discussion_topic_id: ID(row.topic_id.to_string()),
                    parent_id: row.parent_id.map(|p| ID(p.to_string())),
                    user_id: row.user_id.map(|u| ID(u.to_string())),
                    message: row.message,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(DiscussionEntryConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}
