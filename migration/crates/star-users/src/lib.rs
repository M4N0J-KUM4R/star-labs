//! star-users — user CRUD + profile handlers.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use argon2::password_hash::{rand_core::OsRng, SaltString};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use star_auth::{hash_password, AuthUser};
use star_common::{ApiError, AppState};
use std::collections::BTreeMap;

// --------------------------------------------------------------------------
// Domain types
// --------------------------------------------------------------------------

/// User record matching Canvas API v1 shape.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct UserResponse {
    pub id: i64,
    pub name: String,
    pub sortable_name: String,
    pub email: String,
    pub role: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Request to update a user.
#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    pub name: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
}

/// Pagination query parameters.
#[derive(Debug, Deserialize)]
pub struct Pagination {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// Paginated list response.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PaginatedUsers {
    pub data: Vec<UserResponse>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// Profile record matching the Canvas API v1 `Profile` model.
///
/// Identity fields come from `users`; the editable fields live in
/// `user_profiles`. `short_name` falls back to `name` and `primary_email`
/// falls back to `users.email`, mirroring Canvas.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct ProfileResponse {
    pub id: i64,
    pub name: String,
    pub short_name: String,
    pub sortable_name: String,
    pub title: Option<String>,
    pub bio: Option<String>,
    pub primary_email: String,
    pub avatar_url: Option<String>,
    pub time_zone: String,
    pub locale: String,
}

/// Request to update the caller's profile. Every field is optional; omitted
/// fields keep their current value, matching Canvas' partial update behavior.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateProfileRequest {
    pub title: Option<String>,
    pub bio: Option<String>,
    pub primary_email: Option<String>,
    pub avatar_url: Option<String>,
    pub time_zone: Option<String>,
    pub locale: Option<String>,
}

/// Canvas `BOOLEAN_PREFS` from UsersController#settings. Unset preferences
/// read as false (`!!user.preferences[pref]`), which is why a user without a
/// row gets the `Default` all-false value.
#[derive(Debug, Clone, Default, serde::Serialize, sqlx::FromRow)]
pub struct UserSettings {
    pub manual_mark_as_read: bool,
    pub collapse_global_nav: bool,
    pub collapse_course_nav: bool,
    pub hide_dashcard_color_overlays: bool,
    pub release_notes_badge_disabled: bool,
    pub comment_library_suggestions_enabled: bool,
    pub elementary_dashboard_disabled: bool,
    pub default_to_block_editor: bool,
    pub widget_dashboard_user_preference: bool,
    pub widget_dashboard_dark_mode: bool,
}

/// Partial update body for PUT /users/{userId}/settings. Canvas coerces the
/// values with `value_to_boolean`, so booleans, numbers and the usual string
/// forms are accepted; omitted or null fields stay unchanged.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateUserSettingsRequest {
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub manual_mark_as_read: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub collapse_global_nav: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub collapse_course_nav: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub hide_dashcard_color_overlays: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub release_notes_badge_disabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub comment_library_suggestions_enabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub elementary_dashboard_disabled: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub default_to_block_editor: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub widget_dashboard_user_preference: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_bool")]
    pub widget_dashboard_dark_mode: Option<bool>,
}

/// Query params for GET /users/{userId}/settings. Canvas clients send
/// `include[]=mobile_settings`; a plain `include=` is accepted too.
#[derive(Debug, Default, Deserialize)]
pub struct SettingsQuery {
    #[serde(rename = "include[]")]
    pub include_bracket: Option<String>,
    pub include: Option<String>,
}

impl SettingsQuery {
    fn wants_mobile_settings(&self) -> bool {
        [self.include_bracket.as_deref(), self.include.as_deref()]
            .into_iter()
            .flatten()
            .flat_map(|value| value.split(','))
            .any(|value| value.trim() == "mobile_settings")
    }
}

/// Assignment object embedded in a Canvas todo item.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct TodoAssignment {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub points_possible: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One entry of GET /users/self/todo (Canvas lib/api/v1/todo_item.rb).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TodoItem {
    #[serde(rename = "type")]
    pub item_type: String,
    pub context_type: String,
    pub course_id: i64,
    pub context_name: String,
    pub context_short_name: String,
    pub assignment: TodoAssignment,
    pub html_url: String,
    pub ignore: String,
    pub ignore_permanently: String,
    /// Grading todos only; Canvas count of submissions still awaiting a score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub needs_grading_count: Option<i64>,
}

/// Raw todo row joining the assignment to its course (and grading count).
#[derive(Debug, Clone, sqlx::FromRow)]
struct TodoRow {
    id: i64,
    course_id: i64,
    title: String,
    description: Option<String>,
    points_possible: f64,
    due_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    course_name: String,
    course_code: String,
    needs_grading_count: Option<i64>,
}

