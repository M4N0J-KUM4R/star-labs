//! star-media — Canvas media objects and tracks (`media_objects_api`).
//!
//! Endpoints:
//! - GET    /media_attachments                           — list the caller's media objects
//! - POST   /media_attachments                           — create a media object (metadata only)
//! - PUT    /media_attachments/{id}                      — set `user_entered_title`
//! - GET    /media_objects/{id}                          — show media object (info alias)
//! - PUT    /media_objects/{id}                          — update `user_entered_title`
//! - POST   /media_objects/{id}/media_tracks             — create a caption track
//! - GET    /media_objects/{id}/media_tracks             — list tracks with `include[]` fields
//! - PUT    /media_objects/{id}/media_tracks             — bulk replace tracks
//! - DELETE /media_objects/{id}/media_tracks/{track_id}  — delete a track
//! - POST   /media_objects/{id}/asr                      — initiate ASR caption request
//!
//! Metadata only: no binary upload or Kaltura proxy. `media_sources` is always
//! an empty array and `embedded_iframe_url` / `auto_caption_status` are not
//! returned because no transcoding backend is configured.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
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

/// `POST /media_objects/{id}/media_tracks` body.
#[derive(Debug, Deserialize)]
pub struct CreateMediaTrack {
    pub kind: Option<String>,
    pub locale: String,
    pub content: Option<String>,
}

/// `POST /media_objects/{id}/asr` body.
#[derive(Debug, Deserialize)]
pub struct CreateAsrTrack {
    pub locale: String,
}

