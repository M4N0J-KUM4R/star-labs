//! star-conversations — inbox/DM conversations for star-lms.
//!
//! Canvas API v1 parity (simplified):
//! - POST   /conversations              — create conversation + participants + first message
//! - GET    /conversations              — list conversations for authenticated user
//! - GET    /conversations/{id}         — get full conversation with messages
//! - POST   /conversations/{id}/add_message — append message, update last_message_at
//! - DELETE /conversations/{id}         — soft-delete for this user only

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Conversation metadata row from the DB.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Conversation {
    pub id: i64,
    pub subject: String,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A single message in a conversation.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ConversationMessage {
    pub id: i64,
    pub conversation_id: i64,
    pub author_id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// A participant row.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ConversationParticipant {
    pub id: i64,
    pub conversation_id: i64,
    pub user_id: i64,
    pub workflow_state: String,
    pub last_message_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

/// POST /conversations
#[derive(Debug, Deserialize)]
pub struct CreateConversation {
    pub recipients: Vec<i64>, // user IDs to add as participants
    pub subject: String,
    pub body: String, // first message body
}

/// POST /conversations/{id}/add_message
#[derive(Debug, Deserialize)]
pub struct AddMessage {
    pub body: String,
}

fn validate_recipients(recipients: &[i64]) -> Result<(), ApiError> {
    if recipients.is_empty() {
        return Err(ApiError::Validation("recipients cannot be empty".into()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Response shapes
// ---------------------------------------------------------------------------

/// Response when creating a new conversation.
#[derive(Debug, Serialize)]
pub struct CreatedConversation {
    pub id: i64,
    pub subject: String,
    pub workflow_state: String,
    pub participants: Vec<i64>,
    pub messages: Vec<MessageInResponse>,
    pub created_at: DateTime<Utc>,
}

/// Message included in conversation responses.
#[derive(Debug, Serialize)]
pub struct MessageInResponse {
    pub id: i64,
    pub author_id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// Summary shape for GET /conversations.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ConversationSummary {
    pub id: i64,
    pub subject: String,
    pub workflow_state: String,
    pub last_message_preview: Option<String>,
    pub participant_count: i64,
    pub last_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Full conversation with all messages (GET /conversations/{id}).
#[derive(Debug, Serialize)]
pub struct ConversationDetail {
    pub id: i64,
    pub subject: String,
    pub workflow_state: String,
    pub participants: Vec<i64>,
    pub messages: Vec<MessageInResponse>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// Check if a user is an active participant in a conversation.
async fn is_participant(
    pool: &PgPool,
    conversation_id: i64,
    user_id: i64,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM conversation_participants
        WHERE conversation_id = $1 AND user_id = $2 AND workflow_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(conversation_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /conversations — create a new conversation.
/// Creates conversation + participants (author + recipients) + first message.
async fn create_conversation(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateConversation>,
) -> Result<(StatusCode, Json<CreatedConversation>), ApiError> {
    validate_recipients(&payload.recipients)?;

    if payload.subject.trim().is_empty() {
        return Err(ApiError::Validation("subject cannot be empty".into()));
    }
    if payload.body.trim().is_empty() {
        return Err(ApiError::Validation("body cannot be empty".into()));
    }

    let mut tx = state.db.begin().await?;

    // Create conversation.
    let conv: Conversation = sqlx::query_as(
        r#"
        INSERT INTO conversations (subject, workflow_state)
        VALUES ($1, 'active')
        RETURNING id, subject, workflow_state, created_at, updated_at
        "#,
    )
    .bind(&payload.subject)
    .fetch_one(&mut *tx)
    .await?;

    // Collect all participant user IDs: author first, then recipients.
    let mut participant_ids = vec![auth.id];
    for &uid in &payload.recipients {
        if uid != auth.id && !participant_ids.contains(&uid) {
            participant_ids.push(uid);
        }
    }

    // Insert participants.
    for &uid in &participant_ids {
        sqlx::query(
            r#"
            INSERT INTO conversation_participants (conversation_id, user_id, workflow_state, last_message_at)
            VALUES ($1, $2, 'active', now())
            "#,
        )
        .bind(conv.id)
        .bind(uid)
        .execute(&mut *tx)
        .await?;
    }

    // Insert first message.
    let msg: ConversationMessage = sqlx::query_as(
        r#"
        INSERT INTO conversation_messages (conversation_id, author_id, body)
        VALUES ($1, $2, $3)
        RETURNING id, conversation_id, author_id, body, created_at
        "#,
    )
    .bind(conv.id)
    .bind(auth.id)
    .bind(&payload.body)
    .fetch_one(&mut *tx)
    .await?;

    tx.commit().await?;

    let response = CreatedConversation {
        id: conv.id,
        subject: conv.subject,
        workflow_state: conv.workflow_state,
        participants: participant_ids,
        messages: vec![MessageInResponse {
            id: msg.id,
            author_id: msg.author_id,
            body: msg.body,
            created_at: msg.created_at,
        }],
        created_at: conv.created_at,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// GET /conversations — list conversations for the authenticated user.
/// Returns conversations where the user is an active participant.
/// Excludes deleted conversations for this user.
async fn list_conversations(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<ConversationSummary>>, ApiError> {
    let rows: Vec<ConversationSummary> = sqlx::query_as(
        r#"
        SELECT
            c.id,
            c.subject,
            c.workflow_state,
            c.created_at,
            (
                SELECT cm.body
                FROM conversation_messages cm
                WHERE cm.conversation_id = c.id
                ORDER BY cm.created_at DESC
                LIMIT 1
            ) AS last_message_preview,
            (
                SELECT COUNT(*)
                FROM conversation_participants cp
                WHERE cp.conversation_id = c.id AND cp.workflow_state = 'active'
            ) AS participant_count,
            cp.last_message_at
        FROM conversations c
        JOIN conversation_participants cp ON cp.conversation_id = c.id
        WHERE cp.user_id = $1
          AND cp.workflow_state = 'active'
          AND c.workflow_state = 'active'
        ORDER BY cp.last_message_at DESC NULL LAST, c.created_at DESC
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /conversations/{id} — get full conversation with all messages.
/// Must be an active participant.
async fn get_conversation(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<ConversationDetail>, ApiError> {
    // Verify participant.
    if !is_participant(&state.db, id, auth.id).await? {
        return Err(ApiError::NotFound); // Return NotFound to avoid leaking existence
    }

    // Fetch conversation.
    let conv: Option<Conversation> = sqlx::query_as(
        r#"
        SELECT id, subject, workflow_state, created_at, updated_at
        FROM conversations
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    let conv = conv.ok_or(ApiError::NotFound)?;

    // Fetch active participants.
    let participants: Vec<(i64,)> = sqlx::query_as(
        r#"
        SELECT user_id FROM conversation_participants
        WHERE conversation_id = $1 AND workflow_state = 'active'
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let participant_ids: Vec<i64> = participants.into_iter().map(|(uid,)| uid).collect();

    // Fetch all messages (ordered oldest to newest).
    let messages: Vec<ConversationMessage> = sqlx::query_as(
        r#"
        SELECT id, conversation_id, author_id, body, created_at
        FROM conversation_messages
        WHERE conversation_id = $1
        ORDER BY created_at ASC, id ASC
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let messages: Vec<MessageInResponse> = messages
        .into_iter()
        .map(|m| MessageInResponse {
            id: m.id,
            author_id: m.author_id,
            body: m.body,
            created_at: m.created_at,
        })
        .collect();

    Ok(Json(ConversationDetail {
        id: conv.id,
        subject: conv.subject,
        workflow_state: conv.workflow_state,
        participants: participant_ids,
        messages,
        created_at: conv.created_at,
        updated_at: conv.updated_at,
    }))
}

/// POST /conversations/{id}/add_message — append a message.
/// Updates last_message_at for all active participants.
async fn add_message(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<AddMessage>,
) -> Result<Json<MessageInResponse>, ApiError> {
    if payload.body.trim().is_empty() {
        return Err(ApiError::Validation("body cannot be empty".into()));
    }

    // Verify participant.
    if !is_participant(&state.db, id, auth.id).await? {
        return Err(ApiError::NotFound);
    }

    let mut tx = state.db.begin().await?;

    // Insert message.
    let msg: ConversationMessage = sqlx::query_as(
        r#"
        INSERT INTO conversation_messages (conversation_id, author_id, body)
        VALUES ($1, $2, $3)
        RETURNING id, conversation_id, author_id, body, created_at
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .bind(&payload.body)
    .fetch_one(&mut *tx)
    .await?;

    // Update last_message_at for all active participants.
    sqlx::query(
        r#"
        UPDATE conversation_participants
        SET last_message_at = now()
        WHERE conversation_id = $1 AND workflow_state = 'active'
        "#,
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(MessageInResponse {
        id: msg.id,
        author_id: msg.author_id,
        body: msg.body,
        created_at: msg.created_at,
    }))
}

/// DELETE /conversations/{id} — soft-delete for THIS user only.
/// Sets workflow_state = 'deleted' on the user's conversation_participants row.
async fn delete_conversation(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Verify participant.
    if !is_participant(&state.db, id, auth.id).await? {
        return Err(ApiError::NotFound);
    }

    let result = sqlx::query(
        r#"
        UPDATE conversation_participants
        SET workflow_state = 'deleted'
        WHERE conversation_id = $1 AND user_id = $2
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/conversations", post(create_conversation))
        .route("/conversations", get(list_conversations))
        .route("/conversations/{id}", get(get_conversation))
        .route("/conversations/{id}", delete(delete_conversation))
        .route("/conversations/{id}/add_message", post(add_message))
}

#[cfg(test)]
mod tests {
    use super::validate_recipients;
    use star_common::ApiError;

    #[test]
    fn creating_conversation_requires_at_least_one_recipient() {
        assert!(matches!(
            validate_recipients(&[]),
            Err(ApiError::Validation(_))
        ));
        assert!(validate_recipients(&[42]).is_ok());
    }
}
