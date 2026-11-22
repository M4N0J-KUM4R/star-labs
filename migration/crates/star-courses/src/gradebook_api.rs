//! gradebook_api — Canvas `CustomGradebookColumnsApiController` and
//! `GradebookFiltersApiController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /courses/{course_id}/custom_gradebook_columns`
//! - `POST   /courses/{course_id}/custom_gradebook_columns`
//! - `POST   /courses/{course_id}/custom_gradebook_columns/reorder`
//! - `PUT    /courses/{course_id}/custom_gradebook_columns/{id}`
//! - `DELETE /courses/{course_id}/custom_gradebook_columns/{id}`
//! - `GET    /courses/{course_id}/gradebook_filters`
//! - `POST   /courses/{course_id}/gradebook_filters`
//! - `GET    /courses/{course_id}/gradebook_filters/{id}`
//! - `PUT    /courses/{course_id}/gradebook_filters/{id}`
//! - `DELETE /courses/{course_id}/gradebook_filters/{id}`
//!
//! Canvas behavior mirrored here
//! (`app/controllers/custom_gradebook_columns_api_controller.rb`,
//! `app/controllers/gradebook_filters_api_controller.rb`,
//! `app/models/custom_gradebook_column.rb`, `app/models/gradebook_filter.rb`,
//! `lib/api/v1/custom_gradebook_column.rb`):
//!
//! - both controllers run `require_context` first: an unknown course is the
//!   Canvas `404 {"errors":[{"message":"The specified resource does not
//!   exist."}]}`.
//! - the custom gradebook column policy is `course.grants_any_right?(user,
//!   :view_all_grades, :manage_grades)` for both `:read` and `:manage`, so
//!   every column route needs a course teacher (or an admin); a missing right
//!   is the Canvas `403 {"status":"unauthorized","errors":[{"message":"user
//!   not authorized to perform that action"}]}`.
//! - a missing or deleted column is a `404` and that check runs BEFORE the
//!   permission check (`not_deleted.find(params[:id])`).
//! - the column JSON is `api_json(..., only: %w[id title position
//!   teacher_notes read_only])` plus `hidden` — the `hidden` flag is the
//!   `hidden` workflow state, and the `hidden` parameter is cast with
//!   `Canvas::Plugin.value_to_boolean` ("no" is false), not with the Rails
//!   boolean cast.
//! - a validation failure is the Canvas `400` with the Rails error hash
//!   (`{"title":["can't be blank"]}`).
//! - the delete is a soft delete — the row keeps existing with the `deleted`
//!   workflow state, and the deleted column is rendered.
//! - `reorder` has no permission check in Ruby and answers `200 {}`; it
//!   reorders with the `acts_as_list` `update_order` algorithm of the
//!   `gems/acts_as_list` gem Canvas vendors.
//! - the gradebook filter JSON is wrapped in the `gradebook_filter` root —
//!   Canvas makes `ActiveRecord::Base#as_json` include the root by default
//!   (`config/initializers/active_record.rb`), and the controller renders the
//!   record with `render json:`, not with `api_json`.
//! - a gradebook filter is always scoped to the caller: `find_by!(id:,
//!   user: @current_user)` — a filter of another user is a `404`, not a
//!   `403`.
//! - `show` of a gradebook filter answers `201 Created` — that is the Ruby
//!   `render json: @gradebook_filter.as_json, status: :created`.
//!
//! The Canvas request bodies wrap their fields (`column[title]`,
//! `gradebook_filter[name]`). Like the other Canvas wrappers of this crate
//! this port accepts the flat body as well.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::NaiveDateTime;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// `GradebookImporter::GRADEBOOK_IMPORTER_RESERVED_NAMES` — the titles the
/// gradebook importer owns; a custom column may not use one of them.
const GRADEBOOK_IMPORTER_RESERVED_NAMES: &[&str] = &[
    "Student",
    "LastName",
    "FirstName",
    "ID",
    "SIS User ID",
    "SIS Login ID",
    "Section",
    "Integration ID",
    "Root Account",
];

/// `ActiveRecord::Base.maximum_string_length` — the Canvas maximum length of
/// the `title` and `name` columns.
const MAXIMUM_STRING_LENGTH: usize = 255;

/// `Api::PER_PAGE` — the default page size of a Canvas API collection.
const DEFAULT_PER_PAGE: i64 = 10;

/// `Api::MAX_PER_PAGE` — the maximum page size of a Canvas API collection.
const MAX_PER_PAGE: i64 = 100;

