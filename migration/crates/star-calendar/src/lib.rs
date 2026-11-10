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
//! Phase 2 — Canvas Planner Notes (`PlannerNotesController`):
//! - GET    /planner_notes[?start_date=&end_date=&context_codes[]=]
//! - POST   /planner_notes                            — create for current user
//! - GET    /planner_notes/{id}                       — current user's note
//! - PUT    /planner_notes/{id}                       — update own note
//! - DELETE /planner_notes/{id}                       — soft-delete, returns note
//!
//! Canvas parity notes:
//! - Events are soft-deleted: workflow_state = 'deleted' (not hard delete).
//! - start_at is stored as TIMESTAMPTZ (ISO 8601 / RFC 3339).
//! - Deleted events are excluded from all list/query endpoints.
//! - Planner notes are scoped to the current user (Canvas never exposes other
//!   users' notes, admins included) and are soft-deleted via workflow_state.
//! - Planner note list filters: todo_date >= start_date, <= end_date
//!   (date-only end_date is inclusive through end of day), context_codes[]
//!   with `course_<id>` / `user_<id>` asset strings.

use axum::{
    extract::{Path, Query, RawQuery, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
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
// Planner notes (Canvas PlannerNotesController parity)
// ---------------------------------------------------------------------------

/// Planner note response shape (Canvas `planner_note_json`).
///
/// Canvas fields: id, title, details, user_id, course_id, todo_date,
/// workflow_state, created_at, updated_at.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PlannerNote {
    pub id: i64,
    pub title: String,
    pub details: Option<String>,
    pub user_id: i64,
    pub course_id: Option<i64>,
    pub todo_date: DateTime<Utc>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating a planner note (POST /planner_notes).
#[derive(Debug, Deserialize)]
pub struct CreatePlannerNote {
    pub title: Option<String>,
    pub details: Option<String>,
    #[serde(default)]
    pub course_id: Option<serde_json::Value>,
    pub todo_date: Option<String>,
}

/// Request body for updating a planner note (PUT /planner_notes/{id}).
///
/// Double options distinguish "field absent" (None) from "explicit null"
/// (Some(None)), so Canvas clients can clear `details` and detach a note from
/// its course with `"course_id": null` or `"course_id": ""`.
#[allow(clippy::option_option)]
#[derive(Debug, Deserialize)]
pub struct UpdatePlannerNote {
    pub title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub details: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_some")]
    pub course_id: Option<Option<serde_json::Value>>,
    pub todo_date: Option<String>,
}

/// Query params for GET /planner_notes (Canvas start_date/end_date).
#[derive(Debug, Deserialize)]
pub struct PlannerNotesQuery {
    pub start_date: Option<String>,
    pub end_date: Option<String>,
}

/// Parsed Canvas `context_codes[]` filter.
struct PlannerContextFilter {
    course_ids: Vec<i64>,
    include_courseless: bool,
}

/// Deserialize a present value into `Some(value)`, including JSON null.
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
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

/// Canvas treats an end date before the start date as the start date itself.
fn normalize_calendar_range(
    start_date: chrono::NaiveDate,
    end_date: chrono::NaiveDate,
) -> (chrono::NaiveDate, chrono::NaiveDate) {
    (start_date, end_date.max(start_date))
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

    let (start_date, end_date) = normalize_calendar_range(start_date, end_date);

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
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT course_id FROM calendar_events WHERE id = $1")
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
// Planner note helpers
// ---------------------------------------------------------------------------

/// Parse Canvas `todo_date` (yyyy-mm-dd or ISO 8601) as a UTC timestamp.
fn parse_todo_date(raw: &str) -> Result<DateTime<Utc>, ApiError> {
    let value = raw.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return Ok(naive.and_utc());
    }
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let naive = date.and_hms_opt(0, 0, 0).expect("midnight is a valid time");
        return Ok(naive.and_utc());
    }
    Err(ApiError::BadRequest("invalid todo_date".into()))
}

/// Parse a planner list date bound, mirroring Canvas `formatted_planner_date`:
/// a date-only value expands to beginning/end of that day, an ISO 8601
/// datetime is used as given.
fn parse_date_bound(raw: &str, end_of_day: bool) -> Result<DateTime<Utc>, ApiError> {
    let value = raw.trim();
    if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        let naive = if end_of_day {
            date.and_hms_nano_opt(23, 59, 59, 999_999_999)
                .expect("end of day is a valid time")
        } else {
            date.and_hms_opt(0, 0, 0).expect("midnight is a valid time")
        };
        return Ok(naive.and_utc());
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Ok(dt.with_timezone(&Utc));
    }
    let field = if end_of_day { "end_date" } else { "start_date" };
    Err(ApiError::BadRequest(format!(
        "invalid date or datetime for {field}"
    )))
}

