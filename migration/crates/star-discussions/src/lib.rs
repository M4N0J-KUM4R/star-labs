//! star-discussions — discussion topics and threaded entries for star-lms.
//!
//! Endpoints:
//! - POST   /courses/{courseId}/discussion_topics          — create (teacher/admin)
//! - GET    /courses/{courseId}/discussion_topics          — list (enrolled/admin)
//! - GET    /courses/{courseId}/discussion_topics/{id}     — get one (enrolled/admin)
//! - PUT    /courses/{courseId}/discussion_topics/{id}     — update (teacher/admin)
//! - DELETE /courses/{courseId}/discussion_topics/{id}     — soft-delete (teacher/admin)
//! - POST   /courses/{courseId}/discussion_topics/{topicId}/entries  — post entry (enrolled)
//! - GET    /courses/{courseId}/discussion_topics/{topicId}/entries — list with nested replies
//! - PUT    /courses/{courseId}/discussion_topics/{topicId}/read    — mark topic read (enrolled)
//! - DELETE /courses/{courseId}/discussion_topics/{topicId}/read    — mark topic unread (enrolled)
//! - GET    /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId} — get one entry (enrolled)
//! - PUT    /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId} — update entry (author/moderator)
//! - DELETE /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId} — soft-delete entry (author/moderator)
//! - POST   /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/replies — reply to entry (enrolled)
//! - PUT    /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/read — mark entry read (enrolled)
//! - DELETE /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/read — mark entry unread (enrolled)
//! - POST   /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/rating — rate entry 0/1 (enrolled)
//! - PUT    /courses/{courseId}/discussion_topics/{topicId}/subscribed — subscribe (enrolled)
//! - DELETE /courses/{courseId}/discussion_topics/{topicId}/subscribed — unsubscribe (enrolled)
//! - GET    /courses/{courseId}/discussion_topics/{topicId}/summaries — last summary (teacher/admin)
//! - POST   /courses/{courseId}/discussion_topics/{topicId}/summaries — generate summary (teacher/admin)
//! - GET    /groups/{groupId}/discussion_topics/{topicId}/summaries — last summary (group context)
//! - POST   /groups/{groupId}/discussion_topics/{topicId}/summaries — generate summary (group context)
//! - PUT    /courses/{courseId}/discussion_topics/update_discussion_types — set discussion types (teacher/admin)
//! - PUT    /courses/{courseId}/discussion_topics/migrate_disallow — migrate side comments (teacher/admin)
//! - GET    /groups/{groupId}/discussion_topics/{topicId} — group topic show (group read access)
//! - GET/POST /groups/{groupId}/discussion_topics/{topicId}/entries — group entry list / create
//! - GET/PUT/DELETE /groups/{groupId}/discussion_topics/{topicId}/entries/{entryId} — group entry CRUD
//! - GET/POST /groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}/replies — group replies
//! - GET    /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/replies — flat replies (course)
//! - PUT/DELETE .../read, POST .../rating, PUT/DELETE .../subscribed — group read-state / rating / subscription
//! - PUT    /courses/{courseId}/discussion_topics/read_all — mark every topic read
//! - PUT    /groups/{groupId}/discussion_topics/read_all — mark every group topic read
//! - PUT/DELETE /{context}/discussion_topics/{topicId}/read_all — topic + entries read state
//! - GET    /{context}/discussion_topics/{topicId}/view — view (entries, participants, unread, ratings)
//! - GET    /{context}/discussion_topics/{topicId}/entry_list — requested entries, deleted flagged
//! - POST   /{context}/discussion_topics/{topicId}/duplicate — duplicate a topic
//! - PUT    /{context}/discussion_topics/{topicId}/summaries/disable — disable the topic summary
//! - POST   /{context}/discussion_topics/{topicId}/summaries/{summaryId}/feedback — summary feedback
//! - GET/POST /{context}/discussion_topics/{topicId}/insights — insights (empty state per Canvas)
//! - GET/PUT /{context}/discussion_topics/{topicId}/insights/entries — insight entries (empty state)
//! - POST   /{context}/discussion_topics/{topicId}/accessibility/scan — accessibility scan (401 shape)
//! - POST   /{context}/discussion_topics/{topicId}/accessibility/queue_scan — queue scan (401 shape)

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Discussion topic response shape (Canvas API v1 compatible).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DiscussionTopic {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    pub message: Option<String>,
    pub workflow_state: String,
    pub discussion_type: String,
    pub pinned: bool,
    pub locked: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Discussion entry row from DB.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DiscussionEntryRow {
    pub id: i64,
    pub topic_id: i64,
    pub parent_id: Option<i64>,
    pub user_id: Option<i64>,
    pub message: Option<String>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Discussion entry with nested replies (Canvas API v1 shape).
#[derive(Debug, Clone, Serialize)]
pub struct DiscussionEntry {
    pub id: i64,
    pub topic_id: i64,
    pub parent_id: Option<i64>,
    pub user_id: Option<i64>,
    pub message: Option<String>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub replies: Vec<DiscussionEntry>,
    /// Present (and true) only in `entry_list` responses for deleted entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted: Option<bool>,
}

impl From<DiscussionEntryRow> for DiscussionEntry {
    fn from(row: DiscussionEntryRow) -> Self {
        DiscussionEntry {
            id: row.id,
            topic_id: row.topic_id,
            parent_id: row.parent_id,
            user_id: row.user_id,
            message: row.message,
            workflow_state: row.workflow_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
            replies: Vec::new(),
            deleted: None,
        }
    }
}

/// Request body for creating a discussion topic.
#[derive(Debug, Deserialize)]
pub struct CreateTopic {
    pub title: String,
    pub message: Option<String>,
    pub discussion_type: Option<String>,
}

/// Request body for updating a discussion topic.
#[derive(Debug, Deserialize)]
pub struct UpdateTopic {
    pub title: Option<String>,
    pub message: Option<String>,
    pub discussion_type: Option<String>,
    /// Canvas `published` — true maps to the active state, false to unpublished.
    pub published: Option<bool>,
    /// Canvas `pinned` — keep the topic at the top of the forum (teacher/admin).
    pub pinned: Option<bool>,
    /// Canvas `locked` — lock the topic against new comments (teacher/admin).
    pub locked: Option<bool>,
}

/// Request body for creating a discussion entry.
#[derive(Debug, Deserialize)]
pub struct CreateEntry {
    pub message: String,
    pub parent_id: Option<i64>,
}

/// Request body for updating a discussion entry.
#[derive(Debug, Deserialize)]
pub struct UpdateEntry {
    pub message: Option<String>,
}

/// Request body for replying to a discussion entry.
#[derive(Debug, Deserialize)]
pub struct CreateReply {
    pub message: String,
}

/// Pagination query parameters.
#[derive(Debug, Deserialize, Default)]
pub struct Pagination {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl Pagination {
    fn offset(&self) -> i64 {
        let page = self.page.unwrap_or(1).max(1);
        let per_page = self.per_page.unwrap_or(20).clamp(1, 50);
        (page - 1) * per_page
    }

    fn limit(&self) -> i64 {
        self.per_page.unwrap_or(20).clamp(1, 50)
    }
}

/// Query parameters for the read / unread endpoints (Canvas `forced_read_state`).
#[derive(Debug, Deserialize, Default)]
pub struct ReadStateParams {
    pub forced_read_state: Option<String>,
}

/// Canvas parses `forced_read_state` leniently, so keep the raw string and
/// coerce it here rather than failing the request on unusual values.
fn value_to_boolean(raw: Option<&str>) -> Option<bool> {
    raw.map(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "t" | "yes"
        )
    })
}

/// Query form of the Canvas `rating` argument.
#[derive(Debug, Deserialize, Default)]
pub struct RateEntryParams {
    pub rating: Option<String>,
}

/// JSON body form of the Canvas `rating` argument.
#[derive(Debug, Deserialize, Default)]
pub struct RateEntryBody {
    #[serde(default)]
    pub rating: Option<serde_json::Value>,
}

fn parse_rating(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

async fn check_enrolled_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

async fn check_teacher_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/discussion_topics — create a topic (teacher/admin).
async fn create_topic(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateTopic>,
) -> Result<(StatusCode, Json<DiscussionTopic>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title cannot be empty".into()));
    }
    let discussion_type = payload.discussion_type.as_deref().unwrap_or("side_comment");
    if !["side_comment", "threaded", "flat", "not_threaded"].contains(&discussion_type) {
        return Err(ApiError::Validation(
            "discussion_type must be 'side_comment', 'threaded', 'flat', or 'not_threaded'".into(),
        ));
    }
    let row: DiscussionTopic = sqlx::query_as(
        r#"
        INSERT INTO discussion_topics (course_id, title, message, discussion_type, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id, course_id, title, message, workflow_state, discussion_type, pinned, locked, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .bind(&payload.message)
    .bind(discussion_type)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/discussion_topics — list topics (enrolled/admin).
async fn list_topics(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(pag): Query<Pagination>,
    auth: AuthUser,
) -> Result<Json<Vec<DiscussionTopic>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let rows: Vec<DiscussionTopic> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, discussion_type, pinned, locked, created_at, updated_at
        FROM discussion_topics
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        ORDER BY created_at DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(course_id)
    .bind(pag.limit())
    .bind(pag.offset())
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows))
}

/// GET /courses/{courseId}/discussion_topics/{id} — get one topic (enrolled/admin).
async fn get_topic(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DiscussionTopic>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let row: Option<DiscussionTopic> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, discussion_type, pinned, locked, created_at, updated_at
        FROM discussion_topics
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    row.ok_or(ApiError::NotFound).map(Json)
}

/// PUT /courses/{courseId}/discussion_topics/{id} — update a topic (teacher/admin).
async fn update_topic(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateTopic>,
) -> Result<Json<DiscussionTopic>, ApiError> {
    // Fetch existing to verify ownership and get current values.
    let existing: Option<DiscussionTopic> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, discussion_type, pinned, locked, created_at, updated_at
        FROM discussion_topics
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let existing = existing.ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let title = payload.title.unwrap_or(existing.title);
    let message = payload.message.or(existing.message);
    let discussion_type = payload.discussion_type.unwrap_or(existing.discussion_type);
    // Canvas maps `published` onto the workflow state (true -> active, false
    // -> unpublished); `pinned` and `locked` are plain moderation flags.
    let workflow_state = match payload.published {
        Some(true) => "active".to_string(),
        Some(false) => "unpublished".to_string(),
        None => existing.workflow_state.clone(),
    };
    let pinned = payload.pinned.unwrap_or(existing.pinned);
    let locked = payload.locked.unwrap_or(existing.locked);

    if title.trim().is_empty() {
        return Err(ApiError::Validation("title cannot be empty".into()));
    }
    if !["side_comment", "threaded", "flat", "not_threaded"].contains(&discussion_type.as_str()) {
        return Err(ApiError::Validation(
            "discussion_type must be 'side_comment', 'threaded', 'flat', or 'not_threaded'".into(),
        ));
    }

    let updated: DiscussionTopic = sqlx::query_as(
        r#"
        UPDATE discussion_topics
        SET title = $1, message = $2, discussion_type = $3, workflow_state = $4,
            pinned = $5, locked = $6, updated_at = now()
        WHERE id = $7 AND course_id = $8 AND workflow_state <> 'deleted'
        RETURNING id, course_id, title, message, workflow_state, discussion_type, pinned, locked, created_at, updated_at
        "#,
    )
    .bind(&title)
    .bind(&message)
    .bind(&discussion_type)
    .bind(&workflow_state)
    .bind(pinned)
    .bind(locked)
    .bind(id)
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(updated))
}

