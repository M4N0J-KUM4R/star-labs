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
//!
//! Phase 3 — Canvas Scheduler (`AppointmentGroupsController` / `reserve`):
//! - GET    /appointment_groups                      — reservable/manageable list
//! - POST   /appointment_groups                      — create (teacher/admin)
//! - GET    /appointment_groups/{id}                 — show (appointments included)
//! - PUT    /appointment_groups/{id}                 — update (teacher/admin)
//! - DELETE /appointment_groups/{id}                 — soft-delete + cancel reason
//! - GET    /appointment_groups/{id}/users           — possible/registered users
//! - POST   /calendar_events/{id}/reservations       — reserve a time slot
//!
//! Canvas parity notes:
//! - Time slots and reservations are calendar_events rows: slots carry
//!   `appointment_group_id`, reservations carry `parent_event_id` and the
//!   participant as `user_id` (reservation workflow_state = 'locked').
//! - Appointment group workflow: 'pending' -> 'active' on publish, 'deleted'
//!   on destroy; deletion soft-deletes slots and reservations.
//! - Reservation failures return Canvas' 400 array shape
//!   `[{attribute, type, message, reservations}]`.

use axum::{
    extract::{Path, Query, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
    } else {
        // Appointment slot/reservation: fall back to the appointment group ACL.
        let appointment: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT appointment_group_id FROM calendar_events WHERE id = $1")
                .bind(id)
                .fetch_optional(&state.db)
                .await?;
        match appointment.and_then(|(group_id,)| group_id) {
            Some(group_id) if can_read_appointment_group(&state.db, &auth, group_id).await? => {}
            _ => return Err(ApiError::Forbidden),
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
// Appointment groups (Canvas Scheduler parity)
// ---------------------------------------------------------------------------
//
// Canvas stores appointment time slots and reservations as CalendarEvents: a
// slot's context is the appointment group, a reservation is a child event
// whose context is the participant. This module mirrors that model with
// `calendar_events.appointment_group_id` / `parent_event_id`.

const APPOINTMENT_GROUP_COLUMNS: &str = "id, title, description, location_name, location_address, start_at, end_at, \
     max_appointments_per_participant, min_appointments_per_participant, participants_per_appointment, \
     participant_visibility, participant_type, workflow_state, allow_observer_signup, cancel_reason, \
     created_at, updated_at";

const APPOINTMENT_SLOT_COLUMNS: &str = "id, appointment_group_id, parent_event_id, user_id, title, description, \
     start_at, end_at, workflow_state, location_name, location_address, comments, created_at, updated_at";

const APPOINTMENT_GROUP_SELECT: &str = concat!(
    "SELECT id, title, description, location_name, location_address, start_at, end_at, ",
    "max_appointments_per_participant, min_appointments_per_participant, participants_per_appointment, ",
    "participant_visibility, participant_type, workflow_state, allow_observer_signup, cancel_reason, ",
    "created_at, updated_at FROM appointment_groups WHERE id = $1"
);

const APPOINTMENT_SLOT_SELECT: &str = concat!(
    "SELECT id, appointment_group_id, parent_event_id, user_id, title, description, start_at, end_at, ",
    "workflow_state, location_name, location_address, comments, created_at, updated_at ",
    "FROM calendar_events WHERE id = $1"
);

/// Row of `appointment_groups` (Canvas AppointmentGroup).
#[derive(Debug, Clone, sqlx::FromRow)]
struct AppointmentGroupRow {
    id: i64,
    title: String,
    description: Option<String>,
    location_name: Option<String>,
    location_address: Option<String>,
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    max_appointments_per_participant: Option<i32>,
    min_appointments_per_participant: Option<i32>,
    participants_per_appointment: Option<i32>,
    participant_visibility: String,
    participant_type: String,
    workflow_state: String,
    allow_observer_signup: bool,
    cancel_reason: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Calendar event row used for both time slots and reservations.
#[derive(Debug, Clone, sqlx::FromRow)]
struct AppointmentSlotRow {
    id: i64,
    appointment_group_id: Option<i64>,
    parent_event_id: Option<i64>,
    user_id: Option<i64>,
    title: String,
    description: Option<String>,
    start_at: DateTime<Utc>,
    end_at: Option<DateTime<Utc>>,
    workflow_state: String,
    location_name: Option<String>,
    location_address: Option<String>,
    comments: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Reservation (child calendar event) joined with the participant name.
#[derive(Debug, Clone, sqlx::FromRow)]
struct AppointmentReservationRow {
    id: i64,
    parent_event_id: Option<i64>,
    user_id: Option<i64>,
    user_name: Option<String>,
    title: String,
    description: Option<String>,
    start_at: DateTime<Utc>,
    end_at: Option<DateTime<Utc>>,
    workflow_state: String,
    comments: Option<String>,
    location_name: Option<String>,
    location_address: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Canvas `appointment_group_json` response shape.
#[derive(Debug, Clone, Serialize)]
struct AppointmentGroupJson {
    id: i64,
    created_at: DateTime<Utc>,
    description: Option<String>,
    end_at: Option<DateTime<Utc>>,
    max_appointments_per_participant: Option<i32>,
    min_appointments_per_participant: Option<i32>,
    participants_per_appointment: Option<i32>,
    start_at: Option<DateTime<Utc>>,
    title: String,
    updated_at: DateTime<Utc>,
    workflow_state: String,
    participant_visibility: String,
    sub_context_codes: Vec<String>,
    location_address: Option<String>,
    location_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    participant_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reserved_times: Option<Vec<AppointmentReservedTime>>,
    context_codes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    all_context_codes: Option<Vec<String>>,
    requiring_action: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_appointments: Option<Vec<AppointmentEventJson>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    appointments: Option<Vec<AppointmentEventJson>>,
    appointments_count: i64,
    participant_type: String,
    url: String,
    html_url: String,
    allow_observer_signup: bool,
}

/// Canvas `Appointment` shape for `reserved_times`.
#[derive(Debug, Clone, Serialize)]
struct AppointmentReservedTime {
    id: i64,
    start_at: DateTime<Utc>,
    end_at: Option<DateTime<Utc>>,
}

/// Calendar event JSON for appointment slots (`calendar_event_json` subset).
#[derive(Debug, Clone, Serialize)]
struct AppointmentEventJson {
    id: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    start_at: DateTime<Utc>,
    end_at: Option<DateTime<Utc>>,
    all_day: bool,
    all_day_date: Option<NaiveDate>,
    title: String,
    workflow_state: String,
    comments: Option<String>,
    location_name: Option<String>,
    location_address: Option<String>,
    #[serde(rename = "type")]
    kind: &'static str,
    description: Option<String>,
    child_events_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_events: Option<Vec<AppointmentChildEventJson>>,
    context_code: String,
    effective_context_code: String,
    all_context_codes: String,
    parent_event_id: Option<i64>,
    hidden: bool,
    url: String,
    html_url: String,
    appointment_group_id: i64,
    appointment_group_url: String,
    can_manage_appointment_group: bool,
    participant_type: String,
    reserved: bool,
    reserve_comments: String,
    reserve_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    available_slots: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    participants_per_appointment: Option<i32>,
    context_name: Option<String>,
    context_color: Option<String>,
    blackout_date: bool,
    duplicates: Vec<Value>,
    important_dates: bool,
    series_uuid: Option<String>,
    rrule: Option<String>,
}

/// Reservation JSON (child calendar event whose context is the participant).
#[derive(Debug, Clone, Serialize)]
struct AppointmentChildEventJson {
    id: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    start_at: DateTime<Utc>,
    end_at: Option<DateTime<Utc>>,
    all_day: bool,
    all_day_date: Option<NaiveDate>,
    title: String,
    workflow_state: String,
    comments: Option<String>,
    location_name: Option<String>,
    location_address: Option<String>,
    #[serde(rename = "type")]
    kind: &'static str,
    description: Option<String>,
    context_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    effective_context_code: Option<String>,
    all_context_codes: String,
    parent_event_id: Option<i64>,
    user_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user: Option<AppointmentEventUser>,
    #[serde(skip_serializing_if = "Option::is_none")]
    child_events: Option<Vec<Value>>,
    appointment_group_id: i64,
    appointment_group_url: String,
    can_manage_appointment_group: bool,
    participant_type: String,
    child_events_count: i64,
    hidden: bool,
    url: String,
    html_url: String,
    context_name: Option<String>,
    context_color: Option<String>,
    blackout_date: bool,
    duplicates: Vec<Value>,
    important_dates: bool,
    series_uuid: Option<String>,
    rrule: Option<String>,
}

/// Nested participant user for a reservation (`user_json` subset).
#[derive(Debug, Clone, Serialize)]
struct AppointmentEventUser {
    id: i64,
    name: String,
    short_name: String,
    sortable_name: String,
}

/// Participant row for `GET /appointment_groups/{id}/users`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AppointmentParticipant {
    pub id: i64,
    pub name: String,
    pub sortable_name: String,
    pub short_name: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
}

/// Body for POST/PUT /appointment_groups. Canvas nests everything under
/// `appointment_group`; flat bodies are accepted too.
#[derive(Debug, Deserialize, Default)]
pub struct AppointmentGroupWrite {
    pub title: Option<String>,
    pub description: Option<Value>,
    pub location_name: Option<Value>,
    pub location_address: Option<Value>,
    pub publish: Option<Value>,
    pub participants_per_appointment: Option<Value>,
    pub min_appointments_per_participant: Option<Value>,
    pub max_appointments_per_participant: Option<Value>,
    pub participant_visibility: Option<String>,
    pub allow_observer_signup: Option<Value>,
    pub cancel_reason: Option<Value>,
    pub context_codes: Option<Vec<String>>,
    pub sub_context_codes: Option<Vec<String>>,
    pub new_appointments: Option<Value>,
}

/// Envelope accepting `{"appointment_group": {...}}` or a flat object.
#[derive(Debug, Deserialize)]
pub struct AppointmentGroupEnvelope {
    pub appointment_group: Option<AppointmentGroupWrite>,
    #[serde(flatten)]
    pub flat: AppointmentGroupWrite,
}

impl AppointmentGroupEnvelope {
    fn into_params(self) -> AppointmentGroupWrite {
        self.appointment_group.unwrap_or(self.flat)
    }
}

#[derive(Debug, Deserialize)]
pub struct AppointmentGroupListQuery {
    pub scope: Option<String>,
    pub include_past_appointments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AppointmentGroupShowQuery {
    pub include_past_appointments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AppointmentGroupUsersQuery {
    pub registration_status: Option<String>,
    pub context_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AppointmentGroupDeleteQuery {
    pub cancel_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ReservationParams {
    pub participant_id: Option<Value>,
    pub comments: Option<String>,
    pub cancel_existing: Option<Value>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ReservationQuery {
    pub participant_id: Option<String>,
    pub comments: Option<String>,
    pub cancel_existing: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct AppointmentGroupIncludeOptions {
    appointments: bool,
    child_events: bool,
    participant_count: bool,
    reserved_times: bool,
    all_context_codes: bool,
    include_past_appointments: bool,
}

impl AppointmentGroupIncludeOptions {
    fn none() -> Self {
        Self {
            appointments: false,
            child_events: false,
            participant_count: false,
            reserved_times: false,
            all_context_codes: false,
            include_past_appointments: false,
        }
    }

    fn from_query(raw_query: Option<&str>, include_past_appointments: bool) -> Self {
        let include = query_values(raw_query, "include");
        Self {
            appointments: include.iter().any(|value| value == "appointments"),
            child_events: include.iter().any(|value| value == "child_events"),
            participant_count: include.iter().any(|value| value == "participant_count"),
            reserved_times: include.iter().any(|value| value == "reserved_times"),
            all_context_codes: include.iter().any(|value| value == "all_context_codes"),
            include_past_appointments,
        }
    }
}

/// Contexts, sub-contexts, slots and reservations of one appointment group.
struct AppointmentGroupSnapshot {
    context_codes: Vec<String>,
    course_ids: Vec<i64>,
    sub_context_codes: Vec<String>,
    slots: Vec<AppointmentSlotRow>,
    reservations: Vec<AppointmentReservationRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SubContextRef {
    CourseSection(i64),
    GroupCategory(i64),
}

/// Validated sub-context row (section or group category).
struct ValidSubContext {
    sub_context_type: String,
    sub_context_id: i64,
    sub_context_code: String,
    participant_type: String,
}

// ---------------------------------------------------------------------------
// Appointment parsing helpers
// ---------------------------------------------------------------------------

/// Decode percent escapes and `+` from a raw query value.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                if let (Some(high), Some(low)) = (high, low) {
                    out.push((high * 16 + low) as u8);
                    index += 3;
                } else {
                    out.push(b'%');
                    index += 1;
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Collect repeated query values for `key` / `key[]` from the raw query.
fn query_values(raw_query: Option<&str>, key: &str) -> Vec<String> {
    let Some(raw) = raw_query else {
        return Vec::new();
    };
    let bracket_key = format!("{key}[]");
    let mut values = Vec::new();
    for pair in raw.split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let decoded_key = percent_decode(raw_key);
        if decoded_key == key || decoded_key == bracket_key {
            values.push(percent_decode(raw_value));
        }
    }
    values
}

fn truthy_str(value: Option<&str>) -> bool {
    value
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "on" | "yes" | "t"
            )
        })
        .unwrap_or(false)
}

fn truthy_value(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_i64().map(|value| value != 0).unwrap_or(false),
        Value::String(value) => truthy_str(Some(value)),
        _ => false,
    }
}

fn parse_nullable_text(value: &Value) -> Result<Option<String>, ApiError> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) => Ok(Some(text.clone())),
        _ => Err(ApiError::BadRequest("invalid text value".into())),
    }
}

fn parse_nullable_i32(value: &Value) -> Result<Option<i32>, ApiError> {
    match value {
        Value::Null => Ok(None),
        Value::Number(number) => number
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(Some)
            .ok_or_else(|| ApiError::BadRequest("invalid integer".into())),
        Value::String(text) if text.trim().is_empty() => Ok(None),
        Value::String(text) => text
            .trim()
            .parse::<i32>()
            .map(Some)
            .map_err(|_| ApiError::BadRequest("invalid integer".into())),
        _ => Err(ApiError::BadRequest("invalid integer".into())),
    }
}

fn parse_json_id(value: &Value) -> Result<i64, ApiError> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| ApiError::BadRequest("invalid id".into())),
        Value::String(text) => text
            .trim()
            .parse::<i64>()
            .map_err(|_| ApiError::BadRequest("invalid id".into())),
        _ => Err(ApiError::BadRequest("invalid id".into())),
    }
}

fn parse_appointment_timestamp(raw: &str) -> Result<DateTime<Utc>, ApiError> {
    let value = raw.trim();
    if let Ok(datetime) = DateTime::parse_from_rfc3339(value) {
        return Ok(datetime.with_timezone(&Utc));
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return Ok(naive.and_utc());
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return Ok(naive.and_utc());
    }
    Err(ApiError::BadRequest("invalid appointment time".into()))
}

fn parse_appointment_json_timestamp(value: &Value) -> Result<DateTime<Utc>, ApiError> {
    match value {
        Value::String(text) => parse_appointment_timestamp(text),
        _ => Err(ApiError::BadRequest("invalid appointment time".into())),
    }
}

/// Parse Canvas `appointment_group[new_appointments]`.
///
/// Accepts `[[start, end], ...]`, `[{"start_at": ..., "end_at": ...}, ...]`
/// and Rails' hash form `{"0": [start, end], ...}`. A missing end defaults to
/// the start, and an end before the start is clamped (Canvas
/// `populate_missing_dates`).
type AppointmentDateRange = (DateTime<Utc>, DateTime<Utc>);

fn parse_new_appointments(value: Option<&Value>) -> Result<Vec<AppointmentDateRange>, ApiError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let items: Vec<&Value> = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by_key(|key| key.parse::<usize>().unwrap_or(usize::MAX));
            keys.into_iter().filter_map(|key| map.get(key)).collect()
        }
        _ => return Err(ApiError::BadRequest("invalid new_appointments".into())),
    };

    let mut slots = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Value::Array(pair) => {
                if pair.len() < 2 || pair[0].is_null() || pair[1].is_null() {
                    continue;
                }
                let start = parse_appointment_json_timestamp(&pair[0])?;
                let end = parse_appointment_json_timestamp(&pair[1])?;
                slots.push((start, end.max(start)));
            }
            Value::Object(object) => {
                let Some(start_value) = object.get("start_at").or_else(|| object.get("0")) else {
                    continue;
                };
                if start_value.is_null() {
                    continue;
                }
                let start = parse_appointment_json_timestamp(start_value)?;
                let end = match object.get("end_at").or_else(|| object.get("1")) {
                    Some(end_value) if !end_value.is_null() => {
                        parse_appointment_json_timestamp(end_value)?
                    }
                    _ => start,
                };
                slots.push((start, end.max(start)));
            }
            _ => return Err(ApiError::BadRequest("invalid new_appointments".into())),
        }
    }
    Ok(slots)
}