impl TodoRow {
    fn into_item(self, item_type: &str) -> TodoItem {
        let html_url = if item_type == "grading" {
            format!(
                "/courses/{}/gradebook/speed_grader?assignment_id={}",
                self.course_id, self.id
            )
        } else {
            format!("/courses/{}/assignments/{}#submit", self.course_id, self.id)
        };
        TodoItem {
            item_type: item_type.to_string(),
            context_type: "Course".to_string(),
            course_id: self.course_id,
            context_name: self.course_name,
            context_short_name: self.course_code,
            ignore: format!(
                "/api/v1/users/self/todo/assignment_{}/{item_type}?permanent=0",
                self.id
            ),
            ignore_permanently: format!(
                "/api/v1/users/self/todo/assignment_{}/{item_type}?permanent=1",
                self.id
            ),
            needs_grading_count: self.needs_grading_count,
            assignment: TodoAssignment {
                id: self.id,
                course_id: self.course_id,
                title: self.title,
                description: self.description,
                points_possible: self.points_possible,
                due_at: self.due_at,
                created_at: self.created_at,
                updated_at: self.updated_at,
            },
            html_url,
        }
    }
}

/// Canvas Attachment shape for user-context files (metadata only — no upload
/// binary handling).
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct UserFileResponse {
    pub id: i64,
    pub user_id: i64,
    pub folder: String,
    pub folder_id: Option<i64>,
    pub filename: String,
    pub display_name: String,
    pub content_type: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for POST /users/{userId}/files. Canvas names the upload
/// `name`; `filename` is accepted as an alias used elsewhere in this API.
#[derive(Debug, Deserialize)]
pub struct CreateUserFileRequest {
    pub name: Option<String>,
    pub filename: Option<String>,
    pub display_name: Option<String>,
    pub content_type: Option<String>,
    pub size: Option<i64>,
    pub folder: Option<String>,
    pub folder_id: Option<i64>,
}

/// Canvas `UserColor` row (lib/api/v1/user_color.rb).
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct UserColorResponse {
    pub id: i64,
    pub user_id: i64,
    pub context: String,
    pub color: String,
}

/// Body for PUT /users/{userId}/colors/{context}.
#[derive(Debug, Deserialize)]
pub struct SetCustomColorRequest {
    pub hexcode: Option<String>,
}

/// Body for PUT /users/{userId}/dashboard_positions. Canvas form params send
/// string values, JSON clients send numbers, so values stay untyped here.
#[derive(Debug, Deserialize)]
pub struct UpdateDashboardPositionsRequest {
    pub dashboard_positions: BTreeMap<String, serde_json::Value>,
}

/// Body for POST /accounts/{accountId}/users (Canvas users#create).
#[derive(Debug, Default, Deserialize)]
pub struct CreateAccountUserRequest {
    #[serde(default)]
    pub user: AccountUserFields,
    #[serde(default)]
    pub pseudonym: AccountPseudonym,
    pub password: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AccountUserFields {
    pub name: Option<String>,
    pub sortable_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AccountPseudonym {
    pub unique_id: Option<String>,
}

// --------------------------------------------------------------------------
// Permission helpers
// --------------------------------------------------------------------------

/// Look up a user's role from the DB.
async fn get_user_role(pool: &PgPool, user_id: i64) -> Result<Option<String>, ApiError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT role FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(row.map(|(r,)| r))
}

fn is_admin(role: &str) -> bool {
    role == "admin"
}

fn is_self(auth_id: i64, user_id: i64) -> bool {
    auth_id == user_id
}

fn pagination_values(page: Option<i64>, per_page: Option<i64>) -> (i64, i64, i64) {
    let page = page.unwrap_or(1).max(1);
    let per_page = per_page.unwrap_or(50).clamp(1, 100);
    // These values come directly from query parameters. Saturation keeps a
    // request such as `?page=9223372036854775807` from overflowing and
    // panicking the handler before PostgreSQL can return an empty page.
    let offset = page.saturating_sub(1).saturating_mul(per_page);
    (page, per_page, offset)
}

/// Canvas' limits for the profile text fields (`maximum_string_length` and
/// `maximum_text_length`).
const MAX_PROFILE_TITLE_LEN: usize = 255;
const MAX_PROFILE_BIO_LEN: usize = 65_535;

fn validate_profile_update(payload: &UpdateProfileRequest) -> Result<(), ApiError> {
    if payload
        .title
        .as_ref()
        .is_some_and(|title| title.chars().count() > MAX_PROFILE_TITLE_LEN)
    {
        return Err(ApiError::BadRequest(format!(
            "title is too long (maximum is {MAX_PROFILE_TITLE_LEN} characters)"
        )));
    }
    if payload
        .bio
        .as_ref()
        .is_some_and(|bio| bio.chars().count() > MAX_PROFILE_BIO_LEN)
    {
        return Err(ApiError::BadRequest(format!(
            "bio is too long (maximum is {MAX_PROFILE_BIO_LEN} characters)"
        )));
    }
    Ok(())
}

/// Load the Canvas-shaped profile for a user. Works whether or not a
/// `user_profiles` row exists yet: missing rows yield Canvas defaults.
async fn load_profile(pool: &PgPool, user_id: i64) -> Result<ProfileResponse, ApiError> {
    let profile: Option<ProfileResponse> = sqlx::query_as(
        r#"
        SELECT u.id,
               u.name,
               u.name AS short_name,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               p.title,
               p.bio,
               COALESCE(NULLIF(p.primary_email, ''), u.email) AS primary_email,
               p.avatar_url,
               COALESCE(p.time_zone, 'UTC') AS time_zone,
               COALESCE(p.locale, 'en') AS locale
        FROM users u
        LEFT JOIN user_profiles p ON p.user_id = u.id
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;

    profile.ok_or(ApiError::NotFound)
}

/// Create or partially update the profile row, then return the merged result.
/// `None` means "leave unchanged"; empty strings intentionally clear a field.
async fn upsert_profile(
    pool: &PgPool,
    user_id: i64,
    payload: &UpdateProfileRequest,
) -> Result<ProfileResponse, ApiError> {
    let profile: Option<ProfileResponse> = sqlx::query_as(
        r#"
        WITH upsert AS (
            INSERT INTO user_profiles
                (user_id, title, bio, primary_email, avatar_url, time_zone, locale)
            VALUES ($1, $2, $3, $4, $5, COALESCE($6, 'UTC'), COALESCE($7, 'en'))
            ON CONFLICT (user_id) DO UPDATE
            SET title         = COALESCE($2, user_profiles.title),
                bio           = COALESCE($3, user_profiles.bio),
                primary_email = COALESCE($4, user_profiles.primary_email),
                avatar_url    = COALESCE($5, user_profiles.avatar_url),
                time_zone     = COALESCE($6, user_profiles.time_zone),
                locale        = COALESCE($7, user_profiles.locale),
                updated_at    = now()
            RETURNING user_id, title, bio, primary_email, avatar_url, time_zone, locale
        )
        SELECT u.id,
               u.name,
               u.name AS short_name,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               upsert.title,
               upsert.bio,
               COALESCE(NULLIF(upsert.primary_email, ''), u.email) AS primary_email,
               upsert.avatar_url,
               upsert.time_zone,
               upsert.locale
        FROM users u
        JOIN upsert ON upsert.user_id = u.id
        "#,
    )
    .bind(user_id)
    .bind(&payload.title)
    .bind(&payload.bio)
    .bind(&payload.primary_email)
    .bind(&payload.avatar_url)
    .bind(&payload.time_zone)
    .bind(&payload.locale)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;

    profile.ok_or(ApiError::NotFound)
}

// --------------------------------------------------------------------------
// Settings / todo / user files helpers
// --------------------------------------------------------------------------

/// Canvas `api_find(User, params[:id])`: the literal "self" resolves to the
/// caller and anything else must be a numeric id (non-numeric ids are 404).
fn resolve_path_user(raw: &str, auth_id: i64) -> Result<i64, ApiError> {
    if raw == "self" {
        return Ok(auth_id);
    }
    raw.parse::<i64>().map_err(|_| ApiError::NotFound)
}

/// Ensure the addressed user exists. Canvas resolves the user before the
/// permission check, so a missing id is a 404 even for non-admins.
async fn ensure_user_exists(pool: &PgPool, user_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?;
    if row.is_none() {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// Resolve `{userId}` (including "self") and enforce Canvas' read/manage rule:
/// users may only touch their own settings, admins may touch anyone's.
async fn authorize_settings(
    state: &AppState,
    auth: &AuthUser,
    raw_user_id: &str,
) -> Result<i64, ApiError> {
    let user_id = resolve_path_user(raw_user_id, auth.id)?;
    ensure_user_exists(&state.db, user_id).await?;
    if !is_self(auth.id, user_id) && !is_admin(&auth.role) {
        return Err(ApiError::Forbidden);
    }
    Ok(user_id)
}

/// Canvas `value_to_boolean` (lib/base/canvas/plugin.rb): booleans pass
/// through, numbers are truthy when non-zero, and the documented string forms
/// map to true/false. Missing/null values mean "leave unchanged".
fn deserialize_optional_bool<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| canvas_boolean(&value)))
}

fn canvas_boolean(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(value) => Some(*value),
        serde_json::Value::Number(number) => Some(number.as_f64().is_some_and(|n| n != 0.0)),
        serde_json::Value::String(text) => {
            // Canvas falls back to `value.to_i != 0`, so unrecognized strings
            // are false.
            let text = text.trim().to_ascii_lowercase();
            Some(matches!(
                text.as_str(),
                "yes" | "y" | "true" | "t" | "on" | "1"
            ))
        }
        _ => None,
    }
}

/// Load a user's settings; a missing row is Canvas' all-false default.
async fn load_user_settings(pool: &PgPool, user_id: i64) -> Result<UserSettings, ApiError> {
    let settings: Option<UserSettings> = sqlx::query_as(
        r#"
        SELECT manual_mark_as_read,
               collapse_global_nav,
               collapse_course_nav,
               hide_dashcard_color_overlays,
               release_notes_badge_disabled,
               comment_library_suggestions_enabled,
               elementary_dashboard_disabled,
               default_to_block_editor,
               widget_dashboard_user_preference,
               widget_dashboard_dark_mode
        FROM user_settings
        WHERE user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(settings.unwrap_or_default())
}

/// Create or partially update the settings row and return the full result.
/// Unspecified preferences default to false on insert (Canvas behavior) and
/// keep their current value on update.
async fn upsert_user_settings(
    pool: &PgPool,
    user_id: i64,
    payload: &UpdateUserSettingsRequest,
) -> Result<UserSettings, ApiError> {
    sqlx::query_as(
        r#"
        INSERT INTO user_settings (
            user_id,
            manual_mark_as_read,
            collapse_global_nav,
            collapse_course_nav,
            hide_dashcard_color_overlays,
            release_notes_badge_disabled,
            comment_library_suggestions_enabled,
            elementary_dashboard_disabled,
            default_to_block_editor,
            widget_dashboard_user_preference,
            widget_dashboard_dark_mode
        )
        VALUES (
            $1,
            COALESCE($2, FALSE),
            COALESCE($3, FALSE),
            COALESCE($4, FALSE),
            COALESCE($5, FALSE),
            COALESCE($6, FALSE),
            COALESCE($7, FALSE),
            COALESCE($8, FALSE),
            COALESCE($9, FALSE),
            COALESCE($10, FALSE),
            COALESCE($11, FALSE)
        )
        ON CONFLICT (user_id) DO UPDATE SET
            manual_mark_as_read = COALESCE($2, user_settings.manual_mark_as_read),
            collapse_global_nav = COALESCE($3, user_settings.collapse_global_nav),
            collapse_course_nav = COALESCE($4, user_settings.collapse_course_nav),
            hide_dashcard_color_overlays = COALESCE($5, user_settings.hide_dashcard_color_overlays),
            release_notes_badge_disabled = COALESCE($6, user_settings.release_notes_badge_disabled),
            comment_library_suggestions_enabled = COALESCE($7, user_settings.comment_library_suggestions_enabled),
            elementary_dashboard_disabled = COALESCE($8, user_settings.elementary_dashboard_disabled),
            default_to_block_editor = COALESCE($9, user_settings.default_to_block_editor),
            widget_dashboard_user_preference = COALESCE($10, user_settings.widget_dashboard_user_preference),
            widget_dashboard_dark_mode = COALESCE($11, user_settings.widget_dashboard_dark_mode),
            updated_at = now()
        RETURNING manual_mark_as_read,
                  collapse_global_nav,
                  collapse_course_nav,
                  hide_dashcard_color_overlays,
                  release_notes_badge_disabled,
                  comment_library_suggestions_enabled,
                  elementary_dashboard_disabled,
                  default_to_block_editor,
                  widget_dashboard_user_preference,
                  widget_dashboard_dark_mode
        "#,
    )
    .bind(user_id)
    .bind(payload.manual_mark_as_read)
    .bind(payload.collapse_global_nav)
    .bind(payload.collapse_course_nav)
    .bind(payload.hide_dashcard_color_overlays)
    .bind(payload.release_notes_badge_disabled)
    .bind(payload.comment_library_suggestions_enabled)
    .bind(payload.elementary_dashboard_disabled)
    .bind(payload.default_to_block_editor)
    .bind(payload.widget_dashboard_user_preference)
    .bind(payload.widget_dashboard_dark_mode)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)
}

// --------------------------------------------------------------------------
// Custom colors / dashboard positions / account users helpers
// --------------------------------------------------------------------------

/// Canvas `CustomColorHelper::HEX_REGEX` (`/^#?(\h{3}|\h{6})$/`) plus
/// `normalize_hexcode` (leading `#` added when missing).
fn normalize_hexcode(raw: &str) -> Option<String> {
    let digits = raw.strip_prefix('#').unwrap_or(raw);
    let valid = matches!(digits.len(), 3 | 6) && digits.chars().all(|c| c.is_ascii_hexdigit());
    valid.then(|| format!("#{digits}"))
}

/// Canvas `Context.find_by_asset_string`: `<type>_<id>` where the type names a
/// known context table and the id exists. Unknown/missing contexts are 404.
async fn asset_exists(pool: &PgPool, asset_string: &str) -> Result<bool, ApiError> {
    let Some((kind, id)) = asset_string.split_once('_') else {
        return Ok(false);
    };
    let Ok(id) = id.parse::<i64>() else {
        return Ok(false);
    };
    let table = match kind {
        "course" => "courses",
        "group" => "groups",
        "user" => "users",
        _ => return Ok(false),
    };
    let row: Option<(i32,)> = sqlx::query_as(&format!("SELECT 1 FROM {table} WHERE id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(row.is_some())
}

/// Load a user's saved colors as Canvas' `{asset_string: hexcode}` hash.
async fn load_custom_colors(
    pool: &PgPool,
    user_id: i64,
) -> Result<BTreeMap<String, String>, ApiError> {
    let rows: Vec<UserColorResponse> = sqlx::query_as(
        "SELECT id, user_id, context, color FROM user_colors WHERE user_id = $1 ORDER BY context",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows
        .into_iter()
        .map(|row| (row.context, row.color))
        .collect())
}

/// Load a user's dashboard positions as Canvas' `{asset_string: position}` map.
async fn load_dashboard_positions(
    pool: &PgPool,
    user_id: i64,
) -> Result<BTreeMap<String, i64>, ApiError> {
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT course_id, position FROM dashboard_positions WHERE user_id = $1 ORDER BY position",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows
        .into_iter()
        .map(|(course_id, position)| (format!("course_{course_id}"), position))
        .collect())
}

/// Canvas `Integer(val)` for the position values; strings and numbers are
/// accepted, anything else is the "Invalid position provided" 400.
fn parse_dashboard_position(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

// --------------------------------------------------------------------------
// Router
// --------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/self", get(get_self))
        .route(
            "/users/self/profile",
            get(get_self_profile).put(update_self_profile),
        )
        .route("/users/{id}", get(get_user))
        .route("/users/{id}", put(update_user))
        .route("/users/{id}", delete(delete_user))
        .route("/users/{id}/profile", get(get_user_profile))
        // Canvas users_controller#settings: `self` resolves to the caller.
        .route(
            "/users/{userId}/settings",
            get(get_user_settings).put(update_user_settings),
        )
        // Canvas users_controller#create_file (attachment preflight; metadata
        // only, no upload binary handling).
        .route("/users/{userId}/files", post(create_user_file))
        // Canvas accounts users#create.
        .route("/accounts/{accountId}/users", post(create_account_user))
        // Canvas users_controller#get_custom_colors / #get_custom_color /
        // #set_custom_color.
        .route("/users/{userId}/colors", get(get_custom_colors))
        .route(
            "/users/{userId}/colors/{context}",
            get(get_custom_color).put(set_custom_color),
        )
        // Canvas users_controller#get_dashboard_positions /
        // #set_dashboard_positions.
        .route(
            "/users/{userId}/dashboard_positions",
            get(get_dashboard_positions).put(set_dashboard_positions),
        )
        // Canvas users_controller#terminate_sessions.
        .route("/users/{userId}/sessions", delete(terminate_sessions))
}

// --------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------

/// GET /users — list all users (admin only), paginated.
async fn list_users(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<PaginatedUsers>, ApiError> {
    let role = get_user_role(&state.db, auth.id).await?;
    let role = role.unwrap_or_else(|| "student".to_string());
    if !is_admin(&role) {
        return Err(ApiError::Forbidden);
    }

    let (page, per_page, offset) = pagination_values(pag.page, pag.per_page);

    let (total,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?;

    let users: Vec<UserResponse> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, email, role, created_at
        FROM users
        ORDER BY created_at DESC
        LIMIT $1 OFFSET $2
        "#,
    )
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(PaginatedUsers {
        data: users,
        page,
        per_page,
        total,
    }))
}

/// GET /users/self — get the current authenticated user (alias for /auth/me behavior).
/// Queries the users table directly so sortable_name and the canonical
/// UserResponse shape (matching Canvas API v1) are returned.
async fn get_self(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<UserResponse>, ApiError> {
    let user: Option<UserResponse> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, email, role, created_at
        FROM users WHERE id = $1
        "#,
    )
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    user.ok_or(ApiError::NotFound).map(Json)
}

/// GET /users/self/profile — the caller's Canvas-shaped profile.
async fn get_self_profile(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<ProfileResponse>, ApiError> {
    load_profile(&state.db, auth.id).await.map(Json)
}

/// GET /users/{id}/profile — another user's profile (self or admin only).
async fn get_user_profile(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
) -> Result<Json<ProfileResponse>, ApiError> {
    let role = get_user_role(&state.db, auth.id).await?;
    let role = role.unwrap_or_else(|| "student".to_string());
    if !is_self(auth.id, user_id) && !is_admin(&role) {
        return Err(ApiError::Forbidden);
    }

    load_profile(&state.db, user_id).await.map(Json)
}

/// PUT /users/self/profile — create or partially update the caller's profile.
async fn update_self_profile(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<UpdateProfileRequest>,
) -> Result<Json<ProfileResponse>, ApiError> {
    validate_profile_update(&payload)?;
    upsert_profile(&state.db, auth.id, &payload).await.map(Json)
}

/// GET /users/{id} — get a single user (self or admin only).
async fn get_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
) -> Result<Json<UserResponse>, ApiError> {
    let role = get_user_role(&state.db, auth.id).await?;
    let role = role.unwrap_or_else(|| "student".to_string());
    if !is_self(auth.id, user_id) && !is_admin(&role) {
        return Err(ApiError::Forbidden);
    }

    let user: Option<UserResponse> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, email, role, created_at
        FROM users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    user.ok_or(ApiError::NotFound).map(Json)
}

/// PUT /users/{id} — update a user (self or admin).
/// Re-hashes password with argon2 if password field is present.
/// Checks email uniqueness before update.
async fn update_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
    Json(payload): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());
    if !is_self(auth.id, user_id) && !is_admin(&auth_role) {
        return Err(ApiError::Forbidden);
    }

    // Verify the user exists.
    let existing: Option<UserResponse> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, email, role, created_at
        FROM users WHERE id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    let existing = existing.ok_or(ApiError::NotFound)?;

    // If email is being changed, check uniqueness.
    let email = payload.email.clone().unwrap_or(existing.email.clone());
    if payload.email.is_some() && email != existing.email {
        let taken: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM users WHERE email = $1 AND id != $2")
                .bind(&email)
                .bind(user_id)
                .fetch_optional(&state.db)
                .await
                .map_err(ApiError::from)?;

        if taken.is_some() {
            return Err(ApiError::Conflict("email already in use".into()));
        }
    }

    let name = payload.name.clone().unwrap_or(existing.name);

    // Build the update query — handle password_hash separately.
    let updated: UserResponse = if let Some(ref pw) = payload.password {
        let hash = hash_password(pw).map_err(ApiError::BadRequest)?;
        sqlx::query_as(
            r#"
            UPDATE users
            SET name = $2, email = $3, password_hash = $4, updated_at = now()
            WHERE id = $1
            RETURNING id, name, sortable_name, email, role, created_at
            "#,
        )
        .bind(user_id)
        .bind(&name)
        .bind(&email)
        .bind(&hash)
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?
    } else {
        sqlx::query_as(
            r#"
            UPDATE users
            SET name = $2, email = $3, updated_at = now()
            WHERE id = $1
            RETURNING id, name, sortable_name, email, role, created_at
            "#,
        )
        .bind(user_id)
        .bind(&name)
        .bind(&email)
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?
    };

    Ok(Json(updated))
}

