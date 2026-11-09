//! star-notifications — notification feed + notification preferences for star-lms.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, put},
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

#[cfg(test)]
mod tests {
    use super::{NotificationPolicyRow, NotificationPreferencesResponse};
    use chrono::{DateTime, Utc};

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
}