/// Canvas rejects overlapping time slots ("Appointments overlap").
fn validate_appointment_times(slots: &[(DateTime<Utc>, DateTime<Utc>)]) -> Result<(), ApiError> {
    let mut ordered = slots.to_vec();
    ordered.sort_by_key(|(start, _)| *start);
    for pair in ordered.windows(2) {
        if pair[1].0 < pair[0].1 {
            return Err(ApiError::BadRequest("Appointments overlap".into()));
        }
    }
    Ok(())
}

fn parse_course_context_code(code: &str) -> Option<i64> {
    code.trim()
        .strip_prefix("course_")
        .and_then(|rest| rest.parse::<i64>().ok())
}

fn parse_sub_context_code(code: &str) -> Option<SubContextRef> {
    let code = code.trim();
    if let Some(rest) = code.strip_prefix("course_section_") {
        if let Ok(id) = rest.parse::<i64>() {
            return Some(SubContextRef::CourseSection(id));
        }
    }
    if let Some(rest) = code.strip_prefix("group_category_") {
        if let Ok(id) = rest.parse::<i64>() {
            return Some(SubContextRef::GroupCategory(id));
        }
    }
    None
}

/// Canvas `all_appointments_filled?`: every slot at capacity.
fn all_appointments_filled(
    slot_reservation_counts: &[i64],
    participants_per_appointment: Option<i32>,
) -> bool {
    let Some(limit) = participants_per_appointment else {
        return false;
    };
    if slot_reservation_counts.is_empty() {
        return false;
    }
    slot_reservation_counts
        .iter()
        .all(|count| *count >= limit as i64)
}

/// Canvas `requiring_action?` for the current participant.
fn requiring_action(
    min_appointments_per_participant: Option<i32>,
    reservations_for_participant: usize,
    all_filled: bool,
) -> bool {
    let Some(minimum) = min_appointments_per_participant else {
        return false;
    };
    if all_filled {
        return false;
    }
    (reservations_for_participant as i32) < minimum
}

fn appointment_group_url(group_id: i64) -> String {
    format!("/api/v1/appointment_groups/{group_id}")
}

fn appointment_group_html_url(group_id: i64) -> String {
    format!("/appointment_groups/{group_id}")
}

fn appointment_event_html_url(context_codes: &[String], event_id: i64) -> String {
    match context_codes
        .iter()
        .find_map(|code| parse_course_context_code(code))
    {
        Some(course_id) => format!("/courses/{course_id}/calendar?event_id={event_id}"),
        None => format!("/calendar?event_id={event_id}"),
    }
}

// ---------------------------------------------------------------------------
// Appointment persistence helpers
// ---------------------------------------------------------------------------

async fn fetch_appointment_group<'e, E>(
    executor: E,
    id: i64,
) -> Result<Option<AppointmentGroupRow>, ApiError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    Ok(
        sqlx::query_as::<_, AppointmentGroupRow>(APPOINTMENT_GROUP_SELECT)
            .bind(id)
            .fetch_optional(executor)
            .await?,
    )
}

async fn fetch_appointment_slot<'e, E>(
    executor: E,
    id: i64,
) -> Result<Option<AppointmentSlotRow>, ApiError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    Ok(
        sqlx::query_as::<_, AppointmentSlotRow>(APPOINTMENT_SLOT_SELECT)
            .bind(id)
            .fetch_optional(executor)
            .await?,
    )
}

