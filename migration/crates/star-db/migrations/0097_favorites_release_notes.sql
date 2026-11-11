-- Migration 0097: Canvas favorites and release notes.
--
-- favorites
--   Canvas `Favorite` (app/models/favorite.rb, favorites_controller.rb):
--   the set of contexts (currently only Courses) a user has starred.
--   A favorite whose course no longer exists is still allowed upstream, so
--   context_id is intentionally not a foreign key.
-- release_notes
--   Canvas `ReleaseNote` (app/models/release_note.rb) is DynamoDB-backed; the
--   attributes it exposes are id, show_ats (env -> time), target_roles,
--   published and the per-locale langs text. The environment -> time map is
--   modelled as three columns so rows stay plain SQL.
-- users.last_seen_release_note
--   Canvas keeps the last release note time the user acknowledged on the
--   User row; `latest`/`unread_count` compare against it.
--
-- Routes served by star-dashboard:
--   GET/POST/DELETE /users/self/favorites/courses[/{id}]
--   GET/POST /release_notes
--   GET /release_notes/latest, GET /release_notes/unread_count
--   PUT/DELETE /release_notes/{id}

CREATE TABLE IF NOT EXISTS favorites (
    id           BIGSERIAL PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    context_type TEXT NOT NULL DEFAULT 'Course', -- Course
    context_id   BIGINT NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (user_id, context_type, context_id)
);

CREATE INDEX IF NOT EXISTS idx_favorites_user_context
    ON favorites (user_id, context_type);

CREATE TABLE IF NOT EXISTS release_notes (
    id                 BIGSERIAL PRIMARY KEY,
    title              TEXT NOT NULL DEFAULT '',
    description        TEXT,
    url                TEXT,
    published          BOOLEAN NOT NULL DEFAULT FALSE,
    target_roles       TEXT[] NOT NULL DEFAULT ARRAY['user']::text[],
    show_at_production timestamptz,
    show_at_beta       timestamptz,
    show_at_test       timestamptz,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_release_notes_published_created
    ON release_notes (published, created_at DESC);

ALTER TABLE users ADD COLUMN IF NOT EXISTS last_seen_release_note timestamptz;
