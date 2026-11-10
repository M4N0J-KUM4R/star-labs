-- Migration 0051: Canvas-style user profiles.
-- Mirrors Canvas UserProfile (title/bio), plus profile-level copies of the
-- fields Canvas exposes on the Profile model (avatar, time zone, locale).
-- Identity fields (name, short_name, sortable_name) stay on users.

CREATE TABLE IF NOT EXISTS user_profiles (
    id            BIGSERIAL PRIMARY KEY,
    user_id       BIGINT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
    title         TEXT,
    bio           TEXT,
    primary_email TEXT,
    avatar_url    TEXT,
    time_zone     TEXT NOT NULL DEFAULT 'UTC',
    locale        TEXT NOT NULL DEFAULT 'en',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- user_id is always the lookup key; the UNIQUE constraint above already
-- creates the supporting index (also used by ON DELETE CASCADE).