async fn load_appointment_snapshot(
    db: &PgPool,
    group_id: i64,
) -> Result<AppointmentGroupSnapshot, ApiError> {
    let contexts: Vec<(String, i64)> = sqlx::query_as(
        "SELECT context_code, context_id FROM appointment_group_contexts \
         WHERE appointment_group_id = $1 ORDER BY id ASC",
    )
    .bind(group_id)
    .fetch_all(db)
    .await?;
    let context_codes = contexts.iter().map(|(code, _)| code.clone()).collect();
    let course_ids = contexts.iter().map(|(_, id)| *id).collect();

    let sub_context_codes: Vec<String> = sqlx::query_scalar(
        "SELECT sub_context_code FROM appointment_group_sub_contexts \
         WHERE appointment_group_id = $1 ORDER BY id ASC",
    )
    .bind(group_id)
    .fetch_all(db)
    .await?;

    let slots: Vec<AppointmentSlotRow> = sqlx::query_as(
        r#"
        SELECT id, appointment_group_id, parent_event_id, user_id, title, description,
               start_at, end_at, workflow_state, location_name, location_address, comments,
               created_at, updated_at
        FROM calendar_events
        WHERE appointment_group_id = $1
          AND parent_event_id IS NULL
          AND workflow_state <> 'deleted'
        ORDER BY start_at ASC, id ASC
        "#,
    )
    .bind(group_id)
    .fetch_all(db)
    .await?;

    let reservations: Vec<AppointmentReservationRow> = sqlx::query_as(
        r#"
        SELECT c.id, c.parent_event_id, c.user_id, u.name AS user_name, c.title, c.description,
               c.start_at, c.end_at, c.workflow_state, c.comments, c.location_name,
               c.location_address, c.created_at, c.updated_at
        FROM calendar_events c
        LEFT JOIN users u ON u.id = c.user_id
        WHERE c.appointment_group_id = $1
          AND c.parent_event_id IS NOT NULL
          AND c.workflow_state <> 'deleted'
        ORDER BY c.start_at ASC, c.id ASC
        "#,
    )
    .bind(group_id)
    .fetch_all(db)
    .await?;

    Ok(AppointmentGroupSnapshot {
        context_codes,
        course_ids,
        sub_context_codes,
        slots,
        reservations,
    })
}

