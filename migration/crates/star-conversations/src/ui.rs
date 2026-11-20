//! Pub data loaders for the server-rendered inbox (star-ui, issue #72).
//!
//! Same extraction pattern as `star_courses::load_course_for_viewer`: the
//! inbox queries the REST handlers run privately live here as `pub` fns so
//! the shell stays thin and cannot bypass the API's participant gates.
//! `load_conversation_thread` improves on the API's private
//! `load_conversation_detail` by joining participant NAMES (the API shape
//! only returns ids — fine for JSON, useless for server-rendered HTML).
//!
//! Read-state conventions (migrations 0020/0046/0116): a participant row with
//! `read_at IS NULL` is unread; `starred` and `subscribed` are per-participant
//! inbox flags.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::ApiError;

use crate::is_participant;

/// One inbox row for the viewer — Canvas `conversations#index` summary joined
/// with the flags the two-pane inbox renders: unread state, starred flag,
/// participant names, and the last-message preview.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InboxRow {
    pub id: i64,
    pub subject: String,
    pub created_at: DateTime<Utc>,
    /// Last message body preview (NULL when every message was removed for
    /// this viewer).
    pub last_message_preview: Option<String>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub participant_count: i64,
    /// Other active participants' display names, comma-joined in
    /// conversation_participants.id order (empty for a solo thread).
    pub participant_names: String,
    /// `false` when `read_at IS NULL` — the unread inbox style.
    pub starred: bool,
    /// NULL read_at = unread.
    pub read_at: Option<DateTime<Utc>>,
}

/// Which inbox subset `load_inbox` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InboxFilter {
    /// Every active conversation.
    #[default]
    All,
    /// Only conversations with `read_at IS NULL`.
    Unread,
    /// Only conversations the viewer starred.
    Starred,
}

impl InboxFilter {
    /// Parse the `?filter=` query value; unknown values fall back to All.
    pub fn from_str_opt(raw: Option<&str>) -> Self {
        match raw {
            Some("unread") => InboxFilter::Unread,
            Some("starred") => InboxFilter::Starred,
            _ => InboxFilter::All,
        }
    }
}

/// A message in a conversation thread, joined with the author's name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThreadMessageRow {
    pub id: i64,
    pub author_id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
    /// `users.name` of the message author.
    pub author_name: String,
}

/// A participant in a conversation thread, WITH the display name (the API's
/// `ConversationDetail.participants` is ids-only).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ThreadParticipantRow {
    pub user_id: i64,
    pub name: String,
}

/// The full thread for the show page.
#[derive(Debug, Clone)]
pub struct ConversationThread {
    pub id: i64,
    pub subject: String,
    pub created_at: DateTime<Utc>,
    pub participants: Vec<ThreadParticipantRow>,
    pub messages: Vec<ThreadMessageRow>,
    /// The viewer's starred flag.
    pub starred: bool,
    /// The viewer's read_at (NULL = unread).
    pub read_at: Option<DateTime<Utc>>,
}

/// Load the viewer's inbox (active participations only, most recent first).
///
/// Mirrors the API's `list_conversations` scope (participant row active +
/// conversation active) plus the per-user flags from migration 0116, which
/// the JSON summary shape does not carry.
pub async fn load_inbox(
    db: &PgPool,
    viewer_id: i64,
    filter: InboxFilter,
) -> Result<Vec<InboxRow>, ApiError> {
    let filter_clause = match filter {
        InboxFilter::All => "",
        InboxFilter::Unread => " AND cp.read_at IS NULL",
        InboxFilter::Starred => " AND cp.starred",
    };

    // The filter fragment is a compile-time constant selected above, not
    // user input, so the dynamic SQL is safe.
    let sql = format!(
        r#"
        SELECT
            c.id,
            c.subject,
            c.created_at,
            (
                SELECT cm.body FROM conversation_messages cm
                WHERE cm.conversation_id = c.id
                  AND NOT EXISTS (
                      SELECT 1 FROM conversation_message_removals r
                      WHERE r.message_id = cm.id AND r.user_id = $1
                  )
                ORDER BY cm.created_at DESC, cm.id DESC
                LIMIT 1
            ) AS last_message_preview,
            cp.last_message_at,
            (
                SELECT COUNT(*) FROM conversation_participants p2
                WHERE p2.conversation_id = c.id AND p2.workflow_state = 'active'
            ) AS participant_count,
            (
                SELECT COALESCE(STRING_AGG(u.name, ', ' ORDER BY p3.id), '')
                FROM conversation_participants p3
                JOIN users u ON u.id = p3.user_id
                WHERE p3.conversation_id = c.id
                  AND p3.workflow_state = 'active'
                  AND p3.user_id <> $1
            ) AS participant_names,
            cp.starred,
            cp.read_at
        FROM conversations c
        JOIN conversation_participants cp ON cp.conversation_id = c.id
        WHERE cp.user_id = $1
          AND cp.workflow_state = 'active'
          AND c.workflow_state = 'active'{filter_clause}
        ORDER BY cp.last_message_at DESC NULLS LAST, c.created_at DESC
        "#
    );

    let rows: Vec<InboxRow> = sqlx::query_as(&sql).bind(viewer_id).fetch_all(db).await?;

    Ok(rows)
}

