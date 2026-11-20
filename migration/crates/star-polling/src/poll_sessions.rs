//! Canvas `Polling::PollSessionsController` — poll sessions REST port.
//!
//! Faithful port of
//! `canvas-src/app/controllers/polling/poll_sessions_controller.rb` (routes in
//! `canvas-src/config/routes.rb`, `scope(controller: "polling/poll_sessions")`)
//! together with the two models it drives (`Polling::Poll`,
//! `Polling::PollSession`) and the serializer
//! (`Polling::PollSessionSerializer`).
//!
//! Behaviour pinned by the Ruby:
//!
//! * every action answers `{"poll_sessions": [...]}` — single records are
//!   wrapped in a one element array (`serialize_jsonapi` uses `Array.wrap`);
//! * ids are strings (`StringifyIds`), booleans stay booleans;
//! * `results` and `has_public_results` are only rendered for a viewer that
//!   may see the results (`has_public_results` or the poll `:update` right);
//! * `poll_submissions` holds every submission for a teacher and only the
//!   caller's own submission for everybody else;
//! * a missing poll / session is a 404 *before* any permission check, a
//!   failed permission is a 403, a failed validation is a 400;
//! * `open` / `close` are GET routes that flip `is_published`.
//!
//! The `Accept: application/vnd.api+json` header adds the JSON:API `meta`
//! block (`pagination` + `primaryCollection`) to the collection responses,
//! exactly like `Api.jsonapi_paginate` does in the Ruby controller.

use axum::{
    extract::{OriginalUri, Path, Query, State},
    http::{header::ACCEPT, HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Domain router for the poll session routes.
///
/// The crate router (`src/lib.rs`) wires it with
/// `.merge(poll_sessions::router())`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/polls/{poll_id}/poll_sessions", get(index).post(create))
        .route(
            "/polls/{poll_id}/poll_sessions/{id}",
            get(show).put(update).delete(destroy),
        )
        .route("/polls/{poll_id}/poll_sessions/{id}/open", get(open))
        .route("/polls/{poll_id}/poll_sessions/{id}/close", get(close))
        .route("/poll_sessions/opened", get(opened))
        .route("/poll_sessions/closed", get(closed))
}

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// `Polling::Poll` — only the columns this port reads.
#[derive(Debug, Clone, sqlx::FromRow)]
struct Poll {
    id: i64,
    /// The poll's creator. The creator may update/delete the poll.
    user_id: i64,
}

/// `Polling::PollSession`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct PollSession {
    id: i64,
    poll_id: i64,
    course_id: i64,
    course_section_id: Option<i64>,
    has_public_results: bool,
    is_published: bool,
    created_at: DateTime<Utc>,
}

/// `Polling::PollSubmission`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct PollSubmission {
    id: i64,
    poll_session_id: i64,
    poll_choice_id: i64,
    user_id: i64,
    created_at: DateTime<Utc>,
}

/// Canvas `Api::PER_PAGE` / `Api::MAX_PER_PAGE`.
const DEFAULT_PER_PAGE: i64 = 10;
const MAX_PER_PAGE: i64 = 100;

// ---------------------------------------------------------------------------
// Request parameters
// ---------------------------------------------------------------------------

/// Canvas pagination parameters (`page`, `per_page`).
///
/// The values are kept as strings so that a junk value behaves like the Ruby
/// (`to_i` → 0 → clamped) instead of failing the whole query string.
#[derive(Debug, Default, Deserialize)]
struct Pagination {
    page: Option<String>,
    per_page: Option<String>,
}

impl Pagination {
    /// will_paginate pages start at 1.
    fn page(&self) -> i64 {
        self.page
            .as_deref()
            .and_then(|raw| raw.trim().parse::<i64>().ok())
            .filter(|page| *page > 0)
            .unwrap_or(1)
    }

    /// `Api.per_page_for`: default 10, clamped to 1..=100.
    fn per_page(&self) -> i64 {
        match self
            .per_page
            .as_deref()
            .and_then(|raw| raw.trim().parse::<i64>().ok())
        {
            Some(per_page) => per_page.clamp(1, MAX_PER_PAGE),
            None => DEFAULT_PER_PAGE,
        }
    }

    fn offset(&self) -> i64 {
        (self.page() - 1) * self.per_page()
    }
}

