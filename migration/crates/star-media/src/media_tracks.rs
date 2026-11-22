//! media_tracks — Canvas `MediaTracksController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET  /media_objects/{mediaObjectId}/media_tracks` — the tracks of a media object
//! - `PUT  /media_objects/{mediaObjectId}/media_tracks` — replace the tracks
//! - `POST /media_objects/{mediaObjectId}/asr` — ask for an ASR caption
//! - `GET  /media_attachments/{attachmentId}/media_tracks` — the tracks of an attachment
//! - `PUT  /media_attachments/{attachmentId}/media_tracks` — replace the tracks
//! - `POST /media_attachments/{attachmentId}/asr` — ask for an ASR caption
//!
//! Canvas routes (config/routes.rb, the account API block):
//!   get  "media_objects/:media_object_id/media_tracks", action: "index"
//!   put  "media_objects/:media_object_id/media_tracks", action: "update"
//!   post "media_objects/:media_object_id/asr", action: "create_asr"
//!   get  "media_attachments/:attachment_id/media_tracks", action: "index"
//!   put  "media_attachments/:attachment_id/media_tracks", action: "update"
//!   post "media_attachments/:attachment_id/asr", action: "create_asr"
//!
//! Canvas controller (`app/controllers/media_tracks_controller.rb`): the
//! `before_action` chain is `load_media_object` (404 when the media object
//! or the attachment does not exist) and `check_media_permissions` — the
//! `:read` right for the listing, the `:update` right for the writes.
//!
//! The track JSON is the Canvas `Api::V1::MediaTrack#media_track_api_json`
//! shape: `id`, `locale`, `kind`, `workflow_state`, `asr`, `media_object_id`
//! and `user_id`, plus the `include[]` fields (`content`, `webvtt_content`,
//! `updated_at`, `created_at`).

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::media_objects::{can_add_captions, can_read_context, MediaObjectRow};

/// A `media_tracks` row.
#[derive(Debug, sqlx::FromRow)]
struct TrackRow {
    id: i64,
    media_object_id: i64,
    user_id: i64,
    kind: String,
    locale: String,
    content: Option<String>,
    external_id: Option<String>,
    workflow_state: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// The query parameters of the listing.
#[derive(Debug, Default, Deserialize)]
struct TrackQuery {
    include: Option<String>,
}

/// The body of the track update — the whole set of tracks of the media
/// object.
#[derive(Debug, Default, Deserialize)]
struct TrackUpdate {
    locale: Option<String>,
    kind: Option<String>,
    content: Option<String>,
}

/// The body of the ASR request.
#[derive(Debug, Default, Deserialize)]
struct AsrRequest {
    locale: Option<String>,
}

/// Domain router for the Canvas media-track routes.
///
/// The listing of the tracks of a media object is registered by the crate
/// root; this router adds the writes and the attachment routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/media_objects/{id}/media_tracks",
            axum::routing::put(update_object_tracks),
        )
        .route(
            "/media_objects/{media_object_id}/asr",
            axum::routing::post(create_object_asr),
        )
        .route(
            "/media_attachments/{attachment_id}/media_tracks",
            get(index_attachment_tracks).put(update_attachment_tracks),
        )
        .route(
            "/media_attachments/{attachment_id}/asr",
            axum::routing::post(create_attachment_asr),
        )
}

/// The media object a track route addresses — 404 when it does not exist.
async fn load_media_object(
    db: &PgPool,
    media_object_id: i64,
    attachment_id: Option<i64>,
) -> Result<MediaObjectRow, ApiError> {
    let row: Option<MediaObjectRow> = sqlx::query_as(
        "SELECT id, media_id, media_type, title, user_entered_title, user_id, workflow_state, \
                context_type, context_id, old_media_id, attachment_id, viewer_restrictions, \
                auto_caption_status, created_at, updated_at \
         FROM media_objects WHERE id = $1",
    )
    .bind(media_object_id)
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };
    // An attachment route addresses the media object the attachment belongs
    // to.
    if let Some(attachment_id) = attachment_id {
        if row.attachment_id != Some(attachment_id) {
            return Err(ApiError::NotFound);
        }
    }
    Ok(row)
}