/// Load one conversation thread for the viewer: participants WITH names,
/// messages with author names, and the viewer's own starred/read flags.
/// Non-participants get 404 (same non-leaking convention as the API).
pub async fn load_conversation_thread(
    db: &PgPool,
    conversation_id: i64,
    viewer_id: i64,
) -> Result<ConversationThread, ApiError> {
    if !is_participant(db, conversation_id, viewer_id).await? {
        return Err(ApiError::NotFound);
    }

    let conv: Option<(i64, String, DateTime<Utc>, bool, Option<DateTime<Utc>>)> = sqlx::query_as(
        r#"
        SELECT c.id, c.subject, c.created_at, cp.starred, cp.read_at
        FROM conversations c
        JOIN conversation_participants cp ON cp.conversation_id = c.id
        WHERE c.id = $1 AND cp.user_id = $2 AND c.workflow_state = 'active'
        "#,
    )
    .bind(conversation_id)
    .bind(viewer_id)
    .fetch_optional(db)
    .await?;

    let Some((id, subject, created_at, starred, read_at)) = conv else {
        return Err(ApiError::NotFound);
    };

    let participants: Vec<ThreadParticipantRow> = sqlx::query_as(
        r#"
        SELECT p.user_id, u.name
        FROM conversation_participants p
        JOIN users u ON u.id = p.user_id
        WHERE p.conversation_id = $1 AND p.workflow_state = 'active'
        ORDER BY p.id
        "#,
    )
    .bind(conversation_id)
    .fetch_all(db)
    .await?;

    let messages: Vec<ThreadMessageRow> = sqlx::query_as(
        r#"
        SELECT cm.id, cm.author_id, cm.body, cm.created_at, u.name AS author_name
        FROM conversation_messages cm
        JOIN users u ON u.id = cm.author_id
        WHERE cm.conversation_id = $1
          AND NOT EXISTS (
              SELECT 1 FROM conversation_message_removals r
              WHERE r.message_id = cm.id AND r.user_id = $2
          )
        ORDER BY cm.created_at ASC, cm.id ASC
        "#,
    )
    .bind(conversation_id)
    .bind(viewer_id)
    .fetch_all(db)
    .await?;

    Ok(ConversationThread {
        id,
        subject,
        created_at,
        participants,
        messages,
        starred,
        read_at,
    })
}

/// Create a conversation as the viewer (compose). Same semantics as
/// `POST /api/v1/conversations`: conversation + participants (author read,
/// recipients unread) + first message in one transaction. Returns the new
/// conversation id.
pub async fn create_conversation_for_viewer(
    db: &PgPool,
    auth: &AuthUser,
    recipients: &[i64],
    subject: &str,
    body: &str,
) -> Result<i64, ApiError> {
    if recipients.is_empty() {
        return Err(ApiError::Validation("recipients cannot be empty".into()));
    }
    if subject.trim().is_empty() {
        return Err(ApiError::Validation("subject cannot be empty".into()));
    }
    if body.trim().is_empty() {
        return Err(ApiError::Validation("body cannot be empty".into()));
    }

    let mut tx = db.begin().await?;

    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO conversations (subject, workflow_state)
        VALUES ($1, 'active')
        RETURNING id
        "#,
    )
    .bind(subject.trim())
    .fetch_one(&mut *tx)
    .await?;

    // Author first, then recipients — deduped, self excluded (Canvas order).
    // Unknown recipient ids are dropped (Canvas ignores them on compose) so a
    // crafted form POST cannot trip the participants FK into a 500.
    let known: Vec<(i64,)> =
        sqlx::query_as("SELECT id FROM users WHERE id = ANY($1::BIGINT[]) ORDER BY id ASC")
            .bind(recipients)
            .fetch_all(&mut *tx)
            .await?;
    let mut participant_ids = vec![auth.id];
    for (uid,) in known {
        if uid != auth.id && !participant_ids.contains(&uid) {
            participant_ids.push(uid);
        }
    }

    let read_at = Utc::now();
    for &uid in &participant_ids {
        let participant_read_at = if uid == auth.id { Some(read_at) } else { None };
        sqlx::query(
            r#"
            INSERT INTO conversation_participants
                (conversation_id, user_id, workflow_state, last_message_at, read_at)
            VALUES ($1, $2, 'active', now(), $3)
            "#,
        )
        .bind(id)
        .bind(uid)
        .bind(participant_read_at)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        r#"
        INSERT INTO conversation_messages (conversation_id, author_id, body)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .bind(body)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(id)
}

