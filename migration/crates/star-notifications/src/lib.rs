//! star-notifications — notification feed + notification preferences for star-lms.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.
//!
//! Canvas parity additions (migration 0045):
//! - GET    /users/self/communication_channels
//! - POST   /users/self/communication_channels
//! - DELETE /users/self/communication_channels/{id}
//! - GET    /users/self/communication_channels/{communicationChannelId}/notification_preferences
//! - PUT    /users/self/communication_channels/{communicationChannelId}/notification_preferences
//! - PUT    /users/self/communication_channels/{communicationChannelId}/notification_preferences/{notification}
//!
//! Canvas parity additions (migration 0074):
//! - GET    /users/{userId}/notification_preferences
//! - PUT    /users/{userId}/notification_preferences

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// --------------------------------------------------------------------------
// Domain types
// --------------------------------------------------------------------------

/// A notification item as returned by the feed API.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct NotificationItem {
    pub id: i64,
    pub user_id: i64,
    pub subject: String,
    pub body: String,
    pub category: String,
    pub workflow_state: String,
    pub link: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Response for unread count.
#[derive(Debug, Serialize)]
pub struct UnreadCountResponse {
    pub count: i64,
}

/// A notification preference policy.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct NotificationPolicyRow {
    pub id: i64,
    pub user_id: i64,
    pub channel: String,
    pub frequency: String,
    pub notification_category: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Canvas wraps notification policies in a `notification_preferences` key.
#[derive(Debug, Serialize)]
pub struct NotificationPreferencesResponse {
    pub notification_preferences: Vec<NotificationPolicyRow>,
}

/// Request payload for updating a notification preference.
#[derive(Debug, Deserialize)]
pub struct UpdatePreferenceRequest {
    pub notification_category: String,
    pub frequency: String,
}

// --------------------------------------------------------------------------
// Domain types — Communication Channels (Canvas API v1)
// --------------------------------------------------------------------------

/// A user communication channel, shaped like Canvas' `CommunicationChannel`.
///
/// Canvas JSON fields: `id, address, type, position, user_id, workflow_state,
/// created_at`. `path`/`path_type` are the DB column names Canvas uses.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CommunicationChannel {
    pub id: i64,
    #[sqlx(rename = "path")]
    pub address: String,
    #[serde(rename = "type")]
    #[sqlx(rename = "path_type")]
    pub channel_type: String,
    pub position: i32,
    pub user_id: i64,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
}

/// Canvas' documented `communication_channel[...]` parameter bag.
#[derive(Debug, Deserialize)]
pub struct CommunicationChannelFields {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(rename = "type", default)]
    pub channel_type: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub skip_confirmation: Option<bool>,
}

/// POST body for creating a communication channel.
///
/// Accepts both flat JSON fields and Canvas' nested
/// `communication_channel: { address, type, token, skip_confirmation }` form.
#[derive(Debug, Deserialize)]
pub struct CreateCommunicationChannelRequest {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(rename = "type", default)]
    pub channel_type: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub skip_confirmation: Option<bool>,
    #[serde(default)]
    pub communication_channel: Option<CommunicationChannelFields>,
}

impl CreateCommunicationChannelRequest {
    /// Flat fields win over nested Canvas-style fields.
    fn parts(&self) -> (Option<&str>, Option<&str>, Option<&str>, bool) {
        let nested = self.communication_channel.as_ref();
        (
            self.address
                .as_deref()
                .or_else(|| nested.and_then(|n| n.address.as_deref())),
            self.channel_type
                .as_deref()
                .or_else(|| nested.and_then(|n| n.channel_type.as_deref())),
            self.token
                .as_deref()
                .or_else(|| nested.and_then(|n| n.token.as_deref())),
            self.skip_confirmation.unwrap_or(false)
                || nested.and_then(|n| n.skip_confirmation).unwrap_or(false),
        )
    }
}

/// A Canvas `NotificationPreference` row (scoped to one communication channel).
/// Canvas serializes exactly `notification`, `category`, `frequency`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct NotificationPreference {
    pub notification: String,
    pub category: String,
    pub frequency: String,
}

/// Canvas wraps channel preferences in a `notification_preferences` key.
#[derive(Debug, Serialize)]
pub struct CommunicationChannelPreferencesResponse {
    pub notification_preferences: Vec<NotificationPreference>,
}