/// DELETE /users/{id} — delete a user (admin only).
async fn delete_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let role = get_user_role(&state.db, auth.id).await?;
    let role = role.unwrap_or_else(|| "student".to_string());
    if !is_admin(&role) {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(ApiError::from)?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// GET /users/{userId}/settings (and /users/self/settings) — the Canvas
/// boolean preference hash. Readable by the user themselves or an admin.
async fn get_user_settings(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
    Query(params): Query<SettingsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = authorize_settings(&state, &auth, &raw_user_id).await?;
    let settings = load_user_settings(&state.db, user_id).await?;

    let mut body = serde_json::to_value(&settings).map_err(|e| ApiError::Db(e.to_string()))?;
    if params.wants_mobile_settings() {
        // Canvas fills these from DynamicSettings (nil in a stock install)
        // and the account's usage-metrics setting.
        if let Some(object) = body.as_object_mut() {
            object.insert(
                "pendo_mobile_teacher_api_key".into(),
                serde_json::Value::Null,
            );
            object.insert(
                "pendo_mobile_parent_api_key".into(),
                serde_json::Value::Null,
            );
            object.insert(
                "pendo_mobile_student_api_key".into(),
                serde_json::Value::Null,
            );
            object.insert("usage_metrics".into(), serde_json::Value::Bool(false));
        }
    }

    Ok(Json(body))
}

/// PUT /users/{userId}/settings (and /users/self/settings) — partial update of
/// the Canvas boolean preferences. Writable by the user themselves or an
/// admin; the response is the full settings hash.
async fn update_user_settings(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
    Json(payload): Json<UpdateUserSettingsRequest>,
) -> Result<Json<UserSettings>, ApiError> {
    let user_id = authorize_settings(&state, &auth, &raw_user_id).await?;
    upsert_user_settings(&state.db, user_id, &payload)
        .await
        .map(Json)
}

/// GET /users/self/todo — Canvas todo list: assignments the caller can grade
/// (teacher enrollments with ungraded submissions) followed by assignments
/// they still need to submit (student enrollments, due between four weeks ago
/// and one week from now, with no submission from the caller).
#[allow(dead_code)]
async fn self_todo(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<TodoItem>>, ApiError> {
    let grading: Vec<TodoRow> = sqlx::query_as(
        r#"
        SELECT a.id,
               a.course_id,
               a.title,
               a.description,
               a.points_possible,
               a.due_at,
               a.created_at,
               a.updated_at,
               c.name AS course_name,
               c.course_code,
               COUNT(s.id) AS needs_grading_count
        FROM assignments a
        JOIN courses c ON c.id = a.course_id
        JOIN enrollments e
          ON e.course_id = a.course_id
         AND e.user_id = $1
         AND e.role = 'teacher'
        JOIN submissions s
          ON s.assignment_id = a.id
         AND s.score IS NULL
        GROUP BY a.id, c.name, c.course_code
        ORDER BY a.due_at ASC NULLS LAST, a.id
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    let submitting: Vec<TodoRow> = sqlx::query_as(
        r#"
        SELECT a.id,
               a.course_id,
               a.title,
               a.description,
               a.points_possible,
               a.due_at,
               a.created_at,
               a.updated_at,
               c.name AS course_name,
               c.course_code,
               NULL::BIGINT AS needs_grading_count
        FROM assignments a
        JOIN courses c ON c.id = a.course_id
        JOIN enrollments e
          ON e.course_id = a.course_id
         AND e.user_id = $1
         AND e.role = 'student'
        WHERE a.due_at IS NOT NULL
          AND a.due_at >= now() - interval '4 weeks'
          AND a.due_at <= now() + interval '1 week'
          AND NOT EXISTS (
              SELECT 1 FROM submissions s
              WHERE s.assignment_id = a.id AND s.user_id = $1
          )
        ORDER BY a.due_at ASC, a.id
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    let items = grading
        .into_iter()
        .map(|row| row.into_item("grading"))
        .chain(
            submitting
                .into_iter()
                .map(|row| row.into_item("submitting")),
        )
        .collect();

    Ok(Json(items))
}

/// POST /users/{userId}/files — record user-context file metadata (Canvas
/// attachment preflight, metadata only: no binary upload). The user themself
/// or an admin may add files; Canvas responds 200 with the attachment JSON.
async fn create_user_file(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
    Json(payload): Json<CreateUserFileRequest>,
) -> Result<Json<UserFileResponse>, ApiError> {
    let user_id = authorize_settings(&state, &auth, &raw_user_id).await?;

    // Canvas names the upload `name`; `filename` is the alias used by the rest
    // of this API.
    let filename = payload
        .filename
        .or(payload.name)
        .map(|name| name.trim().to_string())
        .unwrap_or_default();
    if filename.is_empty() {
        return Err(ApiError::Validation("filename is required".into()));
    }

    let display_name = payload
        .display_name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| filename.clone());
    let content_type = payload
        .content_type
        .unwrap_or_else(|| "application/octet-stream".into());
    let size = payload.size.unwrap_or(0).max(0);

    let (folder, folder_id) = if let Some(folder_id) = payload.folder_id {
        let found: Option<(String, String, i64)> = sqlx::query_as(
            "SELECT full_name, context_type, context_id FROM folders \
             WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(folder_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;
        let (full_name, context_type, context_id) = found.ok_or(ApiError::NotFound)?;
        if context_type != "User" || context_id != user_id {
            return Err(ApiError::Validation(
                "folder does not belong to this user".into(),
            ));
        }
        (full_name, Some(folder_id))
    } else {
        (payload.folder.unwrap_or_else(|| "unfiled".into()), None)
    };

    let file: UserFileResponse = sqlx::query_as(
        r#"
        INSERT INTO user_files
            (user_id, filename, display_name, content_type, size, folder, folder_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id,
                  user_id,
                  folder,
                  folder_id,
                  filename,
                  display_name,
                  content_type,
                  size,
                  created_at,
                  updated_at
        "#,
    )
    .bind(user_id)
    .bind(&filename)
    .bind(&display_name)
    .bind(&content_type)
    .bind(size)
    .bind(&folder)
    .bind(folder_id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(file))
}

/// POST /accounts/{accountId}/users — Canvas users#create. Account admins (or
/// site admins) create a user with a pseudonym (login email) and optional
/// password. Duplicate logins and missing required fields are 400s.
async fn create_account_user(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    Json(payload): Json<CreateAccountUserRequest>,
) -> Result<Json<UserResponse>, ApiError> {
    let account: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;
    account.ok_or(ApiError::NotFound)?;

    let role = get_user_role(&state.db, auth.id)
        .await?
        .unwrap_or_else(|| "student".to_string());
    if !is_admin(&role) {
        let account_admin: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM account_users \
             WHERE account_id = $1 AND user_id = $2 AND role = 'admin'",
        )
        .bind(account_id)
        .bind(auth.id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;
        if account_admin.is_none() {
            return Err(ApiError::Forbidden);
        }
    }

    let name = payload
        .user
        .name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ApiError::BadRequest("user[name] is required".into()))?;
    let email = payload
        .pseudonym
        .unique_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| ApiError::BadRequest("pseudonym[unique_id] is required".into()))?;

    let taken: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE email = $1")
        .bind(&email)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;
    if taken.is_some() {
        return Err(ApiError::BadRequest("email has already been taken".into()));
    }

    let sortable_name = payload
        .user
        .sortable_name
        .unwrap_or_else(|| match name.rsplit_once(' ') {
            Some((first, last)) => format!("{last}, {first}"),
            None => name.clone(),
        });
    let password = payload
        .password
        .unwrap_or_else(|| SaltString::generate(&mut OsRng).as_str().to_string());
    let hash = hash_password(&password).map_err(ApiError::BadRequest)?;

    let user: UserResponse = sqlx::query_as(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, $4, 'student')
        RETURNING id, name, sortable_name, email, role, created_at
        "#,
    )
    .bind(&name)
    .bind(&sortable_name)
    .bind(&email)
    .bind(&hash)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(user))
}

/// GET /users/{userId}/colors — Canvas `{custom_colors: {asset_string: hex}}`.
async fn get_custom_colors(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = authorize_settings(&state, &auth, &raw_user_id).await?;
    let colors = load_custom_colors(&state.db, user_id).await?;
    Ok(Json(serde_json::json!({ "custom_colors": colors })))
}

/// GET /users/{userId}/colors/{context} — Canvas `{hexcode}`; an unset asset
/// string is 404 ("Asset does not have an associated color.").
async fn get_custom_color(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((raw_user_id, context)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = authorize_settings(&state, &auth, &raw_user_id).await?;
    let color: Option<(String,)> =
        sqlx::query_as("SELECT color FROM user_colors WHERE user_id = $1 AND context = $2")
            .bind(user_id)
            .bind(&context)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::from)?;
    let (hexcode,) = color.ok_or(ApiError::NotFound)?;
    Ok(Json(serde_json::json!({ "hexcode": hexcode })))
}

/// PUT /users/{userId}/colors/{context} — validates the asset string (404 when
/// the context does not exist) and the hexcode (400), then upserts the color.
async fn set_custom_color(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((raw_user_id, context)): Path<(String, String)>,
    Json(payload): Json<SetCustomColorRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = authorize_settings(&state, &auth, &raw_user_id).await?;
    if !asset_exists(&state.db, &context).await? {
        return Err(ApiError::NotFound);
    }
    let hexcode = payload
        .hexcode
        .as_deref()
        .and_then(normalize_hexcode)
        .ok_or_else(|| ApiError::BadRequest("Invalid Hexcode Provided".into()))?;

    sqlx::query(
        r#"
        INSERT INTO user_colors (user_id, context, color)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, context) DO UPDATE
        SET color = EXCLUDED.color, updated_at = now()
        "#,
    )
    .bind(user_id)
    .bind(&context)
    .bind(&hexcode)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(serde_json::json!({ "hexcode": hexcode })))
}

