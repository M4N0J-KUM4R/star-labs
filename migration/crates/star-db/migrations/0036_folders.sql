-- Migration 0036: Canvas folders parity (app/models/folder.rb).
--
-- Canvas folders are a tree scoped to a context (course/user/group/account).
-- star-lms only models course file metadata today, so folders here use the
-- same polymorphic context columns as Canvas (context_type/context_id) but
-- the API only serves Course contexts.
--
-- Columns mirror Canvas folders:
--   name / full_name      display name and full path (root name + ancestors,
--                         e.g. 'course files/lectures/week 1')
--   parent_folder_id      NULL for the context root folder
--   workflow_state        'visible' | 'hidden' | 'deleted' (soft delete)
--   position              sibling sort order (acts_as_list in Canvas)
--   locked/lock_at/unlock_at  file locking (folder_json locked/locked_for_user)
--   for_submissions       read-only submissions folder marker
--
-- files.folder_id links attachments to folders; the existing files.folder text
-- path is kept for backwards compatibility.

CREATE TABLE IF NOT EXISTS folders (
    id               BIGSERIAL PRIMARY KEY,
    name             TEXT NOT NULL,
    full_name        TEXT NOT NULL DEFAULT '',
    context_type     TEXT NOT NULL DEFAULT 'Course',
    context_id       BIGINT NOT NULL,
    parent_folder_id BIGINT REFERENCES folders(id) ON DELETE CASCADE,
    workflow_state   TEXT NOT NULL DEFAULT 'visible',
    position         INTEGER NOT NULL DEFAULT 1,
    locked           BOOLEAN NOT NULL DEFAULT FALSE,
    lock_at          TIMESTAMPTZ,
    unlock_at        TIMESTAMPTZ,
    for_submissions  BOOLEAN NOT NULL DEFAULT FALSE,
    deleted_at       TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_folders_context ON folders(context_type, context_id);
CREATE INDEX IF NOT EXISTS idx_folders_parent ON folders(parent_folder_id);
CREATE INDEX IF NOT EXISTS idx_folders_context_full_name
    ON folders(context_type, context_id, full_name);

-- One active root folder per context (Canvas root_folders).
CREATE UNIQUE INDEX IF NOT EXISTS idx_folders_context_root
    ON folders(context_type, context_id)
    WHERE parent_folder_id IS NULL;

ALTER TABLE files
    ADD COLUMN IF NOT EXISTS folder_id BIGINT REFERENCES folders(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_files_folder_id ON files(folder_id);