/// The permitted poll session attributes
/// (`params.require(:poll_sessions)[0].permit(...)`).
///
/// The outer `Option` records whether the attribute was sent at all, the
/// inner one whether it was sent as `null` — Canvas distinguishes "not
/// sent" from "sent as null" (a null `course_section_id` clears the section,
/// a missing one keeps it).
#[derive(Debug, Default)]
struct SessionParams {
    course_id: Option<Option<i64>>,
    course_section_id: Option<Option<i64>>,
    has_public_results: Option<Option<bool>>,
}

/// Pull the poll session attributes out of a request body.
///
/// Canvas wraps the entity in an array (`{"poll_sessions": [{...}]}`). A bare
/// object is accepted as well, as long as it carries one of the permitted
/// attributes; anything else is Canvas' `ActionController::ParameterMissing`
/// (400).
fn session_params(body: &Value) -> Result<SessionParams, ApiError> {
    let root = match body {
        Value::Object(map) => map,
        Value::Array(items) => match items.first() {
            Some(Value::Object(map)) => map,
            _ => return Err(missing_params()),
        },
        _ => return Err(missing_params()),
    };

    let inner = match root.get("poll_sessions") {
        Some(Value::Array(items)) => match items.first() {
            Some(Value::Object(map)) => map,
            _ => return Err(missing_params()),
        },
        Some(Value::Object(map)) => map,
        Some(Value::Null) => return Err(missing_params()),
        Some(_) => return Err(missing_params()),
        None => {
            // Canvas' `require` is strict, but the spec asks for the bare
            // object form as well: accept it when it carries a permitted key.
            let carries_permitted_key = ["course_id", "course_section_id", "has_public_results"]
                .iter()
                .any(|key| root.contains_key(*key));
            if !carries_permitted_key {
                return Err(missing_params());
            }
            root
        }
    };

    Ok(SessionParams {
        course_id: match inner.get("course_id") {
            Some(raw) => Some(parse_id(raw)?),
            None => None,
        },
        course_section_id: match inner.get("course_section_id") {
            Some(raw) => Some(parse_id(raw)?),
            None => None,
        },
        has_public_results: inner.get("has_public_results").map(parse_bool),
    })
}

fn missing_params() -> ApiError {
    ApiError::BadRequest("poll_sessions parameters are required".into())
}

/// Coerce an id attribute (Canvas casts strings and integers alike).
fn parse_id(raw: &Value) -> Result<Option<i64>, ApiError> {
    match raw {
        Value::Null => Ok(None),
        Value::Number(number) => number
            .as_i64()
            .map(Some)
            .ok_or_else(|| ApiError::BadRequest("invalid id".into())),
        Value::String(text) => text
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| ApiError::BadRequest("invalid id".into())),
        _ => Err(ApiError::BadRequest("invalid id".into())),
    }
}