/// GET /users/{userId}/dashboard_positions — Canvas
/// `{dashboard_positions: {asset_string: position}}`.
async fn get_dashboard_positions(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = authorize_settings(&state, &auth, &raw_user_id).await?;
    let positions = load_dashboard_positions(&state.db, user_id).await?;
    Ok(Json(
        serde_json::json!({ "dashboard_positions": positions }),
    ))
}

/// PUT /users/{userId}/dashboard_positions — merges the submitted positions.
/// Unknown assets are 404; non-numeric or abs(position) > 1000 values are 400.
async fn set_dashboard_positions(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
    Json(payload): Json<UpdateDashboardPositionsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = authorize_settings(&state, &auth, &raw_user_id).await?;

    for (asset_string, value) in &payload.dashboard_positions {
        if !asset_exists(&state.db, asset_string).await? {
            return Err(ApiError::NotFound);
        }
        let position = parse_dashboard_position(value)
            .ok_or_else(|| ApiError::BadRequest("Invalid position provided".into()))?;
        if position.abs() > 1_000 {
            return Err(ApiError::BadRequest(format!(
                "Position {position} is too high. Your dashboard cards can probably be sorted with numbers 1-5, you could even use a 0."
            )));
        }
        let course_id = asset_string
            .strip_prefix("course_")
            .and_then(|id| id.parse::<i64>().ok())
            .ok_or(ApiError::NotFound)?;
        sqlx::query(
            r#"
            INSERT INTO dashboard_positions (user_id, course_id, position)
            VALUES ($1, $2, $3)
            ON CONFLICT (user_id, course_id) DO UPDATE
            SET position = EXCLUDED.position, updated_at = now()
            "#,
        )
        .bind(user_id)
        .bind(course_id)
        .bind(position as i32)
        .execute(&state.db)
        .await
        .map_err(ApiError::from)?;
    }

    let positions = load_dashboard_positions(&state.db, user_id).await?;
    Ok(Json(
        serde_json::json!({ "dashboard_positions": positions }),
    ))
}

