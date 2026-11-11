-- Migration 0082: user account management.
-- Custom dashboard/calendar colors per user+context (Canvas UserColor /
-- users_controller#custom_colors) and the `last_logged_out` marker used by
-- users_controller#terminate_sessions. Dashboard card ordering already lives
-- in `dashboard_positions` (migration 0073).

CREATE TABLE IF NOT EXISTS user_colors (
    id         BIGSERIAL PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    context    TEXT NOT NULL,
    color      TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, context)
);

ALTER TABLE users ADD COLUMN IF NOT EXISTS last_logged_out TIMESTAMPTZ;