/// `GET /media_attachments/:attachment_id/media_tracks` — media_tracks#index.
async fn index_attachment_tracks(
    State(state): State<AppState>,
    Path(attachment_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<TrackQuery>,
) -> Result<Response, ApiError> {
    index_tracks(&state, attachment_id, Some(attachment_id), &auth, &params).await
}

/// The shared listing of the tracks of a media object.
async fn index_tracks(
    state: &AppState,
    media_object_id: i64,
    attachment_id: Option<i64>,
    auth: &AuthUser,
    params: &TrackQuery,
) -> Result<Response, ApiError> {
    let media_object = load_media_object(&state.db, media_object_id, attachment_id).await?;
    require_read(&state.db, auth, &media_object).await?;

    // Canvas lists the tracks of the caller.
    let rows: Vec<TrackRow> = sqlx::query_as(
        "SELECT id, media_object_id, user_id, kind, locale, content, external_id, \
                workflow_state, created_at, updated_at \
         FROM media_tracks WHERE media_object_id = $1 AND user_id = $2 \
         ORDER BY locale ASC, id ASC",
    )
    .bind(media_object.id)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    let include = parse_include(params.include.as_deref());
    let items = rows
        .iter()
        .map(|row| track_json(row, &include))
        .collect::<Vec<Value>>();
    Ok(Json(items).into_response())
}

/// `PUT /media_objects/:media_object_id/media_tracks` — media_tracks#update.
async fn update_object_tracks(
    State(state): State<AppState>,
    Path(media_object_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Vec<TrackUpdate>>>,
) -> Result<Response, ApiError> {
    update_tracks(&state, media_object_id, None, &auth, body).await
}

/// `PUT /media_attachments/:attachment_id/media_tracks` — media_tracks#update.
async fn update_attachment_tracks(
    State(state): State<AppState>,
    Path(attachment_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<Vec<TrackUpdate>>>,
) -> Result<Response, ApiError> {
    update_tracks(&state, attachment_id, Some(attachment_id), &auth, body).await
}

/// The shared replacement of the tracks of a media object.
///
/// Canvas removes the tracks whose locale is not in the new set and creates
/// or updates the ones that are; a track without content is left alone.
async fn update_tracks(
    state: &AppState,
    media_object_id: i64,
    attachment_id: Option<i64>,
    auth: &AuthUser,
    body: Option<Json<Vec<TrackUpdate>>>,
) -> Result<Response, ApiError> {
    let media_object = load_media_object(&state.db, media_object_id, attachment_id).await?;
    require_update(&state.db, auth, &media_object).await?;

    let tracks = body.map(|Json(tracks)| tracks).unwrap_or_default();

    // The locales of the new set.
    let new_locales = tracks
        .iter()
        .filter_map(|track| track.locale.clone())
        .collect::<Vec<String>>();

    // Remove the tracks that are not in the new set.
    let existing: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, locale FROM media_tracks WHERE media_object_id = $1")
            .bind(media_object.id)
            .fetch_all(&state.db)
            .await?;
    for (id, locale) in existing {
        if !new_locales.contains(&locale) {
            sqlx::query("DELETE FROM media_tracks WHERE id = $1")
                .bind(id)
                .execute(&state.db)
                .await?;
        }
    }

    // Create or update the new tracks.
    for track in &tracks {
        // A track without content has not been updated.
        let Some(content) = track
            .content
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let Some(locale) = track.locale.as_ref() else {
            continue;
        };
        let kind = track.kind.as_deref().unwrap_or("subtitles");
        let existing: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM media_tracks WHERE media_object_id = $1 AND locale = $2 LIMIT 1",
        )
        .bind(media_object.id)
        .bind(locale)
        .fetch_optional(&state.db)
        .await?;
        match existing {
            Some((id,)) => {
                sqlx::query(
                    "UPDATE media_tracks SET kind = $1, content = $2, updated_at = now() \
                     WHERE id = $3",
                )
                .bind(kind)
                .bind(content)
                .bind(id)
                .execute(&state.db)
                .await?;
            }
            None => {
                sqlx::query(
                    "INSERT INTO media_tracks (media_object_id, user_id, kind, locale, content) \
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(media_object.id)
                .bind(auth.id)
                .bind(kind)
                .bind(locale)
                .bind(content)
                .execute(&state.db)
                .await?;
            }
        }
    }

    // Canvas answers the listing of the tracks.
    let rows: Vec<TrackRow> = sqlx::query_as(
        "SELECT id, media_object_id, user_id, kind, locale, content, external_id, \
                workflow_state, created_at, updated_at \
         FROM media_tracks WHERE media_object_id = $1 AND user_id = $2 \
         ORDER BY locale ASC, id ASC",
    )
    .bind(media_object.id)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;
    let items = rows
        .iter()
        .map(|row| track_json(row, &[]))
        .collect::<Vec<Value>>();
    Ok(Json(items).into_response())
}

/// `POST /media_objects/:media_object_id/asr` — media_tracks#create_asr.
async fn create_object_asr(
    State(state): State<AppState>,
    Path(media_object_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<AsrRequest>>,
) -> Result<Response, ApiError> {
    create_asr(&state, media_object_id, None, &auth, body).await
}

/// `POST /media_attachments/:attachment_id/asr` — media_tracks#create_asr.
async fn create_attachment_asr(
    State(state): State<AppState>,
    Path(attachment_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<AsrRequest>>,
) -> Result<Response, ApiError> {
    create_asr(&state, attachment_id, Some(attachment_id), &auth, body).await
}

/// The shared ASR caption request.
///
/// Canvas needs the `rce_asr_captioning_improvements` feature flag, the
/// `:add_captions` right and a media object that has been processed; the
/// caption asset itself is created by the Kaltura client, which the
/// workspace does not have.
async fn create_asr(
    state: &AppState,
    media_object_id: i64,
    attachment_id: Option<i64>,
    auth: &AuthUser,
    body: Option<Json<AsrRequest>>,
) -> Result<Response, ApiError> {
    let media_object = load_media_object(&state.db, media_object_id, attachment_id).await?;
    require_update(&state.db, auth, &media_object).await?;

    let locale = body
        .and_then(|Json(body)| body.locale)
        .unwrap_or_default();
    if locale.is_empty() || !locale.chars().all(|c| c.is_ascii_alphabetic() || c == '-') {
        return Err(ApiError::BadRequest(
            "Invalid or missing locale parameter".to_string(),
        ));
    }

    // Canvas creates the caption asset with the Kaltura client; the
    // workspace has no Kaltura, so the caption cannot be created.
    Err(ApiError::Db("Failed to create caption asset".to_string()))
}

/// The `:read` right on the context of the media object.
async fn require_read(
    db: &PgPool,
    auth: &AuthUser,
    media_object: &MediaObjectRow,
) -> Result<(), ApiError> {
    if !can_read_context(db, auth, &context_of(media_object)).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

/// The `:update` right on the media object — the caller has to be able to
/// add captions.
async fn require_update(
    db: &PgPool,
    auth: &AuthUser,
    media_object: &MediaObjectRow,
) -> Result<(), ApiError> {
    if !can_add_captions(db, auth, media_object).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

/// The context of a media object, resolved from its `context_type` and
/// `context_id`.
fn context_of(media_object: &MediaObjectRow) -> crate::media_objects::MediaContext {
    crate::media_objects::context_of(media_object)
}

/// The `include[]` parameter: the extra fields of the track JSON.
fn parse_include(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw.filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    raw.split(',')
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

/// Canvas `Api::V1::MediaTrack#media_track_api_json`.
fn track_json(row: &TrackRow, include: &[String]) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("id".to_string(), json!(row.id));
    object.insert("locale".to_string(), json!(row.locale));
    object.insert("kind".to_string(), json!(row.kind));
    object.insert("workflow_state".to_string(), json!(row.workflow_state));
    // Canvas `MediaTrack#asr?` — the track carries the id of a caption
    // asset that Kaltura created.
    object.insert("asr".to_string(), json!(row.external_id.is_some()));
    object.insert("media_object_id".to_string(), json!(row.media_object_id));
    object.insert("user_id".to_string(), json!(row.user_id));

    for field in include {
        match field.as_str() {
            "content" => {
                object.insert(
                    "content".to_string(),
                    match &row.content {
                        Some(content) => json!(content),
                        None => Value::Null,
                    },
                );
            }
            "webvtt_content" => {
                object.insert(
                    "webvtt_content".to_string(),
                    match &row.content {
                        Some(content) => json!(webvtt_content(content)),
                        None => Value::Null,
                    },
                );
            }
            "updated_at" => {
                object.insert(
                    "updated_at".to_string(),
                    json!(row
                        .updated_at
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                );
            }
            "created_at" => {
                object.insert(
                    "created_at".to_string(),
                    json!(row
                        .created_at
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                );
            }
            _ => {}
        }
    }

    Value::Object(object)
}

/// Canvas `MediaTrack#webvtt_content` — the SRT content in the WebVTT
/// format.
fn webvtt_content(content: &str) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for line in content.lines() {
        // The SRT timestamps use a comma, the WebVTT ones a period.
        if let Some((head, tail)) = line.split_once("-->") {
            let head = head.trim_end();
            let tail = tail.trim_start();
            let head = head.replace(',', ".");
            out.push_str(&format!("{head} --> {tail}\n"));
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}
