//! external_feeds — Canvas `ExternalFeedsController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /courses/{courseId}/external_feeds` — the feeds of a course
//! - `POST   /courses/{courseId}/external_feeds` — create a feed
//! - `DELETE /courses/{courseId}/external_feeds/{externalFeedId}` — delete a feed
//! - `GET    /groups/{groupId}/external_feeds` — the feeds of a group
//! - `POST   /groups/{groupId}/external_feeds` — create a feed
//! - `DELETE /groups/{groupId}/external_feeds/{externalFeedId}` — delete a feed
//!
//! Canvas routes (config/routes.rb, the course and group API blocks):
//!   get    "#{context}s/:#{context}_id/external_feeds", action: :index
//!   post   "#{context}s/:#{context}_id/external_feeds", action: :create
//!   delete "#{context}s/:#{context}_id/external_feeds/:external_feed_id", action: :destroy
//!
//! Canvas controller (`app/controllers/external_feeds_controller.rb`): the
//! `before_action` is `require_context` (404 when the course or the group
//! does not exist); every action needs the right to create an announcement
//! in the context (403).
//!
//! The feed JSON is the Canvas `Api::V1::ExternalFeeds` shape: `id`, `url`,
//! `header_match`, `created_at`, `verbosity` and `display_name`, plus
//! `external_feed_entries_count`.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// An `external_feeds` row.
#[derive(Debug, sqlx::FromRow)]
struct ExternalFeed {
    id: i64,
    title: Option<String>,
    url: String,
    header_match: Option<String>,
    verbosity: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// The columns of an `external_feeds` row.
const FEED_COLUMNS: &str = "id, title, url, header_match, verbosity, \
     created_at::timestamptz AS created_at";

/// The body of a create request.
#[derive(Debug, Default, Deserialize)]
struct FeedRequest {
    url: Option<String>,
    header_match: Option<String>,
    verbosity: Option<String>,
}

/// The context a feed belongs to.
#[derive(Debug, Clone, Copy)]
enum FeedContext {
    Course(i64),
    Group(i64),
}

impl FeedContext {
    /// The Canvas context type of the context.
    fn context_type(self) -> &'static str {
        match self {
            FeedContext::Course(_) => "Course",
            FeedContext::Group(_) => "Group",
        }
    }

    /// The Canvas context id of the context.
    fn context_id(self) -> i64 {
        match self {
            FeedContext::Course(id) => id,
            FeedContext::Group(id) => id,
        }
    }
}

/// Domain router for the Canvas external-feed routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{course_id}/external_feeds",
            get(index_course).post(create_course),
        )
        .route(
            "/courses/{course_id}/external_feeds/{external_feed_id}",
            axum::routing::delete(destroy_course),
        )
        .route(
            "/groups/{group_id}/external_feeds",
            get(index_group).post(create_group),
        )
        .route(
            "/groups/{group_id}/external_feeds/{external_feed_id}",
            axum::routing::delete(destroy_group),
        )
}

/// `GET /courses/:course_id/external_feeds` — external_feeds#index.
async fn index_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    index(&state, FeedContext::Course(course_id), &auth).await
}

/// `GET /groups/:group_id/external_feeds` — external_feeds#index.
async fn index_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    index(&state, FeedContext::Group(group_id), &auth).await
}

/// The shared listing of the feeds of a context.
async fn index(
    state: &AppState,
    context: FeedContext,
    auth: &AuthUser,
) -> Result<Response, ApiError> {
    require_context(&state.db, context).await?;
    require_announcement_right(&state.db, auth, context).await?;

    let feeds = feeds_of(&state.db, context).await?;
    let mut items = Vec::with_capacity(feeds.len());
    for feed in &feeds {
        items.push(feed_json(&state.db, feed).await?);
    }
    Ok(Json(items).into_response())
}

/// `POST /courses/:course_id/external_feeds` — external_feeds#create.
async fn create_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<FeedRequest>>,
) -> Result<Response, ApiError> {
    create(&state, FeedContext::Course(course_id), &auth, body).await
}

/// `POST /groups/:group_id/external_feeds` — external_feeds#create.
async fn create_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<FeedRequest>>,
) -> Result<Response, ApiError> {
    create(&state, FeedContext::Group(group_id), &auth, body).await
}

/// The shared creation of a feed.
async fn create(
    state: &AppState,
    context: FeedContext,
    auth: &AuthUser,
    body: Option<Json<FeedRequest>>,
) -> Result<Response, ApiError> {
    require_context(&state.db, context).await?;
    require_announcement_right(&state.db, auth, context).await?;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    let Some(url) = body.url.as_ref().filter(|value| !value.is_empty()) else {
        return Err(ApiError::BadRequest("Url can't be blank".to_string()));
    };

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO external_feeds (user_id, context_id, context_type, url, header_match, \
             verbosity, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now(), now()) RETURNING id",
    )
    .bind(auth.id)
    .bind(context.context_id())
    .bind(context.context_type())
    .bind(url)
    .bind(&body.header_match)
    .bind(body.verbosity.clone().or(Some("full".to_string())))
    .fetch_one(&state.db)
    .await?;

    let feed = load_feed(&state.db, id).await?;
    Ok(Json(feed_json(&state.db, &feed).await?).into_response())
}

