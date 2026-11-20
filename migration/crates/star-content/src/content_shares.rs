//! content_shares — Canvas `ContentSharesController` parity.
//!
//! Routes:
//! - POST   /users/{userId}/content_shares                — create a share
//! - GET    /users/{userId}/content_shares/sent           — the shares I sent
//! - GET    /users/{userId}/content_shares/received        — the shares I received
//! - GET    /users/{userId}/content_shares/unread_count    — the unread count
//! - GET    /users/{userId}/content_shares/{id}            — one share
//! - DELETE /users/{userId}/content_shares/{id}            — remove a share
//! - POST   /users/{userId}/content_shares/{id}/add_users  — add receivers
//! - PUT    /users/{userId}/content_shares/{id}            — mark read or unread
//!
//! Canvas behavior mirrored here (`app/controllers/content_shares_controller.rb`):
//! - `get_user_param` resolves the path user and answers 404 for an unknown
//!   one.
//! - `require_current_user` guards the create, destroy, add_users and update
//!   routes: another user's share is the Canvas 403
//!   `{"message":"You cannot create or modify other users' content shares"}`.
//! - the index, unread_count and show routes only need the `:read` permission
//!   on the user, so an administrator may read another user's shares.
//! - `get_receivers` needs `receiver_ids`: a missing list is a 400, an
//!   unknown receiver is a 404 and an empty list is the 400
//!   `{"message":"No valid receiving users found"}`.
//! - the create route validates the content type against
//!   `assignment,attachment,discussion_topic,page,quiz,module,module_item`,
//!   exports the content as a common cartridge and stores one row per
//!   receiver — the sender's row is `read`, the receivers' rows are `unread`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use std::collections::HashMap;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// Canvas `ContentShare::TYPE_TO_CLASS` — the content types a share may
/// point at.
const ALLOWED_TYPES: [&str; 7] = [
    "assignment",
    "attachment",
    "discussion_topic",
    "page",
    "quiz",
    "module",
    "module_item",
];

/// Canvas `Api::V1::ContentShare::EXPORT_TYPES` — the export selection keys
/// mapped to the content type the API renders.
const EXPORT_TYPES: [(&str, &str); 7] = [
    ("assignments", "assignment"),
    ("attachments", "attachment"),
    ("discussion_topics", "discussion_topic"),
    ("wiki_pages", "page"),
    ("quizzes", "quiz"),
    ("context_modules", "module"),
    ("content_tags", "module_item"),
];

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct ContentShareRow {
    id: i64,
    name: String,
    user_id: i64,
    content_export_id: i64,
    sender_id: Option<i64>,
    read_state: String,
    share_type: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// A user of a share, in the Canvas `user_display_json` shape.
async fn user_display_json(pool: &PgPool, user_id: i64) -> Result<Option<Value>, ApiError> {
    let row: Option<(i64, String)> = sqlx::query_as("SELECT id, name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|(id, name)| {
        json!({
            "id": id,
            "name": name,
            "display_name": name,
            "avatar_image_url": Value::Null,
        })
    }))
}

/// The receivers of a share: the rows the sender made for them.
async fn receivers_of(
    pool: &PgPool,
    sender_share: &ContentShareRow,
) -> Result<Vec<Value>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT user_id FROM content_shares \
         WHERE sender_id = $1 AND content_export_id = $2 AND type = 'received' \
         ORDER BY id ASC",
    )
    .bind(sender_share.user_id)
    .bind(sender_share.content_export_id)
    .fetch_all(pool)
    .await?;

    let mut receivers = Vec::with_capacity(rows.len());
    for (user_id,) in rows {
        if let Some(user) = user_display_json(pool, user_id).await? {
            receivers.push(user);
        }
    }
    Ok(receivers)
}

