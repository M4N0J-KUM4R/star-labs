//! Calendar parity gaps — batch P3.
//!
//! Canvas `CalendarEventsApiController` reserve/participants + the timetable
//! feature-off routes, and `AppointmentGroupsController`
//! next_appointment/groups:
//!
//!   - POST /calendar_events/{id}/reservations/{participant_id}  (#reserve —
//!     the path-participant spelling of the sibling `POST .../reservations`
//!     route; the request is delegated to that handler so the Canvas
//!     reservation semantics (limits, cancel_existing, past-slot 403, the
//!     `[{attribute, type, message, reservations}]` 400 shape) are identical).
//!   - GET  /calendar_events/{id}/participants                     (#participants)
//!   - GET/POST /courses/{courseId}/calendar_events/timetable      (timetable)
//!   - POST /courses/{courseId}/calendar_events/timetable_events   (timetable)
//!   - GET  /appointment_groups/next_appointment                   (#next_appointment)
//!   - GET  /appointment_groups/{id}/groups                        (#groups)
//!
//! Shared stances (per-handler docs give the detail):
//!   - reservations are `calendar_events` child rows (`parent_event_id`),
//!     exactly like the crate's existing Scheduler implementation;
//!   - the timetable endpoints are Canvas' course-section timetable feature
//!     which this workspace does not back: GET answers the empty
//!     `{"timetable": []}` shape, POST persists nothing and answers the same
//!     empty shape (documented feature-off stance);
//!   - the gating chain is 401 -> 404 (unknown context) -> 403.

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Whether a course exists (soft-deleted courses are gone, matching the
/// workspace's `course_exists` helpers).
async fn course_exists(db: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// calendar_events_api — reserve (path participant)
// ---------------------------------------------------------------------------

/// POST /calendar_events/{id}/reservations/{participant_id} — Canvas:
/// calendar_events_api#reserve —
/// POST /api/v1/calendar_events/:id/reservations/:participant_id.
///
/// Reserves the appointment slot for the participant named in the path: the
/// caller reserving for anyone but themselves must manage the group (Canvas
/// `cancel_existing` / comments still come from the body or query). Delegates
/// to the crate's `reserve_appointment` so every Canvas behavior — unknown
/// slot 404, past-appointment 403, per-participant limits, capacity, the
/// reservation event JSON — matches the sibling
/// `POST /calendar_events/{id}/reservations` route exactly.
async fn reserve_for_participant(
    state: State<AppState>,
    Path((id, participant_id)): Path<(i64, i64)>,
    Query(query): Query<crate::ReservationQuery>,
    auth: AuthUser,
    body: Option<Json<crate::ReservationParams>>,
) -> Result<Response, ApiError> {
    let mut params = body.map(|Json(body)| body).unwrap_or_default();
    // The path participant wins over any body/query spelling.
    params.participant_id = Some(json!(participant_id));
    crate::reserve_appointment(state, Path(id), Query(query), auth, Some(Json(params))).await
}

// ---------------------------------------------------------------------------
// calendar_events_api — participants
// ---------------------------------------------------------------------------

/// Context columns of a calendar event needed for the participants gate.
#[derive(Debug, sqlx::FromRow)]
struct EventContextRow {
    appointment_group_id: Option<i64>,
    course_id: Option<i64>,
    workflow_state: String,
}

/// GET /calendar_events/{id}/participants — Canvas:
/// calendar_events_api#participants —
/// GET /api/v1/calendar_events/:id/participants.
///
/// The participant list of an appointment slot for a manager of its context
/// (group manager / course teacher / admin — 403 otherwise, 404 for an
/// unknown or deleted event). Participants are the users holding a
/// non-deleted child reservation; slots without reservations (and plain
/// non-appointment events) answer `{"participants": []}`.
async fn appointment_participants(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let event: Option<EventContextRow> = sqlx::query_as(
        "SELECT appointment_group_id, course_id, workflow_state \
         FROM calendar_events WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some(row) = event else {
        return Err(ApiError::NotFound);
    };
    if row.workflow_state == "deleted" {
        return Err(ApiError::NotFound);
    }

    // Manage rights on the event's context: the owning appointment group, the
    // owning course, or (personal events) the global admin.
    let can_manage = match (row.appointment_group_id, row.course_id) {
        (Some(group_id), _) => {
            crate::can_manage_appointment_group(&state.db, &auth, group_id).await?
        }
        (None, Some(course_id)) => {
            crate::check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?
        }
        (None, None) => crate::is_admin(&auth),
    };
    if !can_manage {
        return Err(ApiError::Forbidden);
    }

    let participants: Vec<crate::AppointmentParticipant> = sqlx::query_as(
        r#"
        SELECT DISTINCT u.id, u.name, u.name AS sortable_name, u.name AS short_name,
               u.email, u.created_at
        FROM calendar_events c
        JOIN users u ON u.id = c.user_id
        WHERE c.parent_event_id = $1 AND c.workflow_state <> 'deleted'
        ORDER BY u.id ASC
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "participants": participants })))
}

