-- Migration: 0045_communication_channels
-- Canvas API v1 parity: communication channels + per-channel notification preferences.
--
-- Canvas keeps email/SMS/push channels in `communication_channels` (path/path_type,
-- workflow_state unconfirmed|active|retired) and one NotificationPolicy row per
-- (channel, notification) in `notification_policies`. The existing
-- `notification_policies` table is category-level and stays untouched; the new
-- `notification_preferences` table stores the Canvas per-channel policies.

CREATE TABLE IF NOT EXISTS communication_channels (
    id             BIGSERIAL PRIMARY KEY,
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    path           TEXT NOT NULL,
    path_type      TEXT NOT NULL DEFAULT 'email'
        CHECK (path_type IN ('email', 'sms', 'push')),
    position       INTEGER NOT NULL DEFAULT 0,
    workflow_state TEXT NOT NULL DEFAULT 'unconfirmed'
        CHECK (workflow_state IN ('unconfirmed', 'active', 'retired')),
    bounce_count   INTEGER NOT NULL DEFAULT 0,
    last_bounce_at TIMESTAMPTZ,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_communication_channels_user_id
    ON communication_channels (user_id);
CREATE INDEX IF NOT EXISTS idx_communication_channels_user_state
    ON communication_channels (user_id, workflow_state);
-- Canvas forbids duplicate unretired channels for the same user/type/address.
CREATE UNIQUE INDEX IF NOT EXISTS idx_communication_channels_unique_unretired
    ON communication_channels (user_id, lower(path), path_type)
    WHERE workflow_state <> 'retired';

CREATE TABLE IF NOT EXISTS notification_preferences (
    id                      BIGSERIAL PRIMARY KEY,
    user_id                 BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    communication_channel_id BIGINT NOT NULL
        REFERENCES communication_channels(id) ON DELETE CASCADE,
    notification            TEXT NOT NULL,
    category                TEXT NOT NULL DEFAULT 'other',
    frequency               TEXT NOT NULL DEFAULT 'immediately'
        CHECK (frequency IN ('immediately', 'daily', 'weekly', 'never')),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (communication_channel_id, notification)
);

CREATE INDEX IF NOT EXISTS idx_notification_preferences_channel
    ON notification_preferences (communication_channel_id);
CREATE INDEX IF NOT EXISTS idx_notification_preferences_user_id
    ON notification_preferences (user_id);
