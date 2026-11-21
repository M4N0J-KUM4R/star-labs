//! polling/polls and polling/poll_choices — Canvas `Polling::PollsController`
//! and `Polling::PollChoicesController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /polls` — the polls of the current user
//! - `POST   /polls` — create a poll
//! - `GET    /polls/{id}` — show a poll
//! - `PUT    /polls/{id}` — update a poll
//! - `DELETE /polls/{id}` — delete a poll
//! - `GET    /polls/{pollId}/poll_choices` — the choices of a poll
//! - `POST   /polls/{pollId}/poll_choices` — create a choice
//! - `GET    /polls/{pollId}/poll_choices/{id}` — show a choice
//! - `PUT    /polls/{pollId}/poll_choices/{id}` — update a choice
//! - `DELETE /polls/{pollId}/poll_choices/{id}` — delete a choice
//!
//! Canvas routes (config/routes.rb, the polling scope):
//!   get    "polls", action: :index
//!   post   "polls", action: :create
//!   get    "polls/:id", action: :show
//!   put    "polls/:id", action: :update
//!   delete "polls/:id", action: :destroy
//!   get    "polls/:poll_id/poll_choices", action: :index
//!   post   "polls/:poll_id/poll_choices", action: :create
//!   get    "polls/:poll_id/poll_choices/:id", action: :show
//!   put    "polls/:poll_id/poll_choices/:id", action: :update
//!   delete "polls/:poll_id/poll_choices/:id", action: :destroy
//!
//! Canvas controllers (`app/controllers/polling/polls_controller.rb` and
//! `app/controllers/polling/poll_choices_controller.rb`): both include
//! `Filters::Polling`, which loads the poll and answers 404 when it does not
//! exist, and answers 403 when the caller may not read it. A poll of another
//! user is a 403.
//!
//! The response is the Canvas `Canvas::APIArraySerializer` shape: a flat
//! object per record under the `polls` / `poll_choices` root, with the
//! attributes the serializer keeps for the viewer:
//! - a poll has `id`, `question`, `description` and `created_at` for everybody
//!   and `total_results` and `user_id` for the owner;
//! - a poll choice has `id`, `text` and `position`, plus `is_correct` for the
//!   owner and for a viewer of a closed poll.
//!
//! The `Accept: application/vnd.api+json` header adds the JSON:API `meta`
//! block (`pagination` + `primaryCollection`).

use axum::{
    extract::{OriginalUri, Path, Query, State},
    http::{header::ACCEPT, HeaderMap},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Domain router for the Canvas poll and poll-choice routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/polls", get(index_polls).post(create_poll))
        .route(
            "/polls/{id}",
            get(show_poll).put(update_poll).delete(destroy_poll),
        )
        .route(
            "/polls/{poll_id}/poll_choices",
            get(index_choices).post(create_choice),
        )
        .route(
            "/polls/{poll_id}/poll_choices/{id}",
            get(show_choice).put(update_choice).delete(destroy_choice),
        )
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// A poll row.
#[derive(Debug, sqlx::FromRow)]
struct Poll {
    id: i64,
    user_id: i64,
    question: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
}

/// A poll choice row.
#[derive(Debug, sqlx::FromRow)]
struct PollChoice {
    id: i64,
    poll_id: i64,
    name: String,
    is_correct: bool,
    position: Option<i32>,
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// The body of a poll create or update request.
#[derive(Debug, Default, Deserialize)]
struct PollRequest {
    polls: Option<Vec<PollFields>>,
}

/// The fields of a poll in a request.
#[derive(Debug, Default, Deserialize)]
struct PollFields {
    question: Option<String>,
    description: Option<String>,
    course_id: Option<i64>,
}

/// The body of a poll-choice create or update request.
#[derive(Debug, Default, Deserialize)]
struct ChoiceRequest {
    poll_choices: Option<Vec<ChoiceFields>>,
}

/// The fields of a poll choice in a request.
#[derive(Debug, Default, Deserialize)]
struct ChoiceFields {
    text: Option<String>,
    description: Option<String>,
    is_correct: Option<bool>,
    position: Option<i32>,
}

/// Canvas pagination parameters (`page`, `per_page`).
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
            Some(value) if value > 0 => value.min(100),
            _ => 10,
        }
    }

    fn offset(&self) -> i64 {
        (self.page() - 1) * self.per_page()
    }
}

// ---------------------------------------------------------------------------
// Handlers — Polling::PollsController
// ---------------------------------------------------------------------------

