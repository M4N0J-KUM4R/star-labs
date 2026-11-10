//! star-conversations — inbox/DM conversations for star-lms.
//!
//! Canvas API v1 parity (simplified):
//! - POST   /conversations              — create conversation + participants + first message
//! - GET    /conversations              — list conversations for authenticated user
//! - GET    /conversations/{id}         — get full conversation with messages
//! - POST   /conversations/{id}/add_message — append message, update last_message_at
//! - DELETE /conversations/{id}         — soft-delete for this user only
//! - GET    /conversations/{id}/participants    — list active participants
//! - POST   /conversations/{id}/add_recipients  — add users as participants
//! - PUT    /conversations/{id}/read            — mark conversation read for this user

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
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

/// Participant user shape for GET /conversations/{id}/participants.
/// Mirrors Canvas conversation `participants` entries ({id, name, full_name})
/// and adds the per-participant read state for this conversation.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ConversationParticipantUser {
    pub id: i64,
    pub name: String,
    pub full_name: String,
    pub read_at: Option<DateTime<Utc>>,
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

/// POST /conversations/{id}/add_recipients
#[derive(Debug, Deserialize)]
pub struct AddRecipients {
    pub recipients: Vec<i64>, // user IDs to add as participants
}

fn validate_recipients(recipients: &[i64]) -> Result<(), ApiError> {
    if recipients.is_empty() {
        return Err(ApiError::Validation("recipients cannot be empty".into()));
    }
    Ok(())
}

/// Validate the recipients list for add_recipients.
/// Canvas returns 400 (with an empty body) when no recipients are given.
fn validate_new_recipients(recipients: &[i64]) -> Result<(), ApiError> {
    if recipients.is_empty() {
        return Err(ApiError::BadRequest("recipients required".into()));
    }
    Ok(())
}

/// Recipients that still need a participant row: unique, excluding the
/// current user and users that already participate in the conversation.
/// Order is preserved (Canvas dedupes but keeps insertion order).
fn recipients_to_add(
    current_user_id: i64,
    existing_participant_ids: &[i64],
    requested: &[i64],
) -> Vec<i64> {
    let mut to_add: Vec<i64> = Vec::new();
    for &user_id in requested {
        if user_id == current_user_id
            || existing_participant_ids.contains(&user_id)
            || to_add.contains(&user_id)
        {
            continue;
        }
        to_add.push(user_id);
    }
    to_add
}

