//! Canvas user-scoped API routes: user logins (pseudonyms), course
//! nicknames and bookmarks.
//!
//! Port of three Ruby controllers of the Canvas API v1 (`config/routes.rb`,
//! `ApiRouteSet.draw`). All paths are relative to the `/api/v1` nest that
//! `star-http` applies:
//!
//! | Verb   | Path                                       | Canvas action                |
//! |--------|--------------------------------------------|------------------------------|
//! | GET    | `/accounts/{account_id}/logins`           | `pseudonyms#index`           |
//! | GET    | `/users/{user_id}/logins`                 | `pseudonyms#index`           |
//! | POST   | `/accounts/{account_id}/logins`           | `pseudonyms#create`          |
//! | PUT    | `/accounts/{account_id}/logins/{id}`      | `pseudonyms#update`          |
//! | DELETE | `/users/{user_id}/logins/{id}`             | `pseudonyms#destroy`         |
//! | POST   | `/users/reset_password`                   | `pseudonyms#forgot_password` |
//! | GET    | `/users/self/course_nicknames`             | `course_nicknames#index`     |
//! | GET    | `/users/self/course_nicknames/{course_id}` | `course_nicknames#show`      |
//! | PUT    | `/users/self/course_nicknames/{course_id}` | `course_nicknames#update`    |
//! | DELETE | `/users/self/course_nicknames/{course_id}` | `course_nicknames#delete`    |
//! | DELETE | `/users/self/course_nicknames`             | `course_nicknames#clear`     |
//! | GET    | `/users/self/bookmarks`                    | `bookmarks#index`            |
//! | POST   | `/users/self/bookmarks`                    | `bookmarks#create`           |
//! | GET    | `/users/self/bookmarks/{id}`               | `bookmarks#show`             |
//! | PUT    | `/users/self/bookmarks/{id}`               | `bookmarks#update`           |
//! | DELETE | `/users/self/bookmarks/{id}`               | `bookmarks#destroy`          |
//!
//! ## Ruby sources
//!
//! * `app/controllers/pseudonyms_controller.rb`
//!   (`lib/api/v1/pseudonym.rb` for the serializer)
//! * `app/controllers/course_nicknames_controller.rb`
//! * `app/controllers/bookmarks/bookmarks_controller.rb`
//!
//! ## Schema
//!
//! `pseudonyms`, `bookmarks_bookmarks`, `courses`, `accounts`,
//! `account_users`, `authentication_providers` and `communication_channels`
//! already exist in `star-db`. Canvas keeps the course nicknames in
//! `user_preference_values` rows with the key `course_nicknames` and the
//! course id as the sub key; star-lms has no preference-value table, so the
//! port needs one table (the coordinator turns this into a migration):
//!
//! ```sql
//! CREATE TABLE IF NOT EXISTS course_nicknames (
//!     id         BIGSERIAL PRIMARY KEY,
//!     user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
//!     course_id  BIGINT NOT NULL,
//!     nickname   TEXT NOT NULL,
//!     created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
//!     updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
//!     UNIQUE (user_id, course_id)
//! );
//! ```
//!
//! ## Fidelity notes
//!
//! * Canvas `Pseudonym#destroy` soft deletes the login
//!   (`workflow_state = 'deleted'`); the row stays and is reported as such.
//! * Canvas `bookmarks#destroy` answers `200` with the deleted bookmark, not
//!   `204` — the Ruby renders the bookmark it just removed.
//! * Canvas `bookmarks#find_bookmark` answers `head :not_found`, an empty
//!   404 body; the port keeps the empty body.
//! * Canvas `bookmarks#render_errors` wraps the errors hash in an `errors`
//!   key, the pseudonym actions render the errors hash itself.
//! * Canvas `course_nicknames#delete` has no `:read` check (only the course
//!   lookup, which is a 404).
//! * Canvas `check_limited_access_for_students` (the course nickname writes)
//!   needs accounts with limited access; star-lms has no such accounts, so
//!   the guard never fires.
//! * Canvas `forgot_password` mails a reset link; star-lms has no mailer, so
//!   the port only answers the documented JSON body.
//! * star-lms has no per-right account permission table. The stand-in for
//!   `manage_user_logins` / `manage_sis` / `read_sis` is the global admin
//!   role or an `account_users` admin row for the account, the same
//!   stand-in the rest of the workspace uses.
//! * A Canvas login is stored with an argon2 hash (star-lms has no scrypt);
//!   the stored `crypted_password` is therefore not comparable with a Canvas
//!   installation.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use axum::{
    body::Bytes,
    extract::{Path, Query, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::NaiveDateTime;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::{hash_password, verify_password, AuthUser};
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Canvas `Api::PER_PAGE`.
const PER_PAGE: i64 = 10;

/// Canvas `Api::MAX_PER_PAGE`.
const MAX_PER_PAGE: i64 = 100;

/// Canvas `Pseudonym::MAX_UNIQUE_ID_LENGTH`.
const MAX_UNIQUE_ID_LENGTH: usize = 100;

/// Canvas `Pseudonym` `declared_user_type` values.
const DECLARED_USER_TYPES: [&str; 6] = [
    "administrative",
    "observer",
    "staff",
    "student",
    "student_other",
    "teacher",
];

/// Canvas `CourseNicknamesController#update` — the nickname may not be
/// longer than this.
const MAX_NICKNAME_LENGTH: usize = 60;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// The error type of the handlers.
///
/// Canvas answers a few failures with a JSON body of its own (for example
/// `{"message": "Action must be called on a root account."}`) and one with
/// an empty body (`head :not_found`); everything else goes through the house
/// `ApiError`.
enum UsersApiError {
    Api(ApiError),
    /// A Canvas error body.
    Canvas(StatusCode, Value),
    /// A Canvas `head :status` — a status code without a body.
    Empty(StatusCode),
}

impl From<ApiError> for UsersApiError {
    fn from(error: ApiError) -> Self {
        UsersApiError::Api(error)
    }
}

impl From<sqlx::Error> for UsersApiError {
    fn from(error: sqlx::Error) -> Self {
        UsersApiError::Api(ApiError::Db(error.to_string()))
    }
}

impl IntoResponse for UsersApiError {
    fn into_response(self) -> Response {
        match self {
            UsersApiError::Api(error) => error.into_response(),
            UsersApiError::Canvas(status, body) => (status, Json(body)).into_response(),
            UsersApiError::Empty(status) => status.into_response(),
        }
    }
}

/// A Canvas error body of the shape `{"message": "..."}`.
fn canvas_message(status: StatusCode, message: &str) -> UsersApiError {
    UsersApiError::Canvas(status, json!({ "message": message }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The domain router for the Canvas logins, course nicknames and bookmarks
/// routes.
pub fn router() -> Router<AppState> {
    Router::new()
        // PseudonymsController
        .route("/accounts/{account_id}/logins", get(account_logins))
        .route("/users/{user_id}/logins", get(user_logins))
        .route("/accounts/{account_id}/logins", post(create_login))
        .route("/accounts/{account_id}/logins/{id}", put(update_login))
        .route("/users/{user_id}/logins/{id}", delete(destroy_login))
        .route("/users/reset_password", post(forgot_password))
        // CourseNicknamesController
        .route("/users/self/course_nicknames", get(course_nicknames))
        .route(
            "/users/self/course_nicknames",
            delete(clear_course_nicknames),
        )
        .route(
            "/users/self/course_nicknames/{course_id}",
            get(course_nickname),
        )
        .route(
            "/users/self/course_nicknames/{course_id}",
            put(set_course_nickname),
        )
        .route(
            "/users/self/course_nicknames/{course_id}",
            delete(remove_course_nickname),
        )
        // Bookmarks::BookmarksController
        .route("/users/self/bookmarks", get(list_bookmarks))
        .route("/users/self/bookmarks", post(create_bookmark))
        .route("/users/self/bookmarks/{id}", get(show_bookmark))
        .route("/users/self/bookmarks/{id}", put(update_bookmark))
        .route("/users/self/bookmarks/{id}", delete(destroy_bookmark))
}

// ---------------------------------------------------------------------------
// Request helpers
// ---------------------------------------------------------------------------

/// The pagination parameters of a Canvas collection.
#[derive(Debug, Default, Deserialize)]
struct PaginationQuery {
    page: Option<String>,
    per_page: Option<String>,
}

/// Canvas `Api.per_page_for` and `Api.paginate`.
///
/// `per_page` is `params[:per_page].to_i.clamp(1, Api::MAX_PER_PAGE)` and
/// the default is `Api::PER_PAGE`.
fn pagination_values(page: Option<&str>, per_page: Option<&str>) -> (i64, i64, i64) {
    let per_page = per_page
        .map(ruby_to_i)
        .unwrap_or(PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = page.map(ruby_to_i).unwrap_or(1).max(1);
    let offset = page.saturating_sub(1).saturating_mul(per_page);
    (page, per_page, offset)
}

/// Ruby's `String#to_i`: the leading integer of the string, 0 when there is
/// none.
fn ruby_to_i(value: &str) -> i64 {
    let trimmed = value.trim_start();
    let mut end = 0;
    for (index, ch) in trimmed.char_indices() {
        if ch.is_ascii_digit() || (index == 0 && (ch == '-' || ch == '+')) {
            end = index + ch.len_utf8();
        } else {
            break;
        }
    }
    trimmed[..end].parse::<i64>().unwrap_or(0)
}

/// The request body of a Canvas call.
///
/// Canvas accepts `application/json` and
/// `application/x-www-form-urlencoded` bodies. The form syntax
/// (`login[unique_id]=x`) is flattened into the nested JSON shape the
/// handlers work with.
fn parse_body(body: &Bytes) -> Result<Value, UsersApiError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let text = std::str::from_utf8(body)
        .map_err(|_| canvas_message(StatusCode::BAD_REQUEST, "Invalid request body"))?;
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return serde_json::from_str(trimmed)
            .map_err(|_| canvas_message(StatusCode::BAD_REQUEST, "Invalid request body"));
    }
    Ok(parse_form(trimmed))
}

/// The query string of a Canvas call, parsed like a form body.
fn parse_query(raw: Option<&str>) -> Value {
    match raw {
        Some(text) if !text.is_empty() => parse_form(text),
        _ => json!({}),
    }
}

/// Parse an `application/x-www-form-urlencoded` string into a JSON object.
///
/// `login[unique_id]=x` becomes `{"login": {"unique_id": "x"}}`.
fn parse_form(text: &str) -> Value {
    let mut root = Map::new();
    for pair in text.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (raw_key, raw_value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        let path = form_path(raw_key);
        if path.is_empty() {
            continue;
        }
        insert_param(&mut root, &path, Value::String(form_decode(raw_value)));
    }
    Value::Object(root)
}

/// The path of a form parameter: `a[b][c]` becomes `["a", "b", "c"]`.
fn form_path(key: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for ch in key.chars() {
        match ch {
            '[' | ']' => parts.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Insert a form parameter into the nested JSON object.
fn insert_param(map: &mut Map<String, Value>, path: &[String], value: Value) {
    let Some((head, tail)) = path.split_first() else {
        return;
    };
    if tail.is_empty() {
        map.insert(head.clone(), value);
        return;
    }
    let entry = map
        .entry(head.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    if let Some(child) = entry.as_object_mut() {
        insert_param(child, tail, value);
    }
}

/// Percent decode a form value (`+` is a space).
fn form_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        out.push((high * 16 + low) as u8);
                        index += 3;
                    }
                    _ => {
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

/// A parameter value as a string.
fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Canvas `params[:key]`.
fn param_str(params: &Value, key: &str) -> Option<String> {
    params.get(key).and_then(value_to_string)
}

/// Canvas `params[:key].present?`.
fn param_present(params: &Value, key: &str) -> bool {
    match params.get(key).and_then(value_to_string) {
        Some(text) => !text.trim().is_empty(),
        None => false,
    }
}

/// Canvas `params[:key].key?(:other)` — a `null` value still counts as
/// present.
fn param_key(params: &Value, key: &str) -> bool {
    params
        .as_object()
        .map(|object| object.contains_key(key))
        .unwrap_or(false)
}

/// Canvas `params[:user][:id]` — the user a login write is about.
fn user_id_param(query: &Value, body: &Value) -> Option<String> {
    for source in [query, body] {
        if let Some(Value::Object(user)) = source.get("user") {
            if let Some(id) = user.get("id").and_then(value_to_string) {
                return Some(id);
            }
        }
    }
    None
}

/// Canvas `value_to_boolean`.
fn value_to_boolean(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "t" | "yes" | "y" | "on" | "1"
    )
}

/// A random hex string (Canvas generates the temporary password and the
/// session tokens from the system entropy).
fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buffer);
    let mut out = String::with_capacity(bytes * 2);
    for byte in buffer {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('0'));
    }
    out
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// star-lms models a Canvas site admin as `users.role = 'admin'`.
fn is_site_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas `Account#grants_right?(user, :manage_user_logins)`.
///
/// star-lms has no per-right account permission table; the stand-in is the
/// global admin role or an `account_users` admin row for the account.
async fn grants_manage_user_logins(
    pool: &PgPool,
    auth: &AuthUser,
    account_id: i64,
) -> Result<bool, ApiError> {
    if is_site_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 AND role = 'admin'",
    )
    .bind(account_id)
    .bind(auth.id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `Account#grants_any_right?(user, :read_sis, :manage_sis)`.
///
/// The workspace has one account right (`admin`), so the SIS rights share the
/// stand-in of `manage_user_logins`.
async fn grants_sis_rights(
    pool: &PgPool,
    auth: &AuthUser,
    account_id: i64,
) -> Result<bool, ApiError> {
    grants_manage_user_logins(pool, auth, account_id).await
}

/// The accounts the given user belongs to (account membership and the
/// accounts of the courses the user is enrolled in).
async fn user_account_ids(pool: &PgPool, user_id: i64) -> Result<Vec<i64>, ApiError> {
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"
        WITH RECURSIVE member_accounts AS (
            SELECT au.account_id FROM account_users au WHERE au.user_id = $1
            UNION
            SELECT c.account_id
            FROM enrollments e
            JOIN courses c ON c.id = e.course_id
            WHERE e.user_id = $1
        )
        SELECT DISTINCT account_id FROM member_accounts WHERE account_id IS NOT NULL
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

/// Canvas `authorized_action(@user, @current_user, :read)` on a user.
///
/// Canvas grants `:read` on a user to the user themself and to everybody
/// with `manage_user_logins` on one of the user's accounts.
async fn can_read_user(
    pool: &PgPool,
    auth: &AuthUser,
    target_user_id: i64,
) -> Result<bool, ApiError> {
    if target_user_id == auth.id || is_site_admin(auth) {
        return Ok(true);
    }
    for account_id in user_account_ids(pool, target_user_id).await? {
        if grants_manage_user_logins(pool, auth, account_id).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Canvas `User#has_subset_of_account_permissions?(user, account)`.
///
/// The stand-in: the target user may not be an admin of the account unless
/// the caller is one as well.
async fn has_subset_of_account_permissions(
    pool: &PgPool,
    auth: &AuthUser,
    target_user_id: i64,
    account_id: i64,
) -> Result<bool, ApiError> {
    if is_site_admin(auth) {
        return Ok(true);
    }
    let target_admin: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 AND role = 'admin'",
    )
    .bind(account_id)
    .bind(target_user_id)
    .fetch_optional(pool)
    .await?;
    if target_admin.is_none() {
        return Ok(true);
    }
    grants_manage_user_logins(pool, auth, account_id).await
}

/// Canvas `authorized_action(@course, @current_user, :read)`.
///
/// The caller has to be enrolled in the course or administrate the account
/// the course belongs to.
async fn can_read_course(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    if is_site_admin(auth) {
        return Ok(true);
    }
    let enrolled: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2")
            .bind(course_id)
            .bind(auth.id)
            .fetch_optional(pool)
            .await?;
    if enrolled.is_some() {
        return Ok(true);
    }
    let account: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    match account {
        Some((Some(account_id),)) => grants_manage_user_logins(pool, auth, account_id).await,
        _ => Ok(false),
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// A `pseudonyms` row as the API needs it.
#[derive(Debug, sqlx::FromRow)]
struct PseudonymRow {
    id: i64,
    user_id: i64,
    account_id: i64,
    unique_id: String,
    sis_user_id: Option<String>,
    integration_id: Option<String>,
    authentication_provider_id: Option<i64>,
    created_at: NaiveDateTime,
    workflow_state: String,
    declared_user_type: Option<String>,
}

/// A pseudonym row the write handlers mutate.
struct PseudonymState {
    id: i64,
    user_id: i64,
    account_id: i64,
    unique_id: String,
    sis_user_id: Option<String>,
    integration_id: Option<String>,
    authentication_provider_id: Option<i64>,
    declared_user_type: Option<String>,
    workflow_state: String,
    created_at: NaiveDateTime,
    position: Option<i32>,
    /// The stored password hash (used to verify the old password).
    crypted_password: String,
    /// The fields a SIS import has locked.
    stuck_sis_fields: Option<String>,
    /// The new password the request wants to set.
    password: Option<String>,
    /// True when the pseudonym is being created.
    is_new: bool,
}

/// The Canvas rights of the caller on one pseudonym.
#[derive(Debug, Default, Clone, Copy)]
struct PseudonymRights {
    can_update: bool,
    can_change_password: bool,
    can_delete: bool,
}

/// The inputs of the Canvas pseudonym permission rules.
struct PseudonymCheck<'a> {
    account_id: i64,
    target_user_id: i64,
    passwordable: bool,
    is_new: bool,
    sis_user_id: Option<&'a str>,
}

/// Canvas `Pseudonym` permission rules.
///
/// * `:update` / `:create` — `manage_user_logins` on the account, the target
///   user has a subset of the caller's account permissions and the caller
///   may `:read` the target user.
/// * `:manage_sis` — `manage_sis` on the account and `:update`.
/// * `:change_password` — the user themself, or an admin of the account
///   (when the account allows admins to set passwords).
/// * `:delete` — `:update` for a login without a SIS id, `:manage_sis` for a
///   login with one.
async fn pseudonym_rights(
    pool: &PgPool,
    auth: &AuthUser,
    check: PseudonymCheck<'_>,
) -> Result<PseudonymRights, ApiError> {
    let manage_logins = grants_manage_user_logins(pool, auth, check.account_id).await?;
    let subset =
        has_subset_of_account_permissions(pool, auth, check.target_user_id, check.account_id)
            .await?;
    let can_read = can_read_user(pool, auth, check.target_user_id).await?;
    let can_update = manage_logins && subset && can_read;

    let manage_sis = grants_sis_rights(pool, auth, check.account_id).await? && can_update;

    let admins_change_passwords =
        account_flag(pool, check.account_id, "admins_can_change_passwords")
            .await?
            .unwrap_or_default();
    let own_password = check.target_user_id == auth.id;
    let can_change_password = check.passwordable
        && (own_password
            || (check.is_new && can_update)
            || (admins_change_passwords && can_update));

    let can_delete = if check.sis_user_id.is_some() {
        manage_sis
    } else {
        can_update
    };

    Ok(PseudonymRights {
        can_update,
        can_change_password,
        can_delete,
    })
}

/// Canvas `account.settings[:flag]`.
async fn account_flag(
    pool: &PgPool,
    account_id: i64,
    flag: &str,
) -> Result<Option<bool>, ApiError> {
    let row: Option<(Value,)> =
        sqlx::query_as("SELECT COALESCE(settings, '{}'::jsonb) FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_optional(pool)
            .await?;
    let Some((settings,)) = row else {
        return Ok(None);
    };
    Ok(settings.get(flag).and_then(|value| value.as_bool()))
}

// ---------------------------------------------------------------------------
// Pseudonym helpers
// ---------------------------------------------------------------------------

/// Canvas `api_find(Account.active, id)` — a missing or deleted account is a
/// 404.
async fn load_account(pool: &PgPool, account_id: i64) -> Result<Option<bool>, ApiError> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT parent_account_id FROM accounts \
         WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    // Canvas `context_is_root_account?`: the account has no parent.
    Ok(row.map(|(parent,)| parent.is_none()))
}

/// Canvas `api_find(User, id)` — `self` is the current user, an unknown id
/// is a 404.
async fn resolve_user(pool: &PgPool, raw: &str, auth: &AuthUser) -> Result<i64, ApiError> {
    let id = if raw == "self" {
        auth.id
    } else {
        raw.parse::<i64>().map_err(|_| ApiError::NotFound)?
    };
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|(id,)| id).ok_or(ApiError::NotFound)
}

/// Canvas `api_find(Course, id)`.
async fn resolve_course(pool: &PgPool, raw: &str) -> Result<(i64, String), ApiError> {
    let id = raw.parse::<i64>().map_err(|_| ApiError::NotFound)?;
    let row: Option<(i64, String)> = sqlx::query_as("SELECT id, name FROM courses WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.ok_or(ApiError::NotFound)
}

/// The `auth_type` of the authentication provider of a pseudonym.
async fn auth_provider_type(
    pool: &PgPool,
    provider_id: Option<i64>,
) -> Result<Option<String>, ApiError> {
    let Some(provider_id) = provider_id else {
        return Ok(None);
    };
    let row: Option<(String,)> =
        sqlx::query_as("SELECT auth_type FROM authentication_providers WHERE id = $1")
            .bind(provider_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(auth_type,)| auth_type))
}

/// Canvas `Account#canvas_authentication?` — the account has an active Canvas
/// authentication provider.
async fn account_has_canvas_auth(pool: &PgPool, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM authentication_providers \
         WHERE account_id = $1 AND auth_type = 'canvas' AND workflow_state = 'active'",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `Pseudonym#passwordable?`.
async fn pseudonym_passwordable(
    pool: &PgPool,
    account_id: i64,
    provider_id: Option<i64>,
) -> Result<bool, ApiError> {
    match provider_id {
        Some(provider_id) => {
            let auth_type = auth_provider_type(pool, Some(provider_id)).await?;
            Ok(auth_type.as_deref() == Some("canvas"))
        }
        None => account_has_canvas_auth(pool, account_id).await,
    }
}

/// Row shape of `load_pseudonym` (factored out for clippy::type_complexity;
/// distinct from `PseudonymRow`, which is the read-path projection).
type PseudonymStateRow = (
    i64,
    i64,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    String,
    NaiveDateTime,
    Option<i32>,
    String,
    Option<String>,
);

/// Canvas `Pseudonym.active` — every login that is not deleted.
async fn load_pseudonym(pool: &PgPool, raw_id: &str) -> Result<PseudonymState, ApiError> {
    let id = raw_id.parse::<i64>().map_err(|_| ApiError::NotFound)?;
    let row: Option<PseudonymStateRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, account_id, unique_id, sis_user_id, integration_id,
               authentication_provider_id, declared_user_type, workflow_state, created_at,
               position, crypted_password, stuck_sis_fields
        FROM pseudonyms
        WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((
        id,
        user_id,
        account_id,
        unique_id,
        sis_user_id,
        integration_id,
        authentication_provider_id,
        declared_user_type,
        workflow_state,
        created_at,
        position,
        crypted_password,
        stuck_sis_fields,
    )) = row
    else {
        return Err(ApiError::NotFound);
    };
    Ok(PseudonymState {
        id,
        user_id,
        account_id,
        unique_id,
        sis_user_id,
        integration_id,
        authentication_provider_id,
        declared_user_type,
        workflow_state,
        created_at,
        position,
        crypted_password,
        stuck_sis_fields,
        password: None,
        is_new: false,
    })
}

/// Canvas `pseudonym_json` — the `Api::V1::Pseudonym` serializer.
///
/// The SIS fields are only returned when the account grants the caller
/// `:read_sis` or `:manage_sis`.
fn pseudonym_json(row: &PseudonymRow, provider_type: Option<&str>, show_sis: bool) -> Value {
    let mut body = Map::new();
    body.insert("id".into(), json!(row.id));
    body.insert("user_id".into(), json!(row.user_id));
    body.insert("account_id".into(), json!(row.account_id));
    body.insert("unique_id".into(), json!(row.unique_id));
    if show_sis {
        body.insert("sis_user_id".into(), json!(row.sis_user_id));
        body.insert("integration_id".into(), json!(row.integration_id));
    }
    body.insert(
        "authentication_provider_id".into(),
        json!(row.authentication_provider_id),
    );
    body.insert("created_at".into(), json!(format_timestamp(row.created_at)));
    body.insert("workflow_state".into(), json!(row.workflow_state));
    body.insert("declared_user_type".into(), json!(row.declared_user_type));
    if let Some(provider_type) = provider_type {
        body.insert("authentication_provider_type".into(), json!(provider_type));
    }
    Value::Object(body)
}

/// Canvas renders a `timestamp` column as an ISO 8601 instant.
fn format_timestamp(value: NaiveDateTime) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// Canvas `account_scope` — the logins of one user in one account.
async fn account_pseudonyms(
    pool: &PgPool,
    account_id: i64,
    user_id: i64,
) -> Result<Vec<PseudonymRow>, ApiError> {
    let rows = sqlx::query_as::<_, PseudonymRow>(
        r#"
        SELECT id, user_id, account_id, unique_id, sis_user_id, integration_id,
               authentication_provider_id, created_at, workflow_state, declared_user_type
        FROM pseudonyms
        WHERE account_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'
        ORDER BY id
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Canvas `user_scope` — every login of the user, ordered by id.
async fn user_pseudonyms(pool: &PgPool, user_id: i64) -> Result<Vec<PseudonymRow>, ApiError> {
    let rows = sqlx::query_as::<_, PseudonymRow>(
        r#"
        SELECT id, user_id, account_id, unique_id, sis_user_id, integration_id,
               authentication_provider_id, created_at, workflow_state, declared_user_type
        FROM pseudonyms
        WHERE user_id = $1 AND workflow_state <> 'deleted'
        ORDER BY id
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Canvas `pseudonyms_json` — the serialized list of logins.
async fn pseudonyms_json(
    pool: &PgPool,
    auth: &AuthUser,
    rows: &[PseudonymRow],
) -> Result<Vec<Value>, ApiError> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let provider_type = auth_provider_type(pool, row.authentication_provider_id).await?;
        let show_sis = grants_sis_rights(pool, auth, row.account_id).await?;
        out.push(pseudonym_json(row, provider_type.as_deref(), show_sis));
    }
    Ok(out)
}

/// Canvas `find_authentication_provider`.
///
/// The provider is looked up in the domain root account, either by its
/// `auth_type` or by its id. An unknown provider is a 404.
async fn find_authentication_provider(
    pool: &PgPool,
    requested: &str,
) -> Result<Option<i64>, ApiError> {
    let root_account_id: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    let Some((root_account_id,)) = root_account_id else {
        return Ok(None);
    };
    let by_type: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM authentication_providers \
         WHERE account_id = $1 AND auth_type = $2 AND workflow_state = 'active' ORDER BY id LIMIT 1",
    )
    .bind(root_account_id)
    .bind(requested)
    .fetch_optional(pool)
    .await?;
    if let Some((id,)) = by_type {
        return Ok(Some(id));
    }
    let requested_id = requested.parse::<i64>().map_err(|_| ApiError::NotFound)?;
    let by_id: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM authentication_providers \
         WHERE account_id = $1 AND id = $2 AND workflow_state = 'active'",
    )
    .bind(root_account_id)
    .bind(requested_id)
    .fetch_optional(pool)
    .await?;
    by_id.map(|(id,)| Some(id)).ok_or(ApiError::NotFound)
}

/// Canvas `StickySisFields` — the fields a SIS import has locked.
fn stuck_sis_field(text: Option<&str>, field: &str) -> bool {
    let Some(text) = text else {
        return false;
    };
    text.split(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .any(|token| token == field)
}

/// Canvas `value_to_boolean` for `override_sis_stickiness`.
fn override_sis_stickiness(params: &Value) -> bool {
    match params
        .get("override_sis_stickiness")
        .and_then(value_to_string)
    {
        None => true,
        Some(value) => value_to_boolean(&value),
    }
}

/// The ActiveModel errors hash Canvas renders (`render json: model.errors`).
fn errors_body(errors: &[(&str, String)]) -> Value {
    let mut map = Map::new();
    for (attribute, message) in errors {
        let entry = map
            .entry((*attribute).to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(list) = entry.as_array_mut() {
            list.push(Value::String(message.clone()));
        }
    }
    Value::Object(map)
}

/// A Canvas error body with the ActiveModel errors hash.
fn errors_error(errors: &[(&str, String)]) -> UsersApiError {
    UsersApiError::Canvas(StatusCode::BAD_REQUEST, errors_body(errors))
}

/// Canvas `render_errors` — the errors hash wrapped in an `errors` key.
fn wrapped_errors_error(errors: &[(&str, String)]) -> UsersApiError {
    UsersApiError::Canvas(
        StatusCode::BAD_REQUEST,
        json!({ "errors": errors_body(errors) }),
    )
}

/// The values of a new login.
struct NewPseudonym {
    user_id: i64,
    account_id: i64,
    unique_id: String,
    sis_user_id: Option<String>,
    integration_id: Option<String>,
    authentication_provider_id: Option<i64>,
    declared_user_type: Option<String>,
    workflow_state: String,
    crypted_password: String,
    password_auto_generated: bool,
    position: i32,
}

/// Canvas `Pseudonym.normalize` — the port uses the trimmed lower cased
/// login.
fn normalize_unique_id(unique_id: &str) -> String {
    unique_id.trim().to_lowercase()
}

/// Insert a login (Canvas `Pseudonym#save`).
async fn insert_pseudonym(pool: &PgPool, new: &NewPseudonym) -> Result<i64, ApiError> {
    let id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO pseudonyms (
            user_id, account_id, workflow_state, unique_id, unique_id_normalized,
            crypted_password, password_salt, persistence_token, single_access_token,
            perishable_token, password_auto_generated, sis_user_id, integration_id,
            authentication_provider_id, declared_user_type, unique_ids, position,
            created_at, updated_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, '', $7, $8, $9, $10, $11, $12, $13, $14,
                '{}'::jsonb, $15, now(), now())
        RETURNING id
        "#,
    )
    .bind(new.user_id)
    .bind(new.account_id)
    .bind(&new.workflow_state)
    .bind(&new.unique_id)
    .bind(normalize_unique_id(&new.unique_id))
    .bind(&new.crypted_password)
    .bind(random_hex(16))
    .bind(random_hex(16))
    .bind(random_hex(16))
    .bind(new.password_auto_generated)
    .bind(&new.sis_user_id)
    .bind(&new.integration_id)
    .bind(new.authentication_provider_id)
    .bind(&new.declared_user_type)
    .bind(new.position)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Save the changed fields of a login (Canvas `Pseudonym#save`).
async fn save_pseudonym(pool: &PgPool, state: &PseudonymState) -> Result<(), ApiError> {
    let crypted_password = match state.password.as_deref() {
        Some(password) => Some(hash_password(password).map_err(ApiError::Db)?),
        None => None,
    };
    sqlx::query(
        r#"
        UPDATE pseudonyms
        SET unique_id = $2,
            unique_id_normalized = $3,
            sis_user_id = $4,
            integration_id = $5,
            authentication_provider_id = $6,
            declared_user_type = $7,
            workflow_state = $8,
            crypted_password = COALESCE($9, crypted_password),
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(state.id)
    .bind(&state.unique_id)
    .bind(normalize_unique_id(&state.unique_id))
    .bind(&state.sis_user_id)
    .bind(&state.integration_id)
    .bind(state.authentication_provider_id)
    .bind(&state.declared_user_type)
    .bind(&state.workflow_state)
    .bind(&crypted_password)
    .execute(pool)
    .await?;
    Ok(())
}

/// Canvas `Pseudonym` validations for the fields the API writes.
async fn validate_pseudonym(
    pool: &PgPool,
    state: &PseudonymState,
    original_unique_id: Option<&str>,
    original_sis_user_id: Option<&str>,
    original_integration_id: Option<&str>,
) -> Result<Vec<(&'static str, String)>, ApiError> {
    let mut errors: Vec<(&'static str, String)> = Vec::new();
    let own_id = if state.is_new { None } else { Some(state.id) };

    if state.unique_id.is_empty() {
        errors.push((
            "unique_id",
            "is too short (minimum is 1 character)".to_string(),
        ));
    } else {
        if state.unique_id.chars().count() > MAX_UNIQUE_ID_LENGTH {
            errors.push((
                "unique_id",
                format!("is too long (maximum is {MAX_UNIQUE_ID_LENGTH} characters)"),
            ));
        }
        if !state
            .unique_id
            .chars()
            .all(|ch| ch.is_ascii_graphic() || ch == ' ')
        {
            errors.push(("unique_id", "is invalid".to_string()));
        }
        if original_unique_id != Some(state.unique_id.as_str()) {
            let taken: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM pseudonyms \
                 WHERE account_id = $1 AND unique_id = $2 AND workflow_state <> 'deleted' \
                   AND ($3::bigint IS NULL OR id <> $3)",
            )
            .bind(state.account_id)
            .bind(&state.unique_id)
            .bind(own_id)
            .fetch_optional(pool)
            .await?;
            if taken.is_some() {
                errors.push((
                    "unique_id",
                    "ID already in use for this account and authentication provider".to_string(),
                ));
            }
        }
    }

    if let Some(declared_user_type) = state.declared_user_type.as_deref() {
        if !DECLARED_USER_TYPES.contains(&declared_user_type) {
            errors.push((
                "declared_user_type",
                "is not included in the list".to_string(),
            ));
        }
    }

    if let Some(sis_user_id) = state.sis_user_id.as_deref() {
        if original_sis_user_id != Some(sis_user_id) {
            let taken: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM pseudonyms \
                 WHERE account_id = $1 AND sis_user_id = $2 \
                   AND ($3::bigint IS NULL OR id <> $3)",
            )
            .bind(state.account_id)
            .bind(sis_user_id)
            .bind(own_id)
            .fetch_optional(pool)
            .await?;
            if taken.is_some() {
                errors.push((
                    "sis_user_id",
                    format!("SIS ID \"{sis_user_id}\" is already in use"),
                ));
            }
        }
    }

    if let Some(integration_id) = state.integration_id.as_deref() {
        if original_integration_id != Some(integration_id) {
            let taken: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM pseudonyms \
                 WHERE account_id = $1 AND integration_id = $2 \
                   AND ($3::bigint IS NULL OR id <> $3)",
            )
            .bind(state.account_id)
            .bind(integration_id)
            .bind(own_id)
            .fetch_optional(pool)
            .await?;
            if taken.is_some() {
                errors.push((
                    "integration_id",
                    format!("Integration ID \"{integration_id}\" is already in use"),
                ));
            }
        }
    }

    Ok(errors)
}

/// Canvas `update_pseudonym_from_params`.
///
/// The fields are applied in the order of the Ruby method; every field needs
/// its own right, a missing right is a 403.
async fn update_pseudonym_from_params(
    pool: &PgPool,
    state: &mut PseudonymState,
    params: &Value,
    rights: &PseudonymRights,
    manage_sis: bool,
    provider_id: Option<i64>,
) -> Result<(), UsersApiError> {
    let recognized = [
        "unique_id",
        "password",
        "sis_user_id",
        "authentication_provider_id",
        "integration_id",
        "workflow_state",
        "declared_user_type",
    ];
    if !recognized.iter().any(|key| param_key(params, key)) {
        return Err(canvas_message(
            StatusCode::BAD_REQUEST,
            "missing required parameter",
        ));
    }

    let stickiness = override_sis_stickiness(params);

    if param_key(params, "unique_id") {
        if !rights.can_update {
            return Err(ApiError::Forbidden.into());
        }
        if stickiness || !stuck_sis_field(state.stuck_sis_fields.as_deref(), "unique_id") {
            state.unique_id = param_str(params, "unique_id").unwrap_or_default();
        }
    }

    if param_key(params, "authentication_provider_id") {
        if !rights.can_update {
            return Err(ApiError::Forbidden.into());
        }
        if stickiness
            || !stuck_sis_field(state.stuck_sis_fields.as_deref(), "authentication_provider")
        {
            state.authentication_provider_id = provider_id;
        }
    }

    if param_key(params, "declared_user_type") {
        if !rights.can_update {
            return Err(ApiError::Forbidden.into());
        }
        if stickiness || !stuck_sis_field(state.stuck_sis_fields.as_deref(), "declared_user_type") {
            state.declared_user_type = param_str(params, "declared_user_type");
        }
    }

    if param_key(params, "sis_user_id") {
        if !manage_sis {
            return Err(ApiError::Forbidden.into());
        }
        // Canvas: `params[:pseudonym][:sis_user_id].presence` — "" is nil.
        state.sis_user_id = param_str(params, "sis_user_id").filter(|s| !s.trim().is_empty());
    }

    if param_key(params, "integration_id") {
        if !manage_sis {
            return Err(ApiError::Forbidden.into());
        }
        state.integration_id = param_str(params, "integration_id").filter(|s| !s.trim().is_empty());
    }

    // Canvas gives a 400 (not a 403) when the password cannot be set here.
    if param_key(params, "password") {
        let passwordable =
            pseudonym_passwordable(pool, state.account_id, state.authentication_provider_id)
                .await?;
        if !passwordable {
            return Err(errors_error(&[(
                "password",
                "password can only be set for Canvas authentication".to_string(),
            )]));
        }
        if !rights.can_change_password {
            return Err(ApiError::Forbidden.into());
        }
        if stickiness || !stuck_sis_field(state.stuck_sis_fields.as_deref(), "password") {
            state.password = param_str(params, "password");
        }
    }

    if param_key(params, "workflow_state") {
        let workflow_state = param_str(params, "workflow_state").unwrap_or_default();
        if workflow_state != "active" && workflow_state != "suspended" {
            return Err(errors_error(&[(
                "workflow_state",
                "invalid workflow_state".to_string(),
            )]));
        }
        // The delete right depends on the SIS id of the login, which the
        // request may have changed above.
        let delete_allowed = if state.sis_user_id.is_some() {
            manage_sis
        } else {
            rights.can_update
        };
        if !delete_allowed {
            return Err(ApiError::Forbidden.into());
        }
        if stickiness || !stuck_sis_field(state.stuck_sis_fields.as_deref(), "workflow_state") {
            state.workflow_state = workflow_state;
        }
    }

    if param_key(params, "authentication_provider_id") {
        if !rights.can_update {
            return Err(ApiError::Forbidden.into());
        }
        if state.authentication_provider_id.is_some()
            && !param_present(params, "authentication_provider_id")
        {
            state.authentication_provider_id = None;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// GET /accounts/{account_id}/logins
// ---------------------------------------------------------------------------

/// Canvas `PseudonymsController#index` for an account.
///
/// The order of the Ruby action is kept: the account (404), the user (404),
/// the `:read` right (403) and only then the root account check (400).
async fn account_logins(
    State(app): State<AppState>,
    Path(account_id): Path<i64>,
    RawQuery(raw_query): RawQuery,
    Query(pagination): Query<PaginationQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    // The account has to exist (404) and it has to be a root account (400).
    match load_account(&app.db, account_id).await? {
        None => return Err(ApiError::NotFound.into()),
        Some(false) => {
            return Err(canvas_message(
                StatusCode::BAD_REQUEST,
                "Action must be called on a root account.",
            ));
        }
        Some(true) => {}
    }

    let query = parse_query(raw_query.as_deref());
    let target_user_id = match user_id_param(&query, &json!({})) {
        Some(raw) => resolve_user(&app.db, &raw, &auth).await?,
        None => auth.id,
    };

    if !can_read_user(&app.db, &auth, target_user_id).await? {
        return Err(ApiError::Forbidden.into());
    }

    let rows = account_pseudonyms(&app.db, account_id, target_user_id).await?;
    let (_, per_page, offset) =
        pagination_values(pagination.page.as_deref(), pagination.per_page.as_deref());
    let page: Vec<PseudonymRow> = rows
        .into_iter()
        .skip(offset as usize)
        .take(per_page as usize)
        .collect();
    Ok(Json(Value::Array(
        pseudonyms_json(&app.db, &auth, &page).await?,
    )))
}

// ---------------------------------------------------------------------------
// GET /users/{user_id}/logins
// ---------------------------------------------------------------------------

/// Canvas `PseudonymsController#index` for a user.
async fn user_logins(
    State(app): State<AppState>,
    Path(user_id): Path<String>,
    Query(pagination): Query<PaginationQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    let target_user_id = resolve_user(&app.db, &user_id, &auth).await?;
    if !can_read_user(&app.db, &auth, target_user_id).await? {
        return Err(ApiError::Forbidden.into());
    }
    let rows = user_pseudonyms(&app.db, target_user_id).await?;
    let (_, per_page, offset) =
        pagination_values(pagination.page.as_deref(), pagination.per_page.as_deref());
    let page: Vec<PseudonymRow> = rows
        .into_iter()
        .skip(offset as usize)
        .take(per_page as usize)
        .collect();
    Ok(Json(Value::Array(
        pseudonyms_json(&app.db, &auth, &page).await?,
    )))
}

// ---------------------------------------------------------------------------
// POST /accounts/{account_id}/logins
// ---------------------------------------------------------------------------

/// Canvas `PseudonymsController#create`.
async fn create_login(
    State(app): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<Value>, UsersApiError> {
    // The account has to exist (404) and it has to be a root account (400).
    match load_account(&app.db, account_id).await? {
        None => return Err(ApiError::NotFound.into()),
        Some(false) => {
            return Err(canvas_message(
                StatusCode::BAD_REQUEST,
                "Action must be called on a root account.",
            ));
        }
        Some(true) => {}
    }
    let params = parse_body(&body)?;

    // Canvas `get_user`.
    let target_user_id = match user_id_param(&json!({}), &params) {
        Some(raw) => resolve_user(&app.db, &raw, &auth).await?,
        None => auth.id,
    };

    // Canvas: `params[:login] ||= {}; params[:pseudonym] = params[:login]`.
    let login = params.get("login").cloned().unwrap_or(json!({}));

    // Canvas `find_authentication_provider`.
    let provider_id = if param_present(&login, "authentication_provider_id") {
        let requested = param_str(&login, "authentication_provider_id").unwrap_or_default();
        find_authentication_provider(&app.db, &requested).await?
    } else {
        None
    };

    let passwordable = pseudonym_passwordable(&app.db, account_id, provider_id).await?;
    let rights = pseudonym_rights(
        &app.db,
        &auth,
        PseudonymCheck {
            account_id,
            target_user_id,
            passwordable,
            is_new: true,
            sis_user_id: None,
        },
    )
    .await?;
    let manage_sis = grants_sis_rights(&app.db, &auth, account_id).await? && rights.can_update;

    let mut pseudonym = PseudonymState {
        id: 0,
        user_id: target_user_id,
        account_id,
        unique_id: String::new(),
        sis_user_id: None,
        integration_id: None,
        authentication_provider_id: provider_id,
        declared_user_type: None,
        workflow_state: "active".to_string(),
        created_at: chrono::Utc::now().naive_utc(),
        position: None,
        crypted_password: String::new(),
        stuck_sis_fields: None,
        password: None,
        is_new: true,
    };

    update_pseudonym_from_params(
        &app.db,
        &mut pseudonym,
        &login,
        &rights,
        manage_sis,
        provider_id,
    )
    .await?;

    // Canvas `authorized_action(@pseudonym, @current_user, :create)`.
    if !rights.can_update {
        return Err(ApiError::Forbidden.into());
    }

    let errors = validate_pseudonym(&app.db, &pseudonym, None, None, None).await?;
    if !errors.is_empty() {
        return Err(errors_error(&errors));
    }

    // Canvas `@pseudonym.generate_temporary_password unless
    // params[:pseudonym][:password]`.
    let (crypted_password, password_auto_generated) = match pseudonym.password.clone() {
        Some(password) => (hash_password(&password).map_err(ApiError::Db)?, false),
        None => {
            let temporary = random_hex(8);
            (hash_password(&temporary).map_err(ApiError::Db)?, true)
        }
    };

    // Canvas `acts_as_list scope: :user` — the login goes to the bottom.
    let bottom: Option<(i32,)> = sqlx::query_as(
        "SELECT (COALESCE(MAX(position), 0) + 1)::int4 FROM pseudonyms WHERE user_id = $1",
    )
    .bind(target_user_id)
    .fetch_optional(&app.db)
    .await?;
    let position = bottom.map(|(position,)| position).unwrap_or(1);

    let new = NewPseudonym {
        user_id: target_user_id,
        account_id,
        unique_id: pseudonym.unique_id.clone(),
        sis_user_id: pseudonym.sis_user_id.clone(),
        integration_id: pseudonym.integration_id.clone(),
        authentication_provider_id: pseudonym.authentication_provider_id,
        declared_user_type: pseudonym.declared_user_type.clone(),
        workflow_state: pseudonym.workflow_state.clone(),
        crypted_password,
        password_auto_generated,
        position,
    };
    let id = insert_pseudonym(&app.db, &new).await?;
    let created = load_pseudonym(&app.db, &id.to_string()).await?;
    let row = pseudonym_row(&created);
    let provider_type = auth_provider_type(&app.db, row.authentication_provider_id).await?;
    let show_sis = grants_sis_rights(&app.db, &auth, row.account_id).await?;
    Ok(Json(pseudonym_json(
        &row,
        provider_type.as_deref(),
        show_sis,
    )))
}

/// Canvas `PseudonymRow` from a mutable state.
fn pseudonym_row(state: &PseudonymState) -> PseudonymRow {
    PseudonymRow {
        id: state.id,
        user_id: state.user_id,
        account_id: state.account_id,
        unique_id: state.unique_id.clone(),
        sis_user_id: state.sis_user_id.clone(),
        integration_id: state.integration_id.clone(),
        authentication_provider_id: state.authentication_provider_id,
        created_at: state.created_at,
        workflow_state: state.workflow_state.clone(),
        declared_user_type: state.declared_user_type.clone(),
    }
}

// ---------------------------------------------------------------------------
// PUT /accounts/{account_id}/logins/{id}
// ---------------------------------------------------------------------------

/// Canvas `PseudonymsController#update`.
///
/// The Ruby action ignores the account of the path (`get_context` only runs
/// for `index` and `create`); the port does the same.
async fn update_login(
    State(app): State<AppState>,
    Path((_account_id, raw_id)): Path<(i64, String)>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<Value>, UsersApiError> {
    let params = parse_body(&body)?;

    // Canvas `Pseudonym.active.find(params[:id])`.
    let mut pseudonym = load_pseudonym(&app.db, &raw_id).await?;

    // Canvas `@user = @pseudonym.user`.
    let user: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(pseudonym.user_id)
        .fetch_optional(&app.db)
        .await?;
    if user.is_none() {
        return Err(ApiError::NotFound.into());
    }

    let login = params.get("login").cloned().unwrap_or(json!({}));

    // Canvas: a user may only change their own password with the old one.
    if param_present(&login, "password") && pseudonym.user_id == auth.id {
        let errors = self_password_change_errors(&pseudonym, &login);
        if !errors.is_empty() {
            return Err(errors_error(&errors));
        }
    }

    // Canvas `find_authentication_provider`.
    let provider_id = if param_present(&login, "authentication_provider_id") {
        let requested = param_str(&login, "authentication_provider_id").unwrap_or_default();
        find_authentication_provider(&app.db, &requested).await?
    } else {
        pseudonym.authentication_provider_id
    };

    let original_unique_id = pseudonym.unique_id.clone();
    let original_sis_user_id = pseudonym.sis_user_id.clone();
    let original_integration_id = pseudonym.integration_id.clone();

    let passwordable = pseudonym_passwordable(&app.db, pseudonym.account_id, provider_id).await?;
    let rights = pseudonym_rights(
        &app.db,
        &auth,
        PseudonymCheck {
            account_id: pseudonym.account_id,
            target_user_id: pseudonym.user_id,
            passwordable,
            is_new: false,
            sis_user_id: pseudonym.sis_user_id.as_deref(),
        },
    )
    .await?;
    let manage_sis =
        grants_sis_rights(&app.db, &auth, pseudonym.account_id).await? && rights.can_update;

    update_pseudonym_from_params(
        &app.db,
        &mut pseudonym,
        &login,
        &rights,
        manage_sis,
        provider_id,
    )
    .await?;

    // Canvas `authorized_action(@pseudonym, @current_user, [:update,
    // :change_password])`.
    if !rights.can_update && !rights.can_change_password {
        return Err(ApiError::Forbidden.into());
    }

    let errors = validate_pseudonym(
        &app.db,
        &pseudonym,
        Some(original_unique_id.as_str()),
        original_sis_user_id.as_deref(),
        original_integration_id.as_deref(),
    )
    .await?;
    if !errors.is_empty() {
        return Err(errors_error(&errors));
    }

    save_pseudonym(&app.db, &pseudonym).await?;

    let row = pseudonym_row(&pseudonym);
    let provider_type = auth_provider_type(&app.db, row.authentication_provider_id).await?;
    let show_sis = grants_sis_rights(&app.db, &auth, row.account_id).await?;
    Ok(Json(pseudonym_json(
        &row,
        provider_type.as_deref(),
        show_sis,
    )))
}

/// Canvas `allow_self_password_change?`.
fn self_password_change_errors(
    state: &PseudonymState,
    login: &Value,
) -> Vec<(&'static str, String)> {
    let mut errors: Vec<(&'static str, String)> = Vec::new();
    let Some(old_password) = param_str(login, "old_password") else {
        errors.push((
            "old_password",
            "parameter is required to change your password".to_string(),
        ));
        return errors;
    };
    // Canvas `valid_arbitrary_credentials?`: the login has to be active and
    // the password has to match.
    let valid = state.workflow_state == "active"
        && !old_password.trim().is_empty()
        && verify_password(&old_password, &state.crypted_password);
    if !valid {
        errors.push(("old_password", "parameter is incorrect".to_string()));
    }
    errors
}

// ---------------------------------------------------------------------------
// DELETE /users/{user_id}/logins/{id}
// ---------------------------------------------------------------------------

/// Canvas `PseudonymsController#destroy`.
async fn destroy_login(
    State(app): State<AppState>,
    Path((user_id, raw_id)): Path<(String, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    // Canvas `get_user`.
    let target_user_id = resolve_user(&app.db, &user_id, &auth).await?;

    // Canvas `Pseudonym.active.find(params[:id])`.
    let pseudonym = load_pseudonym(&app.db, &raw_id).await?;

    // Canvas: the login has to belong to the user of the path.
    if pseudonym.user_id != target_user_id {
        return Err(ApiError::NotFound.into());
    }

    let rights = pseudonym_rights(
        &app.db,
        &auth,
        PseudonymCheck {
            account_id: pseudonym.account_id,
            target_user_id: pseudonym.user_id,
            passwordable: true,
            is_new: false,
            sis_user_id: pseudonym.sis_user_id.as_deref(),
        },
    )
    .await?;

    // Canvas `authorized_action(@pseudonym, @current_user, :delete)`.
    if !rights.can_delete {
        return Err(ApiError::Forbidden.into());
    }

    // Canvas: a user must keep at least one login.
    let active_logins: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM pseudonyms WHERE user_id = $1 AND workflow_state = 'active'",
    )
    .bind(pseudonym.user_id)
    .fetch_one(&app.db)
    .await?;
    if active_logins.0 < 2 {
        return Err(errors_error(&[(
            "base",
            "Users must have at least one login".to_string(),
        )]));
    }

    // Canvas `Pseudonym#destroy` — the login is soft deleted.
    sqlx::query(
        "UPDATE pseudonyms SET workflow_state = 'deleted', deleted_at = now(), updated_at = now() \
         WHERE id = $1",
    )
    .bind(pseudonym.id)
    .execute(&app.db)
    .await?;

    // Canvas `acts_as_list` — the remaining logins move up.
    if let Some(position) = pseudonym.position {
        sqlx::query(
            "UPDATE pseudonyms SET position = position - 1, updated_at = now() \
             WHERE user_id = $1 AND position > $2",
        )
        .bind(pseudonym.user_id)
        .bind(position)
        .execute(&app.db)
        .await?;
    }

    let mut row = pseudonym_row(&pseudonym);
    row.workflow_state = "deleted".to_string();
    let provider_type = auth_provider_type(&app.db, row.authentication_provider_id).await?;
    let show_sis = grants_sis_rights(&app.db, &auth, row.account_id).await?;
    Ok(Json(pseudonym_json(
        &row,
        provider_type.as_deref(),
        show_sis,
    )))
}

// ---------------------------------------------------------------------------
// POST /users/reset_password
// ---------------------------------------------------------------------------

/// Canvas `PseudonymsController#forgot_password`.
///
/// Canvas answers the API call with `{"requested": true}` when the address
/// belongs to a user of an account the caller administrates and with
/// `{"requested": false}` (404) when it does not. The mailer itself does not
/// exist in star-lms.
async fn forgot_password(
    State(app): State<AppState>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<Value>, UsersApiError> {
    let params = parse_body(&body)?;

    // Canvas `authorized_action(@current_user.pseudonym.account,
    // @current_user, [:manage_user_logins])`.
    let own_account: Option<(i64,)> = sqlx::query_as(
        "SELECT account_id FROM pseudonyms WHERE user_id = $1 AND workflow_state <> 'deleted' \
         ORDER BY position NULLS LAST, id LIMIT 1",
    )
    .bind(auth.id)
    .fetch_optional(&app.db)
    .await?;
    let Some((own_account_id,)) = own_account else {
        return Err(ApiError::Forbidden.into());
    };
    if !grants_manage_user_logins(&app.db, &auth, own_account_id).await? {
        return Err(ApiError::Forbidden.into());
    }

    // The domain root account — Canvas looks the address up there.
    let root_account_id: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id LIMIT 1",
    )
    .fetch_optional(&app.db)
    .await?;

    // Canvas collects the communication channels of the address and the
    // logins of the address in the domain root account.
    let mut candidates: Vec<i64> = Vec::new();
    if let Some(email) = param_str(&params, "email").filter(|value| !value.trim().is_empty()) {
        let channel_users: Vec<(i64,)> = sqlx::query_as(
            "SELECT DISTINCT user_id FROM communication_channels \
             WHERE path = $1 AND path_type = 'email' AND workflow_state = 'active'",
        )
        .bind(&email)
        .fetch_all(&app.db)
        .await?;
        for (user_id,) in channel_users {
            candidates.push(user_id);
        }
        if let Some((root_account_id,)) = root_account_id {
            let login_users: Vec<(i64,)> = sqlx::query_as(
                "SELECT DISTINCT user_id FROM pseudonyms \
                 WHERE account_id = $1 AND unique_id = $2 AND workflow_state = 'active'",
            )
            .bind(root_account_id)
            .bind(&email)
            .fetch_all(&app.db)
            .await?;
            for (user_id,) in login_users {
                candidates.push(user_id);
            }
        }
    }

    // Canvas keeps the channels whose user has a login in the domain root
    // account and reports the account of the user's first login.
    let mut seen_users: Vec<i64> = Vec::new();
    let mut matched: Vec<i64> = Vec::new();
    for user_id in candidates {
        if seen_users.contains(&user_id) {
            continue;
        }
        seen_users.push(user_id);
        let account: Option<(i64,)> = sqlx::query_as(
            "SELECT account_id FROM pseudonyms WHERE user_id = $1 AND workflow_state <> 'deleted' \
             ORDER BY position NULLS LAST, id LIMIT 1",
        )
        .bind(user_id)
        .fetch_optional(&app.db)
        .await?;
        let Some((account_id,)) = account else {
            continue;
        };
        if let Some((root_account_id,)) = root_account_id {
            let in_root: Option<(i32,)> = sqlx::query_as(
                "SELECT 1 FROM pseudonyms \
                 WHERE user_id = $1 AND account_id = $2 AND workflow_state <> 'deleted' LIMIT 1",
            )
            .bind(user_id)
            .bind(root_account_id)
            .fetch_optional(&app.db)
            .await?;
            if in_root.is_none() {
                continue;
            }
        }
        matched.push(account_id);
    }

    if matched.is_empty() {
        return Err(UsersApiError::Canvas(
            StatusCode::NOT_FOUND,
            json!({ "requested": false }),
        ));
    }

    // Canvas: every channel has to be in an account the caller administrates.
    for account_id in &matched {
        if !grants_manage_user_logins(&app.db, &auth, *account_id).await? {
            return Err(ApiError::Forbidden.into());
        }
    }

    Ok(Json(json!({ "requested": true })))
}

// ---------------------------------------------------------------------------
// Course nicknames
// ---------------------------------------------------------------------------

/// Canvas `CourseNicknamesController#course_nickname_json`.
fn course_nickname_json(course_id: i64, course_name: &str, nickname: Option<&str>) -> Value {
    json!({
        "course_id": course_id,
        "name": course_name,
        "nickname": nickname,
    })
}

/// Canvas `CourseNicknamesController#index`.
async fn course_nicknames(
    State(app): State<AppState>,
    Query(pagination): Query<PaginationQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT course_id, nickname FROM course_nicknames WHERE user_id = $1 ORDER BY id",
    )
    .bind(auth.id)
    .fetch_all(&app.db)
    .await?;

    let (_, per_page, offset) =
        pagination_values(pagination.page.as_deref(), pagination.per_page.as_deref());

    let mut out = Vec::new();
    for (course_id, nickname) in rows
        .into_iter()
        .skip(offset as usize)
        .take(per_page as usize)
    {
        // Canvas: a nickname without a course is not returned.
        let course: Option<(i64, String)> =
            sqlx::query_as("SELECT id, name FROM courses WHERE id = $1")
                .bind(course_id)
                .fetch_optional(&app.db)
                .await?;
        let Some((course_id, course_name)) = course else {
            continue;
        };
        out.push(course_nickname_json(
            course_id,
            &course_name,
            Some(nickname.as_str()),
        ));
    }
    Ok(Json(Value::Array(out)))
}

/// Canvas `CourseNicknamesController#show`.
async fn course_nickname(
    State(app): State<AppState>,
    Path(course_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    let (course_id, course_name) = resolve_course(&app.db, &course_id).await?;
    if !can_read_course(&app.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden.into());
    }
    let nickname: Option<(String,)> = sqlx::query_as(
        "SELECT nickname FROM course_nicknames WHERE user_id = $1 AND course_id = $2",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(&app.db)
    .await?;
    let nickname = nickname.map(|(nickname,)| nickname);
    Ok(Json(course_nickname_json(
        course_id,
        &course_name,
        nickname.as_deref(),
    )))
}

/// Canvas `CourseNicknamesController#update`.
async fn set_course_nickname(
    State(app): State<AppState>,
    Path(course_id): Path<String>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<Value>, UsersApiError> {
    let (course_id, course_name) = resolve_course(&app.db, &course_id).await?;
    if !can_read_course(&app.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden.into());
    }
    let params = parse_body(&body)?;

    if !param_present(&params, "nickname") {
        return Err(canvas_message(StatusCode::BAD_REQUEST, "missing nickname"));
    }
    let nickname = param_str(&params, "nickname").unwrap_or_default();
    if nickname.chars().count() >= MAX_NICKNAME_LENGTH {
        return Err(canvas_message(StatusCode::BAD_REQUEST, "nickname too long"));
    }

    sqlx::query(
        "INSERT INTO course_nicknames (user_id, course_id, nickname) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, course_id) DO UPDATE SET nickname = EXCLUDED.nickname, \
         updated_at = now()",
    )
    .bind(auth.id)
    .bind(course_id)
    .bind(&nickname)
    .execute(&app.db)
    .await?;

    Ok(Json(course_nickname_json(
        course_id,
        &course_name,
        Some(nickname.as_str()),
    )))
}

/// Canvas `CourseNicknamesController#delete`.
async fn remove_course_nickname(
    State(app): State<AppState>,
    Path(course_id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    let (course_id, course_name) = resolve_course(&app.db, &course_id).await?;

    // Canvas has no `:read` check in this action.
    let nickname: Option<(String,)> = sqlx::query_as(
        "SELECT nickname FROM course_nicknames WHERE user_id = $1 AND course_id = $2",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(&app.db)
    .await?;

    if nickname.is_none() {
        return Err(canvas_message(
            StatusCode::NOT_FOUND,
            "no nickname exists for course",
        ));
    }

    sqlx::query("DELETE FROM course_nicknames WHERE user_id = $1 AND course_id = $2")
        .bind(auth.id)
        .bind(course_id)
        .execute(&app.db)
        .await?;

    Ok(Json(course_nickname_json(course_id, &course_name, None)))
}

/// Canvas `CourseNicknamesController#clear`.
async fn clear_course_nicknames(
    State(app): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    sqlx::query("DELETE FROM course_nicknames WHERE user_id = $1")
        .bind(auth.id)
        .execute(&app.db)
        .await?;
    Ok(Json(json!({ "message": "OK" })))
}

// ---------------------------------------------------------------------------
// Bookmarks
// ---------------------------------------------------------------------------

/// A `bookmarks_bookmarks` row.
#[derive(Debug, sqlx::FromRow)]
struct BookmarkRow {
    id: i64,
    name: String,
    url: String,
    position: Option<i32>,
    json: Option<String>,
}

/// Canvas `Bookmarks::Bookmark#as_json`.
///
/// The model drops the `json` and `user_id` columns and adds the parsed
/// `data` hash.
fn bookmark_json(row: &BookmarkRow) -> Value {
    let data = row
        .json
        .as_deref()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .unwrap_or(Value::Null);
    json!({
        "id": row.id,
        "name": row.name,
        "url": row.url,
        "position": row.position,
        "data": data,
    })
}

/// Canvas `Bookmarks::Bookmark.find_by(id:, user_id:)` — a bookmark of
/// another user is not found.
async fn find_bookmark(
    pool: &PgPool,
    raw_id: &str,
    user_id: i64,
) -> Result<Option<BookmarkRow>, ApiError> {
    let Ok(id) = raw_id.parse::<i64>() else {
        return Ok(None);
    };
    let row: Option<BookmarkRow> = sqlx::query_as(
        "SELECT id, name, url, position, json FROM bookmarks_bookmarks \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Canvas `Bookmarks::Bookmark.where(user_id:).ordered`.
async fn list_bookmark_rows(pool: &PgPool, user_id: i64) -> Result<Vec<BookmarkRow>, ApiError> {
    let rows = sqlx::query_as::<_, BookmarkRow>(
        "SELECT id, name, url, position, json FROM bookmarks_bookmarks \
         WHERE user_id = $1 ORDER BY position NULLS LAST, id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Canvas `Bookmarks::Bookmark#index`.
async fn list_bookmarks(
    State(app): State<AppState>,
    Query(pagination): Query<PaginationQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    let rows = list_bookmark_rows(&app.db, auth.id).await?;
    let (_, per_page, offset) =
        pagination_values(pagination.page.as_deref(), pagination.per_page.as_deref());
    let page: Vec<Value> = rows
        .into_iter()
        .skip(offset as usize)
        .take(per_page as usize)
        .map(|row| bookmark_json(&row))
        .collect();
    Ok(Json(Value::Array(page)))
}

/// Canvas `Bookmarks::Bookmark#show`.
async fn show_bookmark(
    State(app): State<AppState>,
    Path(id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    let Some(row) = find_bookmark(&app.db, &id, auth.id).await? else {
        // Canvas `head :not_found`.
        return Err(UsersApiError::Empty(StatusCode::NOT_FOUND));
    };
    Ok(Json(bookmark_json(&row)))
}

/// The body of a bookmark write.
struct BookmarkParams {
    name: Option<String>,
    url: Option<String>,
    data: Option<Value>,
    position: Option<i64>,
}

/// Canvas `valid_params`.
fn bookmark_params(params: &Value) -> BookmarkParams {
    let data = match params.get("data") {
        Some(Value::Object(_)) | Some(Value::Array(_)) => params.get("data").cloned(),
        _ => None,
    };
    BookmarkParams {
        name: param_str(params, "name"),
        url: param_str(params, "url"),
        data,
        position: params
            .get("position")
            .and_then(value_to_string)
            .map(|value| ruby_to_i(&value)),
    }
}

/// Canvas `Bookmarks::Bookmark#create`.
async fn create_bookmark(
    State(app): State<AppState>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<Value>, UsersApiError> {
    let params = parse_body(&body)?;
    let input = bookmark_params(&params);

    // The Canvas columns are `null: false`, so a missing name or url is a
    // validation error.
    let Some(name) = input.name else {
        return Err(wrapped_errors_error(&[(
            "name",
            "can't be blank".to_string(),
        )]));
    };
    let Some(url) = input.url else {
        return Err(wrapped_errors_error(&[(
            "url",
            "can't be blank".to_string(),
        )]));
    };
    let json = input.data.as_ref().map(|data| data.to_string());

    // Canvas `acts_as_list`: without a position the bookmark goes to the
    // bottom.
    let position = match input.position {
        Some(position) => position.clamp(1, i32::MAX as i64) as i32,
        None => {
            let bottom: Option<(i32,)> = sqlx::query_as(
                "SELECT (COALESCE(MAX(position), 0) + 1)::int4 FROM bookmarks_bookmarks \
                 WHERE user_id = $1",
            )
            .bind(auth.id)
            .fetch_optional(&app.db)
            .await?;
            bottom.map(|(position,)| position).unwrap_or(1)
        }
    };

    // Canvas `insert_at` moves the bookmarks behind it one place down.
    sqlx::query(
        "UPDATE bookmarks_bookmarks SET position = position + 1 \
         WHERE user_id = $1 AND position >= $2",
    )
    .bind(auth.id)
    .bind(position)
    .execute(&app.db)
    .await?;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO bookmarks_bookmarks (user_id, name, url, position, json) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(auth.id)
    .bind(&name)
    .bind(&url)
    .bind(position)
    .bind(&json)
    .fetch_one(&app.db)
    .await?;

    let row = BookmarkRow {
        id,
        name,
        url,
        position: Some(position),
        json,
    };
    Ok(Json(bookmark_json(&row)))
}

/// Canvas `Bookmarks::Bookmark#update`.
async fn update_bookmark(
    State(app): State<AppState>,
    Path(id): Path<String>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<Value>, UsersApiError> {
    let Some(existing) = find_bookmark(&app.db, &id, auth.id).await? else {
        return Err(UsersApiError::Empty(StatusCode::NOT_FOUND));
    };

    let params = parse_body(&body)?;
    let input = bookmark_params(&params);

    let name = input.name.unwrap_or_else(|| existing.name.clone());
    let url = input.url.unwrap_or_else(|| existing.url.clone());
    let json = match input.data {
        Some(data) => Some(data.to_string()),
        None => existing.json.clone(),
    };

    let position = match input.position {
        Some(position) => {
            let position = position.clamp(1, i32::MAX as i64) as i32;
            sqlx::query(
                "UPDATE bookmarks_bookmarks SET position = position + 1 \
                 WHERE user_id = $1 AND position >= $2 AND id <> $3",
            )
            .bind(auth.id)
            .bind(position)
            .bind(existing.id)
            .execute(&app.db)
            .await?;
            position
        }
        None => existing.position.unwrap_or(1),
    };

    sqlx::query(
        "UPDATE bookmarks_bookmarks SET name = $2, url = $3, position = $4, json = $5 WHERE id = $1",
    )
    .bind(existing.id)
    .bind(&name)
    .bind(&url)
    .bind(position)
    .bind(&json)
    .execute(&app.db)
    .await?;

    let row = BookmarkRow {
        id: existing.id,
        name,
        url,
        position: Some(position),
        json,
    };
    Ok(Json(bookmark_json(&row)))
}

/// Canvas `Bookmarks::Bookmark#destroy`.
async fn destroy_bookmark(
    State(app): State<AppState>,
    Path(id): Path<String>,
    auth: AuthUser,
) -> Result<Json<Value>, UsersApiError> {
    let Some(existing) = find_bookmark(&app.db, &id, auth.id).await? else {
        return Err(UsersApiError::Empty(StatusCode::NOT_FOUND));
    };

    sqlx::query("DELETE FROM bookmarks_bookmarks WHERE id = $1 AND user_id = $2")
        .bind(existing.id)
        .bind(auth.id)
        .execute(&app.db)
        .await?;

    // Canvas `acts_as_list` — the bookmarks behind it move up.
    if let Some(position) = existing.position {
        sqlx::query(
            "UPDATE bookmarks_bookmarks SET position = position - 1 \
             WHERE user_id = $1 AND position > $2",
        )
        .bind(auth.id)
        .bind(position)
        .execute(&app.db)
        .await?;
    }

    // Canvas renders the bookmark it just removed.
    Ok(Json(bookmark_json(&existing)))
}
