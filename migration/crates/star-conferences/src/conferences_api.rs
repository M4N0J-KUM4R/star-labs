//! Canvas web-conferences API — port of `ConferencesController`
//! (`canvas-src/app/controllers/conferences_controller.rb`) and of
//! `Api::V1::Conferences` (`canvas-src/lib/api/v1/conferences.rb`).
//!
//! The coordinator mounts this router under `/api/v1` (like every other
//! `star-*` crate, the paths below are API-relative).
//!
//! | Verb | Path | Canvas action |
//! |---|---|---|
//! | GET  | `/courses/{course_id}/conferences` | `index` |
//! | POST | `/courses/{course_id}/conferences` | `create` |
//! | GET  | `/groups/{group_id}/conferences` | `index` |
//! | POST | `/groups/{group_id}/conferences` | `create` |
//! | POST | `/courses/{course_id}/conferences/{conference_id}/recording_ready` | `recording_ready` |
//! | POST | `/groups/{group_id}/conferences/{conference_id}/recording_ready` | `recording_ready` |
//! | GET  | `/conferences` | `for_user` |
//!
//! ## Guard order
//!
//! Canvas runs `require_context` (except `for_user`), `require_config`
//! (except `for_user`), `reject_student_view_student` and `get_conference`
//! (except `index`/`create`/`for_user`) as `before_action`s. The port keeps
//! that order, so a missing context is a 404 *before* a missing permission
//! is a 403, and `recording_ready` — which skips `load_user`/`require_user`
//! in Canvas — needs no session at all.
//!
//! ## The conference provider
//!
//! Canvas' conferences are backed by a plugin (`wimba`, `big_blue_button`,
//! …) whose settings live in `plugin_settings`. `WebConference.config` is
//! the first *enabled* plugin, `WebConference.active_conference_type_names`
//! the conference types of the enabled plugins. The port reads the same
//! table:
//!
//! * `plugin_settings.name` is the plugin id (`big_blue_button`);
//! * `plugin_settings.settings` is the plugin's settings JSON and holds the
//!   decrypted provider secret under `secret_dec` — the key
//!   `recording_ready` verifies the provider's JWT with;
//! * `plugin_settings.disabled` switches the plugin off; a disabled plugin's
//!   conferences are not listed any more.
//!
//! `require_config` fails when no plugin is enabled. Canvas flashes
//! "Web conferencing has not been enabled for this Canvas site" and
//! redirects to the context URL (an HTML flow); the API answers `403` with
//! the same message (`specs/p5-rest-conferences.md` §8).
//!
//! ## Serializers
//!
//! `index` and `for_user` render `Api::V1::Conferences#api_conference_json`,
//! `create` renders `WebConference#as_json`. They differ:
//!
//! * `api_json` renders `WebConference#as_json` with
//!   `only: API_CONFERENCE_JSON_OPTS[:only]`. Rails' `only` list applies to
//!   attributes *and* methods, so the model methods (`has_advanced_settings`,
//!   `long_running`, `user_settings`, `recordings`, …) are dropped. The
//!   helper then sets `has_advanced_settings`/`long_running` to `false`,
//!   casts `duration` with `to_i` and renames `user_ids` to `users`
//!   (`invitees`/`attendees` stay empty arrays).
//! * `create` renders the model's own `as_json`, whose `only` list is
//!   `id title description conference_type duration started_at ended_at
//!   user_ids context_id context_type context_code start_at end_at` — so it
//!   answers `user_ids` (the participant ids) and the raw `duration`.
//!
//! ## Schema
//!
//! `web_conferences` and `web_conference_participants` already exist
//! (`star-db` migration `0225_canvas_schema_tables_c.sql`), no new DDL is
//! needed. The columns this module reads and writes:
//!
//! ```sql
//! CREATE TABLE web_conferences (
//!     id                               BIGSERIAL PRIMARY KEY,
//!     title                            varchar(255) NOT NULL,
//!     conference_type                  varchar(255) NOT NULL,
//!     conference_key                   varchar(255),
//!     context_id                       bigint NOT NULL,
//!     context_type                     varchar(255) NOT NULL,
//!     user_ids                         varchar(255),
//!     added_user_ids                   varchar(255),
//!     user_id                          bigint,
//!     started_at                       timestamp,
//!     description                      text,
//!     duration                         double precision,
//!     created_at                       timestamp NOT NULL,
//!     updated_at                       timestamp NOT NULL,
//!     uuid                             varchar(255),
//!     invited_user_ids                 varchar(255),
//!     ended_at                         timestamp,
//!     start_at                         timestamp,
//!     end_at                           timestamp,
//!     context_code                     varchar(255),
//!     type                             varchar(255),
//!     settings                         text,
//!     recording_ready                  boolean,
//!     root_account_id                  bigint NOT NULL
//! );
//!
//! CREATE TABLE web_conference_participants (
//!     id                               BIGSERIAL PRIMARY KEY,
//!     user_id                          bigint,
//!     web_conference_id                bigint,
//!     participation_type               varchar(255),
//!     created_at                       timestamp NOT NULL,
//!     updated_at                       timestamp NOT NULL,
//!     root_account_id                  bigint NOT NULL
//! );
//! ```
//!
//! `calendar_events` has no `web_conference_id` column in star-lms, so the
//! calendar event `create` links to the conference is written without the
//! back-reference. Adding it would make the link possible:
//!
//! ```sql
//! ALTER TABLE calendar_events ADD COLUMN IF NOT EXISTS web_conference_id bigint;
//! ```

use std::collections::BTreeMap;

use axum::{
    body::Bytes,
    extract::{Path, Query, RawQuery, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration, NaiveDateTime, Utc};
use serde_json::{json, Map, Value};
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

/// Canvas `WebConference::MAX_DURATION`.
const MAX_DURATION: i64 = 99_999_999;

/// Canvas `WebConference` title length (`validates :title, length: { within:
/// 0..255 }`).
const MAX_TITLE_LENGTH: usize = 255;

/// Canvas `maximum_text_length` (the description is a `text` column).
const MAX_TEXT_LENGTH: usize = 255;

/// Canvas `RoleOverride::GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS`.
const GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS: [&str; 3] = [
    "manage_course_content_add",
    "manage_course_content_edit",
    "manage_course_content_delete",
];

/// Canvas `BigBlueButtonConference.user_setting_fields` — the settings that
/// `WebConference#merge_user_settings` copies into `settings`.
const USER_SETTING_FIELDS: [&str; 9] = [
    "record",
    "scheduled_date",
    "create_time",
    "share_webcam",
    "share_microphone",
    "send_public_chat",
    "send_private_chat",
    "enable_waiting_room",
    "share_other_webcams",
];

/// The `user_setting_field` types that `WebConference#cast_setting` casts to
/// a boolean.
const BOOLEAN_SETTING_FIELDS: [&str; 6] = [
    "record",
    "share_webcam",
    "share_microphone",
    "send_public_chat",
    "send_private_chat",
    "enable_waiting_room",
];

/// The columns of a `web_conferences` row this module reads.
const CONFERENCE_COLUMNS: &str = "w.id, w.title, w.conference_type, w.conference_key, \
     w.context_id, w.context_type, w.start_at, w.description, w.duration, \
     w.ended_at, w.end_at, w.context_code";

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Domain router for the Canvas web-conferences routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{course_id}/conferences",
            get(index_course).post(create_course),
        )
        .route(
            "/groups/{group_id}/conferences",
            get(index_group).post(create_group),
        )
        .route(
            "/courses/{course_id}/conferences/{conference_id}/recording_ready",
            post(recording_ready_course),
        )
        .route(
            "/groups/{group_id}/conferences/{conference_id}/recording_ready",
            post(recording_ready_group),
        )
        .route("/conferences", get(for_user))
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// A `web_conferences` row as this module needs it.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ConferenceRow {
    id: i64,
    title: String,
    conference_type: String,
    conference_key: Option<String>,
    context_id: i64,
    context_type: String,
    start_at: Option<DateTime<Utc>>,
    started_at: Option<DateTime<Utc>>,
    description: Option<String>,
    duration: Option<f64>,
    ended_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    context_code: Option<String>,
}

