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

/// One entry of the `PUT .../media_tracks` batch body (Canvas `update` reads
/// a JSON array of tracks).
#[derive(Debug, Deserialize)]
pub struct TrackInput {
    pub kind: Option<String>,
    pub locale: String,
    #[serde(default)]
    pub content: Option<String>,
}

/// `POST .../asr` body.
#[derive(Debug, Deserialize)]
pub struct AsrParams {
    pub locale: String,
}

/// `POST /media_objects` context reference (Canvas `context_code`, e.g.
/// `course_123` or `group_45`).
#[derive(Debug, Deserialize)]
pub struct CreateMediaObjectWithContext {
    pub media_id: String,
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub user_entered_title: Option<String>,
    pub context_code: Option<String>,
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
// Context-scoped index (Canvas `index` with course/group context)
// ---------------------------------------------------------------------------

/// `media_objects` reached through a context's media attachments: the
/// Canvas `Attachment.not_deleted.is_media_object.where(context:)` join.
/// star's attachments live in the Canvas-parity `attachments` table
/// (migration 0223) with `media_entry_id` pointing back at `media_id`.
async fn context_media_ids(
    pool: &PgPool,
    context_id: i64,
    context_type: &str,
) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT a.media_entry_id FROM attachments a
        WHERE a.context_id = $1 AND a.context_type = $2
          AND a.media_entry_id IS NOT NULL
          AND (a.file_state IS NULL OR a.file_state <> 'deleted')
          AND (a.workflow_state IS NULL OR a.workflow_state <> 'deleted')
          AND (a.content_type LIKE 'video/%' OR a.content_type LIKE 'audio/%')
        "#,
    )
    .bind(context_id)
    .bind(context_type)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Shared context index: sort/order like the user-scoped listing.
async fn list_context_media_objects(
    state: &AppState,
    params: &ListParams,
    context_id: i64,
    context_type: &str,
    auth: &AuthUser,
) -> Result<Json<Vec<MediaObjectJson>>, ApiError> {
    let media_ids = context_media_ids(&state.db, context_id, context_type).await?;
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
        WHERE media_id = ANY($1) AND workflow_state <> 'deleted'
        ORDER BY {sort} {order} NULLS LAST, id ASC
        "#
    );
    let rows: Vec<MediaObject> = sqlx::query_as(&sql)
        .bind(&media_ids)
        .fetch_all(&state.db)
        .await?;

    let mut result = Vec::with_capacity(rows.len());
    for row in rows {
        let tracks = load_tracks(&state.db, row.id).await?;
        result.push(to_json(auth, row, tracks)?);
    }
    Ok(Json(result))
}

/// GET /courses/{courseId}/media_objects and /media_attachments — Canvas
/// `index` for a course context. Reading the root folder's contents maps to
/// being enrolled in the course (or a site admin).
async fn list_course_media_objects(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<ListParams>,
    auth: AuthUser,
) -> Result<Json<Vec<MediaObjectJson>>, ApiError> {
    let course_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM courses WHERE id = $1)")
            .bind(course_id)
            .fetch_one(&state.db)
            .await?;
    if !course_exists {
        return Err(ApiError::NotFound);
    }

    let enrolled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2)",
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    if !enrolled && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    list_context_media_objects(&state, &params, course_id, "Course", &auth).await
}

/// GET /groups/{groupId}/media_objects and /media_attachments — Canvas
/// `index` for a group context. Reading the group's files maps to group
/// membership (or a site admin).
async fn list_group_media_objects(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(params): Query<ListParams>,
    auth: AuthUser,
) -> Result<Json<Vec<MediaObjectJson>>, ApiError> {
    let group_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups WHERE id = $1)")
            .bind(group_id)
            .fetch_one(&state.db)
            .await?;
    if !group_exists {
        return Err(ApiError::NotFound);
    }

    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM group_memberships WHERE group_id = $1 AND user_id = $2)",
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;
    if !member && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    list_context_media_objects(&state, &params, group_id, "Group", &auth).await
}

