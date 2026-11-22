//! star-polling — polls, poll choices, poll sessions and poll submissions
//! (Canvas Polling::PollsController, PollChoicesController,
//! PollSessionsController, PollSubmissionsApiController) — the in-class
//! "clicker" feature (issue #89, spec `specs/p5-rest-polling.md`).
//!
//! Endpoints (spec §2, 21 routes):
//! - GET/POST  /polls                                        — polls index/create
//! - GET/PUT/DELETE /polls/{id}                              — poll show/update/destroy
//! - GET/POST  /polls/{poll_id}/poll_choices                 — choices index/create
//! - GET/PUT/DELETE /polls/{poll_id}/poll_choices/{id}       — choice show/update/destroy
//! - GET/POST  /polls/{poll_id}/poll_sessions                — sessions index/create
//! - GET/PUT/DELETE /polls/{poll_id}/poll_sessions/{id}      — session show/update/destroy
//! - GET /polls/{poll_id}/poll_sessions/{id}/open            — open (GET mutates, Canvas parity)
//! - GET /polls/{poll_id}/poll_sessions/{id}/close           — close (GET mutates, Canvas parity)
//! - GET /poll_sessions/opened, /poll_sessions/closed        — global state-filtered lists
//! - GET  /polls/{poll_id}/poll_sessions/{sid}/poll_submissions/{id} — submission show
//! - POST /polls/{poll_id}/poll_sessions/{sid}/poll_submissions      — submission create
//!
//! Behavior contract (spec §6):
//! - create/update bodies are array-wrapped (`{"polls": [{...}]}`) with a
//!   plain object tolerated; responses wrap every payload in the plural root
//!   (`{"polls": [...]}`) plus `meta.pagination`, including single records.
//! - polls#update ignores `is_correct` entirely (upstream deletes a blank
//!   `is_correct` from the params; polls carry no such field).
//! - poll_choices#update keeps the stored `is_correct` when the incoming
//!   value is blank/absent; create coerces blank to false.
//! - poll_sessions store the state as the upstream `is_published` boolean
//!   (`publish!`/`close!`); new sessions start unpublished ("closed").
//! - poll_submissions#create rejects a second submission by the same user
//!   with 400 (upstream uniqueness validation on (poll_session_id, user_id)).
//! - Permission order (spec §3): 401 anonymous; 404 missing/out-of-scope
//!   ids (scoped lookups); 403 failed ability; 400 validation.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Polling-domain errors. `Invalid` carries a Rails-style errors body
/// (`{"<attribute>": ["message"]}`, the shape `render json: @record.errors`
/// produces upstream); the plain variants map to the same status codes as
/// `star_common::ApiError`.
enum PollError {
    NotFound,
    Forbidden,
    Invalid(Value),
    Db,
}

impl From<sqlx::Error> for PollError {
    fn from(e: sqlx::Error) -> Self {
        eprintln!("database error: {e}");
        PollError::Db
    }
}

impl IntoResponse for PollError {
    fn into_response(self) -> Response {
        match self {
            PollError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            PollError::Forbidden => (StatusCode::FORBIDDEN, "forbidden").into_response(),
            PollError::Invalid(body) => (StatusCode::BAD_REQUEST, Json(body)).into_response(),
            PollError::Db => (StatusCode::INTERNAL_SERVER_ERROR, "database error").into_response(),
        }
    }
}

fn param_errors(attr: &str, message: &str) -> PollError {
    PollError::Invalid(json!({ attr: [message] }))
}