// ---------------------------------------------------------------------------
// calendar_events_api — course timetable (feature-off)
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/calendar_events/timetable — Canvas:
/// calendar_events_api#get_course_timetable —
/// GET /api/v1/courses/:course_id/calendar_events/timetable.
///
/// Canvas renders the course-section timetable (`CourseSection#timetable`
/// strings). The workspace does not back the timetable feature, so this
/// answers Canvas' empty shape `{"timetable": []}` (teacher/admin only; 404
/// for an unknown course).
async fn get_course_timetable(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !crate::check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(json!({ "timetable": [] })))
}

/// POST /courses/{courseId}/calendar_events/timetable — Canvas:
/// calendar_events_api#set_course_timetable —
/// POST /api/v1/courses/:course_id/calendar_events/timetable.
///
/// Feature-off stance: the posted timetable is accepted but persists nothing
/// and the empty timetable shape is echoed (teacher/admin only; 404 for an
/// unknown course).
async fn set_course_timetable(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    _payload: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !crate::check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(json!({ "timetable": [] })))
}

/// POST /courses/{courseId}/calendar_events/timetable_events — Canvas:
/// calendar_events_api#set_course_timetable_events —
/// POST /api/v1/courses/:course_id/calendar_events/timetable_events.
///
/// Feature-off stance, same as the timetable pair: no events are generated
/// and the empty `timetable_events` shape is echoed (teacher/admin only; 404
/// for an unknown course).
async fn set_course_timetable_events(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    _payload: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !crate::check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(json!({ "timetable_events": [] })))
}

// ---------------------------------------------------------------------------
// appointment_groups — next_appointment
// ---------------------------------------------------------------------------

/// The caller's earliest upcoming reservation plus its appointment group id
/// (the parent slot carries the group).
#[derive(Debug, sqlx::FromRow)]
struct UpcomingReservation {
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
    group_id: i64,
}