/// User-level preference lists use the same Canvas envelope as channel ones.
pub type UserNotificationPreferencesResponse = CommunicationChannelPreferencesResponse;

const CHANNEL_TYPES: [&str; 3] = ["email", "sms", "push"];
const FREQUENCIES: [&str; 4] = ["immediately", "daily", "weekly", "never"];

/// Lightweight email check mirroring Canvas' EmailAddressValidator intent
/// (local part and dotted domain, no whitespace).
fn is_valid_email(value: &str) -> bool {
    let mut parts = value.split('@');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(local), Some(domain), None) => {
            !local.is_empty()
                && !domain.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
                && !value.chars().any(char::is_whitespace)
        }
        _ => false,
    }
}

/// Normalize a channel create payload into `(address, type)`.
/// Errors are 400s, matching Canvas' `render json: @cc.errors, status: :bad_request`.
fn validate_new_channel(
    address: Option<&str>,
    channel_type: Option<&str>,
    token: Option<&str>,
) -> Result<(String, String), ApiError> {
    let channel_type = channel_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest("communication_channel[type] is required".into()))?;
    if !CHANNEL_TYPES.contains(&channel_type) {
        return Err(ApiError::BadRequest(
            "type must be one of: email, sms, push".into(),
        ));
    }
    if channel_type == "push" {
        let has_token = token.map(str::trim).is_some_and(|value| !value.is_empty());
        if !has_token {
            return Err(ApiError::BadRequest(
                "communication_channel[token] is required for push channels".into(),
            ));
        }
        // Canvas stores "push" as the path; the device token lives on a
        // separate notification endpoint table that this schema does not have.
        return Ok(("push".to_string(), channel_type.to_string()));
    }
    let address = address
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::BadRequest("communication_channel[address] is required".into()))?;
    if channel_type == "email" && !is_valid_email(address) {
        return Err(ApiError::BadRequest("address is invalid".into()));
    }
    Ok((address.to_string(), channel_type.to_string()))
}

fn validate_frequency(raw: Option<&str>) -> Result<String, ApiError> {
    let frequency = raw.map(str::trim).unwrap_or_default();
    if FREQUENCIES.contains(&frequency) {
        Ok(frequency.to_string())
    } else {
        Err(ApiError::Validation(
            "frequency must be one of: immediately, daily, weekly, never".into(),
        ))
    }
}

fn validate_notification(raw: &str) -> Result<String, ApiError> {
    let notification = raw.trim().to_ascii_lowercase();
    let is_slug = !notification.is_empty()
        && notification
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_');
    if is_slug {
        Ok(notification)
    } else {
        Err(ApiError::Validation(
            "notification must be an underscored name".into(),
        ))
    }
}

/// Best-effort mapping from a Canvas notification name to its category slug
/// (`new_announcement` -> `announcement`). Canvas keeps this in the Notification
/// catalog; this schema has no catalog table.
fn derive_category(notification: &str) -> &'static str {
    if notification.contains("announcement") {
        "announcement"
    } else if notification.contains("discussion") {
        "discussion"
    } else if notification.contains("conversation") || notification.contains("message") {
        "conversation"
    } else if notification.contains("grade") || notification.contains("submission") {
        "grading"
    } else if notification.contains("due_date") || notification.contains("assignment") {
        "due_date"
    } else if notification.contains("calendar") || notification.contains("appointment") {
        "calendar"
    } else if notification.contains("enrollment") {
        "registration"
    } else if notification.contains("account") || notification.contains("report") {
        "administrative"
    } else {
        "other"
    }
}

/// Extract the frequency for a single-notification update. Canvas accepts
/// `notification_preferences[frequency]`, JSON-API array form, or a bare object.
fn parse_single_frequency(body: &serde_json::Value) -> Result<String, ApiError> {
    let preference = body.get("notification_preferences").unwrap_or(body);
    match preference {
        serde_json::Value::Object(map) => {
            validate_frequency(map.get("frequency").and_then(|v| v.as_str()))
        }
        serde_json::Value::Array(items) => {
            let first = items.first().ok_or_else(|| {
                ApiError::Validation("notification_preferences must not be empty".into())
            })?;
            parse_single_frequency(first)
        }
        serde_json::Value::String(frequency) => validate_frequency(Some(frequency.as_str())),
        _ => Err(ApiError::Validation(
            "notification_preferences[frequency] is required".into(),
        )),
    }
}

