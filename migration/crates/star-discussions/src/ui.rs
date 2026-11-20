//! Pub data loaders for the server-rendered UI shell (star-ui, issue #72).
//!
//! Mirrors the extraction pattern of `star_courses::load_course_for_viewer` /
//! `load_course_tabs`: every query the discussions pages render lives here as
//! a `pub` fn with the SAME authorization gate the REST handlers enforce
//! (`check_enrolled_or_admin` / `check_teacher_or_admin`, which live as
//! crate-private helpers in lib.rs and are re-exported here for this module's
//! use). The shell handlers stay thin — they authorize, call a loader, and
//! render — so the UI cannot bypass `/api/v1` discussion authorization.
//!
//! Deviations from Canvas (documented in the PR):
//! - Entry lists are FLAT: the threaded `/view` endpoint is missing in the
//!   Rust API (see specs/ui-research/canvas-reference.md §5), so the show page
//!   renders `discussion_entries` ordered by `created_at` with `parent_id`
//!   rendered as an inline "in reply to" reference.
//! - No `delayed_post` column exists in this schema, so the new-topic form
//!   omits the delayed-post field Canvas offers.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::ApiError;

use crate::{check_enrolled_or_admin, check_teacher_or_admin};

/// One row of the discussions index for the viewing user — Canvas'
/// `discussion_topics#index` topic row (title, author, unread state, reply
/// count, last activity, pinned/locked icons) in one JOINed query.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TopicIndexRow {
    pub id: i64,
    pub title: String,
    pub message: Option<String>,
    pub workflow_state: String,
    pub discussion_type: String,
    pub pinned: bool,
    pub locked: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `users.name` of the topic author (NULL for legacy authorless topics).
    pub author_name: Option<String>,
    /// Per-viewer read state from `discussion_topic_reads`; the implicit
    /// default is unread (Canvas `change_read_state` semantics).
    pub read_state: Option<String>,
    /// Count of non-deleted entries (root entries + replies).
    pub entry_count: i64,
    /// Most recent entry timestamp, NULL when the topic has no entries.
    pub last_reply_at: Option<DateTime<Utc>>,
}

/// A discussion entry for the show page, joined with the author's name.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TopicEntryRow {
    pub id: i64,
    pub topic_id: i64,
    pub parent_id: Option<i64>,
    pub user_id: Option<i64>,
    pub message: Option<String>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `users.name` of the entry author (NULL when the author was deleted).
    pub author_name: Option<String>,
}

/// The full topic detail for the show page: the topic row itself plus the
/// author name and the flat entry list with author names.
#[derive(Debug, Clone)]
pub struct TopicDetail {
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
    pub author_name: Option<String>,
    pub entries: Vec<TopicEntryRow>,
}

/// The topic fields the create loader accepts. `locked` is included as an
/// explicit parameter (the prior session flagged the read-but-not-stored gap
/// in the API's `CreateTopic`): a locked topic starts closed for comments.
#[derive(Debug, Clone)]
pub struct NewTopic {
    pub title: String,
    pub message: Option<String>,
    pub discussion_type: Option<String>,
    pub pinned: bool,
    pub locked: bool,
}