/// A `web_conference_participants` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ParticipantRow {
    web_conference_id: i64,
    user_id: i64,
}

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// The Canvas context a conference lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    Course(i64),
    Group(i64),
}

impl Context {
    /// Canvas `context_type` of the context.
    fn context_type(self) -> &'static str {
        match self {
            Context::Course(_) => "Course",
            Context::Group(_) => "Group",
        }
    }

    /// The id of the context.
    fn context_id(self) -> i64 {
        match self {
            Context::Course(id) | Context::Group(id) => id,
        }
    }

    /// The url segment of the context (`courses`, `groups`).
    fn url_segment(self) -> &'static str {
        match self {
            Context::Course(_) => "courses",
            Context::Group(_) => "groups",
        }
    }

    /// Canvas `context_code` (`course_5`, `group_7`).
    fn context_code(self) -> String {
        let prefix = match self {
            Context::Course(_) => "course",
            Context::Group(_) => "group",
        };
        format!("{prefix}_{}", self.context_id())
    }
}

/// A context that exists, plus the account its rows belong to.
struct ResolvedContext {
    context: Context,
    root_account_id: i64,
}

/// Canvas `require_context` — `Course.find` / `Group.find` raise
/// `ActiveRecord::RecordNotFound` -> 404.
async fn resolve_context(db: &PgPool, context: Context) -> Result<ResolvedContext, ApiError> {
    match context {
        Context::Course(course_id) => {
            let row: Option<(Option<i64>,)> = sqlx::query_as(
                "SELECT account_id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
            )
            .bind(course_id)
            .fetch_optional(db)
            .await?;
            let Some((account_id,)) = row else {
                return Err(ApiError::NotFound);
            };
            Ok(ResolvedContext {
                context,
                root_account_id: account_id.unwrap_or(1),
            })
        }
        Context::Group(group_id) => {
            let row: Option<(Option<i64>,)> = sqlx::query_as(
                "SELECT course_id FROM groups WHERE id = $1 AND workflow_state <> 'deleted'",
            )
            .bind(group_id)
            .fetch_optional(db)
            .await?;
            let Some((course_id,)) = row else {
                return Err(ApiError::NotFound);
            };
            let root_account_id = match course_id {
                Some(course_id) => course_account_id(db, course_id).await?,
                None => 1,
            };
            Ok(ResolvedContext {
                context,
                root_account_id,
            })
        }
    }
}

/// The account a course belongs to.
async fn course_account_id(db: &PgPool, course_id: i64) -> Result<i64, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    Ok(row.and_then(|(account_id,)| account_id).unwrap_or(1))
}

// ---------------------------------------------------------------------------
// Provider configuration
// ---------------------------------------------------------------------------

/// A configured web-conferencing provider (`plugin_settings` row).
struct Provider {
    /// The plugin id (`big_blue_button`).
    plugin_id: String,
    /// Canvas `conference_type` of the plugin (`BigBlueButton`).
    conference_type: String,
    /// The plugin's decrypted secret (`secret_dec`).
    secret: Option<String>,
}

/// The enabled web-conferencing plugins, in Canvas' registration order.
async fn load_providers(db: &PgPool) -> Result<Vec<Provider>, ApiError> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT name, settings FROM plugin_settings \
         WHERE disabled IS NOT TRUE AND settings IS NOT NULL",
    )
    .fetch_all(db)
    .await?;

    let mut providers: Vec<Provider> = rows
        .into_iter()
        .filter_map(|(plugin_id, settings)| {
            let settings = settings?;
            let parsed: Value = serde_json::from_str(&settings).ok()?;
            if !parsed.is_object() {
                return None;
            }
            let secret = parsed
                .get("secret_dec")
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(Provider {
                conference_type: classify(&plugin_id),
                plugin_id,
                secret,
            })
        })
        .collect();

    // Canvas registers the web-conferencing plugins in a fixed order
    // (`wimba` before `big_blue_button`); `WebConference.config` is the first
    // one, so the default conference type depends on that order.
    providers.sort_by(|a, b| {
        plugin_order(&a.plugin_id)
            .cmp(&plugin_order(&b.plugin_id))
            .then_with(|| a.plugin_id.cmp(&b.plugin_id))
    });
    Ok(providers)
}

/// The order Canvas registers the web-conferencing plugins in.
fn plugin_order(plugin_id: &str) -> u8 {
    match plugin_id {
        "wimba" => 0,
        "big_blue_button" => 1,
        _ => 2,
    }
}