/// `DELETE /courses/:course_id/external_feeds/:external_feed_id` —
/// external_feeds#destroy.
async fn destroy_course(
    State(state): State<AppState>,
    Path((course_id, external_feed_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    destroy(
        &state,
        FeedContext::Course(course_id),
        external_feed_id,
        &auth,
    )
    .await
}

/// `DELETE /groups/:group_id/external_feeds/:external_feed_id` —
/// external_feeds#destroy.
async fn destroy_group(
    State(state): State<AppState>,
    Path((group_id, external_feed_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    destroy(
        &state,
        FeedContext::Group(group_id),
        external_feed_id,
        &auth,
    )
    .await
}

/// The shared deletion of a feed.
async fn destroy(
    state: &AppState,
    context: FeedContext,
    external_feed_id: i64,
    auth: &AuthUser,
) -> Result<Response, ApiError> {
    require_context(&state.db, context).await?;
    require_announcement_right(&state.db, auth, context).await?;

    // Canvas finds the feed in the context — a feed of another context is a
    // 404.
    let feed = feed_in_context(&state.db, context, external_feed_id).await?;
    let Some(feed) = feed else {
        return Err(ApiError::NotFound);
    };

    sqlx::query("DELETE FROM external_feeds WHERE id = $1")
        .bind(feed.id)
        .execute(&state.db)
        .await?;

    Ok(Json(feed_json(&state.db, &feed).await?).into_response())
}

// ---------------------------------------------------------------------------
// Guards
// ---------------------------------------------------------------------------

/// `require_context` — the course or the group has to exist.
async fn require_context(db: &PgPool, context: FeedContext) -> Result<(), ApiError> {
    let (table, id) = match context {
        FeedContext::Course(id) => ("courses", id),
        FeedContext::Group(id) => ("groups", id),
    };
    let sql = format!("SELECT id FROM {table} WHERE id = $1 AND workflow_state <> 'deleted'");
    let row: Option<(i64,)> = sqlx::query_as(&sql).bind(id).fetch_optional(db).await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// The caller has to be able to create an announcement in the context.
async fn require_announcement_right(
    db: &PgPool,
    auth: &AuthUser,
    context: FeedContext,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }
    let row: Option<(i32,)> = match context {
        FeedContext::Course(course_id) => {
            sqlx::query_as(
                "SELECT 1 FROM enrollments \
             WHERE course_id = $1 AND user_id = $2 AND role = 'teacher' LIMIT 1",
            )
            .bind(course_id)
            .bind(auth.id)
            .fetch_optional(db)
            .await?
        }
        FeedContext::Group(group_id) => {
            sqlx::query_as(
                "SELECT 1 FROM group_memberships \
             WHERE group_id = $1 AND user_id = $2 AND workflow_state <> 'deleted' LIMIT 1",
            )
            .bind(group_id)
            .bind(auth.id)
            .fetch_optional(db)
            .await?
        }
    };
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// The feeds of a context.
async fn feeds_of(db: &PgPool, context: FeedContext) -> Result<Vec<ExternalFeed>, ApiError> {
    let sql = format!(
        "SELECT {FEED_COLUMNS} FROM external_feeds \
         WHERE context_type = $1 AND context_id = $2 ORDER BY id ASC"
    );
    sqlx::query_as(&sql)
        .bind(context.context_type())
        .bind(context.context_id())
        .fetch_all(db)
        .await
        .map_err(Into::into)
}

/// One feed of a context.
async fn feed_in_context(
    db: &PgPool,
    context: FeedContext,
    external_feed_id: i64,
) -> Result<Option<ExternalFeed>, ApiError> {
    let sql = format!(
        "SELECT {FEED_COLUMNS} FROM external_feeds \
         WHERE id = $1 AND context_type = $2 AND context_id = $3"
    );
    sqlx::query_as(&sql)
        .bind(external_feed_id)
        .bind(context.context_type())
        .bind(context.context_id())
        .fetch_optional(db)
        .await
        .map_err(Into::into)
}

/// One feed of an id.
async fn load_feed(db: &PgPool, id: i64) -> Result<ExternalFeed, ApiError> {
    let sql = format!("SELECT {FEED_COLUMNS} FROM external_feeds WHERE id = $1");
    sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::ExternalFeeds#external_feed_api_json`.
async fn feed_json(db: &PgPool, feed: &ExternalFeed) -> Result<Value, ApiError> {
    // The number of the entries the feed pulled.
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM external_feed_entries WHERE external_feed_id = $1")
            .bind(feed.id)
            .fetch_one(db)
            .await?;

    // Canvas shows the title of the feed; until the feed was pulled a
    // temporary name is synthesized from the URL.
    let display_name = feed
        .title
        .clone()
        .unwrap_or_else(|| format!("Feed {:?}", feed.url));

    Ok(json!({
        "id": feed.id,
        "url": feed.url,
        "header_match": feed.header_match,
        "created_at": feed.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "verbosity": feed.verbosity,
        "display_name": display_name,
        "external_feed_entries_count": count.0,
    }))
}
