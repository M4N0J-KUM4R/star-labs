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

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
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
}
