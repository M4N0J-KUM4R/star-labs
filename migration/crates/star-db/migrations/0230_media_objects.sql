-- Migration 0230: the media object columns, its tracks and the group and user
-- media files.
--
-- Canvas `MediaObject` is the entry of a media item in a course, a group or a
-- user; `MediaTrack` is a caption or a transcript of it. The group and user
-- media files are the attachment rows of the scope the object belongs to.

ALTER TABLE media_objects
    ADD COLUMN IF NOT EXISTS context_type TEXT,
    ADD COLUMN IF NOT EXISTS context_id BIGINT,
    ADD COLUMN IF NOT EXISTS old_media_id TEXT,
    ADD COLUMN IF NOT EXISTS attachment_id BIGINT,
    ADD COLUMN IF NOT EXISTS viewer_restrictions JSONB NOT NULL DEFAULT '{}'::jsonb,
    ADD COLUMN IF NOT EXISTS auto_caption_status TEXT;

CREATE INDEX IF NOT EXISTS idx_media_objects_context
    ON media_objects (context_type, context_id);

CREATE TABLE IF NOT EXISTS media_tracks (
    id               BIGSERIAL PRIMARY KEY,
    media_object_id  BIGINT NOT NULL REFERENCES media_objects(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL,
    locale           TEXT NOT NULL,
    external_id      TEXT,
    workflow_state   TEXT NOT NULL DEFAULT 'active',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_media_tracks_object ON media_tracks (media_object_id);

CREATE TABLE IF NOT EXISTS group_files (
    id             BIGSERIAL PRIMARY KEY,
    group_id       BIGINT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    uploader_id    BIGINT REFERENCES users(id) ON DELETE SET NULL,
    filename       TEXT NOT NULL,
    display_name   TEXT NOT NULL,
    content_type   TEXT,
    size           BIGINT NOT NULL DEFAULT 0,
    folder         TEXT,
    uuid           TEXT NOT NULL,
    media_entry_id TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS user_files (
    id             BIGSERIAL PRIMARY KEY,
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename       TEXT NOT NULL,
    display_name   TEXT NOT NULL,
    content_type   TEXT,
    size           BIGINT NOT NULL DEFAULT 0,
    folder         TEXT,
    uuid           TEXT NOT NULL,
    media_entry_id TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_group_files_group ON group_files (group_id);
CREATE INDEX IF NOT EXISTS idx_user_files_user ON user_files (user_id);