/// Canvas add_recipients responses only include the latest message.
fn keep_latest_message(messages: &mut Vec<MessageInResponse>) {
    if messages.len() > 1 {
        let latest = messages.split_off(messages.len() - 1);
        *messages = latest;
    }
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

/// Load the full Canvas-shaped conversation detail (participants + messages).
async fn load_conversation_detail(pool: &PgPool, id: i64) -> Result<ConversationDetail, ApiError> {
    // Fetch conversation.
    let conv: Option<Conversation> = sqlx::query_as(
        r#"
        SELECT id, subject, workflow_state, created_at, updated_at
        FROM conversations
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
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
    .fetch_all(pool)
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
    .fetch_all(pool)
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

    Ok(ConversationDetail {
        id: conv.id,
        subject: conv.subject,
        workflow_state: conv.workflow_state,
        participants: participant_ids,
        messages,
        created_at: conv.created_at,
        updated_at: conv.updated_at,
    })
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

    // Insert participants. The author has already read the first message;
    // recipients start out unread (read_at = NULL).
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
        .bind(conv.id)
        .bind(uid)
        .bind(participant_read_at)
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

    Ok(Json(load_conversation_detail(&state.db, id).await?))
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

    // Update last_message_at for all active participants. The author has just
    // read the thread; everyone else's view becomes unread again.
    sqlx::query(
        r#"
        UPDATE conversation_participants
        SET last_message_at = now(),
            read_at = CASE WHEN user_id = $2 THEN now() ELSE NULL END
        WHERE conversation_id = $1 AND workflow_state = 'active'
        "#,
    )
    .bind(id)
    .bind(auth.id)
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

/// GET /conversations/{id}/participants — list active participants.
/// Canvas conversation `participants` shape ({id, name, full_name}) plus the
/// per-participant read state (read_at) for this conversation.
async fn list_participants(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<ConversationParticipantUser>>, ApiError> {
    // Verify participant.
    if !is_participant(&state.db, id, auth.id).await? {
        return Err(ApiError::NotFound);
    }

    let rows: Vec<ConversationParticipantUser> = sqlx::query_as(
        r#"
        SELECT u.id AS id, u.name AS name, u.name AS full_name, cp.read_at AS read_at
        FROM conversation_participants cp
        JOIN users u ON u.id = cp.user_id
        WHERE cp.conversation_id = $1 AND cp.workflow_state = 'active'
        ORDER BY u.id ASC
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// POST /conversations/{id}/add_recipients — add users to a conversation.
/// Canvas request field: recipients[] (user ids). The response mirrors
/// GET /conversations/{id} but only includes the latest message. Unknown user
/// ids are ignored and an empty recipients array returns 400.
async fn add_recipients(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<AddRecipients>,
) -> Result<Json<ConversationDetail>, ApiError> {
    validate_new_recipients(&payload.recipients)?;

    // Verify participant.
    if !is_participant(&state.db, id, auth.id).await? {
        return Err(ApiError::NotFound);
    }

    // Unknown user ids are ignored (Canvas drops unknown recipients).
    let known: Vec<(i64,)> =
        sqlx::query_as("SELECT id FROM users WHERE id = ANY($1::BIGINT[]) ORDER BY id ASC")
            .bind(payload.recipients.as_slice())
            .fetch_all(&state.db)
            .await?;
    let requested: Vec<i64> = known.into_iter().map(|(uid,)| uid).collect();

    let existing: Vec<(i64,)> = sqlx::query_as(
        r#"
        SELECT user_id FROM conversation_participants WHERE conversation_id = $1
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let existing_ids: Vec<i64> = existing.into_iter().map(|(uid,)| uid).collect();

    let to_add = recipients_to_add(auth.id, &existing_ids, &requested);

    if !to_add.is_empty() {
        let mut tx = state.db.begin().await?;
        for user_id in &to_add {
            // New participants start unread and do not jump to the top of the
            // inbox: last_message_at is set to the current latest message.
            sqlx::query(
                r#"
                INSERT INTO conversation_participants
                    (conversation_id, user_id, workflow_state, last_message_at, read_at)
                VALUES (
                    $1,
                    $2,
                    'active',
                    (SELECT MAX(created_at) FROM conversation_messages WHERE conversation_id = $1),
                    NULL
                )
                "#,
            )
            .bind(id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
    }

    let mut detail = load_conversation_detail(&state.db, id).await?;
    keep_latest_message(&mut detail.messages);
    Ok(Json(detail))
}

/// PUT /conversations/{id}/read — mark the conversation read for this user.
/// Idempotent: sets read_at on the current user's participant row.
async fn mark_read(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<ConversationDetail>, ApiError> {
    // Verify participant.
    if !is_participant(&state.db, id, auth.id).await? {
        return Err(ApiError::NotFound);
    }

    sqlx::query(
        r#"
        UPDATE conversation_participants
        SET read_at = now()
        WHERE conversation_id = $1 AND user_id = $2 AND workflow_state = 'active'
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    Ok(Json(load_conversation_detail(&state.db, id).await?))
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
        .route("/conversations/{id}/participants", get(list_participants))
        .route("/conversations/{id}/add_recipients", post(add_recipients))
        .route("/conversations/{id}/read", put(mark_read))
}

#[cfg(test)]
mod tests {
    use super::{
        keep_latest_message, recipients_to_add, validate_new_recipients, validate_recipients,
        MessageInResponse,
    };
    use chrono::Utc;
    use star_common::ApiError;

    #[test]
    fn creating_conversation_requires_at_least_one_recipient() {
        assert!(matches!(
            validate_recipients(&[]),
            Err(ApiError::Validation(_))
        ));
        assert!(validate_recipients(&[42]).is_ok());
    }

    #[test]
    fn add_recipients_requires_a_non_empty_recipient_list() {
        assert!(matches!(
            validate_new_recipients(&[]),
            Err(ApiError::BadRequest(_))
        ));
        assert!(validate_new_recipients(&[42]).is_ok());
    }

    #[test]
    fn recipients_to_add_skips_self_existing_and_duplicates() {
        let to_add = recipients_to_add(1, &[2, 3], &[2, 4, 4, 1, 5, 3]);
        assert_eq!(to_add, vec![4, 5]);
        assert!(recipients_to_add(1, &[2], &[1, 2]).is_empty());
    }

    #[test]
    fn add_recipients_response_keeps_only_the_latest_message() {
        let message = |id: i64, body: &str| MessageInResponse {
            id,
            author_id: 1,
            body: body.to_string(),
            created_at: Utc::now(),
        };

        let mut empty: Vec<MessageInResponse> = Vec::new();
        keep_latest_message(&mut empty);
        assert!(empty.is_empty());

        let mut single = vec![message(1, "first")];
        keep_latest_message(&mut single);
        assert_eq!(single.len(), 1);

        let mut many = vec![message(1, "first"), message(2, "latest")];
        keep_latest_message(&mut many);
        assert_eq!(many.len(), 1);
        assert_eq!(many[0].body, "latest");
    }
}