/// Coerce a boolean attribute the way ActiveRecord casts it: `""` and
/// anything unparseable are falsey.
fn parse_bool(raw: &Value) -> Option<bool> {
    match raw {
        Value::Null => None,
        Value::Bool(flag) => Some(*flag),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "t" | "yes" | "y" => Some(true),
            _ => Some(false),
        },
        Value::Number(number) => Some(number.as_i64().unwrap_or(0) != 0),
        _ => Some(false),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /polls/:poll_id/poll_sessions` — poll_sessions#index.
async fn index(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(poll_id): Path<i64>,
    Query(pagination): Query<Pagination>,
    headers: HeaderMap,
    uri: OriginalUri,
) -> Result<Json<Value>, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;

    // authorized_action(@poll, @current_user, :update)
    if !poll_grants_update(&app.db, auth, &poll).await? {
        return Err(ApiError::Forbidden);
    }

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poll_sessions WHERE poll_id = $1")
        .bind(poll.id)
        .fetch_one(&app.db)
        .await?;

    let sessions: Vec<PollSession> = sqlx::query_as(
        "SELECT id, poll_id, course_id, course_section_id, has_public_results, is_published, created_at \
         FROM poll_sessions WHERE poll_id = $1 ORDER BY id LIMIT $2 OFFSET $3",
    )
    .bind(poll.id)
    .bind(pagination.per_page())
    .bind(pagination.offset())
    .fetch_all(&app.db)
    .await?;

    let items = build_items(&app.db, &auth, &sessions).await?;
    let meta = jsonapi_meta(&headers, &uri, total, &pagination);
    Ok(Json(envelope(items, meta)))
}

/// `GET /polls/:poll_id/poll_sessions/:id` — poll_sessions#show.
async fn show(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((poll_id, id)): Path<(i64, i64)>,
) -> Result<Json<Value>, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    let session = load_session(&app.db, poll.id, id).await?;

    // authorized_action(@poll_session, @current_user, :read)
    if !session_visible_to(&app.db, &auth, &session).await? {
        return Err(ApiError::Forbidden);
    }

    let items = build_items(&app.db, &auth, &[session]).await?;
    Ok(Json(envelope(items, None)))
}

/// `POST /polls/:poll_id/poll_sessions` — poll_sessions#create.
async fn create(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(poll_id): Path<i64>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    let params = session_params(&body)?;

    // Course.find(course_id) — a missing course is a 404, a missing
    // `course_id` is the Ruby "Course is required." 404.
    let course_id = match params.course_id {
        Some(Some(course_id)) => course_id,
        _ => return Err(ApiError::NotFound),
    };
    if !course_exists(&app.db, course_id).await? {
        return Err(ApiError::NotFound);
    }

    // @course.course_sections.find(course_section_id) — scoped to the course.
    let course_section_id = match params.course_section_id {
        Some(Some(section_id)) => {
            if !section_in_course(&app.db, section_id, course_id).await? {
                return Err(ApiError::NotFound);
            }
            Some(section_id)
        }
        _ => None,
    };

    // authorized_action(@poll, @current_user, :create) &&
    // authorized_action(@course, @current_user, :update)
    if !poll_grants_create(&app.db, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if !course_grants_update(&app.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }

    // A blank `has_public_results` is stored as false.
    let has_public_results = params.has_public_results.flatten().unwrap_or(false);

    let session: PollSession = sqlx::query_as(
        "INSERT INTO poll_sessions \
           (poll_id, course_id, course_section_id, has_public_results, is_published) \
         VALUES ($1, $2, $3, $4, false) \
         RETURNING id, poll_id, course_id, course_section_id, has_public_results, is_published, created_at",
    )
    .bind(poll.id)
    .bind(course_id)
    .bind(course_section_id)
    .bind(has_public_results)
    .fetch_one(&app.db)
    .await?;

    let items = build_items(&app.db, &auth, &[session]).await?;
    Ok(Json(envelope(items, None)))
}

/// `PUT /polls/:poll_id/poll_sessions/:id` — poll_sessions#update.
async fn update(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((poll_id, id)): Path<(i64, i64)>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    let session = load_session(&app.db, poll.id, id).await?;

    // authorized_action(@poll, @current_user, :update)
    if !poll_grants_update(&app.db, &auth, &poll).await? {
        return Err(ApiError::Forbidden);
    }

    let params = session_params(&body)?;

    let mut course_id = session.course_id;
    if let Some(requested) = params.course_id {
        // `belongs_to :course` — a course that does not exist fails the
        // validation and the controller answers 400.
        let requested =
            requested.ok_or_else(|| ApiError::BadRequest("Course must exist".into()))?;
        if !course_exists(&app.db, requested).await? {
            return Err(ApiError::BadRequest("Course must exist".into()));
        }
        course_id = requested;
    }

    let mut course_section_id = session.course_section_id;
    if let Some(requested) = params.course_section_id {
        course_section_id = requested;
    }

    // `validate :section_belongs_to_course` — the section has to belong to
    // the (possibly new) course of the session.
    if let Some(section_id) = course_section_id {
        if !section_in_course(&app.db, section_id, course_id).await? {
            return Err(ApiError::BadRequest(
                "That course section does not belong to the existing course.".into(),
            ));
        }
    }

    let mut has_public_results = session.has_public_results;
    if let Some(requested) = params.has_public_results {
        has_public_results = requested.unwrap_or(false);
    }

    let updated: PollSession = sqlx::query_as(
        "UPDATE poll_sessions \
           SET course_id = $1, course_section_id = $2, has_public_results = $3, updated_at = now() \
         WHERE id = $4 \
         RETURNING id, poll_id, course_id, course_section_id, has_public_results, is_published, created_at",
    )
    .bind(course_id)
    .bind(course_section_id)
    .bind(has_public_results)
    .bind(session.id)
    .fetch_one(&app.db)
    .await?;

    let items = build_items(&app.db, &auth, &[updated]).await?;
    Ok(Json(envelope(items, None)))
}

/// `DELETE /polls/:poll_id/poll_sessions/:id` — poll_sessions#destroy.
async fn destroy(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((poll_id, id)): Path<(i64, i64)>,
) -> Result<StatusCode, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    let session = load_session(&app.db, poll.id, id).await?;

    // authorized_action(@poll_session, @current_user, :delete)
    if !poll_grants_update(&app.db, &auth, &poll).await? {
        return Err(ApiError::Forbidden);
    }

    // `has_many :poll_submissions, dependent: :destroy`
    sqlx::query("DELETE FROM poll_submissions WHERE poll_session_id = $1")
        .bind(session.id)
        .execute(&app.db)
        .await?;
    sqlx::query("DELETE FROM poll_sessions WHERE id = $1")
        .bind(session.id)
        .execute(&app.db)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /polls/:poll_id/poll_sessions/:id/open` — poll_sessions#open.
async fn open(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((poll_id, id)): Path<(i64, i64)>,
) -> Result<Json<Value>, ApiError> {
    set_published(&app, &auth, poll_id, id, true).await
}

/// `GET /polls/:poll_id/poll_sessions/:id/close` — poll_sessions#close.
async fn close(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((poll_id, id)): Path<(i64, i64)>,
) -> Result<Json<Value>, ApiError> {
    set_published(&app, &auth, poll_id, id, false).await
}

/// Shared implementation of `open` / `close` (`publish!` / `close!`).
async fn set_published(
    app: &AppState,
    auth: &AuthUser,
    poll_id: i64,
    id: i64,
    published: bool,
) -> Result<Json<Value>, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    let session = load_session(&app.db, poll.id, id).await?;

    // authorized_action(@poll_session, @current_user, :publish)
    if !poll_grants_update(&app.db, &auth, &poll).await? {
        return Err(ApiError::Forbidden);
    }

    let updated: PollSession = sqlx::query_as(
        "UPDATE poll_sessions SET is_published = $1, updated_at = now() WHERE id = $2 \
         RETURNING id, poll_id, course_id, course_section_id, has_public_results, is_published, created_at",
    )
    .bind(published)
    .bind(session.id)
    .fetch_one(&app.db)
    .await?;

    let items = build_items(&app.db, auth, &[updated]).await?;
    Ok(Json(envelope(items, None)))
}