/// `PUT /media_objects/{id}/media_tracks` body — bulk track replacement.
#[derive(Debug, Deserialize)]
pub struct BulkTrackUpdate {
    pub locale: String,
    pub content: Option<String>,
    pub kind: Option<String>,
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
            .map(|track| {
                let asr = track.is_asr();
                MediaTrackSummary {
                    id: track.id,
                    kind: track.kind,
                    locale: track.locale,
                    workflow_state: track.workflow_state,
                    asr,
                    created_at: track.created_at,
                    updated_at: track.updated_at,
                }
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

/// Validate locale: must match `[A-Za-z-]+` (Canvas `create_asr` validation).
fn is_valid_locale(locale: &str) -> bool {
    !locale.is_empty()
        && locale
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Validate track kind (Canvas accepts: subtitles | captions | descriptions |
/// chapters | metadata).
fn is_valid_kind(kind: &str) -> bool {
    matches!(
        kind,
        "subtitles" | "captions" | "descriptions" | "chapters" | "metadata"
    )
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

/// GET /media_objects/{id} — show media object (Canvas `/info` alias).
async fn show_media_object(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<MediaObjectJson>, ApiError> {
    let row = fetch_media_object(&state.db, id).await?;
    let tracks = load_tracks(&state.db, row.id).await?;
    to_json(&auth, row, tracks)
}

/// PUT /media_objects/{id} — update `user_entered_title` (owner/admin).
#[allow(clippy::result_large_err)]
async fn update_media_object(
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

/// POST /media_objects/{id}/media_tracks — create a caption track.
async fn create_media_track(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateMediaTrack>,
) -> Result<Json<MediaObjectJson>, ApiError> {
    let media_object = fetch_media_object(&state.db, id).await?;

    // Authorization: owner or admin can add captions.
    if media_object.user_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let locale = &payload.locale;
    if !is_valid_locale(locale) {
        return Err(ApiError::Validation(
            "Invalid or missing locale parameter".into(),
        ));
    }

    let kind = payload.kind.as_deref().unwrap_or("subtitles");
    if !is_valid_kind(kind) {
        return Err(ApiError::Validation(
            "Invalid or missing kind parameter".into(),
        ));
    }

    // Check for existing track for this locale under this user.
    let existing: Option<MediaTrack> = sqlx::query_as(
        r#"
        SELECT id, media_object_id, user_id, kind, locale, content, external_id,
               workflow_state, created_at, updated_at
        FROM media_tracks
        WHERE media_object_id = $1 AND user_id = $2 AND locale = $3
          AND workflow_state <> 'deleted'
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .bind(locale)
    .fetch_optional(&state.db)
    .await?;

    let track_id = if let Some(existing) = existing {
        sqlx::query(
            r#"
            UPDATE media_tracks
            SET kind = $1, content = $2, updated_at = now()
            WHERE id = $3
            "#,
        )
        .bind(kind)
        .bind(&payload.content)
        .bind(existing.id)
        .execute(&state.db)
        .await?;
        existing.id
    } else {
        let inserted: MediaTrack = sqlx::query_as(
            r#"
            INSERT INTO media_tracks (media_object_id, user_id, kind, locale, content)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, media_object_id, user_id, kind, locale, content, external_id,
                      workflow_state, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(auth.id)
        .bind(kind)
        .bind(locale)
        .bind(&payload.content)
        .fetch_one(&state.db)
        .await?;
        inserted.id
    };

    // Return full media object JSON as Canvas does.
    let tracks = load_tracks(&state.db, id).await?;
    to_json(&auth, media_object, tracks)
}

/// GET /media_objects/{id}/media_tracks — list tracks with `include[]` fields.
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

/// PUT /media_objects/{id}/media_tracks — bulk replace tracks.
async fn update_media_tracks(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<HashMap<String, String>>,
    auth: AuthUser,
    Json(payload): Json<Vec<BulkTrackUpdate>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let media_object = fetch_media_object(&state.db, id).await?;

    if media_object.user_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Collect new locales.
    let new_locales: Vec<&str> = payload.iter().map(|t| t.locale.as_str()).collect();

    // Soft-delete removed tracks.
    if !new_locales.is_empty() {
        sqlx::query(
            r#"
            UPDATE media_tracks
            SET workflow_state = 'deleted', updated_at = now()
            WHERE media_object_id = $1
              AND user_id = $2
              AND workflow_state <> 'deleted'
              AND locale <> ALL($3)
            "#,
        )
        .bind(id)
        .bind(auth.id)
        .bind(&new_locales)
        .execute(&state.db)
        .await?;
    }

    // Upsert incoming tracks.
    for track in &payload {
        let kind = track.kind.as_deref().unwrap_or("subtitles");
        let content = track.content.as_deref().filter(|c| !c.trim().is_empty());

        sqlx::query(
            r#"
            INSERT INTO media_tracks (media_object_id, user_id, kind, locale, content)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (media_object_id, user_id, locale)
              WHERE workflow_state <> 'deleted'
            DO UPDATE SET kind = EXCLUDED.kind,
                          content = COALESCE(EXCLUDED.content, media_tracks.content),
                          updated_at = now()
            "#,
        )
        .bind(id)
        .bind(auth.id)
        .bind(kind)
        .bind(&track.locale)
        .bind(content)
        .execute(&state.db)
        .await?;
    }

    // Re-fetch tracks for response.
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
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    let include = query
        .get("include[]")
        .or_else(|| query.get("include"))
        .map(|raw| {
            raw.split(',')
                .map(|value| value.trim().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

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

/// DELETE /media_objects/{id}/media_tracks/{track_id} — delete a track.
async fn delete_media_track(
    State(state): State<AppState>,
    Path((media_object_id, track_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<MediaObjectJson>, ApiError> {
    let media_object = fetch_media_object(&state.db, media_object_id).await?;

    if media_object.user_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let deleted = sqlx::query(
        r#"
        UPDATE media_tracks
        SET workflow_state = 'deleted', updated_at = now()
        WHERE id = $1 AND media_object_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(track_id)
    .bind(media_object_id)
    .execute(&state.db)
    .await?;

    if deleted.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    let tracks = load_tracks(&state.db, media_object_id).await?;
    to_json(&auth, media_object, tracks)
}

/// POST /media_objects/{id}/asr — initiate ASR caption request.
/// No-op in metadata-only mode: creates a track with external_id set but no
/// content (Canvas `create_asr` with no Kaltura).
async fn create_asr_track(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateAsrTrack>,
) -> Result<Json<MediaObjectJson>, ApiError> {
    let media_object = fetch_media_object(&state.db, id).await?;

    if media_object.user_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let locale = &payload.locale;
    if !is_valid_locale(locale) {
        return Err(ApiError::Validation(
            "Invalid or missing locale parameter".into(),
        ));
    }

    // Check for existing processing track.
    let existing: Option<MediaTrack> = sqlx::query_as(
        r#"
        SELECT id, media_object_id, user_id, kind, locale, content, external_id,
               workflow_state, created_at, updated_at
        FROM media_tracks
        WHERE media_object_id = $1 AND user_id = $2 AND locale = $3
          AND workflow_state <> 'deleted' AND external_id IS NOT NULL
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .bind(locale)
    .fetch_optional(&state.db)
    .await?;

    if existing.is_some() {
        // Already has a processing/generated track for this locale — skip.
        let tracks = load_tracks(&state.db, id).await?;
        return to_json(&auth, media_object, tracks);
    }

    // Insert a placeholder track with external_id = "asr.<locale>" and empty
    // content; workflow_state = "processing".
    sqlx::query(
        r#"
        INSERT INTO media_tracks (media_object_id, user_id, kind, locale, content, external_id, workflow_state)
        VALUES ($1, $2, 'subtitles', $3, '', 'asr.', 'processing')
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .bind(locale)
    .execute(&state.db)
    .await?;

    let tracks = load_tracks(&state.db, id).await?;
    to_json(&auth, media_object, tracks)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/media_attachments", get(list_media_attachments))
        .route("/media_attachments", post(create_media_attachment))
        .route("/media_attachments/{id}", put(update_media_attachment))
        .route("/media_objects/{id}", get(show_media_object))
        .route("/media_objects/{id}", put(update_media_object))
        .route("/media_objects/{id}/media_tracks", post(create_media_track))
        .route("/media_objects/{id}/media_tracks", get(list_media_tracks))
        .route("/media_objects/{id}/media_tracks", put(update_media_tracks))
        .route(
            "/media_objects/{id}/media_tracks/{track_id}",
            delete(delete_media_track),
        )
        .route("/media_objects/{id}/asr", post(create_asr_track))
}