/// Canvas `String#classify` for a plugin id (`big_blue_button` ->
/// `BigBlueButton`).
fn classify(plugin_id: &str) -> String {
    plugin_id
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Canvas `String#underscore` for a conference type (`BigBlueButton` ->
/// `big_blue_button`).
fn underscore(value: &str) -> String {
    let mut out = String::new();
    for (index, ch) in value.chars().enumerate() {
        if ch.is_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The conference types of the enabled providers (`WebConference.active`).
fn enabled_conference_types(providers: &[Provider]) -> Vec<String> {
    providers
        .iter()
        .map(|provider| provider.conference_type.clone())
        .collect()
}

/// The API rendering of Canvas' `require_config` failure.
fn config_missing_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "errors": [{ "message": "Web conferencing has not been enabled for this Canvas site" }]
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Rails' `params` — the query string merged with the request body.
///
/// Canvas accepts a JSON body and a form-urlencoded body; both are parsed
/// into the same shape (`user[1]=1` is a hash, `user[]=1` is an array).
#[derive(Debug, Default)]
struct Params {
    root: Value,
}

impl Params {
    /// Merge a form-urlencoded body into the parameters.
    fn parse_form(&mut self, body: &str) {
        for pair in body.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((key, value)) => (percent_decode(key), percent_decode(value)),
                None => (percent_decode(pair), String::new()),
            };
            insert_path(&mut self.root, &key, Value::String(value));
        }
    }

    /// Merge a JSON body into the parameters.
    fn parse_json(&mut self, body: &[u8]) {
        let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) else {
            return;
        };
        for (key, value) in map {
            insert_path(&mut self.root, &key, value);
        }
    }

    /// The value of a parameter.
    fn value(&self, name: &str) -> Option<&Value> {
        self.root.get(name)
    }

    /// The value of a parameter as a hash.
    fn object(&self, name: &str) -> Option<&Map<String, Value>> {
        self.value(name).and_then(Value::as_object)
    }
}

/// Insert `key=value` into a Rails-style parameter tree.
///
/// `web_conference[title]` becomes a nested hash, `user[]` an array.
fn insert_path(root: &mut Value, key: &str, value: Value) {
    let mut parts: Vec<String> = Vec::new();
    let mut rest = key;
    if let Some(open) = rest.find('[') {
        parts.push(rest[..open].to_string());
        rest = &rest[open..];
        while let Some(close) = rest.find(']') {
            parts.push(rest[1..close].to_string());
            rest = &rest[close + 1..];
        }
    } else {
        parts.push(rest.to_string());
    }
    set_path(root, &parts, value);
}

/// Write `value` at `parts` of the parameter tree.
fn set_path(node: &mut Value, parts: &[String], value: Value) {
    let Some((head, tail)) = parts.split_first() else {
        return;
    };
    if !node.is_object() {
        *node = Value::Object(Map::new());
    }
    let Some(map) = node.as_object_mut() else {
        return;
    };
    if tail.is_empty() {
        map.insert(head.clone(), value);
        return;
    }
    if tail[0].is_empty() {
        // `user[]=1` — an array.
        let entry = map.entry(head.clone()).or_insert(Value::Array(Vec::new()));
        if let Some(list) = entry.as_array_mut() {
            list.push(value);
        }
        return;
    }
    let entry = map
        .entry(head.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    set_path(entry, tail, value);
}

/// The scalar rendering of a JSON value.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Percent-decode a form value (`+` is a space).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = &input[index + 1..index + 3];
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Canvas `value_to_boolean`.
fn value_to_boolean(value: Option<&str>) -> bool {
    match value {
        Some(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "t" | "yes"
        ),
        None => false,
    }
}

/// Canvas `value_to_boolean` for a parameter that may be a hash — a hash is
/// truthy, a string follows the usual truthy strings.
fn truthy_param(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Object(_)) | Some(Value::Array(_)) => true,
        Some(Value::Null) | None => false,
        Some(other) => value_to_boolean(scalar_text(other).as_deref()),
    }
}

/// The query parameters of a request (`per_page`, `page`, `state`, …).
type QueryParams = BTreeMap<String, String>;

/// The pagination of a Canvas API listing.
struct Pagination {
    page: i64,
    per_page: i64,
    offset: i64,
}