/// `GET /poll_sessions/opened` — poll_sessions#opened.
async fn opened(
    State(app): State<AppState>,
    auth: AuthUser,
    Query(pagination): Query<Pagination>,
    headers: HeaderMap,
    uri: OriginalUri,
) -> Result<Json<Value>, ApiError> {
    available_sessions(&app, &auth, true, &pagination, &headers, &uri).await
}

/// `GET /poll_sessions/closed` — poll_sessions#closed.
async fn closed(
    State(app): State<AppState>,
    auth: AuthUser,
    Query(pagination): Query<Pagination>,
    headers: HeaderMap,
    uri: OriginalUri,
) -> Result<Json<Value>, ApiError> {
    available_sessions(&app, &auth, false, &pagination, &headers, &uri).await
}

/// `Polling::PollSession.available_for(user)` filtered by the published
/// state — the shared body of `opened` and `closed`.
async fn available_sessions(
    app: &AppState,
    auth: &AuthUser,
    published: bool,
    pagination: &Pagination,
    headers: &HeaderMap,
    uri: &OriginalUri,
) -> Result<Json<Value>, ApiError> {
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM poll_sessions ps \
         WHERE ps.is_published = $2 \
           AND ps.course_id IN (SELECT course_id FROM enrollments \
                                WHERE user_id = $1 AND workflow_state <> 'deleted') \
           AND (ps.course_section_id IS NULL \
                OR ps.course_section_id IN (SELECT course_section_id FROM enrollments \
                                            WHERE user_id = $1 AND workflow_state <> 'deleted' \
                                              AND course_section_id IS NOT NULL))",
    )
    .bind(auth.id)
    .bind(published)
    .fetch_one(&app.db)
    .await?;

    let sessions: Vec<PollSession> = sqlx::query_as(
        "SELECT id, poll_id, course_id, course_section_id, has_public_results, is_published, created_at \
         FROM poll_sessions ps \
         WHERE ps.is_published = $2 \
           AND ps.course_id IN (SELECT course_id FROM enrollments \
                                WHERE user_id = $1 AND workflow_state <> 'deleted') \
           AND (ps.course_section_id IS NULL \
                OR ps.course_section_id IN (SELECT course_section_id FROM enrollments \
                                            WHERE user_id = $1 AND workflow_state <> 'deleted' \
                                              AND course_section_id IS NOT NULL)) \
         ORDER BY ps.id LIMIT $3 OFFSET $4",
    )
    .bind(auth.id)
    .bind(published)
    .bind(pagination.per_page())
    .bind(pagination.offset())
    .fetch_all(&app.db)
    .await?;

    let items = build_items(&app.db, auth, &sessions).await?;
    let meta = jsonapi_meta(headers, uri, total, pagination);
    Ok(Json(envelope(items, meta)))
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// The response envelope: `{"poll_sessions": [...]}` (+ `meta` for JSON:API).
fn envelope(items: Vec<Value>, meta: Option<Value>) -> Value {
    let mut root = Map::new();
    root.insert("poll_sessions".to_string(), Value::Array(items));
    if let Some(meta) = meta {
        root.insert("meta".to_string(), meta);
    }
    Value::Object(root)
}