/// Canvas `Api::V1::ContentShare#content_share_json`.
async fn content_share_json(
    pool: &PgPool,
    share: &ContentShareRow,
    user: &AuthUser,
) -> Result<Value, ApiError> {
    fn ts(value: &DateTime<Utc>) -> String {
        value.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    let mut json = json!({
        "id": share.id,
        "name": share.name,
        "created_at": ts(&share.created_at),
        "updated_at": ts(&share.updated_at),
        "user_id": share.user_id,
        "read_state": share.read_state,
    });
    let map = json.as_object_mut().expect("json object");

    let sender = match share.sender_id {
        Some(sender_id) => user_display_json(pool, sender_id).await?,
        None => None,
    };
    map.insert("sender".to_string(), sender.unwrap_or(Value::Null));
    // Only a sent share has receivers.
    let receivers = if share.share_type == "sent" {
        receivers_of(pool, share).await?
    } else {
        Vec::new()
    };
    map.insert("receivers".to_string(), Value::Array(receivers));

    // The export of the share, with the content type it points at.
    let export: Option<crate::content_exports::ContentExportRow> = sqlx::query_as(&format!(
        "SELECT {} FROM content_exports WHERE id = $1",
        crate::content_exports::EXPORT_COLUMNS
    ))
    .bind(share.content_export_id)
    .fetch_optional(pool)
    .await?;

    if let Some(export) = export {
        let settings: Option<Value> =
            sqlx::query_scalar("SELECT settings FROM content_exports WHERE id = $1")
                .bind(share.content_export_id)
                .fetch_optional(pool)
                .await?;
        let content_type =
            content_type_from_export_settings(settings.as_ref().unwrap_or(&Value::Null));
        map.insert(
            "content_type".to_string(),
            content_type.unwrap_or(Value::Null),
        );
        map.insert("content_export".to_string(), export.to_json(&[]));

        // A course export carries its source course.
        if export.context_type.as_deref() == Some("Course") {
            if let Some(course_id) = export.course_id {
                let course: Option<(i64, String)> =
                    sqlx::query_as("SELECT id, name FROM courses WHERE id = $1")
                        .bind(course_id)
                        .fetch_optional(pool)
                        .await?;
                if let Some((id, name)) = course {
                    map.insert(
                        "source_course".to_string(),
                        json!({ "id": id, "name": name }),
                    );
                }
            }
        }
    }

    let _ = user;
    Ok(json)
}

/// Canvas `Api::V1::ContentShare#get_content_type_from_export_settings`.
fn content_type_from_export_settings(settings: &Value) -> Option<String> {
    let selected = settings.get("selected_content")?.as_object()?;

    let selected_types: Vec<&str> = selected
        .keys()
        .filter_map(|key| {
            EXPORT_TYPES
                .iter()
                .find(|(export_key, _)| export_key == key)
                .map(|(_, content_type)| *content_type)
        })
        .collect();

    // A module or a module item wins over the other selected types.
    for kind in ["module", "module_item"] {
        if selected_types.contains(&kind) {
            return Some(kind.to_string());
        }
    }
    selected_types.first().map(|kind| kind.to_string())
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas `get_user_param`: 404 for an unknown user.
async fn ensure_user(pool: &PgPool, user_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas `authorized_action(@user, @current_user, :read)` on a user: the user
/// themself and an administrator may read the shares.
fn can_read_user(auth: &AuthUser, user_id: i64) -> bool {
    is_admin(auth) || auth.id == user_id
}

/// Canvas `require_current_user`.
fn require_current_user(auth: &AuthUser, user_id: i64) -> Result<(), ApiError> {
    if auth.id == user_id {
        return Ok(());
    }
    Err(ApiError::Forbidden)
}

/// Canvas `get_receivers`: the receiver ids of the request.
fn parse_receiver_ids(
    params: &HashMap<String, String>,
    body: Option<&Value>,
) -> Result<Vec<i64>, ApiError> {
    let mut ids: Vec<i64> = Vec::new();

    let mut push = |value: &str| {
        if let Ok(id) = value.trim().parse::<i64>() {
            ids.push(id);
        }
    };

    if let Some(body) = body {
        match body.get("receiver_ids") {
            Some(Value::Array(items)) => {
                for item in items {
                    if let Some(id) = item.as_i64() {
                        ids.push(id);
                    } else if let Some(text) = item.as_str() {
                        push(text);
                    }
                }
            }
            Some(Value::String(value)) => {
                for item in value.split(',') {
                    push(item);
                }
            }
            _ => {}
        }
    }

    for (key, value) in params {
        if key == "receiver_ids[]" || key == "receiver_ids" {
            push(value);
        }
    }

    Ok(ids)
}

/// Canvas `get_receivers`: the receivers have to exist.
async fn resolve_receivers(pool: &PgPool, ids: &[i64]) -> Result<Vec<i64>, ApiError> {
    if ids.is_empty() {
        return Err(ApiError::BadRequest(
            "No valid receiving users found".into(),
        ));
    }
    let mut receivers = Vec::new();
    for id in ids {
        let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        match row {
            Some(_) => receivers.push(*id),
            // Canvas: api_find_all raises RecordNotFound for an unknown user.
            None => return Err(ApiError::NotFound),
        }
    }
    if receivers.is_empty() {
        return Err(ApiError::BadRequest(
            "No valid receiving users found".into(),
        ));
    }
    Ok(receivers)
}

/// The table and the column of the content a share points at.
fn content_table(content_type: &str) -> Option<(&'static str, &'static str)> {
    match content_type {
        "assignment" => Some(("assignments", "title")),
        "attachment" => Some(("files", "filename")),
        "discussion_topic" => Some(("discussion_topics", "title")),
        "page" => Some(("pages", "title")),
        "quiz" => Some(("quizzes", "title")),
        "module" => Some(("modules", "name")),
        // Canvas maps the module item to a content tag; the port stores no
        // content tags, so such a share finds no content.
        _ => None,
    }
}

/// The shared content: its course and its name (`Context.asset_name`).
///
/// Canvas only shares the content that is not deleted, and the discussion
/// topics only when they are active.
async fn content_info(
    pool: &PgPool,
    content_type: &str,
    content_id: i64,
) -> Result<Option<(i64, String)>, ApiError> {
    let Some((table, title_column)) = content_table(content_type) else {
        return Ok(None);
    };
    let filter = if content_type == "discussion_topic" {
        " AND workflow_state = 'active'"
    } else {
        ""
    };
    let sql = format!("SELECT course_id, {title_column} FROM {table} WHERE id = $1{filter}");
    let row: Option<(Option<i64>, String)> = sqlx::query_as(&sql)
        .bind(content_id)
        .fetch_optional(pool)
        .await?;
    let Some((course_id, title)) = row else {
        return Ok(None);
    };
    let Some(course_id) = course_id else {
        return Ok(None);
    };
    Ok(Some((course_id, title)))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /users/{userId}/content_shares — Canvas `create`.
async fn create_content_share(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<HashMap<String, String>>,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    ensure_user(&state.db, user_id).await?;
    require_current_user(&auth, user_id)?;

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let receiver_ids = parse_receiver_ids(&params, Some(&payload))?;
    let receivers = resolve_receivers(&state.db, &receiver_ids).await?;

    let content_type = payload
        .get("content_type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let content_id = payload.get("content_id").and_then(|value| match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    });

    let (Some(content_id), false) = (content_id, content_type.is_empty()) else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Content type and id required" })),
        ));
    };
    if !ALLOWED_TYPES.contains(&content_type.as_str()) {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": format!(
                    "Content type not allowed. Allowed types: {}",
                    ALLOWED_TYPES.join(",")
                )
            })),
        ));
    }

    // The shared content has to exist.
    let Some((course_id, content_name)) =
        content_info(&state.db, &content_type, content_id).await?
    else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Requested share content not found" })),
        ));
    };

    // Canvas exports the content as a common cartridge; the selection is
    // keyed by the pluralized content type.
    let select_key = match content_type.as_str() {
        "assignment" => "assignments",
        "attachment" => "attachments",
        "discussion_topic" => "discussion_topics",
        "page" => "pages",
        "quiz" => "quizzes",
        "module" => "modules",
        "module_item" => "module_items",
        _ => "",
    };

    let settings = json!({
        "selected_content": { select_key: [content_id.to_string()] },
        "skip_notifications": true,
    });

    let sql = format!(
        "INSERT INTO content_exports \
         (course_id, user_id, context_type, context_id, export_type, workflow_state, progress, settings) \
         VALUES ($1, $2, 'Course', $1, 'common_cartridge', 'created', 0, $3) \
         RETURNING {}",
        crate::content_exports::EXPORT_COLUMNS
    );
    let export_id: i64 = sqlx::query_scalar(&sql)
        .bind(course_id)
        .bind(auth.id)
        .bind(settings)
        .fetch_one(&state.db)
        .await?;

    // The sender's share is read, the receivers' shares are unread.
    let root_account_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;

    let sender_share_id: i64 = sqlx::query_scalar(
        "INSERT INTO content_shares \
         (name, user_id, content_export_id, sender_id, read_state, type, root_account_id) \
         VALUES ($1, $2, $3, $2, 'read', 'sent', $4) RETURNING id",
    )
    .bind(&content_name)
    .bind(auth.id)
    .bind(export_id)
    .bind(root_account_id)
    .fetch_one(&state.db)
    .await?;

    for receiver in &receivers {
        sqlx::query(
            "INSERT INTO content_shares \
             (name, user_id, content_export_id, sender_id, read_state, type, root_account_id) \
             VALUES ($1, $2, $3, $4, 'unread', 'received', $5) \
             ON CONFLICT (user_id, content_export_id, sender_id) DO NOTHING",
        )
        .bind(&content_name)
        .bind(receiver)
        .bind(export_id)
        .bind(auth.id)
        .bind(root_account_id)
        .execute(&state.db)
        .await?;
    }

    let share = find_share(&state.db, sender_share_id).await?;
    Ok((
        StatusCode::CREATED,
        Json(content_share_json(&state.db, &share, &auth).await?),
    ))
}