/// Canvas `Api.paginate` — `per_page` defaults to `Api::PER_PAGE` and is
/// capped at `Api::MAX_PER_PAGE`.
fn pagination(query: &QueryParams) -> Pagination {
    let per_page = query
        .get("per_page")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = query
        .get("page")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    Pagination {
        page,
        per_page,
        offset: (page - 1) * per_page,
    }
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

/// Whether `user_id` may read the contents of the course.
///
/// Canvas `authorized_action(@context, @current_user, :read)`.
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
async fn can_read_context(
    db: &PgPool,
    auth: &AuthUser,
    context: Context,
) -> Result<bool, ApiError> {
    match context {
        Context::Course(course_id) => can_read_course(db, auth.id, course_id).await,
        Context::Group(group_id) => can_read_group(db, auth.id, group_id).await,
    }
}

/// Canvas `authorized_action(@context.web_conferences.temp_record,
/// @current_user, :create)` — the context must grant `create_conferences`.
///
/// Canvas grants `create_conferences` to every enrolled role but the
/// observer (`config/initializers/permissions_registry.rb`); a group grants
/// it when the course the group belongs to grants it.
async fn can_create_conferences(
    db: &PgPool,
    auth: &AuthUser,
    context: Context,
) -> Result<bool, ApiError> {
    match context {
        Context::Course(course_id) => {
            let enrolled: Option<(i32,)> = sqlx::query_as(
                r#"
                SELECT 1
                FROM enrollments
                WHERE user_id = $1 AND course_id = $2
                  AND workflow_state IN ('active', 'invited')
                  AND role <> 'observer'
                LIMIT 1
                "#,
            )
            .bind(auth.id)
            .bind(course_id)
            .fetch_optional(db)
            .await?;
            if enrolled.is_some() {
                return Ok(true);
            }
            let account_id = course_account_id(db, course_id).await?;
            is_account_admin(db, auth.id, account_id).await
        }
        Context::Group(group_id) => {
            let course: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT course_id FROM groups WHERE id = $1")
                    .bind(group_id)
                    .fetch_optional(db)
                    .await?;
            let Some((Some(course_id),)) = course else {
                return Ok(false);
            };
            // A group resolves to its course, so the recursion is one level
            // deep. The compiler cannot prove that, so the recursive call is
            // boxed: an async fn that calls itself needs indirection.
            Box::pin(can_create_conferences(db, auth, Context::Course(course_id))).await
        }
    }
}

/// Whether `user_id` holds one of
/// `RoleOverride::GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS` in the course.
///
/// Those permissions are granted through role overrides; the course-level
/// overrides win over the inherited account-level ones.
async fn has_granular_content_right(
    db: &PgPool,
    user_id: i64,
    context: Context,
) -> Result<bool, ApiError> {
    let course_id = match context {
        Context::Course(course_id) => course_id,
        // A group has no role overrides of its own.
        Context::Group(_) => return Ok(false),
    };

    let roles: Vec<(String,)> = sqlx::query_as(
        "SELECT role FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND workflow_state IN ('active', 'invited')",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_all(db)
    .await?;
    let role_ids: Vec<i64> = roles.iter().map(|(role,)| builtin_role_id(role)).collect();
    if role_ids.is_empty() {
        return Ok(false);
    }

    let account_id = course_account_id(db, course_id).await?;
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
        FROM role_overrides ro
        WHERE ro.permission = ANY($1)
          AND ro.enabled = TRUE
          AND ro.role_id = ANY($3)
          AND (
            (ro.context_type = 'Account' AND ro.context_id IN (SELECT id FROM account_chain))
            OR (ro.context_type = 'Course' AND ro.context_id = $4)
          )
        LIMIT 1
        "#,
    )
    .bind(GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS.to_vec())
    .bind(account_id)
    .bind(role_ids)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// The id of the built-in role an enrollment role maps to (the ids
/// `star-permissions` uses for `role_overrides.role_id`).
fn builtin_role_id(role: &str) -> i64 {
    match role {
        "student" => 1,
        "teacher" => 2,
        "ta" => 3,
        "designer" => 4,
        "observer" => 5,
        _ => 1,
    }
}

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

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// The conferences of a context (Canvas `index`).
///
/// `participant` is `None` for the callers that hold one of
/// `GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS` (they see every conference
/// of the context) and `Some(user_id)` for everybody else (they see the
/// conferences they participate in).
async fn fetch_context_conferences(
    db: &PgPool,
    context: Context,
    types: &[String],
    participant: Option<i64>,
    pagination: &Pagination,
) -> Result<(Vec<ConferenceRow>, i64), ApiError> {
    let where_sql = "w.context_type = $1 AND w.context_id = $2 \
         AND w.conference_type::text = ANY($3) \
         AND ($4::bigint IS NULL OR EXISTS ( \
              SELECT 1 FROM web_conference_participants p \
              WHERE p.web_conference_id = w.id AND p.user_id = $4))";

    let list_sql = format!(
        "SELECT {CONFERENCE_COLUMNS} FROM web_conferences w \
         WHERE {where_sql} ORDER BY w.created_at DESC, w.id DESC \
         LIMIT $5 OFFSET $6"
    );
    let rows: Vec<ConferenceRow> = sqlx::query_as(&list_sql)
        .bind(context.context_type())
        .bind(context.context_id())
        .bind(types.to_vec())
        .bind(participant)
        .bind(pagination.per_page)
        .bind(pagination.offset)
        .fetch_all(db)
        .await?;

    let count_sql = format!("SELECT COUNT(*) FROM web_conferences w WHERE {where_sql}");
    let total: (i64,) = sqlx::query_as(&count_sql)
        .bind(context.context_type())
        .bind(context.context_id())
        .bind(types.to_vec())
        .bind(participant)
        .fetch_one(db)
        .await?;

    Ok((rows, total.0))
}

/// The conferences of the current user (Canvas `for_user`).
///
/// The user must participate in the conference and be an active member of
/// the course or group the conference belongs to.
async fn fetch_user_conferences(
    db: &PgPool,
    user_id: i64,
    types: &[String],
    live_only: bool,
    pagination: &Pagination,
) -> Result<(Vec<ConferenceRow>, i64), ApiError> {
    let where_sql = "w.conference_type::text = ANY($1) \
         AND EXISTS ( \
              SELECT 1 FROM web_conference_participants p \
              WHERE p.web_conference_id = w.id AND p.user_id = $2) \
         AND ( \
              (w.context_type = 'Course' AND EXISTS ( \
                   SELECT 1 FROM enrollments e \
                   WHERE e.user_id = $2 AND e.course_id = w.context_id \
                     AND e.workflow_state = 'active')) \
              OR (w.context_type = 'Group' AND EXISTS ( \
                   SELECT 1 FROM group_memberships m \
                   WHERE m.user_id = $2 AND m.group_id = w.context_id \
                     AND m.workflow_state = 'accepted')) \
         ) \
         AND ($3::boolean = false OR ( \
              w.started_at BETWEEN (NOW() - INTERVAL '1 day') AND NOW() \
              AND (w.ended_at IS NULL OR w.ended_at > NOW())))";

    let list_sql = format!(
        "SELECT {CONFERENCE_COLUMNS} FROM web_conferences w \
         WHERE {where_sql} ORDER BY w.created_at DESC, w.id DESC \
         LIMIT $4 OFFSET $5"
    );
    let rows: Vec<ConferenceRow> = sqlx::query_as(&list_sql)
        .bind(types.to_vec())
        .bind(user_id)
        .bind(live_only)
        .bind(pagination.per_page)
        .bind(pagination.offset)
        .fetch_all(db)
        .await?;

    let count_sql = format!("SELECT COUNT(*) FROM web_conferences w WHERE {where_sql}");
    let total: (i64,) = sqlx::query_as(&count_sql)
        .bind(types.to_vec())
        .bind(user_id)
        .bind(live_only)
        .fetch_one(db)
        .await?;

    Ok((rows, total.0))
}

/// The participants of the given conferences.
async fn fetch_participants(
    db: &PgPool,
    conferences: &[ConferenceRow],
) -> Result<BTreeMap<i64, Vec<i64>>, ApiError> {
    let mut participants: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    if conferences.is_empty() {
        return Ok(participants);
    }
    let ids: Vec<i64> = conferences.iter().map(|row| row.id).collect();
    let rows: Vec<ParticipantRow> = sqlx::query_as(
        "SELECT web_conference_id, user_id FROM web_conference_participants \
         WHERE web_conference_id = ANY($1) ORDER BY id",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    for row in rows {
        participants
            .entry(row.web_conference_id)
            .or_default()
            .push(row.user_id);
    }
    Ok(participants)
}

/// The ids of the users of the context (`Course#user_ids` / `Group#user_ids`).
async fn context_user_ids(db: &PgPool, context: Context) -> Result<Vec<i64>, ApiError> {
    let ids: Vec<i64> = match context {
        Context::Course(course_id) => {
            sqlx::query_scalar(
                "SELECT DISTINCT user_id FROM enrollments WHERE course_id = $1 ORDER BY user_id",
            )
            .bind(course_id)
            .fetch_all(db)
            .await?
        }
        Context::Group(group_id) => sqlx::query_scalar(
            "SELECT DISTINCT user_id FROM group_memberships WHERE group_id = $1 ORDER BY user_id",
        )
        .bind(group_id)
        .fetch_all(db)
        .await?,
    };
    Ok(ids)
}

/// The ids of the observers of the course (`Course#observers`).
async fn course_observer_ids(db: &PgPool, course_id: i64) -> Result<Vec<i64>, ApiError> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT user_id FROM enrollments \
         WHERE course_id = $1 AND role = 'observer' ORDER BY user_id",
    )
    .bind(course_id)
    .fetch_all(db)
    .await?;
    Ok(ids)
}

/// The users of the context that actually take part in it
/// (`Course#participating_users` / `Group#participating_users_in_context`).
async fn participating_users(
    db: &PgPool,
    context: Context,
    user_ids: &[i64],
) -> Result<Vec<i64>, ApiError> {
    if user_ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = match context {
        Context::Course(course_id) => {
            sqlx::query_scalar(
                "SELECT DISTINCT user_id FROM enrollments \
                 WHERE course_id = $1 AND user_id = ANY($2) AND workflow_state = 'active' \
                 ORDER BY user_id",
            )
            .bind(course_id)
            .bind(user_ids.to_vec())
            .fetch_all(db)
            .await?
        }
        Context::Group(group_id) => {
            let course_id: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT course_id FROM groups WHERE id = $1")
                    .bind(group_id)
                    .fetch_optional(db)
                    .await?;
            let mut ids: Vec<i64> = sqlx::query_scalar(
                "SELECT DISTINCT user_id FROM group_memberships \
                 WHERE group_id = $1 AND user_id = ANY($2) ORDER BY user_id",
            )
            .bind(group_id)
            .bind(user_ids.to_vec())
            .fetch_all(db)
            .await?;
            // `Group#participating_users_in_context` drops the members that
            // are not actively enrolled in the course the group belongs to.
            if let Some((Some(course_id),)) = course_id {
                let enrolled: Vec<i64> = sqlx::query_scalar(
                    "SELECT DISTINCT user_id FROM enrollments \
                     WHERE course_id = $1 AND user_id = ANY($2) AND workflow_state = 'active'",
                )
                .bind(course_id)
                .bind(ids.clone())
                .fetch_all(db)
                .await?;
                ids.retain(|id| enrolled.contains(id));
            }
            ids
        }
    };
    Ok(ids)
}

// ---------------------------------------------------------------------------
// Serializers
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::Conferences#api_conference_json`.
fn api_conference_json(row: &ConferenceRow, participant_ids: &[i64], base: &str) -> Value {
    json!({
        "id": row.id,
        "title": row.title,
        "description": row.description,
        "conference_type": row.conference_type,
        "duration": row.duration.map(|duration| duration as i64),
        "started_at": row.started_at,
        "ended_at": row.ended_at,
        "conference_key": row.conference_key,
        "context_type": row.context_type,
        "context_id": row.context_id,
        "start_at": row.start_at,
        "end_at": row.end_at,
        "join_url": Value::Null,
        "url": conference_url(base, &row.context_type, row.context_id, row.id),
        "has_advanced_settings": false,
        "long_running": false,
        "users": participant_ids,
        "invitees": Vec::<i64>::new(),
        "attendees": Vec::<i64>::new(),
    })
}

/// Canvas `WebConference#as_json` — the shape `conferences#create` renders.
fn model_conference_json(row: &ConferenceRow, participant_ids: &[i64], base: &str) -> Value {
    json!({
        "id": row.id,
        "title": row.title,
        "description": row.description,
        "conference_type": row.conference_type,
        "duration": row.duration,
        "started_at": row.started_at,
        "ended_at": row.ended_at,
        "user_ids": participant_ids,
        "context_id": row.context_id,
        "context_type": row.context_type,
        "context_code": row.context_code,
        "start_at": row.start_at,
        "end_at": row.end_at,
        "join_url": Value::Null,
        "url": conference_url(base, &row.context_type, row.context_id, row.id),
    })
}

/// Canvas `named_context_url(conference.context, :context_conference_url,
/// conference)`.
fn conference_url(base: &str, context_type: &str, context_id: i64, conference_id: i64) -> String {
    let prefix = match context_type {
        "Course" => "courses",
        "Group" => "groups",
        _ => "contexts",
    };
    format!("{base}/{prefix}/{context_id}/conferences/{conference_id}")
}

/// The base URL of the request (`x-forwarded-proto` + `Host`).
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

/// The query string of the request, without the pagination parameters.
fn link_query(query: &QueryParams) -> String {
    query
        .iter()
        .filter(|(key, _)| key.as_str() != "page" && key.as_str() != "per_page")
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

// ---------------------------------------------------------------------------
// index
// ---------------------------------------------------------------------------

/// `GET /api/v1/courses/:course_id/conferences`.
async fn index_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<QueryParams>,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    index_conferences(&state, &headers, &auth, Context::Course(course_id), &query).await
}

/// `GET /api/v1/groups/:group_id/conferences`.
async fn index_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(query): Query<QueryParams>,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    index_conferences(&state, &headers, &auth, Context::Group(group_id), &query).await
}