/// Serialize a list of poll sessions.
///
/// `results` / `has_public_results` are only rendered when the viewer may see
/// the results (`has_public_results` or the poll `:update` right), and
/// `poll_submissions` only holds the caller's own submissions for everybody
/// but such a viewer.
async fn build_items(
    db: &PgPool,
    auth: &AuthUser,
    sessions: &[PollSession],
) -> Result<Vec<Value>, ApiError> {
    if sessions.is_empty() {
        return Ok(Vec::new());
    }

    let session_ids: Vec<i64> = sessions.iter().map(|session| session.id).collect();
    let updatable_polls = updatable_poll_ids(db, auth, sessions).await?;

    let submissions: Vec<PollSubmission> = sqlx::query_as(
        "SELECT id, poll_session_id, poll_choice_id, user_id, created_at \
         FROM poll_submissions WHERE poll_session_id = ANY($1) ORDER BY id",
    )
    .bind(&session_ids)
    .fetch_all(db)
    .await?;

    let counts: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT poll_session_id, poll_choice_id, COUNT(*) \
         FROM poll_submissions WHERE poll_session_id = ANY($1) \
         GROUP BY poll_session_id, poll_choice_id",
    )
    .bind(&session_ids)
    .fetch_all(db)
    .await?;

    let mut submissions_by_session: HashMap<i64, Vec<&PollSubmission>> = HashMap::new();
    for submission in &submissions {
        submissions_by_session
            .entry(submission.poll_session_id)
            .or_default()
            .push(submission);
    }

    let mut counts_by_session: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    for (session_id, choice_id, count) in counts {
        counts_by_session
            .entry(session_id)
            .or_default()
            .push((choice_id, count));
    }

    let mut items = Vec::with_capacity(sessions.len());
    for session in sessions {
        let can_view_results =
            session.has_public_results || updatable_polls.contains(&session.poll_id);
        let own_submissions: Vec<&PollSubmission> = submissions_by_session
            .get(&session.id)
            .map(|rows| {
                rows.iter()
                    .copied()
                    .filter(|submission| submission.user_id == auth.id)
                    .collect()
            })
            .unwrap_or_default();
        let visible_submissions: Vec<&PollSubmission> = if can_view_results {
            submissions_by_session
                .get(&session.id)
                .cloned()
                .unwrap_or_default()
        } else {
            own_submissions
        };

        items.push(session_json(
            session,
            can_view_results,
            counts_by_session.get(&session.id),
            &visible_submissions,
            auth.id,
        ));
    }

    Ok(items)
}

