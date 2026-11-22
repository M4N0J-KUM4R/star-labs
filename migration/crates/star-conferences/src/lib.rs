//! star-conferences — web conferences and recordings (Canvas
//! ConferencesController) over course/group contexts, plus the
//! current-user listing and the recording_ready provider webhook
//! (issue #91, spec `specs/p5-rest-conferences.md`).
//!
//! Endpoints (spec §2, 7 routes):
//! - GET  /courses/{course_id}/conferences   — index (recordings preloaded)
//! - GET  /groups/{group_id}/conferences     — index (group twin)
//! - POST /courses/{course_id}/conferences   — create (invitees + calendar_event)
//! - POST /groups/{group_id}/conferences     — create (group twin)
//! - GET  /conferences                       — for_user (state=live filter)
//! - POST /courses/{course_id}/conferences/{conference_id}/recording_ready
//! - POST /groups/{group_id}/conferences/{conference_id}/recording_ready
//!
//! Behavior contract (spec §6 + canvas-lms ConferencesController):
//! - index: users with granular manage-course-content rights see all
//!   conferences of the context; everyone else sees only the ones they
//!   participate in. 401 anonymous → 404 unknown context → 403 non-member.
//!   Every item carries `recordings` (empty until the webhook fires) and
//!   the model's as_json field set (`users`/`invitees`/`attendees`
//!   arrays, `has_advanced_settings`, `long_running`, `user_settings`).
//! - create: nested `web_conference` params (Canvas
//!   `params.require(:web_conference).permit(:start_at, :end_at,
//!   :sync_attendees, :title, :duration, :description, :conference_type,
//!   user_settings: ..., lti_settings: ...)`); invitees via the `user`
//!   id map / `user[all]` / default all context members;
//!   `observers[remove]` invites context users minus observers; a truthy
//!   `web_conference[calendar_event]` creates a linked calendar_events
//!   row (Course contexts only — the Canvas helper returns nil for
//!   groups; and only with a start_at, CalendarEvent validates its
//!   presence). `conference_type` must be a configured provider type
//!   ("BigBlueButton" or "LtiConference"); unknown → 400 Rails error
//!   body. 200 with the conference JSON (Canvas renders with the
//!   default status).
//! - for_user: the caller's conferences across all Course/Group contexts
//!   where they are a participant, newest first; `state=live` keeps only
//!   conferences started within the last day and not yet ended
//!   (WebConference scope :live).
//! - recording_ready: NO auth user (Canvas skips load_user/require_user).
//!   Body carries `signed_parameters`, a JWT signed with the provider
//!   secret (star-lms: `AppState.jwt_secret`, the platform secret
//!   star-lti also verifies provider JWTs against — verified with the
//!   workspace `jsonwebtoken` crate, same pattern as
//!   star-lti::decode_message_hint). Valid JWT whose `meeting_id` equals
//!   the conference's `conference_key` → recording_ready=true + a
//!   recordings entry → 202 with `[]`; valid JWT, wrong meeting → 422
//!   with Canvas' `signed_id_invalid_json`; invalid JWT → 401 with
//!   Canvas' `invalid_jwt_token_json`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{NaiveDateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Conference-domain errors, mirroring the star-polling pattern:
/// plain-text bodies for the HTTP-status variants; `Invalid` carries a
/// Rails-style errors body (`render json: @conference.errors, status:
/// :bad_request` upstream); `JsonError` carries a fully-rendered Canvas
/// error body with its own status (recording_ready).
enum ConferenceError {
    NotFound,
    Forbidden,
    Invalid(Value),
    JsonError(StatusCode, Value),
    Db,
}

impl From<sqlx::Error> for ConferenceError {
    fn from(e: sqlx::Error) -> Self {
        eprintln!("database error: {e}");
        ConferenceError::Db
    }
}