/// Extract all `(notification, frequency)` pairs from Canvas' `update_all` body.
///
/// Supported shapes:
/// - `{"notification_preferences": {"new_announcement": {"frequency": "daily"}}}`
/// - `{"notification_preferences": [{"notification": "x", "frequency": "daily"}]}`
/// - `{"notification_preferences": [["x", "daily"]]}`
/// - `{"notification_preferences": {"notification": "x", "frequency": "daily"}}`
fn parse_preference_updates(body: &serde_json::Value) -> Result<Vec<(String, String)>, ApiError> {
    let preference = body.get("notification_preferences").unwrap_or(body);
    let mut updates: Vec<(String, String)> = Vec::new();
    match preference {
        serde_json::Value::Object(map) if map.contains_key("frequency") => {
            let notification = map
                .get("notification")
                .or_else(|| body.get("notification"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::Validation("notification is required".into()))?;
            updates.push((
                validate_notification(notification)?,
                validate_frequency(map.get("frequency").and_then(|v| v.as_str()))?,
            ));
        }
        serde_json::Value::Object(map) => {
            for (notification, raw) in map {
                let frequency = match raw {
                    serde_json::Value::Object(entry) => {
                        entry.get("frequency").and_then(|v| v.as_str())
                    }
                    serde_json::Value::String(value) => Some(value.as_str()),
                    _ => None,
                };
                updates.push((
                    validate_notification(notification)?,
                    validate_frequency(frequency)?,
                ));
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                match item {
                    serde_json::Value::Object(map) => {
                        let notification = map
                            .get("notification")
                            .and_then(|v| v.as_str())
                            .ok_or_else(|| {
                                ApiError::Validation("notification is required".into())
                            })?;
                        updates.push((
                            validate_notification(notification)?,
                            validate_frequency(map.get("frequency").and_then(|v| v.as_str()))?,
                        ));
                    }
                    serde_json::Value::Array(pair) => {
                        let notification =
                            pair.first().and_then(|v| v.as_str()).ok_or_else(|| {
                                ApiError::Validation("notification is required".into())
                            })?;
                        updates.push((
                            validate_notification(notification)?,
                            validate_frequency(pair.get(1).and_then(|v| v.as_str()))?,
                        ));
                    }
                    _ => {
                        return Err(ApiError::Validation(
                            "notification_preferences entries must be objects".into(),
                        ));
                    }
                }
            }
        }
        _ => {
            return Err(ApiError::Validation(
                "notification_preferences is required".into(),
            ));
        }
    }
    if updates.is_empty() {
        return Err(ApiError::Validation(
            "notification_preferences must not be empty".into(),
        ));
    }
    Ok(updates)
}

// --------------------------------------------------------------------------
// Router
// --------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // Notification feed
        .route("/users/self/notifications", get(list_notifications))
        .route("/notifications/{id}/read", put(mark_read))
        .route("/notifications/{id}", delete(delete_notification))
        .route("/notifications/unread_count", get(unread_count))
        // Notification preferences
        .route(
            "/users/self/notification_preferences",
            get(list_preferences),
        )
        .route(
            "/users/self/notification_preferences",
            put(upsert_preference),
        )
        // Communication channels + per-channel notification preferences
        .route(
            "/users/self/communication_channels",
            get(list_communication_channels),
        )
        .route(
            "/users/self/communication_channels",
            post(create_communication_channel),
        )
        .route(
            "/users/self/communication_channels/{id}",
            delete(delete_communication_channel),
        )
        .route(
            "/users/self/communication_channels/{communication_channel_id}/notification_preferences",
            get(list_channel_preferences),
        )
        .route(
            "/users/self/communication_channels/{communication_channel_id}/notification_preferences",
            put(update_channel_preferences),
        )
        .route(
            "/users/self/communication_channels/{communication_channel_id}/notification_preferences/{notification}",
            put(update_channel_preference),
        )
        // User-level notification preferences (Canvas API v1)
        .route(
            "/users/{user_id}/notification_preferences",
            get(list_user_preferences),
        )
        .route(
            "/users/{user_id}/notification_preferences",
            put(update_user_preferences),
        )
}

// --------------------------------------------------------------------------
// Handlers — Notifications
// --------------------------------------------------------------------------

