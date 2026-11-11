//! star-learning-dates — Canvas learning object dates (`learning_object_dates_api`).
//!
//! Endpoints:
//! - GET /courses/{courseId}/assignments/{assignmentId}/date_details
//! - PUT /courses/{courseId}/assignments/{assignmentId}/date_details
//! - GET /courses/{courseId}/quizzes/{quizId}/date_details
//! - PUT /courses/{courseId}/quizzes/{quizId}/date_details
//! - GET /courses/{courseId}/discussion_topics/{topicId}/date_details
//! - PUT /courses/{courseId}/discussion_topics/{topicId}/date_details
//! - GET /courses/{courseId}/pages/{urlOrId}/date_details
//! - PUT /courses/{courseId}/pages/{urlOrId}/date_details
//!
//! Mirrors Canvas `LearningObjectDatesController`: the response is
//! `LearningObjectDates` (id, due_at, lock_at, unlock_at,
//! only_visible_to_overrides, graded, visible_to_everyone, overrides) and the
//! PUT answers `204 No Content`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
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

/// Learning object kinds that expose `date_details`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asset {
    Assignment,
    Quiz,
    DiscussionTopic,
    WikiPage,
}

impl Asset {
    fn as_str(self) -> &'static str {
        match self {
            Asset::Assignment => "Assignment",
            Asset::Quiz => "Quiz",
            Asset::DiscussionTopic => "DiscussionTopic",
            Asset::WikiPage => "WikiPage",
        }
    }

    /// Canvas `graded` flag: only assignable objects have a due date.
    fn graded(self) -> bool {
        matches!(self, Asset::Assignment | Asset::Quiz)
    }

    fn table(self) -> &'static str {
        match self {
            Asset::Assignment => "assignments",
            Asset::Quiz => "quizzes",
            Asset::DiscussionTopic => "discussion_topics",
            Asset::WikiPage => "pages",
        }
    }
}

/// Canvas `AssignmentOverride` JSON.
#[derive(Debug, Clone, Serialize)]
pub struct DateOverride {
    pub id: i64,
    pub title: Option<String>,
    pub student_ids: Vec<i64>,
    pub group_id: Option<i64>,
    pub course_section_id: Option<i64>,
    pub noop_id: Option<i64>,
    pub due_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
}

/// Canvas `LearningObjectDates` JSON.
#[derive(Debug, Clone, Serialize)]
pub struct DateDetails {
    pub id: i64,
    pub due_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub only_visible_to_overrides: bool,
    pub graded: bool,
    pub visible_to_everyone: bool,
    pub overrides: Vec<DateOverride>,
}

#[derive(Debug, sqlx::FromRow)]
struct DateRow {
    id: i64,
    due_at: Option<DateTime<Utc>>,
    unlock_at: Option<DateTime<Utc>>,
    lock_at: Option<DateTime<Utc>>,
    only_visible_to_overrides: bool,
}

#[derive(Debug, sqlx::FromRow)]
struct OverrideRow {
    id: i64,
    title: Option<String>,
    set_type: Option<String>,
    set_id: Option<i64>,
    student_ids: Vec<i64>,
    due_at: Option<DateTime<Utc>>,
    unlock_at: Option<DateTime<Utc>>,
    lock_at: Option<DateTime<Utc>>,
}

impl From<OverrideRow> for DateOverride {
    fn from(row: OverrideRow) -> Self {
        let (group_id, course_section_id, noop_id) = match row.set_type.as_deref() {
            Some("Group") => (row.set_id, None, None),
            Some("CourseSection") => (None, row.set_id, None),
            Some("Noop") => (None, None, row.set_id),
            _ => (None, None, None),
        };
        DateOverride {
            id: row.id,
            title: row.title,
            student_ids: row.student_ids,
            group_id,
            course_section_id,
            noop_id,
            due_at: row.due_at,
            unlock_at: row.unlock_at,
            lock_at: row.lock_at,
        }
    }
}