impl IntoResponse for ConferenceError {
    fn into_response(self) -> Response {
        match self {
            ConferenceError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            ConferenceError::Forbidden => (StatusCode::FORBIDDEN, "forbidden").into_response(),
            ConferenceError::Invalid(body) => (StatusCode::BAD_REQUEST, Json(body)).into_response(),
            ConferenceError::JsonError(status, body) => (status, Json(body)).into_response(),
            ConferenceError::Db => {
                (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response()
            }
        }
    }
}

fn param_errors(attr: &str, message: &str) -> ConferenceError {
    ConferenceError::Invalid(json!({ attr: [message] }))
}

// ---------------------------------------------------------------------------
// Provider configuration (spec §4: no provider exists in the workspace;
// the stub pins Canvas's plugin types as configured so the permission and
// validation behavior stays observable)
// ---------------------------------------------------------------------------

/// The conference types Canvas would accept for this site:
/// WebConference.conference_types = plugin_types + lti_types — the
/// BigBlueButton plugin and the LTI conference_selection placement.
const CONFIGURED_TYPES: [&str; 2] = ["BigBlueButton", "LtiConference"];

// ---------------------------------------------------------------------------
// Row shapes (only the columns the API serializes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)] // user_id/created_at/recording_ready: SELECT * row shape kept whole
struct ConferenceRow {
    id: i64,
    title: String,
    conference_type: String,
    conference_key: Option<String>,
    context_id: i64,
    context_type: String,
    user_id: Option<i64>,
    started_at: Option<NaiveDateTime>,
    description: Option<String>,
    duration: Option<f64>,
    created_at: NaiveDateTime,
    ended_at: Option<NaiveDateTime>,
    start_at: Option<NaiveDateTime>,
    end_at: Option<NaiveDateTime>,
    settings: Option<String>,
    recording_ready: Option<bool>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct ParticipantRow {
    user_id: Option<i64>,
    participation_type: Option<String>,
}

/// ISO8601 (UTC) rendering of a naive snapshot timestamp column.
fn iso8601(t: NaiveDateTime) -> String {
    t.and_utc()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct PaginationParams {
    page: Option<i64>,
    per_page: Option<i64>,
    state: Option<String>,
}

fn pagination_values(params: &PaginationParams) -> (i64, i64, i64) {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).clamp(1, 100);
    let offset = page.saturating_sub(1).saturating_mul(per_page);
    (page, per_page, offset)
}

/// Canvas `value_to_boolean`: true, "true", "1", "on" (case-insensitive)
/// are truthy; everything else is falsy.
fn value_to_boolean(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::String(s) => matches!(s.to_lowercase().as_str(), "true" | "1" | "on"),
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        _ => false,
    }
}

/// Coerce a JSON value to an integer param (number or numeric string).
fn int_param(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Timestamp param: an RFC 3339 / ISO 8601 string (Rails binds
/// Time.zone.parse-able strings; star-lms accepts ISO8601).
fn time_param(value: Option<&Value>, attr: &str) -> Result<Option<NaiveDateTime>, ConferenceError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.trim().is_empty() => s
            .parse::<chrono::DateTime<Utc>>()
            .map(|t| Some(t.naive_utc()))
            .map_err(|_| param_errors(attr, "is invalid")),
        Some(Value::String(_)) => Ok(None),
        Some(_) => Err(param_errors(attr, "is invalid")),
    }
}

const MAX_TEXT_LEN: usize = 255;

fn check_text_len(s: &str, key: &str) -> Result<(), ConferenceError> {
    if s.chars().count() > MAX_TEXT_LEN {
        Err(param_errors(key, "is too long (maximum is 255 characters)"))
    } else {
        Ok(())
    }
}

/// `{"web_conference": [{...}]}` array-wrapped or `{"web_conference":
/// {...}}` / plain-object body (the API docs post nested hashes;
/// `params.require(:web_conference)` reads the nested hash).
fn unwrap_params<'a>(
    body: &'a Value,
    key: &str,
) -> Result<&'a Map<String, Value>, ConferenceError> {
    let missing = || {
        ConferenceError::Invalid(json!({
            "error": format!("param is missing or the value is empty: {key}")
        }))
    };
    match body.get(key) {
        Some(Value::Array(items)) => items.first().and_then(Value::as_object).ok_or_else(missing),
        Some(Value::Object(map)) => Ok(map),
        Some(_) => Err(missing()),
        None => body.as_object().ok_or_else(missing),
    }
}

// ---------------------------------------------------------------------------
// Context loading + permissions (spec §3 matrix)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Context {
    Course(i64),
    Group(i64),
}