/// The columns of the `custom_gradebook_columns` row, in the order the port
/// reads them.
const COLUMN_COLUMNS: &str = "id, title, position, workflow_state, course_id, \
     teacher_notes, read_only, root_account_id";

/// The columns of the `gradebook_filters` row, in the order the port reads
/// them.
const FILTER_COLUMNS: &str = "id, course_id, user_id, name, payload, created_at, updated_at";

/// Guard against a circular account hierarchy while the root account of a
/// course is resolved.
const MAX_ACCOUNT_DEPTH: usize = 10;

// ---------------------------------------------------------------------------
// Canvas-shaped error bodies
// ---------------------------------------------------------------------------

/// Canvas `rescue_action_in_api` for `ActiveRecord::RecordNotFound` — 404.
fn resource_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "errors": [{ "message": "The specified resource does not exist." }]
        })),
    )
        .into_response()
}

/// Canvas `render_json_unauthorized` for a logged in user — 403.
fn unauthorized() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({
            "status": "unauthorized",
            "errors": [{ "message": "user not authorized to perform that action" }],
        })),
    )
        .into_response()
}

/// Canvas `render json: record.errors` — 400 with the Rails error hash.
fn validation_errors(errors: Map<String, Value>) -> Response {
    (StatusCode::BAD_REQUEST, Json(Value::Object(errors))).into_response()
}

/// The Rails `500` of an unhandled Ruby exception — the `order` parameter of
/// the reorder route is required, and the Ruby raises a `NoMethodError`
/// without it.
fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "status": 500, "error": "Internal Server Error" })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Domain rows
// ---------------------------------------------------------------------------

/// A row of the `custom_gradebook_columns` table.
#[derive(Debug, Clone, sqlx::FromRow)]
#[allow(dead_code)]
struct ColumnRow {
    id: i64,
    title: String,
    position: i32,
    workflow_state: String,
    course_id: i64,
    teacher_notes: bool,
    read_only: bool,
    root_account_id: Option<i64>,
}

/// A row of the `gradebook_filters` table.
#[derive(Debug, Clone, sqlx::FromRow)]
struct FilterRow {
    id: i64,
    course_id: i64,
    user_id: i64,
    name: String,
    payload: Value,
    created_at: NaiveDateTime,
    updated_at: NaiveDateTime,
}

/// The path parameters of the custom gradebook column routes.
#[derive(Debug, Deserialize)]
struct ColumnPath {
    course_id: i64,
    id: i64,
}

/// The path parameters of the gradebook filter routes.
#[derive(Debug, Deserialize)]
struct FilterPath {
    course_id: i64,
    id: i64,
}

/// The query parameters of the custom gradebook column index.
#[derive(Debug, Default, Deserialize)]
struct ColumnListQuery {
    include_hidden: Option<String>,
    page: Option<String>,
    per_page: Option<String>,
}

// ---------------------------------------------------------------------------
// Permissions and course scope
// ---------------------------------------------------------------------------