/// DELETE /users/{userId}/sessions — Canvas users#terminate_sessions: stamps
/// `last_logged_out` and expires every active access token for the user.
async fn terminate_sessions(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let user_id = authorize_settings(&state, &auth, &raw_user_id).await?;

    sqlx::query("UPDATE users SET last_logged_out = now(), updated_at = now() WHERE id = $1")
        .bind(user_id)
        .execute(&state.db)
        .await
        .map_err(ApiError::from)?;
    sqlx::query(
        "UPDATE access_tokens \
         SET updated_at = now(), permanent_expires_at = now() \
         WHERE user_id = $1 AND workflow_state = 'active'",
    )
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(
        serde_json::json!({ "message": "All sessions terminated" }),
    ))
}

#[cfg(test)]
mod tests {
    use super::normalize_hexcode;
    use super::pagination_values;

    #[test]
    fn normalize_hexcode_matches_canvas_regex() {
        assert_eq!(normalize_hexcode("fffeee"), Some("#fffeee".into()));
        assert_eq!(normalize_hexcode("#ABC"), Some("#ABC".into()));
        assert_eq!(normalize_hexcode("#abcd"), None);
        assert_eq!(normalize_hexcode("zzz"), None);
        assert_eq!(normalize_hexcode(""), None);
    }

    #[test]
    fn pagination_clamps_limits_and_handles_extreme_page_numbers() {
        assert_eq!(pagination_values(None, None), (1, 50, 0));
        assert_eq!(pagination_values(Some(0), Some(500)), (1, 100, 0));
        assert_eq!(
            pagination_values(Some(i64::MAX), Some(100)),
            (i64::MAX, 100, i64::MAX)
        );
    }
}