/// The shared implementation of both `index` routes.
async fn index_conferences(
    state: &AppState,
    headers: &HeaderMap,
    auth: &AuthUser,
    context: Context,
    query: &QueryParams,
) -> Result<Response, ApiError> {
    let resolved = resolve_context(&state.db, context).await?;

    // Canvas `require_config`.
    let providers = load_providers(&state.db).await?;
    if providers.is_empty() {
        return Ok(config_missing_response());
    }

    // Canvas `authorized_action(@context, @current_user, :read)`.
    if !can_read_context(&state.db, auth, resolved.context).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas `tab_enabled?(Course::TAB_CONFERENCES)`: the tab is available to
    // every reader of the context (`delete_unless.call([... TAB_CONFERENCES
    // ...], :read, *GRANULAR_MANAGE_COURSE_CONTENT_PERMISSIONS)`), and the
    // action already required `:read`, so the tab check passes.

    let manager = has_granular_content_right(&state.db, auth.id, resolved.context).await?;
    let participant = if manager { None } else { Some(auth.id) };

    let types = enabled_conference_types(&providers);
    let pagination = pagination(query);
    let (rows, total) = fetch_context_conferences(
        &state.db,
        resolved.context,
        &types,
        participant,
        &pagination,
    )
    .await?;

    let participants = fetch_participants(&state.db, &rows).await?;
    let base = base_url(headers);
    let conferences: Vec<Value> = rows
        .iter()
        .map(|row| {
            let ids = participants.get(&row.id).map(Vec::as_slice).unwrap_or(&[]);
            api_conference_json(row, ids, &base)
        })
        .collect();

    let body = Json(json!({ "conferences": conferences }));
    let path = format!(
        "{base}/{}/{}/conferences",
        resolved.context.url_segment(),
        resolved.context.context_id()
    );
    Ok(with_pagination_link(
        body,
        &pagination,
        total,
        &path,
        &link_query(query),
    ))
}

/// Attach the `Link` header Canvas' `Api.paginate` builds.
fn with_pagination_link(
    body: Json<Value>,
    pagination: &Pagination,
    total: i64,
    url: &str,
    query: &str,
) -> Response {
    let links = pagination_links(url, query, pagination.page, pagination.per_page, total);
    let mut response = body.into_response();
    let link_header = links.and_then(|links| header::HeaderValue::from_str(&links).ok());
    if let Some(value) = link_header {
        response.headers_mut().insert(header::LINK, value);
    }
    response
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

/// `POST /api/v1/courses/:course_id/conferences`.
async fn create_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<QueryParams>,
    headers: HeaderMap,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let params = request_params(Some(&query), &body);
    create_conference(&state, &headers, &auth, Context::Course(course_id), &params).await
}

/// `POST /api/v1/groups/:group_id/conferences`.
async fn create_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    Query(query): Query<QueryParams>,
    headers: HeaderMap,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let params = request_params(Some(&query), &body);
    create_conference(&state, &headers, &auth, Context::Group(group_id), &params).await
}

/// The parameters of a request (query string and body).
fn request_params(query: Option<&QueryParams>, body: &Bytes) -> Params {
    let mut params = Params::default();
    if let Some(query) = query {
        for (key, value) in query {
            insert_path(&mut params.root, key, Value::String(value.clone()));
        }
    }
    let text = String::from_utf8_lossy(body);
    if text.trim_start().starts_with('{') {
        params.parse_json(body);
    } else {
        params.parse_form(&text);
    }
    params
}