/// PUT /media_objects/{id} — Canvas `update_media_object` keyed by the media
/// object id: owner-only (media objects carry no permissions, so Canvas
/// checks `user_id == current_user` with no admin bypass). The response
/// excludes `sources` and `tracks` (Canvas renders with
/// `%w[sources tracks]`).
async fn update_media_object(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateMediaObject>,
) -> Result<Response, ApiError> {
    let existing = fetch_media_object(&state.db, id).await?;

    // Canvas: the media-object variant has no admin escape hatch.
    if existing.user_id != auth.id {
        return Err(ApiError::Forbidden);
    }

    let title = payload
        .user_entered_title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(truncate_title);
    let Some(title) = title else {
        return Ok((
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
    .await?;

    // Canvas excludes `sources` and `tracks` from this response.
    let mut json = serde_json::to_value(MediaObjectJson {
        id: row.id,
        media_id: row.media_id.clone(),
        media_type: row.media_type,
        title: guaranteed_title(&row),
        user_entered_title: row.user_entered_title,
        can_add_captions: true,
        media_sources: Vec::new(),
        media_tracks: Vec::new(),
        viewer_restrictions: serde_json::json!({}),
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
    .map_err(|e| ApiError::Db(e.to_string()))?;
    let object = json
        .as_object_mut()
        .ok_or_else(|| ApiError::Db("serialization".into()))?;
    object.remove("media_sources");
    object.remove("media_tracks");

    Ok(Json(json).into_response())
}

/// POST /media_objects — Canvas `create_media_object`. The optional
/// `context_code` (`course_123` / `group_45`) is resolved and read-checked
/// first; the media object itself is user-scoped in star's metadata-only
/// model, so creation matches `POST /media_attachments`.
async fn create_media_object(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateMediaObjectWithContext>,
) -> Result<(StatusCode, Json<MediaObjectJson>), ApiError> {
    if let Some(context_code) = payload.context_code.as_deref() {
        let Some((kind, id)) = context_code.split_once('_') else {
            return Err(ApiError::NotFound);
        };
        let id: i64 = id.parse().map_err(|_| ApiError::NotFound)?;
        match kind {
            "course" => {
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM courses WHERE id = $1)")
                        .bind(id)
                        .fetch_one(&state.db)
                        .await?;
                if !exists {
                    return Err(ApiError::NotFound);
                }
                let enrolled: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2)",
                )
                .bind(id)
                .bind(auth.id)
                .fetch_one(&state.db)
                .await?;
                if !enrolled && !is_admin(&auth) {
                    return Err(ApiError::Forbidden);
                }
            }
            "group" => {
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM groups WHERE id = $1)")
                        .bind(id)
                        .fetch_one(&state.db)
                        .await?;
                if !exists {
                    return Err(ApiError::NotFound);
                }
                let member: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM group_memberships WHERE group_id = $1 AND user_id = $2)",
                )
                .bind(id)
                .bind(auth.id)
                .fetch_one(&state.db)
                .await?;
                if !member && !is_admin(&auth) {
                    return Err(ApiError::Forbidden);
                }
            }
            _ => return Err(ApiError::NotFound),
        }
    }

    let inner = CreateMediaObject {
        media_id: payload.media_id,
        media_type: payload.media_type,
        title: payload.title,
        user_entered_title: payload.user_entered_title,
    };
    create_media_attachment(State(state), auth, Json(inner)).await
}

// ---------------------------------------------------------------------------
// Media tracks batch update + ASR (Canvas media_tracks#update / #create_asr)
// ---------------------------------------------------------------------------

/// PUT /media_objects/{id}/media_tracks and /media_attachments/{id}/media_tracks
/// — Canvas `update`: the batch replaces the track set. Locales missing from
/// the payload are deleted; entries with content are created or updated
/// (keyed by the caller + locale); entries without content are left alone.
/// The response re-renders the index (the caller's tracks).
async fn update_media_tracks(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(new_tracks): Json<Vec<TrackInput>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let media_object = fetch_media_object(&state.db, id).await?;

    if media_object.user_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Delete the tracks whose locale is absent from the new set (Canvas
    // destroys them across the whole media object).
    let locales: Vec<String> = new_tracks.iter().map(|t| t.locale.clone()).collect();
    sqlx::query(
        r#"
        DELETE FROM media_tracks
        WHERE media_object_id = $1
          AND locale <> ALL($2)
          AND workflow_state <> 'deleted'
        "#,
    )
    .bind(id)
    .bind(&locales)
    .execute(&state.db)
    .await?;

    for track in new_tracks {
        // Canvas skips entries whose content is blank: they were not edited.
        let Some(content) = track.content.as_deref().filter(|c| !c.trim().is_empty()) else {
            continue;
        };
        let kind = track.kind.unwrap_or_else(|| "subtitles".to_string());
        // find_or_create by (media object, caller, locale) like Canvas'
        // `find_or_create_track` for the media-object variant.
        let existing: Option<(i64,)> = sqlx::query_as(
            "SELECT id FROM media_tracks \
             WHERE media_object_id = $1 AND user_id = $2 AND locale = $3 AND workflow_state <> 'deleted'",
        )
        .bind(id)
        .bind(auth.id)
        .bind(&track.locale)
        .fetch_optional(&state.db)
        .await?;
        if existing.is_some() {
            sqlx::query(
                "UPDATE media_tracks SET kind = $3, content = $4, updated_at = now() \
                 WHERE media_object_id = $1 AND user_id = $2 AND locale = $5 AND workflow_state <> 'deleted'",
            )
            .bind(id)
            .bind(auth.id)
            .bind(&kind)
            .bind(content)
            .bind(&track.locale)
            .execute(&state.db)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO media_tracks (media_object_id, user_id, kind, locale, content) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(auth.id)
            .bind(&kind)
            .bind(&track.locale)
            .bind(content)
            .execute(&state.db)
            .await?;
        }
    }

    // Canvas `update` finishes by rendering `index` (the caller's tracks).
    let rows: Vec<MediaTrack> = sqlx::query_as(
        r#"
        SELECT id, media_object_id, user_id, kind, locale, content, external_id,
               workflow_state, created_at, updated_at
        FROM media_tracks
        WHERE media_object_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'
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
            serde_json::json!({
                "id": track.id,
                "locale": track.locale,
                "kind": track.kind,
                "workflow_state": track.workflow_state,
                "asr": track.is_asr(),
                "media_object_id": track.media_object_id,
                "user_id": track.user_id,
            })
        })
        .collect();
    Ok(Json(result))
}