impl Context {
    fn context_type(&self) -> &'static str {
        match self {
            Context::Course(_) => "Course",
            Context::Group(_) => "Group",
        }
    }

    fn context_id(&self) -> i64 {
        match self {
            Context::Course(id) | Context::Group(id) => *id,
        }
    }
}

/// Load the context; unknown (or soft-deleted) course/group is a 404
/// BEFORE any permission check (Canvas `require_context`).
async fn load_context(
    pool: &PgPool,
    course_id: Option<i64>,
    group_id: Option<i64>,
) -> Result<Context, ConferenceError> {
    if let Some(course_id) = course_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM courses \
             WHERE id = $1 AND COALESCE(workflow_state, 'available') <> 'deleted')",
        )
        .bind(course_id)
        .fetch_one(pool)
        .await?;
        if exists {
            return Ok(Context::Course(course_id));
        }
        return Err(ConferenceError::NotFound);
    }
    if let Some(group_id) = group_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM groups \
             WHERE id = $1 AND COALESCE(workflow_state, 'available') <> 'deleted')",
        )
        .bind(group_id)
        .fetch_one(pool)
        .await?;
        if exists {
            return Ok(Context::Group(group_id));
        }
    }
    Err(ConferenceError::NotFound)
}

/// Context `:read` ability (spec §3): any enrollment in the course /
/// accepted group membership; site admins bypass (Canvas grants admins
/// course `:read`; group admins read via allow_course_admin_actions).
async fn can_read_context(
    pool: &PgPool,
    auth: &AuthUser,
    context: &Context,
) -> Result<bool, ConferenceError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let ok =
        match context {
            Context::Course(course_id) => sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2)",
            )
            .bind(auth.id)
            .bind(course_id)
            .fetch_one(pool)
            .await?,
            Context::Group(group_id) => {
                sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM group_memberships \
                 WHERE user_id = $1 AND group_id = $2 AND workflow_state = 'accepted')",
                )
                .bind(auth.id)
                .bind(group_id)
                .fetch_one(pool)
                .await?
            }
        };
    Ok(ok)
}

/// The granular manage-course-content branch of index (Canvas
/// `@context.grants_any_right?(@current_user,
/// *GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS)`): course teachers/TAs
/// (admins bypass), or a group moderator (Group#has_moderator?). The
/// `:create` ability on `context.web_conferences.temp_record` covers the
/// same population (group members get manage_course_content_add from the
/// base policy, but create_conferences additionally requires the
/// moderator/teacher side).
async fn can_manage_content(
    pool: &PgPool,
    auth: &AuthUser,
    context: &Context,
) -> Result<bool, ConferenceError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let ok = match context {
        Context::Course(course_id) => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM enrollments \
                 WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta'))",
            )
            .bind(auth.id)
            .bind(course_id)
            .fetch_one(pool)
            .await?
        }
        Context::Group(group_id) => {
            sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM group_memberships \
                 WHERE user_id = $1 AND group_id = $2 AND workflow_state = 'accepted' \
                 AND COALESCE(moderator, false))",
            )
            .bind(auth.id)
            .bind(group_id)
            .fetch_one(pool)
            .await?
        }
    };
    Ok(ok)
}

/// The ids of users participating in the context (Canvas
/// `context.user_ids`): course enrollments / accepted group memberships.
async fn context_user_ids(pool: &PgPool, context: &Context) -> Result<Vec<i64>, ConferenceError> {
    let ids: Vec<(i64,)> =
        match context {
            Context::Course(course_id) => sqlx::query_as(
                "SELECT DISTINCT user_id FROM enrollments WHERE course_id = $1 ORDER BY user_id",
            )
            .bind(course_id)
            .fetch_all(pool)
            .await?,
            Context::Group(group_id) => {
                sqlx::query_as(
                    "SELECT user_id FROM group_memberships \
                 WHERE group_id = $1 AND workflow_state = 'accepted' ORDER BY user_id",
                )
                .bind(group_id)
                .fetch_all(pool)
                .await?
            }
        };
    Ok(ids.into_iter().map(|(id,)| id).collect())
}

// ---------------------------------------------------------------------------
// Serialization (WebConference#as_json + Api::V1::Conferences)
// ---------------------------------------------------------------------------

const CONFERENCE_COLUMNS: &str = "id, title, conference_type, conference_key, context_id, \
     context_type, user_id, started_at, description, duration, created_at, ended_at, \
     start_at, end_at, settings, recording_ready";