/// The shared implementation of both `create` routes.
async fn create_conference(
    state: &AppState,
    headers: &HeaderMap,
    auth: &AuthUser,
    context: Context,
    params: &Params,
) -> Result<Response, ApiError> {
    let resolved = resolve_context(&state.db, context).await?;

    // Canvas `require_config`.
    let providers = load_providers(&state.db).await?;
    if providers.is_empty() {
        return Ok(config_missing_response());
    }

    // Canvas `authorized_action(@context.web_conferences.temp_record,
    // @current_user, :create)`.
    if !can_create_conferences(&state.db, auth, resolved.context).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas `params.require(:web_conference)`.
    let Some(web_conference) = params.object("web_conference") else {
        return Err(ApiError::BadRequest(
            "param is missing or the value is empty: web_conference".to_string(),
        ));
    };

    // `sync_attendees` is permitted but `WebConference` has no such
    // attribute: Rails raises `ActiveModel::UnknownAttributeError`, which
    // the API answers with a 400.
    if web_conference.contains_key("sync_attendees") {
        return Err(ApiError::BadRequest(
            "unknown attribute 'sync_attendees' for WebConference".to_string(),
        ));
    }

    let attributes =
        ConferenceAttributes::from_params(resolved.context, web_conference, &providers);
    let mut errors: Vec<(String, String)> = Vec::new();
    attributes.validate(&mut errors);
    if !errors.is_empty() {
        return Ok(validation_errors(errors));
    }

    let base = base_url(headers);
    let conference_id = insert_conference(
        state,
        resolved.context,
        resolved.root_account_id,
        auth.id,
        &attributes,
        &base,
        params,
    )
    .await?;

    // Canvas `member_ids` + `add_initiator` + `invite_users_from_context`.
    let member_ids = member_ids(state, resolved.context, auth.id, params).await?;
    add_initiator(state, conference_id, auth.id, resolved.root_account_id).await?;
    invite_users(
        state,
        resolved.context,
        conference_id,
        &member_ids,
        resolved.root_account_id,
    )
    .await?;

    if truthy_param(
        params
            .object("web_conference")
            .and_then(|attrs| attrs.get("calendar_event")),
    ) {
        create_calendar_event(state, resolved.context, &attributes).await?;
    }

    let row = load_conference(state, conference_id).await?;
    let participants = fetch_participants(&state.db, std::slice::from_ref(&row)).await?;
    let ids = participants.get(&row.id).map(Vec::as_slice).unwrap_or(&[]);
    Ok(Json(model_conference_json(&row, ids, &base)).into_response())
}

/// The attributes of a new conference.
struct ConferenceAttributes {
    title: String,
    description: Option<String>,
    conference_type: String,
    duration: Option<f64>,
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    user_settings: Map<String, Value>,
    lti_settings: Option<Value>,
}

impl ConferenceAttributes {
    /// Read the permitted parameters of `web_conference`.
    fn from_params(context: Context, params: &Map<String, Value>, providers: &[Provider]) -> Self {
        // Canvas `WebConference#conference_type=`: an unknown type leaves the
        // attribute empty, so the presence validation fails.
        let conference_type = match params.get("conference_type").and_then(scalar_text) {
            Some(value) if !value.trim().is_empty() => {
                let value = value.trim();
                let known = providers
                    .iter()
                    .any(|provider| provider.conference_type == value);
                if value == "LtiConference" || known {
                    value.to_string()
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        };

        let user_settings = params
            .get("user_settings")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let lti_settings = params.get("lti_settings").cloned();

        // Canvas `infer_conference_details`: a blank title gets the default
        // name of the context.
        let title = params
            .get("title")
            .and_then(scalar_text)
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| match context {
                Context::Course(_) => "Course Web Conference".to_string(),
                Context::Group(_) => "Group Web Conference".to_string(),
            });

        Self {
            title,
            description: params.get("description").and_then(scalar_text),
            conference_type,
            duration: params
                .get("duration")
                .and_then(scalar_text)
                .and_then(|value| value.trim().parse::<f64>().ok()),
            start_at: params
                .get("start_at")
                .and_then(scalar_text)
                .and_then(|v| parse_time(&v)),
            end_at: params
                .get("end_at")
                .and_then(scalar_text)
                .and_then(|v| parse_time(&v)),
            user_settings,
            lti_settings,
        }
    }

    /// The Canvas validations of `WebConference`.
    fn validate(&self, errors: &mut Vec<(String, String)>) {
        if self.conference_type.is_empty() {
            errors.push(("conference_type".to_string(), "can't be blank".to_string()));
        }
        if self.title.chars().count() > MAX_TITLE_LENGTH {
            errors.push((
                "title".to_string(),
                format!("is too long (maximum is {MAX_TITLE_LENGTH} characters)"),
            ));
        }
        if let Some(description) = &self.description {
            if description.chars().count() > MAX_TEXT_LENGTH {
                errors.push((
                    "description".to_string(),
                    format!("is too long (maximum is {MAX_TEXT_LENGTH} characters)"),
                ));
            }
        }
        if let Some(duration) = self.duration {
            if duration > MAX_DURATION as f64 {
                errors.push((
                    "duration".to_string(),
                    format!("is less than or equal to {MAX_DURATION}"),
                ));
            }
        }
        if self.conference_type == "LtiConference" {
            let tool_id = self
                .lti_settings
                .as_ref()
                .and_then(|settings| settings.get("tool_id"))
                .and_then(scalar_text)
                .map(|tool_id| tool_id.trim().to_string())
                .filter(|tool_id| !tool_id.is_empty());
            if tool_id.is_none() {
                errors.push((
                    "settings".to_string(),
                    "settings[lti_settings][tool_id] must exist for LtiConference".to_string(),
                ));
            }
        }
    }
}

/// The Canvas validation errors of a conference (`render json:
/// @conference.errors, status: :bad_request`).
fn validation_errors(errors: Vec<(String, String)>) -> Response {
    let mut body: Map<String, Value> = Map::new();
    for (field, message) in errors {
        let entry = body
            .entry(field)
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(list) = entry.as_array_mut() {
            list.push(Value::String(message));
        }
    }
    (StatusCode::BAD_REQUEST, Json(Value::Object(body))).into_response()
}

/// Parse a Canvas timestamp (`start_at`, `end_at`).
fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(value) {
        return Some(parsed.with_timezone(&Utc));
    }
    if let Ok(parsed) = NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%S") {
        return Some(parsed.and_utc());
    }
    if let Ok(parsed) = NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S") {
        return Some(parsed.and_utc());
    }
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|naive| naive.and_utc())
}