/// Canvas `require_context` — the course of the route has to exist.
async fn course_exists(pool: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Canvas `course.grants_any_right?(user, :view_all_grades, :manage_grades)`.
///
/// Canvas grants `manage_grades` to `TeacherEnrollment` and `view_all_grades`
/// to teachers and TAs, so a course teacher, TA or designer passes; a
/// global admin passes as well.
async fn can_manage_grades(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    if crate::is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 \
           AND role IN ('teacher', 'ta', 'designer') \
           AND workflow_state <> 'deleted' LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `CustomGradebookColumn#set_root_account_id` — the root account of
/// the owning course.
async fn course_root_account_id(pool: &PgPool, course_id: i64) -> Result<Option<i64>, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    let Some((Some(account_id),)) = row else {
        return Ok(None);
    };

    let mut current = account_id;
    let mut root = account_id;
    for _ in 0..MAX_ACCOUNT_DEPTH {
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(current)
                .fetch_optional(pool)
                .await?;
        match parent {
            Some((Some(parent_id),)) => {
                root = parent_id;
                current = parent_id;
            }
            _ => break,
        }
    }
    Ok(Some(root))
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// Canvas `custom_gradebook_column_json` — the `api_json` field set plus the
/// `hidden` flag of the workflow state.
fn column_json(row: &ColumnRow) -> Value {
    json!({
        "id": row.id,
        "title": row.title,
        "position": row.position,
        "teacher_notes": row.teacher_notes,
        "read_only": row.read_only,
        "hidden": row.workflow_state == "hidden",
    })
}

/// Canvas renders a `timestamp` column as an ISO 8601 instant.
fn format_timestamp(value: NaiveDateTime) -> String {
    value.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// The `gradebook_filter` root — Canvas includes the root of an
/// `ActiveRecord::Base` in every `as_json` unless `include_root: false` is
/// asked for, and the gradebook filter controller renders the record
/// directly.
fn filter_json(row: &FilterRow) -> Value {
    json!({
        "gradebook_filter": {
            "id": row.id,
            "course_id": row.course_id,
            "user_id": row.user_id,
            "name": row.name,
            "payload": row.payload,
            "created_at": format_timestamp(row.created_at),
            "updated_at": format_timestamp(row.updated_at),
        }
    })
}

// ---------------------------------------------------------------------------
// Parameters and validation
// ---------------------------------------------------------------------------

/// The permitted fields of a custom gradebook column request.
///
/// `None` means "the parameter was not sent"; the value is the Ruby cast of
/// the parameter.
#[derive(Debug, Default)]
struct ColumnFields {
    title: Option<String>,
    position: Option<i64>,
    teacher_notes: Option<Option<bool>>,
    hidden: Option<bool>,
    read_only: Option<bool>,
}

/// Read the permitted column fields out of the request body.
fn read_column_fields(body: &Value) -> ColumnFields {
    let source = match body.get("column") {
        Some(inner) if inner.is_object() => inner.clone(),
        _ => body.clone(),
    };

    ColumnFields {
        title: read_string(&source, "title"),
        position: read_i64(&source, "position"),
        teacher_notes: read_boolean(&source, "teacher_notes"),
        hidden: read_hidden(&source),
        read_only: read_boolean(&source, "read_only").flatten(),
    }
}

/// The Rails error hash of the `CustomGradebookColumn` validations.
///
/// `title` is required, at most 255 characters and may not be one of the
/// gradebook importer reserved names. `teacher_notes` has to be `true` or
/// `false` — a value the Rails boolean cast turns into `nil` fails.
fn validate_column(title: Option<&str>, teacher_notes_nil: bool) -> Map<String, Value> {
    let mut errors = Map::new();

    match title {
        None => {
            errors.insert("title".into(), json!(["can't be blank"]));
        }
        Some(title) if title.trim().is_empty() => {
            errors.insert("title".into(), json!(["can't be blank"]));
        }
        Some(title) if title.chars().count() > MAXIMUM_STRING_LENGTH => {
            errors.insert(
                "title".into(),
                json!([format!(
                    "is too long (maximum is {MAXIMUM_STRING_LENGTH} characters)"
                )]),
            );
        }
        Some(title) if GRADEBOOK_IMPORTER_RESERVED_NAMES.contains(&title) => {
            errors.insert(
                "title".into(),
                json!(["cannot use gradebook importer reserved names"]),
            );
        }
        Some(_) => {}
    }

    if teacher_notes_nil {
        errors.insert(
            "teacher_notes".into(),
            json!(["teacher_notes must be true or false"]),
        );
    }

    errors
}

/// The fields of a gradebook filter request.
#[derive(Debug, Default)]
struct FilterFields {
    name: Option<String>,
    payload: Option<Value>,
}

/// Read the permitted gradebook filter fields out of the request body.
fn read_filter_fields(body: &Value) -> FilterFields {
    let source = match body.get("gradebook_filter") {
        Some(inner) if inner.is_object() => inner.clone(),
        _ => body.clone(),
    };

    FilterFields {
        name: read_string(&source, "name"),
        payload: source.get("payload").cloned(),
    }
}

/// The Rails error hash of the `GradebookFilter` validations.
///
/// `name` is required and at most 255 characters, `payload` is required and
/// has to be a hash.
fn validate_filter(name: Option<&str>, payload: Option<&Value>) -> Map<String, Value> {
    let mut errors = Map::new();

    match name {
        None => {
            errors.insert("name".into(), json!(["can't be blank"]));
        }
        Some(name) if name.trim().is_empty() => {
            errors.insert("name".into(), json!(["can't be blank"]));
        }
        Some(name) if name.chars().count() > MAXIMUM_STRING_LENGTH => {
            errors.insert(
                "name".into(),
                json!([format!(
                    "is too long (maximum is {MAXIMUM_STRING_LENGTH} characters)"
                )]),
            );
        }
        Some(_) => {}
    }

    match payload {
        None => {
            errors.insert("payload".into(), json!(["can't be blank"]));
        }
        Some(value) if is_blank(value) => {
            errors.insert("payload".into(), json!(["can't be blank"]));
        }
        Some(value) if !value.is_object() => {
            errors.insert("payload".into(), json!(["must be a hash"]));
        }
        Some(_) => {}
    }

    errors
}

/// Rails `Object#blank?` for a JSON value.
fn is_blank(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(text) => text.trim().is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Ruby casting helpers
// ---------------------------------------------------------------------------

/// Read a string parameter. `None` means "not sent"; an explicit null is
/// cast to the empty string, like the Rails string cast of `nil`.
fn read_string(source: &Value, key: &str) -> Option<String> {
    let value = source.get(key)?;
    Some(match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

/// Read an integer parameter. `None` means "not sent" or "not a number".
fn read_i64(source: &Value, key: &str) -> Option<i64> {
    let value = source.get(key)?;
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|f| f as i64)),
        Value::String(text) => Some(ruby_to_i(text)),
        _ => None,
    }
}

/// Read a boolean parameter with the Rails boolean cast.
///
/// `Some(None)` is a parameter the Rails cast turns into `nil` — the
/// `teacher_notes` inclusion validation fails for such a value.
fn read_boolean(source: &Value, key: &str) -> Option<Option<bool>> {
    let value = source.get(key)?;
    Some(rails_boolean(value))
}

/// The Rails boolean cast of a value.
///
/// `""` and `nil` cast to `nil`, the Rails false values cast to `false`,
/// everything else casts to `true`.
fn rails_boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Null => None,
        Value::Number(number) => Some(number.as_f64().map(|f| f != 0.0).unwrap_or(false)),
        Value::String(text) => {
            if text.trim().is_empty() {
                None
            } else {
                Some(!matches!(
                    text.as_str(),
                    "0" | "f" | "F" | "false" | "FALSE" | "off" | "OFF"
                ))
            }
        }
        _ => Some(true),
    }
}

/// Read the `hidden` parameter.
///
/// `CustomGradebookColumn#hidden=` casts the value with
/// `Canvas::Plugin.value_to_boolean`, not with the Rails boolean cast — so
/// "no" is `false` and "yes" is `true`.
fn read_hidden(source: &Value) -> Option<bool> {
    let value = source.get("hidden")?;
    Some(value_to_boolean(value))
}

/// Canvas `Canvas::Plugin.value_to_boolean`.
fn value_to_boolean(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => value_to_boolean_str(text),
        Value::Number(number) => number.as_f64().map(|f| f != 0.0).unwrap_or(false),
        _ => false,
    }
}

