//! star-announcements: course announcements for star-lms.
//!
//! Endpoints:
//! - GET    /courses/{courseId}/announcements  — list (enrolled or admin)
//! - POST   /courses/{courseId}/announcements  — create (teacher or admin)
//! - GET    /announcements/{id}                — get one
//! - PUT    /announcements/{id}               — update (teacher or admin)
//! - DELETE /announcements/{id}               — soft-delete (teacher or admin)

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_common::{ApiError, AppState};

use star_auth::AuthUser;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Announcement response shape (Canvas API v1 compatible).
///
/// Canvas parity: Announcement subclasses DiscussionTopic, so responses carry
/// `workflow_state` ("active"; soft-delete sets "deleted") and `posted_at`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Announcement {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    pub message: String,
    pub workflow_state: String,
    pub posted_at: Option<DateTime<Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Request body for creating an announcement.
#[derive(Debug, Deserialize)]
pub struct CreateAnnouncement {
    pub title: String,
    pub message: String,
}

/// Request body for updating an announcement.
#[derive(Debug, Deserialize)]
pub struct UpdateAnnouncement {
    pub title: Option<String>,
    pub message: Option<String>,
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

/// Check if user is enrolled in a course (as any role) or is admin.
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

/// Check if user is a teacher in a course or admin.
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

/// GET /courses/{courseId}/announcements
async fn list_announcements(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(pag): Query<Pagination>,
    auth: AuthUser,
) -> Result<Json<Vec<Announcement>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<Announcement> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, posted_at, created_at, updated_at
        FROM announcements
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

/// POST /courses/{courseId}/announcements
async fn create_announcement(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateAnnouncement>,
) -> Result<(StatusCode, Json<Announcement>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title cannot be empty".into()));
    }
    if payload.message.trim().is_empty() {
        return Err(ApiError::Validation("message cannot be empty".into()));
    }

    let row: Announcement = sqlx::query_as(
        r#"
        INSERT INTO announcements (course_id, title, message, workflow_state, posted_at)
        VALUES ($1, $2, $3, 'active', now())
        RETURNING id, course_id, title, message, workflow_state, posted_at, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .bind(&payload.message)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /announcements/{id}
async fn get_announcement(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Announcement>, ApiError> {
    let row: Option<Announcement> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, posted_at, created_at, updated_at
        FROM announcements
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    let announcement = row.ok_or(ApiError::NotFound)?;

    if !check_enrolled_or_admin(&state.db, auth.id, announcement.course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(announcement))
}

/// PUT /announcements/{id}
async fn update_announcement(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateAnnouncement>,
) -> Result<Json<Announcement>, ApiError> {
    // Fetch existing to get course_id for auth check.
    let existing: Option<Announcement> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, message, workflow_state, posted_at, created_at, updated_at
        FROM announcements
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    let existing = existing.ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, existing.course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let title = payload.title.unwrap_or(existing.title);
    let message = payload.message.unwrap_or(existing.message);

    if title.trim().is_empty() {
        return Err(ApiError::Validation("title cannot be empty".into()));
    }
    if message.trim().is_empty() {
        return Err(ApiError::Validation("message cannot be empty".into()));
    }

    let updated: Announcement = sqlx::query_as(
        r#"
        UPDATE announcements
        SET title = $1, message = $2, updated_at = now()
        WHERE id = $3 AND workflow_state <> 'deleted'
        RETURNING id, course_id, title, message, workflow_state, posted_at, created_at, updated_at
        "#,
    )
    .bind(&title)
    .bind(&message)
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /announcements/{id}
async fn delete_announcement(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Fetch course_id for auth check.
    let row: Option<(i64,)> = sqlx::query_as("SELECT course_id FROM announcements WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await?;

    let course_id = row.ok_or(ApiError::NotFound)?.0;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas parity: announcements are soft-deleted (workflow_state='deleted',
    // mirroring DiscussionTopic destroy semantics), not hard-deleted.
    sqlx::query(
        "UPDATE announcements SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/announcements", get(list_announcements))
        .route(
            "/courses/{courseId}/announcements",
            post(create_announcement),
        )
        .route("/announcements/{id}", get(get_announcement))
        .route("/announcements/{id}", put(update_announcement))
        .route("/announcements/{id}", delete(delete_announcement))
}