/// PUT body. Outer `Option` = key absent, inner `Option` = explicit `null`.
#[derive(Debug, Deserialize)]
pub struct UpdateDates {
    #[serde(default)]
    pub due_at: Option<Option<DateTime<Utc>>>,
    #[serde(default)]
    pub unlock_at: Option<Option<DateTime<Utc>>>,
    #[serde(default)]
    pub lock_at: Option<Option<DateTime<Utc>>>,
    #[serde(default)]
    pub only_visible_to_overrides: Option<bool>,
    #[serde(default)]
    pub assignment_overrides: Option<Vec<UpdateOverride>>,
}

/// One override in a PUT body (`id` is accepted but ignored: the list replaces
/// every existing override, as Canvas `assignment_overrides=` does).
#[derive(Debug, Deserialize)]
pub struct UpdateOverride {
    pub id: Option<i64>,
    pub title: Option<String>,
    pub set_type: Option<String>,
    pub set_id: Option<i64>,
    pub group_id: Option<i64>,
    pub course_section_id: Option<i64>,
    pub noop_id: Option<i64>,
    #[serde(default)]
    pub student_ids: Vec<i64>,
    pub due_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Permission helpers (same model as the other course-scoped crates)
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
// Lookup helpers
// ---------------------------------------------------------------------------

/// Resolves the learning object in the course, 404 when it does not exist.
async fn resolve_asset(
    pool: &PgPool,
    course_id: i64,
    asset: Asset,
    key: &str,
) -> Result<i64, ApiError> {
    let sql = if asset == Asset::WikiPage {
        format!(
            "SELECT id FROM {} WHERE course_id = $1 AND (url = $2 OR id::text = $2) LIMIT 1",
            asset.table()
        )
    } else {
        format!(
            "SELECT id FROM {} WHERE course_id = $1 AND id::text = $2 LIMIT 1",
            asset.table()
        )
    };
    let row: Option<(i64,)> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(key)
        .fetch_optional(pool)
        .await?;
    row.map(|(id,)| id).ok_or(ApiError::NotFound)
}

/// Loads (or defaults) the stored dates for one learning object.
async fn load_details(
    pool: &PgPool,
    course_id: i64,
    asset: Asset,
    object_id: i64,
) -> Result<DateDetails, ApiError> {
    let stored: Option<DateRow> = sqlx::query_as(
        r#"
        SELECT id, due_at, unlock_at, lock_at, only_visible_to_overrides
        FROM learning_object_dates
        WHERE learning_object_type = $1 AND learning_object_id = $2
        "#,
    )
    .bind(asset.as_str())
    .bind(object_id)
    .fetch_optional(pool)
    .await?;

    // No date_details row yet: fall back to the assignment's own due date,
    // matching Canvas (the assignment is the overridable object).
    let stored_is_none = stored.is_none();
    let mut details = match stored {
        Some(row) => DateDetails {
            id: object_id,
            due_at: row.due_at,
            lock_at: row.lock_at,
            unlock_at: row.unlock_at,
            only_visible_to_overrides: row.only_visible_to_overrides,
            graded: asset.graded(),
            visible_to_everyone: !row.only_visible_to_overrides,
            overrides: Vec::new(),
        },
        None => DateDetails {
            id: object_id,
            due_at: None,
            lock_at: None,
            unlock_at: None,
            only_visible_to_overrides: false,
            graded: asset.graded(),
            visible_to_everyone: true,
            overrides: Vec::new(),
        },
    };

    if stored_is_none && asset == Asset::Assignment {
        details.due_at = sqlx::query_scalar("SELECT due_at FROM assignments WHERE id = $1")
            .bind(object_id)
            .fetch_one(pool)
            .await?;
    }

    let rows: Vec<OverrideRow> = sqlx::query_as(
        r#"
        SELECT o.id, o.title, o.set_type, o.set_id, o.student_ids,
               o.due_at, o.unlock_at, o.lock_at
        FROM learning_object_date_overrides o
        JOIN learning_object_dates d ON d.id = o.learning_object_date_id
        WHERE d.course_id = $1
          AND d.learning_object_type = $2
          AND d.learning_object_id = $3
        ORDER BY o.id
        "#,
    )
    .bind(course_id)
    .bind(asset.as_str())
    .bind(object_id)
    .fetch_all(pool)
    .await?;

    details.overrides = rows.into_iter().map(DateOverride::from).collect();
    Ok(details)
}

/// Applies a `date_details` update. Returns 204 with no body on success.
async fn apply_update(
    pool: &PgPool,
    course_id: i64,
    asset: Asset,
    object_id: i64,
    payload: UpdateDates,
) -> Result<StatusCode, ApiError> {
    let (set_due, due_at): (bool, Option<DateTime<Utc>>) = match payload.due_at {
        Some(value) => (true, value),
        None => (false, None),
    };
    let (set_unlock, unlock_at): (bool, Option<DateTime<Utc>>) = match payload.unlock_at {
        Some(value) => (true, value),
        None => (false, None),
    };
    let (set_lock, lock_at): (bool, Option<DateTime<Utc>>) = match payload.lock_at {
        Some(value) => (true, value),
        None => (false, None),
    };

    let existing: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM learning_object_dates
        WHERE learning_object_type = $1 AND learning_object_id = $2
        "#,
    )
    .bind(asset.as_str())
    .bind(object_id)
    .fetch_optional(pool)
    .await?;