/// Write the new conference row.
async fn insert_conference(
    state: &AppState,
    context: Context,
    root_account_id: i64,
    user_id: i64,
    attributes: &ConferenceAttributes,
    base: &str,
    params: &Params,
) -> Result<i64, ApiError> {
    // Canvas `infer_conference_details`: the end time defaults to the start
    // time plus the duration.
    let start_at = attributes.start_at;
    let end_at = match attributes.end_at {
        Some(end_at) => Some(end_at),
        None => match (start_at, attributes.duration) {
            (Some(start_at), Some(duration)) => {
                Some(start_at + Duration::seconds((duration * 60.0) as i64))
            }
            _ => None,
        },
    };

    // Canvas `@conference.settings[:default_return_url]`.
    let mut settings: Map<String, Value> = Map::new();
    settings.insert(
        "default_return_url".to_string(),
        Value::String(format!(
            "{base}/{}/{}",
            context.url_segment(),
            context.context_id()
        )),
    );

    // Canvas: `invite_all_enabled` / `remove_observers_enabled`.
    let remove_observers = value_to_boolean(
        params
            .object("observers")
            .and_then(|observers| observers.get("remove"))
            .and_then(scalar_text)
            .as_deref(),
    );
    let user_all = params
        .object("user")
        .and_then(|user| user.get("all"))
        .and_then(scalar_text);
    let invite_all =
        params.value("user").is_none() || value_to_boolean(user_all.as_deref()) || remove_observers;
    settings.insert("invite_all".to_string(), Value::Bool(invite_all));
    if remove_observers {
        settings.insert("remove_observers".to_string(), Value::Bool(true));
    }

    // Canvas `lti_settings=`.
    if let Some(lti_settings) = &attributes.lti_settings {
        settings.insert("lti_settings".to_string(), lti_settings.clone());
    }

    // Canvas `merge_user_settings`: only the known user setting fields are
    // copied into the settings.
    for field in USER_SETTING_FIELDS {
        let Some(value) = attributes.user_settings.get(field) else {
            continue;
        };
        let Some(value) = scalar_text(value) else {
            continue;
        };
        let value = if BOOLEAN_SETTING_FIELDS.contains(&field) {
            Value::Bool(value_to_boolean(Some(&value)))
        } else {
            Value::String(value)
        };
        settings.insert(field.to_string(), value);
    }

    let conference_id: i64 = sqlx::query_scalar(
        "INSERT INTO web_conferences ( \
             title, conference_type, context_id, context_type, user_id, description, \
             duration, start_at, end_at, context_code, settings, added_user_ids, \
             created_at, updated_at, recording_ready, root_account_id, uuid \
         ) VALUES ( \
             $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, '', NOW(), NOW(), FALSE, $12, \
             gen_random_uuid()::text \
         ) RETURNING id",
    )
    .bind(attributes.title.as_str())
    .bind(attributes.conference_type.as_str())
    .bind(context.context_id())
    .bind(context.context_type())
    .bind(user_id)
    .bind(attributes.description.as_deref())
    .bind(attributes.duration)
    .bind(start_at.map(|value| value.naive_utc()))
    .bind(end_at.map(|value| value.naive_utc()))
    .bind(context.context_code())
    .bind(Value::Object(settings).to_string())
    .bind(root_account_id)
    .fetch_one(&state.db)
    .await?;
    Ok(conference_id)
}

/// Canvas `create_or_update_calendar_event_for_conference` — only course
/// conferences get a calendar event.
async fn create_calendar_event(
    state: &AppState,
    context: Context,
    attributes: &ConferenceAttributes,
) -> Result<(), ApiError> {
    let Context::Course(course_id) = context else {
        return Ok(());
    };
    let start_at = attributes.start_at.unwrap_or_else(Utc::now);
    sqlx::query(
        "INSERT INTO calendar_events \
             (course_id, user_id, title, start_at, end_at, workflow_state, created_at, updated_at) \
         VALUES ($1, NULL, $2, $3, $4, 'active', NOW(), NOW())",
    )
    .bind(course_id)
    .bind(attributes.title.as_str())
    .bind(start_at)
    .bind(attributes.end_at)
    .execute(&state.db)
    .await?;
    Ok(())
}

/// Load a conference row.
async fn load_conference(state: &AppState, conference_id: i64) -> Result<ConferenceRow, ApiError> {
    let select_sql = format!("SELECT {CONFERENCE_COLUMNS} FROM web_conferences w WHERE w.id = $1");
    let row: ConferenceRow = sqlx::query_as(&select_sql)
        .bind(conference_id)
        .fetch_one(&state.db)
        .await?;
    Ok(row)
}

/// Canvas `member_ids` — the users that are invited to the conference.
async fn member_ids(
    state: &AppState,
    context: Context,
    current_user_id: i64,
    params: &Params,
) -> Result<Vec<i64>, ApiError> {
    let remove_observers = params
        .object("observers")
        .and_then(|observers| observers.get("remove"))
        .and_then(scalar_text)
        .map(|value| value == "1")
        .unwrap_or(false);

    if remove_observers {
        let mut ids = vec![current_user_id];
        let users = context_user_ids(&state.db, context).await?;
        let observers = match context {
            Context::Course(course_id) => course_observer_ids(&state.db, course_id).await?,
            Context::Group(_) => Vec::new(),
        };
        ids.extend(users.into_iter().filter(|id| !observers.contains(id)));
        return Ok(ids);
    }

    if let Some(user) = params.value("user") {
        match user {
            Value::Object(map) => {
                if map.get("all").and_then(scalar_text).as_deref() != Some("1") {
                    // Rails iterates the hash: `id.to_i if val == "1"`.
                    return Ok(map
                        .iter()
                        .filter(|(_, value)| scalar_text(value).as_deref() == Some("1"))
                        .filter_map(|(key, _)| key.parse::<i64>().ok())
                        .filter(|id| *id > 0)
                        .collect());
                }
            }
            // An array yields `(value, nil)` pairs, so no id is ever selected.
            Value::Array(_) => return Ok(Vec::new()),
            _ => {}
        }
    }

    context_user_ids(&state.db, context).await
}

/// Canvas `WebConference#add_initiator`.
async fn add_initiator(
    state: &AppState,
    conference_id: i64,
    user_id: i64,
    root_account_id: i64,
) -> Result<(), ApiError> {
    upsert_participant(state, conference_id, user_id, "initiator", root_account_id).await
}

/// Canvas `WebConference#invite_users_from_context`.
async fn invite_users(
    state: &AppState,
    context: Context,
    conference_id: i64,
    member_ids: &[i64],
    root_account_id: i64,
) -> Result<(), ApiError> {
    let members = participating_users(&state.db, context, member_ids).await?;
    for member_id in members {
        upsert_participant(state, conference_id, member_id, "invitee", root_account_id).await?;
    }
    Ok(())
}