/// Canvas `Canvas::Plugin.value_to_boolean` for a string.
fn value_to_boolean_str(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    if matches!(lower.as_str(), "yes" | "y" | "true" | "t" | "on" | "1") {
        return true;
    }
    if matches!(lower.as_str(), "no" | "n" | "false" | "f" | "off" | "0") {
        return false;
    }
    ruby_to_i(text) != 0
}

/// Ruby's `String#to_i` — the leading integer of the string, `0` when there
/// is none.
fn ruby_to_i(text: &str) -> i64 {
    let trimmed = text.trim_start();
    let bytes = trimmed.as_bytes();
    let mut end = 0;
    if end < bytes.len() && (bytes[end] == b'-' || bytes[end] == b'+') {
        end += 1;
    }
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    trimmed[..end].parse::<i64>().unwrap_or(0)
}

/// The Ruby `to_i` of a JSON value.
fn ruby_to_i_value(value: &Value) -> i64 {
    match value {
        Value::Number(number) => number.as_i64().unwrap_or(0),
        Value::String(text) => ruby_to_i(text),
        _ => 0,
    }
}

/// Canvas `Api.per_page_for` and the will_paginate page resolution.
fn resolve_pagination(page: Option<&str>, per_page: Option<&str>) -> (i64, i64) {
    let per_page = per_page
        .map(ruby_to_i)
        .unwrap_or(DEFAULT_PER_PAGE)
        .clamp(1, MAX_PER_PAGE);
    let page = page.map(ruby_to_i).unwrap_or(1);
    (page, per_page)
}

// ---------------------------------------------------------------------------
// Handlers — custom gradebook columns
// ---------------------------------------------------------------------------