/// `GET /polls` — polls#index.
async fn index_polls(
    State(app): State<AppState>,
    auth: AuthUser,
    Query(pagination): Query<Pagination>,
    headers: HeaderMap,
    uri: OriginalUri,
) -> Result<impl IntoResponse, ApiError> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM polls WHERE user_id = $1")
        .bind(auth.id)
        .fetch_one(&app.db)
        .await?;

    let polls: Vec<Poll> = sqlx::query_as(
        "SELECT id, user_id, question, description, created_at FROM polls \
         WHERE user_id = $1 ORDER BY created_at DESC, id DESC LIMIT $2 OFFSET $3",
    )
    .bind(auth.id)
    .bind(pagination.per_page())
    .bind(pagination.offset())
    .fetch_all(&app.db)
    .await?;

    let mut items = Vec::with_capacity(polls.len());
    for poll in &polls {
        items.push(poll_json(&app.db, poll, true).await?);
    }

    let meta = jsonapi_meta(&headers, &uri, total, &pagination, "polls");
    Ok(Json(envelope("polls", items, meta)))
}

/// `GET /polls/:id` — polls#show.
async fn show_poll(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let poll = load_poll(&app.db, id).await?;
    let owner = poll.user_id == auth.id || auth.role == "admin";
    require_poll_read(&app.db, &auth, &poll).await?;
    let items = vec![poll_json(&app.db, &poll, owner).await?];
    Ok(Json(envelope("polls", items, None)))
}

/// `POST /polls` — polls#create.
async fn create_poll(
    State(app): State<AppState>,
    auth: AuthUser,
    Json(body): Json<PollRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let fields = body.polls.and_then(|polls| polls.into_iter().next());
    let question = fields.as_ref().and_then(|fields| fields.question.clone());
    let Some(question) = question else {
        return Err(ApiError::BadRequest(
            "Validation failed: Question can't be blank".to_string(),
        ));
    };
    let description = fields
        .as_ref()
        .and_then(|fields| fields.description.clone());
    let course_id = fields.as_ref().and_then(|fields| fields.course_id);

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO polls (user_id, course_id, question, description) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(auth.id)
    .bind(course_id)
    .bind(&question)
    .bind(&description)
    .fetch_one(&app.db)
    .await?;

    let poll = load_poll(&app.db, id).await?;
    let items = vec![poll_json(&app.db, &poll, true).await?];
    Ok(Json(envelope("polls", items, None)))
}

/// `PUT /polls/:id` — polls#update.
async fn update_poll(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
    Json(body): Json<PollRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let poll = load_poll(&app.db, id).await?;
    require_poll_update(&app.db, &auth, &poll).await?;

    let fields = body.polls.and_then(|polls| polls.into_iter().next());
    let question = fields.as_ref().and_then(|fields| fields.question.clone());
    let description = fields
        .as_ref()
        .and_then(|fields| fields.description.clone());
    let course_id = fields.as_ref().and_then(|fields| fields.course_id);

    sqlx::query(
        "UPDATE polls SET question = COALESCE($1, question), \
             description = COALESCE($2, description), course_id = COALESCE($3, course_id), \
             updated_at = now() WHERE id = $4",
    )
    .bind(question)
    .bind(description)
    .bind(course_id)
    .bind(id)
    .execute(&app.db)
    .await?;

    let poll = load_poll(&app.db, id).await?;
    let items = vec![poll_json(&app.db, &poll, true).await?];
    Ok(Json(envelope("polls", items, None)))
}

/// `DELETE /polls/:id` — polls#destroy.
async fn destroy_poll(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    let poll = load_poll(&app.db, id).await?;
    require_poll_update(&app.db, &auth, &poll).await?;

    sqlx::query("DELETE FROM polls WHERE id = $1")
        .bind(id)
        .execute(&app.db)
        .await?;

    let items = vec![poll_json(&app.db, &poll, true).await?];
    Ok(Json(envelope("polls", items, None)))
}

// ---------------------------------------------------------------------------
// Handlers — Polling::PollChoicesController
// ---------------------------------------------------------------------------

/// `GET /polls/:poll_id/poll_choices` — poll_choices#index.
async fn index_choices(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(poll_id): Path<i64>,
    Query(pagination): Query<Pagination>,
    headers: HeaderMap,
    uri: OriginalUri,
) -> Result<impl IntoResponse, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    let owner = poll.user_id == auth.id || auth.role == "admin";
    require_poll_read(&app.db, &auth, &poll).await?;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM poll_choices WHERE poll_id = $1")
        .bind(poll_id)
        .fetch_one(&app.db)
        .await?;

    let choices: Vec<PollChoice> = sqlx::query_as(
        "SELECT id, poll_id, name, is_correct, position FROM poll_choices \
         WHERE poll_id = $1 ORDER BY position ASC NULLS LAST, id ASC LIMIT $2 OFFSET $3",
    )
    .bind(poll_id)
    .bind(pagination.per_page())
    .bind(pagination.offset())
    .fetch_all(&app.db)
    .await?;

    let items = choices
        .iter()
        .map(|choice| choice_json(choice, owner))
        .collect::<Vec<Value>>();

    let meta = jsonapi_meta(&headers, &uri, total, &pagination, "poll_choices");
    Ok(Json(envelope("poll_choices", items, meta)))
}