/// GET /users/{userId}/content_shares/sent — Canvas `index` (sent).
async fn list_sent_content_shares(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    list_content_shares(&state, user_id, "sent", &params, &auth).await
}

/// GET /users/{userId}/content_shares/received — Canvas `index` (received).
async fn list_received_content_shares(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    list_content_shares(&state, user_id, "received", &params, &auth).await
}

/// The shared body of the two index routes.
async fn list_content_shares(
    state: &AppState,
    user_id: i64,
    list: &str,
    params: &HashMap<String, String>,
    auth: &AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_user(&state.db, user_id).await?;
    if !can_read_user(auth, user_id) {
        return Err(ApiError::Forbidden);
    }

    let page = params
        .get("page")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(1)
        .max(1);
    let per_page = params
        .get("per_page")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(10)
        .clamp(1, 100);
    let offset = (page - 1) * per_page;

    // Canvas: `@user.sent_content_shares.by_date` / `received_content_shares.by_date`
    let rows: Vec<ContentShareRow> = if list == "received" {
        sqlx::query_as(
            "SELECT id, name, user_id, content_export_id, sender_id, read_state, type AS share_type, \
                    created_at, updated_at \
             FROM content_shares WHERE user_id = $1 AND type = 'received' \
             ORDER BY created_at DESC, id DESC LIMIT $2 OFFSET $3",
        )
        .bind(user_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT id, name, user_id, content_export_id, sender_id, read_state, type AS share_type, \
                    created_at, updated_at \
             FROM content_shares WHERE user_id = $1 AND type = 'sent' \
             ORDER BY created_at DESC, id DESC LIMIT $2 OFFSET $3",
        )
        .bind(user_id)
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?
    };

    let mut list = Vec::with_capacity(rows.len());
    for row in &rows {
        list.push(content_share_json(&state.db, row, auth).await?);
    }
    Ok(Json(Value::Array(list)))
}