/// `GET /courses/{course_id}/custom_gradebook_columns` — Canvas `index`.
///
/// Without `include_hidden` only the `active` columns are listed; with it
/// every column that is not deleted is listed.
async fn index_columns(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<ColumnListQuery>,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Ok(resource_not_found());
    }
    if !can_manage_grades(&state.db, &auth, course_id).await? {
        return Ok(unauthorized());
    }

    let include_hidden = query
        .include_hidden
        .as_deref()
        .map(value_to_boolean_str)
        .unwrap_or(false);

    let sql = if include_hidden {
        format!(
            "SELECT {COLUMN_COLUMNS} FROM custom_gradebook_columns \
             WHERE course_id = $1 AND workflow_state <> 'deleted' \
             ORDER BY position, title"
        )
    } else {
        format!(
            "SELECT {COLUMN_COLUMNS} FROM custom_gradebook_columns \
             WHERE course_id = $1 AND workflow_state = 'active' \
             ORDER BY position, title"
        )
    };
    let rows: Vec<ColumnRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .fetch_all(&state.db)
        .await?;

    let (page, per_page) = resolve_pagination(query.page.as_deref(), query.per_page.as_deref());
    if page < 1 {
        // Canvas turns an out of range page into a 404.
        return Ok(resource_not_found());
    }
    let offset = ((page - 1) * per_page) as usize;

    let columns: Vec<Value> = rows
        .iter()
        .skip(offset)
        .take(per_page as usize)
        .map(column_json)
        .collect();

    Ok(Json(columns).into_response())
}

/// `POST /courses/{course_id}/custom_gradebook_columns` — Canvas `create`.
async fn create_column(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Ok(resource_not_found());
    }
    if !can_manage_grades(&state.db, &auth, course_id).await? {
        return Ok(unauthorized());
    }

    let fields = read_column_fields(&body);
    let title = fields.title.clone();
    let errors = validate_column(title.as_deref(), matches!(fields.teacher_notes, Some(None)));
    if !errors.is_empty() {
        return Ok(validation_errors(errors));
    }

    let position = match fields.position {
        Some(position) => position,
        // Canvas `acts_as_list` `add_to_list_bottom` puts a new column at the
        // bottom of the list of the course.
        None => next_position(&state.db, course_id).await?,
    };
    let workflow_state = match fields.hidden {
        Some(true) => "hidden",
        _ => "active",
    };
    let teacher_notes = fields.teacher_notes.flatten().unwrap_or(false);
    let read_only = fields.read_only.unwrap_or(false);
    let root_account_id = course_root_account_id(&state.db, course_id).await?;

    let row: ColumnRow = sqlx::query_as(&format!(
        "INSERT INTO custom_gradebook_columns \
         (title, position, workflow_state, course_id, teacher_notes, read_only, \
          root_account_id, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now(), now()) \
         RETURNING {COLUMN_COLUMNS}"
    ))
    .bind(title.unwrap_or_default())
    .bind(position as i32)
    .bind(workflow_state)
    .bind(course_id)
    .bind(teacher_notes)
    .bind(read_only)
    .bind(root_account_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(column_json(&row)).into_response())
}

/// `PUT /courses/{course_id}/custom_gradebook_columns/{id}` — Canvas
/// `update`.
async fn update_column(
    State(state): State<AppState>,
    Path(path): Path<ColumnPath>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, path.course_id).await? {
        return Ok(resource_not_found());
    }
    // Canvas loads the column before it authorizes the caller, so a missing
    // column is a 404 and not a 403.
    let Some(current) = load_column(&state.db, path.course_id, path.id).await? else {
        return Ok(resource_not_found());
    };
    if !can_manage_grades(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }

    let fields = read_column_fields(&body);
    let title = fields
        .title
        .clone()
        .unwrap_or_else(|| current.title.clone());
    let errors = validate_column(Some(&title), matches!(fields.teacher_notes, Some(None)));
    if !errors.is_empty() {
        return Ok(validation_errors(errors));
    }

    let position = fields.position.unwrap_or(i64::from(current.position)) as i32;
    let teacher_notes = fields
        .teacher_notes
        .flatten()
        .unwrap_or(current.teacher_notes);
    let read_only = fields.read_only.unwrap_or(current.read_only);
    let workflow_state = match fields.hidden {
        Some(true) => "hidden".to_string(),
        Some(false) => "active".to_string(),
        None => current.workflow_state.clone(),
    };

    let row: ColumnRow = sqlx::query_as(&format!(
        "UPDATE custom_gradebook_columns SET \
           title = $1, position = $2, workflow_state = $3, teacher_notes = $4, \
           read_only = $5, updated_at = now() \
         WHERE id = $6 RETURNING {COLUMN_COLUMNS}"
    ))
    .bind(title)
    .bind(position)
    .bind(workflow_state)
    .bind(teacher_notes)
    .bind(read_only)
    .bind(current.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(column_json(&row)).into_response())
}

