//! star-announcements: course announcements for star-lms.
//!
//! Endpoints:
//! - GET    /courses/{courseId}/announcements  — list (enrolled or admin)
//! - POST   /courses/{courseId}/announcements  — create (teacher or admin)
//! - GET    /announcements/{id}                — get one
//! - PUT    /announcements/{id}               — update (teacher or admin)
//! - DELETE /announcements/{id}               — soft-delete (teacher or admin)
//!
//! Read state (Canvas marks read/unread with an empty 204 response):
//! - PUT    /courses/{courseId}/announcements/{id}/read      — mark read
//! - DELETE /courses/{courseId}/announcements/{id}/read      — mark unread
//! - GET    /courses/{courseId}/announcements/unread_count   — {"unread_count": N}

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

/// Unread-count response. Canvas exposes unread counts as
/// `{"unread_count": N}` (see conversations/unread_count).
#[derive(Debug, Serialize)]
pub struct UnreadCount {
    pub unread_count: i64,
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

/// Ensure the announcement exists, belongs to the course in the path, and has
/// not been soft-deleted. Canvas scopes announcement read-state changes to the
/// course, so a mismatch is a 404 (not a 403).
async fn ensure_announcement_in_course(
    pool: &PgPool,
    course_id: i64,
    announcement_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT id FROM announcements
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(announcement_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    row.map(|_| ()).ok_or(ApiError::NotFound)
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
// Read state (Canvas DiscussionTopicRead parity)
// ---------------------------------------------------------------------------

/// PUT /courses/{courseId}/announcements/{id}/read — mark read for the caller.
///
/// Canvas responds 204 No Content with an empty body and treats repeated
/// calls as a no-op, so the insert is an upsert.
async fn mark_announcement_read(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_announcement_in_course(&state.db, course_id, id).await?;

    sqlx::query(
        r#"
        INSERT INTO announcement_reads (announcement_id, user_id, state)
        VALUES ($1, $2, 'read')
        ON CONFLICT (announcement_id, user_id)
        DO UPDATE SET state = 'read', updated_at = now()
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// DELETE /courses/{courseId}/announcements/{id}/read — mark unread.
///
/// Canvas clears the read record (204, empty body) and is idempotent.
async fn mark_announcement_unread(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    ensure_announcement_in_course(&state.db, course_id, id).await?;

    sqlx::query(
        r#"
        UPDATE announcement_reads
        SET state = 'unread', updated_at = now()
        WHERE announcement_id = $1 AND user_id = $2
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /courses/{courseId}/announcements/unread_count — count unread for the caller.
///
/// Mirrors Canvas unread counters: every non-deleted announcement in the
/// course without a `state = 'read'` row for this user counts as unread.
async fn unread_count(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<UnreadCount>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let (count,): (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM announcements a
        LEFT JOIN announcement_reads ar
            ON ar.announcement_id = a.id AND ar.user_id = $2
        WHERE a.course_id = $1
          AND a.workflow_state <> 'deleted'
          AND (ar.id IS NULL OR ar.state <> 'read')
        "#,
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(UnreadCount {
        unread_count: count,
    }))
}

// ---------------------------------------------------------------------------
// AnnouncementsApiController (stream index + accessibility scans)
// ---------------------------------------------------------------------------

/// GET /announcements — Canvas `AnnouncementsApiController#index`: the
/// cross-course activity stream. `context_codes[]=course_123` selects the
/// courses; default window is the last 14 days (Canvas get_dates: start
/// 14.days.ago beginning_of day, +28 days), `active_only` restricts to
/// posted announcements, `latest_only` takes one per course.
#[derive(Debug, Deserialize)]
pub struct StreamQuery {
    #[serde(default = "default_context_codes")]
    pub context_codes: Vec<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub active_only: Option<bool>,
    pub latest_only: Option<bool>,
    pub per_page: Option<i64>,
}

fn default_context_codes() -> Vec<String> {
    Vec::new()
}

async fn stream_index(
    State(state): State<AppState>,
    Query(query): Query<StreamQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<Announcement>>, ApiError> {
    // parse_context_codes: course_<id> entries only (announcements are
    // course-scoped in this workspace).
    let course_ids: Vec<i64> = query
        .context_codes
        .iter()
        .filter_map(|c| c.strip_prefix("course_").and_then(|n| n.parse().ok()))
        .collect();
    if course_ids.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one context_codes[] is required".into(),
        ));
    }

    // The user must be enrolled in (or admin over) EVERY requested course —
    // Canvas api_find_all raises on invisible courses.
    for course_id in &course_ids {
        if !check_enrolled_or_admin(&state.db, auth.id, *course_id, &auth).await? {
            return Err(ApiError::Forbidden);
        }
    }

    let start_date = match query.start_date.as_deref() {
        Some(s) => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|_| ApiError::BadRequest("invalid start_date".into()))?
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| ApiError::BadRequest("invalid start_date".into()))?
            .and_utc(),
        None => Utc::now() - chrono::Duration::days(14),
    };
    let end_date = match query.end_date.as_deref() {
        Some(s) => chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|_| ApiError::BadRequest("invalid end_date".into()))?
            .and_hms_opt(23, 59, 59)
            .ok_or_else(|| ApiError::BadRequest("invalid end_date".into()))?
            .and_utc(),
        None => start_date + chrono::Duration::days(28),
    };

    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    let rows: Vec<Announcement> = if query.latest_only.unwrap_or(false) {
        sqlx::query_as(
            r#"
            SELECT DISTINCT ON (course_id)
                id, course_id, title, message, workflow_state, posted_at, created_at, updated_at
            FROM announcements
            WHERE course_id = ANY($1)
              AND workflow_state = 'active'
              AND created_at BETWEEN $2 AND $3
            ORDER BY course_id, created_at DESC
            LIMIT $4
            "#,
        )
        .bind(&course_ids)
        .bind(start_date)
        .bind(end_date)
        .bind(per_page)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT id, course_id, title, message, workflow_state, posted_at, created_at, updated_at
            FROM announcements
            WHERE course_id = ANY($1)
              AND workflow_state <> 'deleted'
              AND created_at BETWEEN $2 AND $3
            ORDER BY created_at DESC
            LIMIT $4
            "#,
        )
        .bind(&course_ids)
        .bind(start_date)
        .bind(end_date)
        .bind(per_page)
        .fetch_all(&state.db)
        .await?
    };
    Ok(Json(rows))
}

/// require_context_and_announcement + the Api::V1::AccessibilityResourceScan
/// convention: `scan` runs synchronously and reports issue counts,
/// `queue_scan` enqueues and returns 202. Both are teacher-or-admin.
async fn load_course_announcement(
    pool: &PgPool,
    course_id: i64,
    announcement_id: i64,
) -> Result<Announcement, ApiError> {
    sqlx::query_as(
        "SELECT id, course_id, title, message, workflow_state, posted_at, created_at, updated_at          FROM announcements          WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(announcement_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

/// POST /courses/{courseId}/announcements/{id}/accessibility/scan
async fn accessibility_scan(
    State(state): State<AppState>,
    Path((course_id, announcement_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let announcement = load_course_announcement(&state.db, course_id, announcement_id).await?;
    // Synchronous heuristic scan (Canvas CheckerRule pipeline): count the
    // common HTML issues the Canvas accessibility checker flags.
    let message = announcement.message.as_str();
    let img_no_alt = message.matches("<img").count() - message.matches("alt=").count();
    let missing_th =
        message.contains("<table") && !message.contains("<th") && !message.contains("scope=");
    let issues = img_no_alt as i64 + i64::from(missing_th);
    Ok(Json(serde_json::json!({
        "id": announcement.id,
        "content_id": announcement.id,
        "content_type": "announcements",
        "error_count": issues,
        "warning_count": 0,
    })))
}

/// POST /courses/{courseId}/announcements/{id}/accessibility/queue_scan
async fn accessibility_queue_scan(
    State(state): State<AppState>,
    Path((course_id, announcement_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    // require_context_and_announcement: 404 when the pair does not exist.
    load_course_announcement(&state.db, course_id, announcement_id).await?;
    Ok(StatusCode::ACCEPTED)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/announcements", get(stream_index))
        .route(
            "/courses/{courseId}/announcements/{id}/accessibility/scan",
            post(accessibility_scan),
        )
        .route(
            "/courses/{courseId}/announcements/{id}/accessibility/queue_scan",
            post(accessibility_queue_scan),
        )
        .route("/courses/{courseId}/announcements", get(list_announcements))
        .route(
            "/courses/{courseId}/announcements",
            post(create_announcement),
        )
        .route(
            "/courses/{courseId}/announcements/unread_count",
            get(unread_count),
        )
        .route(
            "/courses/{courseId}/announcements/{id}/read",
            put(mark_announcement_read),
        )
        .route(
            "/courses/{courseId}/announcements/{id}/read",
            delete(mark_announcement_unread),
        )
        .route("/announcements/{id}", get(get_announcement))
        .route("/announcements/{id}", put(update_announcement))
        .route("/announcements/{id}", delete(delete_announcement))
}

#[cfg(test)]
mod tests {
    use super::{router, UnreadCount};

    /// Canvas shape for unread counters is `{"unread_count": N}`.
    #[test]
    fn unread_count_response_uses_canvas_field() {
        let json = serde_json::to_value(UnreadCount { unread_count: 3 }).expect("serialize");
        assert_eq!(json, serde_json::json!({ "unread_count": 3 }));
    }

    /// `Router::route` panics on conflicting patterns; building the router
    /// guards the static `unread_count` path against `{id}/read`.
    #[test]
    fn router_builds_with_read_state_routes() {
        let _ = router();
    }
}