/// Canvas `manage` on an appointment group: admin or teacher in any context.
async fn can_manage_appointment_group(
    db: &PgPool,
    auth: &AuthUser,
    group_id: i64,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM appointment_group_contexts agc
        JOIN enrollments e ON e.course_id = agc.context_id AND e.user_id = $2
        WHERE agc.appointment_group_id = $1 AND e.role = 'teacher'
        LIMIT 1
        "#,
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Canvas `read` on an appointment group: manager, admin, or active participant.
async fn can_read_appointment_group(
    db: &PgPool,
    auth: &AuthUser,
    group_id: i64,
) -> Result<bool, ApiError> {
    let Some(group) = fetch_appointment_group(db, group_id).await? else {
        return Ok(false);
    };
    if group.workflow_state == "deleted" {
        return Ok(is_admin(auth));
    }
    if can_manage_appointment_group(db, auth, group_id).await? {
        return Ok(true);
    }
    if group.workflow_state != "active" {
        return Ok(false);
    }
    let snapshot = load_appointment_snapshot(db, group_id).await?;
    has_course_membership(db, auth.id, &snapshot.course_ids, None).await
}

/// Enrolled (optionally with a specific role) in any of the courses.
async fn has_course_membership(
    db: &PgPool,
    user_id: i64,
    course_ids: &[i64],
    role: Option<&str>,
) -> Result<bool, ApiError> {
    if course_ids.is_empty() {
        return Ok(false);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = ANY($2) \
           AND ($3::text IS NULL OR role = $3) LIMIT 1",
    )
    .bind(user_id)
    .bind(course_ids)
    .bind(role)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Resolve and authorize the course contexts of an appointment group.
/// Unknown codes are ignored like Canvas `Context.find_by_asset_string`.
async fn resolve_managed_courses(
    db: &PgPool,
    auth: &AuthUser,
    codes: &[String],
    empty_is_not_found: bool,
) -> Result<Vec<i64>, ApiError> {
    let mut course_ids: Vec<i64> = Vec::new();
    for code in codes {
        if let Some(course_id) = parse_course_context_code(code) {
            if !course_ids.contains(&course_id) {
                course_ids.push(course_id);
            }
        }
    }
    if course_ids.is_empty() {
        return Err(if empty_is_not_found {
            ApiError::NotFound
        } else {
            ApiError::BadRequest("Must have at least one context".into())
        });
    }
    let existing: Vec<i64> = sqlx::query_scalar("SELECT id FROM courses WHERE id = ANY($1)")
        .bind(&course_ids)
        .fetch_all(db)
        .await?;
    if existing.is_empty() {
        return Err(if empty_is_not_found {
            ApiError::NotFound
        } else {
            ApiError::BadRequest("Must have at least one context".into())
        });
    }
    for course_id in &existing {
        if !check_teacher_or_admin(db, auth.id, *course_id, auth).await? {
            return Err(ApiError::Forbidden);
        }
    }
    Ok(existing)
}

/// Validate sub-context codes (sections / group categories).
async fn resolve_sub_contexts(
    db: &PgPool,
    codes: &[String],
) -> Result<Vec<ValidSubContext>, ApiError> {
    let mut resolved: Vec<ValidSubContext> = Vec::new();
    for code in codes {
        let candidate = match parse_sub_context_code(code) {
            Some(SubContextRef::CourseSection(id)) => {
                let exists: Option<(i32,)> =
                    sqlx::query_as("SELECT 1 FROM course_sections WHERE id = $1")
                        .bind(id)
                        .fetch_optional(db)
                        .await?;
                exists.map(|_| ValidSubContext {
                    sub_context_type: "CourseSection".into(),
                    sub_context_id: id,
                    sub_context_code: format!("course_section_{id}"),
                    participant_type: "User".into(),
                })
            }
            Some(SubContextRef::GroupCategory(id)) => {
                let exists: Option<(i32,)> =
                    sqlx::query_as("SELECT 1 FROM group_categories WHERE id = $1")
                        .bind(id)
                        .fetch_optional(db)
                        .await?;
                exists.map(|_| ValidSubContext {
                    sub_context_type: "GroupCategory".into(),
                    sub_context_id: id,
                    sub_context_code: format!("group_category_{id}"),
                    participant_type: "Group".into(),
                })
            }
            None => None,
        };
        if let Some(candidate) = candidate {
            if !resolved
                .iter()
                .any(|existing| existing.sub_context_code == candidate.sub_context_code)
            {
                resolved.push(candidate);
            }
        }
    }
    Ok(resolved)
}

fn participant_type_for(sub_contexts: &[ValidSubContext]) -> &'static str {
    if sub_contexts
        .iter()
        .any(|sub_context| sub_context.participant_type == "Group")
    {
        "Group"
    } else {
        "User"
    }
}

async fn insert_group_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    group_id: i64,
    course_id: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO appointment_group_contexts \
         (appointment_group_id, context_type, context_id, context_code) \
         VALUES ($1, 'Course', $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(group_id)
    .bind(course_id)
    .bind(format!("course_{course_id}"))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_group_sub_context(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    group_id: i64,
    sub_context: &ValidSubContext,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO appointment_group_sub_contexts \
         (appointment_group_id, sub_context_type, sub_context_id, sub_context_code, participant_type) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
    )
    .bind(group_id)
    .bind(&sub_context.sub_context_type)
    .bind(sub_context.sub_context_id)
    .bind(&sub_context.sub_context_code)
    .bind(&sub_context.participant_type)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Insert a time slot calendar event (context = appointment group).
async fn insert_appointment_slot(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    group: &AppointmentGroupRow,
    start_at: DateTime<Utc>,
    end_at: DateTime<Utc>,
) -> Result<AppointmentSlotRow, ApiError> {
    let sql = format!(
        r#"
        INSERT INTO calendar_events
            (course_id, user_id, appointment_group_id, parent_event_id, title, description,
             start_at, end_at, workflow_state, location_name, location_address)
        VALUES (NULL, NULL, $1, NULL, $2, $3, $4, $5, 'active', $6, $7)
        RETURNING {APPOINTMENT_SLOT_COLUMNS}
        "#
    );
    Ok(sqlx::query_as::<_, AppointmentSlotRow>(&sql)
        .bind(group.id)
        .bind(&group.title)
        .bind(&group.description)
        .bind(start_at)
        .bind(end_at)
        .bind(&group.location_name)
        .bind(&group.location_address)
        .fetch_one(&mut **tx)
        .await?)
}

/// Canvas `update_cached_values`: group start/end are the slot min/max.
async fn refresh_group_cached_times(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    group_id: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        UPDATE appointment_groups
        SET start_at = (
                SELECT min(start_at) FROM calendar_events
                WHERE appointment_group_id = $1
                  AND parent_event_id IS NULL
                  AND workflow_state <> 'deleted'
            ),
            end_at = (
                SELECT max(COALESCE(end_at, start_at)) FROM calendar_events
                WHERE appointment_group_id = $1
                  AND parent_event_id IS NULL
                  AND workflow_state <> 'deleted'
            ),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(group_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Canvas cascades group event attributes to slots and reservations.
async fn propagate_group_attributes(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    group: &AppointmentGroupRow,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE calendar_events \
         SET title = $2, description = $3, location_name = $4, location_address = $5, updated_at = now() \
         WHERE appointment_group_id = $1",
    )
    .bind(group.id)
    .bind(&group.title)
    .bind(&group.description)
    .bind(&group.location_name)
    .bind(&group.location_address)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Appointment JSON builders
// ---------------------------------------------------------------------------

fn build_appointment_event_json(
    slot: &AppointmentSlotRow,
    group: &AppointmentGroupRow,
    context_codes: &[String],
    can_manage: bool,
    auth_user_id: i64,
    slot_reservations: &[&AppointmentReservationRow],
    include_child_events: bool,
) -> AppointmentEventJson {
    let own_reservations: Vec<&AppointmentReservationRow> = slot_reservations
        .iter()
        .copied()
        .filter(|reservation| reservation.user_id == Some(auth_user_id))
        .collect();
    let reserved = !own_reservations.is_empty();
    let reserve_comments = own_reservations
        .iter()
        .filter_map(|reservation| reservation.comments.clone())
        .collect::<Vec<_>>()
        .join(", ");

    let child_events = if include_child_events || reserved {
        Some(
            slot_reservations
                .iter()
                .filter(|reservation| can_manage || reservation.user_id == Some(auth_user_id))
                .map(|reservation| {
                    build_appointment_child_event_json(
                        reservation,
                        group,
                        context_codes,
                        can_manage,
                    )
                })
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    let available_slots = group
        .participants_per_appointment
        .map(|limit| (limit as i64 - slot_reservations.len() as i64).max(0));

    AppointmentEventJson {
        id: slot.id,
        created_at: slot.created_at,
        updated_at: slot.updated_at,
        start_at: slot.start_at,
        end_at: slot.end_at,
        all_day: false,
        all_day_date: None,
        title: slot.title.clone(),
        workflow_state: slot.workflow_state.clone(),
        comments: slot.comments.clone(),
        location_name: slot.location_name.clone(),
        location_address: slot.location_address.clone(),
        kind: "event",
        description: slot.description.clone(),
        child_events_count: slot_reservations.len() as i64,
        child_events,
        context_code: context_codes.first().cloned().unwrap_or_default(),
        effective_context_code: context_codes.join(","),
        all_context_codes: context_codes.join(","),
        parent_event_id: slot.parent_event_id,
        hidden: false,
        url: format!("/api/v1/calendar_events/{}", slot.id),
        html_url: appointment_event_html_url(context_codes, slot.id),
        appointment_group_id: group.id,
        appointment_group_url: appointment_group_url(group.id),
        can_manage_appointment_group: can_manage,
        participant_type: group.participant_type.clone(),
        reserved,
        reserve_comments,
        reserve_url: format!("/api/v1/calendar_events/{}/reservations", slot.id),
        available_slots,
        participants_per_appointment: group.participants_per_appointment,
        context_name: None,
        context_color: None,
        blackout_date: false,
        duplicates: Vec::new(),
        important_dates: false,
        series_uuid: None,
        rrule: None,
    }
}

fn build_appointment_child_event_json(
    reservation: &AppointmentReservationRow,
    group: &AppointmentGroupRow,
    context_codes: &[String],
    can_manage: bool,
) -> AppointmentChildEventJson {
    let context_code = match reservation.user_id {
        Some(user_id) => format!("user_{user_id}"),
        None => String::new(),
    };
    let user = reservation.user_id.and_then(|user_id| {
        reservation
            .user_name
            .as_ref()
            .map(|name| AppointmentEventUser {
                id: user_id,
                name: name.clone(),
                short_name: name.clone(),
                sortable_name: name.clone(),
            })
    });

    AppointmentChildEventJson {
        id: reservation.id,
        created_at: reservation.created_at,
        updated_at: reservation.updated_at,
        start_at: reservation.start_at,
        end_at: reservation.end_at,
        all_day: false,
        all_day_date: None,
        title: reservation.title.clone(),
        workflow_state: reservation.workflow_state.clone(),
        comments: reservation.comments.clone(),
        location_name: reservation.location_name.clone(),
        location_address: reservation.location_address.clone(),
        kind: "event",
        description: reservation.description.clone(),
        context_code,
        effective_context_code: if group.participant_type == "User" {
            Some(context_codes.join(","))
        } else {
            None
        },
        all_context_codes: context_codes.join(","),
        parent_event_id: reservation.parent_event_id,
        user_id: reservation.user_id,
        user,
        child_events: None,
        appointment_group_id: group.id,
        appointment_group_url: appointment_group_url(group.id),
        can_manage_appointment_group: can_manage,
        participant_type: group.participant_type.clone(),
        child_events_count: 0,
        hidden: false,
        url: format!("/api/v1/calendar_events/{}", reservation.id),
        html_url: appointment_event_html_url(context_codes, reservation.id),
        context_name: None,
        context_color: None,
        blackout_date: false,
        duplicates: Vec::new(),
        important_dates: false,
        series_uuid: None,
        rrule: None,
    }
}

fn render_appointment_group_json(
    group: &AppointmentGroupRow,
    snapshot: &AppointmentGroupSnapshot,
    auth_user_id: i64,
    is_student: bool,
    can_manage: bool,
    options: AppointmentGroupIncludeOptions,
    new_slots: Option<&[AppointmentSlotRow]>,
) -> AppointmentGroupJson {
    let reservations_for_user = snapshot
        .reservations
        .iter()
        .filter(|reservation| reservation.user_id == Some(auth_user_id))
        .count();
    let slot_reservation_counts: Vec<i64> = snapshot
        .slots
        .iter()
        .map(|slot| {
            snapshot
                .reservations
                .iter()
                .filter(|reservation| reservation.parent_event_id == Some(slot.id))
                .count() as i64
        })
        .collect();
    let all_filled =
        all_appointments_filled(&slot_reservation_counts, group.participants_per_appointment);
    let requires_action = is_student
        && requiring_action(
            group.min_appointments_per_participant,
            reservations_for_user,
            all_filled,
        );

    let now = Utc::now();
    let appointments = if options.appointments {
        Some(
            snapshot
                .slots
                .iter()
                .filter(|slot| {
                    options.include_past_appointments
                        || slot.end_at.is_none_or(|end_at| end_at > now)
                })
                .map(|slot| {
                    let slot_reservations: Vec<&AppointmentReservationRow> = snapshot
                        .reservations
                        .iter()
                        .filter(|reservation| reservation.parent_event_id == Some(slot.id))
                        .collect();
                    build_appointment_event_json(
                        slot,
                        group,
                        &snapshot.context_codes,
                        can_manage,
                        auth_user_id,
                        &slot_reservations,
                        options.child_events,
                    )
                })
                .collect::<Vec<_>>(),
        )
    } else {
        None
    };

    let participant_count = options
        .participant_count
        .then_some(snapshot.reservations.len() as i64);
    let reserved_times = options.reserved_times.then(|| {
        snapshot
            .reservations
            .iter()
            .filter(|reservation| reservation.user_id == Some(auth_user_id))
            .map(|reservation| AppointmentReservedTime {
                id: reservation.id,
                start_at: reservation.start_at,
                end_at: reservation.end_at,
            })
            .collect::<Vec<_>>()
    });
    let all_context_codes =
        (options.all_context_codes && can_manage).then(|| snapshot.context_codes.clone());
    let new_appointments = new_slots.filter(|slots| !slots.is_empty()).map(|slots| {
        slots
            .iter()
            .map(|slot| {
                build_appointment_event_json(
                    slot,
                    group,
                    &snapshot.context_codes,
                    can_manage,
                    auth_user_id,
                    &[],
                    false,
                )
            })
            .collect::<Vec<_>>()
    });

    AppointmentGroupJson {
        id: group.id,
        created_at: group.created_at,
        description: group.description.clone(),
        end_at: group.end_at,
        max_appointments_per_participant: group.max_appointments_per_participant,
        min_appointments_per_participant: group.min_appointments_per_participant,
        participants_per_appointment: group.participants_per_appointment,
        start_at: group.start_at,
        title: group.title.clone(),
        updated_at: group.updated_at,
        workflow_state: group.workflow_state.clone(),
        participant_visibility: group.participant_visibility.clone(),
        sub_context_codes: snapshot.sub_context_codes.clone(),
        location_address: group.location_address.clone(),
        location_name: group.location_name.clone(),
        participant_count,
        reserved_times,
        context_codes: snapshot.context_codes.clone(),
        all_context_codes,
        requiring_action: requires_action,
        new_appointments,
        appointments,
        appointments_count: snapshot.slots.len() as i64,
        participant_type: group.participant_type.clone(),
        url: appointment_group_url(group.id),
        html_url: appointment_group_html_url(group.id),
        allow_observer_signup: group.allow_observer_signup,
    }
}

/// Load a group's snapshot and render it (used by list / create / update).
async fn build_appointment_group_json(
    db: &PgPool,
    group: &AppointmentGroupRow,
    auth: &AuthUser,
    options: AppointmentGroupIncludeOptions,
    new_slots: Option<&[AppointmentSlotRow]>,
) -> Result<AppointmentGroupJson, ApiError> {
    let snapshot = load_appointment_snapshot(db, group.id).await?;
    let can_manage = can_manage_appointment_group(db, auth, group.id).await?;
    let is_student =
        has_course_membership(db, auth.id, &snapshot.course_ids, Some("student")).await?;
    Ok(render_appointment_group_json(
        group, &snapshot, auth.id, is_student, can_manage, options, new_slots,
    ))
}

/// Canvas reservation error list: 400 `[{attribute, type, message, reservations}]`.
fn reservation_failure(message: &str, reservations: Vec<AppointmentChildEventJson>) -> Response {
    let body = json!([{
        "attribute": "reservation",
        "type": "calendar_event",
        "message": message,
        "reservations": reservations,
    }]);
    (StatusCode::BAD_REQUEST, Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Appointment group handlers
// ---------------------------------------------------------------------------

/// GET /appointment_groups?scope=reservable|manageable&context_codes[]=
async fn list_appointment_groups(
    State(state): State<AppState>,
    Query(params): Query<AppointmentGroupListQuery>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Json<Vec<AppointmentGroupJson>>, ApiError> {
    let manageable = params.scope.as_deref() == Some("manageable");
    let include_past = truthy_str(params.include_past_appointments.as_deref());
    let context_codes: Vec<String> = query_values(raw_query.as_deref(), "context_codes")
        .into_iter()
        .filter(|code| parse_course_context_code(code).is_some())
        .collect();
    let context_filter = if context_codes.is_empty() {
        None
    } else {
        Some(context_codes)
    };
    let options = AppointmentGroupIncludeOptions::from_query(raw_query.as_deref(), include_past);

    let sql = format!(
        r#"
        SELECT {APPOINTMENT_GROUP_COLUMNS}
        FROM appointment_groups g
        WHERE g.workflow_state <> 'deleted'
          AND ($2 OR g.workflow_state = 'active')
          AND ($3 OR g.end_at >= now() OR ($2 AND g.end_at IS NULL))
          AND ($4::text[] IS NULL OR EXISTS (
              SELECT 1 FROM appointment_group_contexts agc2
              WHERE agc2.appointment_group_id = g.id
                AND agc2.context_code = ANY($4)
          ))
          AND (
              ($2 AND $5)
              OR EXISTS (
                  SELECT 1 FROM appointment_group_contexts agc
                  JOIN enrollments e ON e.course_id = agc.context_id
                  WHERE agc.appointment_group_id = g.id
                    AND e.user_id = $1
                    AND (($2 AND e.role = 'teacher') OR (NOT $2 AND e.role = 'student'))
              )
          )
        ORDER BY g.id ASC
        "#
    );
    let groups: Vec<AppointmentGroupRow> = sqlx::query_as(&sql)
        .bind(auth.id)
        .bind(manageable)
        .bind(include_past)
        .bind(context_filter.as_ref())
        .bind(is_admin(&auth))
        .fetch_all(&state.db)
        .await?;

    let mut response = Vec::with_capacity(groups.len());
    for group in groups {
        response.push(build_appointment_group_json(&state.db, &group, &auth, options, None).await?);
    }
    Ok(Json(response))
}

/// POST /appointment_groups — create a group with optional time slots.
async fn create_appointment_group(
    State(state): State<AppState>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
    Json(envelope): Json<AppointmentGroupEnvelope>,
) -> Result<(StatusCode, Json<AppointmentGroupJson>), ApiError> {
    let params = envelope.into_params();

    let title = params.title.unwrap_or_default();
    if title.trim().is_empty() {
        return Err(ApiError::BadRequest("title can't be blank".into()));
    }
    let description = match params.description.as_ref() {
        Some(value) => parse_nullable_text(value)?,
        None => None,
    };
    let location_name = match params.location_name.as_ref() {
        Some(value) => parse_nullable_text(value)?,
        None => None,
    };
    let location_address = match params.location_address.as_ref() {
        Some(value) => parse_nullable_text(value)?,
        None => None,
    };
    let cancel_reason = match params.cancel_reason.as_ref() {
        Some(value) => parse_nullable_text(value)?,
        None => None,
    };
    let participants_per_appointment = match params.participants_per_appointment.as_ref() {
        Some(value) => parse_nullable_i32(value)?,
        None => None,
    };
    let min_appointments_per_participant = match params.min_appointments_per_participant.as_ref() {
        Some(value) => parse_nullable_i32(value)?,
        None => None,
    };
    let max_appointments_per_participant = match params.max_appointments_per_participant.as_ref() {
        Some(value) => parse_nullable_i32(value)?,
        None => None,
    };
    let participant_visibility = params
        .participant_visibility
        .unwrap_or_else(|| "private".into());
    if participant_visibility != "private" && participant_visibility != "protected" {
        return Err(ApiError::BadRequest(
            "participant_visibility must be private or protected".into(),
        ));
    }
    let allow_observer_signup = params
        .allow_observer_signup
        .as_ref()
        .map(truthy_value)
        .unwrap_or(false);
    let publish = params.publish.as_ref().map(truthy_value).unwrap_or(false);
    let workflow_state = if publish { "active" } else { "pending" };

    let context_codes = params.context_codes.unwrap_or_default();
    let course_ids = resolve_managed_courses(&state.db, &auth, &context_codes, true).await?;
    let sub_context_codes = params.sub_context_codes.unwrap_or_default();
    let sub_contexts = resolve_sub_contexts(&state.db, &sub_context_codes).await?;
    let participant_type = participant_type_for(&sub_contexts);

    let new_appointments = parse_new_appointments(params.new_appointments.as_ref())?;
    validate_appointment_times(&new_appointments)?;

    let mut tx = state.db.begin().await?;
    let insert_sql = format!(
        r#"
        INSERT INTO appointment_groups (
            title, description, location_name, location_address,
            participant_visibility, participant_type, workflow_state,
            allow_observer_signup, participants_per_appointment,
            min_appointments_per_participant, max_appointments_per_participant
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        RETURNING {APPOINTMENT_GROUP_COLUMNS}
        "#
    );
    let group: AppointmentGroupRow = sqlx::query_as(&insert_sql)
        .bind(&title)
        .bind(&description)
        .bind(&location_name)
        .bind(&location_address)
        .bind(&participant_visibility)
        .bind(participant_type)
        .bind(workflow_state)
        .bind(allow_observer_signup)
        .bind(participants_per_appointment)
        .bind(min_appointments_per_participant)
        .bind(max_appointments_per_participant)
        .fetch_one(&mut *tx)
        .await?;

    for course_id in &course_ids {
        insert_group_context(&mut tx, group.id, *course_id).await?;
    }
    for sub_context in &sub_contexts {
        insert_group_sub_context(&mut tx, group.id, sub_context).await?;
    }

    let mut new_slots = Vec::with_capacity(new_appointments.len());
    for (start_at, end_at) in &new_appointments {
        new_slots.push(insert_appointment_slot(&mut tx, &group, *start_at, *end_at).await?);
    }
    refresh_group_cached_times(&mut tx, group.id).await?;
    if let Some(reason) = &cancel_reason {
        sqlx::query("UPDATE appointment_groups SET cancel_reason = $2 WHERE id = $1")
            .bind(group.id)
            .bind(reason)
            .execute(&mut *tx)
            .await?;
    }
    let group = fetch_appointment_group(&mut *tx, group.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    tx.commit().await?;

    let options = AppointmentGroupIncludeOptions::from_query(raw_query.as_deref(), false);
    let json = build_appointment_group_json(
        &state.db,
        &group,
        &auth,
        options,
        Some(new_slots.as_slice()),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json)))
}

/// GET /appointment_groups/{id} — appointments are always included.
async fn get_appointment_group(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<AppointmentGroupShowQuery>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Json<AppointmentGroupJson>, ApiError> {
    let group = fetch_appointment_group(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if group.workflow_state == "deleted" {
        return Err(ApiError::Forbidden);
    }
    let snapshot = load_appointment_snapshot(&state.db, id).await?;
    let can_manage = can_manage_appointment_group(&state.db, &auth, id).await?;
    if !can_manage {
        if group.workflow_state != "active" {
            return Err(ApiError::Forbidden);
        }
        if !has_course_membership(&state.db, auth.id, &snapshot.course_ids, None).await? {
            return Err(ApiError::Forbidden);
        }
    }
    let is_student =
        has_course_membership(&state.db, auth.id, &snapshot.course_ids, Some("student")).await?;
    let include_past = can_manage || truthy_str(params.include_past_appointments.as_deref());
    let mut options =
        AppointmentGroupIncludeOptions::from_query(raw_query.as_deref(), include_past);
    options.appointments = true;
    Ok(Json(render_appointment_group_json(
        &group, &snapshot, auth.id, is_student, can_manage, options, None,
    )))
}

/// PUT /appointment_groups/{id} — update fields, contexts and add slots.
async fn update_appointment_group(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
    Json(envelope): Json<AppointmentGroupEnvelope>,
) -> Result<Json<AppointmentGroupJson>, ApiError> {
    let params = envelope.into_params();
    let mut tx = state.db.begin().await?;
    let existing = fetch_appointment_group(&mut *tx, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if existing.workflow_state == "deleted" {
        return Err(ApiError::Forbidden);
    }
    if !can_manage_appointment_group(&state.db, &auth, id).await? {
        return Err(ApiError::Forbidden);
    }

    let title = match params.title {
        Some(title) if title.trim().is_empty() => {
            return Err(ApiError::BadRequest("title can't be blank".into()));
        }
        Some(title) => title,
        None => existing.title.clone(),
    };
    let description = match params.description.as_ref() {
        Some(value) => parse_nullable_text(value)?,
        None => existing.description.clone(),
    };
    let location_name = match params.location_name.as_ref() {
        Some(value) => parse_nullable_text(value)?,
        None => existing.location_name.clone(),
    };
    let location_address = match params.location_address.as_ref() {
        Some(value) => parse_nullable_text(value)?,
        None => existing.location_address.clone(),
    };
    let cancel_reason = match params.cancel_reason.as_ref() {
        Some(value) => parse_nullable_text(value)?,
        None => existing.cancel_reason.clone(),
    };
    let participants_per_appointment = match params.participants_per_appointment.as_ref() {
        Some(value) => parse_nullable_i32(value)?,
        None => existing.participants_per_appointment,
    };
    let min_appointments_per_participant = match params.min_appointments_per_participant.as_ref() {
        Some(value) => parse_nullable_i32(value)?,
        None => existing.min_appointments_per_participant,
    };
    let max_appointments_per_participant = match params.max_appointments_per_participant.as_ref() {
        Some(value) => parse_nullable_i32(value)?,
        None => existing.max_appointments_per_participant,
    };
    let participant_visibility = params
        .participant_visibility
        .unwrap_or_else(|| existing.participant_visibility.clone());
    if participant_visibility != "private" && participant_visibility != "protected" {
        return Err(ApiError::BadRequest(
            "participant_visibility must be private or protected".into(),
        ));
    }
    let allow_observer_signup = params
        .allow_observer_signup
        .as_ref()
        .map(truthy_value)
        .unwrap_or(existing.allow_observer_signup);
    let publish = params.publish.as_ref().map(truthy_value).unwrap_or(false);
    let workflow_state = if publish && existing.workflow_state == "pending" {
        "active".to_string()
    } else {
        existing.workflow_state.clone()
    };

    if let Some(codes) = params.context_codes.as_ref() {
        let course_ids = resolve_managed_courses(&state.db, &auth, codes, false).await?;
        sqlx::query("DELETE FROM appointment_group_contexts WHERE appointment_group_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for course_id in &course_ids {
            insert_group_context(&mut tx, id, *course_id).await?;
        }
    }

    let mut participant_type = existing.participant_type.clone();
    if let Some(codes) = params.sub_context_codes.as_ref() {
        let sub_contexts = resolve_sub_contexts(&state.db, codes).await?;
        participant_type = participant_type_for(&sub_contexts).to_string();
        sqlx::query("DELETE FROM appointment_group_sub_contexts WHERE appointment_group_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for sub_context in &sub_contexts {
            insert_group_sub_context(&mut tx, id, sub_context).await?;
        }
    }

    let mut new_slots = Vec::new();
    let new_appointments = parse_new_appointments(params.new_appointments.as_ref())?;
    if !new_appointments.is_empty() {
        let existing_times: Vec<(DateTime<Utc>, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT start_at, end_at FROM calendar_events \
             WHERE appointment_group_id = $1 AND parent_event_id IS NULL \
               AND workflow_state <> 'deleted'",
        )
        .bind(id)
        .fetch_all(&mut *tx)
        .await?;
        let mut all_times: Vec<(DateTime<Utc>, DateTime<Utc>)> = existing_times
            .into_iter()
            .map(|(start_at, end_at)| (start_at, end_at.unwrap_or(start_at)))
            .collect();
        all_times.extend(new_appointments.iter().copied());
        validate_appointment_times(&all_times)?;
    }

    let update_sql = format!(
        r#"
        UPDATE appointment_groups
        SET title = $2,
            description = $3,
            location_name = $4,
            location_address = $5,
            participants_per_appointment = $6,
            min_appointments_per_participant = $7,
            max_appointments_per_participant = $8,
            participant_visibility = $9,
            participant_type = $10,
            workflow_state = $11,
            allow_observer_signup = $12,
            cancel_reason = $13,
            updated_at = now()
        WHERE id = $1
        RETURNING {APPOINTMENT_GROUP_COLUMNS}
        "#
    );
    let updated: AppointmentGroupRow = sqlx::query_as(&update_sql)
        .bind(id)
        .bind(&title)
        .bind(&description)
        .bind(&location_name)
        .bind(&location_address)
        .bind(participants_per_appointment)
        .bind(min_appointments_per_participant)
        .bind(max_appointments_per_participant)
        .bind(&participant_visibility)
        .bind(&participant_type)
        .bind(&workflow_state)
        .bind(allow_observer_signup)
        .bind(&cancel_reason)
        .fetch_one(&mut *tx)
        .await?;

    propagate_group_attributes(&mut tx, &updated).await?;
    for (start_at, end_at) in &new_appointments {
        new_slots.push(insert_appointment_slot(&mut tx, &updated, *start_at, *end_at).await?);
    }
    refresh_group_cached_times(&mut tx, id).await?;
    let updated = fetch_appointment_group(&mut *tx, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    tx.commit().await?;

    let options = AppointmentGroupIncludeOptions::from_query(raw_query.as_deref(), false);
    let json = build_appointment_group_json(
        &state.db,
        &updated,
        &auth,
        options,
        Some(new_slots.as_slice()),
    )
    .await?;
    Ok(Json(json))
}

/// DELETE /appointment_groups/{id} — soft-delete the group, slots, reservations.
async fn delete_appointment_group(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<AppointmentGroupDeleteQuery>,
    auth: AuthUser,
) -> Result<Json<AppointmentGroupJson>, ApiError> {
    let mut tx = state.db.begin().await?;
    let existing = fetch_appointment_group(&mut *tx, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if existing.workflow_state == "deleted" {
        return Err(ApiError::NotFound);
    }
    if !can_manage_appointment_group(&state.db, &auth, id).await? {
        return Err(ApiError::Forbidden);
    }
    sqlx::query(
        "UPDATE appointment_groups \
         SET workflow_state = 'deleted', cancel_reason = $2, updated_at = now() WHERE id = $1",
    )
    .bind(id)
    .bind(&params.cancel_reason)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE calendar_events SET workflow_state = 'deleted', updated_at = now() \
         WHERE appointment_group_id = $1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let group = fetch_appointment_group(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let json = build_appointment_group_json(
        &state.db,
        &group,
        &auth,
        AppointmentGroupIncludeOptions::none(),
        None,
    )
    .await?;
    Ok(Json(json))
}

/// GET /appointment_groups/{id}/users — possible/registered participants.
async fn list_appointment_group_users(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(params): Query<AppointmentGroupUsersQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<AppointmentParticipant>>, ApiError> {
    let group = fetch_appointment_group(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if group.workflow_state == "deleted" {
        return Err(ApiError::Forbidden);
    }
    let snapshot = load_appointment_snapshot(&state.db, id).await?;
    let can_manage = can_manage_appointment_group(&state.db, &auth, id).await?;
    if !can_manage {
        if group.workflow_state != "active" {
            return Err(ApiError::Forbidden);
        }
        if !has_course_membership(&state.db, auth.id, &snapshot.course_ids, None).await? {
            return Err(ApiError::Forbidden);
        }
    }
    if group.participant_type != "User" || snapshot.course_ids.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let context_course = params
        .context_code
        .as_deref()
        .and_then(parse_course_context_code)
        .filter(|course_id| snapshot.course_ids.contains(course_id));
    let participants: Vec<AppointmentParticipant> = sqlx::query_as(
        r#"
        SELECT u.id, u.name, u.name AS sortable_name, u.name AS short_name,
               u.email, u.created_at
        FROM users u
        WHERE EXISTS (
            SELECT 1 FROM enrollments e
            WHERE e.user_id = u.id
              AND e.course_id = ANY($1)
              AND e.role = 'student'
              AND ($2::bigint IS NULL OR e.course_id = $2)
        )
        ORDER BY u.name ASC, u.id ASC
        "#,
    )
    .bind(&snapshot.course_ids)
    .bind(context_course)
    .fetch_all(&state.db)
    .await?;

    let registered_user_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT user_id FROM calendar_events \
         WHERE appointment_group_id = $1 AND parent_event_id IS NOT NULL \
           AND workflow_state <> 'deleted' AND user_id IS NOT NULL",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let filtered = match params.registration_status.as_deref() {
        Some("registered") => participants
            .into_iter()
            .filter(|participant| registered_user_ids.contains(&participant.id))
            .collect(),
        Some("unregistered") => participants
            .into_iter()
            .filter(|participant| !registered_user_ids.contains(&participant.id))
            .collect(),
        _ => participants,
    };
    Ok(Json(filtered))
}

/// POST /calendar_events/{id}/reservations — reserve a time slot.
async fn reserve_appointment(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<ReservationQuery>,
    auth: AuthUser,
    body: Option<Json<ReservationParams>>,
) -> Result<Response, ApiError> {
    let body = body.map(|Json(body)| body).unwrap_or_default();

    let slot = fetch_appointment_slot(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if slot.workflow_state == "deleted" {
        return Err(ApiError::NotFound);
    }
    let Some(group_id) = slot.appointment_group_id else {
        return Err(ApiError::Forbidden);
    };
    if slot.parent_event_id.is_some() {
        return Err(ApiError::Forbidden);
    }
    let group = fetch_appointment_group(&state.db, group_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    if group.workflow_state != "active" {
        return Err(ApiError::Forbidden);
    }
    let snapshot = load_appointment_snapshot(&state.db, group_id).await?;
    let can_manage = can_manage_appointment_group(&state.db, &auth, group_id).await?;

    let now = Utc::now();
    let slot_end = slot.end_at.unwrap_or(slot.start_at);
    if slot_end < now && !can_manage {
        return Ok((
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Cannot create or change reservation for past appointment" })),
        )
            .into_response());
    }

    if group.participant_type != "User" {
        return Ok(reservation_failure("invalid participant", Vec::new()));
    }

    let requested_participant_id = match body.participant_id.as_ref() {
        Some(value) if !value.is_null() => Some(parse_json_id(value)?),
        Some(_) => None,
        None => match query.participant_id.as_deref() {
            Some(raw) if !raw.trim().is_empty() => Some(
                raw.trim()
                    .parse::<i64>()
                    .map_err(|_| ApiError::BadRequest("invalid participant_id".into()))?,
            ),
            _ => None,
        },
    };
    let participant_id = match requested_participant_id {
        Some(participant_id) if participant_id != auth.id => {
            if !can_manage {
                return Ok(reservation_failure("invalid participant", Vec::new()));
            }
            participant_id
        }
        Some(participant_id) => participant_id,
        None => auth.id,
    };
    if !has_course_membership(
        &state.db,
        participant_id,
        &snapshot.course_ids,
        Some("student"),
    )
    .await?
    {
        return Ok(reservation_failure("invalid participant", Vec::new()));
    }
    let participant: Option<AppointmentParticipant> = sqlx::query_as(
        "SELECT u.id, u.name, u.name AS sortable_name, u.name AS short_name, \
                u.email, u.created_at \
         FROM users u WHERE u.id = $1",
    )
    .bind(participant_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(participant) = participant else {
        return Ok(reservation_failure("invalid participant", Vec::new()));
    };

    let cancel_existing = body
        .cancel_existing
        .as_ref()
        .map(truthy_value)
        .unwrap_or_else(|| truthy_str(query.cancel_existing.as_deref()));
    let comments = body.comments.clone().or_else(|| query.comments.clone());

    let participant_reservations: Vec<&AppointmentReservationRow> = snapshot
        .reservations
        .iter()
        .filter(|reservation| reservation.user_id == Some(participant_id))
        .collect();
    let reservations_json = || -> Vec<AppointmentChildEventJson> {
        participant_reservations
            .iter()
            .map(|reservation| {
                build_appointment_child_event_json(
                    reservation,
                    &group,
                    &snapshot.context_codes,
                    can_manage,
                )
            })
            .collect()
    };

    if cancel_existing
        && participant_reservations
            .iter()
            .any(|reservation| reservation.end_at.unwrap_or(reservation.start_at) < now)
    {
        return Ok(reservation_failure(
            "cannot cancel past reservation",
            reservations_json(),
        ));
    }

    let remaining_reservations = if cancel_existing {
        0
    } else {
        participant_reservations.len()
    };
    if let Some(maximum) = group.max_appointments_per_participant {
        if remaining_reservations as i32 >= maximum {
            return Ok(reservation_failure(
                "participant has met per-participant limit",
                reservations_json(),
            ));
        }
    }

    let slot_reservations: Vec<&AppointmentReservationRow> = snapshot
        .reservations
        .iter()
        .filter(|reservation| reservation.parent_event_id == Some(slot.id))
        .collect();
    let mut slot_filled = slot_reservations.len() as i32;
    if cancel_existing {
        slot_filled -= slot_reservations
            .iter()
            .filter(|reservation| reservation.user_id == Some(participant_id))
            .count() as i32;
    }
    if let Some(limit) = group.participants_per_appointment {
        if slot_filled >= limit {
            return Ok(reservation_failure("all slots filled", reservations_json()));
        }
    }

    if !cancel_existing
        && participant_reservations
            .iter()
            .any(|reservation| reservation.parent_event_id == Some(slot.id))
    {
        return Ok(reservation_failure(
            "participant has already reserved this appointment",
            reservations_json(),
        ));
    }

    let mut tx = state.db.begin().await?;
    if cancel_existing {
        sqlx::query(
            "UPDATE calendar_events SET workflow_state = 'deleted', updated_at = now() \
             WHERE appointment_group_id = $1 AND parent_event_id IS NOT NULL \
               AND user_id = $2 AND workflow_state <> 'deleted'",
        )
        .bind(group_id)
        .bind(participant_id)
        .execute(&mut *tx)
        .await?;
    }
    let insert_sql = format!(
        r#"
        INSERT INTO calendar_events
            (course_id, user_id, appointment_group_id, parent_event_id, title, description,
             start_at, end_at, workflow_state, location_name, location_address, comments)
        VALUES (NULL, $1, $2, $3, $4, $5, $6, $7, 'locked', $8, $9, $10)
        RETURNING {APPOINTMENT_SLOT_COLUMNS}
        "#
    );
    let inserted: AppointmentSlotRow = sqlx::query_as(&insert_sql)
        .bind(participant_id)
        .bind(group_id)
        .bind(slot.id)
        .bind(&slot.title)
        .bind(&slot.description)
        .bind(slot.start_at)
        .bind(slot.end_at.unwrap_or(slot.start_at))
        .bind(&slot.location_name)
        .bind(&slot.location_address)
        .bind(&comments)
        .fetch_one(&mut *tx)
        .await?;
    // Canvas locks the slot once it has a reservation.
    sqlx::query(
        "UPDATE calendar_events SET workflow_state = 'locked', updated_at = now() \
         WHERE id = $1 AND workflow_state = 'active'",
    )
    .bind(slot.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let reservation = AppointmentReservationRow {
        id: inserted.id,
        parent_event_id: inserted.parent_event_id,
        user_id: inserted.user_id,
        user_name: Some(participant.name.clone()),
        title: inserted.title,
        description: inserted.description,
        start_at: inserted.start_at,
        end_at: inserted.end_at,
        workflow_state: inserted.workflow_state,
        comments: inserted.comments,
        location_name: inserted.location_name,
        location_address: inserted.location_address,
        created_at: inserted.created_at,
        updated_at: inserted.updated_at,
    };
    let mut json = build_appointment_child_event_json(
        &reservation,
        &group,
        &snapshot.context_codes,
        can_manage,
    );
    // Canvas renders reservation events with the default child_events include.
    json.child_events = Some(Vec::new());
    Ok((StatusCode::OK, Json(json)).into_response())
}

// ---------------------------------------------------------------------------
// Phase 4 — calendar preferences + visible contexts
// (Canvas CalendarEventsApiController: visible_contexts, save_selected_contexts,
//  save_enabled_account_calendars)
// ---------------------------------------------------------------------------

/// Body for POST /calendar_events/save_selected_contexts.
#[derive(Debug, Default, Deserialize)]
pub struct SaveSelectedContexts {
    #[serde(default)]
    pub selected_contexts: Option<Vec<String>>,
}

/// Body for POST /calendar_events/save_enabled_account_calendars.
#[derive(Debug, Default, Deserialize)]
pub struct SaveEnabledAccountCalendars {
    /// Kept as a loose value: Canvas coerces string/boolean form input.
    #[serde(default)]
    pub mark_feature_as_seen: Option<Value>,
    #[serde(default)]
    pub enabled_account_calendars: Option<Vec<String>>,
}

/// Canvas `calendar_events/visible_contexts` entry.
#[derive(Debug, Clone, Serialize)]
pub struct VisibleContext {
    pub id: String,
    pub name: String,
    pub asset_string: String,
    pub color: Option<String>,
    pub selected: bool,
    pub allow_observers_in_appointment_groups: bool,
    pub can_create_appointment_groups: bool,
    pub sections: Vec<Value>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct VisibleContextCourse {
    id: i64,
    name: String,
    can_manage: bool,
}

/// Build the Canvas `{ contexts: [...] }` payload. The user's personal calendar
/// comes first, then the enrolled courses (Canvas `favorites_first`).
fn build_visible_contexts(
    user_id: i64,
    user_name: &str,
    courses: &[VisibleContextCourse],
    selected_contexts: &[String],
) -> Value {
    let is_selected = |asset_string: &str| selected_contexts.iter().any(|s| s == asset_string);

    let user_asset = format!("user_{user_id}");
    let mut contexts = vec![VisibleContext {
        id: user_id.to_string(),
        name: user_name.to_string(),
        asset_string: user_asset.clone(),
        color: None,
        selected: is_selected(&user_asset),
        allow_observers_in_appointment_groups: false,
        can_create_appointment_groups: false,
        sections: Vec::new(),
    }];

    for course in courses {
        let asset_string = format!("course_{}", course.id);
        contexts.push(VisibleContext {
            id: course.id.to_string(),
            name: course.name.clone(),
            selected: is_selected(&asset_string),
            asset_string,
            color: None,
            allow_observers_in_appointment_groups: false,
            can_create_appointment_groups: course.can_manage,
            sections: Vec::new(),
        });
    }

    json!({ "contexts": contexts })
}

/// Load `selected_calendar_contexts`, `enabled_account_calendars` and
/// `account_calendar_events_seen` for a user (empty defaults when unset).
async fn load_calendar_preferences(
    db: &PgPool,
    user_id: i64,
) -> Result<(Vec<String>, Vec<String>, bool), ApiError> {
    let row: Option<(Vec<String>, Vec<String>, bool)> = sqlx::query_as(
        r#"
        SELECT selected_contexts, enabled_account_calendars, account_calendars_seen
        FROM calendar_user_preferences
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(db)
    .await?;

    Ok(row.unwrap_or_default())
}

/// Upsert only the preferences present in the request (Canvas `set_preference`).
async fn save_calendar_preferences(
    db: &PgPool,
    user_id: i64,
    selected_contexts: Option<Vec<String>>,
    enabled_account_calendars: Option<Vec<String>>,
    mark_feature_as_seen: Option<bool>,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO calendar_user_preferences
            (user_id, selected_contexts, enabled_account_calendars, account_calendars_seen)
        VALUES ($1, COALESCE($2, '{}'), COALESCE($3, '{}'), COALESCE($4, false))
        ON CONFLICT (user_id) DO UPDATE
        SET selected_contexts = COALESCE($2, calendar_user_preferences.selected_contexts),
            enabled_account_calendars = COALESCE($3, calendar_user_preferences.enabled_account_calendars),
            account_calendars_seen = COALESCE($4, calendar_user_preferences.account_calendars_seen),
            updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(selected_contexts)
    .bind(enabled_account_calendars)
    .bind(mark_feature_as_seen)
    .execute(db)
    .await?;

    Ok(())
}

/// Parse `start_date` / `end_date` for the user-calendar range (defaults today).
fn parse_calendar_range(params: &CalendarRangeParams) -> (NaiveDate, NaiveDate) {
    let today = Utc::now().date_naive();
    let start_date = params
        .start_date
        .as_ref()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or(today);
    let end_date = params
        .end_date
        .as_ref()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or(today);
    normalize_calendar_range(start_date, end_date)
}

/// POST /calendar_events — create a personal event on the caller's calendar.
async fn create_own_calendar_event(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateCalendarEvent>,
) -> Result<(StatusCode, Json<CalendarEvent>), ApiError> {
    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title cannot be empty".into()));
    }

    let row: CalendarEvent = sqlx::query_as(
        r#"
        INSERT INTO calendar_events (course_id, user_id, title, description, start_at, end_at, workflow_state, location_name)
        VALUES (NULL, $1, $2, $3, $4, $5, 'active', $6)
        RETURNING id, course_id, user_id, title, description, start_at, end_at, workflow_state, location_name, created_at, updated_at
        "#,
    )
    .bind(auth.id)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(payload.start_at)
    .bind(payload.end_at)
    .bind(&payload.location_name)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /users/{userId}/calendar_events — Canvas `user_index`.
///
/// Self or admin only (Canvas additionally allows observers; the crate has no
/// observer role). Returns the user's personal events plus events of the
/// courses they are enrolled in, within the requested date range.
async fn list_calendar_events_for_user(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    Query(params): Query<CalendarRangeParams>,
    auth: AuthUser,
) -> Result<Json<Vec<CalendarEvent>>, ApiError> {
    let target_id = if user_id == "self" {
        auth.id
    } else {
        user_id
            .trim()
            .parse::<i64>()
            .map_err(|_| ApiError::NotFound)?
    };

    if target_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let existing: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(target_id)
        .fetch_optional(&state.db)
        .await?;
    if existing.is_none() {
        return Err(ApiError::NotFound);
    }

    let (start_date, end_date) = parse_calendar_range(&params);

    let course_ids: Vec<i64> =
        sqlx::query_scalar("SELECT DISTINCT course_id FROM enrollments WHERE user_id = $1")
            .bind(target_id)
            .fetch_all(&state.db)
            .await?;

    let rows: Vec<CalendarEvent> = sqlx::query_as(
        r#"
        SELECT id, course_id, user_id, title, description, start_at, end_at,
               workflow_state, location_name, created_at, updated_at
        FROM calendar_events
        WHERE workflow_state <> 'deleted'
          AND start_at >= $2
          AND start_at < ($3 + interval '1 day')
          AND (user_id = $1 OR course_id = ANY($4))
        ORDER BY start_at ASC
        "#,
    )
    .bind(target_id)
    .bind(start_date)
    .bind(end_date)
    .bind(&course_ids)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /calendar_events/visible_contexts — contexts the user can see.
async fn list_visible_contexts(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let (selected_contexts, _, _) = load_calendar_preferences(&state.db, auth.id).await?;

    let user_name: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
        .bind(auth.id)
        .fetch_optional(&state.db)
        .await?;
    let user_name = user_name.map(|(name,)| name).unwrap_or_default();

    let courses: Vec<VisibleContextCourse> = sqlx::query_as(
        r#"
        SELECT c.id,
               c.name,
               EXISTS (
                   SELECT 1 FROM enrollments te
                   WHERE te.course_id = c.id AND te.user_id = $1 AND te.role = 'teacher'
               ) AS can_manage
        FROM courses c
        JOIN enrollments e ON e.course_id = c.id
        WHERE e.user_id = $1
        ORDER BY c.id
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(build_visible_contexts(
        auth.id,
        &user_name,
        &courses,
        &selected_contexts,
    )))
}

/// POST /calendar_events/save_selected_contexts — persist the selection.
async fn save_selected_contexts(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<SaveSelectedContexts>,
) -> Result<Json<Value>, ApiError> {
    save_calendar_preferences(&state.db, auth.id, payload.selected_contexts, None, None).await?;
    Ok(Json(json!({ "status": "ok" })))
}

/// POST /calendar_events/save_enabled_account_calendars — persist account picks.
async fn save_enabled_account_calendars(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<SaveEnabledAccountCalendars>,
) -> Result<Json<Value>, ApiError> {
    let mark_feature_as_seen = payload.mark_feature_as_seen.as_ref().map(truthy_value);

    save_calendar_preferences(
        &state.db,
        auth.id,
        None,
        payload.enabled_account_calendars,
        mark_feature_as_seen,
    )
    .await?;
    Ok(Json(json!({ "status": "ok" })))
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
        .route("/calendar_events", post(create_own_calendar_event))
        .route(
            "/users/{userId}/calendar_events",
            get(list_calendar_events_for_user),
        )
        .route(
            "/calendar_events/visible_contexts",
            get(list_visible_contexts),
        )
        .route(
            "/calendar_events/save_selected_contexts",
            post(save_selected_contexts),
        )
        .route(
            "/calendar_events/save_enabled_account_calendars",
            post(save_enabled_account_calendars),
        )
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
        // Appointment groups (Canvas Scheduler)
        .route(
            "/appointment_groups",
            get(list_appointment_groups).post(create_appointment_group),
        )
        .route("/appointment_groups/{id}", get(get_appointment_group))
        .route("/appointment_groups/{id}", put(update_appointment_group))
        .route("/appointment_groups/{id}", delete(delete_appointment_group))
        .route(
            "/appointment_groups/{id}/users",
            get(list_appointment_group_users),
        )
        .route(
            "/calendar_events/{id}/reservations",
            post(reserve_appointment),
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

#[cfg(test)]
mod appointment_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_appointments_accept_arrays_objects_and_hash_form() {
        let value = json!([
            ["2030-01-01T10:00:00Z", "2030-01-01T11:00:00Z"],
            { "start_at": "2030-01-01T12:00:00Z" }
        ]);
        let slots = parse_new_appointments(Some(&value)).unwrap();
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].0.to_rfc3339(), "2030-01-01T10:00:00+00:00");
        assert_eq!(slots[0].1.to_rfc3339(), "2030-01-01T11:00:00+00:00");
        // A missing end_at defaults to start_at (Canvas populate_missing_dates).
        assert_eq!(slots[1].0, slots[1].1);

        let hash_form = json!({
            "1": ["2030-01-02 09:00:00", "2030-01-02 10:00:00"],
            "0": ["2030-01-01 09:00:00", "2030-01-01 10:00:00"]
        });
        let slots = parse_new_appointments(Some(&hash_form)).unwrap();
        assert_eq!(slots[0].0.to_rfc3339(), "2030-01-01T09:00:00+00:00");
        assert_eq!(slots[1].0.to_rfc3339(), "2030-01-02T09:00:00+00:00");

        assert!(parse_new_appointments(None).unwrap().is_empty());
        assert!(parse_new_appointments(Some(&json!("nope"))).is_err());
        assert!(parse_new_appointments(Some(&json!([["not a date", "nope"]]))).is_err());

        let nested: AppointmentGroupEnvelope = serde_json::from_value(json!({
            "appointment_group": {
                "title": "Office Hours",
                "context_codes": ["course_1"],
                "new_appointments": [["2030-01-01T10:00:00Z", "2030-01-01T11:00:00Z"]]
            }
        }))
        .unwrap();
        let params = nested.into_params();
        assert_eq!(params.title.as_deref(), Some("Office Hours"));
        assert_eq!(params.context_codes.unwrap(), vec!["course_1".to_string()]);

        let flat: AppointmentGroupEnvelope =
            serde_json::from_value(json!({ "title": "Flat", "publish": true })).unwrap();
        let params = flat.into_params();
        assert_eq!(params.title.as_deref(), Some("Flat"));
        assert!(params.publish.as_ref().map(truthy_value).unwrap());
    }

    #[test]
    fn appointment_times_clamp_reversed_ends_and_reject_overlaps() {
        let value = json!([["2030-01-01T10:00:00Z", "2030-01-01T09:00:00Z"]]);
        let slots = parse_new_appointments(Some(&value)).unwrap();
        assert_eq!(slots[0].0, slots[0].1);
        validate_appointment_times(&slots).unwrap();

        let start = parse_appointment_timestamp("2030-01-01T10:00:00Z").unwrap();
        let overlapping = vec![
            (start, start + chrono::Duration::hours(2)),
            (
                start + chrono::Duration::hours(1),
                start + chrono::Duration::hours(3),
            ),
        ];
        assert!(validate_appointment_times(&overlapping).is_err());

        let adjacent = vec![
            (start, start + chrono::Duration::hours(1)),
            (
                start + chrono::Duration::hours(1),
                start + chrono::Duration::hours(2),
            ),
        ];
        validate_appointment_times(&adjacent).unwrap();
    }

    #[test]
    fn context_codes_parse_courses_sections_and_group_categories() {
        assert_eq!(parse_course_context_code("course_42"), Some(42));
        assert_eq!(parse_course_context_code("course_section_7"), None);
        assert_eq!(parse_course_context_code("group_category_9"), None);
        assert_eq!(
            parse_sub_context_code("course_section_7"),
            Some(SubContextRef::CourseSection(7))
        );
        assert_eq!(
            parse_sub_context_code("group_category_9"),
            Some(SubContextRef::GroupCategory(9))
        );
        assert_eq!(parse_sub_context_code("course_42"), None);
    }

    #[test]
    fn capacity_and_minimum_signup_helpers_match_canvas() {
        assert!(all_appointments_filled(&[2, 2], Some(2)));
        assert!(!all_appointments_filled(&[2, 1], Some(2)));
        assert!(!all_appointments_filled(&[2, 2], None));
        assert!(!all_appointments_filled(&[], Some(1)));
        assert!(all_appointments_filled(&[0], Some(0)));

        assert!(requiring_action(Some(2), 1, false));
        assert!(!requiring_action(Some(2), 2, false));
        assert!(!requiring_action(Some(2), 0, true));
        assert!(!requiring_action(None, 0, false));
    }
}

#[cfg(test)]
mod calendar_deep_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn visible_contexts_stringify_ids_and_mark_selection() {
        let courses = vec![
            VisibleContextCourse {
                id: 1,
                name: "Course One".into(),
                can_manage: true,
            },
            VisibleContextCourse {
                id: 2,
                name: "Course Two".into(),
                can_manage: false,
            },
        ];
        let selected = vec!["course_2".to_string(), "user_9".to_string()];

        let payload = build_visible_contexts(9, "Ada Lovelace", &courses, &selected);
        let contexts = payload["contexts"].as_array().unwrap();
        assert_eq!(contexts.len(), 3);

        assert_eq!(contexts[0]["id"], json!("9"));
        assert_eq!(contexts[0]["asset_string"], json!("user_9"));
        assert_eq!(contexts[0]["name"], json!("Ada Lovelace"));
        assert_eq!(contexts[0]["selected"], json!(true));
        assert_eq!(contexts[0]["sections"], json!([]));

        assert_eq!(contexts[1]["asset_string"], json!("course_1"));
        assert_eq!(contexts[1]["selected"], json!(false));
        assert_eq!(contexts[1]["can_create_appointment_groups"], json!(true));

        assert_eq!(contexts[2]["id"], json!("2"));
        assert_eq!(contexts[2]["selected"], json!(true));
        assert_eq!(contexts[2]["can_create_appointment_groups"], json!(false));
        assert_eq!(contexts[2]["color"], Value::Null);
    }

    #[test]
    fn save_bodies_default_to_noop_and_coerce_mark_feature_as_seen() {
        let empty: SaveSelectedContexts = serde_json::from_value(json!({})).unwrap();
        assert!(empty.selected_contexts.is_none());

        let body: SaveSelectedContexts =
            serde_json::from_value(json!({ "selected_contexts": ["course_1", "user_2"] })).unwrap();
        assert_eq!(body.selected_contexts.unwrap().len(), 2);

        let seen: SaveEnabledAccountCalendars = serde_json::from_value(json!({
            "mark_feature_as_seen": "true",
            "enabled_account_calendars": ["1", "2"]
        }))
        .unwrap();
        assert!(truthy_value(seen.mark_feature_as_seen.as_ref().unwrap()));
        assert_eq!(seen.enabled_account_calendars.unwrap().len(), 2);

        let never_seen: SaveEnabledAccountCalendars =
            serde_json::from_value(json!({ "mark_feature_as_seen": false })).unwrap();
        assert!(!truthy_value(
            never_seen.mark_feature_as_seen.as_ref().unwrap()
        ));
    }

    /// DB-backed: preference round-trip pins the actual upsert behavior
    /// (absent fields keep their stored value).
    #[tokio::test]
    async fn calendar_preferences_round_trip() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            println!("SKIP: DATABASE_URL not set");
            return;
        };

        let db = PgPool::connect(&database_url).await.expect("connect");
        let prefix = format!(
            "caldeep-{}-{}",
            std::process::id(),
            Utc::now().timestamp_millis()
        );

        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (name, email, password_hash) VALUES ($1, $2, 'x') RETURNING id",
        )
        .bind(&prefix)
        .bind(format!("{prefix}@example.test"))
        .fetch_one(&db)
        .await
        .expect("insert user");

        // Unset preferences load as empty defaults.
        assert_eq!(
            load_calendar_preferences(&db, user_id).await.unwrap(),
            (Vec::<String>::new(), Vec::<String>::new(), false)
        );

        save_calendar_preferences(
            &db,
            user_id,
            Some(vec!["course_1".to_string()]),
            Some(vec!["7".to_string()]),
            Some(true),
        )
        .await
        .unwrap();
        assert_eq!(
            load_calendar_preferences(&db, user_id).await.unwrap(),
            (vec!["course_1".to_string()], vec!["7".to_string()], true)
        );

        // Absent fields (NULL binds) keep the stored values.
        save_calendar_preferences(&db, user_id, Some(vec!["user_3".to_string()]), None, None)
            .await
            .unwrap();
        assert_eq!(
            load_calendar_preferences(&db, user_id).await.unwrap(),
            (vec!["user_3".to_string()], vec!["7".to_string()], true)
        );

        sqlx::query("DELETE FROM calendar_user_preferences WHERE user_id = $1")
            .bind(user_id)
            .execute(&db)
            .await
            .unwrap();
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&db)
            .await
            .unwrap();
    }
}