/// `DELETE /courses/{course_id}/custom_gradebook_columns/{id}` — Canvas
/// `destroy`.
///
/// The delete is a soft delete — the row keeps existing with the `deleted`
/// workflow state.
async fn destroy_column(
    State(state): State<AppState>,
    Path(path): Path<ColumnPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, path.course_id).await? {
        return Ok(resource_not_found());
    }
    let Some(current) = load_column(&state.db, path.course_id, path.id).await? else {
        return Ok(resource_not_found());
    };
    if !can_manage_grades(&state.db, &auth, path.course_id).await? {
        return Ok(unauthorized());
    }

    let row: ColumnRow = sqlx::query_as(&format!(
        "UPDATE custom_gradebook_columns SET workflow_state = 'deleted', updated_at = now() \
         WHERE id = $1 RETURNING {COLUMN_COLUMNS}"
    ))
    .bind(current.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(column_json(&row)).into_response())
}

/// `POST /courses/{course_id}/custom_gradebook_columns/reorder` — Canvas
/// `reorder`.
///
/// The Ruby action has no permission check — it only reorders the columns of
/// the course and answers `200 {}`.
async fn reorder_columns(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    _auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Ok(resource_not_found());
    }

    let Some(order) = body.get("order").and_then(Value::as_array) else {
        // Ruby: `params[:order]` is nil and `update_order` raises.
        return Ok(internal_error());
    };

    let ids = dedup_ids(order);
    if ids.is_empty() {
        return Ok(Json(json!({})).into_response());
    }

    apply_reorder(&state.db, course_id, &ids).await?;

    Ok(Json(json!({})).into_response())
}

/// Canvas `acts_as_list` `update_order` — the ids of the columns to move, in
/// their new order.
///
/// The Ruby skips every id that is not a positive number and every id that
/// was already seen.
fn dedup_ids(order: &[Value]) -> Vec<i64> {
    let mut ids: Vec<i64> = Vec::new();
    for value in order {
        let id = ruby_to_i_value(value);
        if id <= 0 || ids.contains(&id) {
            continue;
        }
        ids.push(id);
    }
    ids
}

