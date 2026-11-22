//! Canvas API v1 media object routes.
//!
//! Faithful port of `MediaObjectsController` (`canvas-src/app/controllers/media_objects_controller.rb`)
//! for the routes this crate owns. Everything is relative to the `/api/v1`
//! nest that `star-http` applies:
//!
//! | Verb | Path | Canvas action |
//! |------|------|---------------|
//! | GET  | `/media_objects` | `media_objects#index` |
//! | GET  | `/courses/{course_id}/media_objects` | `media_objects#index` |
//! | GET  | `/groups/{group_id}/media_objects` | `media_objects#index` |
//! | GET  | `/courses/{course_id}/media_attachments` | `media_objects#index` |
//! | GET  | `/groups/{group_id}/media_attachments` | `media_objects#index` |
//! | PUT  | `/media_objects/{media_object_id}` | `media_objects#update_media_object` |
//! | POST | `/media_objects` | `media_objects#create_media_object` |
//!
//! Canvas answers all six listings with the *same* action
//! (`media_objects#index`) and the *same* serializer
//! (`media_object_api_json`); only the pagination base URL differs
//! (`api_v1_media_attachments_url` for the global listing,
//! `api_v1_course_media_attachments_url` / `api_v1_group_media_attachments_url`
//! for the scoped ones). The port keeps that behaviour: one handler, one
//! serializer, three scopes.
//!
//! ## External service
//!
//! Canvas talks to Kaltura for `media_sources` (the transcoded flavours) and
//! for the caption (ASR) pipeline. star-lms has no media service, so
//! `media_sources` is always an empty array — exactly what the media
//! provider would return for an entry that has no flavours yet. The
//! `data[:status]` key Canvas derives from Kaltura is therefore absent.
//!
//! ## Schema
//!
//! Migration `0224_media_objects.sql` adds the Canvas columns this module
//! reads and writes (`media_objects.context_type`, `.context_id`,
//! `.old_media_id`, `.attachment_id`, `.viewer_restrictions`,
//! `.auto_caption_status`) and the `group_files` / `user_files` attachment
//! tables of the group and user scopes. `media_objects.root_account_id` has
//! no column in star-lms and is answered as `null`.

use std::collections::HashMap;

