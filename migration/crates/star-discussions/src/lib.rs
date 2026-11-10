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
//! - PUT    /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/read — mark entry read (enrolled)
//! - DELETE /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/read — mark entry unread (enrolled)
//! - POST   /courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/rating — rate entry 0/1 (enrolled)

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
}

/// Request body for creating a discussion entry.
#[derive(Debug, Deserialize)]
pub struct CreateEntry {
    pub message: String,
    pub parent_id: Option<i64>,
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
        RETURNING id, course_id, title, message, workflow_state, discussion_type, created_at, updated_at
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
        SELECT id, course_id, title, message, workflow_state, discussion_type, created_at, updated_at
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
        SELECT id, course_id, title, message, workflow_state, discussion_type, created_at, updated_at
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
        SELECT id, course_id, title, message, workflow_state, discussion_type, created_at, updated_at
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
        SET title = $1, message = $2, discussion_type = $3, updated_at = now()
        WHERE id = $4 AND course_id = $5 AND workflow_state <> 'deleted'
        RETURNING id, course_id, title, message, workflow_state, discussion_type, created_at, updated_at
        "#,
    )
    .bind(&title)
    .bind(&message)
    .bind(&discussion_type)
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
            "/courses/{courseId}/discussion_topics/{topicId}/read",
            put(mark_topic_read).delete(mark_topic_unread),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/read",
            put(mark_entry_read).delete(mark_entry_unread),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/entries/{entryId}/rating",
            post(rate_entry),
        )
}