/// GET /users/{userId}/content_shares/unread_count — Canvas `unread_count`.
async fn content_shares_unread_count(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_user(&state.db, user_id).await?;
    if !can_read_user(&auth, user_id) {
        return Err(ApiError::Forbidden);
    }

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM content_shares \
         WHERE user_id = $1 AND type = 'received' AND read_state = 'unread'",
    )
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(json!({ "unread_count": count })))
}

/// GET /users/{userId}/content_shares/{id} — Canvas `show`.
async fn show_content_share(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_user(&state.db, user_id).await?;
    if !can_read_user(&auth, user_id) {
        return Err(ApiError::Forbidden);
    }

    // Canvas: `@user.content_shares.find(params[:id])`
    let row: Option<ContentShareRow> = sqlx::query_as(
        "SELECT id, name, user_id, content_export_id, sender_id, read_state, type AS share_type, \
                created_at, updated_at \
         FROM content_shares WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or(ApiError::NotFound)?;

    Ok(Json(content_share_json(&state.db, &row, &auth).await?))
}

/// DELETE /users/{userId}/content_shares/{id} — Canvas `destroy`.
async fn destroy_content_share(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    ensure_user(&state.db, user_id).await?;
    require_current_user(&auth, user_id)?;

    // Canvas: `@current_user.content_shares.find(params[:id])`
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM content_shares WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(auth.id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::NotFound)?;

    sqlx::query("DELETE FROM content_shares WHERE id = $1")
        .bind(row.0)
        .execute(&state.db)
        .await?;

    Ok(Json(json!({ "message": "content share deleted" })))
}

/// POST /users/{userId}/content_shares/{id}/add_users — Canvas `add_users`.
async fn add_content_share_users(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<HashMap<String, String>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    ensure_user(&state.db, user_id).await?;
    require_current_user(&auth, user_id)?;

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let receiver_ids = parse_receiver_ids(&params, Some(&payload))?;
    let receivers = resolve_receivers(&state.db, &receiver_ids).await?;

    // Canvas: `@current_user.content_shares.find(params[:id])`
    let share = find_share(&state.db, id).await?;
    if share.user_id != auth.id {
        return Err(ApiError::NotFound);
    }
    // Canvas: the share has to be a sent share.
    if share.share_type != "sent" {
        return Err(ApiError::BadRequest(
            "Content share not owned by you".into(),
        ));
    }

    let existing = receivers_of(&state.db, &share).await?;
    let existing_ids: Vec<i64> = existing
        .iter()
        .filter_map(|user| user.get("id").and_then(|id| id.as_i64()))
        .collect();

    let root_account_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(&state.db)
    .await?;

    for receiver in &receivers {
        if existing_ids.contains(receiver) {
            continue;
        }
        sqlx::query(
            "INSERT INTO content_shares \
             (name, user_id, content_export_id, sender_id, read_state, type, root_account_id) \
             VALUES ($1, $2, $3, $4, 'unread', 'received', $5) \
             ON CONFLICT (user_id, content_export_id, sender_id) DO NOTHING",
        )
        .bind(&share.name)
        .bind(receiver)
        .bind(share.content_export_id)
        .bind(auth.id)
        .bind(root_account_id)
        .execute(&state.db)
        .await?;
    }

    let share = find_share(&state.db, id).await?;
    Ok(Json(content_share_json(&state.db, &share, &auth).await?))
}

/// PUT /users/{userId}/content_shares/{id} — Canvas `update`.
async fn update_content_share(
    State(state): State<AppState>,
    Path((user_id, id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<HashMap<String, String>>,
    body: Option<Json<Value>>,
) -> Result<Json<Value>, ApiError> {
    ensure_user(&state.db, user_id).await?;
    require_current_user(&auth, user_id)?;

    // Canvas: `@current_user.content_shares.find(params[:id])`
    let share = find_share(&state.db, id).await?;
    if share.user_id != auth.id {
        return Err(ApiError::NotFound);
    }

    let payload = body.map(|Json(value)| value).unwrap_or(Value::Null);
    let read_state = payload
        .get("read_state")
        .and_then(Value::as_str)
        .or_else(|| params.get("read_state").map(|value| value.as_str()))
        .unwrap_or("")
        .trim()
        .to_string();

    // Canvas validates the read state.
    if !read_state.is_empty() && read_state != "read" && read_state != "unread" {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({ "read_state": ["is not included in the list"] })),
        ));
    }

    if !read_state.is_empty() {
        sqlx::query("UPDATE content_shares SET read_state = $1, updated_at = now() WHERE id = $2")
            .bind(&read_state)
            .bind(share.id)
            .execute(&state.db)
            .await?;
    }

    let share = find_share(&state.db, id).await?;
    Ok(Json(content_share_json(&state.db, &share, &auth).await?))
}