/// DELETE /courses/{courseId}/discussion_topics/{id} — soft-delete a topic.
async fn delete_topic(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT course_id FROM discussion_topics WHERE id = $1 AND course_id = $2")
            .bind(id)
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    let _ = row.ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    sqlx::query(
        "UPDATE discussion_topics SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /courses/{courseId}/discussion_topics/{topicId}/entries — post an entry.
async fn create_entry(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateEntry>,
) -> Result<(StatusCode, Json<DiscussionEntry>), ApiError> {
    // Verify topic exists and user is enrolled.
    let topic: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(&state.db)
    .await?;
    let (topic_course_id,) = topic.ok_or(ApiError::NotFound)?;

    if topic_course_id != course_id {
        return Err(ApiError::NotFound);
    }

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.message.trim().is_empty() {
        return Err(ApiError::Validation("message cannot be empty".into()));
    }

    // If parent_id provided, verify it belongs to the same topic.
    if let Some(parent_id) = payload.parent_id {
        let parent: Option<(i64,)> = sqlx::query_as(
            "SELECT topic_id FROM discussion_entries WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(parent_id)
        .fetch_optional(&state.db)
        .await?;
        match parent {
            Some((pt_id,)) if pt_id == topic_id => {}
            _ => {
                return Err(ApiError::Validation(
                    "parent_id must belong to the same topic".into(),
                ))
            }
        }
    }

    let row: DiscussionEntryRow = sqlx::query_as(
        r#"
        INSERT INTO discussion_entries (topic_id, parent_id, user_id, message, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        "#,
    )
    .bind(topic_id)
    .bind(payload.parent_id)
    .bind(auth.id)
    .bind(&payload.message)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(DiscussionEntry::from(row))))
}

/// GET /courses/{courseId}/discussion_topics/{topicId}/entries — list with nested replies.
async fn list_entries(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<DiscussionEntry>>, ApiError> {
    // Verify topic exists.
    let topic: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(&state.db)
    .await?;
    let (topic_course_id,) = topic.ok_or(ApiError::NotFound)?;

    if topic_course_id != course_id {
        return Err(ApiError::NotFound);
    }

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Load all non-deleted entries for this topic, ordered by created_at.
    let rows: Vec<DiscussionEntryRow> = sqlx::query_as(
        r#"
        SELECT id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        FROM discussion_entries
        WHERE topic_id = $1 AND workflow_state <> 'deleted'
        ORDER BY created_at ASC
        "#,
    )
    .bind(topic_id)
    .fetch_all(&state.db)
    .await?;

    // Build a map from id -> entry index.
    let mut entries: Vec<DiscussionEntry> = rows.into_iter().map(DiscussionEntry::from).collect();
    let total = entries.len();

    // Build parent_id -> children indices map.
    let mut children: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(parent_id) = entry.parent_id {
            children.entry(parent_id).or_default().push(i);
        }
    }

    // Collect top-level entry indices (parent_id IS NULL).
    let top_level: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.parent_id.is_none())
        .map(|(i, _)| i)
        .collect();

    // For each top-level entry, build nested replies recursively.
    fn build_replies(
        entries: &mut Vec<DiscussionEntry>,
        children: &HashMap<i64, Vec<usize>>,
        idx: usize,
    ) {
        if let Some(child_indices) = children.get(&entries[idx].id) {
            let child_indices = child_indices.clone();
            for child_idx in child_indices {
                build_replies(entries, children, child_idx);
                let reply = entries[child_idx].clone();
                entries[idx].replies.push(reply);
            }
        }
    }

    for &idx in &top_level {
        build_replies(&mut entries, &children, idx);
    }

    // Return only top-level entries (with nested replies populated).
    let result: Vec<DiscussionEntry> = top_level.into_iter().map(|i| entries[i].clone()).collect();

    let _ = total; // suppress unused warning

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Single-entry read/update/delete + replies (Canvas entry parity)
// ---------------------------------------------------------------------------

/// Canvas `DiscussionEntry` :update / :delete policy: the entry's author or a
/// forum moderator (course teacher / global admin).
async fn check_entry_editor(
    pool: &PgPool,
    course_id: i64,
    author_id: Option<i64>,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if author_id == Some(auth.id) {
        return Ok(true);
    }
    check_teacher_or_admin(pool, auth.id, course_id, auth).await
}

/// GET /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}.
async fn get_entry(
    State(state): State<AppState>,
    Path((course_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DiscussionEntry>, ApiError> {
    ensure_entry_in_topic(&state.db, course_id, topic_id, entry_id, &auth).await?;

    let row: Option<DiscussionEntryRow> = sqlx::query_as(
        r#"
        SELECT id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        FROM discussion_entries
        WHERE id = $1 AND topic_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(entry_id)
    .bind(topic_id)
    .fetch_optional(&state.db)
    .await?;
    row.map(|row| Json(DiscussionEntry::from(row)))
        .ok_or(ApiError::NotFound)
}

/// PUT /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}.
///
/// Only the entry body is mutable here; Canvas also accepts
/// `plaintext_message`/`remove_attachment`, which this metadata-only API
/// ignores.
async fn update_entry(
    State(state): State<AppState>,
    Path((course_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateEntry>,
) -> Result<Json<DiscussionEntry>, ApiError> {
    ensure_topic_in_course(&state.db, course_id, topic_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas raises RecordNotFound for deleted or cross-topic entries before
    // checking the :update permission.
    let existing: Option<(Option<i64>,)> = sqlx::query_as(
        r#"
        SELECT user_id FROM discussion_entries
        WHERE id = $1 AND topic_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(entry_id)
    .bind(topic_id)
    .fetch_optional(&state.db)
    .await?;
    let (author_id,) = existing.ok_or(ApiError::NotFound)?;

    if !check_entry_editor(&state.db, course_id, author_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas `DiscussionEntry` validates presence of `message`, so a missing
    // or blank body fails validation.
    let message = payload
        .message
        .filter(|message| !message.trim().is_empty())
        .ok_or_else(|| ApiError::Validation("message cannot be empty".into()))?;

    let row: DiscussionEntryRow = sqlx::query_as(
        r#"
        UPDATE discussion_entries
        SET message = $1, updated_at = now()
        WHERE id = $2 AND topic_id = $3 AND workflow_state <> 'deleted'
        RETURNING id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        "#,
    )
    .bind(&message)
    .bind(entry_id)
    .bind(topic_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(DiscussionEntry::from(row)))
}

/// DELETE /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}.
///
/// Canvas soft-deletes the entry and clears its author and body.
async fn delete_entry(
    State(state): State<AppState>,
    Path((course_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    ensure_topic_in_course(&state.db, course_id, topic_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let existing: Option<(Option<i64>,)> = sqlx::query_as(
        r#"
        SELECT user_id FROM discussion_entries
        WHERE id = $1 AND topic_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(entry_id)
    .bind(topic_id)
    .fetch_optional(&state.db)
    .await?;
    let (author_id,) = existing.ok_or(ApiError::NotFound)?;

    if !check_entry_editor(&state.db, course_id, author_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    sqlx::query(
        r#"
        UPDATE discussion_entries
        SET workflow_state = 'deleted', user_id = NULL, message = NULL, updated_at = now()
        WHERE id = $1 AND topic_id = $2
        "#,
    )
    .bind(entry_id)
    .bind(topic_id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/replies.
///
/// Canvas `add_reply`: the parent must be a live entry of this topic, and the
/// created entry is nested under it (replies to replies stay nested one level
/// deeper, exactly like `POST .../entries` with `parent_id`).
async fn add_reply(
    State(state): State<AppState>,
    Path((course_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateReply>,
) -> Result<(StatusCode, Json<DiscussionEntry>), ApiError> {
    ensure_topic_in_course(&state.db, course_id, topic_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas `all_entries(@topic).find(params[:entry_id])` -> 404 when the
    // parent is missing, deleted, or belongs to another topic.
    let parent: Option<(i64,)> = sqlx::query_as(
        "SELECT topic_id FROM discussion_entries WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(entry_id)
    .fetch_optional(&state.db)
    .await?;
    let (parent_topic_id,) = parent.ok_or(ApiError::NotFound)?;
    if parent_topic_id != topic_id {
        return Err(ApiError::NotFound);
    }

    if payload.message.trim().is_empty() {
        return Err(ApiError::Validation("message cannot be empty".into()));
    }

    let row: DiscussionEntryRow = sqlx::query_as(
        r#"
        INSERT INTO discussion_entries (topic_id, parent_id, user_id, message, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        "#,
    )
    .bind(topic_id)
    .bind(entry_id)
    .bind(auth.id)
    .bind(&payload.message)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(DiscussionEntry::from(row))))
}

// ---------------------------------------------------------------------------
// Read state + rating helpers (Canvas discussion_topics_api semantics)
// ---------------------------------------------------------------------------

/// 404 unless the topic exists, is not soft-deleted, and belongs to the course
/// in the URL (Canvas `require_topic` + `@context.all_discussion_topics.active`).
async fn ensure_topic_in_course(
    pool: &PgPool,
    course_id: i64,
    topic_id: i64,
) -> Result<(), ApiError> {
    let topic: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(pool)
    .await?;
    let (topic_course_id,) = topic.ok_or(ApiError::NotFound)?;
    if topic_course_id != course_id {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// Canvas `require_topic` + `require_entry` ordering: a missing/deleted topic or
/// an entry from another topic is a 404; an unenrolled caller is a 403.
async fn ensure_entry_in_topic(
    pool: &PgPool,
    course_id: i64,
    topic_id: i64,
    entry_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    ensure_topic_in_course(pool, course_id, topic_id).await?;

    if !check_enrolled_or_admin(pool, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let entry: Option<(i64,)> = sqlx::query_as(
        "SELECT topic_id FROM discussion_entries WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(entry_id)
    .fetch_optional(pool)
    .await?;
    let (entry_topic_id,) = entry.ok_or(ApiError::NotFound)?;
    if entry_topic_id != topic_id {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// Apply a topic read/unread change. Mirrors Canvas `change_read_state`: the
/// implicit default state is "unread", and no row is written (and
/// `forced_read_state` is left untouched) when the requested state already
/// matches, so marking an unread topic unread is a 204 no-op.
async fn set_topic_read_state(
    pool: &PgPool,
    course_id: i64,
    topic_id: i64,
    auth: &AuthUser,
    new_state: &str,
    forced: Option<bool>,
) -> Result<StatusCode, ApiError> {
    ensure_topic_in_course(pool, course_id, topic_id).await?;

    if !check_enrolled_or_admin(pool, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let current: Option<(String,)> = sqlx::query_as(
        "SELECT workflow_state FROM discussion_topic_reads WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    let current_state = current
        .map(|(state,)| state)
        .unwrap_or_else(|| "unread".to_string());
    if current_state == new_state {
        return Ok(StatusCode::NO_CONTENT);
    }

    sqlx::query(
        r#"
        INSERT INTO discussion_topic_reads (topic_id, user_id, workflow_state, forced_read_state, read_at)
        VALUES ($1, $2, $3, $4, CASE WHEN $3 = 'read' THEN now() ELSE NULL END)
        ON CONFLICT (topic_id, user_id) DO UPDATE
        SET workflow_state = EXCLUDED.workflow_state,
            forced_read_state = COALESCE(EXCLUDED.forced_read_state, discussion_topic_reads.forced_read_state),
            read_at = CASE WHEN EXCLUDED.workflow_state = 'read' THEN now() ELSE discussion_topic_reads.read_at END,
            updated_at = now()
        "#,
    )
    .bind(topic_id)
    .bind(auth.id)
    .bind(new_state)
    .bind(forced)
    .execute(pool)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Apply an entry read/unread change (same Canvas no-op semantics as topics).
async fn set_entry_read_state(
    pool: &PgPool,
    course_id: i64,
    topic_id: i64,
    entry_id: i64,
    auth: &AuthUser,
    new_state: &str,
    forced: Option<bool>,
) -> Result<StatusCode, ApiError> {
    ensure_entry_in_topic(pool, course_id, topic_id, entry_id, auth).await?;

    let current: Option<(String,)> = sqlx::query_as(
        "SELECT workflow_state FROM discussion_entry_reads WHERE entry_id = $1 AND user_id = $2",
    )
    .bind(entry_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    let current_state = current
        .map(|(state,)| state)
        .unwrap_or_else(|| "unread".to_string());
    if current_state == new_state {
        return Ok(StatusCode::NO_CONTENT);
    }

    sqlx::query(
        r#"
        INSERT INTO discussion_entry_reads (entry_id, user_id, workflow_state, forced_read_state, read_at)
        VALUES ($1, $2, $3, $4, CASE WHEN $3 = 'read' THEN now() ELSE NULL END)
        ON CONFLICT (entry_id, user_id) DO UPDATE
        SET workflow_state = EXCLUDED.workflow_state,
            forced_read_state = COALESCE(EXCLUDED.forced_read_state, discussion_entry_reads.forced_read_state),
            read_at = CASE WHEN EXCLUDED.workflow_state = 'read' THEN now() ELSE discussion_entry_reads.read_at END,
            updated_at = now()
        "#,
    )
    .bind(entry_id)
    .bind(auth.id)
    .bind(new_state)
    .bind(forced)
    .execute(pool)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// PUT .../discussion_topics/{topicId}/read — mark the topic read (Canvas: 204).
async fn mark_topic_read(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_topic_read_state(
        &state.db,
        course_id,
        topic_id,
        &auth,
        "read",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

/// DELETE .../discussion_topics/{topicId}/read — mark the topic unread (Canvas: 204).
async fn mark_topic_unread(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_topic_read_state(
        &state.db,
        course_id,
        topic_id,
        &auth,
        "unread",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

/// PUT .../entries/{entryId}/read — mark the entry read (Canvas: 204).
async fn mark_entry_read(
    State(state): State<AppState>,
    Path((course_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_entry_read_state(
        &state.db,
        course_id,
        topic_id,
        entry_id,
        &auth,
        "read",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

/// DELETE .../entries/{entryId}/read — mark the entry unread (Canvas: 204).
async fn mark_entry_unread(
    State(state): State<AppState>,
    Path((course_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_entry_read_state(
        &state.db,
        course_id,
        topic_id,
        entry_id,
        &auth,
        "unread",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

/// POST .../entries/{entryId}/rating — rate an entry 0 (clear) or 1 (Canvas: 204).
///
/// Canvas coerces a missing `rating` via `params[:rating].to_i`, so an absent
/// argument behaves like `rating=0`; anything outside 0/1 is a 400 with
/// `{"message":"Invalid rating given"}`.
#[allow(clippy::result_large_err)]
async fn rate_entry(
    State(state): State<AppState>,
    Path((course_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    Query(params): Query<RateEntryParams>,
    auth: AuthUser,
    payload: Option<Json<RateEntryBody>>,
) -> Result<StatusCode, Response> {
    ensure_entry_in_topic(&state.db, course_id, topic_id, entry_id, &auth)
        .await
        .map_err(|e| e.into_response())?;

    let rating = payload
        .as_ref()
        .and_then(|Json(body)| body.rating.as_ref())
        .and_then(parse_rating)
        .or_else(|| {
            params
                .rating
                .as_deref()
                .and_then(|raw| raw.trim().parse::<i64>().ok())
        })
        .unwrap_or(0);

    if rating != 0 && rating != 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "message": "Invalid rating given" })),
        )
            .into_response());
    }

    sqlx::query(
        r#"
        INSERT INTO discussion_entry_ratings (entry_id, user_id, rating)
        VALUES ($1, $2, $3)
        ON CONFLICT (entry_id, user_id) DO UPDATE
        SET rating = EXCLUDED.rating, updated_at = now()
        "#,
    )
    .bind(entry_id)
    .bind(auth.id)
    .bind(rating as i32)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::from(e).into_response())?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Topic subscription (Canvas subscribe_topic / unsubscribe_topic)
// ---------------------------------------------------------------------------

/// Apply a subscription change. Mirrors Canvas `change_subscribed_state`: the
/// implicit default is "unsubscribed", and a request that already matches the
/// current state is a 204 no-op that writes no row.
async fn set_subscription(
    pool: &PgPool,
    course_id: i64,
    topic_id: i64,
    auth: &AuthUser,
    subscribed: bool,
) -> Result<StatusCode, ApiError> {
    ensure_topic_in_course(pool, course_id, topic_id).await?;

    if !check_enrolled_or_admin(pool, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let current: Option<(bool,)> = sqlx::query_as(
        "SELECT subscribed FROM discussion_subscriptions WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if current.map(|(state,)| state).unwrap_or(false) == subscribed {
        return Ok(StatusCode::NO_CONTENT);
    }

    sqlx::query(
        r#"
        INSERT INTO discussion_subscriptions (topic_id, user_id, subscribed)
        VALUES ($1, $2, $3)
        ON CONFLICT (topic_id, user_id) DO UPDATE
        SET subscribed = EXCLUDED.subscribed, updated_at = now()
        "#,
    )
    .bind(topic_id)
    .bind(auth.id)
    .bind(subscribed)
    .execute(pool)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// PUT .../discussion_topics/{topicId}/subscribed — subscribe (Canvas: 204).
async fn subscribe_topic(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_subscription(&state.db, course_id, topic_id, &auth, true).await
}

/// DELETE .../discussion_topics/{topicId}/subscribed — unsubscribe (Canvas: 204).
async fn unsubscribe_topic(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_subscription(&state.db, course_id, topic_id, &auth, false).await
}

// ---------------------------------------------------------------------------
// Discussion summaries (Canvas find_summary / find_or_create_summary)
// ---------------------------------------------------------------------------

/// Canvas stores two summary rows per generation: the "raw" summary and the
/// "refined" summary that points back at it through `parent_id`.
const SUMMARY_CONFIG_RAW: &str = "discussion_topic_summary_raw";
const SUMMARY_CONFIG_REFINED: &str = "discussion_topic_summary_refined";

/// Canvas `llm_config_raw.rate_limit[:limit]`: the number of summaries a user
/// may generate per day for one context (config/llm_configs/discussion_topic_summary_raw.yml).
const SUMMARY_RATE_LIMIT: i64 = 25;

/// Canvas `usage` object: `{"currentCount": 1, "limit": 5}`.
#[derive(Debug, Clone, Serialize)]
pub struct SummaryUsage {
    #[serde(rename = "currentCount")]
    pub current_count: i64,
    pub limit: i64,
}

/// Canvas `find_or_create_summary` response body.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedSummary {
    pub id: i64,
    pub text: Option<String>,
    pub usage: SummaryUsage,
}

/// Canvas `find_summary` response body.
#[derive(Debug, Clone, Serialize)]
pub struct LastSummary {
    pub id: i64,
    pub text: Option<String>,
    #[serde(rename = "userInput")]
    pub user_input: Option<String>,
    pub obsolete: bool,
    pub usage: SummaryUsage,
}

/// `userInput` argument of the summary endpoints (query string or JSON body).
#[derive(Debug, Deserialize, Default)]
pub struct SummaryParams {
    #[serde(rename = "userInput")]
    pub user_input: Option<String>,
}

/// Canvas `update_discussion_types` arguments: two arrays of topic ids.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateDiscussionTypesParams {
    #[serde(alias = "threaded[]")]
    pub threaded: Option<Vec<i64>>,
    #[serde(alias = "not_threaded[]")]
    pub not_threaded: Option<Vec<i64>>,
}

/// Canvas `migrate_disallow` response body.
#[derive(Debug, Clone, Serialize)]
pub struct MigrateDisallowResult {
    pub update_count: i64,
}

/// Canvas `discussion_topic_summaries` row.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
struct SummaryRow {
    id: i64,
    discussion_topic_id: i64,
    user_id: i64,
    llm_config_version: String,
    dynamic_content_hash: String,
    user_input: Option<String>,
    summary: Option<String>,
    parent_id: Option<i64>,
}

/// Canvas renders the API errors of these endpoints as `{"error": "..."}`.
fn canvas_error(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

/// Canvas `update_discussion_types` invalid-data body.
fn canvas_error_with_count(status: StatusCode, message: &str, count: i64) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": message, "count": count })),
    )
        .into_response()
}

/// FNV-1a (64 bit) rendered as hex. Canvas stores an opaque
/// `dynamic_content_hash` next to every generated summary.
fn fnv1a_hex(parts: &[String]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Canvas stores a `dynamic_content_hash` next to every summary and compares
/// the raw summary's hash with a freshly computed one to flag an outdated
/// summary. The hash covers the live entries of the topic.
async fn topic_content_hash(pool: &PgPool, topic_id: i64) -> Result<String, ApiError> {
    let rows: Vec<(i64, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT id, updated_at FROM discussion_entries
        WHERE topic_id = $1 AND workflow_state <> 'deleted'
        ORDER BY id
        "#,
    )
    .bind(topic_id)
    .fetch_all(pool)
    .await?;

    let mut parts: Vec<String> = Vec::with_capacity(rows.len() + 1);
    parts.push(format!("topic={topic_id}"));
    for (id, updated_at) in rows {
        parts.push(format!("{id}:{}", updated_at.timestamp_micros()));
    }
    Ok(fnv1a_hex(&parts))
}

/// The topic title and its live entries, oldest first.
async fn topic_entries(
    pool: &PgPool,
    topic_id: i64,
) -> Result<(String, Vec<(i64, Option<String>)>), ApiError> {
    let title: Option<String> =
        sqlx::query_scalar("SELECT title FROM discussion_topics WHERE id = $1")
            .bind(topic_id)
            .fetch_optional(pool)
            .await?;
    let rows: Vec<(i64, Option<String>)> = sqlx::query_as(
        r#"
        SELECT id, message FROM discussion_entries
        WHERE topic_id = $1 AND workflow_state <> 'deleted'
        ORDER BY id
        "#,
    )
    .bind(topic_id)
    .fetch_all(pool)
    .await?;
    Ok((title.unwrap_or_default(), rows))
}

fn truncate(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((index, _)) => &text[..index],
        None => text,
    }
}

/// Deterministic local stand-in for Canvas' LLM summary: an extractive digest
/// of the discussion so far. Canvas delegates this to its LLM service.
fn build_summary_text(
    title: &str,
    focus: Option<&str>,
    entries: &[(i64, Option<String>)],
) -> String {
    let mut text = String::new();
    if let Some(focus) = focus.map(str::trim).filter(|value| !value.is_empty()) {
        text.push_str(&format!("Focus: {}. ", truncate(focus, 200)));
    }
    text.push_str(&format!(
        "{} entries posted in \"{}\".",
        entries.len(),
        title
    ));
    if let Some(message) = entries
        .first()
        .and_then(|(_, message)| message.as_deref())
        .map(str::trim)
        .filter(|message| !message.is_empty())
    {
        text.push_str(&format!(" First: {}", truncate(message, 280)));
    }
    if entries.len() > 1 {
        if let Some(message) = entries
            .last()
            .and_then(|(_, message)| message.as_deref())
            .map(str::trim)
            .filter(|message| !message.is_empty())
        {
            text.push_str(&format!(" Latest: {}", truncate(message, 280)));
        }
    }
    text
}

/// Canvas counts the summaries a user generated today (`cache_key` holds the
/// user uuid, the LLM config and the date).
async fn summary_usage(pool: &PgPool, user_id: i64) -> Result<SummaryUsage, ApiError> {
    let current_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*) FROM discussion_topic_summaries
        WHERE user_id = $1 AND llm_config_version = $2
          AND created_at >= date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
        "#,
    )
    .bind(user_id)
    .bind(SUMMARY_CONFIG_REFINED)
    .fetch_one(pool)
    .await?;
    Ok(SummaryUsage {
        current_count,
        limit: SUMMARY_RATE_LIMIT,
    })
}

/// Canvas `fetch_or_create_summary`: the summary is reused while the
/// discussion content (and the requested focus) are unchanged; otherwise a
/// raw summary and its refined counterpart are stored.
async fn create_or_reuse_summary(
    pool: &PgPool,
    topic_id: i64,
    user_id: i64,
    user_input: Option<&str>,
) -> Result<SummaryRow, ApiError> {
    let content_hash = topic_content_hash(pool, topic_id).await?;
    let refined_hash = fnv1a_hex(&[
        content_hash.clone(),
        user_input.unwrap_or_default().to_string(),
    ]);

    let existing: Option<SummaryRow> = sqlx::query_as(
        r#"
        SELECT id, discussion_topic_id, user_id, llm_config_version, dynamic_content_hash,
               user_input, summary, parent_id
        FROM discussion_topic_summaries
        WHERE discussion_topic_id = $1 AND user_id = $2 AND llm_config_version = $3
          AND dynamic_content_hash = $4
          AND user_input IS NOT DISTINCT FROM $5
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(topic_id)
    .bind(user_id)
    .bind(SUMMARY_CONFIG_REFINED)
    .bind(&refined_hash)
    .bind(user_input)
    .fetch_optional(pool)
    .await?;

    if let Some(existing) = existing {
        return Ok(existing);
    }

    let (title, entries) = topic_entries(pool, topic_id).await?;
    let raw_text = build_summary_text(&title, None, &entries);
    let refined_text = build_summary_text(&title, user_input, &entries);

    let raw: SummaryRow = sqlx::query_as(
        r#"
        INSERT INTO discussion_topic_summaries
            (discussion_topic_id, user_id, llm_config_version, dynamic_content_hash, user_input, summary)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, discussion_topic_id, user_id, llm_config_version, dynamic_content_hash,
                  user_input, summary, parent_id
        "#,
    )
    .bind(topic_id)
    .bind(user_id)
    .bind(SUMMARY_CONFIG_RAW)
    .bind(&content_hash)
    .bind(user_input)
    .bind(&raw_text)
    .fetch_one(pool)
    .await?;

    let refined: SummaryRow = sqlx::query_as(
        r#"
        INSERT INTO discussion_topic_summaries
            (discussion_topic_id, user_id, llm_config_version, dynamic_content_hash, user_input,
             summary, parent_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, discussion_topic_id, user_id, llm_config_version, dynamic_content_hash,
                  user_input, summary, parent_id
        "#,
    )
    .bind(topic_id)
    .bind(user_id)
    .bind(SUMMARY_CONFIG_REFINED)
    .bind(&refined_hash)
    .bind(user_input)
    .bind(&refined_text)
    .bind(raw.id)
    .fetch_one(pool)
    .await?;

    // Canvas enables the topic summary once a summary has been generated.
    sqlx::query(
        "UPDATE discussion_topics SET summary_enabled = TRUE, updated_at = now() WHERE id = $1",
    )
    .bind(topic_id)
    .execute(pool)
    .await?;

    Ok(refined)
}

/// Canvas `find_summary`: the caller's most recent refined summary of the
/// topic together with its raw parent. A missing row is a 404.
async fn load_last_summary(
    pool: &PgPool,
    topic_id: i64,
    user_id: i64,
) -> Result<Option<LastSummary>, ApiError> {
    let summary: Option<SummaryRow> = sqlx::query_as(
        r#"
        SELECT id, discussion_topic_id, user_id, llm_config_version, dynamic_content_hash,
               user_input, summary, parent_id
        FROM discussion_topic_summaries
        WHERE discussion_topic_id = $1 AND user_id = $2 AND llm_config_version = $3
        ORDER BY created_at DESC, id DESC
        LIMIT 1
        "#,
    )
    .bind(topic_id)
    .bind(user_id)
    .bind(SUMMARY_CONFIG_REFINED)
    .fetch_optional(pool)
    .await?;

    let Some(summary) = summary else {
        return Ok(None);
    };

    // Canvas only answers when the refined summary still has its raw parent.
    let parent: Option<SummaryRow> = match summary.parent_id {
        Some(parent_id) => {
            sqlx::query_as(
                r#"
                SELECT id, discussion_topic_id, user_id, llm_config_version, dynamic_content_hash,
                       user_input, summary, parent_id
                FROM discussion_topic_summaries
                WHERE id = $1
                "#,
            )
            .bind(parent_id)
            .fetch_optional(pool)
            .await?
        }
        None => None,
    };
    let Some(parent) = parent else {
        return Ok(None);
    };

    let current_hash = topic_content_hash(pool, topic_id).await?;
    let usage = summary_usage(pool, user_id).await?;
    Ok(Some(LastSummary {
        id: summary.id,
        text: summary.summary,
        user_input: summary.user_input,
        obsolete: parent.dynamic_content_hash != current_hash,
        usage,
    }))
}

/// 404 unless the topic is an active topic of the course context (Canvas'
/// `@context.all_discussion_topics.active.find(topic_id)`).
async fn ensure_course_topic(pool: &PgPool, course_id: i64, topic_id: i64) -> Result<(), ApiError> {
    let topic: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT id FROM discussion_topics
        WHERE id = $1 AND course_id = $2 AND group_id IS NULL AND workflow_state <> 'deleted'
        "#,
    )
    .bind(topic_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    topic.map(|_| ()).ok_or(ApiError::NotFound)
}

/// 404 unless the topic is an active topic of the group context.
async fn ensure_group_topic(pool: &PgPool, group_id: i64, topic_id: i64) -> Result<(), ApiError> {
    let topic: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT id FROM discussion_topics
        WHERE id = $1 AND group_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(topic_id)
    .bind(group_id)
    .fetch_optional(pool)
    .await?;
    topic.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `require_context_and_read_access` for a group context: group
/// members, the parent course's teachers and admins may read it.
async fn ensure_group_read_access(
    pool: &PgPool,
    group_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    if is_admin(auth) {
        return Ok(());
    }
    let membership: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM group_memberships
        WHERE group_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'
        LIMIT 1
        "#,
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if membership.is_some() {
        return Ok(());
    }
    let teacher: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM enrollments e
        JOIN groups g ON g.course_id = e.course_id
        WHERE g.id = $1 AND e.user_id = $2 AND e.role = 'teacher'
        LIMIT 1
        "#,
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    if teacher.is_some() {
        return Ok(());
    }
    Err(ApiError::Forbidden)
}

/// `userInput` may arrive as a query argument or inside the JSON body.
fn merge_user_input(query: Option<String>, body: Option<String>) -> Option<String> {
    body.or(query)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// GET /courses/{courseId}/discussion_topics/{topicId}/summaries.
///
/// Canvas `find_summary`: the caller's last summary of the discussion.
async fn find_course_summary(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    // Canvas order: require_context_and_read_access, require_topic,
    // user_can_summarize?.
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_course_topic(&state.db, course_id, topic_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    match load_last_summary(&state.db, topic_id, auth.id).await? {
        Some(summary) => Ok(Json(summary).into_response()),
        None => Ok(canvas_error(StatusCode::NOT_FOUND, "No summary found.")),
    }
}

/// POST /courses/{courseId}/discussion_topics/{topicId}/summaries.
///
/// Canvas `find_or_create_summary`: generates (or reuses) a summary of the
/// discussion so far.
async fn find_or_create_course_summary(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<SummaryParams>,
    auth: AuthUser,
    payload: Option<Json<SummaryParams>>,
) -> Result<Response, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_course_topic(&state.db, course_id, topic_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let user_input = merge_user_input(
        params.user_input,
        payload.and_then(|Json(body)| body.user_input),
    );
    let summary =
        create_or_reuse_summary(&state.db, topic_id, auth.id, user_input.as_deref()).await?;
    let usage = summary_usage(&state.db, auth.id).await?;
    Ok(Json(CreatedSummary {
        id: summary.id,
        text: summary.summary,
        usage,
    })
    .into_response())
}

/// GET /groups/{groupId}/discussion_topics/{topicId}/summaries.
///
/// Canvas `find_summary` for a group context: `user_can_summarize?` is false
/// for every group topic (a group topic has no course), so the endpoint
/// answers 403 once the caller passes the context and topic checks.
async fn find_group_summary(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    ensure_group_topic(&state.db, group_id, topic_id).await?;
    Err(ApiError::Forbidden)
}

/// POST /groups/{groupId}/discussion_topics/{topicId}/summaries.
///
/// Canvas `find_or_create_summary` for a group context: summaries are only
/// available in course contexts, so the endpoint answers 403 once the caller
/// passes the context and topic checks.
async fn find_or_create_group_summary(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    Query(_params): Query<SummaryParams>,
    auth: AuthUser,
    _payload: Option<Json<SummaryParams>>,
) -> Result<Response, ApiError> {
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    ensure_group_topic(&state.db, group_id, topic_id).await?;
    Err(ApiError::Forbidden)
}

/// Number of active side-comment topics of the course that are part of `ids`.
async fn count_side_comment_topics(
    pool: &PgPool,
    course_id: i64,
    ids: &[i64],
) -> Result<i64, ApiError> {
    if ids.is_empty() {
        return Ok(0);
    }
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*) FROM discussion_topics
        WHERE course_id = $1 AND group_id IS NULL AND workflow_state <> 'deleted'
          AND discussion_type = 'side_comment' AND id = ANY($2)
        "#,
    )
    .bind(course_id)
    .bind(ids.to_vec())
    .fetch_one(pool)
    .await?;
    Ok(count)
}

/// Move the given side-comment topics of the course to another discussion type.
async fn set_discussion_types(
    pool: &PgPool,
    course_id: i64,
    ids: &[i64],
    discussion_type: &str,
) -> Result<(), ApiError> {
    if ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        r#"
        UPDATE discussion_topics
        SET discussion_type = $3, updated_at = now()
        WHERE course_id = $1 AND group_id IS NULL AND workflow_state <> 'deleted'
          AND discussion_type = 'side_comment' AND id = ANY($2)
        "#,
    )
    .bind(course_id)
    .bind(ids.to_vec())
    .bind(discussion_type)
    .execute(pool)
    .await?;
    Ok(())
}

/// PUT /courses/{courseId}/discussion_topics/update_discussion_types.
///
/// Canvas `update_discussion_types`: assigns every side-comment topic of the
/// course to either the threaded or the not-threaded discussion type. The
/// request has to cover each side-comment topic exactly once.
async fn update_discussion_types(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<UpdateDiscussionTypesParams>,
    auth: AuthUser,
    payload: Option<Json<UpdateDiscussionTypesParams>>,
) -> Result<Response, ApiError> {
    // Canvas gates the endpoint behind a site-admin feature flag (404 when
    // it is off); the workspace has no feature-flag store, so the gate is
    // not reproduced here.
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let body = payload.map(|Json(body)| body).unwrap_or_default();
    let threaded = body.threaded.or(params.threaded).unwrap_or_default();
    let not_threaded = body
        .not_threaded
        .or(params.not_threaded)
        .unwrap_or_default();

    if threaded.is_empty() && not_threaded.is_empty() {
        return Ok(canvas_error(StatusCode::BAD_REQUEST, "Missing data"));
    }

    let side_comment_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*) FROM discussion_topics
        WHERE course_id = $1 AND group_id IS NULL AND workflow_state <> 'deleted'
          AND discussion_type = 'side_comment'
        "#,
    )
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;

    let to_threaded = count_side_comment_topics(&state.db, course_id, &threaded).await?;
    let to_not_threaded = count_side_comment_topics(&state.db, course_id, &not_threaded).await?;

    if to_threaded != threaded.len() as i64
        || to_not_threaded != not_threaded.len() as i64
        || to_threaded + to_not_threaded != side_comment_count
    {
        return Ok(canvas_error_with_count(
            StatusCode::BAD_REQUEST,
            "Invalid data",
            side_comment_count,
        ));
    }

    set_discussion_types(&state.db, course_id, &threaded, "threaded").await?;
    set_discussion_types(&state.db, course_id, &not_threaded, "not_threaded").await?;

    Ok(Json(serde_json::json!({ "success": "true" })).into_response())
}

/// PUT /courses/{courseId}/discussion_topics/migrate_disallow.
///
/// Canvas `migrate_disallow`: turns every side-comment topic of the course
/// into a threaded one.
async fn migrate_disallow(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<MigrateDisallowResult>, ApiError> {
    // Canvas gates the endpoint behind a site-admin feature flag (404 when
    // it is off); the workspace has no feature-flag store, so the gate is
    // not reproduced here.
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let updated = sqlx::query(
        r#"
        UPDATE discussion_topics
        SET discussion_type = 'threaded', updated_at = now()
        WHERE course_id = $1 AND group_id IS NULL AND workflow_state <> 'deleted'
          AND discussion_type = 'side_comment'
        "#,
    )
    .bind(course_id)
    .execute(&state.db)
    .await?;

    Ok(Json(MigrateDisallowResult {
        update_count: updated.rows_affected() as i64,
    }))
}

// ---------------------------------------------------------------------------
// Context-generic helpers (course + group Canvas contexts)
// ---------------------------------------------------------------------------

/// A discussion context: a course or a group (Canvas `@context`).
#[derive(Debug, Clone, Copy)]
enum TopicContext {
    Course(i64),
    Group(i64),
}

/// 404 unless the topic exists, is not soft-deleted, and belongs to the
/// context in the URL (course topics have `group_id IS NULL`, group topics
/// carry the group id).
async fn ensure_ctx_topic(pool: &PgPool, ctx: TopicContext, topic_id: i64) -> Result<(), ApiError> {
    let (sql, ctx_id) = match ctx {
        TopicContext::Course(course_id) => (
            "SELECT id FROM discussion_topics WHERE id = $1 AND course_id = $2 AND group_id IS NULL AND workflow_state <> 'deleted'",
            course_id,
        ),
        TopicContext::Group(group_id) => (
            "SELECT id FROM discussion_topics WHERE id = $1 AND group_id = $2 AND workflow_state <> 'deleted'",
            group_id,
        ),
    };
    let row: Option<(i64,)> = sqlx::query_as(sql)
        .bind(topic_id)
        .bind(ctx_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `require_context_and_read_access`: enrolled users and admins may
/// read a course; group members, the parent course's teachers and admins may
/// read a group.
async fn check_ctx_read_access(
    pool: &PgPool,
    ctx: TopicContext,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    match ctx {
        TopicContext::Course(course_id) => {
            check_enrolled_or_admin(pool, auth.id, course_id, auth).await
        }
        TopicContext::Group(group_id) => {
            ensure_group_read_access(pool, group_id, auth).await?;
            Ok(true)
        }
    }
}

/// Canvas `moderate_forum`: course teachers/admins for a course; group
/// members, the parent course's teachers and admins for a group.
async fn check_ctx_moderate_access(
    pool: &PgPool,
    ctx: TopicContext,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    match ctx {
        TopicContext::Course(course_id) => {
            check_teacher_or_admin(pool, auth.id, course_id, auth).await
        }
        TopicContext::Group(group_id) => {
            if is_admin(auth) {
                return Ok(true);
            }
            let member: Option<(i32,)> = sqlx::query_as(
                r#"
                SELECT 1 FROM group_memberships
                WHERE group_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'
                LIMIT 1
                "#,
            )
            .bind(group_id)
            .bind(auth.id)
            .fetch_optional(pool)
            .await?;
            if member.is_some() {
                return Ok(true);
            }
            let teacher: Option<(i32,)> = sqlx::query_as(
                r#"
                SELECT 1 FROM enrollments e
                JOIN groups g ON g.course_id = e.course_id
                WHERE g.id = $1 AND e.user_id = $2 AND e.role = 'teacher'
                LIMIT 1
                "#,
            )
            .bind(group_id)
            .bind(auth.id)
            .fetch_optional(pool)
            .await?;
            Ok(teacher.is_some())
        }
    }
}

/// Context + topic + entry existence + read access (Canvas `require_entry`).
async fn ensure_ctx_entry(
    pool: &PgPool,
    ctx: TopicContext,
    topic_id: i64,
    entry_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    ensure_ctx_topic(pool, ctx, topic_id).await?;
    if !check_ctx_read_access(pool, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }
    let entry: Option<(i64,)> = sqlx::query_as(
        "SELECT topic_id FROM discussion_entries WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(entry_id)
    .fetch_optional(pool)
    .await?;
    let (entry_topic_id,) = entry.ok_or(ApiError::NotFound)?;
    if entry_topic_id != topic_id {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// Canvas `DiscussionEntry` :update / :delete policy: the author or a forum
/// moderator of the entry's context.
async fn check_ctx_entry_editor(
    pool: &PgPool,
    ctx: TopicContext,
    author_id: Option<i64>,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if author_id == Some(auth.id) {
        return Ok(true);
    }
    check_ctx_moderate_access(pool, ctx, auth).await
}

/// Upsert one topic read-state row (Canvas `change_read_state` no-op semantics).
async fn upsert_topic_read_state(
    pool: &PgPool,
    topic_id: i64,
    user_id: i64,
    new_state: &str,
    forced: Option<bool>,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO discussion_topic_reads (topic_id, user_id, workflow_state, forced_read_state, read_at)
        VALUES ($1, $2, $3, $4, CASE WHEN $3 = 'read' THEN now() ELSE NULL END)
        ON CONFLICT (topic_id, user_id) DO UPDATE
        SET workflow_state = EXCLUDED.workflow_state,
            forced_read_state = COALESCE(EXCLUDED.forced_read_state, discussion_topic_reads.forced_read_state),
            read_at = CASE WHEN EXCLUDED.workflow_state = 'read' THEN now() ELSE discussion_topic_reads.read_at END,
            updated_at = now()
        "#,
    )
    .bind(topic_id)
    .bind(user_id)
    .bind(new_state)
    .bind(forced)
    .execute(pool)
    .await?;
    Ok(())
}

/// Upsert one entry read-state row (same no-op semantics as topics).
async fn upsert_entry_read_state(
    pool: &PgPool,
    entry_id: i64,
    user_id: i64,
    new_state: &str,
    forced: Option<bool>,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO discussion_entry_reads (entry_id, user_id, workflow_state, forced_read_state, read_at)
        VALUES ($1, $2, $3, $4, CASE WHEN $3 = 'read' THEN now() ELSE NULL END)
        ON CONFLICT (entry_id, user_id) DO UPDATE
        SET workflow_state = EXCLUDED.workflow_state,
            forced_read_state = COALESCE(EXCLUDED.forced_read_state, discussion_entry_reads.forced_read_state),
            read_at = CASE WHEN EXCLUDED.workflow_state = 'read' THEN now() ELSE discussion_entry_reads.read_at END,
            updated_at = now()
        "#,
    )
    .bind(entry_id)
    .bind(user_id)
    .bind(new_state)
    .bind(forced)
    .execute(pool)
    .await?;
    Ok(())
}

/// Canvas `change_topic_read_state` for either context.
async fn set_ctx_topic_read_state(
    pool: &PgPool,
    ctx: TopicContext,
    topic_id: i64,
    auth: &AuthUser,
    new_state: &str,
    forced: Option<bool>,
) -> Result<StatusCode, ApiError> {
    ensure_ctx_topic(pool, ctx, topic_id).await?;
    if !check_ctx_read_access(pool, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }
    upsert_topic_read_state(pool, topic_id, auth.id, new_state, forced).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Canvas `change_entry_read_state` for either context.
async fn set_ctx_entry_read_state(
    pool: &PgPool,
    ctx: TopicContext,
    topic_id: i64,
    entry_id: i64,
    auth: &AuthUser,
    new_state: &str,
    forced: Option<bool>,
) -> Result<StatusCode, ApiError> {
    ensure_ctx_entry(pool, ctx, topic_id, entry_id, auth).await?;
    upsert_entry_read_state(pool, entry_id, auth.id, new_state, forced).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Canvas `change_topic_all_read_state`: the topic and every active entry.
async fn set_ctx_all_read_state(
    pool: &PgPool,
    ctx: TopicContext,
    topic_id: i64,
    auth: &AuthUser,
    new_state: &str,
    forced: Option<bool>,
) -> Result<StatusCode, ApiError> {
    ensure_ctx_topic(pool, ctx, topic_id).await?;
    if !check_ctx_read_access(pool, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }
    upsert_topic_read_state(pool, topic_id, auth.id, new_state, forced).await?;
    sqlx::query(
        r#"
        INSERT INTO discussion_entry_reads (entry_id, user_id, workflow_state, forced_read_state, read_at)
        SELECT e.id, $2, $3, $4, CASE WHEN $3 = 'read' THEN now() ELSE NULL END
        FROM discussion_entries e
        WHERE e.topic_id = $1 AND e.workflow_state <> 'deleted'
        ON CONFLICT (entry_id, user_id) DO UPDATE
        SET workflow_state = EXCLUDED.workflow_state,
            forced_read_state = COALESCE(EXCLUDED.forced_read_state, discussion_entry_reads.forced_read_state),
            read_at = CASE WHEN EXCLUDED.workflow_state = 'read' THEN now() ELSE discussion_entry_reads.read_at END,
            updated_at = now()
        "#,
    )
    .bind(topic_id)
    .bind(auth.id)
    .bind(new_state)
    .bind(forced)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Canvas `mark_all_topic_read`: every published topic of the context.
async fn mark_all_ctx_topics_read(
    pool: &PgPool,
    ctx: TopicContext,
    auth: &AuthUser,
) -> Result<StatusCode, ApiError> {
    if !check_ctx_read_access(pool, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }
    let (sql, ctx_id) = match ctx {
        TopicContext::Course(course_id) => (
            r#"SELECT id FROM discussion_topics
               WHERE course_id = $1 AND group_id IS NULL AND workflow_state = 'active'"#,
            course_id,
        ),
        TopicContext::Group(group_id) => (
            r#"SELECT id FROM discussion_topics
               WHERE group_id = $1 AND workflow_state = 'active'"#,
            group_id,
        ),
    };
    sqlx::query(
        r#"
        INSERT INTO discussion_topic_reads (topic_id, user_id, workflow_state, read_at)
        SELECT t.id, $2, 'read', now()
        FROM discussion_topics t
        WHERE t.id = ANY($1::BIGINT[])
        ON CONFLICT (topic_id, user_id) DO UPDATE
        SET workflow_state = 'read', read_at = now(), updated_at = now()
        "#,
    )
    .bind(
        sqlx::query_scalar::<_, i64>(sql)
            .bind(ctx_id)
            .fetch_all(pool)
            .await?,
    )
    .bind(auth.id)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Canvas `change_subscribed_state` for either context (204, no-op safe).
async fn set_ctx_subscription(
    pool: &PgPool,
    ctx: TopicContext,
    topic_id: i64,
    auth: &AuthUser,
    subscribed: bool,
) -> Result<StatusCode, ApiError> {
    ensure_ctx_topic(pool, ctx, topic_id).await?;
    if !check_ctx_read_access(pool, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }
    sqlx::query(
        r#"
        INSERT INTO discussion_subscriptions (topic_id, user_id, subscribed)
        VALUES ($1, $2, $3)
        ON CONFLICT (topic_id, user_id) DO UPDATE
        SET subscribed = EXCLUDED.subscribed, updated_at = now()
        "#,
    )
    .bind(topic_id)
    .bind(auth.id)
    .bind(subscribed)
    .execute(pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Nested (top-level with replies) entries of a topic.
async fn load_nested_entries(
    pool: &PgPool,
    topic_id: i64,
) -> Result<Vec<DiscussionEntry>, ApiError> {
    let rows: Vec<DiscussionEntryRow> = sqlx::query_as(
        r#"
        SELECT id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        FROM discussion_entries
        WHERE topic_id = $1 AND workflow_state <> 'deleted'
        ORDER BY created_at ASC
        "#,
    )
    .bind(topic_id)
    .fetch_all(pool)
    .await?;

    let mut entries: Vec<DiscussionEntry> = rows.into_iter().map(DiscussionEntry::from).collect();
    let mut children: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(parent_id) = entry.parent_id {
            children.entry(parent_id).or_default().push(i);
        }
    }
    let top_level: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.parent_id.is_none())
        .map(|(i, _)| i)
        .collect();

    fn build_replies(
        entries: &mut Vec<DiscussionEntry>,
        children: &HashMap<i64, Vec<usize>>,
        idx: usize,
    ) {
        if let Some(child_indices) = children.get(&entries[idx].id) {
            let child_indices = child_indices.clone();
            for child_idx in child_indices {
                build_replies(entries, children, child_idx);
                let reply = entries[child_idx].clone();
                entries[idx].replies.push(reply);
            }
        }
    }

    for &idx in &top_level {
        build_replies(&mut entries, &children, idx);
    }
    Ok(top_level.into_iter().map(|i| entries[i].clone()).collect())
}

// ---------------------------------------------------------------------------
// Group-context topic/entry routes (Canvas group discussion parity)
// ---------------------------------------------------------------------------

/// GET /groups/{groupId}/discussion_topics/{topicId} — show (group read).
async fn group_show_topic(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DiscussionTopic>, ApiError> {
    let ctx = TopicContext::Group(group_id);
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_read_access(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let row: Option<DiscussionTopic> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, discussion_type, pinned, locked, created_at, updated_at
        FROM discussion_topics
        WHERE id = $1 AND group_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(topic_id)
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;
    row.ok_or(ApiError::NotFound).map(Json)
}

/// GET /groups/{groupId}/discussion_topics/{topicId}/entries.
async fn group_list_entries(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<DiscussionEntry>>, ApiError> {
    let ctx = TopicContext::Group(group_id);
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_read_access(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(load_nested_entries(&state.db, topic_id).await?))
}

/// POST /groups/{groupId}/discussion_topics/{topicId}/entries.
async fn group_create_entry(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateEntry>,
) -> Result<(StatusCode, Json<DiscussionEntry>), ApiError> {
    let ctx = TopicContext::Group(group_id);
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_read_access(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if payload.message.trim().is_empty() {
        return Err(ApiError::Validation("message cannot be empty".into()));
    }
    if let Some(parent_id) = payload.parent_id {
        let parent: Option<(i64,)> = sqlx::query_as(
            "SELECT topic_id FROM discussion_entries WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(parent_id)
        .fetch_optional(&state.db)
        .await?;
        match parent {
            Some((pt_id,)) if pt_id == topic_id => {}
            _ => {
                return Err(ApiError::Validation(
                    "parent_id must belong to the same topic".into(),
                ))
            }
        }
    }
    let row: DiscussionEntryRow = sqlx::query_as(
        r#"
        INSERT INTO discussion_entries (topic_id, parent_id, user_id, message, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        "#,
    )
    .bind(topic_id)
    .bind(payload.parent_id)
    .bind(auth.id)
    .bind(&payload.message)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(DiscussionEntry::from(row))))
}

/// GET /groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}.
async fn group_get_entry(
    State(state): State<AppState>,
    Path((group_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DiscussionEntry>, ApiError> {
    ensure_ctx_entry(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        entry_id,
        &auth,
    )
    .await?;
    let row: Option<DiscussionEntryRow> = sqlx::query_as(
        r#"
        SELECT id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        FROM discussion_entries
        WHERE id = $1 AND topic_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(entry_id)
    .bind(topic_id)
    .fetch_optional(&state.db)
    .await?;
    row.map(|row| Json(DiscussionEntry::from(row)))
        .ok_or(ApiError::NotFound)
}

/// PUT /groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}.
async fn group_update_entry(
    State(state): State<AppState>,
    Path((group_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateEntry>,
) -> Result<Json<DiscussionEntry>, ApiError> {
    let ctx = TopicContext::Group(group_id);
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_read_access(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let existing: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT user_id FROM discussion_entries WHERE id = $1 AND topic_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(entry_id)
    .bind(topic_id)
    .fetch_optional(&state.db)
    .await?;
    let (author_id,) = existing.ok_or(ApiError::NotFound)?;
    if !check_ctx_entry_editor(&state.db, ctx, author_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let message = payload
        .message
        .filter(|message| !message.trim().is_empty())
        .ok_or_else(|| ApiError::Validation("message cannot be empty".into()))?;
    let row: DiscussionEntryRow = sqlx::query_as(
        r#"
        UPDATE discussion_entries
        SET message = $1, updated_at = now()
        WHERE id = $2 AND topic_id = $3 AND workflow_state <> 'deleted'
        RETURNING id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        "#,
    )
    .bind(&message)
    .bind(entry_id)
    .bind(topic_id)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(DiscussionEntry::from(row)))
}

/// DELETE /groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}.
async fn group_delete_entry(
    State(state): State<AppState>,
    Path((group_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let ctx = TopicContext::Group(group_id);
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_read_access(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let existing: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT user_id FROM discussion_entries WHERE id = $1 AND topic_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(entry_id)
    .bind(topic_id)
    .fetch_optional(&state.db)
    .await?;
    let (author_id,) = existing.ok_or(ApiError::NotFound)?;
    if !check_ctx_entry_editor(&state.db, ctx, author_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    sqlx::query(
        r#"
        UPDATE discussion_entries
        SET workflow_state = 'deleted', user_id = NULL, message = NULL, updated_at = now()
        WHERE id = $1 AND topic_id = $2
        "#,
    )
    .bind(entry_id)
    .bind(topic_id)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET .../entries/{entryId}/replies — all descendants, newest first (Canvas
/// `flattened_discussion_subentries.newest_first`). The entry's topic is
/// verified by `ensure_ctx_entry` first.
async fn ctx_list_replies(
    state: &AppState,
    ctx: TopicContext,
    topic_id: i64,
    entry_id: i64,
    auth: &AuthUser,
) -> Result<Json<Vec<DiscussionEntry>>, ApiError> {
    ensure_ctx_entry(&state.db, ctx, topic_id, entry_id, auth).await?;
    let rows: Vec<DiscussionEntryRow> = sqlx::query_as(
        r#"
        WITH RECURSIVE subtree AS (
            SELECT id FROM discussion_entries WHERE id = $1
            UNION ALL
            SELECT e.id FROM discussion_entries e JOIN subtree s ON e.parent_id = s.id
        )
        SELECT id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        FROM discussion_entries
        WHERE id IN (SELECT id FROM subtree) AND id <> $1 AND workflow_state <> 'deleted'
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .bind(entry_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(DiscussionEntry::from).collect()))
}

/// GET /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/replies.
async fn course_list_replies(
    State(state): State<AppState>,
    Path((course_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<DiscussionEntry>>, ApiError> {
    ctx_list_replies(
        &state,
        TopicContext::Course(course_id),
        topic_id,
        entry_id,
        &auth,
    )
    .await
}

/// GET /groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}/replies.
async fn group_list_replies(
    State(state): State<AppState>,
    Path((group_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<DiscussionEntry>>, ApiError> {
    ctx_list_replies(
        &state,
        TopicContext::Group(group_id),
        topic_id,
        entry_id,
        &auth,
    )
    .await
}

/// POST /groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}/replies.
async fn group_add_reply(
    State(state): State<AppState>,
    Path((group_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateReply>,
) -> Result<(StatusCode, Json<DiscussionEntry>), ApiError> {
    let ctx = TopicContext::Group(group_id);
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_read_access(&state.db, ctx, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let parent: Option<(i64,)> = sqlx::query_as(
        "SELECT topic_id FROM discussion_entries WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(entry_id)
    .fetch_optional(&state.db)
    .await?;
    let (parent_topic_id,) = parent.ok_or(ApiError::NotFound)?;
    if parent_topic_id != topic_id {
        return Err(ApiError::NotFound);
    }
    if payload.message.trim().is_empty() {
        return Err(ApiError::Validation("message cannot be empty".into()));
    }
    let row: DiscussionEntryRow = sqlx::query_as(
        r#"
        INSERT INTO discussion_entries (topic_id, parent_id, user_id, message, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        "#,
    )
    .bind(topic_id)
    .bind(entry_id)
    .bind(auth.id)
    .bind(&payload.message)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(DiscussionEntry::from(row))))
}

/// POST /groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}/rating.
#[allow(clippy::result_large_err)]
async fn group_rate_entry(
    State(state): State<AppState>,
    Path((group_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    Query(params): Query<RateEntryParams>,
    auth: AuthUser,
    payload: Option<Json<RateEntryBody>>,
) -> Result<StatusCode, Response> {
    ensure_ctx_entry(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        entry_id,
        &auth,
    )
    .await
    .map_err(|e| e.into_response())?;

    let rating = payload
        .as_ref()
        .and_then(|Json(body)| body.rating.as_ref())
        .and_then(parse_rating)
        .or_else(|| {
            params
                .rating
                .as_deref()
                .and_then(|raw| raw.trim().parse::<i64>().ok())
        })
        .unwrap_or(0);

    if rating != 0 && rating != 1 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "message": "Invalid rating given" })),
        )
            .into_response());
    }

    sqlx::query(
        r#"
        INSERT INTO discussion_entry_ratings (entry_id, user_id, rating)
        VALUES ($1, $2, $3)
        ON CONFLICT (entry_id, user_id) DO UPDATE
        SET rating = EXCLUDED.rating, updated_at = now()
        "#,
    )
    .bind(entry_id)
    .bind(auth.id)
    .bind(rating as i32)
    .execute(&state.db)
    .await
    .map_err(|e| ApiError::from(e).into_response())?;

    Ok(StatusCode::NO_CONTENT)
}

/// Group read-state wrappers.
async fn group_mark_topic_read(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_topic_read_state(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        &auth,
        "read",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

async fn group_mark_topic_unread(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_topic_read_state(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        &auth,
        "unread",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

async fn group_mark_entry_read(
    State(state): State<AppState>,
    Path((group_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_entry_read_state(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        entry_id,
        &auth,
        "read",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

async fn group_mark_entry_unread(
    State(state): State<AppState>,
    Path((group_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_entry_read_state(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        entry_id,
        &auth,
        "unread",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

/// Mark-all wrappers (topic level, both contexts).
async fn course_mark_all_read(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_all_read_state(
        &state.db,
        TopicContext::Course(course_id),
        topic_id,
        &auth,
        "read",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

async fn course_mark_all_unread(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_all_read_state(
        &state.db,
        TopicContext::Course(course_id),
        topic_id,
        &auth,
        "unread",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

async fn group_mark_all_read(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_all_read_state(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        &auth,
        "read",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

async fn group_mark_all_unread(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<ReadStateParams>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_all_read_state(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        &auth,
        "unread",
        value_to_boolean(params.forced_read_state.as_deref()),
    )
    .await
}

/// Context-level mark-all wrappers.
async fn course_read_all_topics(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    mark_all_ctx_topics_read(&state.db, TopicContext::Course(course_id), &auth).await
}

async fn group_read_all_topics(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    mark_all_ctx_topics_read(&state.db, TopicContext::Group(group_id), &auth).await
}

/// Subscription wrappers (group context).
async fn group_subscribe_topic(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_subscription(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        &auth,
        true,
    )
    .await
}

async fn group_unsubscribe_topic(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    set_ctx_subscription(
        &state.db,
        TopicContext::Group(group_id),
        topic_id,
        &auth,
        false,
    )
    .await
}

// ---------------------------------------------------------------------------
// View + entry_list + duplicate (Canvas view / entry_list / duplicate)
// ---------------------------------------------------------------------------

/// Build the Canvas `view` response body for either context.
async fn ctx_topic_view(
    state: &AppState,
    ctx: TopicContext,
    topic_id: i64,
    auth: &AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_read_access(&state.db, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let view = load_nested_entries(&state.db, topic_id).await?;
    let entry_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM discussion_entries WHERE topic_id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_all(&state.db)
    .await?;

    let participants: Vec<serde_json::Value> = sqlx::query_as::<_, (i64, String)>(
        r#"
        SELECT DISTINCT u.id, u.name
        FROM users u
        JOIN discussion_entries e ON e.user_id = u.id
        WHERE e.topic_id = $1 AND e.workflow_state <> 'deleted'
        ORDER BY u.id
        "#,
    )
    .bind(topic_id)
    .fetch_all(&state.db)
    .await?
    .into_iter()
    .map(|(id, display_name)| serde_json::json!({ "id": id, "display_name": display_name }))
    .collect();

    let read_ids: Vec<i64> = if entry_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar(
            r#"
            SELECT entry_id FROM discussion_entry_reads
            WHERE user_id = $1 AND workflow_state = 'read' AND entry_id = ANY($2)
            "#,
        )
        .bind(auth.id)
        .bind(&entry_ids)
        .fetch_all(&state.db)
        .await?
    };
    let unread_entries: Vec<i64> = entry_ids
        .iter()
        .copied()
        .filter(|id| !read_ids.contains(id))
        .collect();

    let forced_entries: Vec<i64> = if entry_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_scalar(
            r#"
            SELECT entry_id FROM discussion_entry_reads
            WHERE user_id = $1 AND forced_read_state = TRUE AND entry_id = ANY($2)
            "#,
        )
        .bind(auth.id)
        .bind(&entry_ids)
        .fetch_all(&state.db)
        .await?
    };

    let ratings: Vec<(i64, i32)> = if entry_ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT entry_id, rating FROM discussion_entry_ratings WHERE user_id = $1 AND entry_id = ANY($2)",
        )
        .bind(auth.id)
        .bind(&entry_ids)
        .fetch_all(&state.db)
        .await?
    };
    let entry_ratings: serde_json::Map<String, serde_json::Value> = ratings
        .into_iter()
        .map(|(entry_id, rating)| (entry_id.to_string(), serde_json::json!(rating)))
        .collect();

    Ok(Json(serde_json::json!({
        "view": view,
        "participants": participants,
        "unread_entries": unread_entries,
        "forced_entries": forced_entries,
        "entry_ratings": entry_ratings,
        "new_entries": [],
    })))
}

/// GET /courses/{courseId}/discussion_topics/{topicId}/view.
async fn course_topic_view(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    ctx_topic_view(&state, TopicContext::Course(course_id), topic_id, &auth).await
}

/// GET /groups/{groupId}/discussion_topics/{topicId}/view.
async fn group_topic_view(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    ctx_topic_view(&state, TopicContext::Group(group_id), topic_id, &auth).await
}

/// Query params of the entry_list endpoints: `ids[]=1&ids[]=2`.
#[derive(Debug, Deserialize, Default)]
pub struct EntryListParams {
    #[serde(rename = "ids[]", default)]
    pub ids_bracket: Vec<i64>,
    #[serde(default)]
    pub ids: Vec<i64>,
}

impl EntryListParams {
    fn entry_ids(&self) -> Vec<i64> {
        if !self.ids_bracket.is_empty() {
            self.ids_bracket.clone()
        } else {
            self.ids.clone()
        }
    }
}

/// Canvas `entry_list`: the requested entries in id order; deleted entries
/// are included and flagged with `deleted: true`.
async fn ctx_entry_list(
    state: &AppState,
    ctx: TopicContext,
    topic_id: i64,
    params: &EntryListParams,
    auth: &AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_read_access(&state.db, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let ids = params.entry_ids();
    if ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let rows: Vec<DiscussionEntryRow> = sqlx::query_as(
        r#"
        SELECT id, topic_id, parent_id, user_id, message, workflow_state, created_at, updated_at
        FROM discussion_entries
        WHERE topic_id = $1 AND id = ANY($2)
        ORDER BY id
        "#,
    )
    .bind(topic_id)
    .bind(&ids)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.into_iter()
            .map(|row| {
                let mut entry = DiscussionEntry::from(row);
                entry.deleted = (entry.workflow_state == "deleted").then_some(true);
                serde_json::to_value(entry).expect("entry serializes")
            })
            .collect(),
    ))
}

/// GET /courses/{courseId}/discussion_topics/{topicId}/entry_list.
async fn course_entry_list(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<EntryListParams>,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    ctx_entry_list(
        &state,
        TopicContext::Course(course_id),
        topic_id,
        &params,
        &auth,
    )
    .await
}

/// GET /groups/{groupId}/discussion_topics/{topicId}/entry_list.
async fn group_entry_list(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    Query(params): Query<EntryListParams>,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    ctx_entry_list(
        &state,
        TopicContext::Group(group_id),
        topic_id,
        &params,
        &auth,
    )
    .await
}

/// Canvas `duplicate`: copies the topic into the same context. Course context
/// requires the duplicate right (teacher/admin); a group member may duplicate
/// their group's topics.
async fn ctx_duplicate_topic(
    state: &AppState,
    ctx: TopicContext,
    topic_id: i64,
    auth: &AuthUser,
) -> Result<(StatusCode, Json<DiscussionTopic>), ApiError> {
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    let allowed = match ctx {
        TopicContext::Course(_) => check_ctx_moderate_access(&state.db, ctx, auth).await?,
        TopicContext::Group(_) => check_ctx_read_access(&state.db, ctx, auth).await?,
    };
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    let row: DiscussionTopic = sqlx::query_as(
        r#"
        INSERT INTO discussion_topics (course_id, group_id, title, message, discussion_type, workflow_state, pinned, locked)
        SELECT course_id, group_id, title, message, discussion_type, 'active', FALSE, FALSE
        FROM discussion_topics
        WHERE id = $1
        RETURNING id, course_id, title, message, workflow_state, discussion_type, pinned, locked, created_at, updated_at
        "#,
    )
    .bind(topic_id)
    .fetch_one(&state.db)
    .await?;
    Ok((StatusCode::CREATED, Json(row)))
}

/// POST /courses/{courseId}/discussion_topics/{topicId}/duplicate.
async fn course_duplicate_topic(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<DiscussionTopic>), ApiError> {
    ctx_duplicate_topic(&state, TopicContext::Course(course_id), topic_id, &auth).await
}

/// POST /groups/{groupId}/discussion_topics/{topicId}/duplicate.
async fn group_duplicate_topic(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<DiscussionTopic>), ApiError> {
    ctx_duplicate_topic(&state, TopicContext::Group(group_id), topic_id, &auth).await
}

// ---------------------------------------------------------------------------
// Summary disable + feedback (Canvas disable_summary / summary_feedback)
// ---------------------------------------------------------------------------

/// PUT .../summaries/disable — Canvas `user_can_summarize?` holders only.
async fn course_disable_summary(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_course_topic(&state.db, course_id, topic_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    sqlx::query(
        "UPDATE discussion_topics SET summary_enabled = FALSE, updated_at = now() WHERE id = $1",
    )
    .bind(topic_id)
    .execute(&state.db)
    .await?;
    Ok(Json(serde_json::json!({ "success": true })).into_response())
}

/// PUT /groups/.../summaries/disable — summaries do not exist in group
/// contexts (`user_can_summarize?` is false), so 403 after the checks.
async fn group_disable_summary(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    ensure_group_topic(&state.db, group_id, topic_id).await?;
    Err(ApiError::Forbidden)
}

/// Body of the summary feedback endpoint: `_action` + optional `comment`.
#[derive(Debug, Deserialize, Default)]
pub struct SummaryFeedbackParams {
    #[serde(rename = "_action", alias = "action", default)]
    pub action: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

/// POST .../summaries/{summaryId}/feedback (Canvas summary_feedback).
async fn course_summary_feedback(
    State(state): State<AppState>,
    Path((course_id, topic_id, summary_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    payload: Option<Json<SummaryFeedbackParams>>,
) -> Result<Response, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_course_topic(&state.db, course_id, topic_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas looks the summary up among the topic's summaries: 404 otherwise.
    let summary: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM discussion_topic_summaries WHERE id = $1 AND discussion_topic_id = $2",
    )
    .bind(summary_id)
    .bind(topic_id)
    .fetch_optional(&state.db)
    .await?;
    summary.ok_or(ApiError::NotFound)?;

    let body = payload.map(|Json(body)| body).unwrap_or_default();
    let action = body.action.as_deref().unwrap_or_default();

    let (liked, disliked, comment): (bool, bool, Option<String>) = match action {
        "seen" => (false, false, None),
        "like" => (true, false, None),
        "dislike" => (false, true, None),
        "add_comment" => {
            // Canvas answers 400 with `{"error": ...}` for a blank or
            // over-long comment.
            let Some(comment) = body
                .comment
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(canvas_error(
                    StatusCode::BAD_REQUEST,
                    "Comment is required.",
                ));
            };
            if comment.chars().count() > 1024 {
                return Ok(canvas_error(
                    StatusCode::BAD_REQUEST,
                    "Comment is too long (maximum is 1024 characters).",
                ));
            }
            (false, false, Some(comment.to_string()))
        }
        "reset_like" => (false, false, None),
        "disable_summary" => (false, false, None),
        _ => return Ok(canvas_error(StatusCode::BAD_REQUEST, "Invalid action.")),
    };

    sqlx::query(
        r#"
        INSERT INTO discussion_topic_summary_feedback (summary_id, user_id, liked, disliked, comment)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (summary_id, user_id) DO UPDATE
        SET liked = EXCLUDED.liked OR discussion_topic_summary_feedback.liked,
            disliked = EXCLUDED.disliked OR discussion_topic_summary_feedback.disliked,
            comment = COALESCE(EXCLUDED.comment, discussion_topic_summary_feedback.comment),
            updated_at = now()
        "#,
    )
    .bind(summary_id)
    .bind(auth.id)
    .bind(liked)
    .bind(disliked)
    .bind(&comment)
    .execute(&state.db)
    .await?;

    let (liked, disliked, comment): (bool, bool, Option<String>) = sqlx::query_as(
        "SELECT liked, disliked, comment FROM discussion_topic_summary_feedback WHERE summary_id = $1 AND user_id = $2",
    )
    .bind(summary_id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(serde_json::json!({
        "liked": liked,
        "disliked": disliked,
        "comment": comment,
    }))
    .into_response())
}

/// POST /groups/.../summaries/{summaryId}/feedback — 403 like the other group
/// summary routes.
async fn group_summary_feedback(
    State(state): State<AppState>,
    Path((group_id, topic_id, _summary_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    _payload: Option<Json<SummaryFeedbackParams>>,
) -> Result<Response, ApiError> {
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    ensure_group_topic(&state.db, group_id, topic_id).await?;
    Err(ApiError::Forbidden)
}

// ---------------------------------------------------------------------------
// Insights (Canvas insight / insight_generation / insight_entries /
// insight_entry_update)
// ---------------------------------------------------------------------------

/// Canvas gates insights behind the `discussion_insights` course feature and
/// restricts them to instructors / read-as-admin users. This workspace has no
/// insight store, so authorized callers get the "no insight generated yet"
/// shapes: `{"workflow_state": null}`, `[]` and `{}`.
async fn check_ctx_insight_access(
    state: &AppState,
    ctx: TopicContext,
    topic_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_read_access(&state.db, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }
    if !check_ctx_moderate_access(&state.db, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

async fn course_insight(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_ctx_insight_access(&state, TopicContext::Course(course_id), topic_id, &auth).await?;
    Ok(Json(serde_json::json!({ "workflow_state": null })))
}

async fn course_insight_generation(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    check_ctx_insight_access(&state, TopicContext::Course(course_id), topic_id, &auth).await?;
    Ok(Json(serde_json::json!({})))
}

async fn course_insight_entries(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    check_ctx_insight_access(&state, TopicContext::Course(course_id), topic_id, &auth).await?;
    Ok(Json(Vec::new()))
}

/// Body of the insight entry feedback: `relevance_human_feedback_action` and
/// `relevance_human_feedback_notes`.
#[derive(Debug, Deserialize, Default)]
pub struct InsightEntryFeedback {
    #[serde(rename = "relevance_human_feedback_action", default)]
    pub action: Option<String>,
    #[serde(rename = "relevance_human_feedback_notes", default)]
    pub notes: Option<String>,
}

async fn course_insight_entry_update(
    State(state): State<AppState>,
    Path((course_id, topic_id, entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    payload: Option<Json<InsightEntryFeedback>>,
) -> Result<Response, ApiError> {
    check_ctx_insight_access(&state, TopicContext::Course(course_id), topic_id, &auth).await?;

    let entry: Option<(i64,)> =
        sqlx::query_as("SELECT topic_id FROM discussion_entries WHERE id = $1 AND topic_id = $2")
            .bind(entry_id)
            .bind(topic_id)
            .fetch_optional(&state.db)
            .await?;
    if entry.is_none() {
        return Ok(canvas_error(StatusCode::NOT_FOUND, "Entry not found."));
    }

    let body = payload.map(|Json(body)| body).unwrap_or_default();
    let action = body.action.as_deref().unwrap_or_default();
    if !["like", "dislike", "reset_like"].contains(&action) {
        return Ok(canvas_error(StatusCode::BAD_REQUEST, "Invalid action."));
    }
    if body.notes.is_none() {
        return Ok(canvas_error(StatusCode::BAD_REQUEST, "Missing notes."));
    }
    Ok(Json(serde_json::json!({})).into_response())
}

/// Group insight routes: a group topic has no course, so Canvas'
/// `user_can_access_insights?` is false — 403 after the context checks.
async fn group_insight_forbidden(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    ensure_group_topic(&state.db, group_id, topic_id).await?;
    Err(ApiError::Forbidden)
}

/// PUT /groups/.../insights/entries/{entryId} — 403 like the other group
/// insight routes; the entry id segment keeps this a three-param route, so it
/// cannot share the two-param handler above.
async fn group_insight_entry_update(
    State(state): State<AppState>,
    Path((group_id, topic_id, _entry_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    ensure_group_read_access(&state.db, group_id, &auth).await?;
    ensure_group_topic(&state.db, group_id, topic_id).await?;
    Err(ApiError::Forbidden)
}

// ---------------------------------------------------------------------------
// Accessibility scans (Canvas accessibility_scan / accessibility_queue_scan)
// ---------------------------------------------------------------------------

/// Canvas requires the topic :update right plus the account-level
/// accessibility checker; the workspace has no checker, so authorized callers
/// get Canvas' unauthorized response (401) like a checker-disabled account.
async fn ctx_accessibility_scan(
    state: &AppState,
    ctx: TopicContext,
    topic_id: i64,
    auth: &AuthUser,
) -> Result<StatusCode, ApiError> {
    ensure_ctx_topic(&state.db, ctx, topic_id).await?;
    if !check_ctx_moderate_access(&state.db, ctx, auth).await? {
        return Err(ApiError::Forbidden);
    }
    Err(ApiError::Unauthorized)
}

async fn course_accessibility_scan(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    ctx_accessibility_scan(&state, TopicContext::Course(course_id), topic_id, &auth).await
}

async fn course_accessibility_queue_scan(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    ctx_accessibility_scan(&state, TopicContext::Course(course_id), topic_id, &auth).await
}

async fn group_accessibility_scan(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    ctx_accessibility_scan(&state, TopicContext::Group(group_id), topic_id, &auth).await
}

async fn group_accessibility_queue_scan(
    State(state): State<AppState>,
    Path((group_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    ctx_accessibility_scan(&state, TopicContext::Group(group_id), topic_id, &auth).await
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/discussion_topics", post(create_topic))
        .route("/courses/{courseId}/discussion_topics", get(list_topics))
        .route("/courses/{courseId}/discussion_topics/{id}", get(get_topic))
        .route(
            "/courses/{courseId}/discussion_topics/{id}",
            put(update_topic),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{id}",
            delete(delete_topic),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entries",
            post(create_entry),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entries",
            get(list_entries),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}",
            get(get_entry).put(update_entry).delete(delete_entry),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/replies",
            post(add_reply),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/read",
            put(mark_topic_read).delete(mark_topic_unread),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/subscribed",
            put(subscribe_topic).delete(unsubscribe_topic),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/read",
            put(mark_entry_read).delete(mark_entry_unread),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/rating",
            post(rate_entry),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/summaries",
            get(find_course_summary),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/summaries",
            post(find_or_create_course_summary),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/summaries",
            get(find_group_summary),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/summaries",
            post(find_or_create_group_summary),
        )
        .route(
            "/courses/{courseId}/discussion_topics/update_discussion_types",
            put(update_discussion_types),
        )
        .route(
            "/courses/{courseId}/discussion_topics/migrate_disallow",
            put(migrate_disallow),
        )
        // Group-context topics and entries (Canvas group discussion parity).
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}",
            get(group_show_topic),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/entries",
            get(group_list_entries).post(group_create_entry),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}",
            get(group_get_entry)
                .put(group_update_entry)
                .delete(group_delete_entry),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}/replies",
            get(group_list_replies).post(group_add_reply),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}/read",
            put(group_mark_entry_read).delete(group_mark_entry_unread),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/entries/{entryId}/rating",
            post(group_rate_entry),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/read",
            put(group_mark_topic_read).delete(group_mark_topic_unread),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/subscribed",
            put(group_subscribe_topic).delete(group_unsubscribe_topic),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/replies",
            get(course_list_replies),
        )
        // read_all variants (context-level and topic-level).
        .route(
            "/courses/{courseId}/discussion_topics/read_all",
            put(course_read_all_topics),
        )
        .route(
            "/groups/{groupId}/discussion_topics/read_all",
            put(group_read_all_topics),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/read_all",
            put(course_mark_all_read).delete(course_mark_all_unread),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/read_all",
            put(group_mark_all_read).delete(group_mark_all_unread),
        )
        // view + entry_list (both contexts).
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/view",
            get(course_topic_view),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/view",
            get(group_topic_view),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entry_list",
            get(course_entry_list),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/entry_list",
            get(group_entry_list),
        )
        // duplicate (both contexts).
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/duplicate",
            post(course_duplicate_topic),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/duplicate",
            post(group_duplicate_topic),
        )
        // Summary disable + feedback (both contexts).
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/summaries/disable",
            put(course_disable_summary),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/summaries/disable",
            put(group_disable_summary),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/summaries/{summaryId}/feedback",
            post(course_summary_feedback),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/summaries/{summaryId}/feedback",
            post(group_summary_feedback),
        )
        // Insights (Canvas empty-state shapes).
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/insights",
            get(course_insight).post(course_insight_generation),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/insights",
            get(group_insight_forbidden).post(group_insight_forbidden),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/insights/entries",
            get(course_insight_entries),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/insights/entries",
            get(group_insight_forbidden),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/insights/entries/{entryId}",
            put(course_insight_entry_update),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/insights/entries/{entryId}",
            put(group_insight_entry_update),
        )
        // Accessibility scans (Canvas 401 shape).
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/accessibility/scan",
            post(course_accessibility_scan),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/accessibility/queue_scan",
            post(course_accessibility_queue_scan),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/accessibility/scan",
            post(group_accessibility_scan),
        )
        .route(
            "/groups/{groupId}/discussion_topics/{topicId}/accessibility/queue_scan",
            post(group_accessibility_queue_scan),
        )
}