/// Interpret a JSON `course_id` value: number/string -> id, null/"" -> none.
fn parse_course_id(value: Option<&serde_json::Value>) -> Result<Option<i64>, ApiError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => n
            .as_i64()
            .map(Some)
            .ok_or_else(|| ApiError::BadRequest("invalid course_id".into())),
        Some(serde_json::Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(serde_json::Value::String(s)) => s
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| ApiError::BadRequest("invalid course_id".into())),
        Some(_) => Err(ApiError::BadRequest("invalid course_id".into())),
    }
}

/// Parse Canvas `context_codes[]` (`course_42`, `user_7`) from the raw query.
fn parse_context_codes(raw_query: Option<&str>, user_id: i64) -> Option<PlannerContextFilter> {
    let raw = raw_query?;
    let mut seen = false;
    let mut course_ids = Vec::new();
    let mut include_courseless = false;

    for pair in raw.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        // Accept both `context_codes[]` and its percent-encoded spelling.
        let key = key
            .to_ascii_lowercase()
            .replace("%5b", "[")
            .replace("%5d", "]");
        if key != "context_codes" && key != "context_codes[]" {
            continue;
        }
        seen = true;
        let value = value.trim();
        if let Some(rest) = value.strip_prefix("course_") {
            if let Ok(course_id) = rest.parse::<i64>() {
                course_ids.push(course_id);
            }
        } else if value == format!("user_{user_id}") {
            include_courseless = true;
        }
    }

    seen.then_some(PlannerContextFilter {
        course_ids,
        include_courseless,
    })
}