/// The Canvas `acts_as_list` `update_order` algorithm.
///
/// The listed columns get the position of their rank in the new order, the
/// remaining columns of the course are pushed down.
async fn apply_reorder(pool: &PgPool, course_id: i64, ids: &[i64]) -> Result<(), ApiError> {
    let rows: Vec<(i64, i32)> = sqlx::query_as(
        "SELECT id, position FROM custom_gradebook_columns \
         WHERE course_id = $1 ORDER BY position, id",
    )
    .bind(course_id)
    .fetch_all(pool)
    .await?;

    let moving_positions: Vec<i32> = rows
        .iter()
        .filter(|(id, _)| ids.contains(id))
        .map(|(_, position)| *position)
        .collect();
    let moving_count = moving_positions.len() as i64;

    for (id, position) in rows {
        let new_position = match ids.iter().position(|candidate| *candidate == id) {
            Some(rank) => rank as i64 + 1,
            None => {
                let mut shifted = i64::from(position);
                for (index, moving) in moving_positions.iter().enumerate() {
                    if position <= *moving {
                        shifted = i64::from(position) + (moving_count - index as i64);
                        break;
                    }
                }
                shifted
            }
        };
        if new_position != i64::from(position) {
            sqlx::query("UPDATE custom_gradebook_columns SET position = $1 WHERE id = $2")
                .bind(new_position as i32)
                .bind(id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// Canvas `@context.custom_gradebook_columns.not_deleted.find(params[:id])`.
async fn load_column(
    pool: &PgPool,
    course_id: i64,
    id: i64,
) -> Result<Option<ColumnRow>, ApiError> {
    let row: Option<ColumnRow> = sqlx::query_as(&format!(
        "SELECT {COLUMN_COLUMNS} FROM custom_gradebook_columns \
         WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'"
    ))
    .bind(id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Canvas `acts_as_list` `add_to_list_bottom` — the position of a new column
/// at the bottom of the list of the course.
async fn next_position(pool: &PgPool, course_id: i64) -> Result<i64, ApiError> {
    let max_position: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(position) FROM custom_gradebook_columns WHERE course_id = $1",
    )
    .bind(course_id)
    .fetch_one(pool)
    .await?;
    Ok(i64::from(max_position.unwrap_or(0)) + 1)
}

// ---------------------------------------------------------------------------
// Handlers — gradebook filters
// ---------------------------------------------------------------------------

/// `GET /courses/{course_id}/gradebook_filters` — Canvas `index`.
///
/// Only the filters of the caller are listed.
async fn index_filters(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Ok(resource_not_found());
    }

    let rows: Vec<FilterRow> = sqlx::query_as(&format!(
        "SELECT {FILTER_COLUMNS} FROM gradebook_filters \
         WHERE course_id = $1 AND user_id = $2 ORDER BY created_at"
    ))
    .bind(course_id)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    let filters: Vec<Value> = rows.iter().map(filter_json).collect();
    Ok(Json(filters).into_response())
}

/// `POST /courses/{course_id}/gradebook_filters` — Canvas `create`.
async fn create_filter(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Ok(resource_not_found());
    }

    let fields = read_filter_fields(&body);
    let errors = validate_filter(fields.name.as_deref(), fields.payload.as_ref());
    if !errors.is_empty() {
        return Ok(validation_errors(errors));
    }

    let row: FilterRow = sqlx::query_as(&format!(
        "INSERT INTO gradebook_filters \
         (course_id, user_id, name, payload, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, now(), now()) \
         RETURNING {FILTER_COLUMNS}"
    ))
    .bind(course_id)
    .bind(auth.id)
    .bind(fields.name.unwrap_or_default())
    .bind(fields.payload.unwrap_or_else(|| json!({})))
    .fetch_one(&state.db)
    .await?;

    Ok(Json(filter_json(&row)).into_response())
}

/// `GET /courses/{course_id}/gradebook_filters/{id}` — Canvas `show`.
///
/// The Ruby answers `201 Created` here.
async fn show_filter(
    State(state): State<AppState>,
    Path(path): Path<FilterPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, path.course_id).await? {
        return Ok(resource_not_found());
    }
    let Some(row) = load_filter(&state.db, path.course_id, path.id, auth.id).await? else {
        return Ok(resource_not_found());
    };
    // Canvas `GradebookFilter` grants `:read` to the owner of the filter.
    if row.user_id != auth.id {
        return Ok(unauthorized());
    }

    Ok((StatusCode::CREATED, Json(filter_json(&row))).into_response())
}

/// `PUT /courses/{course_id}/gradebook_filters/{id}` — Canvas `update`.
async fn update_filter(
    State(state): State<AppState>,
    Path(path): Path<FilterPath>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, path.course_id).await? {
        return Ok(resource_not_found());
    }
    let Some(current) = load_filter(&state.db, path.course_id, path.id, auth.id).await? else {
        return Ok(resource_not_found());
    };
    // Canvas `GradebookFilter` grants `:update` to the owner of the filter.
    if current.user_id != auth.id {
        return Ok(unauthorized());
    }

    let fields = read_filter_fields(&body);
    let name = fields.name.unwrap_or_else(|| current.name.clone());
    let payload = fields.payload.unwrap_or_else(|| current.payload.clone());
    let errors = validate_filter(Some(&name), Some(&payload));
    if !errors.is_empty() {
        return Ok(validation_errors(errors));
    }

    let row: FilterRow = sqlx::query_as(&format!(
        "UPDATE gradebook_filters SET name = $1, payload = $2, updated_at = now() \
         WHERE id = $3 RETURNING {FILTER_COLUMNS}"
    ))
    .bind(name)
    .bind(payload)
    .bind(current.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(filter_json(&row)).into_response())
}

/// `DELETE /courses/{course_id}/gradebook_filters/{id}` — Canvas `destroy`.
async fn destroy_filter(
    State(state): State<AppState>,
    Path(path): Path<FilterPath>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, path.course_id).await? {
        return Ok(resource_not_found());
    }
    let Some(current) = load_filter(&state.db, path.course_id, path.id, auth.id).await? else {
        return Ok(resource_not_found());
    };
    // Canvas `GradebookFilter` grants `:destroy` to the owner of the filter.
    if current.user_id != auth.id {
        return Ok(unauthorized());
    }

    sqlx::query("DELETE FROM gradebook_filters WHERE id = $1")
        .bind(current.id)
        .execute(&state.db)
        .await?;

    Ok(Json(filter_json(&current)).into_response())
}