/// One poll session, in the Canvas API shape.
fn session_json(
    session: &PollSession,
    can_view_results: bool,
    counts: Option<&Vec<(i64, i64)>>,
    submissions: &[&PollSubmission],
    viewer_id: i64,
) -> Value {
    let mut object = Map::new();
    object.insert("id".to_string(), json!(session.id.to_string()));
    object.insert("is_published".to_string(), json!(session.is_published));
    if can_view_results {
        object.insert(
            "has_public_results".to_string(),
            json!(session.has_public_results),
        );
        let mut results = Map::new();
        if let Some(counts) = counts {
            for (choice_id, count) in counts {
                results.insert(choice_id.to_string(), json!(count));
            }
        }
        object.insert("results".to_string(), Value::Object(results));
    }
    object.insert(
        "course_id".to_string(),
        json!(session.course_id.to_string()),
    );
    object.insert(
        "course_section_id".to_string(),
        match session.course_section_id {
            Some(section_id) => json!(section_id.to_string()),
            None => Value::Null,
        },
    );
    object.insert(
        "created_at".to_string(),
        json!(timestamp(session.created_at)),
    );
    object.insert("poll_id".to_string(), json!(session.poll_id.to_string()));
    object.insert(
        "poll_submissions".to_string(),
        Value::Array(submissions.iter().map(|row| submission_json(row)).collect()),
    );
    object.insert(
        "has_submitted".to_string(),
        json!(submissions.iter().any(|row| row.user_id == viewer_id)),
    );
    Value::Object(object)
}

/// One poll submission, in the Canvas API shape (`Polling::PollSubmissionSerializer`).
fn submission_json(submission: &PollSubmission) -> Value {
    json!({
        "id": submission.id.to_string(),
        "poll_session_id": submission.poll_session_id.to_string(),
        "poll_choice_id": submission.poll_choice_id.to_string(),
        "user_id": submission.user_id.to_string(),
        "created_at": timestamp(submission.created_at),
    })
}

/// Canvas renders timestamps with millisecond precision in UTC.
fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// Pagination meta
// ---------------------------------------------------------------------------

/// The JSON:API `meta` block (`Api.jsonapi_paginate`).
///
/// Canvas only adds it when the request asks for JSON:API
/// (`Accept: application/vnd.api+json`).
fn jsonapi_meta(
    headers: &HeaderMap,
    uri: &OriginalUri,
    total: i64,
    pagination: &Pagination,
) -> Option<Value> {
    let accepts_jsonapi = headers
        .get(ACCEPT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("application/vnd.api+json"))
        .unwrap_or(false);
    if !accepts_jsonapi {
        return None;
    }

    let per_page = pagination.per_page();
    let page = pagination.page();
    let page_count = if per_page > 0 {
        (total + per_page - 1) / per_page
    } else {
        0
    };

    let current = page_url(headers, uri, &page.to_string(), per_page);
    let mut pagination_meta = Map::new();
    pagination_meta.insert("per_page".to_string(), json!(per_page));
    pagination_meta.insert("page".to_string(), json!(page));
    pagination_meta.insert("count".to_string(), json!(total));
    pagination_meta.insert("page_count".to_string(), json!(page_count));
    pagination_meta.insert("current".to_string(), json!(current));
    pagination_meta.insert(
        "template".to_string(),
        json!(page_url(headers, uri, "{page}", per_page)),
    );
    if page < page_count {
        pagination_meta.insert(
            "next".to_string(),
            json!(page_url(headers, uri, &(page + 1).to_string(), per_page)),
        );
    }
    if page > 1 {
        pagination_meta.insert(
            "prev".to_string(),
            json!(page_url(headers, uri, &(page - 1).to_string(), per_page)),
        );
    }
    pagination_meta.insert(
        "first".to_string(),
        json!(page_url(headers, uri, "1", per_page)),
    );
    if page_count > 0 {
        pagination_meta.insert(
            "last".to_string(),
            json!(page_url(headers, uri, &page_count.to_string(), per_page)),
        );
    }

    let mut meta = Map::new();
    meta.insert("pagination".to_string(), Value::Object(pagination_meta));
    meta.insert("primaryCollection".to_string(), json!("poll_sessions"));
    Some(Value::Object(meta))
}

/// Build the pagination link of a collection.
fn page_url(headers: &HeaderMap, uri: &OriginalUri, page: &str, per_page: i64) -> String {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    format!(
        "http://{host}{path}?page={page}&per_page={per_page}",
        host = host,
        path = uri.path(),
        page = page,
        per_page = per_page,
    )
}

// ---------------------------------------------------------------------------
// Lookups
// ---------------------------------------------------------------------------