/// Canvas checks `:read` on the course before associating a planner note.
async fn validate_note_course(
    db: &PgPool,
    user_id: i64,
    auth: &AuthUser,
    course_id: Option<i64>,
) -> Result<(), ApiError> {
    let Some(course_id) = course_id else {
        return Ok(());
    };
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(db, user_id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

/// Fetch a planner note owned by the current user (Canvas scopes by user).
async fn fetch_owned_planner_note(
    db: &PgPool,
    id: i64,
    user_id: i64,
) -> Result<PlannerNote, ApiError> {
    let note: Option<PlannerNote> = sqlx::query_as(
        r#"
        SELECT id, title, details, user_id, course_id, todo_date,
               workflow_state, created_at, updated_at
        FROM planner_notes
        WHERE id = $1 AND user_id = $2
        "#,
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;

    note.ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Planner note handlers
// ---------------------------------------------------------------------------

/// GET /planner_notes — list the current user's active planner notes.
///
/// Canvas filters: start_date/end_date on todo_date, context_codes[] for
/// course association (plus `user_<id>` for notes not tied to a course).
async fn list_planner_notes(
    State(state): State<AppState>,
    Query(params): Query<PlannerNotesQuery>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Json<Vec<PlannerNote>>, ApiError> {
    let start_date = params
        .start_date
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| parse_date_bound(value, false))
        .transpose()?;
    let end_date = params
        .end_date
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| parse_date_bound(value, true))
        .transpose()?;

    let context_filter = parse_context_codes(raw_query.as_deref(), auth.id);

    let rows: Vec<PlannerNote> = match context_filter {
        Some(filter) => {
            sqlx::query_as(
                r#"
                SELECT n.id, n.title, n.details, n.user_id, n.course_id, n.todo_date,
                       n.workflow_state, n.created_at, n.updated_at
                FROM planner_notes n
                LEFT JOIN courses c ON c.id = n.course_id
                WHERE n.user_id = $1
                  AND n.workflow_state <> 'deleted'
                  AND (n.course_id IS NULL OR c.workflow_state <> 'deleted')
                  AND ($2::timestamptz IS NULL OR n.todo_date >= $2)
                  AND ($3::timestamptz IS NULL OR n.todo_date <= $3)
                  AND (n.course_id = ANY($4) OR (n.course_id IS NULL AND $5))
                ORDER BY n.todo_date ASC, n.id ASC
                "#,
            )
            .bind(auth.id)
            .bind(start_date)
            .bind(end_date)
            .bind(filter.course_ids.as_slice())
            .bind(filter.include_courseless)
            .fetch_all(&state.db)
            .await?
        }
        None => {
            sqlx::query_as(
                r#"
                SELECT n.id, n.title, n.details, n.user_id, n.course_id, n.todo_date,
                       n.workflow_state, n.created_at, n.updated_at
                FROM planner_notes n
                LEFT JOIN courses c ON c.id = n.course_id
                WHERE n.user_id = $1
                  AND n.workflow_state <> 'deleted'
                  AND (n.course_id IS NULL OR c.workflow_state <> 'deleted')
                  AND ($2::timestamptz IS NULL OR n.todo_date >= $2)
                  AND ($3::timestamptz IS NULL OR n.todo_date <= $3)
                ORDER BY n.todo_date ASC, n.id ASC
                "#,
            )
            .bind(auth.id)
            .bind(start_date)
            .bind(end_date)
            .fetch_all(&state.db)
            .await?
        }
    };

    Ok(Json(rows))
}

/// POST /planner_notes — create a planner note for the current user (201).
async fn create_planner_note(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreatePlannerNote>,
) -> Result<(StatusCode, Json<PlannerNote>), ApiError> {
    let title = payload
        .title
        .ok_or_else(|| ApiError::BadRequest("title can't be blank".into()))?;
    if title.trim().is_empty() {
        return Err(ApiError::BadRequest("title can't be blank".into()));
    }

    let todo_date = payload
        .todo_date
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("todo_date can't be blank".into()))
        .and_then(parse_todo_date)?;

    let course_id = parse_course_id(payload.course_id.as_ref())?;
    validate_note_course(&state.db, auth.id, &auth, course_id).await?;

    let note: PlannerNote = sqlx::query_as(
        r#"
        INSERT INTO planner_notes (user_id, course_id, title, details, todo_date, workflow_state)
        VALUES ($1, $2, $3, $4, $5, 'active')
        RETURNING id, title, details, user_id, course_id, todo_date,
                  workflow_state, created_at, updated_at
        "#,
    )
    .bind(auth.id)
    .bind(course_id)
    .bind(&title)
    .bind(&payload.details)
    .bind(todo_date)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(note)))
}