/// POST /media_objects/{id}/asr and /media_attachments/{id}/asr — Canvas
/// `create_asr`: request automatic caption generation for a locale. star has
/// no Kaltura backend, so the track is created with a placeholder
/// `external_id` (the ASR marker) exactly like Canvas' pending caption asset.
async fn create_asr_track(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    auth: AuthUser,
    Json(params): Json<AsrParams>,
) -> Result<Response, ApiError> {
    let media_object = fetch_media_object(&state.db, id).await?;

    if media_object.user_id != auth.id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Canvas validates the locale shape with /\A[A-Za-z-]+\z/.
    if params.locale.is_empty()
        || !params
            .locale
            .chars()
            .all(|c| c.is_ascii_alphabetic() || c == '-')
    {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Invalid or missing locale parameter"
            })),
        )
            .into_response());
    }

    // find_or_create the ASR track (Canvas `find_or_create_track` with the
    // Kaltura asset id as external_id; star mints a deterministic marker).
    let external_id = format!("asr-{}-{}", media_object.media_id, params.locale);
    let existing: Option<MediaTrack> = sqlx::query_as(
        r#"
        SELECT id, media_object_id, user_id, kind, locale, content, external_id,
               workflow_state, created_at, updated_at
        FROM media_tracks
        WHERE media_object_id = $1 AND locale = $2 AND workflow_state <> 'deleted'
        ORDER BY id ASC LIMIT 1
        "#,
    )
    .bind(id)
    .bind(&params.locale)
    .fetch_optional(&state.db)
    .await?;
    let track: MediaTrack = match existing {
        Some(track) => track,
        None => {
            sqlx::query_as(
                r#"
            INSERT INTO media_tracks (media_object_id, user_id, kind, locale, external_id)
            VALUES ($1, $2, 'subtitles', $3, $4)
            RETURNING id, media_object_id, user_id, kind, locale, content, external_id,
                      workflow_state, created_at, updated_at
            "#,
            )
            .bind(id)
            .bind(auth.id)
            .bind(&params.locale)
            .bind(&external_id)
            .fetch_one(&state.db)
            .await?
        }
    };

    // Canvas renders the media object json for the media_objects variant.
    let tracks = load_tracks(&state.db, media_object.id).await?;
    let mut object = serde_json::to_value(to_json(&auth, media_object, tracks)?)
        .map_err(|e| ApiError::Db(e.to_string()))?;
    if let Some(obj) = object.as_object_mut() {
        obj.insert(
            "media_tracks".into(),
            serde_json::json!([{
                "id": track.id,
                "locale": track.locale,
                "kind": track.kind,
                "workflow_state": track.workflow_state,
                "asr": track.is_asr(),
                "media_object_id": track.media_object_id,
                "user_id": track.user_id,
            }]),
        );
    }
    Ok(Json(object).into_response())
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
        // Canvas media_objects#index: user, course, and group contexts.
        .route("/media_objects", get(list_media_attachments))
        .route(
            "/courses/{courseId}/media_objects",
            get(list_course_media_objects),
        )
        .route(
            "/courses/{courseId}/media_attachments",
            get(list_course_media_objects),
        )
        .route(
            "/groups/{groupId}/media_objects",
            get(list_group_media_objects),
        )
        .route(
            "/groups/{groupId}/media_attachments",
            get(list_group_media_objects),
        )
        // Canvas media_objects#create_media_object / #update_media_object.
        .route("/media_objects", post(create_media_object))
        .route("/media_objects/{id}", put(update_media_object))
        // Canvas media_tracks#update (batch replace) and #create_asr, in both
        // the media_objects and media_attachments key spaces.
        .route("/media_objects/{id}/media_tracks", put(update_media_tracks))
        .route("/media_objects/{id}/asr", post(create_asr_track))
        .route(
            "/media_attachments/{id}/media_tracks",
            get(list_media_tracks),
        )
        .route(
            "/media_attachments/{id}/media_tracks",
            put(update_media_tracks),
        )
        .route("/media_attachments/{id}/asr", post(create_asr_track))
}