/// Append a message to a conversation — same semantics as
/// `POST /conversations/{id}/add_message`: inserts the message, refreshes
/// `last_message_at` for all active participants, marks the author read and
/// everyone else unread again. Returns the new message id.
pub async fn add_message_for_viewer(
    db: &PgPool,
    conversation_id: i64,
    auth: &AuthUser,
    body: &str,
) -> Result<i64, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::Validation("body cannot be empty".into()));
    }

    if !is_participant(db, conversation_id, auth.id).await? {
        return Err(ApiError::NotFound);
    }

    let mut tx = db.begin().await?;

    let message_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO conversation_messages (conversation_id, author_id, body)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(conversation_id)
    .bind(auth.id)
    .bind(body)
    .fetch_one(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        UPDATE conversation_participants
        SET last_message_at = now(),
            read_at = CASE WHEN user_id = $2 THEN now() ELSE NULL END
        WHERE conversation_id = $1 AND workflow_state = 'active'
        "#,
    )
    .bind(conversation_id)
    .bind(auth.id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(message_id)
}

/// Mark one conversation read/unread for the viewer (participant gate).
pub async fn mark_conversation_read_for_viewer(
    db: &PgPool,
    conversation_id: i64,
    viewer_id: i64,
    read: bool,
) -> Result<(), ApiError> {
    if !is_participant(db, conversation_id, viewer_id).await? {
        return Err(ApiError::NotFound);
    }

    let read_at = if read { Some(Utc::now()) } else { None };
    sqlx::query(
        r#"
        UPDATE conversation_participants
        SET read_at = $3
        WHERE conversation_id = $1 AND user_id = $2 AND workflow_state = 'active'
        "#,
    )
    .bind(conversation_id)
    .bind(viewer_id)
    .bind(read_at)
    .execute(db)
    .await?;

    Ok(())
}

/// Mark every active conversation of the viewer read — the UI equivalent of
/// `POST /conversations/mark_all_as_read`.
pub async fn mark_all_conversations_read(db: &PgPool, viewer_id: i64) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE conversation_participants SET read_at = now() \
         WHERE user_id = $1 AND workflow_state = 'active'",
    )
    .bind(viewer_id)
    .execute(db)
    .await?;
    Ok(())
}

/// The viewer's unread conversation count (badge in the global nav). Same
/// count the `/conversations/unread_count` API serves, as an integer.
pub async fn load_unread_count(db: &PgPool, viewer_id: i64) -> Result<i64, ApiError> {
    let (count,): (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM conversations c
        JOIN conversation_participants cp ON cp.conversation_id = c.id
        WHERE cp.user_id = $1
          AND cp.workflow_state = 'active'
          AND c.workflow_state = 'active'
          AND cp.read_at IS NULL
        "#,
    )
    .bind(viewer_id)
    .fetch_one(db)
    .await?;
    Ok(count)
}

/// A recipient option for the compose form: user id + display label.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RecipientOption {
    pub id: i64,
    pub name: String,
    pub email: String,
}

/// Load the recipient options for the compose multi-select. Everyone except
/// the viewer (Canvas' recipient finder searches all users; the no-JS port
/// offers the full list, capped at 100 like the API's default limit).
pub async fn load_recipient_options(
    db: &PgPool,
    viewer_id: i64,
) -> Result<Vec<RecipientOption>, ApiError> {
    let rows: Vec<RecipientOption> = sqlx::query_as(
        r#"
        SELECT id, name, email
        FROM users
        WHERE id <> $1
        ORDER BY name, id
        LIMIT 100
        "#,
    )
    .bind(viewer_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}