impl UpcomingReservation {
    fn into_reservation_row(self) -> crate::AppointmentReservationRow {
        crate::AppointmentReservationRow {
            id: self.id,
            parent_event_id: self.parent_event_id,
            user_id: self.user_id,
            user_name: self.user_name,
            title: self.title,
            description: self.description,
            start_at: self.start_at,
            end_at: self.end_at,
            workflow_state: self.workflow_state,
            comments: self.comments,
            location_name: self.location_name,
            location_address: self.location_address,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// GET /appointment_groups/next_appointment — Canvas:
/// appointment_groups#next_appointment —
/// GET /api/v1/appointment_groups/next_appointment.
///
/// The caller's next reserved appointment: the earliest reservation that has
/// not ended yet (start_at order), rendered as the reservation calendar event
/// (the same child-event JSON the appointment group API emits). Canvas
/// answers 404 when the caller has no upcoming reservation.
async fn next_appointment(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<crate::AppointmentChildEventJson>, ApiError> {
    let upcoming: Option<UpcomingReservation> = sqlx::query_as(
        r#"
        SELECT c.id, c.parent_event_id, c.user_id, u.name AS user_name, c.title,
               c.description, c.start_at, c.end_at, c.workflow_state, c.comments,
               c.location_name, c.location_address, c.created_at, c.updated_at,
               p.appointment_group_id AS group_id
        FROM calendar_events c
        LEFT JOIN users u ON u.id = c.user_id
        JOIN calendar_events p ON p.id = c.parent_event_id
        WHERE c.user_id = $1
          AND c.parent_event_id IS NOT NULL
          AND c.workflow_state <> 'deleted'
          AND p.appointment_group_id IS NOT NULL
          AND COALESCE(c.end_at, c.start_at) > now()
        ORDER BY c.start_at ASC, c.id ASC
        LIMIT 1
        "#,
    )
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    let Some(upcoming) = upcoming else {
        return Err(ApiError::NotFound);
    };

    let group = crate::fetch_appointment_group(&state.db, upcoming.group_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let snapshot = crate::load_appointment_snapshot(&state.db, upcoming.group_id).await?;
    let can_manage =
        crate::can_manage_appointment_group(&state.db, &auth, upcoming.group_id).await?;
    let reservation = upcoming.into_reservation_row();
    Ok(Json(crate::build_appointment_child_event_json(
        &reservation,
        &group,
        &snapshot.context_codes,
        can_manage,
    )))
}

// ---------------------------------------------------------------------------
// appointment_groups — groups
// ---------------------------------------------------------------------------

/// One eligible signup group (Canvas `group_json` subset).
#[derive(Debug, serde::Serialize, sqlx::FromRow)]
struct EligibleGroup {
    id: i64,
    name: String,
    group_category_id: Option<i64>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// GET /appointment_groups/{id}/groups — Canvas: appointment_groups#groups —
/// GET /api/v1/appointment_groups/:id/groups.
///
/// The groups eligible to sign up: the groups of the appointment group's
/// group-category sub-contexts (Canvas `AppointmentGroup#groups` — group
/// signups are restricted through GroupCategory sub-contexts). Readable by
/// anyone who can read the appointment group (404 unknown/deleted group,
/// 403 otherwise); answers `{"groups": []}` when the group has no
/// group-category sub-contexts.
async fn appointment_group_groups(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let group = crate::fetch_appointment_group(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Deleted groups stay visible to admins only (same stance as the rest of
    // the Scheduler surface).
    if group.workflow_state == "deleted" && !crate::is_admin(&auth) {
        return Err(ApiError::NotFound);
    }
    if !crate::can_read_appointment_group(&state.db, &auth, id).await? {
        return Err(ApiError::Forbidden);
    }

    let groups: Vec<EligibleGroup> = sqlx::query_as(
        r#"
        SELECT g.id, g.name, g.group_category_id, g.workflow_state, g.created_at, g.updated_at
        FROM groups g
        JOIN appointment_group_sub_contexts s
          ON s.sub_context_type = 'GroupCategory' AND s.sub_context_id = g.group_category_id
        WHERE s.appointment_group_id = $1 AND g.workflow_state <> 'deleted'
        ORDER BY g.id ASC
        "#,
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({ "groups": groups })))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn calendar_gaps_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/calendar_events/{id}/reservations/{participant_id}",
            post(reserve_for_participant),
        )
        .route(
            "/calendar_events/{id}/participants",
            get(appointment_participants),
        )
        .route(
            "/courses/{courseId}/calendar_events/timetable",
            get(get_course_timetable).post(set_course_timetable),
        )
        .route(
            "/courses/{courseId}/calendar_events/timetable_events",
            post(set_course_timetable_events),
        )
        .route(
            "/appointment_groups/next_appointment",
            get(next_appointment),
        )
        .route(
            "/appointment_groups/{id}/groups",
            get(appointment_group_groups),
        )
}