// ---------------------------------------------------------------------------
// Row shapes (only the columns the API serializes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct PollRow {
    id: i64,
    user_id: i64,
    question: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct PollChoiceRow {
    id: i64,
    poll_id: i64,
    text: String,
    is_correct: bool,
    position: Option<i32>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct PollSessionRow {
    id: i64,
    poll_id: i64,
    course_id: Option<i64>,
    course_section_id: Option<i64>,
    has_public_results: bool,
    is_published: bool,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct PollSubmissionRow {
    id: i64,
    poll_session_id: i64,
    poll_choice_id: i64,
    user_id: i64,
    created_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// Pagination query params (Canvas `Api.paginate`): 1-based page, per_page
/// defaulting to 50 and clamped to [1, 100].
#[derive(Debug, Default, Deserialize)]
struct PaginationParams {
    page: Option<i64>,
    per_page: Option<i64>,
}

fn pagination_values(params: &PaginationParams) -> (i64, i64, i64) {
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(50).clamp(1, 100);
    // Saturating arithmetic: an extreme `page` must not overflow the offset.
    let offset = page.saturating_sub(1).saturating_mul(per_page);
    (page, per_page, offset)
}

/// Unwrap Canvas' array-wrapped params (`params.require(:polls)[0]`):
/// `{"polls": [{...}]}` takes the first element, `{"polls": {...}}` and a
/// plain `{...}` body are tolerated (spec §6).
fn unwrap_params<'a>(body: &'a Value, key: &str) -> Result<&'a Map<String, Value>, PollError> {
    let missing = || {
        PollError::Invalid(json!({
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

/// Coerce a JSON value to a Canvas boolean param. Booleans and the strings
/// "true"/"false" (the API-doc example format) pass; absent, null or garbage
/// values are "blank" → None.
fn bool_param(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::String(s)) => match s.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Coerce a JSON value to an integer param (number or numeric string);
/// absent, null, fractional or garbage values are "blank" → None.
fn int_param(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

const MAX_TEXT_LEN: usize = 255;

/// Required non-blank text field with Canvas' 255-character limit
/// (`validates :question, presence: true, length: { maximum: 255 }`).
fn required_text(obj: &Map<String, Value>, key: &str) -> Result<String, PollError> {
    match obj.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            check_text_len(s, key)?;
            Ok(s.clone())
        }
        _ => Err(param_errors(key, "can't be blank")),
    }
}

/// Optional text field: absent/null → None (stored NULL); any string
/// (including blank) is stored as-is.
fn optional_text(obj: &Map<String, Value>, key: &str) -> Result<Option<String>, PollError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => {
            check_text_len(s, key)?;
            Ok(Some(s.clone()))
        }
        Some(_) => Err(param_errors(key, "is invalid")),
    }
}

fn check_text_len(s: &str, key: &str) -> Result<(), PollError> {
    if s.chars().count() > MAX_TEXT_LEN {
        Err(param_errors(key, "is too long (maximum is 255 characters)"))
    } else {
        Ok(())
    }
}

/// Optional `position` field: absent/null → NULL, integer otherwise.
fn position_param(obj: &Map<String, Value>) -> Result<Option<i32>, PollError> {
    match int_param(obj.get("position")) {
        None => Ok(None),
        Some(v) => i32::try_from(v)
            .map(Some)
            .map_err(|_| param_errors("position", "is out of range")),
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// `{"<root>": [...], "meta": {"pagination": {...}}}` — the
/// `Canvas::APIArraySerializer` envelope (spec §6), used for collections and
/// single records alike (`Array.wrap` upstream).
fn envelope(root: &str, items: Vec<Value>, page: i64, per_page: i64, total: i64) -> Response {
    let mut body = Map::new();
    body.insert(root.to_string(), Value::Array(items));
    body.insert(
        "meta".to_string(),
        json!({"pagination": {"page": page, "per_page": per_page, "total": total}}),
    );
    (StatusCode::OK, Json(Value::Object(body))).into_response()
}

fn single_envelope(root: &str, item: Value) -> Response {
    envelope(root, vec![item], 1, 1, 1)
}

fn choice_json(row: &PollChoiceRow) -> Value {
    json!({
        "id": row.id,
        "poll_id": row.poll_id,
        "text": row.text,
        "is_correct": row.is_correct,
        "position": row.position,
    })
}

fn submission_json(row: &PollSubmissionRow) -> Value {
    json!({
        "id": row.id,
        "poll_session_id": row.poll_session_id,
        "poll_choice_id": row.poll_choice_id,
        "user_id": row.user_id,
        "created_at": row.created_at,
    })
}

/// Poll JSON with `total_results` (submission counts keyed by poll choice id
/// across all sessions of the poll — upstream `Poll#total_results`).
async fn poll_json(pool: &PgPool, row: &PollRow) -> Result<Value, PollError> {
    let counts: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT poll_choice_id, COUNT(*) FROM poll_submissions \
         WHERE poll_id = $1 GROUP BY poll_choice_id",
    )
    .bind(row.id)
    .fetch_all(pool)
    .await?;
    let mut total_results = Map::new();
    for (choice_id, count) in counts {
        total_results.insert(choice_id.to_string(), json!(count));
    }
    Ok(json!({
        "id": row.id,
        "question": row.question,
        "description": row.description,
        "created_at": row.created_at,
        "user_id": row.user_id,
        "total_results": Value::Object(total_results),
    }))
}

/// Session JSON: results (per-choice submission counts for THIS session),
/// the embedded submissions and `has_submitted` for the current user —
/// upstream `PollSessionSerializer`. Star-lms only exposes sessions to the
/// poll's owner (spec §3), which is the serializer's `can_view_results?`
/// branch, so all submissions are embedded.
async fn session_json(
    pool: &PgPool,
    row: &PollSessionRow,
    current_user_id: i64,
) -> Result<Value, PollError> {
    let submissions: Vec<PollSubmissionRow> = sqlx::query_as(
        "SELECT id, poll_session_id, poll_choice_id, user_id, created_at \
         FROM poll_submissions WHERE poll_session_id = $1 ORDER BY id ASC",
    )
    .bind(row.id)
    .fetch_all(pool)
    .await?;
    let mut results = Map::new();
    for submission in &submissions {
        let key = submission.poll_choice_id.to_string();
        let count = results.get(&key).and_then(Value::as_i64).unwrap_or(0) + 1;
        results.insert(key, json!(count));
    }
    Ok(json!({
        "id": row.id,
        "poll_id": row.poll_id,
        "course_id": row.course_id,
        "course_section_id": row.course_section_id,
        "is_published": row.is_published,
        "has_public_results": row.has_public_results,
        "created_at": row.created_at,
        "results": Value::Object(results),
        "poll_submissions": submissions.iter().map(submission_json).collect::<Vec<_>>(),
        "has_submitted": submissions.iter().any(|s| s.user_id == current_user_id),
    }))
}

// ---------------------------------------------------------------------------
// Scoped lookups (the guard chain, spec §2)
// ---------------------------------------------------------------------------

const POLL_COLUMNS: &str = "id, user_id, question, description, created_at";
const SESSION_COLUMNS: &str =
    "id, poll_id, course_id, course_section_id, has_public_results, is_published, created_at";

/// Poll lookup scoped to the caller's own polls — spec §3: out-of-scope ids
/// are 404s (mirroring Canvas' `@current_user.polls` scoped lookups), never
/// 403s.
async fn own_poll(pool: &PgPool, poll_id: i64, auth: &AuthUser) -> Result<PollRow, PollError> {
    sqlx::query_as::<_, PollRow>(&format!(
        "SELECT {POLL_COLUMNS} FROM polls WHERE id = $1 AND user_id = $2"
    ))
    .bind(poll_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?
    .ok_or(PollError::NotFound)
}

/// Unscoped poll lookup (existence only) — used by poll_submissions, where
/// the submitting student is not the poll's owner.
async fn require_poll(pool: &PgPool, poll_id: i64) -> Result<PollRow, PollError> {
    sqlx::query_as::<_, PollRow>(&format!("SELECT {POLL_COLUMNS} FROM polls WHERE id = $1"))
        .bind(poll_id)
        .fetch_optional(pool)
        .await?
        .ok_or(PollError::NotFound)
}

/// Session lookup scoped to the poll from the path (`require_poll_session`).
async fn poll_session(
    pool: &PgPool,
    poll_id: i64,
    session_id: i64,
) -> Result<PollSessionRow, PollError> {
    sqlx::query_as::<_, PollSessionRow>(&format!(
        "SELECT {SESSION_COLUMNS} FROM poll_sessions WHERE id = $1 AND poll_id = $2"
    ))
    .bind(session_id)
    .bind(poll_id)
    .fetch_optional(pool)
    .await?
    .ok_or(PollError::NotFound)
}

/// Choice lookup scoped to the poll from the path (`@poll.poll_choices.find`).
async fn poll_choice(
    pool: &PgPool,
    poll_id: i64,
    choice_id: i64,
) -> Result<PollChoiceRow, PollError> {
    sqlx::query_as::<_, PollChoiceRow>(
        "SELECT id, poll_id, text, is_correct, position FROM poll_choices \
         WHERE id = $1 AND poll_id = $2",
    )
    .bind(choice_id)
    .bind(poll_id)
    .fetch_optional(pool)
    .await?
    .ok_or(PollError::NotFound)
}

/// The `:submit` ability (upstream `PollSession#visible_to?`): the caller
/// needs an enrollment in the session's course; a course-wide enrollment
/// covers section-bound sessions, a section enrollment covers course-wide
/// sessions. Site admins bypass (Canvas grants admins course `:read`).
async fn can_submit(
    pool: &PgPool,
    auth: &AuthUser,
    session: &PollSessionRow,
) -> Result<bool, PollError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let Some(course_id) = session.course_id else {
        return Ok(false);
    };
    let enrolled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 \
         AND ($3::BIGINT IS NULL OR course_section_id IS NULL OR course_section_id = $3))",
    )
    .bind(auth.id)
    .bind(course_id)
    .bind(session.course_section_id)
    .fetch_one(pool)
    .await?;
    Ok(enrolled)
}

// ---------------------------------------------------------------------------
// Handlers — polls
// ---------------------------------------------------------------------------

/// GET /polls — the caller's polls, newest first (spec §3 row 1).
async fn list_polls(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    let (page, per_page, offset) = pagination_values(&params);
    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM polls WHERE user_id = $1")
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;
    let rows: Vec<PollRow> = sqlx::query_as(&format!(
        "SELECT {POLL_COLUMNS} FROM polls WHERE user_id = $1 \
         ORDER BY created_at DESC, id DESC LIMIT $2 OFFSET $3"
    ))
    .bind(auth.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(poll_json(&state.db, row).await?);
    }
    Ok(envelope("polls", items, page, per_page, total))
}

/// GET /polls/{id} — a poll readable only by its creator (spec §3 row 1:
/// 404 via the scoped query, not 403).
async fn show_poll(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    let row = own_poll(&state.db, id, &auth).await?;
    Ok(single_envelope("polls", poll_json(&state.db, &row).await?))
}

/// POST /polls — create a poll for the caller. Canvas renders the serialized
/// record with the default status (200, not 201).
async fn create_poll(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, PollError> {
    let obj = unwrap_params(&body, "polls")?;
    let question = required_text(obj, "question")?;
    let description = optional_text(obj, "description")?;
    let row: PollRow = sqlx::query_as(&format!(
        "INSERT INTO polls (user_id, question, description) VALUES ($1, $2, $3) \
         RETURNING {POLL_COLUMNS}"
    ))
    .bind(auth.id)
    .bind(&question)
    .bind(&description)
    .fetch_one(&state.db)
    .await?;
    Ok(single_envelope("polls", poll_json(&state.db, &row).await?))
}

/// PUT /polls/{id} — update an own poll. `is_correct` is never honored on
/// this endpoint: upstream deletes a blank `is_correct` from the params and
/// polls carry no such column (spec §6 quirk).
async fn update_poll(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, id, &auth).await?;
    let obj = unwrap_params(&body, "polls")?;
    let question = match obj.get("question") {
        None => poll.question,
        Some(_) => required_text(obj, "question")?,
    };
    let description = match obj.get("description") {
        None => poll.description,
        Some(_) => optional_text(obj, "description")?,
    };
    let row: PollRow = sqlx::query_as(&format!(
        "UPDATE polls SET question = $1, description = $2, updated_at = now() \
         WHERE id = $3 RETURNING {POLL_COLUMNS}"
    ))
    .bind(&question)
    .bind(&description)
    .bind(poll.id)
    .fetch_one(&state.db)
    .await?;
    Ok(single_envelope("polls", poll_json(&state.db, &row).await?))
}

/// DELETE /polls/{id} — hard delete; 204 No Content (upstream `head
/// :no_content`). Choices, sessions and submissions cascade (migration 0100,
/// mirroring Canvas' `dependent: :destroy`).
async fn destroy_poll(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, PollError> {
    let poll = own_poll(&state.db, id, &auth).await?;
    sqlx::query("DELETE FROM polls WHERE id = $1")
        .bind(poll.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Handlers — poll choices
// ---------------------------------------------------------------------------

/// GET /polls/{poll_id}/poll_choices — the poll's choices ordered by
/// `position` (upstream `has_many :poll_choices, -> { order(:position) }`).
async fn list_choices(
    State(state): State<AppState>,
    Path(poll_id): Path<i64>,
    Query(params): Query<PaginationParams>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let (page, per_page, offset) = pagination_values(&params);
    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM poll_choices WHERE poll_id = $1")
        .bind(poll.id)
        .fetch_one(&state.db)
        .await?;
    let rows: Vec<PollChoiceRow> = sqlx::query_as(
        "SELECT id, poll_id, text, is_correct, position FROM poll_choices \
         WHERE poll_id = $1 ORDER BY position ASC, id ASC LIMIT $2 OFFSET $3",
    )
    .bind(poll.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    Ok(envelope(
        "poll_choices",
        rows.iter().map(choice_json).collect(),
        page,
        per_page,
        total,
    ))
}

/// GET /polls/{poll_id}/poll_choices/{id} — a choice of an own poll; a choice
/// id from another poll is 404 (scoped `@poll.poll_choices.find`).
async fn show_choice(
    State(state): State<AppState>,
    Path((poll_id, choice_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let row = poll_choice(&state.db, poll.id, choice_id).await?;
    Ok(single_envelope("poll_choices", choice_json(&row)))
}

/// POST /polls/{poll_id}/poll_choices — a blank `is_correct` coerces to false
/// (upstream poll_choices#create).
async fn create_choice(
    State(state): State<AppState>,
    Path(poll_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let obj = unwrap_params(&body, "poll_choices")?;
    let text = required_text(obj, "text")?;
    let is_correct = bool_param(obj.get("is_correct")).unwrap_or(false);
    let position = position_param(obj)?;
    let row: PollChoiceRow = sqlx::query_as(
        "INSERT INTO poll_choices (poll_id, text, is_correct, position) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, poll_id, text, is_correct, position",
    )
    .bind(poll.id)
    .bind(&text)
    .bind(is_correct)
    .bind(position)
    .fetch_one(&state.db)
    .await?;
    Ok(single_envelope("poll_choices", choice_json(&row)))
}

/// PUT /polls/{poll_id}/poll_choices/{id} — a blank/absent `is_correct` keeps
/// the stored value (upstream poll_choices#update quirk).
async fn update_choice(
    State(state): State<AppState>,
    Path((poll_id, choice_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let choice = poll_choice(&state.db, poll.id, choice_id).await?;
    let obj = unwrap_params(&body, "poll_choices")?;
    let text = match obj.get("text") {
        None => choice.text,
        Some(_) => required_text(obj, "text")?,
    };
    let is_correct = bool_param(obj.get("is_correct")).unwrap_or(choice.is_correct);
    let position = match obj.get("position") {
        None | Some(Value::Null) => choice.position,
        Some(_) => position_param(obj)?,
    };
    let row: PollChoiceRow = sqlx::query_as(
        "UPDATE poll_choices SET text = $1, is_correct = $2, position = $3, updated_at = now() \
         WHERE id = $4 RETURNING id, poll_id, text, is_correct, position",
    )
    .bind(&text)
    .bind(is_correct)
    .bind(position)
    .bind(choice.id)
    .fetch_one(&state.db)
    .await?;
    Ok(single_envelope("poll_choices", choice_json(&row)))
}

/// DELETE /polls/{poll_id}/poll_choices/{id} — 204 No Content.
async fn destroy_choice(
    State(state): State<AppState>,
    Path((poll_id, choice_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let choice = poll_choice(&state.db, poll.id, choice_id).await?;
    sqlx::query("DELETE FROM poll_choices WHERE id = $1")
        .bind(choice.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Handlers — poll sessions
// ---------------------------------------------------------------------------

/// GET /polls/{poll_id}/poll_sessions — the poll's sessions (requires the
/// poll `:update` ability upstream → owner only in star-lms, spec §3).
async fn list_sessions(
    State(state): State<AppState>,
    Path(poll_id): Path<i64>,
    Query(params): Query<PaginationParams>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let (page, per_page, offset) = pagination_values(&params);
    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM poll_sessions WHERE poll_id = $1")
        .bind(poll.id)
        .fetch_one(&state.db)
        .await?;
    let rows: Vec<PollSessionRow> = sqlx::query_as(&format!(
        "SELECT {SESSION_COLUMNS} FROM poll_sessions WHERE poll_id = $1 \
         ORDER BY id ASC LIMIT $2 OFFSET $3"
    ))
    .bind(poll.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(session_json(&state.db, row, auth.id).await?);
    }
    Ok(envelope("poll_sessions", items, page, per_page, total))
}

/// GET /polls/{poll_id}/poll_sessions/{id} — a session of an own poll.
async fn show_session(
    State(state): State<AppState>,
    Path((poll_id, session_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let row = poll_session(&state.db, poll.id, session_id).await?;
    Ok(single_envelope(
        "poll_sessions",
        session_json(&state.db, &row, auth.id).await?,
    ))
}

/// POST /polls/{poll_id}/poll_sessions — bind a session to a course
/// (required) and optionally a section of that course. Unknown course or a
/// section outside the course is 404 (upstream `Course.find` /
/// `@course.course_sections.find`). A blank `has_public_results` is false.
async fn create_session(
    State(state): State<AppState>,
    Path(poll_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let obj = unwrap_params(&body, "poll_sessions")?;
    // Upstream raises RecordNotFound ("Course is required.") for a missing,
    // blank or unknown course id.
    let Some(course_id) = int_param(obj.get("course_id")) else {
        return Err(PollError::NotFound);
    };
    let course_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM courses WHERE id = $1)")
            .bind(course_id)
            .fetch_one(&state.db)
            .await?;
    if !course_exists {
        return Err(PollError::NotFound);
    }
    let course_section_id = match obj.get("course_section_id") {
        None | Some(Value::Null) => None,
        Some(_) => {
            let section_id = int_param(obj.get("course_section_id")).ok_or(PollError::NotFound)?;
            let in_course: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM course_sections WHERE id = $1 AND course_id = $2)",
            )
            .bind(section_id)
            .bind(course_id)
            .fetch_one(&state.db)
            .await?;
            if !in_course {
                return Err(PollError::NotFound);
            }
            Some(section_id)
        }
    };
    let has_public_results = bool_param(obj.get("has_public_results")).unwrap_or(false);
    let row: PollSessionRow = sqlx::query_as(&format!(
        "INSERT INTO poll_sessions \
         (poll_id, course_id, course_section_id, has_public_results, is_published) \
         VALUES ($1, $2, $3, $4, false) RETURNING {SESSION_COLUMNS}"
    ))
    .bind(poll.id)
    .bind(course_id)
    .bind(course_section_id)
    .bind(has_public_results)
    .fetch_one(&state.db)
    .await?;
    Ok(single_envelope(
        "poll_sessions",
        session_json(&state.db, &row, auth.id).await?,
    ))
}

/// PUT /polls/{poll_id}/poll_sessions/{id} — update course/section binding
/// and/or `has_public_results`. Permitted fields are written directly
/// (upstream `@poll_session.update(poll_session_params)`); a section that
/// does not belong to the effective course fails the upstream
/// `section_belongs_to_course` validation → 400.
async fn update_session(
    State(state): State<AppState>,
    Path((poll_id, session_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let session = poll_session(&state.db, poll.id, session_id).await?;
    let obj = unwrap_params(&body, "poll_sessions")?;
    let course_id = match obj.get("course_id") {
        None => session.course_id,
        Some(_) => {
            let course_id = int_param(obj.get("course_id")).ok_or(PollError::NotFound)?;
            let course_exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM courses WHERE id = $1)")
                    .bind(course_id)
                    .fetch_one(&state.db)
                    .await?;
            if !course_exists {
                return Err(PollError::NotFound);
            }
            Some(course_id)
        }
    };
    let course_section_id = match obj.get("course_section_id") {
        None => session.course_section_id,
        Some(Value::Null) => None,
        Some(_) => Some(int_param(obj.get("course_section_id")).ok_or(PollError::NotFound)?),
    };
    if let (Some(course_id), Some(section_id)) = (course_id, course_section_id) {
        let in_course: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM course_sections WHERE id = $1 AND course_id = $2)",
        )
        .bind(section_id)
        .bind(course_id)
        .fetch_one(&state.db)
        .await?;
        if !in_course {
            return Err(PollError::Invalid(json!({
                "base": ["That course section does not belong to the existing course."]
            })));
        }
    }
    let has_public_results = match obj.get("has_public_results") {
        None => session.has_public_results,
        Some(_) => bool_param(obj.get("has_public_results")).unwrap_or(false),
    };
    let row: PollSessionRow = sqlx::query_as(&format!(
        "UPDATE poll_sessions SET course_id = $1, course_section_id = $2, \
         has_public_results = $3, updated_at = now() WHERE id = $4 RETURNING {SESSION_COLUMNS}"
    ))
    .bind(course_id)
    .bind(course_section_id)
    .bind(has_public_results)
    .bind(session.id)
    .fetch_one(&state.db)
    .await?;
    Ok(single_envelope(
        "poll_sessions",
        session_json(&state.db, &row, auth.id).await?,
    ))
}

/// DELETE /polls/{poll_id}/poll_sessions/{id} — 204 No Content.
async fn destroy_session(
    State(state): State<AppState>,
    Path((poll_id, session_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, PollError> {
    let poll = own_poll(&state.db, poll_id, &auth).await?;
    let session = poll_session(&state.db, poll.id, session_id).await?;
    sqlx::query("DELETE FROM poll_sessions WHERE id = $1")
        .bind(session.id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /polls/{poll_id}/poll_sessions/{id}/open — a GET that mutates state
/// (Canvas route parity, spec §6): `publish!` sets `is_published = true`.
async fn open_session(
    State(state): State<AppState>,
    Path((poll_id, session_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    transition_session(&state, poll_id, session_id, &auth, true).await
}

/// GET /polls/{poll_id}/poll_sessions/{id}/close — `close!` sets
/// `is_published = false`.
async fn close_session(
    State(state): State<AppState>,
    Path((poll_id, session_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    transition_session(&state, poll_id, session_id, &auth, false).await
}

async fn transition_session(
    state: &AppState,
    poll_id: i64,
    session_id: i64,
    auth: &AuthUser,
    publish: bool,
) -> Result<Response, PollError> {
    let poll = own_poll(&state.db, poll_id, auth).await?;
    let session = poll_session(&state.db, poll.id, session_id).await?;
    let row: PollSessionRow = sqlx::query_as(&format!(
        "UPDATE poll_sessions SET is_published = $1, updated_at = now() WHERE id = $2 \
         RETURNING {SESSION_COLUMNS}"
    ))
    .bind(publish)
    .bind(session.id)
    .fetch_one(&state.db)
    .await?;
    Ok(single_envelope(
        "poll_sessions",
        session_json(&state.db, &row, auth.id).await?,
    ))
}

/// GET /poll_sessions/opened and /poll_sessions/closed — global,
/// state-filtered lists of the sessions of the current user's polls
/// (spec §6; Canvas' enrollment-scoped `available_for` is narrowed to poll
/// ownership in star-lms).
async fn sessions_by_state(
    state: &AppState,
    auth: &AuthUser,
    params: &PaginationParams,
    published: bool,
) -> Result<Response, PollError> {
    let (page, per_page, offset) = pagination_values(params);
    let (total,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM poll_sessions s JOIN polls p ON p.id = s.poll_id \
         WHERE p.user_id = $1 AND s.is_published = $2",
    )
    .bind(auth.id)
    .bind(published)
    .fetch_one(&state.db)
    .await?;
    let rows: Vec<PollSessionRow> = sqlx::query_as(
        "SELECT s.id, s.poll_id, s.course_id, s.course_section_id, s.has_public_results, \
         s.is_published, s.created_at FROM poll_sessions s \
         JOIN polls p ON p.id = s.poll_id \
         WHERE p.user_id = $1 AND s.is_published = $2 ORDER BY s.id ASC LIMIT $3 OFFSET $4",
    )
    .bind(auth.id)
    .bind(published)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;
    let mut items = Vec::with_capacity(rows.len());
    for row in &rows {
        items.push(session_json(&state.db, row, auth.id).await?);
    }
    Ok(envelope("poll_sessions", items, page, per_page, total))
}

async fn opened_sessions(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    sessions_by_state(&state, &auth, &params, true).await
}

async fn closed_sessions(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    sessions_by_state(&state, &auth, &params, false).await
}

// ---------------------------------------------------------------------------
// Handlers — poll submissions
// ---------------------------------------------------------------------------

/// GET /polls/{poll_id}/poll_sessions/{sid}/poll_submissions/{id} — readable
/// by the submitter or the poll owner (spec §3 row 4); anyone else gets 403.
async fn show_submission(
    State(state): State<AppState>,
    Path((poll_id, session_id, submission_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Response, PollError> {
    let poll = require_poll(&state.db, poll_id).await?;
    let session = poll_session(&state.db, poll.id, session_id).await?;
    let submission: Option<PollSubmissionRow> = sqlx::query_as(
        "SELECT id, poll_session_id, poll_choice_id, user_id, created_at \
         FROM poll_submissions WHERE id = $1 AND poll_session_id = $2",
    )
    .bind(submission_id)
    .bind(session.id)
    .fetch_optional(&state.db)
    .await?;
    let Some(submission) = submission else {
        return Err(PollError::NotFound);
    };
    if poll.user_id != auth.id && submission.user_id != auth.id {
        return Err(PollError::Forbidden);
    }
    Ok(single_envelope(
        "poll_submissions",
        submission_json(&submission),
    ))
}

/// POST /polls/{poll_id}/poll_sessions/{sid}/poll_submissions — submit one
/// choice. Guard order (upstream): unknown poll/session/choice → 404, then
/// the `:submit` ability (session open + enrolled) → 403, then save (a
/// duplicate submission fails the uniqueness validation) → 400.
async fn create_submission(
    State(state): State<AppState>,
    Path((poll_id, session_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, PollError> {
    let poll = require_poll(&state.db, poll_id).await?;
    let session = poll_session(&state.db, poll.id, session_id).await?;
    let obj = unwrap_params(&body, "poll_submissions")?;
    // Upstream `@poll.poll_choices.find(...)`: a missing, blank or unknown
    // poll_choice_id raises RecordNotFound before any ability check.
    let choice_id = int_param(obj.get("poll_choice_id")).ok_or(PollError::NotFound)?;
    let choice_in_poll: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM poll_choices WHERE id = $1 AND poll_id = $2)",
    )
    .bind(choice_id)
    .bind(poll.id)
    .fetch_one(&state.db)
    .await?;
    if !choice_in_poll {
        return Err(PollError::NotFound);
    }
    // `:submit` = session open + caller visible in the bound course.
    if !session.is_published {
        return Err(PollError::Forbidden);
    }
    if !can_submit(&state.db, &auth, &session).await? {
        return Err(PollError::Forbidden);
    }
    let inserted: Result<PollSubmissionRow, sqlx::Error> = sqlx::query_as(
        "INSERT INTO poll_submissions (poll_id, poll_session_id, poll_choice_id, user_id) \
         VALUES ($1, $2, $3, $4) \
         RETURNING id, poll_session_id, poll_choice_id, user_id, created_at",
    )
    .bind(poll.id)
    .bind(session.id)
    .bind(choice_id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await;
    let row = match inserted {
        Ok(row) => row,
        Err(ref e)
            if e.as_database_error()
                .is_some_and(|db| db.is_unique_violation()) =>
        {
            return Err(PollError::Invalid(json!({
                "user_id": ["can only submit one choice per poll session."]
            })));
        }
        Err(e) => return Err(e.into()),
    };
    Ok(single_envelope("poll_submissions", submission_json(&row)))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Domain router for star-polling. All session-id path params share the
/// `{poll_session_id}` name so the nested session/submission routes stay the
/// same shape in axum's router (a different param name at the same position
/// panics at startup).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/polls", get(list_polls).post(create_poll))
        .route(
            "/polls/{id}",
            get(show_poll).put(update_poll).delete(destroy_poll),
        )
        .route(
            "/polls/{poll_id}/poll_choices",
            get(list_choices).post(create_choice),
        )
        .route(
            "/polls/{poll_id}/poll_choices/{id}",
            get(show_choice).put(update_choice).delete(destroy_choice),
        )
        .route(
            "/polls/{poll_id}/poll_sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/polls/{poll_id}/poll_sessions/{poll_session_id}",
            get(show_session)
                .put(update_session)
                .delete(destroy_session),
        )
        .route(
            "/polls/{poll_id}/poll_sessions/{poll_session_id}/open",
            get(open_session),
        )
        .route(
            "/polls/{poll_id}/poll_sessions/{poll_session_id}/close",
            get(close_session),
        )
        .route(
            "/polls/{poll_id}/poll_sessions/{poll_session_id}/poll_submissions",
            post(create_submission),
        )
        .route(
            "/polls/{poll_id}/poll_sessions/{poll_session_id}/poll_submissions/{id}",
            get(show_submission),
        )
        .route("/poll_sessions/opened", get(opened_sessions))
        .route("/poll_sessions/closed", get(closed_sessions))
}