/// Load one share.
async fn find_share(pool: &PgPool, id: i64) -> Result<ContentShareRow, ApiError> {
    sqlx::query_as(
        "SELECT id, name, user_id, content_export_id, sender_id, read_state, type AS share_type, \
                created_at, updated_at \
         FROM content_shares WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the content shares router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/users/{userId}/content_shares", post(create_content_share))
        .route(
            "/users/{userId}/content_shares/sent",
            get(list_sent_content_shares),
        )
        .route(
            "/users/{userId}/content_shares/received",
            get(list_received_content_shares),
        )
        .route(
            "/users/{userId}/content_shares/unread_count",
            get(content_shares_unread_count),
        )
        .route(
            "/users/{userId}/content_shares/{id}",
            get(show_content_share)
                .put(update_content_share)
                .delete(destroy_content_share),
        )
        .route(
            "/users/{userId}/content_shares/{id}/add_users",
            post(add_content_share_users),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowed_content_types_match_canvas() {
        assert_eq!(
            ALLOWED_TYPES,
            [
                "assignment",
                "attachment",
                "discussion_topic",
                "page",
                "quiz",
                "module",
                "module_item"
            ]
        );
    }

    #[test]
    fn the_content_type_comes_from_the_export_settings() {
        let settings = json!({ "selected_content": { "assignments": ["1"] } });
        assert_eq!(
            content_type_from_export_settings(&settings),
            Some("assignment".to_string())
        );

        // A module wins over the other selected types.
        let settings =
            json!({ "selected_content": { "assignments": ["1"], "context_modules": ["2"] } });
        assert_eq!(
            content_type_from_export_settings(&settings),
            Some("module".to_string())
        );

        // A module item wins over a module.
        let settings = json!({
            "selected_content": { "context_modules": ["2"], "content_tags": ["3"] }
        });
        assert_eq!(
            content_type_from_export_settings(&settings),
            Some("module_item".to_string())
        );

        // Without a selection there is no content type.
        assert_eq!(content_type_from_export_settings(&json!({})), None);
    }

    #[test]
    fn the_receiver_ids_are_read_from_the_body_and_the_query() {
        let mut params = HashMap::new();
        params.insert("receiver_ids[]".to_string(), "7".to_string());
        let body = json!({ "receiver_ids": [8, "9"] });
        let ids = parse_receiver_ids(&params, Some(&body)).expect("parse the receiver ids");
        assert_eq!(ids, vec![8, 9, 7]);
    }
}