/// Conference JSON. Field set from Canvas WebConference#as_json (only: +
/// methods:) filtered through api_conference_json: `duration` is coerced
/// to an integer, `user_ids` becomes `users`, `invitees_ids` →
/// `invitees`, `attendees_ids` → `attendees`, plus the computed
/// `has_advanced_settings`, `long_running` (duration nil),
/// `user_settings` (the type's settings subset), `recordings` (from the
/// 0229 recordings column, [] until the webhook) and `url` (the provider
/// join URL; null in star-lms).
async fn conference_json(
    pool: &PgPool,
    row: &ConferenceRow,
    include_recordings: bool,
) -> Result<Value, ConferenceError> {
    let participants: Vec<ParticipantRow> = sqlx::query_as(
        "SELECT user_id, participation_type FROM web_conference_participants \
         WHERE web_conference_id = $1 ORDER BY id ASC",
    )
    .bind(row.id)
    .fetch_all(pool)
    .await?;
    let users: Vec<i64> = participants.iter().filter_map(|p| p.user_id).collect();
    let by_type = |t: &str| -> Vec<i64> {
        participants
            .iter()
            .filter(|p| p.participation_type.as_deref() == Some(t))
            .filter_map(|p| p.user_id)
            .collect()
    };
    let invitees = by_type("invitee");
    let attendees = by_type("attendee");

    let recordings: Value = if include_recordings {
        recording_payload(pool, row.id)
            .await?
            .unwrap_or_else(|| json!([]))
    } else {
        json!([])
    };

    Ok(json!({
        "id": row.id,
        "title": row.title,
        "description": row.description,
        "conference_type": row.conference_type,
        "conference_key": row.conference_key,
        "context_type": row.context_type,
        "context_id": row.context_id,
        "duration": row.duration.map(|d| d as i64),
        "started_at": row.started_at.map(iso8601),
        "ended_at": row.ended_at.map(iso8601),
        "start_at": row.start_at.map(iso8601),
        "end_at": row.end_at.map(iso8601),
        "users": users,
        "invitees": invitees,
        "attendees": attendees,
        "has_advanced_settings": false,
        "long_running": row.duration.is_none(),
        "user_settings": user_settings_of(&row.settings),
        "recordings": recordings,
        "url": Value::Null,
    }))
}

/// The recordings JSON stored by the webhook (0229 column, read through
/// a ::TEXT cast so the plain sqlx text protocol is enough).
async fn recording_payload(
    pool: &PgPool,
    conference_id: i64,
) -> Result<Option<Value>, ConferenceError> {
    let (raw,): (Option<String>,) =
        sqlx::query_as("SELECT recordings::TEXT FROM web_conferences WHERE id = $1")
            .bind(conference_id)
            .fetch_one(pool)
            .await?;
    Ok(raw.and_then(|s| serde_json::from_str::<Value>(&s).ok()))
}

