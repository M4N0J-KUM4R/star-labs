-- Migration: 0074_notification_preferences
-- Canvas API v1 parity: user-level notification preferences.
--
-- GET/PUT /users/{user_id}/notification_preferences aggregate the existing
-- `notification_preferences` policies across a user's unretired
-- `communication_channels`. The composite index backs the per-user
-- `DISTINCT ON (notification)` read used by both endpoints.
--
-- No new tables: Canvas' NotificationPolicy data already lives in
-- `notification_preferences` (migration 0045); the legacy category-level
-- `notification_policies` table keeps serving /users/self/notification_preferences.

CREATE INDEX IF NOT EXISTS idx_notification_preferences_user_notification
    ON notification_preferences (user_id, notification);
