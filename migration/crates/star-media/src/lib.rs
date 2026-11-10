//! star-media — Canvas media objects and tracks (`media_objects_api`).
//!
//! Endpoints:
//! - GET  /media_attachments                          — list the caller's media objects
//! - POST /media_attachments                          — create a media object (metadata only)
//! - PUT  /media_attachments/{id}                     — set `user_entered_title`
//! - GET  /media_objects/{id}/media_tracks            — list tracks with `include[]` fields
//!
//! Metadata only: no binary upload or Kaltura proxy. `media_sources` is always
//! an empty array and `embedded_iframe_url` / `auto_caption_status` are not
//! returned because no transcoding backend is configured.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Row shape of `media_objects`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MediaObject {
    pub id: i64,
    pub media_id: String,
    pub media_type: String,
    pub title: Option<String>,
    pub user_entered_title: Option<String>,
    pub user_id: i64,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Track summary embedded in a media object payload (Canvas
/// `media_object_api_json`).
#[derive(Debug, Clone, Serialize)]
pub struct MediaTrackSummary {
    pub id: i64,
    pub kind: String,
    pub locale: String,
    pub workflow_state: String,
    pub asr: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Canvas `MediaObject` response shape (API v1).
#[derive(Debug, Clone, Serialize)]
pub struct MediaObjectJson {
    pub id: i64,
    pub media_id: String,
    pub media_type: String,
    pub title: String,
    pub user_entered_title: Option<String>,
    pub can_add_captions: bool,
    pub media_sources: Vec<serde_json::Value>,
    pub media_tracks: Vec<MediaTrackSummary>,
    pub viewer_restrictions: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Row shape of `media_tracks`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MediaTrack {
    pub id: i64,
    pub media_object_id: i64,
    pub user_id: i64,
    pub kind: String,
    pub locale: String,
    pub content: Option<String>,
    pub external_id: Option<String>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl MediaTrack {
    /// Canvas `MediaTrack#asr?`.
    fn is_asr(&self) -> bool {
        self.kind == "subtitles" && self.external_id.is_some()
    }
}

/// `GET /media_attachments` query parameters (Canvas `sort` / `order`).
#[derive(Debug, Deserialize, Default)]
pub struct ListParams {
    pub sort: Option<String>,
    pub order: Option<String>,
}

/// `POST /media_attachments` body.
#[derive(Debug, Deserialize)]
pub struct CreateMediaObject {
    pub media_id: String,
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub user_entered_title: Option<String>,
}

/// `PUT /media_attachments/{id}` body.
#[derive(Debug, Deserialize)]
pub struct UpdateMediaObject {
    pub user_entered_title: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas `MediaObject#guaranteed_title`.
fn guaranteed_title(row: &MediaObject) -> String {
    row.user_entered_title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            row.title
                .as_deref()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or("Untitled")
        .to_string()
}

/// Canvas truncates titles with `CanvasTextHelper.truncate_text(max: 255)`.
fn truncate_title(value: &str) -> String {
    value.chars().take(255).collect()
}

async fn load_tracks(pool: &PgPool, media_object_id: i64) -> Result<Vec<MediaTrack>, ApiError> {
    let rows: Vec<MediaTrack> = sqlx::query_as(
        r#"
        SELECT id, media_object_id, user_id, kind, locale, content, external_id,
               workflow_state, created_at, updated_at
        FROM media_tracks
        WHERE media_object_id = $1 AND workflow_state <> 'deleted'
        ORDER BY id ASC
        "#,
    )
    .bind(media_object_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

fn to_json(
    auth: &AuthUser,
    row: MediaObject,
    tracks: Vec<MediaTrack>,
) -> Result<MediaObjectJson, ApiError> {
    let can_add_captions = row.user_id == auth.id || is_admin(auth);
    let title = guaranteed_title(&row);
    Ok(MediaObjectJson {
        id: row.id,
        media_id: row.media_id,
        media_type: row.media_type,
        title,
        user_entered_title: row.user_entered_title,
        can_add_captions,
        media_sources: Vec::new(),
        media_tracks: tracks
            .into_iter()
            .map(|track| MediaTrackSummary {
                id: track.id,
                kind: track.kind,
                locale: track.locale,
                workflow_state: track.workflow_state,
                asr: track.is_asr(),
                created_at: track.created_at,
                updated_at: track.updated_at,
            })
            .collect(),
        viewer_restrictions: serde_json::json!({}),
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

async fn fetch_media_object(pool: &PgPool, id: i64) -> Result<MediaObject, ApiError> {
    let row: Option<MediaObject> = sqlx::query_as(
        r#"
        SELECT id, media_id, media_type, title, user_entered_title, user_id,
               workflow_state, created_at, updated_at
        FROM media_objects
        WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /media_attachments — media objects owned by the caller.
async fn list_media_attachments(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
    auth: AuthUser,
) -> Result<Json<Vec<MediaObjectJson>>, ApiError> {
    // Canvas: sort defaults to title (user_entered_title, falling back to
    // title) ascending; `created_at` and `order=desc` are supported.
    let sort = match params.sort.as_deref() {
        Some("created_at") => "created_at",
        _ => "COALESCE(user_entered_title, title)",
    };
    let order = if params.order.as_deref() == Some("desc") {
        "DESC"
    } else {
        "ASC"
    };
    let sql = format!(
        r#"
        SELECT id, media_id, media_type, title, user_entered_title, user_id,
               workflow_state, created_at, updated_at
        FROM media_objects
        WHERE user_id = $1 AND workflow_state <> 'deleted'
        ORDER BY {sort} {order}, id ASC
        "#
    );
    let rows: Vec<MediaObject> = sqlx::query_as(&sql)
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?;

    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let tracks = load_tracks(&state.db, row.id).await?;
        result.push(to_json(&auth, row, tracks)?);
    }
    Ok(Json(result))
}

/// POST /media_attachments — create or refresh a metadata-only media object.
async fn create_media_attachment(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateMediaObject>,
) -> Result<(StatusCode, Json<MediaObjectJson>), ApiError> {
    if payload.media_id.trim().is_empty() {
        return Err(ApiError::Validation("media_id cannot be empty".into()));
    }
    let media_type = payload.media_type.as_deref().unwrap_or("video");
    let title = payload.title.as_deref().map(truncate_title);
    let user_entered_title = payload
        .user_entered_title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(truncate_title);

    // Canvas `first_or_initialize` on (media_id, media_type, context).
    let row: MediaObject = sqlx::query_as(
        r#"
        INSERT INTO media_objects (media_id, media_type, title, user_entered_title, user_id)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (media_id) DO UPDATE
        SET media_type = EXCLUDED.media_type,
            title = COALESCE(EXCLUDED.title, media_objects.title),
            user_entered_title = COALESCE(EXCLUDED.user_entered_title, media_objects.user_entered_title),
            updated_at = now()
        RETURNING id, media_id, media_type, title, user_entered_title, user_id,
                  workflow_state, created_at, updated_at
        "#,
    )
    .bind(&payload.media_id)
    .bind(media_type)
    .bind(&title)
    .bind(&user_entered_title)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    let tracks = load_tracks(&state.db, row.id).await?;
    Ok((StatusCode::CREATED, Json(to_json(&auth, row, tracks)?)))
}

/// PUT /media_attachments/{id} — update `user_entered_title` (owner/admin).
#[allow(clippy::result_large_err)]
async fn update_media_attachment(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateMediaObject>,
) -> Result<Json<MediaObjectJson>, Response> {
    let existing = fetch_media_object(&state.db, id)
        .await
        .map_err(|e| e.into_response())?;

    if existing.user_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden.into_response());
    }

    // Canvas: blank `user_entered_title` is a 400 with a JSON message body.
    let title = payload
        .user_entered_title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(truncate_title);
    let Some(title) = title else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "The user_entered_title parameter must have a value"
            })),
        )
            .into_response());
    };

    let row: MediaObject = sqlx::query_as(
        r#"
        UPDATE media_objects
        SET user_entered_title = $1, updated_at = now()
        WHERE id = $2 AND workflow_state <> 'deleted'
        RETURNING id, media_id, media_type, title, user_entered_title, user_id,
                  workflow_state, created_at, updated_at
        "#,
    )
    .bind(&title)
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(|e| ApiError::from(e).into_response())?;

    let tracks = load_tracks(&state.db, row.id)
        .await
        .map_err(|e| e.into_response())?;
    to_json(&auth, row, tracks)
        .map(Json)
        .map_err(|e| e.into_response())
}

/// GET /media_objects/{id}/media_tracks — list tracks (owner/admin).
///
/// Canvas media-object track listing is scoped to `user_id = current_user`;
/// `include[]=content|webvtt_content|updated_at|created_at` adds those fields.
async fn list_media_tracks(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let media_object = fetch_media_object(&state.db, id).await?;

    if media_object.user_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let include = query
        .get("include[]")
        .or_else(|| query.get("include"))
        .map(|raw| {
            raw.split(',')
                .map(|value| value.trim().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let rows: Vec<MediaTrack> = sqlx::query_as(
        r#"
        SELECT id, media_object_id, user_id, kind, locale, content, external_id,
               workflow_state, created_at, updated_at
        FROM media_tracks
        WHERE media_object_id = $1
          AND user_id = $2
          AND workflow_state <> 'deleted'
        ORDER BY id ASC
        "#,
    )
    .bind(id)
    .bind(media_object.user_id)
    .fetch_all(&state.db)
    .await?;

    let result = rows
        .into_iter()
        .map(|track| {
            let mut json = serde_json::json!({
                "id": track.id,
                "locale": track.locale,
                "kind": track.kind,
                "workflow_state": track.workflow_state,
                "asr": track.is_asr(),
                "media_object_id": track.media_object_id,
                "user_id": track.user_id,
            });
            if include.iter().any(|field| field == "content") {
                json["content"] = serde_json::json!(track.content);
            }
            if include.iter().any(|field| field == "created_at") {
                json["created_at"] = serde_json::json!(track.created_at);
            }
            if include.iter().any(|field| field == "updated_at") {
                json["updated_at"] = serde_json::json!(track.updated_at);
            }
            json
        })
        .collect();

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/media_attachments", get(list_media_attachments))
        .route("/media_attachments", post(create_media_attachment))
        .route("/media_attachments/{id}", put(update_media_attachment))
        .route("/media_objects/{id}/media_tracks", get(list_media_tracks))
}