/// `user_settings` (WebConference#user_settings): the subset of the
/// serialized `settings` hash named by the type's user_setting_fields
/// (BigBlueButtonConference declares record, scheduled_date,
/// create_time, share_webcam, share_microphone, send_public_chat,
/// send_private_chat, share_other_webcams, enable_waiting_room). Canvas
/// renders `{}` when the settings column is empty; present values are
/// surfaced.
fn user_settings_of(settings: &Option<String>) -> Value {
    let Some(raw) = settings else {
        return json!({});
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(raw) else {
        return json!({});
    };
    const FIELDS: [&str; 9] = [
        "record",
        "scheduled_date",
        "create_time",
        "share_webcam",
        "share_microphone",
        "send_public_chat",
        "send_private_chat",
        "share_other_webcams",
        "enable_waiting_room",
    ];
    let mut out = Map::new();
    for field in FIELDS {
        if let Some(v) = map.get(field) {
            out.insert(field.to_string(), v.clone());
        }
    }
    Value::Object(out)
}

/// `{"conferences": [...]}` — Canvas renders a plain root-wrapped array
/// (`api_conferences_json`), no meta envelope.
fn conferences_body(items: Vec<Value>) -> Response {
    (StatusCode::OK, Json(json!({ "conferences": items }))).into_response()
}

// ---------------------------------------------------------------------------
// Guard chain for per-record actions
// ---------------------------------------------------------------------------

/// `get_conference`: find the conference scoped to the path context
/// (`@context.web_conferences.find`) — an id from another context is a
/// 404, never a 403.
async fn conference_in_context(
    pool: &PgPool,
    context: &Context,
    conference_id: i64,
) -> Result<ConferenceRow, ConferenceError> {
    sqlx::query_as::<_, ConferenceRow>(&format!(
        "SELECT {CONFERENCE_COLUMNS} FROM web_conferences \
         WHERE id = $1 AND context_id = $2 AND context_type = $3"
    ))
    .bind(conference_id)
    .bind(context.context_id())
    .bind(context.context_type())
    .fetch_optional(pool)
    .await?
    .ok_or(ConferenceError::NotFound)
}

// ---------------------------------------------------------------------------
// Handlers — index / for_user
// ---------------------------------------------------------------------------

/// GET /courses/{course_id}/conferences — spec §3: 401 (extractor) → 404
/// unknown context → 403 non-reader → 200 with the paginated list,
/// recordings preloaded. Managers see all conferences; everyone else
/// only their own participations.
async fn index(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<PaginationParams>,
    auth: AuthUser,
) -> Result<Response, ConferenceError> {
    index_inner(&state, Some(course_id), None, &params, &auth).await
}

/// GET /groups/{group_id}/conferences — the group twin.
async fn group_index(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(params): Query<PaginationParams>,
    auth: AuthUser,
) -> Result<Response, ConferenceError> {
    index_inner(&state, None, Some(group_id), &params, &auth).await
}

async fn index_inner(
    state: &AppState,
    course_id: Option<i64>,
    group_id: Option<i64>,
    params: &PaginationParams,
    auth: &AuthUser,
) -> Result<Response, ConferenceError> {
    let context = load_context(&state.db, course_id, group_id).await?;
    if !can_read_context(&state.db, auth, &context).await? {
        return Err(ConferenceError::Forbidden);
    }
    let (_, per_page, offset) = pagination_values(params);

    let manager = can_manage_content(&state.db, auth, &context).await?;
    let rows: Vec<ConferenceRow> = if manager {
        sqlx::query_as::<_, ConferenceRow>(&format!(
            "SELECT {CONFERENCE_COLUMNS} FROM web_conferences \
             WHERE context_id = $1 AND context_type = $2 \
             ORDER BY created_at DESC, id DESC LIMIT $3 OFFSET $4"
        ))
        .bind(context.context_id())
        .bind(context.context_type())
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as::<_, ConferenceRow>(&format!(
            "SELECT {CONFERENCE_COLUMNS} FROM web_conferences w \
             WHERE context_id = $1 AND context_type = $2 \
             AND EXISTS (SELECT 1 FROM web_conference_participants p \
              WHERE p.web_conference_id = w.id AND p.user_id = $3) \
             ORDER BY created_at DESC, id DESC LIMIT $4 OFFSET $5"
        ))
        .bind(context.context_id())
        .bind(context.context_type())
        .bind(auth.id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?
    };

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(conference_json(&state.db, row, true).await?);
    }
    Ok(conferences_body(items))
}

/// GET /conferences — for_user: the caller's conferences across all
/// Course/Group contexts where they have a web_conference_participants
/// row (create adds the initiator as one). `state=live` keeps only
/// conferences started within the last day and not yet ended
/// (WebConference scope :live).
async fn for_user(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
    auth: AuthUser,
) -> Result<Response, ConferenceError> {
    let (_, per_page, offset) = pagination_values(&params);
    let live = params.state.as_deref() == Some("live");

    let columns = CONFERENCE_COLUMNS
        .split(", ")
        .map(|c| format!("w.{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let live_filter = if live {
        "AND w.started_at BETWEEN (NOW() - INTERVAL '1 day') AND NOW() \
         AND (w.ended_at IS NULL OR w.ended_at > NOW())"
    } else {
        ""
    };
    let rows: Vec<ConferenceRow> = sqlx::query_as(&format!(
        "SELECT {columns} FROM web_conferences w \
         WHERE EXISTS (SELECT 1 FROM web_conference_participants p \
          WHERE p.web_conference_id = w.id AND p.user_id = $1) \
         AND w.context_type IN ('Course', 'Group') {live_filter} \
         ORDER BY w.created_at DESC, w.id DESC LIMIT $2 OFFSET $3"
    ))
    .bind(auth.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(conference_json(&state.db, row, true).await?);
    }
    Ok(conferences_body(items))
}

// ---------------------------------------------------------------------------
// Handlers — create
// ---------------------------------------------------------------------------

/// POST /courses/{course_id}/conferences — spec §3: 401 → 404 unknown
/// context → 403 no :create ability → 400 validation → 200 with the
/// conference JSON.
async fn create(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ConferenceError> {
    create_inner(&state, Some(course_id), None, &auth, &body).await
}

/// POST /groups/{group_id}/conferences — the group twin.
async fn group_create(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ConferenceError> {
    create_inner(&state, None, Some(group_id), &auth, &body).await
}

async fn create_inner(
    state: &AppState,
    course_id: Option<i64>,
    group_id: Option<i64>,
    auth: &AuthUser,
    body: &Value,
) -> Result<Response, ConferenceError> {
    let context = load_context(&state.db, course_id, group_id).await?;
    if !can_manage_content(&state.db, auth, &context).await? {
        return Err(ConferenceError::Forbidden);
    }
    let obj = unwrap_params(body, "web_conference")?;

    // conference_params permit list: start_at, end_at, sync_attendees,
    // title, duration, description, conference_type (+ free-form
    // user_settings/lti_settings folded into the serialized settings).
    let conference_type = obj
        .get("conference_type")
        .and_then(Value::as_str)
        .unwrap_or("BigBlueButton")
        .to_string();
    if !CONFIGURED_TYPES.contains(&conference_type.as_str()) {
        return Err(param_errors(
            "conference_type",
            "is not included in the list",
        ));
    }

    // infer_conference_details: blank titles default per context type.
    let title = match obj.get("title") {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            check_text_len(s, "title")?;
            s.clone()
        }
        _ => match context {
            Context::Course(_) => "Course Web Conference".to_string(),
            Context::Group(_) => "Group Web Conference".to_string(),
        },
    };

    let description = match obj.get("description") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(param_errors("description", "is invalid")),
    };

    // validates :duration, numericality: { <= MAX_DURATION, allow_nil }
    let duration = match obj.get("duration") {
        None | Some(Value::Null) => None,
        Some(v) => match int_param(Some(v)) {
            Some(d) if d <= 99_999_999 => Some(d as f64),
            Some(_) => {
                return Err(param_errors(
                    "duration",
                    "must be less than or equal to 99999999",
                ))
            }
            None => return Err(param_errors("duration", "is not a number")),
        },
    };

    let start_at = time_param(obj.get("start_at"), "start_at")?;
    let end_at = time_param(obj.get("end_at"), "end_at")?;

    // `settings` (serialized text column): default_return_url plus the
    // free-form user_settings/lti_settings merges (merge_user_settings /
    // lti_settings= fold them into settings).
    let mut settings = Map::new();
    settings.insert("default_return_url".to_string(), json!(null));
    if let Some(v @ Value::Object(_)) = obj.get("lti_settings") {
        settings.insert("lti_settings".to_string(), v.clone());
    }
    if let Some(Value::Object(user_settings)) = obj.get("user_settings") {
        for (k, val) in user_settings {
            settings.insert(k.clone(), val.clone());
        }
    }
    if value_to_boolean(obj.get("sync_attendees").unwrap_or(&Value::Bool(false))) {
        settings.insert("sync_attendees".to_string(), json!(true));
    }
    let settings_text = Value::Object(settings).to_string();

    // infer_conference_details: context_code = "<type>_<id>".
    let context_code = format!(
        "{}_{}",
        context.context_type().to_lowercase(),
        context.context_id()
    );

    let row: ConferenceRow = sqlx::query_as(&format!(
        "INSERT INTO web_conferences \
         (title, conference_type, context_id, context_type, user_id, description, duration, \
          start_at, end_at, settings, context_code, type, root_account_id, \
          created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, 1, now(), now()) \
         RETURNING {CONFERENCE_COLUMNS}"
    ))
    .bind(&title)
    .bind(&conference_type)
    .bind(context.context_id())
    .bind(context.context_type())
    .bind(auth.id)
    .bind(&description)
    .bind(duration)
    .bind(start_at)
    .bind(end_at)
    .bind(&settings_text)
    .bind(&context_code)
    .bind("WebConference")
    .fetch_one(&state.db)
    .await?;

    // Invitees (member_ids → invite_users_from_context): user[] id map,
    // user[all], observers[remove] (context users minus observers) or the
    // whole context. add_initiator always adds the creator's initiator
    // participant row.
    let invitee_ids = member_ids(&state.db, &context, auth, body).await?;
    let mut tx = state.db.begin().await?;
    for uid in &invitee_ids {
        sqlx::query(
            "INSERT INTO web_conference_participants \
             (user_id, web_conference_id, participation_type, root_account_id) \
             VALUES ($1, $2, 'invitee', 1)",
        )
        .bind(uid)
        .bind(row.id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO web_conference_participants \
         (user_id, web_conference_id, participation_type, root_account_id) \
         VALUES ($1, $2, 'initiator', 1)",
    )
    .bind(auth.id)
    .bind(row.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    // Calendar event: only Course contexts and only when the extracted
    // `web_conference[calendar_event]` param is truthy AND the conference
    // has a start_at (create_or_update_calendar_event_for_conference
    // returns nil for groups; CalendarEvent validates start_at presence).
    let calendar_event_param = obj.get("calendar_event").cloned().unwrap_or_else(|| {
        body.get("calendar_event")
            .cloned()
            .unwrap_or(Value::Bool(false))
    });
    if let Context::Course(course_id) = context {
        if value_to_boolean(&calendar_event_param) && row.start_at.is_some() {
            sqlx::query(
                "INSERT INTO calendar_events \
                 (course_id, title, start_at, end_at, web_conference_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(course_id)
            .bind(&row.title)
            .bind(row.start_at.map(|t| t.and_utc()))
            .bind(row.end_at.map(|t| t.and_utc()))
            .bind(row.id)
            .execute(&state.db)
            .await?;
        }
    }

    let row = conference_in_context(&state.db, &context, row.id).await?;
    Ok(conferences_body(vec![
        conference_json(&state.db, &row, true).await?,
    ]))
}

/// Canvas `member_ids`:
/// - `observers[remove]` truthy → context users minus course observers;
/// - `user` present without a truthy `user[all]` → only the checked ids
///   (`user[id] = "1"`);
/// - otherwise (no `user` param, or `user[all]`) → the whole context.
/// The initiator row is added separately by add_initiator.
async fn member_ids(
    pool: &PgPool,
    context: &Context,
    auth: &AuthUser,
    body: &Value,
) -> Result<Vec<i64>, ConferenceError> {
    let remove_observers = body
        .get("observers")
        .and_then(|o| o.get("remove"))
        .map(value_to_boolean)
        .unwrap_or(false);

    if remove_observers {
        if let Context::Course(course_id) = context {
            let rows: Vec<(i64,)> = sqlx::query_as(
                "SELECT DISTINCT user_id FROM enrollments \
                 WHERE course_id = $1 AND COALESCE(role, 'student') <> 'observer' \
                 ORDER BY user_id",
            )
            .bind(course_id)
            .fetch_all(pool)
            .await?;
            return Ok(rows.into_iter().map(|(id,)| id).collect());
        }
        return context_user_ids(pool, context).await;
    }

    if let Some(Value::Object(map)) = body.get("user") {
        if !map.get("all").map(value_to_boolean).unwrap_or(false) {
            let mut ids: Vec<i64> = map
                .iter()
                .filter(|(k, v)| k.as_str() != "all" && value_to_boolean(v))
                .filter_map(|(k, _)| k.parse::<i64>().ok())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            return Ok(ids);
        }
    }

    let mut ids = context_user_ids(pool, context).await?;
    if !ids.contains(&auth.id) {
        ids.push(auth.id);
        ids.sort_unstable();
        ids.dedup();
    }
    Ok(ids)
}

// ---------------------------------------------------------------------------
// Handlers — recording_ready (JWT webhook, no auth user)
// ---------------------------------------------------------------------------

/// POST /courses/{course_id}/conferences/{conference_id}/recording_ready
/// and the group twin — Canvas skips load_user/require_user; the auth is
/// the JWT. Guard order: 404 unknown context/conference (require_context
/// + get_conference run before the action), then JWT verification.
async fn recording_ready(
    State(state): State<AppState>,
    Path((course_id, conference_id)): Path<(i64, i64)>,
    Json(body): Json<Value>,
) -> Result<Response, ConferenceError> {
    recording_ready_inner(&state, Some(course_id), None, conference_id, &body).await
}

async fn group_recording_ready(
    State(state): State<AppState>,
    Path((group_id, conference_id)): Path<(i64, i64)>,
    Json(body): Json<Value>,
) -> Result<Response, ConferenceError> {
    recording_ready_inner(&state, None, Some(group_id), conference_id, &body).await
}

async fn recording_ready_inner(
    state: &AppState,
    course_id: Option<i64>,
    group_id: Option<i64>,
    conference_id: i64,
    body: &Value,
) -> Result<Response, ConferenceError> {
    let context = load_context(&state.db, course_id, group_id).await?;
    let row = conference_in_context(&state.db, &context, conference_id).await?;

    // Canvas: decode_jwt(params[:signed_parameters], [config[:secret_dec]]).
    // star-lms provider secret: the platform JWT secret (same convention
    // as star-lti's provider JWT verification). Signature failure,
    // malformed token or an expired `exp` all reject (Canvas
    // CanvasSecurity::InvalidToken → 401).
    let token = body
        .get("signed_parameters")
        .and_then(Value::as_str)
        .unwrap_or("");
    let Some(claims) = decode_provider_jwt(token, &state.jwt_secret) else {
        // invalid_jwt_token_json (Api::V1::Conferences): 401.
        return Err(ConferenceError::JsonError(
            StatusCode::UNAUTHORIZED,
            json!({
                "status": "unauthorized",
                "errors": [{ "message": "JWT signature invalid" }]
            }),
        ));
    };

    let meeting_id = claims.get("meeting_id").and_then(|v| {
        v.as_str()
            .map(|s| s.to_string())
            .or_else(|| v.as_i64().map(|i| i.to_string()))
    });

    if meeting_id.is_none() || meeting_id.as_deref() != row.conference_key.as_deref() {
        // signed_id_invalid_json: 422 (unprocessable_content).
        return Err(ConferenceError::JsonError(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({
                "status": "unprocessable entity",
                "errors": [{ "message": "Signed meeting id invalid" }]
            }),
        ));
    }

    // recording_ready! → recording_ready = true; the provider's recording
    // metadata is stored in the 0229 recordings column with the
    // ConferenceRecording API shape (duration_minutes / title /
    // created_at / updated_at / playback_url) so index/for_user serve it.
    let now = Utc::now();
    let recording = json!({
        "duration_minutes": 0,
        "title": row.title,
        "created_at": now,
        "updated_at": now,
        "playback_url": Value::Null,
    });
    let mut recordings = recording_payload(&state.db, row.id)
        .await?
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    recordings.push(recording);
    sqlx::query(
        "UPDATE web_conferences SET recording_ready = true, recordings = $1::JSONB \
         WHERE id = $2",
    )
    .bind(Value::Array(recordings).to_string())
    .bind(row.id)
    .execute(&state.db)
    .await?;

    // Canvas: render json: [], status: :accepted.
    Ok((StatusCode::ACCEPTED, Json(json!([]))).into_response())
}

// ---------------------------------------------------------------------------
// Provider JWT verification (workspace jsonwebtoken, the star-lti
// pattern for no-user provider JWTs — see decode_message_hint)
// ---------------------------------------------------------------------------

fn decode_provider_jwt(token: &str, secret: &str) -> Option<Map<String, Value>> {
    if token.is_empty() {
        return None;
    }
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_aud = false;
    validation.required_spec_claims.clear();
    let data = jsonwebtoken::decode::<Value>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .ok()?;
    data.claims.as_object().cloned()
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{course_id}/conferences", get(index).post(create))
        .route(
            "/groups/{group_id}/conferences",
            get(group_index).post(group_create),
        )
        .route("/conferences", get(for_user))
        .route(
            "/courses/{course_id}/conferences/{conference_id}/recording_ready",
            post(recording_ready),
        )
        .route(
            "/groups/{group_id}/conferences/{conference_id}/recording_ready",
            post(group_recording_ready),
        )
}