/// Canvas `WebConference#add_user` — reuse the participant row of the user.
async fn upsert_participant(
    state: &AppState,
    conference_id: i64,
    user_id: i64,
    participation_type: &str,
    root_account_id: i64,
) -> Result<(), ApiError> {
    let updated = sqlx::query(
        "UPDATE web_conference_participants SET participation_type = $1, updated_at = NOW() \
         WHERE web_conference_id = $2 AND user_id = $3",
    )
    .bind(participation_type)
    .bind(conference_id)
    .bind(user_id)
    .execute(&state.db)
    .await?;
    if updated.rows_affected() == 0 {
        sqlx::query(
            "INSERT INTO web_conference_participants \
                 (user_id, web_conference_id, participation_type, created_at, updated_at, \
                  root_account_id) \
             VALUES ($1, $2, $3, NOW(), NOW(), $4)",
        )
        .bind(user_id)
        .bind(conference_id)
        .bind(participation_type)
        .bind(root_account_id)
        .execute(&state.db)
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// for_user
// ---------------------------------------------------------------------------

/// `GET /api/v1/conferences`.
async fn for_user(
    State(state): State<AppState>,
    Query(query): Query<QueryParams>,
    headers: HeaderMap,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let providers = load_providers(&state.db).await?;
    if providers.is_empty() {
        // Canvas `return render json: api_conferences_json([], @current_user,
        // session) unless WebConference.config`.
        return Ok(Json(json!({ "conferences": [] })).into_response());
    }

    let types = enabled_conference_types(&providers);
    let live_only = query.get("state").map(String::as_str) == Some("live");
    let pagination = pagination(&query);
    let (rows, total) =
        fetch_user_conferences(&state.db, auth.id, &types, live_only, &pagination).await?;

    let participants = fetch_participants(&state.db, &rows).await?;
    let base = base_url(&headers);
    let conferences: Vec<Value> = rows
        .iter()
        .map(|row| {
            let ids = participants.get(&row.id).map(Vec::as_slice).unwrap_or(&[]);
            api_conference_json(row, ids, &base)
        })
        .collect();

    let body = Json(json!({ "conferences": conferences }));
    let path = format!("{base}/conferences");
    Ok(with_pagination_link(
        body,
        &pagination,
        total,
        &path,
        &link_query(&query),
    ))
}

// ---------------------------------------------------------------------------
// recording_ready
// ---------------------------------------------------------------------------

/// `POST /api/v1/courses/:course_id/conferences/:conference_id/recording_ready`.
async fn recording_ready_course(
    State(state): State<AppState>,
    Path((course_id, conference_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    body: Bytes,
) -> Result<Response, ApiError> {
    recording_ready(
        &state,
        Context::Course(course_id),
        conference_id,
        raw_query.as_deref(),
        &body,
    )
    .await
}

/// `POST /api/v1/groups/:group_id/conferences/:conference_id/recording_ready`.
async fn recording_ready_group(
    State(state): State<AppState>,
    Path((group_id, conference_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    body: Bytes,
) -> Result<Response, ApiError> {
    recording_ready(
        &state,
        Context::Group(group_id),
        conference_id,
        raw_query.as_deref(),
        &body,
    )
    .await
}

/// The shared implementation of both `recording_ready` routes.
///
/// Canvas skips `load_user`/`require_user` here: the provider authenticates
/// itself with a JWT signed with the provider secret.
async fn recording_ready(
    state: &AppState,
    context: Context,
    conference_id: i64,
    raw_query: Option<&str>,
    body: &Bytes,
) -> Result<Response, ApiError> {
    let resolved = resolve_context(&state.db, context).await?;

    // Canvas `require_config`.
    let providers = load_providers(&state.db).await?;
    if providers.is_empty() {
        return Ok(config_missing_response());
    }

    // Canvas `get_conference` — the conference has to be in the context.
    let select_sql = format!(
        "SELECT {CONFERENCE_COLUMNS} FROM web_conferences w \
         WHERE w.id = $1 AND w.context_type = $2 AND w.context_id = $3"
    );
    let row: ConferenceRow = sqlx::query_as(&select_sql)
        .bind(conference_id)
        .bind(resolved.context.context_type())
        .bind(resolved.context.context_id())
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    let Some(secret) = provider_secret(&providers, &row.conference_type) else {
        return Ok(invalid_jwt_token_response());
    };

    let Some(token) = signed_parameters(raw_query, body) else {
        return Ok(invalid_jwt_token_response());
    };

    let Some(claims) = verify_signed_parameters(&token, &secret) else {
        return Ok(invalid_jwt_token_response());
    };

    let meeting_id = claims.get("meeting_id").and_then(scalar_text);
    if meeting_id.as_deref() != row.conference_key.as_deref() {
        return Ok(signed_id_invalid_response());
    }

    // Canvas `WebConference#recording_ready!`.
    sqlx::query(
        "UPDATE web_conferences SET recording_ready = TRUE, updated_at = NOW() WHERE id = $1",
    )
    .bind(row.id)
    .execute(&state.db)
    .await?;

    Ok((StatusCode::ACCEPTED, Json(json!([]))).into_response())
}

/// The secret of the provider that owns the conference type.
fn provider_secret(providers: &[Provider], conference_type: &str) -> Option<String> {
    let plugin_id = underscore(conference_type);
    providers
        .iter()
        .find(|provider| provider.plugin_id == plugin_id)
        .and_then(|provider| provider.secret.clone())
}

/// The `signed_parameters` parameter of the request.
fn signed_parameters(raw_query: Option<&str>, body: &Bytes) -> Option<String> {
    if let Some(query) = raw_query {
        if let Some(value) = form_value(query, "signed_parameters") {
            return Some(value);
        }
    }

    let text = String::from_utf8_lossy(body);
    if text.trim_start().starts_with('{') {
        if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(body) {
            if let Some(value) = map.get("signed_parameters").and_then(scalar_text) {
                return Some(value);
            }
        }
        return None;
    }
    form_value(&text, "signed_parameters")
}

/// The value of a form-urlencoded parameter.
fn form_value(body: &str, name: &str) -> Option<String> {
    for pair in body.split('&') {
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (percent_decode(key), percent_decode(value)),
            None => (percent_decode(pair), String::new()),
        };
        if key == name {
            return Some(value);
        }
    }
    None
}

/// Canvas `Canvas::Security.decode_jwt` — verify the provider's JWT.
fn verify_signed_parameters(token: &str, secret: &str) -> Option<Value> {
    let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
    // Canvas does not require an `exp` claim and does not check one.
    validation.required_spec_claims.clear();
    validation.validate_exp = false;
    let data = jsonwebtoken::decode::<Value>(
        token,
        &jsonwebtoken::DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .ok()?;
    Some(data.claims)
}

/// Canvas `Api::V1::Conferences#invalid_jwt_token_json`.
fn invalid_jwt_token_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "status": "unauthorized",
            "errors": [{ "message": "JWT signature invalid" }]
        })),
    )
        .into_response()
}

/// Canvas `Api::V1::Conferences#signed_id_invalid_json`.
fn signed_id_invalid_response() -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({
            "status": "unprocessable entity",
            "errors": [{ "message": "Signed meeting id invalid" }]
        })),
    )
        .into_response()
}
