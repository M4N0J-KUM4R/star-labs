//! CommunicationChannel, Notification and NotificationPolicy object types
//! with their `User` fields (GraphQL parity gap completion).
//!
//! Mirrors Canvas `app/graphql/types/communication_channel_type.rb`,
//! `notification_type.rb`, `notification_policy_type.rb` and the
//! `communicationChannels` / `notificationPreferences` fields on
//! `user_type.rb`. Schema backing: migrations 0045
//! (`communication_channels`, `notification_preferences` — the
//! Canvas-shaped policy rows, keyed by channel + notification name) and
//! 0021 (`notifications` for the persisted stream).
//!
//! Canvas shape (subset, camelCase):
//! - CommunicationChannel: `id`, `path`, `pathType`, `position`,
//!   `workflowState`, `createdAt`, `updatedAt`;
//! - Notification: `id`, `subject`, `body`, `category`, `workflowState`,
//!   `createdAt` (Canvas `NotificationType` wraps the notification
//!   dispatcher's stream items; the star-db `notifications` table is the
//!   persisted equivalent);
//! - NotificationPolicy: `id`, `communicationChannelId`, `notification`,
//!   `frequency` (the 0045 preference rows carry no workflow_state —
//!   retiring a channel retires its policies via the channel filter).
//!
//! Authorization: all three are strictly self-or-admin — Canvas gates
//! `communicationChannels` on `self` (the user object is only reachable
//! for self or admins through `user(id)` anyway, but the resolvers also
//! enforce it) and notification data is private by definition. Anyone
//! else resolves null / empty lists, never an error.

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, GraphQLContext};

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct CommunicationChannelRow {
    id: i64,
    path: String,
    path_type: String,
    position: i32,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct NotificationRow {
    id: i64,
    subject: String,
    body: String,
    category: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct NotificationPolicyRow {
    id: i64,
    communication_channel_id: i64,
    notification: String,
    frequency: String,
    workflow_state: String,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `CommunicationChannel`
/// (`app/graphql/types/communication_channel_type.rb`, subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct CommunicationChannel {
    pub id: ID,
    /// Canvas `path` — the address (email / sms number).
    pub path: String,
    /// Canvas `pathType` — `email` | `sms` | `push`.
    pub path_type: String,
    pub position: i32,
    /// Canvas `workflowState` — `unconfirmed` | `active` | `retired`.
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Canvas GraphQL `Notification`
/// (`app/graphql/types/notification_type.rb`, subset over the persisted
/// notifications table).
#[derive(Debug, Clone, SimpleObject)]
pub struct Notification {
    pub id: ID,
    pub subject: String,
    pub body: String,
    pub category: String,
    /// `unread` | `read` | `deleted` (Canvas workflow states).
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
}

/// Canvas GraphQL `NotificationPolicy`
/// (`app/graphql/types/notification_policy_type.rb`, subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct NotificationPolicy {
    pub id: ID,
    pub communication_channel_id: ID,
    /// The Canvas notification name this policy tunes (e.g.
    /// `new_announcement`); star-db stores it on the row.
    pub notification: String,
    /// `immediately` | `daily` | `weekly` | `never` (Canvas frequencies).
    pub frequency: String,
}

// ---------------------------------------------------------------------------
// Fetchers (all self-or-admin at the call site)
// ---------------------------------------------------------------------------

/// The user's non-retired communication channels, ordered by position
/// (Canvas `user.communication_channels` ordering).
pub(crate) async fn fetch_communication_channels(
    pool: &PgPool,
    user_id: i64,
) -> Result<Vec<CommunicationChannel>, async_graphql::Error> {
    let rows: Vec<CommunicationChannelRow> = sqlx::query_as(
        r#"
        SELECT id, path, path_type, position, workflow_state, created_at, updated_at
        FROM communication_channels
        WHERE user_id = $1 AND workflow_state <> 'retired'
        ORDER BY position, id
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|r| CommunicationChannel {
            id: ID(r.id.to_string()),
            path: r.path,
            path_type: r.path_type,
            position: r.position,
            workflow_state: r.workflow_state,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// The user's non-deleted notifications, newest first (Canvas stream
/// ordering by created_at desc).
pub(crate) async fn fetch_notifications(
    pool: &PgPool,
    user_id: i64,
) -> Result<Vec<Notification>, async_graphql::Error> {
    let rows: Vec<NotificationRow> = sqlx::query_as(
        r#"
        SELECT id, subject, body, category, workflow_state, created_at
        FROM notifications
        WHERE user_id = $1 AND workflow_state <> 'deleted'
        ORDER BY created_at DESC, id DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|r| Notification {
            id: ID(r.id.to_string()),
            subject: r.subject,
            body: r.body,
            category: r.category,
            workflow_state: r.workflow_state,
            created_at: r.created_at,
        })
        .collect())
}

/// The user's notification policies on non-retired channels (Canvas
/// `user.notification_policies`; 0045 preference rows).
pub(crate) async fn fetch_notification_policies(
    pool: &PgPool,
    user_id: i64,
) -> Result<Vec<NotificationPolicy>, async_graphql::Error> {
    let rows: Vec<NotificationPolicyRow> = sqlx::query_as(
        r#"
        SELECT p.id, p.communication_channel_id, p.notification, p.frequency
        FROM notification_preferences p
        JOIN communication_channels c ON c.id = p.communication_channel_id
        WHERE p.user_id = $1 AND c.workflow_state <> 'retired'
        ORDER BY p.id
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|r| NotificationPolicy {
            id: ID(r.id.to_string()),
            communication_channel_id: ID(r.communication_channel_id.to_string()),
            notification: r.notification,
            frequency: r.frequency,
        })
        .collect())
}

/// `true` when `caller` may read `target`'s private channels,
/// notifications and policies: the user themself or a global admin
/// (Canvas `user_type.rb` gates these fields on self / admins).
pub(crate) fn is_self_or_admin(caller: &GraphQLContext, target: i64) -> bool {
    caller.auth_user.id == target || caller.is_admin()
}