/// GET /planner_notes/{id} — fetch one of the current user's notes.
async fn get_planner_note(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<PlannerNote>, ApiError> {
    let note = fetch_owned_planner_note(&state.db, id, auth.id).await?;
    Ok(Json(note))
}

/// PUT /planner_notes/{id} — update one of the current user's notes.
async fn update_planner_note(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdatePlannerNote>,
) -> Result<Json<PlannerNote>, ApiError> {
    let existing = fetch_owned_planner_note(&state.db, id, auth.id).await?;

    let course_id = match payload.course_id {
        Some(value) => {
            let new_course_id = parse_course_id(value.as_ref())?;
            if new_course_id != existing.course_id {
                validate_note_course(&state.db, auth.id, &auth, new_course_id).await?;
            }
            new_course_id
        }
        None => existing.course_id,
    };

    let title = match payload.title {
        Some(title) if title.trim().is_empty() => {
            return Err(ApiError::BadRequest("title can't be blank".into()));
        }
        Some(title) => title,
        None => existing.title,
    };
    let details = match payload.details {
        Some(details) => details,
        None => existing.details,
    };
    let todo_date = match payload.todo_date {
        Some(raw) => parse_todo_date(&raw)?,
        None => existing.todo_date,
    };

    let note: PlannerNote = sqlx::query_as(
        r#"
        UPDATE planner_notes
        SET title = $1,
            details = $2,
            course_id = $3,
            todo_date = $4,
            updated_at = now()
        WHERE id = $5 AND user_id = $6
        RETURNING id, title, details, user_id, course_id, todo_date,
                  workflow_state, created_at, updated_at
        "#,
    )
    .bind(&title)
    .bind(&details)
    .bind(course_id)
    .bind(todo_date)
    .bind(id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(note))
}

/// DELETE /planner_notes/{id} — Canvas soft-delete, returns the note JSON (200).
async fn delete_planner_note(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<PlannerNote>, ApiError> {
    let note: Option<PlannerNote> = sqlx::query_as(
        r#"
        UPDATE planner_notes
        SET workflow_state = 'deleted', updated_at = now()
        WHERE id = $1 AND user_id = $2
        RETURNING id, title, details, user_id, course_id, todo_date,
                  workflow_state, created_at, updated_at
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;

    let note = note.ok_or(ApiError::NotFound)?;
    Ok(Json(note))
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
        // Planner notes (current user only)
        .route(
            "/planner_notes",
            get(list_planner_notes).post(create_planner_note),
        )
        .route(
            "/planner_notes/{id}",
            get(get_planner_note)
                .put(update_planner_note)
                .delete(delete_planner_note),
        )
}

#[cfg(test)]
mod date_range_tests {
    use super::normalize_calendar_range;
    use chrono::NaiveDate;

    #[test]
    fn reversed_end_date_is_clamped_to_start_date() {
        let start = NaiveDate::from_ymd_opt(2012, 1, 8).unwrap();
        let end = NaiveDate::from_ymd_opt(2012, 1, 7).unwrap();

        assert_eq!(normalize_calendar_range(start, end), (start, start));
    }

    #[test]
    fn valid_end_date_is_preserved() {
        let start = NaiveDate::from_ymd_opt(2012, 1, 7).unwrap();
        let end = NaiveDate::from_ymd_opt(2012, 1, 8).unwrap();

        assert_eq!(normalize_calendar_range(start, end), (start, end));
    }
}

#[cfg(test)]
mod planner_note_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn todo_date_accepts_date_only_and_iso8601() {
        let from_date = parse_todo_date("2017-05-09").unwrap();
        let from_iso = parse_todo_date("2017-05-09T00:00:00Z").unwrap();
        assert_eq!(from_date, from_iso);
        assert_eq!(from_date.to_rfc3339(), "2017-05-09T00:00:00+00:00");
        assert!(parse_todo_date("not a date").is_err());
    }

    #[test]
    fn date_bounds_expand_date_only_values() {
        let start = parse_date_bound("2017-05-09", false).unwrap();
        let end = parse_date_bound("2017-05-09", true).unwrap();
        assert_eq!(start.to_rfc3339(), "2017-05-09T00:00:00+00:00");
        assert_eq!(end.to_rfc3339(), "2017-05-09T23:59:59.999999999+00:00");
        assert!(end > start);

        // ISO 8601 datetime bounds are kept as given (Canvas parity).
        let end_dt = parse_date_bound("2017-05-09T10:12:00Z", true).unwrap();
        assert_eq!(end_dt.to_rfc3339(), "2017-05-09T10:12:00+00:00");
        assert!(parse_date_bound("nope", false).is_err());
    }

    #[test]
    fn update_body_distinguishes_absent_from_null_course_id() {
        let absent: UpdatePlannerNote = serde_json::from_value(json!({})).unwrap();
        assert!(absent.course_id.is_none());

        let cleared: UpdatePlannerNote =
            serde_json::from_value(json!({ "course_id": null })).unwrap();
        assert_eq!(cleared.course_id, Some(None));
        assert_eq!(
            parse_course_id(cleared.course_id.unwrap().as_ref()).unwrap(),
            None
        );

        let set: UpdatePlannerNote = serde_json::from_value(json!({ "course_id": 42 })).unwrap();
        assert_eq!(
            parse_course_id(set.course_id.unwrap().as_ref()).unwrap(),
            Some(42)
        );

        let empty: UpdatePlannerNote = serde_json::from_value(json!({ "course_id": "" })).unwrap();
        assert_eq!(
            parse_course_id(empty.course_id.unwrap().as_ref()).unwrap(),
            None
        );
    }

    #[test]
    fn context_codes_parse_course_and_user_contexts() {
        let filter = parse_context_codes(
            Some("start_date=2017-05-01&context_codes%5B%5D=course_42&context_codes[]=course_7&context_codes[]=user_9"),
            9,
        )
        .expect("context codes present");
        assert_eq!(filter.course_ids, vec![42, 7]);
        assert!(filter.include_courseless);

        assert!(parse_context_codes(Some("start_date=2017-05-01"), 9).is_none());
    }
}
