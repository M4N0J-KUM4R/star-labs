-- Migration 0056: Canvas user settings + user-context file attachments.
--
-- user_settings mirrors the boolean preferences exposed by
-- UsersController#settings (BOOLEAN_PREFS). Canvas stores these as
-- UserPreference rows and treats a missing preference as false, so every
-- column defaults to false and a missing row is equivalent to an all-false
-- row.
--
-- user_files mirrors POST /api/v1/users/:user_id/files (attachment
-- preflight, metadata only — no binary upload or S3 storage). Attachments in
-- Canvas are polymorphic; the existing files table is course-scoped, so
-- user-context attachments get their own table instead of a nullable
-- course_id on files. folder_id reuses the polymorphic folders table with
-- context_type 'User'.

CREATE TABLE IF NOT EXISTS user_settings (
    user_id                            BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    manual_mark_as_read                BOOLEAN NOT NULL DEFAULT FALSE,
    collapse_global_nav                BOOLEAN NOT NULL DEFAULT FALSE,
    collapse_course_nav                BOOLEAN NOT NULL DEFAULT FALSE,
    hide_dashcard_color_overlays       BOOLEAN NOT NULL DEFAULT FALSE,
    release_notes_badge_disabled       BOOLEAN NOT NULL DEFAULT FALSE,
    comment_library_suggestions_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    elementary_dashboard_disabled      BOOLEAN NOT NULL DEFAULT FALSE,
    default_to_block_editor            BOOLEAN NOT NULL DEFAULT FALSE,
    widget_dashboard_user_preference   BOOLEAN NOT NULL DEFAULT FALSE,
    widget_dashboard_dark_mode         BOOLEAN NOT NULL DEFAULT FALSE,
    created_at                         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The primary key on user_id already provides the lookup index.

CREATE TABLE IF NOT EXISTS user_files (
    id           BIGSERIAL PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename     TEXT NOT NULL,
    display_name TEXT NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
    size         BIGINT NOT NULL DEFAULT 0,
    folder       TEXT NOT NULL DEFAULT 'unfiled',
    folder_id    BIGINT REFERENCES folders(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_user_files_user ON user_files(user_id);
CREATE INDEX IF NOT EXISTS idx_user_files_folder ON user_files(folder_id);