/// `GET /polls/:poll_id/poll_choices/:id` — poll_choices#show.
async fn show_choice(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((poll_id, id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    let owner = poll.user_id == auth.id || auth.role == "admin";
    require_poll_read(&app.db, &auth, &poll).await?;
    let choice = load_choice(&app.db, poll_id, id).await?;
    let items = vec![choice_json(&choice, owner)];
    Ok(Json(envelope("poll_choices", items, None)))
}

/// `POST /polls/:poll_id/poll_choices` — poll_choices#create.
async fn create_choice(
    State(app): State<AppState>,
    auth: AuthUser,
    Path(poll_id): Path<i64>,
    Json(body): Json<ChoiceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    require_poll_update(&app.db, &auth, &poll).await?;

    let fields = body
        .poll_choices
        .and_then(|choices| choices.into_iter().next());
    let text = fields.as_ref().and_then(|fields| fields.text.clone());
    let Some(text) = text else {
        return Err(ApiError::BadRequest(
            "Validation failed: Text can't be blank".to_string(),
        ));
    };
    let description = fields
        .as_ref()
        .and_then(|fields| fields.description.clone());
    let is_correct = fields
        .as_ref()
        .and_then(|fields| fields.is_correct)
        .unwrap_or(false);
    let position = fields.as_ref().and_then(|fields| fields.position);

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO poll_choices (poll_id, name, description, is_correct, position) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(poll_id)
    .bind(&text)
    .bind(&description)
    .bind(is_correct)
    .bind(position)
    .fetch_one(&app.db)
    .await?;

    let choice = load_choice(&app.db, poll_id, id).await?;
    let items = vec![choice_json(&choice, true)];
    Ok(Json(envelope("poll_choices", items, None)))
}

/// `PUT /polls/:poll_id/poll_choices/:id` — poll_choices#update.
async fn update_choice(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((poll_id, id)): Path<(i64, i64)>,
    Json(body): Json<ChoiceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    require_poll_update(&app.db, &auth, &poll).await?;
    load_choice(&app.db, poll_id, id).await?;

    let fields = body
        .poll_choices
        .and_then(|choices| choices.into_iter().next());
    let text = fields.as_ref().and_then(|fields| fields.text.clone());
    let description = fields
        .as_ref()
        .and_then(|fields| fields.description.clone());
    let is_correct = fields.as_ref().and_then(|fields| fields.is_correct);
    let position = fields.as_ref().and_then(|fields| fields.position);

    sqlx::query(
        "UPDATE poll_choices SET name = COALESCE($1, name), \
             description = COALESCE($2, description), is_correct = COALESCE($3, is_correct), \
             position = COALESCE($4, position), updated_at = now() WHERE id = $5",
    )
    .bind(text)
    .bind(description)
    .bind(is_correct)
    .bind(position)
    .bind(id)
    .execute(&app.db)
    .await?;

    let choice = load_choice(&app.db, poll_id, id).await?;
    let items = vec![choice_json(&choice, true)];
    Ok(Json(envelope("poll_choices", items, None)))
}

/// `DELETE /polls/:poll_id/poll_choices/:id` — poll_choices#destroy.
async fn destroy_choice(
    State(app): State<AppState>,
    auth: AuthUser,
    Path((poll_id, id)): Path<(i64, i64)>,
) -> Result<impl IntoResponse, ApiError> {
    let poll = load_poll(&app.db, poll_id).await?;
    require_poll_update(&app.db, &auth, &poll).await?;
    let choice = load_choice(&app.db, poll_id, id).await?;

    sqlx::query("DELETE FROM poll_choices WHERE id = $1")
        .bind(choice.id)
        .execute(&app.db)
        .await?;

    let items = vec![choice_json(&choice, true)];
    Ok(Json(envelope("poll_choices", items, None)))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The poll of an id — 404 when it does not exist.
async fn load_poll(db: &PgPool, poll_id: i64) -> Result<Poll, ApiError> {
    sqlx::query_as(
        "SELECT id, user_id, course_id, question, description, created_at \
         FROM polls WHERE id = $1",
    )
    .bind(poll_id)
    .fetch_optional(db)
    .await?
    .ok_or(ApiError::NotFound)
}

/// The choice of a poll — 404 when it does not belong to the poll.
async fn load_choice(db: &PgPool, poll_id: i64, choice_id: i64) -> Result<PollChoice, ApiError> {
    sqlx::query_as(
        "SELECT id, poll_id, name, is_correct, position FROM poll_choices \
         WHERE id = $1 AND poll_id = $2",
    )
    .bind(choice_id)
    .bind(poll_id)
    .fetch_optional(db)
    .await?
    .ok_or(ApiError::NotFound)
}

/// `authorized_action(@poll, @current_user, :read)` — the owner of the poll
/// and the site admins may read it.
async fn require_poll_read(db: &PgPool, auth: &AuthUser, poll: &Poll) -> Result<(), ApiError> {
    if poll.user_id == auth.id || auth.role == "admin" {
        return Ok(());
    }
    let _ = db;
    Err(ApiError::Forbidden)
}

/// `authorized_action(@poll, @current_user, :update)` — only the owner of the
/// poll and the site admins may change it.
async fn require_poll_update(db: &PgPool, auth: &AuthUser, poll: &Poll) -> Result<(), ApiError> {
    if poll.user_id == auth.id || auth.role == "admin" {
        return Ok(());
    }
    let _ = db;
    Err(ApiError::Forbidden)
}

/// One poll, in the Canvas `Polling::PollSerializer` shape.
async fn poll_json(db: &PgPool, poll: &Poll, owner: bool) -> Result<Value, ApiError> {
    let mut object = Map::new();
    object.insert("id".to_string(), json!(poll.id.to_string()));
    object.insert("question".to_string(), json!(poll.question));
    object.insert(
        "description".to_string(),
        match &poll.description {
            Some(description) => json!(description),
            None => Value::Null,
        },
    );
    object.insert("created_at".to_string(), json!(timestamp(poll.created_at)));

    if owner {
        // Canvas aggregates the results of the sessions of the poll, keyed by
        // the id of the poll choice.
        let counts: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT s.poll_choice_id, COUNT(*) FROM poll_submissions s \
             JOIN poll_sessions ps ON ps.id = s.poll_session_id \
             WHERE ps.poll_id = $1 GROUP BY s.poll_choice_id",
        )
        .bind(poll.id)
        .fetch_all(db)
        .await?;

        let mut results = Map::new();
        for (choice_id, count) in counts {
            results.insert(choice_id.to_string(), json!(count));
        }
        object.insert("total_results".to_string(), Value::Object(results));
        object.insert("user_id".to_string(), json!(poll.user_id.to_string()));
    }

    // Canvas embeds the ids of the choices of the poll.
    let choice_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM poll_choices WHERE poll_id = $1 ORDER BY position ASC NULLS LAST, id ASC",
    )
    .bind(poll.id)
    .fetch_all(db)
    .await?;
    object.insert(
        "poll_choices".to_string(),
        Value::Array(choice_ids.iter().map(|id| json!(id.to_string())).collect()),
    );

    Ok(Value::Object(object))
}

/// One poll choice, in the Canvas `Polling::PollChoiceSerializer` shape.
fn choice_json(choice: &PollChoice, owner: bool) -> Value {
    let mut object = Map::new();
    object.insert("id".to_string(), json!(choice.id.to_string()));
    // Canvas calls the text of a choice `text`.
    object.insert("text".to_string(), json!(choice.name));
    object.insert(
        "position".to_string(),
        match choice.position {
            Some(position) => json!(position),
            None => Value::Null,
        },
    );
    if owner {
        object.insert("is_correct".to_string(), json!(choice.is_correct));
    }
    object.insert("poll".to_string(), json!(choice.poll_id.to_string()));
    Value::Object(object)
}

/// Canvas renders timestamps with millisecond precision in UTC.
fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The Canvas `Canvas::APIArraySerializer` envelope.
fn envelope(root: &str, items: Vec<Value>, meta: Option<Value>) -> Value {
    let mut object = Map::new();
    object.insert(root.to_string(), Value::Array(items));
    if let Some(meta) = meta {
        object.insert("meta".to_string(), meta);
    }
    Value::Object(object)
}

/// The JSON:API `meta` block (`Api.jsonapi_paginate`).
///
/// Canvas only adds it when the request asks for JSON:API
/// (`Accept: application/vnd.api+json`).
fn jsonapi_meta(
    headers: &HeaderMap,
    uri: &OriginalUri,
    total: i64,
    pagination: &Pagination,
    collection: &str,
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
    meta.insert("primaryCollection".to_string(), json!(collection));
    Some(Value::Object(meta))
}

/// The URL of a page of the listing.
fn page_url(headers: &HeaderMap, uri: &OriginalUri, page: &str, per_page: i64) -> String {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    let mut url = format!(
        "http://{host}{}?page={page}&per_page={per_page}",
        uri.path()
    );
    if let Some(query) = uri.query() {
        if !query.contains("page=") && !query.contains("per_page=") {
            url.push('&');
            url.push_str(query);
        }
    }
    url
}
