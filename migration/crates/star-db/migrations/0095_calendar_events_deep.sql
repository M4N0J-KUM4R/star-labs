-- Migration 0095: calendar events deep parity.
--
-- Stores the user-level calendar preferences served by Canvas
-- CalendarEventsApiController:
--   - selected_calendar_contexts   (POST /calendar_events/save_selected_contexts)
--   - enabled_account_calendars    (POST /calendar_events/save_enabled_account_calendars)
--   - account_calendar_events_seen (mark_feature_as_seen)
-- These feed GET /calendar_events/visible_contexts (`selected` flags).

CREATE TABLE IF NOT EXISTS calendar_user_preferences (
    id                        BIGSERIAL PRIMARY KEY,
    user_id                   BIGINT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    selected_contexts         TEXT[] NOT NULL DEFAULT '{}',
    enabled_account_calendars TEXT[] NOT NULL DEFAULT '{}',
    account_calendars_seen    BOOLEAN NOT NULL DEFAULT false,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_calendar_user_preferences_user
    ON calendar_user_preferences (user_id);