/// GET /users/self/notifications — list the authenticated user's notifications.
/// Excludes deleted notifications; newest first.
async fn list_notifications(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<NotificationItem>>, ApiError> {
    let rows: Vec<NotificationItem> = sqlx::query_as(
        r#"
        SELECT id, user_id, subject, body, category, workflow_state, link, created_at, updated_at
        FROM notifications
        WHERE user_id = $1 AND workflow_state != 'deleted'
        ORDER BY created_at DESC
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(rows))
}

/// PUT /notifications/{id}/read — mark a notification as read.
/// Only the notification owner can do this.
async fn mark_read(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<NotificationItem>, ApiError> {
    // Verify the notification exists and belongs to the authenticated user.
    let row: Option<NotificationItem> = sqlx::query_as(
        r#"
        SELECT id, user_id, subject, body, category, workflow_state, link, created_at, updated_at
        FROM notifications
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await
    .map_err(ApiError::from)?;

    let notification = row.ok_or(ApiError::NotFound)?;

    if notification.user_id != auth.id {
        return Err(ApiError::Forbidden);
    }

    let updated: NotificationItem = sqlx::query_as(
        r#"
        UPDATE notifications
        SET workflow_state = 'read', updated_at = NOW()
        WHERE id = $1
        RETURNING id, user_id, subject, body, category, workflow_state, link, created_at, updated_at
        "#,
    )
    .bind(id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(updated))
}

/// DELETE /notifications/{id} — soft-delete a notification (set workflow_state = 'deleted').
/// Only the notification owner can do this.
async fn delete_notification(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    // Verify the notification exists and belongs to the authenticated user.
    let row: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM notifications WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;

    let user_id = row.ok_or(ApiError::NotFound)?.0;

    if user_id != auth.id {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        "UPDATE notifications SET workflow_state = 'deleted', updated_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// GET /notifications/unread_count — return the count of unread notifications for the authed user.
async fn unread_count(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<UnreadCountResponse>, ApiError> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)::BIGINT FROM notifications
        WHERE user_id = $1 AND workflow_state = 'unread'
        "#,
    )
    .bind(auth.id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(UnreadCountResponse { count: row.0 }))
}

// --------------------------------------------------------------------------
// Handlers — Notification Preferences
// --------------------------------------------------------------------------

/// PUT /users/self/notification_preferences — upsert a notification preference.
/// If a policy for (user_id, notification_category) exists, update it; otherwise insert.
async fn upsert_preference(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<UpdatePreferenceRequest>,
) -> Result<Json<NotificationPreferencesResponse>, ApiError> {
    // Validate frequency value.
    match payload.frequency.as_str() {
        "immediately" | "daily" | "weekly" | "never" => {}
        _ => {
            return Err(ApiError::Validation(
                "frequency must be one of: immediately, daily, weekly, never".into(),
            ));
        }
    }

    let row: NotificationPolicyRow = sqlx::query_as(
        r#"
        INSERT INTO notification_policies (user_id, channel, frequency, notification_category)
        VALUES ($1, 'email', $2, $3)
        ON CONFLICT (user_id, notification_category)
        DO UPDATE SET frequency = EXCLUDED.frequency, updated_at = NOW()
        RETURNING id, user_id, channel, frequency, notification_category, created_at, updated_at
        "#,
    )
    .bind(auth.id)
    .bind(&payload.frequency)
    .bind(&payload.notification_category)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(NotificationPreferencesResponse {
        notification_preferences: vec![row],
    }))
}

/// GET /users/self/notification_preferences — list all notification preferences for the authed user.
async fn list_preferences(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<NotificationPreferencesResponse>, ApiError> {
    let rows: Vec<NotificationPolicyRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, channel, frequency, notification_category, created_at, updated_at
        FROM notification_policies
        WHERE user_id = $1
        ORDER BY notification_category ASC
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(NotificationPreferencesResponse {
        notification_preferences: rows,
    }))
}

// --------------------------------------------------------------------------
// Handlers — Communication Channels
// --------------------------------------------------------------------------

/// GET /users/self/communication_channels — unretired channels by position.
async fn list_communication_channels(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<CommunicationChannel>>, ApiError> {
    let rows: Vec<CommunicationChannel> = sqlx::query_as(
        r#"
        SELECT id, path, path_type, position, user_id, workflow_state, created_at
        FROM communication_channels
        WHERE user_id = $1 AND workflow_state != 'retired'
        ORDER BY position ASC, id ASC
        "#,
    )
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// POST /users/self/communication_channels — create (or reactivate) a channel.
/// Canvas returns 200 with the channel JSON; duplicates are 400.
async fn create_communication_channel(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateCommunicationChannelRequest>,
) -> Result<Json<CommunicationChannel>, ApiError> {
    let (address, channel_type, token, skip_confirmation) = payload.parts();
    let (address, channel_type) = validate_new_channel(address, channel_type, token)?;

    // Canvas only lets admins skip confirmation; everyone else is unconfirmed.
    let workflow_state = if skip_confirmation && auth.role == "admin" {
        "active"
    } else {
        "unconfirmed"
    };

    let existing: Option<(i64, String)> = sqlx::query_as(
        r#"
        SELECT id, workflow_state
        FROM communication_channels
        WHERE user_id = $1 AND path_type = $2 AND lower(path) = lower($3)
        ORDER BY CASE WHEN workflow_state = 'retired' THEN 1 ELSE 0 END, id ASC
        LIMIT 1
        "#,
    )
    .bind(auth.id)
    .bind(&channel_type)
    .bind(&address)
    .fetch_optional(&state.db)
    .await?;

    if let Some((id, existing_state)) = existing {
        if existing_state != "retired" {
            return Err(ApiError::BadRequest("path: unique!".into()));
        }
        // Reactivate a retired channel in place (Canvas' re_activate!).
        let reactivated: CommunicationChannel = sqlx::query_as(
            r#"
            UPDATE communication_channels
            SET workflow_state = $2, updated_at = NOW()
            WHERE id = $1
            RETURNING id, path, path_type, position, user_id, workflow_state, created_at
            "#,
        )
        .bind(id)
        .bind(workflow_state)
        .fetch_one(&state.db)
        .await?;
        return Ok(Json(reactivated));
    }

    let position: (i32,) = sqlx::query_as(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM communication_channels WHERE user_id = $1",
    )
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    let channel: CommunicationChannel = sqlx::query_as(
        r#"
        INSERT INTO communication_channels (user_id, path, path_type, position, workflow_state)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, path, path_type, position, user_id, workflow_state, created_at
        "#,
    )
    .bind(auth.id)
    .bind(&address)
    .bind(&channel_type)
    .bind(position.0)
    .bind(workflow_state)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(channel))
}

/// DELETE /users/self/communication_channels/{id} — retire a channel.
/// Canvas returns the retired channel JSON (200), not 204.
async fn delete_communication_channel(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<i64>,
) -> Result<Json<CommunicationChannel>, ApiError> {
    let row: Option<CommunicationChannel> = sqlx::query_as(
        r#"
        UPDATE communication_channels
        SET workflow_state = 'retired', updated_at = NOW()
        WHERE id = $1 AND user_id = $2 AND workflow_state != 'retired'
        RETURNING id, path, path_type, position, user_id, workflow_state, created_at
        "#,
    )
    .bind(id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;

    row.map(Json).ok_or(ApiError::NotFound)
}

// --------------------------------------------------------------------------
// Handlers — Per-channel notification preferences (Canvas API v1)
// --------------------------------------------------------------------------

/// Verify the channel belongs to the caller and is not retired.
async fn owned_channel_exists(
    pool: &sqlx::PgPool,
    user_id: i64,
    channel_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT id FROM communication_channels
        WHERE id = $1 AND user_id = $2 AND workflow_state != 'retired'
        "#,
    )
    .bind(channel_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    row.map(|_| ()).ok_or(ApiError::NotFound)
}

async fn list_preferences_for_channel(
    pool: &sqlx::PgPool,
    user_id: i64,
    channel_id: i64,
) -> Result<Vec<NotificationPreference>, ApiError> {
    let rows: Vec<NotificationPreference> = sqlx::query_as(
        r#"
        SELECT notification, category, frequency
        FROM notification_preferences
        WHERE communication_channel_id = $1 AND user_id = $2
        ORDER BY notification ASC
        "#,
    )
    .bind(channel_id)
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

async fn upsert_channel_preference(
    pool: &sqlx::PgPool,
    user_id: i64,
    channel_id: i64,
    notification: &str,
    frequency: &str,
) -> Result<NotificationPreference, ApiError> {
    let category = derive_category(notification);
    let row: NotificationPreference = sqlx::query_as(
        r#"
        INSERT INTO notification_preferences
            (user_id, communication_channel_id, notification, category, frequency)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (communication_channel_id, notification)
        DO UPDATE SET
            frequency = EXCLUDED.frequency,
            category = EXCLUDED.category,
            updated_at = NOW()
        RETURNING notification, category, frequency
        "#,
    )
    .bind(user_id)
    .bind(channel_id)
    .bind(notification)
    .bind(category)
    .bind(frequency)
    .fetch_one(pool)
    .await?;

    Ok(row)
}

/// GET /users/self/communication_channels/{id}/notification_preferences
async fn list_channel_preferences(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
) -> Result<Json<CommunicationChannelPreferencesResponse>, ApiError> {
    owned_channel_exists(&state.db, auth.id, channel_id).await?;
    let rows = list_preferences_for_channel(&state.db, auth.id, channel_id).await?;
    Ok(Json(CommunicationChannelPreferencesResponse {
        notification_preferences: rows,
    }))
}

/// PUT /users/self/communication_channels/{id}/notification_preferences
/// Canvas' `update_all`: upsert every notification in `notification_preferences`
/// and return all stored preferences for the channel.
async fn update_channel_preferences(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(channel_id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<CommunicationChannelPreferencesResponse>, ApiError> {
    owned_channel_exists(&state.db, auth.id, channel_id).await?;
    let updates = parse_preference_updates(&body)?;
    for (notification, frequency) in &updates {
        upsert_channel_preference(&state.db, auth.id, channel_id, notification, frequency).await?;
    }
    let rows = list_preferences_for_channel(&state.db, auth.id, channel_id).await?;
    Ok(Json(CommunicationChannelPreferencesResponse {
        notification_preferences: rows,
    }))
}

/// PUT /users/self/communication_channels/{id}/notification_preferences/{notification}
/// Canvas' single-notification update; returns a one-element envelope.
async fn update_channel_preference(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((channel_id, notification)): Path<(i64, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<CommunicationChannelPreferencesResponse>, ApiError> {
    owned_channel_exists(&state.db, auth.id, channel_id).await?;
    let notification = validate_notification(&notification)?;
    let frequency = parse_single_frequency(&body)?;
    let row = upsert_channel_preference(&state.db, auth.id, channel_id, &notification, &frequency)
        .await?;

    Ok(Json(CommunicationChannelPreferencesResponse {
        notification_preferences: vec![row],
    }))
}

// --------------------------------------------------------------------------
// Handlers — User-level notification preferences (Canvas API v1)
// --------------------------------------------------------------------------

/// Resolve Canvas' `:user_id` path segment. `self` maps to the authenticated
/// user; anything that is not a positive integer is a 404 (`api_find`).
fn resolve_user_segment(raw: &str, self_id: i64) -> Result<i64, ApiError> {
    if raw == "self" {
        return Ok(self_id);
    }
    raw.parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or(ApiError::NotFound)
}

async fn user_exists(pool: &sqlx::PgPool, user_id: i64) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Canvas lets authorized viewers (view_statistics) read another user's
/// preferences, but only the user themselves updates them. This crate has no
/// view_statistics permission, so reads are self-or-admin and writes are self.
fn authorize_preferences_read(auth: &AuthUser, target_user_id: i64) -> Result<(), ApiError> {
    if target_user_id == auth.id || auth.role == "admin" {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

fn authorize_preferences_write(auth: &AuthUser, target_user_id: i64) -> Result<(), ApiError> {
    if target_user_id == auth.id {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// One row per notification, taken from the user's primary (lowest position)
/// unretired channel. Canvas' `NotificationPreference` model has no channel
/// reference, so the user-level view collapses per-channel policies.
async fn list_user_preferences_for(
    pool: &sqlx::PgPool,
    user_id: i64,
) -> Result<Vec<NotificationPreference>, ApiError> {
    let rows: Vec<NotificationPreference> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (np.notification) np.notification, np.category, np.frequency
        FROM notification_preferences np
        JOIN communication_channels cc ON cc.id = np.communication_channel_id
        WHERE np.user_id = $1 AND cc.workflow_state <> 'retired'
        ORDER BY np.notification ASC, cc.position ASC, cc.id ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Canvas provisions an email communication channel for every user; this
/// schema does not, so a preference update first creates the primary email
/// channel from `users.email` (active, since the address is already verified).
async fn ensure_unretired_channel(pool: &sqlx::PgPool, user_id: i64) -> Result<(), ApiError> {
    let existing: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT id FROM communication_channels
        WHERE user_id = $1 AND workflow_state <> 'retired'
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    if existing.is_some() {
        return Ok(());
    }

    let email: (String,) = sqlx::query_as("SELECT email FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await?;

    sqlx::query(
        r#"
        INSERT INTO communication_channels (user_id, path, path_type, position, workflow_state)
        VALUES ($1, $2, 'email', 0, 'active')
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(email.0)
    .execute(pool)
    .await?;

    Ok(())
}

/// GET /users/{user_id}/notification_preferences — the user's notification
/// preferences aggregated over every unretired communication channel.
async fn list_user_preferences(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
) -> Result<Json<UserNotificationPreferencesResponse>, ApiError> {
    let target_user_id = resolve_user_segment(&raw_user_id, auth.id)?;
    user_exists(&state.db, target_user_id).await?;
    authorize_preferences_read(&auth, target_user_id)?;

    let rows = list_user_preferences_for(&state.db, target_user_id).await?;
    Ok(Json(UserNotificationPreferencesResponse {
        notification_preferences: rows,
    }))
}

/// PUT /users/{user_id}/notification_preferences — Canvas' `update_all` body
/// shapes applied to every unretired communication channel, then the aggregate
/// preference list is returned.
async fn update_user_preferences(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(raw_user_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<UserNotificationPreferencesResponse>, ApiError> {
    let target_user_id = resolve_user_segment(&raw_user_id, auth.id)?;
    user_exists(&state.db, target_user_id).await?;
    authorize_preferences_write(&auth, target_user_id)?;

    let updates = parse_preference_updates(&body)?;
    ensure_unretired_channel(&state.db, target_user_id).await?;

    let channel_ids: Vec<(i64,)> = sqlx::query_as(
        r#"
        SELECT id FROM communication_channels
        WHERE user_id = $1 AND workflow_state <> 'retired'
        ORDER BY position ASC, id ASC
        "#,
    )
    .bind(target_user_id)
    .fetch_all(&state.db)
    .await?;

    for (channel_id,) in &channel_ids {
        for (notification, frequency) in &updates {
            upsert_channel_preference(
                &state.db,
                target_user_id,
                *channel_id,
                notification,
                frequency,
            )
            .await?;
        }
    }

    let rows = list_user_preferences_for(&state.db, target_user_id).await?;
    Ok(Json(UserNotificationPreferencesResponse {
        notification_preferences: rows,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        authorize_preferences_read, authorize_preferences_write, derive_category,
        parse_preference_updates, parse_single_frequency, resolve_user_segment, router,
        validate_new_channel, CommunicationChannelPreferencesResponse, NotificationPolicyRow,
        NotificationPreference, NotificationPreferencesResponse,
    };
    use chrono::{DateTime, Utc};
    use star_auth::AuthUser;
    use star_common::ApiError;

    #[test]
    fn user_preference_routes_build_without_conflicts() {
        // Route insertion panics in matchit on conflicting paths; building the
        // router proves the new /users/{user_id} routes coexist with
        // /users/self/notification_preferences.
        let _router = router();
    }

    #[test]
    fn user_id_segment_matches_canvas_api_find_semantics() {
        assert_eq!(resolve_user_segment("self", 9).unwrap(), 9);
        assert_eq!(resolve_user_segment("42", 9).unwrap(), 42);
        assert!(matches!(
            resolve_user_segment("bogus", 9),
            Err(ApiError::NotFound)
        ));
        assert!(matches!(
            resolve_user_segment("0", 9),
            Err(ApiError::NotFound)
        ));
        assert!(matches!(
            resolve_user_segment("-3", 9),
            Err(ApiError::NotFound)
        ));
    }

    #[test]
    fn user_preference_access_is_self_and_admin_read_but_self_only_write() {
        let student = AuthUser {
            id: 5,
            role: "student".into(),
        };
        let admin = AuthUser {
            id: 1,
            role: "admin".into(),
        };

        assert!(authorize_preferences_read(&student, 5).is_ok());
        assert!(authorize_preferences_write(&student, 5).is_ok());
        assert!(matches!(
            authorize_preferences_read(&student, 6),
            Err(ApiError::Forbidden)
        ));
        assert!(matches!(
            authorize_preferences_write(&student, 6),
            Err(ApiError::Forbidden)
        ));

        // Canvas only exposes another user's preferences to privileged
        // viewers; updates stay self-only even for admins.
        assert!(authorize_preferences_read(&admin, 6).is_ok());
        assert!(matches!(
            authorize_preferences_write(&admin, 6),
            Err(ApiError::Forbidden)
        ));
    }

    #[test]
    fn preferences_response_uses_canvas_envelope_for_list_and_update() {
        let timestamp: DateTime<Utc> = DateTime::from_timestamp(0, 0).unwrap();
        let response = NotificationPreferencesResponse {
            notification_preferences: vec![NotificationPolicyRow {
                id: 7,
                user_id: 11,
                channel: "email".into(),
                frequency: "daily".into(),
                notification_category: "announcement".into(),
                created_at: timestamp,
                updated_at: timestamp,
            }],
        };

        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json["notification_preferences"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            json["notification_preferences"][0]["notification_category"],
            "announcement"
        );
        assert_eq!(json["notification_preferences"][0]["frequency"], "daily");
    }

    #[test]
    fn new_channel_validation_matches_canvas_types() {
        assert_eq!(
            validate_new_channel(Some("student@example.edu"), Some("email"), None).unwrap(),
            ("student@example.edu".to_string(), "email".to_string())
        );
        assert_eq!(
            validate_new_channel(Some("+15551234567"), Some("sms"), None).unwrap(),
            ("+15551234567".to_string(), "sms".to_string())
        );
        assert!(matches!(
            validate_new_channel(Some("not-an-email"), Some("email"), None),
            Err(ApiError::BadRequest(_))
        ));
        assert!(matches!(
            validate_new_channel(None, Some("push"), None),
            Err(ApiError::BadRequest(_))
        ));
        assert_eq!(
            validate_new_channel(None, Some("push"), Some("device-token")).unwrap(),
            ("push".to_string(), "push".to_string())
        );
        assert!(matches!(
            validate_new_channel(Some("a@b.co"), Some("carrier_pigeon"), None),
            Err(ApiError::BadRequest(_))
        ));
    }

    #[test]
    fn channel_preference_payloads_accept_canvas_shapes() {
        let single = serde_json::json!({"notification_preferences": {"frequency": "never"}});
        assert_eq!(parse_single_frequency(&single).unwrap(), "never");
        let flat = serde_json::json!({"frequency": "daily"});
        assert_eq!(parse_single_frequency(&flat).unwrap(), "daily");

        let hash = serde_json::json!({"notification_preferences": {
            "new_announcement": {"frequency": "daily"},
            "due_date": {"frequency": "weekly"}
        }});
        let mut updates = parse_preference_updates(&hash).unwrap();
        updates.sort();
        assert_eq!(
            updates,
            vec![
                ("due_date".to_string(), "weekly".to_string()),
                ("new_announcement".to_string(), "daily".to_string()),
            ]
        );

        let pairs = serde_json::json!({
            "notification_preferences": [["new_announcement", "never"]]
        });
        assert_eq!(
            parse_preference_updates(&pairs).unwrap(),
            vec![("new_announcement".to_string(), "never".to_string())]
        );

        let invalid = serde_json::json!({"notification_preferences": {"frequency": "hourly"}});
        assert!(matches!(
            parse_single_frequency(&invalid),
            Err(ApiError::Validation(_))
        ));
    }

    #[test]
    fn channel_preference_json_uses_canvas_fields_only() {
        assert_eq!(derive_category("new_announcement"), "announcement");
        assert_eq!(derive_category("new_discussion_entry"), "discussion");
        assert_eq!(derive_category("due_date"), "due_date");
        assert_eq!(derive_category("unknown_notification"), "other");

        let response = CommunicationChannelPreferencesResponse {
            notification_preferences: vec![NotificationPreference {
                notification: "new_announcement".into(),
                category: "announcement".into(),
                frequency: "daily".into(),
            }],
        };
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(
            json["notification_preferences"][0]["notification"],
            "new_announcement"
        );
        assert_eq!(
            json["notification_preferences"][0]["category"],
            "announcement"
        );
        assert_eq!(json["notification_preferences"][0]["frequency"], "daily");
        assert!(json["notification_preferences"][0].get("id").is_none());
    }
}