    let date_id = match existing {
        Some(id) => {
            sqlx::query(
                r#"
                UPDATE learning_object_dates SET
                    due_at = CASE WHEN $2 THEN $3 ELSE due_at END,
                    unlock_at = CASE WHEN $4 THEN $5 ELSE unlock_at END,
                    lock_at = CASE WHEN $6 THEN $7 ELSE lock_at END,
                    only_visible_to_overrides =
                        COALESCE($8, only_visible_to_overrides),
                    updated_at = now()
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(set_due)
            .bind(due_at)
            .bind(set_unlock)
            .bind(unlock_at)
            .bind(set_lock)
            .bind(lock_at)
            .bind(payload.only_visible_to_overrides)
            .execute(pool)
            .await?;
            id
        }
        None => {
            sqlx::query_scalar(
                r#"
            INSERT INTO learning_object_dates
                (course_id, learning_object_type, learning_object_id,
                 due_at, unlock_at, lock_at, only_visible_to_overrides)
            VALUES ($1, $2, $3, $4, $5, $6, COALESCE($7, false))
            RETURNING id
            "#,
            )
            .bind(course_id)
            .bind(asset.as_str())
            .bind(object_id)
            .bind(due_at)
            .bind(unlock_at)
            .bind(lock_at)
            .bind(payload.only_visible_to_overrides)
            .fetch_one(pool)
            .await?
        }
    };

    if asset == Asset::Assignment && set_due {
        sqlx::query("UPDATE assignments SET due_at = $1, updated_at = now() WHERE id = $2")
            .bind(due_at)
            .bind(object_id)
            .execute(pool)
            .await?;
    }

    if let Some(overrides) = payload.assignment_overrides {
        sqlx::query(
            "DELETE FROM learning_object_date_overrides WHERE learning_object_date_id = $1",
        )
        .bind(date_id)
        .execute(pool)
        .await?;

        for item in overrides {
            let (set_type, set_id) = if let Some(set_type) = item.set_type {
                (Some(set_type), item.set_id)
            } else if let Some(group_id) = item.group_id {
                (Some("Group".to_string()), Some(group_id))
            } else if let Some(section_id) = item.course_section_id {
                (Some("CourseSection".to_string()), Some(section_id))
            } else if let Some(noop_id) = item.noop_id {
                (Some("Noop".to_string()), Some(noop_id))
            } else if !item.student_ids.is_empty() {
                (Some("ADHOC".to_string()), None)
            } else {
                (None, None)
            };

            sqlx::query(
                r#"
                INSERT INTO learning_object_date_overrides
                    (learning_object_date_id, title, set_type, set_id,
                     student_ids, due_at, unlock_at, lock_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
            )
            .bind(date_id)
            .bind(item.title)
            .bind(set_type)
            .bind(set_id)
            .bind(item.student_ids.clone())
            .bind(item.due_at)
            .bind(item.unlock_at)
            .bind(item.lock_at)
            .execute(pool)
            .await?;
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn show(
    state: AppState,
    auth: AuthUser,
    course_id: i64,
    asset: Asset,
    key: String,
) -> Result<Json<DateDetails>, ApiError> {
    let object_id = resolve_asset(&state.db, course_id, asset, &key).await?;
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(
        load_details(&state.db, course_id, asset, object_id).await?,
    ))
}

async fn update(
    state: AppState,
    auth: AuthUser,
    course_id: i64,
    asset: Asset,
    key: String,
    payload: UpdateDates,
) -> Result<StatusCode, ApiError> {
    let object_id = resolve_asset(&state.db, course_id, asset, &key).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    apply_update(&state.db, course_id, asset, object_id, payload).await
}

async fn get_assignment_dates(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DateDetails>, ApiError> {
    show(
        state,
        auth,
        course_id,
        Asset::Assignment,
        assignment_id.to_string(),
    )
    .await
}

async fn put_assignment_dates(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateDates>,
) -> Result<StatusCode, ApiError> {
    update(
        state,
        auth,
        course_id,
        Asset::Assignment,
        assignment_id.to_string(),
        payload,
    )
    .await
}

async fn get_quiz_dates(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DateDetails>, ApiError> {
    show(state, auth, course_id, Asset::Quiz, quiz_id.to_string()).await
}

async fn put_quiz_dates(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateDates>,
) -> Result<StatusCode, ApiError> {
    update(
        state,
        auth,
        course_id,
        Asset::Quiz,
        quiz_id.to_string(),
        payload,
    )
    .await
}

async fn get_topic_dates(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<DateDetails>, ApiError> {
    show(
        state,
        auth,
        course_id,
        Asset::DiscussionTopic,
        topic_id.to_string(),
    )
    .await
}

async fn put_topic_dates(
    State(state): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateDates>,
) -> Result<StatusCode, ApiError> {
    update(
        state,
        auth,
        course_id,
        Asset::DiscussionTopic,
        topic_id.to_string(),
        payload,
    )
    .await
}

async fn get_page_dates(
    State(state): State<AppState>,
    Path((course_id, url_or_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<DateDetails>, ApiError> {
    show(state, auth, course_id, Asset::WikiPage, url_or_id).await
}

async fn put_page_dates(
    State(state): State<AppState>,
    Path((course_id, url_or_id)): Path<(i64, String)>,
    auth: AuthUser,
    Json(payload): Json<UpdateDates>,
) -> Result<StatusCode, ApiError> {
    update(state, auth, course_id, Asset::WikiPage, url_or_id, payload).await
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/date_details",
            get(get_assignment_dates),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/date_details",
            put(put_assignment_dates),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/date_details",
            get(get_quiz_dates),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/date_details",
            put(put_quiz_dates),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/date_details",
            get(get_topic_dates),
        )
        .route(
            "/courses/{courseId}/discussion_topics/{topicId}/date_details",
            put(put_topic_dates),
        )
        .route(
            "/courses/{courseId}/pages/{urlOrId}/date_details",
            get(get_page_dates),
        )
        .route(
            "/courses/{courseId}/pages/{urlOrId}/date_details",
            put(put_page_dates),
        )
}