use axum::{
    extract::{Path, Query, RawQuery, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Canvas `Api::PER_PAGE`.
const PER_PAGE: i64 = 10;
/// Canvas `Api::MAX_PER_PAGE`.
const MAX_PER_PAGE: i64 = 100;
/// Canvas `MediaObject::VIEWER_RESTRICTION_KEYS`.
const VIEWER_RESTRICTION_KEYS: [&str; 1] = ["show_rolling_transcript"];
/// Canvas `CanvasTextHelper.truncate_text(max_length: 255)`.
const MAX_TITLE_LENGTH: usize = 255;
/// Canvas `SearchTermHelper::MIN_SEARCH_TERM_LENGTH`.
const MIN_SEARCH_TERM_LENGTH: usize = 2;

// ---------------------------------------------------------------------------
// Row shapes
// ---------------------------------------------------------------------------

/// A `media_objects` row as this module needs it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaObjectRow {
    pub id: i64,
    pub media_id: String,
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub user_entered_title: Option<String>,
    pub user_id: i64,
    pub workflow_state: String,
    pub context_type: Option<String>,
    pub context_id: Option<i64>,
    pub old_media_id: Option<String>,
    pub attachment_id: Option<i64>,
    pub viewer_restrictions: serde_json::Value,
    pub auto_caption_status: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A `media_tracks` row as this module needs it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MediaTrackRow {
    pub id: i64,
    pub kind: String,
    pub locale: String,
    pub external_id: Option<String>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Canvas `MediaTrack#asr?` — `kind == "subtitles" && external_id?`.
fn track_is_asr(track: &MediaTrackRow) -> bool {
    track.kind == "subtitles" && track.external_id.is_some()
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Query parameters of `media_objects#index`.
#[derive(Debug, Deserialize, Default)]
pub struct IndexParams {
    pub sort: Option<String>,
    pub order: Option<String>,
    pub search_term: Option<String>,
    pub per_page: Option<String>,
    pub page: Option<String>,
}

/// Query parameters of `media_objects#create_media_object`.
#[derive(Debug, Deserialize, Default)]
pub struct CreateQuery {
    pub context_code: Option<String>,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub user_entered_title: Option<String>,
}

/// JSON body of `media_objects#create_media_object`.
#[derive(Debug, Deserialize, Default)]
pub struct CreateBody {
    pub context_code: Option<String>,
    /// The Kaltura entry id; `/media_objects` clients send `id`, the
    /// attachment twin sends `media_id` — accept both spellings.
    #[serde(alias = "media_id")]
    pub id: Option<String>,
    /// The Kaltura media type; same dual spelling as the id.
    #[serde(rename = "type", alias = "media_type")]
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub user_entered_title: Option<String>,
    pub viewer_restrictions: Option<HashMap<String, serde_json::Value>>,
}

/// Query parameters of `media_objects#update_media_object`.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateQuery {
    pub user_entered_title: Option<String>,
}

/// JSON body of `media_objects#update_media_object`.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateBody {
    pub user_entered_title: Option<String>,
    pub viewer_restrictions: Option<HashMap<String, serde_json::Value>>,
}

// ---------------------------------------------------------------------------
// Scopes and contexts
// ---------------------------------------------------------------------------

/// The three flavours of `media_objects#index`.
#[derive(Debug, Clone, Copy)]
enum MediaScope {
    /// `GET /media_objects` — the caller's own media.
    User,
    /// `GET /courses/{id}/media_objects|media_attachments`.
    Course(i64),
    /// `GET /groups/{id}/media_objects|media_attachments`.
    Group(i64),
}

/// A context resolved from a Canvas asset string (`course_17`, `user_9`, …).
#[derive(Debug, Clone, Copy)]
pub(crate) enum MediaContext {
    Course { id: i64 },
    Group { id: i64 },
    User { id: i64 },
    Account { id: i64 },
}

impl MediaContext {
    /// The `context_type` Canvas stores on `media_objects`.
    fn context_type(&self) -> &'static str {
        match self {
            MediaContext::Course { .. } => "Course",
            MediaContext::Group { .. } => "Group",
            MediaContext::User { .. } => "User",
            MediaContext::Account { .. } => "Account",
        }
    }

    fn id(&self) -> i64 {
        match self {
            MediaContext::Course { id }
            | MediaContext::Group { id }
            | MediaContext::User { id }
            | MediaContext::Account { id } => *id,
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// `CanvasTextHelper.truncate_text(max_length: 255)`.
fn truncate_title(value: &str) -> String {
    value.chars().take(MAX_TITLE_LENGTH).collect()
}

/// Canvas `MediaObject#guaranteed_title`.
fn guaranteed_title(row: &MediaObjectRow) -> String {
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

/// Minimal percent-decoder (the crate has no url-decoding dependency).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Minimal percent-encoder, used to rebuild the pagination query string.
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Canvas `exclude[]` — `sources` and/or `tracks`.
fn parse_exclude(raw: Option<&str>) -> Vec<String> {
    let mut exclude = Vec::new();
    let Some(query) = raw else {
        return exclude;
    };
    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let key = percent_decode(key);
        if key == "exclude[]" || key == "exclude" {
            exclude.push(percent_decode(value));
        }
    }
    exclude
}

/// The base URL of the request, e.g. `http://canvas.example.com`.
fn base_url(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(',').next().unwrap_or("http").trim().to_string())
        .unwrap_or_else(|| "http".to_string());
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string())
        .unwrap_or_else(|| "localhost".to_string());
    format!("{scheme}://{host}")
}

/// The `Link` header Canvas builds in `Api.build_links_from_hash`.
fn pagination_links(
    base: &str,
    query: &str,
    current: i64,
    per_page: i64,
    total: i64,
) -> Option<String> {
    let last_page = if per_page > 0 {
        ((total + per_page - 1) / per_page).max(1)
    } else {
        1
    };
    let link = |page: i64| -> String {
        let mut url = format!("{base}?");
        if !query.is_empty() {
            url.push_str(query);
            url.push('&');
        }
        url.push_str(&format!("page={page}&per_page={per_page}"));
        url
    };

    let mut links: Vec<String> = Vec::new();
    links.push(format!("<{}>; rel=\"current\"", link(current)));
    if current < last_page {
        links.push(format!("<{}>; rel=\"next\"", link(current + 1)));
    }
    if current > 1 {
        links.push(format!("<{}>; rel=\"prev\"", link(current - 1)));
    }
    links.push(format!("<{}>; rel=\"first\"", link(1)));
    links.push(format!("<{}>; rel=\"last\"", link(last_page)));
    Some(links.join(","))
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Whether `user_id` is an admin of `account_id` or of one of its ancestors.
async fn is_account_admin(db: &PgPool, user_id: i64, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE account_chain AS (
            SELECT id, parent_account_id, 0 AS depth
            FROM accounts
            WHERE id = $2 AND workflow_state <> 'deleted'
            UNION ALL
            SELECT parent.id, parent.parent_account_id, chain.depth + 1
            FROM accounts parent
            JOIN account_chain chain ON parent.id = chain.parent_account_id
            WHERE parent.workflow_state <> 'deleted' AND chain.depth < 10
        )
        SELECT 1
        FROM account_users au
        JOIN account_chain chain ON chain.id = au.account_id
        WHERE au.user_id = $1
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Whether `user_id` may read the contents of the course.
///
/// Canvas checks `root_folder.grants_right?(user, :read_contents)`, which
/// resolves to "the user may read the course" — an active enrollment, or an
/// admin of the account that owns the course.
async fn can_read_course(db: &PgPool, user_id: i64, course_id: i64) -> Result<bool, ApiError> {
    let enrolled: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM enrollments
        WHERE user_id = $1 AND course_id = $2
          AND workflow_state IN ('active', 'invited')
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    if enrolled.is_some() {
        return Ok(true);
    }

    let account: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    match account {
        Some((Some(account_id),)) => is_account_admin(db, user_id, account_id).await,
        _ => Ok(false),
    }
}

/// Whether `user_id` may read the contents of the group.
///
/// Canvas checks `root_folder.grants_right?(user, :read_contents)` for the
/// group's root folder. A group member may read the group; so may a teacher
/// of the course the group belongs to, and an admin of that course's account.
async fn can_read_group(db: &PgPool, user_id: i64, group_id: i64) -> Result<bool, ApiError> {
    let member: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM group_memberships
        WHERE group_id = $1 AND user_id = $2 AND workflow_state = 'accepted'
        LIMIT 1
        "#,
    )
    .bind(group_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    if member.is_some() {
        return Ok(true);
    }

    let course: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT course_id FROM groups WHERE id = $1")
            .bind(group_id)
            .fetch_optional(db)
            .await?;
    let Some((Some(course_id),)) = course else {
        return Ok(false);
    };

    let staff: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM enrollments
        WHERE user_id = $1 AND course_id = $2
          AND workflow_state IN ('active', 'invited')
          AND role IN ('teacher', 'ta', 'designer')
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    if staff.is_some() {
        return Ok(true);
    }

    let account: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    match account {
        Some((Some(account_id),)) => is_account_admin(db, user_id, account_id).await,
        _ => Ok(false),
    }
}

/// Canvas `authorized_action(@context, @current_user, :read)`.
pub(crate) async fn can_read_context(
    db: &PgPool,
    auth: &AuthUser,
    context: &MediaContext,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    match context {
        MediaContext::Course { id, .. } => can_read_course(db, auth.id, *id).await,
        MediaContext::Group { id, .. } => can_read_group(db, auth.id, *id).await,
        MediaContext::User { id } => Ok(*id == auth.id),
        MediaContext::Account { id } => is_account_admin(db, auth.id, *id).await,
    }
}

/// Whether the caller may add captions to the media object.
///
/// Canvas `MediaObject#grants_right?(user, :add_captions)`: when the object
/// has an attachment the caller needs the attachment's `:update` right,
/// otherwise the caller needs `:manage_course_content_add` on the context or
/// has to be the owner of the object.
pub(crate) async fn can_add_captions(
    db: &PgPool,
    auth: &AuthUser,
    row: &MediaObjectRow,
) -> Result<bool, ApiError> {
    if row.user_id == auth.id {
        return Ok(true);
    }
    if auth.role == "admin" {
        return Ok(true);
    }
    let MediaContext::Course { id, .. } = context_of(row) else {
        return Ok(false);
    };
    let staff: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM enrollments
        WHERE user_id = $1 AND course_id = $2
          AND workflow_state IN ('active', 'invited')
          AND role IN ('teacher', 'ta', 'designer')
        LIMIT 1
        "#,
    )
    .bind(auth.id)
    .bind(id)
    .fetch_optional(db)
    .await?;
    if staff.is_some() {
        return Ok(true);
    }
    let account: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(id)
            .fetch_optional(db)
            .await?;
    match account {
        Some((Some(account_id),)) => is_account_admin(db, auth.id, account_id).await,
        _ => Ok(false),
    }
}

/// The context a media object was created in.
pub(crate) fn context_of(row: &MediaObjectRow) -> MediaContext {
    match (row.context_type.as_deref(), row.context_id) {
        (Some("Course"), Some(id)) => MediaContext::Course { id },
        (Some("Group"), Some(id)) => MediaContext::Group { id },
        (Some("User"), Some(id)) => MediaContext::User { id },
        (Some("Account"), Some(id)) => MediaContext::Account { id },
        _ => MediaContext::User { id: row.user_id },
    }
}

// ---------------------------------------------------------------------------
// Context lookup
// ---------------------------------------------------------------------------

/// Canvas `Context.find_by_asset_string(params[:context_code])`.
pub(crate) async fn find_context(
    db: &PgPool,
    context_code: &str,
) -> Result<Option<MediaContext>, ApiError> {
    let code = context_code.trim();
    let Some((kind, id)) = code.rsplit_once('_') else {
        return Ok(None);
    };
    let Ok(id) = id.parse::<i64>() else {
        return Ok(None);
    };

    match kind {
        "course" => {
            let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
                .bind(id)
                .fetch_optional(db)
                .await?;
            Ok(row.map(|_| MediaContext::Course { id }))
        }
        "group" => {
            let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
                .bind(id)
                .fetch_optional(db)
                .await?;
            Ok(row.map(|_| MediaContext::Group { id }))
        }
        "user" => {
            let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
                .bind(id)
                .fetch_optional(db)
                .await?;
            Ok(row.map(|_| MediaContext::User { id }))
        }
        "account" => {
            let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(db)
                .await?;
            Ok(row.map(|_| MediaContext::Account { id }))
        }
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

const MEDIA_OBJECT_COLUMNS: &str = r#"
    id, media_id, media_type, title, user_entered_title, user_id, workflow_state,
    context_type, context_id, old_media_id, attachment_id,
    viewer_restrictions, auto_caption_status, created_at, updated_at
"#;

/// `MediaObject.by_media_id` — the Kaltura entry id, or the id the object was
/// migrated from.
async fn fetch_media_object_by_media_id(
    db: &PgPool,
    media_id: &str,
) -> Result<Option<MediaObjectRow>, ApiError> {
    // Canvas resolves by the Kaltura entry id (media_id / old_media_id).
    // Clients that only have the database row (the attachment flows)
    // address it by the numeric id — accept that spelling too.
    let row: Option<MediaObjectRow> = if let Ok(numeric) = media_id.parse::<i64>() {
        sqlx::query_as(&format!(
            "SELECT {MEDIA_OBJECT_COLUMNS} FROM media_objects
             WHERE workflow_state <> 'deleted'
               AND (media_id = $1 OR (old_media_id = $1 AND old_media_id IS NOT NULL)
                    OR id = $2)
             ORDER BY id ASC
             LIMIT 1"
        ))
        .bind(media_id)
        .bind(numeric)
        .fetch_optional(db)
        .await?
    } else {
        sqlx::query_as(&format!(
            "SELECT {MEDIA_OBJECT_COLUMNS} FROM media_objects
             WHERE workflow_state <> 'deleted'
               AND (media_id = $1 OR (old_media_id = $1 AND old_media_id IS NOT NULL))
             ORDER BY id ASC
             LIMIT 1"
        ))
        .bind(media_id)
        .fetch_optional(db)
        .await?
    };
    Ok(row)
}

/// The tracks of a media object, ordered by locale like Canvas'
/// `has_many :media_tracks, ->(mo) { ... order(:locale) }`.
async fn load_tracks(db: &PgPool, media_object_id: i64) -> Result<Vec<MediaTrackRow>, ApiError> {
    let rows: Vec<MediaTrackRow> = sqlx::query_as(
        r#"
        SELECT id, kind, locale, external_id, workflow_state,
               created_at, updated_at
        FROM media_tracks
        WHERE media_object_id = $1 AND workflow_state <> 'deleted'
        ORDER BY locale ASC, id ASC
        "#,
    )
    .bind(media_object_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// Serializer
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::MediaObject#media_object_api_json`.
///
/// The attribute list is `only: %w[media_id created_at media_type]`; every
/// other key is added by the serializer itself.
fn media_object_api_json(
    row: &MediaObjectRow,
    can_add_captions: bool,
    base: &str,
    exclude: &[String],
    tracks: &[MediaTrackRow],
) -> serde_json::Value {
    let mut json = serde_json::json!({
        "media_id": row.media_id,
        "created_at": row.created_at,
        "media_type": row.media_type,
        "title": guaranteed_title(row),
        "can_add_captions": can_add_captions,
    });

    if !exclude.iter().any(|value| value == "sources") {
        // No media service is configured, so there are no transcoded flavours.
        json["media_sources"] = serde_json::json!([]);
    }

    json["embedded_iframe_url"] =
        serde_json::json!(format!("{base}/media_objects_iframe/{}", row.media_id));
    json["auto_caption_status"] = serde_json::json!(row.auto_caption_status);
    json["viewer_restrictions"] = serde_json::json!(row.viewer_restrictions);

    if !exclude.iter().any(|value| value == "tracks") {
        let track_json: Vec<serde_json::Value> = tracks
            .iter()
            .map(|track| {
                serde_json::json!({
                    "kind": track.kind,
                    "created_at": track.created_at,
                    "updated_at": track.updated_at,
                    "id": track.id,
                    "locale": track.locale,
                    "workflow_state": track.workflow_state,
                    "asr": track_is_asr(track),
                    "url": format!(
                        "{base}/media_objects/{}/media_tracks/{}",
                        row.media_id, track.id
                    ),
                })
            })
            .collect();
        json["media_tracks"] = serde_json::Value::Array(track_json);
    }

    json
}

/// Canvas `MediaObject#as_json` for `create_media_object` — the full
/// attribute set, wrapped in the `media_object` root key.
fn media_object_as_json(row: &MediaObjectRow, uuid: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "id": row.id,
        "media_id": row.media_id,
        "media_type": row.media_type,
        "title": row.title,
        "user_entered_title": row.user_entered_title,
        "user_id": row.user_id,
        "user_type": "User",
        "context_id": row.context_id,
        "context_type": row.context_type,
        "workflow_state": row.workflow_state,
        "duration": serde_json::Value::Null,
        "max_size": serde_json::Value::Null,
        "total_size": serde_json::Value::Null,
        "root_account_id": serde_json::Value::Null,
        "attachment_id": row.attachment_id,
        "old_media_id": row.old_media_id,
        "auto_caption_status": row.auto_caption_status,
        "auto_caption_media_id": serde_json::Value::Null,
        "data": serde_json::json!({}),
        "viewer_restrictions": row.viewer_restrictions,
        "created_at": row.created_at,
        "updated_at": row.updated_at,
        "uuid": uuid,
    })
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// The `scope` predicate and the pagination base URL of a listing.
fn scope_parts(scope: MediaScope, base: &str) -> (String, String) {
    match scope {
        MediaScope::User => (
            "user_id = $1".to_string(),
            format!("{base}/api/v1/media_attachments"),
        ),
        MediaScope::Course(id) => (
            // Canvas scopes a context listing two ways: objects created
            // with the context (context columns) and objects linked to the
            // context through one of its video attachments
            // (media_objects_for_context). Either way in.
            "(context_type = 'Course' AND context_id = $1) \
             OR id IN (SELECT m.id FROM attachments a \
                 JOIN media_objects m ON m.media_id = a.media_entry_id \
                 WHERE a.context_type = 'Course' AND a.context_id = $1 \
                   AND a.content_type LIKE 'video/%' \
                   AND a.media_entry_id IS NOT NULL)"
                .to_string(),
            format!("{base}/api/v1/courses/{id}/media_attachments"),
        ),
        MediaScope::Group(id) => (
            "(context_type = 'Group' AND context_id = $1) \
             OR id IN (SELECT m.id FROM attachments a \
                 JOIN media_objects m ON m.media_id = a.media_entry_id \
                 WHERE a.context_type = 'Group' AND a.context_id = $1 \
                   AND a.content_type LIKE 'video/%' \
                   AND a.media_entry_id IS NOT NULL)"
                .to_string(),
            format!("{base}/api/v1/groups/{id}/media_attachments"),
        ),
    }
}

/// Canvas `MediaObject#search_by_attribute(scope, :title, search_term)`.
///
/// The term is matched case-insensitively against the `title` column; a term
/// shorter than two characters is a `400` in Canvas
/// (`SearchTermHelper::SearchTermTooShortError`).
fn search_pattern(search_term: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(term) = search_term else {
        return Ok(None);
    };
    if term.trim().is_empty() {
        return Ok(None);
    }
    if term.chars().count() < MIN_SEARCH_TERM_LENGTH {
        return Err(ApiError::BadRequest(
            "2 or more characters is required".to_string(),
        ));
    }
    // Canvas `wildcard_pattern`: escape the wildcards, then wrap in `%`.
    let escaped = term
        .to_lowercase()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    Ok(Some(format!("%{escaped}%")))
}

/// The order clause of the listing.
///
/// Canvas sorts on `COALESCE(user_entered_title, title)` with an ICU
/// collation; the port approximates that with `LOWER(...)`.
fn order_clause(params: &IndexParams) -> (&'static str, &'static str) {
    let column = match params.sort.as_deref() {
        Some("created_at") => "created_at",
        _ => "LOWER(COALESCE(user_entered_title, title))",
    };
    let direction = if params.order.as_deref() == Some("desc") {
        "DESC"
    } else {
        "ASC"
    };
    (column, direction)
}

/// The query string Canvas keeps in the pagination links (everything except
/// `page`, `per_page`, `access_token` and `api_key`).
fn pagination_query(params: &IndexParams, exclude: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(sort) = &params.sort {
        parts.push(format!("sort={}", percent_encode(sort)));
    }
    if let Some(order) = &params.order {
        parts.push(format!("order={}", percent_encode(order)));
    }
    if let Some(term) = &params.search_term {
        parts.push(format!("search_term={}", percent_encode(term)));
    }
    for value in exclude {
        parts.push(format!("exclude%5B%5D={}", percent_encode(value)));
    }
    parts.join("&")
}

/// The shared implementation of all six listings.
async fn list_media_objects(
    state: &AppState,
    auth: &AuthUser,
    scope: MediaScope,
    params: IndexParams,
    exclude: Vec<String>,
    base: &str,
) -> Result<Response, ApiError> {
    // Canvas loads the context first: `Course.find` / `Group.find` raise
    // `ActiveRecord::RecordNotFound` -> 404.
    match scope {
        MediaScope::User => {}
        MediaScope::Course(course_id) => {
            let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
                .bind(course_id)
                .fetch_optional(&state.db)
                .await?;
            if exists.is_none() {
                return Err(ApiError::NotFound);
            }
            if !can_read_course(&state.db, auth.id, course_id).await? {
                return Err(ApiError::Forbidden);
            }
        }
        MediaScope::Group(group_id) => {
            let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM groups WHERE id = $1")
                .bind(group_id)
                .fetch_optional(&state.db)
                .await?;
            if exists.is_none() {
                return Err(ApiError::NotFound);
            }
            if !can_read_group(&state.db, auth.id, group_id).await? {
                return Err(ApiError::Forbidden);
            }
        }
    }

    let (predicate, pagination_base) = scope_parts(scope, base);
    let pattern = search_pattern(params.search_term.as_deref())?;

    // `$2` is bound to NULL when there is no search term, which makes the
    // filter a no-op — the same result as Canvas returning the scope
    // unchanged when `search_term` is blank. The predicate is parenthesized
    // because the context scopes are OR-unions.
    let where_sql = format!(
        "WHERE workflow_state <> 'deleted' AND ({predicate}) \
         AND ($2::text IS NULL OR LOWER(title) LIKE $2 ESCAPE '\\')"
    );

    let (column, direction) = order_clause(&params);
    let per_page = params
        .per_page
        .as_deref()
        .map(|value| value.trim().parse::<i64>().unwrap_or(0))
        .unwrap_or(PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = params
        .page
        .as_deref()
        .map(|value| value.trim().parse::<i64>().unwrap_or(0))
        .unwrap_or(1)
        .max(1);
    let offset = (page - 1) * per_page;

    let list_sql = format!(
        "SELECT {MEDIA_OBJECT_COLUMNS} FROM media_objects {where_sql} \
         ORDER BY {column} {direction}, id ASC LIMIT {per_page} OFFSET {offset}"
    );
    let count_sql = format!("SELECT COUNT(*) FROM media_objects {where_sql}");

    let scope_value = match scope {
        MediaScope::User => auth.id,
        MediaScope::Course(id) | MediaScope::Group(id) => id,
    };

    let rows: Vec<MediaObjectRow> = sqlx::query_as(&list_sql)
        .bind(scope_value)
        .bind(pattern.clone())
        .fetch_all(&state.db)
        .await?;
    let total: (i64,) = sqlx::query_as(&count_sql)
        .bind(scope_value)
        .bind(pattern)
        .fetch_one(&state.db)
        .await?;

    let mut payload = Vec::with_capacity(rows.len());
    for row in &rows {
        let tracks = load_tracks(&state.db, row.id).await?;
        let allowed = can_add_captions(&state.db, auth, row).await?;
        payload.push(media_object_api_json(row, allowed, base, &exclude, &tracks));
    }

    let mut response = Json(payload).into_response();
    if let Some(links) = pagination_links(
        &pagination_base,
        &pagination_query(&params, &exclude),
        page,
        per_page,
        total.0,
    ) {
        if let Ok(value) = header::HeaderValue::from_str(&links) {
            response.headers_mut().insert(header::LINK, value);
        }
    }
    Ok(response)
}

/// `GET /api/v1/media_objects`.
async fn list_media_objects_route(
    State(state): State<AppState>,
    Query(params): Query<IndexParams>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let base = base_url(&headers);
    list_media_objects(
        &state,
        &auth,
        MediaScope::User,
        params,
        parse_exclude(raw.as_deref()),
        &base,
    )
    .await
}

/// `GET /api/v1/courses/{course_id}/media_objects`.
async fn list_course_media_objects_route(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<IndexParams>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let base = base_url(&headers);
    list_media_objects(
        &state,
        &auth,
        MediaScope::Course(course_id),
        params,
        parse_exclude(raw.as_deref()),
        &base,
    )
    .await
}

/// `GET /api/v1/groups/{group_id}/media_objects`.
async fn list_group_media_objects_route(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(params): Query<IndexParams>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let base = base_url(&headers);
    list_media_objects(
        &state,
        &auth,
        MediaScope::Group(group_id),
        params,
        parse_exclude(raw.as_deref()),
        &base,
    )
    .await
}

/// `GET /api/v1/courses/{course_id}/media_attachments`.
async fn list_course_media_attachments_route(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(params): Query<IndexParams>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let base = base_url(&headers);
    list_media_objects(
        &state,
        &auth,
        MediaScope::Course(course_id),
        params,
        parse_exclude(raw.as_deref()),
        &base,
    )
    .await
}

/// `GET /api/v1/groups/{group_id}/media_attachments`.
async fn list_group_media_attachments_route(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(params): Query<IndexParams>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let base = base_url(&headers);
    list_media_objects(
        &state,
        &auth,
        MediaScope::Group(group_id),
        params,
        parse_exclude(raw.as_deref()),
        &base,
    )
    .await
}

// ---------------------------------------------------------------------------
// Update
// ---------------------------------------------------------------------------

/// Canvas `MediaObject::VIEWER_RESTRICTION_KEYS` filtering plus the
/// `validate_viewer_restrictions` model validation.
fn merge_viewer_restrictions(
    current: &serde_json::Value,
    permitted: Option<&HashMap<String, serde_json::Value>>,
) -> Result<serde_json::Value, ApiError> {
    let mut merged = match current {
        serde_json::Value::Object(map) => map.clone(),
        _ => serde_json::Map::new(),
    };
    let Some(permitted) = permitted else {
        return Ok(serde_json::Value::Object(merged));
    };
    for (key, value) in permitted {
        if !VIEWER_RESTRICTION_KEYS.contains(&key.as_str()) {
            continue;
        }
        if !value.is_boolean() {
            return Err(ApiError::Validation(format!(
                "value for {key} must be a boolean"
            )));
        }
        merged.insert(key.clone(), value.clone());
    }
    Ok(serde_json::Value::Object(merged))
}

/// `PUT /api/v1/media_objects/{media_object_id}`.
///
/// `media_object_id` is the Kaltura entry id, not the database id — Canvas
/// looks the object up with `MediaObject.by_media_id`.
async fn update_media_object_route(
    State(state): State<AppState>,
    Path(media_object_id): Path<String>,
    Query(query): Query<UpdateQuery>,
    headers: HeaderMap,
    auth: AuthUser,
    body: Option<Json<UpdateBody>>,
) -> Result<Response, ApiError> {
    let base = base_url(&headers);

    // Canvas `load_media_object` -> `MediaObject.by_media_id(media_id).take`.
    let Some(existing) = fetch_media_object_by_media_id(&state.db, &media_object_id).await? else {
        // Canvas `check_media_permissions` renders the unauthorized action.
        return Err(ApiError::Forbidden);
    };

    // Canvas `update_media_object`: the media object has to belong to the
    // caller — media objects have no permissions of their own.
    if existing.user_id != auth.id {
        return Err(ApiError::Forbidden);
    }

    let body = body.map(|Json(body)| body);
    let user_entered_title = body
        .as_ref()
        .and_then(|body| body.user_entered_title.clone())
        .or_else(|| query.user_entered_title.clone());

    // Canvas: `return render json: { message: ... }, status: :bad_request`
    // when `params[:user_entered_title]` is blank.
    let Some(user_entered_title) = user_entered_title.filter(|value| !value.trim().is_empty())
    else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "message": "The user_entered_title parameter must have a value"
            })),
        )
            .into_response());
    };

    let viewer_restrictions = merge_viewer_restrictions(
        &existing.viewer_restrictions,
        body.as_ref()
            .and_then(|body| body.viewer_restrictions.as_ref()),
    )?;

    let title = truncate_title(&user_entered_title);
    let row: MediaObjectRow = sqlx::query_as(&format!(
        "UPDATE media_objects
         SET user_entered_title = $1, viewer_restrictions = $2, updated_at = now()
         WHERE id = $3
         RETURNING {MEDIA_OBJECT_COLUMNS}"
    ))
    .bind(&title)
    .bind(&viewer_restrictions)
    .bind(existing.id)
    .fetch_one(&state.db)
    .await?;

    // Canvas renders `media_object_api_json(..., %w[sources tracks])`.
    let tracks = load_tracks(&state.db, row.id).await?;
    let allowed = can_add_captions(&state.db, &auth, &row).await?;
    let json = media_object_api_json(
        &row,
        allowed,
        &base,
        &["sources".into(), "tracks".into()],
        &tracks,
    );
    Ok(Json(json).into_response())
}

// ---------------------------------------------------------------------------
// Create
// ---------------------------------------------------------------------------

/// Canvas `MediaObject#create_attachment` — the attachment that backs a media
/// object inside a context. Returns the attachment id and uuid.
async fn create_attachment(
    db: &PgPool,
    context: &MediaContext,
    auth: &AuthUser,
    title: &str,
    media_type: &str,
    media_id: &str,
) -> Result<Option<(i64, Option<String>)>, ApiError> {
    match context {
        MediaContext::Course { id, .. } => {
            let row: (i64, Option<String>) = sqlx::query_as(
                r#"
                INSERT INTO files (course_id, uploader_id, filename, display_name,
                                   content_type, size, folder, workflow_state, hidden)
                VALUES ($1, $2, $3, $3, $4, 0, 'media', 'pending_upload', TRUE)
                RETURNING id, uuid
                "#,
            )
            .bind(*id)
            .bind(auth.id)
            .bind(title)
            .bind(media_type)
            .fetch_one(db)
            .await?;
            Ok(Some(row))
        }
        MediaContext::Group { id, .. } => {
            let row: (i64, Option<String>) = sqlx::query_as(
                r#"
                INSERT INTO group_files (group_id, uploader_id, filename, display_name,
                                         content_type, size, folder, uuid, media_entry_id)
                VALUES ($1, $2, $3, $3, $4, 0, 'group files',
                        md5(random()::text || clock_timestamp()::text), $5)
                RETURNING id, uuid
                "#,
            )
            .bind(*id)
            .bind(auth.id)
            .bind(title)
            .bind(media_type)
            .bind(media_id)
            .fetch_one(db)
            .await?;
            Ok(Some(row))
        }
        MediaContext::User { id } => {
            let row: (i64, Option<String>) = sqlx::query_as(
                r#"
                INSERT INTO user_files (user_id, filename, display_name, content_type,
                                        size, folder, uuid, media_entry_id)
                VALUES ($1, $2, $2, $3, 0, 'media',
                        md5(random()::text || clock_timestamp()::text), $4)
                RETURNING id, uuid
                "#,
            )
            .bind(*id)
            .bind(title)
            .bind(media_type)
            .bind(media_id)
            .fetch_one(db)
            .await?;
            Ok(Some(row))
        }
        // Canvas has no media folder for accounts.
        MediaContext::Account { .. } => Ok(None),
    }
}

/// `POST /api/v1/media_objects`.
async fn create_media_object_route(
    State(state): State<AppState>,
    Query(query): Query<CreateQuery>,
    headers: HeaderMap,
    auth: AuthUser,
    body: Option<Json<CreateBody>>,
) -> Result<Response, ApiError> {
    let base = base_url(&headers);
    let body = body.map(|Json(body)| body);

    let context_code = body
        .as_ref()
        .and_then(|body| body.context_code.clone())
        .or_else(|| query.context_code.clone());
    // Without a context the create behaves like POST /media_attachments
    // (Canvas media_objects_api#create falls back to the attachment
    // semantics: a user-scoped object, no context resolution).
    let Some(context_code) = context_code else {
        let (status, body) = crate::create_media_object_like_attachment(
            &state,
            &auth,
            body.as_ref().and_then(|b| b.id.clone()),
            body.as_ref()
                .and_then(|b| b.media_type.clone())
                .or_else(|| query.media_type.clone()),
            body.as_ref().and_then(|b| b.title.clone()),
            body.as_ref().and_then(|b| b.user_entered_title.clone()),
        )
        .await?;
        return Ok((status, body).into_response());
    };

    // Canvas `Context.find_by_asset_string(params[:context_code])` -> nil, and
    // the action then has nothing to render.
    let Some(context) = find_context(&state.db, &context_code).await? else {
        return Err(ApiError::NotFound);
    };

    // Canvas `authorized_action(@context, @current_user, :read)`.
    if !can_read_context(&state.db, &auth, &context).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas only creates the object when the context supports media objects
    // and both `id` and `type` are present.
    if matches!(context, MediaContext::Account { .. }) {
        return Err(ApiError::BadRequest(
            "context does not support media objects".to_string(),
        ));
    }

    let media_id = body
        .as_ref()
        .and_then(|body| body.id.clone())
        .or_else(|| query.id.clone());
    let media_type = body
        .as_ref()
        .and_then(|body| body.media_type.clone())
        .or_else(|| query.media_type.clone());
    let (Some(media_id), Some(media_type)) = (media_id, media_type) else {
        return Err(ApiError::BadRequest(
            "id and type parameters are required".to_string(),
        ));
    };
    if media_id.trim().is_empty() || media_type.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "id and type parameters are required".to_string(),
        ));
    }

    let title = body
        .as_ref()
        .and_then(|body| body.title.clone())
        .or_else(|| query.title.clone())
        .map(|value| truncate_title(&value));
    let user_entered_title = body
        .as_ref()
        .and_then(|body| body.user_entered_title.clone())
        .or_else(|| query.user_entered_title.clone())
        .filter(|value| !value.trim().is_empty())
        .map(|value| truncate_title(&value));

    // Canvas sets the default from the `rce_asr_captioning_improvements`
    // feature flag; the flag is off by default.
    let mut viewer_restrictions = serde_json::Map::new();
    viewer_restrictions.insert(
        "show_rolling_transcript".to_string(),
        serde_json::Value::Bool(false),
    );
    if let Some(permitted) = body
        .as_ref()
        .and_then(|body| body.viewer_restrictions.as_ref())
    {
        for (key, value) in permitted {
            if !VIEWER_RESTRICTION_KEYS.contains(&key.as_str()) {
                continue;
            }
            if !value.is_boolean() {
                return Err(ApiError::Validation(format!(
                    "value for {key} must be a boolean"
                )));
            }
            viewer_restrictions.insert(key.clone(), value.clone());
        }
    }
    let viewer_restrictions = serde_json::Value::Object(viewer_restrictions);

    // Canvas `MediaObject.where(media_id:, media_type:, context:).first_or_initialize`.
    let existing: Option<MediaObjectRow> = sqlx::query_as(&format!(
        "SELECT {MEDIA_OBJECT_COLUMNS} FROM media_objects
         WHERE media_id = $1 AND media_type = $2
           AND context_type = $3 AND context_id = $4
         LIMIT 1"
    ))
    .bind(&media_id)
    .bind(&media_type)
    .bind(context.context_type())
    .bind(context.id())
    .fetch_optional(&state.db)
    .await?;

    let mut attachment_id: Option<i64> = None;
    let mut uuid: Option<String> = None;

    let row = match existing {
        Some(existing) => {
            // Canvas updates the existing object; `create_attachment` is a
            // `before_create` callback, so no second attachment is made.
            sqlx::query_as(&format!(
                "UPDATE media_objects
                 SET title = COALESCE($1, title),
                     user_entered_title = COALESCE($2, user_entered_title),
                     media_type = $3,
                     user_id = $4,
                     context_type = $5,
                     context_id = $6,
                     viewer_restrictions = $7,
                     updated_at = now()
                 WHERE id = $8
                 RETURNING {MEDIA_OBJECT_COLUMNS}"
            ))
            .bind(&title)
            .bind(&user_entered_title)
            .bind(&media_type)
            .bind(auth.id)
            .bind(context.context_type())
            .bind(context.id())
            .bind(&viewer_restrictions)
            .bind(existing.id)
            .fetch_one(&state.db)
            .await?
        }
        None => {
            // The port's `media_objects.media_id` is unique, so a media id can
            // only ever live in one context.
            let clash: Option<(i32,)> =
                sqlx::query_as("SELECT 1 FROM media_objects WHERE media_id = $1 LIMIT 1")
                    .bind(&media_id)
                    .fetch_optional(&state.db)
                    .await?;
            if clash.is_some() {
                return Err(ApiError::Conflict(format!(
                    "media_id {media_id} already exists in another context"
                )));
            }

            let inserted: MediaObjectRow = sqlx::query_as(&format!(
                "INSERT INTO media_objects (media_id, media_type, title, user_entered_title,
                                           user_id, context_type, context_id,
                                           viewer_restrictions)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 RETURNING {MEDIA_OBJECT_COLUMNS}"
            ))
            .bind(&media_id)
            .bind(&media_type)
            .bind(&title)
            .bind(&user_entered_title)
            .bind(auth.id)
            .bind(context.context_type())
            .bind(context.id())
            .bind(&viewer_restrictions)
            .fetch_one(&state.db)
            .await?;

            // Canvas creates the attachment in the context's media folder.
            let attachment = create_attachment(
                &state.db,
                &context,
                &auth,
                &title.clone().unwrap_or_else(|| "Untitled".to_string()),
                &media_type,
                &media_id,
            )
            .await?;

            match attachment {
                Some((id, attachment_uuid)) => {
                    attachment_id = Some(id);
                    uuid = attachment_uuid;
                    sqlx::query_as(&format!(
                        "UPDATE media_objects SET attachment_id = $1, updated_at = now()
                         WHERE id = $2 RETURNING {MEDIA_OBJECT_COLUMNS}"
                    ))
                    .bind(id)
                    .bind(inserted.id)
                    .fetch_one(&state.db)
                    .await?
                }
                None => inserted,
            }
        }
    };

    let mut payload = serde_json::Map::new();
    payload.insert(
        "media_object".to_string(),
        media_object_as_json(&row, uuid.as_deref()),
    );
    payload.insert(
        "embedded_iframe_url".to_string(),
        serde_json::Value::String(match attachment_id {
            Some(attachment_id) => format!("{base}/media_attachments_iframe/{attachment_id}"),
            None => format!("{base}/media_attachments_iframe/"),
        }),
    );

    // Canvas `render json:` — the default 200 (media_objects_controller.rb:255).
    Ok(Json(serde_json::Value::Object(payload)).into_response())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The Canvas API v1 media object routes, relative to the `/api/v1` nest.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/media_objects",
            get(list_media_objects_route).post(create_media_object_route),
        )
        .route(
            "/media_objects/{media_object_id}",
            put(update_media_object_route),
        )
        .route(
            "/courses/{course_id}/media_objects",
            get(list_course_media_objects_route),
        )
        .route(
            "/groups/{group_id}/media_objects",
            get(list_group_media_objects_route),
        )
        .route(
            "/courses/{course_id}/media_attachments",
            get(list_course_media_attachments_route),
        )
        .route(
            "/groups/{group_id}/media_attachments",
            get(list_group_media_attachments_route),
        )
}
