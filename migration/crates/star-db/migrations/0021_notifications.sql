-- Migration: 0021_notifications
-- Notification feed + notification preferences (Canvas API v1 parity)

-- notifications: per-user notification items in the feed
CREATE TABLE notifications (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL,
    subject TEXT NOT NULL,
    body TEXT NOT NULL,
    category TEXT NOT NULL DEFAULT 'other',
    workflow_state TEXT NOT NULL DEFAULT 'unread'
        CHECK (workflow_state IN ('unread', 'read', 'deleted')),
    link TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_notifications_user_id ON notifications (user_id);
CREATE INDEX idx_notifications_user_created ON notifications (user_id, created_at DESC);
CREATE INDEX idx_notifications_workflow_state ON notifications (workflow_state);

-- notification_policies: per-user notification channel preferences
CREATE TABLE notification_policies (
    id BIGSERIAL PRIMARY KEY,
    user_id BIGINT NOT NULL,
    channel TEXT NOT NULL DEFAULT 'email',
    frequency TEXT NOT NULL DEFAULT 'immediately'
        CHECK (frequency IN ('immediately', 'daily', 'weekly', 'never')),
    notification_category TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (user_id, notification_category)
);

CREATE INDEX idx_notification_policies_user_id ON notification_policies (user_id);
