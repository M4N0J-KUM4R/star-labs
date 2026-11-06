//! star-calendar — Canvas calendar events API for star-lms.
//!
//! Implements the core Canvas API v1 calendar event endpoints:
//! - POST   /courses/{courseId}/calendar_events        — create (teacher/admin)
//! - GET    /courses/{courseId}/calendar_events       — list course events (enrolled)
//! - GET    /calendar_events?start_date=&end_date=    — user calendar in range
//! - GET    /calendar_events/{id}                    — get one
//! - PUT    /calendar_events/{id}                     — update (teacher/admin)
//! - DELETE /calendar_events/{id}                     — soft-delete (teacher/admin)
//!
//! Canvas parity notes:
//! - Events are soft-deleted: workflow_state = 'deleted' (not hard delete).
//! - start_at is stored as TIMESTAMPTZ (ISO 8601 / RFC 3339).
//! - Deleted events are excluded from all list/query endpoints.

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

/// Calendar event response shape (Canvas API v1 compatible).
///
/// Canvas parity fields:
///   id, title, description, start_at, end_at, workflow_state,
///   location_name, created_at, updated_at.
/// course_context_code is added for convenience (Canvas uses context_code).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CalendarEvent {
    pub id: i64,
    pub course_id: Option<i64>,
    pub user_id: Option<i64>,
    pub title: String,
    pub description: Option<String>,
    pub start_at: DateTime<Utc>,
    pub end_at: Option<DateTime<Utc>>,
    pub workflow_state: String,
    pub location_name: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating a calendar event (POST).
#[derive(Debug, Deserialize)]
pub struct CreateCalendarEvent {
    pub title: String,
    pub description: Option<String>,
    #[serde(rename = "start_at")]
    pub start_at: DateTime<Utc>,
    pub end_at: Option<DateTime<Utc>>,
    pub location_name: Option<String>,
}

/// Request body for updating a calendar event (PUT).
#[derive(Debug, Deserialize)]
pub struct UpdateCalendarEvent {
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "start_at")]
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub location_name: Option<String>,
}

/// Query params for /calendar_events (user calendar range query).
#[derive(Debug, Deserialize)]
pub struct CalendarRangeParams {
    #[serde(rename = "start_date")]
    pub start_date: Option<String>,
    #[serde(rename = "end_date")]
    pub end_date: Option<String>,
}