/// Canvas `@context.gradebook_filters.find_by!(id: params[:id], user:
/// @current_user)` — a filter of another user is not found.
async fn load_filter(
    pool: &PgPool,
    course_id: i64,
    id: i64,
    user_id: i64,
) -> Result<Option<FilterRow>, ApiError> {
    let row: Option<FilterRow> = sqlx::query_as(&format!(
        "SELECT {FILTER_COLUMNS} FROM gradebook_filters \
         WHERE id = $1 AND course_id = $2 AND user_id = $3"
    ))
    .bind(id)
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The router of the custom gradebook column and gradebook filter routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{course_id}/custom_gradebook_columns",
            get(index_columns).post(create_column),
        )
        .route(
            "/courses/{course_id}/custom_gradebook_columns/reorder",
            post(reorder_columns),
        )
        .route(
            "/courses/{course_id}/custom_gradebook_columns/{id}",
            put(update_column).delete(destroy_column),
        )
        .route(
            "/courses/{course_id}/gradebook_filters",
            get(index_filters).post(create_filter),
        )
        .route(
            "/courses/{course_id}/gradebook_filters/{id}",
            delete(destroy_filter).put(update_filter).get(show_filter),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_build() {
        let _ = router();
    }

    #[test]
    fn value_to_boolean_follows_the_canvas_plugin() {
        assert!(value_to_boolean_str("yes"));
        assert!(value_to_boolean_str("1"));
        assert!(value_to_boolean_str("TRUE"));
        assert!(!value_to_boolean_str("no"));
        assert!(!value_to_boolean_str("0"));
        assert!(!value_to_boolean_str(""));
        assert!(!value_to_boolean_str("abc"));
    }

    #[test]
    fn the_rails_boolean_cast_turns_an_empty_string_into_nil() {
        assert_eq!(rails_boolean(&Value::String(String::new())), None);
        assert_eq!(rails_boolean(&Value::Bool(true)), Some(true));
        assert_eq!(rails_boolean(&Value::String("no".into())), Some(true));
        assert_eq!(rails_boolean(&Value::String("off".into())), Some(false));
    }

    #[test]
    fn the_column_validation_reports_the_rails_errors() {
        let errors = validate_column(None, false);
        assert_eq!(errors.get("title").unwrap()[0], "can't be blank");

        let errors = validate_column(Some("Student"), false);
        assert_eq!(
            errors.get("title").unwrap()[0],
            "cannot use gradebook importer reserved names"
        );

        let long_title = "x".repeat(256);
        let errors = validate_column(Some(&long_title), false);
        assert_eq!(
            errors.get("title").unwrap()[0],
            "is too long (maximum is 255 characters)"
        );

        let errors = validate_column(Some("Stuff"), true);
        assert_eq!(
            errors.get("teacher_notes").unwrap()[0],
            "teacher_notes must be true or false"
        );

        assert!(validate_column(Some("Stuff"), false).is_empty());
    }

    #[test]
    fn the_filter_validation_reports_the_rails_errors() {
        let errors = validate_filter(None, None);
        assert_eq!(errors.get("name").unwrap()[0], "can't be blank");
        assert_eq!(errors.get("payload").unwrap()[0], "can't be blank");

        let errors = validate_filter(Some("n"), Some(&json!({})));
        assert_eq!(errors.get("payload").unwrap()[0], "can't be blank");

        let errors = validate_filter(Some("n"), Some(&json!("text")));
        assert_eq!(errors.get("payload").unwrap()[0], "must be a hash");

        assert!(validate_filter(Some("n"), Some(&json!({ "foo": "bar" }))).is_empty());
    }

    #[test]
    fn the_reorder_ids_skip_the_invalid_and_the_repeated_ones() {
        let order = vec![
            json!(3),
            json!("1"),
            json!(0),
            json!(-2),
            json!(3),
            json!("x"),
        ];
        assert_eq!(dedup_ids(&order), vec![3, 1]);
    }

    #[test]
    fn the_pagination_defaults_to_the_canvas_page_size() {
        assert_eq!(resolve_pagination(None, None), (1, 10));
        assert_eq!(resolve_pagination(Some("2"), Some("1")), (2, 1));
        assert_eq!(resolve_pagination(None, Some("1000")), (1, 100));
        assert_eq!(resolve_pagination(None, Some("0")), (1, 1));
    }
}