/// Load the course's topic index for the viewing user (enrolled-or-admin
/// gate, same as `GET /courses/{courseId}/discussion_topics`).
///
/// Unread state comes from `discussion_topic_reads` (migration 0041):
/// `workflow_state = 'unread'` or the absence of a row both read as unread —
/// the implicit default Canvas' `change_read_state` documents.
pub async fn load_topic_index(
    db: &PgPool,
    course_id: i64,
    auth: &AuthUser,
) -> Result<Vec<TopicIndexRow>, ApiError> {
    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<TopicIndexRow> = sqlx::query_as(
        r#"
        SELECT
            t.id,
            t.title,
            t.message,
            t.workflow_state,
            t.discussion_type,
            t.pinned,
            t.locked,
            t.created_at,
            t.updated_at,
            u.name AS author_name,
            r.workflow_state AS read_state,
            (
                SELECT COUNT(*) FROM discussion_entries e
                WHERE e.topic_id = t.id AND e.workflow_state <> 'deleted'
            ) AS entry_count,
            (
                SELECT MAX(e.created_at) FROM discussion_entries e
                WHERE e.topic_id = t.id AND e.workflow_state <> 'deleted'
            ) AS last_reply_at
        FROM discussion_topics t
        LEFT JOIN users u ON u.id = t.user_id
        LEFT JOIN discussion_topic_reads r
            ON r.topic_id = t.id AND r.user_id = $2
        WHERE t.course_id = $1
          AND t.group_id IS NULL
          AND t.workflow_state <> 'deleted'
        ORDER BY t.pinned DESC, t.created_at DESC, t.id DESC
        "#,
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_all(db)
    .await?;

    Ok(rows)
}

/// Load one topic with its flat entry list (author names included) behind the
/// same enrolled-or-admin gate as `GET .../discussion_topics/{id}`: 404 when
/// the topic does not exist / is deleted / belongs to another course or a
/// group, 403 when the viewer is not enrolled.
pub async fn load_topic_for_viewer(
    db: &PgPool,
    course_id: i64,
    topic_id: i64,
    auth: &AuthUser,
) -> Result<TopicDetail, ApiError> {
    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let topic: Option<(
        i64,
        i64,
        String,
        Option<String>,
        String,
        String,
        bool,
        bool,
        DateTime<Utc>,
        DateTime<Utc>,
        Option<String>,
    )> = sqlx::query_as(
        r#"
        SELECT t.id, t.course_id, t.title, t.message, t.workflow_state,
               t.discussion_type, t.pinned, t.locked, t.created_at, t.updated_at,
               u.name
        FROM discussion_topics t
        LEFT JOIN users u ON u.id = t.user_id
        WHERE t.id = $1 AND t.course_id = $2 AND t.group_id IS NULL
          AND t.workflow_state <> 'deleted'
        "#,
    )
    .bind(topic_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    let Some(
        (
            id,
            topic_course_id,
            title,
            message,
            workflow_state,
            discussion_type,
            pinned,
            locked,
            created_at,
            updated_at,
            author_name,
        ),
    ) = topic
    else {
        return Err(ApiError::NotFound);
    };
    debug_assert_eq!(topic_course_id, course_id);

    let entries: Vec<TopicEntryRow> = sqlx::query_as(
        r#"
        SELECT e.id, e.topic_id, e.parent_id, e.user_id, e.message,
               e.workflow_state, e.created_at, e.updated_at, u.name AS author_name
        FROM discussion_entries e
        LEFT JOIN users u ON u.id = e.user_id
        WHERE e.topic_id = $1 AND e.workflow_state <> 'deleted'
        ORDER BY e.created_at ASC, e.id ASC
        "#,
    )
    .bind(topic_id)
    .fetch_all(db)
    .await?;

    Ok(TopicDetail {
        id,
        course_id,
        title,
        message,
        workflow_state,
        discussion_type,
        pinned,
        locked,
        created_at,
        updated_at,
        author_name,
        entries,
    })
}

/// Create a topic as the given viewer (teacher/admin gate, same as
/// `POST /courses/{courseId}/discussion_topics`). Returns the new topic id.
///
/// `locked` is stored — the API's `CreateTopic` reads the Canvas `locked`
/// flag in updates but drops it on create; the UI create flow passes it
/// through so the form's checkbox is honored.
pub async fn create_topic_for_viewer(
    db: &PgPool,
    course_id: i64,
    auth: &AuthUser,
    topic: &NewTopic,
) -> Result<i64, ApiError> {
    if !check_teacher_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    if topic.title.trim().is_empty() {
        return Err(ApiError::Validation("title cannot be empty".into()));
    }

    let discussion_type = topic.discussion_type.as_deref().unwrap_or("side_comment");
    if !["side_comment", "threaded", "flat", "not_threaded"].contains(&discussion_type) {
        return Err(ApiError::Validation(
            "discussion_type must be 'side_comment', 'threaded', 'flat', or 'not_threaded'".into(),
        ));
    }

    // Pinned topics join the bottom of the pinned list (acts_as_list
    // add_to_list_bottom, same as the API's create).
    let position: Option<i64> = if topic.pinned {
        sqlx::query_scalar(
            r#"
            SELECT COALESCE(MAX(position), 0) + 1 FROM discussion_topics
            WHERE course_id = $1 AND group_id IS NULL AND pinned AND workflow_state <> 'deleted'
            "#,
        )
        .bind(course_id)
        .fetch_one(db)
        .await?
    } else {
        None
    };

    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO discussion_topics
            (course_id, title, message, discussion_type, workflow_state,
             pinned, locked, position, user_id)
        VALUES ($1, $2, $3, $4, 'active', $5, $6, $7, $8)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(topic.title.trim())
    .bind(&topic.message)
    .bind(discussion_type)
    .bind(topic.pinned)
    .bind(topic.locked)
    .bind(position)
    .bind(auth.id)
    .fetch_one(db)
    .await?;

    Ok(id)
}

/// Post a root entry (or an inline reply, via `parent_id`) on a topic — same
/// gate and same parent-validation as
/// `POST /courses/{courseId}/discussion_topics/{topicId}/entries`. Returns
/// the new entry id.
pub async fn create_entry_for_viewer(
    db: &PgPool,
    course_id: i64,
    topic_id: i64,
    auth: &AuthUser,
    message: &str,
    parent_id: Option<i64>,
) -> Result<i64, ApiError> {
    let topic: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(db)
    .await?;
    let (topic_course_id,) = topic.ok_or(ApiError::NotFound)?;
    if topic_course_id != course_id {
        return Err(ApiError::NotFound);
    }

    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    if message.trim().is_empty() {
        return Err(ApiError::Validation("message cannot be empty".into()));
    }

    if let Some(parent_id) = parent_id {
        let parent: Option<(i64,)> = sqlx::query_as(
            "SELECT topic_id FROM discussion_entries \
             WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(parent_id)
        .fetch_optional(db)
        .await?;
        match parent {
            Some((parent_topic_id,)) if parent_topic_id == topic_id => {}
            _ => {
                return Err(ApiError::Validation(
                    "parent_id must belong to the same topic".into(),
                ))
            }
        }
    }

    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO discussion_entries (topic_id, parent_id, user_id, message, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id
        "#,
    )
    .bind(topic_id)
    .bind(parent_id)
    .bind(auth.id)
    .bind(message)
    .fetch_one(db)
    .await?;

    Ok(id)
}

/// Reply to a specific entry — the form-target equivalent of
/// `POST .../entries/{entryId}/replies`. Shares the entry loader's gate.
pub async fn create_reply_for_viewer(
    db: &PgPool,
    course_id: i64,
    topic_id: i64,
    entry_id: i64,
    auth: &AuthUser,
    message: &str,
) -> Result<i64, ApiError> {
    // 404 unless the topic + entry exist and belong together (Canvas
    // require_topic + require_entry ordering), 403 when unenrolled.
    let topic: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(db)
    .await?;
    let (topic_course_id,) = topic.ok_or(ApiError::NotFound)?;
    if topic_course_id != course_id {
        return Err(ApiError::NotFound);
    }

    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let entry: Option<(i64,)> = sqlx::query_as(
        "SELECT topic_id FROM discussion_entries \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(entry_id)
    .fetch_optional(db)
    .await?;
    let (entry_topic_id,) = entry.ok_or(ApiError::NotFound)?;
    if entry_topic_id != topic_id {
        return Err(ApiError::NotFound);
    }

    if message.trim().is_empty() {
        return Err(ApiError::Validation("message cannot be empty".into()));
    }

    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO discussion_entries (topic_id, parent_id, user_id, message, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id
        "#,
    )
    .bind(topic_id)
    .bind(entry_id)
    .bind(auth.id)
    .bind(message)
    .fetch_one(db)
    .await?;

    Ok(id)
}

/// Whether the viewer has NOT read the topic yet — the pre-mark state the
/// show page renders. Reads `discussion_topic_reads` behind the same gate;
/// missing row means unread (implicit default).
pub async fn load_topic_read_state(
    db: &PgPool,
    course_id: i64,
    topic_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    let topic: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(db)
    .await?;
    let (topic_course_id,) = topic.ok_or(ApiError::NotFound)?;
    if topic_course_id != course_id {
        return Err(ApiError::NotFound);
    }

    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let state: Option<(String,)> = sqlx::query_as(
        "SELECT workflow_state FROM discussion_topic_reads \
         WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;

    Ok(state.map(|(s,)| s).as_deref() != Some("read"))
}

/// Mark a topic read/unread for the viewer — the same upsert Canvas'
/// `change_read_state` performs (implicit unread default, no row written when
/// the state already matches).
pub async fn mark_topic_read_for_viewer(
    db: &PgPool,
    course_id: i64,
    topic_id: i64,
    auth: &AuthUser,
    read: bool,
) -> Result<(), ApiError> {
    // Same 404-before-403 gate as the API read endpoints.
    let topic: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM discussion_topics \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(topic_id)
    .fetch_optional(db)
    .await?;
    let (topic_course_id,) = topic.ok_or(ApiError::NotFound)?;
    if topic_course_id != course_id {
        return Err(ApiError::NotFound);
    }

    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let new_state = if read { "read" } else { "unread" };

    let current: Option<(String,)> = sqlx::query_as(
        "SELECT workflow_state FROM discussion_topic_reads \
         WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    let current_state = current
        .map(|(state,)| state)
        .unwrap_or_else(|| "unread".to_string());
    if current_state == new_state {
        return Ok(());
    }

    sqlx::query(
        r#"
        INSERT INTO discussion_topic_reads (topic_id, user_id, workflow_state, forced_read_state, read_at)
        VALUES ($1, $2, $3, NULL, CASE WHEN $3 = 'read' THEN now() ELSE NULL END)
        ON CONFLICT (topic_id, user_id) DO UPDATE
        SET workflow_state = EXCLUDED.workflow_state,
            forced_read_state = discussion_topic_reads.forced_read_state,
            read_at = CASE WHEN EXCLUDED.workflow_state = 'read'
                           THEN now() ELSE discussion_topic_reads.read_at END,
            updated_at = now()
        "#,
    )
    .bind(topic_id)
    .bind(auth.id)
    .bind(new_state)
    .execute(db)
    .await?;

    Ok(())
}
