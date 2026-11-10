-- Migration 0035: account notifications — Canvas global announcements.
-- Mirrors Canvas AccountNotification / AccountNotificationRole models. The
-- account-level API (Api::V1::AccountNotifications) serializes
-- id, subject, start_at, end_at, icon, message plus the role names/ids of the
-- notification's targeting roles.

CREATE TABLE IF NOT EXISTS account_notifications (
    id              BIGSERIAL PRIMARY KEY,
    account_id      BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    user_id         BIGINT REFERENCES users(id) ON DELETE SET NULL,
    subject         TEXT NOT NULL,
    message         TEXT NOT NULL,
    start_at        TIMESTAMPTZ NOT NULL,
    end_at          TIMESTAMPTZ NOT NULL,
    icon            TEXT,
    workflow_state  TEXT NOT NULL DEFAULT 'active',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS account_notification_roles (
    id                      BIGSERIAL PRIMARY KEY,
    account_notification_id BIGINT NOT NULL REFERENCES account_notifications(id) ON DELETE CASCADE,
    role_id                 BIGINT,
    role_name               TEXT NOT NULL,
    workflow_state          TEXT NOT NULL DEFAULT 'active',
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_account_notifications_account
    ON account_notifications(account_id);
CREATE INDEX IF NOT EXISTS idx_account_notifications_window
    ON account_notifications(account_id, start_at DESC);
CREATE INDEX IF NOT EXISTS idx_account_notification_roles_notification
    ON account_notification_roles(account_notification_id);