/// Query params for course calendar list (order_by, etc.).
#[derive(Debug, Deserialize)]
pub struct CourseCalendarParams {
    #[serde(rename = "order_by")]
    pub order_by: Option<String>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Check if user is enrolled in a course (any role) or is admin.
async fn check_enrolled_or_admin(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1")
            .bind(user_id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

/// Check if user is a teacher in a course or admin.
async fn check_teacher_or_admin(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Check if user is enrolled in course OR is the owner (user_id matches) OR admin.
/// Used for the global calendar_events endpoint.
async fn check_global_calendar_access(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1")
            .bind(user_id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/calendar_events — create a calendar event (teacher/admin).
async fn create_calendar_event(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateCalendarEvent>,
) -> Result<(StatusCode, Json<CalendarEvent>), ApiError> {
    // Check teacher or admin permission
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title cannot be empty".into()));
    }

    let row: CalendarEvent = sqlx::query_as(
        r#"
        INSERT INTO calendar_events (course_id, user_id, title, description, start_at, end_at, workflow_state, location_name)
        VALUES ($1, NULL, $2, $3, $4, $5, 'active', $6)
        RETURNING id, course_id, user_id, title, description, start_at, end_at, workflow_state, location_name, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(payload.start_at)
    .bind(payload.end_at)
    .bind(&payload.location_name)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/calendar_events — list course calendar events (enrolled).
/// Excludes deleted events; ordered by start_at ASC.
async fn list_course_calendar_events(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<CalendarEvent>>, ApiError> {
    // Check enrolled or admin
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<CalendarEvent> = sqlx::query_as(
        r#"
        SELECT id, course_id, user_id, title, description, start_at, end_at,
               workflow_state, location_name, created_at, updated_at
        FROM calendar_events
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        ORDER BY start_at ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /calendar_events?start_date=&end_date= — user calendar across enrolled courses.
/// Returns events within the given date range for all courses the user is enrolled in.
async fn list_user_calendar_events(
    State(state): State<AppState>,
    Query(params): Query<CalendarRangeParams>,
    auth: AuthUser,
) -> Result<Json<Vec<CalendarEvent>>, ApiError> {
    // Parse dates; default to today if not provided
    let today = Utc::now().date_naive();
    let start_date = params
        .start_date
        .as_ref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or(today);
    let end_date = params
        .end_date
        .as_ref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or(today);

    // Get list of course IDs the user is enrolled in
    let course_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT course_id FROM enrollments WHERE user_id = $1
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    if course_ids.is_empty() {
        return Ok(Json(vec![]));
    }

    // Query events across enrolled courses within the date range
    let rows: Vec<CalendarEvent> = sqlx::query_as(
        r#"
        SELECT id, course_id, user_id, title, description, start_at, end_at,
               workflow_state, location_name, created_at, updated_at
        FROM calendar_events
        WHERE course_id = ANY($1)
          AND workflow_state <> 'deleted'
          AND start_at >= $2
          AND start_at < ($3 + interval '1 day')
        ORDER BY start_at ASC
        "#,
    )
    .bind(&course_ids)
    .bind(start_date)
    .bind(end_date)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /calendar_events/{id} — get a single calendar event.
async fn get_calendar_event(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<CalendarEvent>, ApiError> {
    let row: Option<CalendarEvent> = sqlx::query_as(
        r#"
        SELECT id, course_id, user_id, title, description, start_at, end_at,
               workflow_state, location_name, created_at, updated_at
        FROM calendar_events
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    let event = row.ok_or(ApiError::NotFound)?;

    // Check access: either course enrolled or is admin
    if let Some(course_id) = event.course_id {
        if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
            return Err(ApiError::Forbidden);
        }
    } else if let Some(user_id) = event.user_id {
        // Personal event: only the owner or admin can view
        if user_id != auth.id && !is_admin(&auth) {
            return Err(ApiError::Forbidden);
        }
    }

    Ok(Json(event))
}

/// PUT /calendar_events/{id} — update a calendar event (teacher/admin).
async fn update_calendar_event(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateCalendarEvent>,
) -> Result<Json<CalendarEvent>, ApiError> {
    // Fetch existing to get course_id for auth check
    let existing: Option<CalendarEvent> = sqlx::query_as(
        r#"
        SELECT id, course_id, user_id, title, description, start_at, end_at,
               workflow_state, location_name, created_at, updated_at
        FROM calendar_events
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    let existing = existing.ok_or(ApiError::NotFound)?;

    // Require course teacher/admin or admin
    if let Some(course_id) = existing.course_id {
        if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
            return Err(ApiError::Forbidden);
        }
    } else if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Validate title if provided
    if let Some(ref title) = payload.title {
        if title.trim().is_empty() {
            return Err(ApiError::Validation("title cannot be empty".into()));
        }
    }

    let title = payload.title.unwrap_or(existing.title);
    let description = payload.description.or(existing.description);
    let start_at = payload.start_at.unwrap_or(existing.start_at);
    let end_at = payload.end_at.or(existing.end_at);
    let location_name = payload.location_name.or(existing.location_name);

    let updated: CalendarEvent = sqlx::query_as(
        r#"
        UPDATE calendar_events
        SET title = $1,
            description = $2,
            start_at = $3,
            end_at = $4,
            location_name = $5,
            updated_at = now()
        WHERE id = $6 AND workflow_state <> 'deleted'
        RETURNING id, course_id, user_id, title, description, start_at, end_at,
                  workflow_state, location_name, created_at, updated_at
        "#,
    )
    .bind(&title)
    .bind(&description)
    .bind(start_at)
    .bind(end_at)
    .bind(&location_name)
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /calendar_events/{id} — soft-delete a calendar event (teacher/admin).
async fn delete_calendar_event(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Fetch course_id for auth check
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT course_id FROM calendar_events WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;

    let (course_id,) = row.ok_or(ApiError::NotFound)?;

    // Check permission: course teacher/admin or admin
    if let Some(cid) = course_id {
        if !check_teacher_or_admin(&state.db, auth.id, cid, &auth).await? {
            return Err(ApiError::Forbidden);
        }
    } else if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Soft delete: set workflow_state = 'deleted'
    sqlx::query(
        "UPDATE calendar_events SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
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
        // Course-scoped
        .route(
            "/courses/{courseId}/calendar_events",
            post(create_calendar_event),
        )
        .route(
            "/courses/{courseId}/calendar_events",
            get(list_course_calendar_events),
        )
        // Global (user calendar)
        .route("/calendar_events", get(list_user_calendar_events))
        // Singleton
        .route("/calendar_events/{id}", get(get_calendar_event))
        .route("/calendar_events/{id}", put(update_calendar_event))
        .route("/calendar_events/{id}", delete(delete_calendar_event))
}