/// `Polling::Poll.find(id)` — 404 when the poll does not exist.
async fn load_poll(db: &PgPool, poll_id: i64) -> Result<Poll, ApiError> {
    sqlx::query_as("SELECT id, user_id FROM polls WHERE id = $1")
        .bind(poll_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

/// `@poll.poll_sessions.find(id)` — 404 when the session does not belong to
/// the poll.
async fn load_session(db: &PgPool, poll_id: i64, session_id: i64) -> Result<PollSession, ApiError> {
    sqlx::query_as(
        "SELECT id, poll_id, course_id, course_section_id, has_public_results, is_published, created_at \
         FROM poll_sessions WHERE id = $1 AND poll_id = $2",
    )
    .bind(session_id)
    .bind(poll_id)
    .fetch_optional(db)
    .await?
    .ok_or(ApiError::NotFound)
}

async fn course_exists(db: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1)")
        .bind(course_id)
        .fetch_one(db)
        .await?;
    Ok(exists)
}

async fn section_in_course(db: &PgPool, section_id: i64, course_id: i64) -> Result<bool, ApiError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM course_sections WHERE id = $1 AND course_id = $2)",
    )
    .bind(section_id)
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(exists)
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// `poll.grants_right?(user, :update)`.
///
/// The creator of the poll may update it; so may a teacher of a course the
/// poll has a session in (`manage_course_content_edit`).
async fn poll_grants_update(db: &PgPool, auth: &AuthUser, poll: &Poll) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    if poll.user_id == auth.id {
        return Ok(true);
    }
    let granted: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
           SELECT 1 FROM poll_sessions ps \
           JOIN enrollments e ON e.course_id = ps.course_id \
           WHERE ps.poll_id = $1 AND e.user_id = $2 AND e.role = 'teacher' \
             AND e.workflow_state <> 'deleted')",
    )
    .bind(poll.id)
    .bind(auth.id)
    .fetch_one(db)
    .await?;
    Ok(granted)
}

/// `poll.grants_right?(user, :create)` — any active teacher enrolment.
async fn poll_grants_create(db: &PgPool, auth: &AuthUser) -> Result<bool, ApiError> {
    let granted: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND role = 'teacher' AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .fetch_one(db)
    .await?;
    Ok(granted)
}

/// `course.grants_right?(user, :update)` — a teacher of that course.
async fn course_grants_update(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    let granted: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' \
           AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(granted)
}

/// `poll_session.visible_to?(user, session)`.
///
/// The course has to grant `:read` and, when the session is bound to a
/// section, that section has to grant `:read` as well.
async fn session_visible_to(
    db: &PgPool,
    auth: &AuthUser,
    session: &PollSession,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }

    let course_read: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(session.course_id)
    .fetch_one(db)
    .await?;
    if !course_read {
        return Ok(false);
    }

    let Some(section_id) = session.course_section_id else {
        return Ok(true);
    };

    let section_read: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
          WHERE user_id = $1 AND course_section_id = $2 AND workflow_state <> 'deleted') \
         OR EXISTS (SELECT 1 FROM enrollments \
          WHERE user_id = $1 AND course_id = $3 AND role = 'teacher' \
            AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(section_id)
    .bind(session.course_id)
    .fetch_one(db)
    .await?;
    Ok(section_read)
}

/// The polls of the given sessions the user may update — the right that
/// also unlocks `results` for the serializer.
async fn updatable_poll_ids(
    db: &PgPool,
    auth: &AuthUser,
    sessions: &[PollSession],
) -> Result<HashSet<i64>, ApiError> {
    let mut poll_ids: Vec<i64> = sessions.iter().map(|session| session.poll_id).collect();
    poll_ids.sort_unstable();
    poll_ids.dedup();
    if poll_ids.is_empty() {
        return Ok(HashSet::new());
    }
    if auth.role == "admin" {
        return Ok(poll_ids.into_iter().collect());
    }

    let granted: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT p.id FROM polls p WHERE p.id = ANY($1) AND ( \
           p.user_id = $2 \
           OR EXISTS (SELECT 1 FROM poll_sessions ps \
                      JOIN enrollments e ON e.course_id = ps.course_id \
                      WHERE ps.poll_id = p.id AND e.user_id = $2 AND e.role = 'teacher' \
                        AND e.workflow_state <> 'deleted'))",
    )
    .bind(poll_ids)
    .bind(auth.id)
    .fetch_all(db)
    .await?;
    Ok(granted.into_iter().collect())
}
