-- Migration 0061: File/folder tree extras (Canvas folders_and_files + quota).
--
-- API scope (star-files):
-- - GET /courses/{courseId}/folders_and_files   (Canvas list_all_folders_and_files)
-- - GET /folders/{id}/all                       (Canvas list_folders_and_files)
-- - GET /courses/{courseId}/files/quota         (Canvas files#api_quota)
-- - GET /files/quota?course_id=                 (shorthand for the above)
--
-- Canvas attachment_json (lib/api/v1/attachment.rb) exposes hidden/locked/
-- lock_at/unlock_at/modified_at/mime_class/category/uuid/visibility_level, so
-- the metadata-only files table gains those columns. No binary storage.
--
-- Canvas Attachment.get_quota uses context.storage_quota (courses.storage_quota)
-- with a 50 MB default and a 512-byte per-file minimum.

-- Canvas courses.storage_quota (NULL -> Attachment::CONTEXT_DEFAULT_QUOTA).
ALTER TABLE courses ADD COLUMN IF NOT EXISTS storage_quota BIGINT;

-- Canvas attachments columns used by attachment_json.
ALTER TABLE files ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'processed';
ALTER TABLE files ADD COLUMN IF NOT EXISTS hidden BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE files ADD COLUMN IF NOT EXISTS locked BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE files ADD COLUMN IF NOT EXISTS lock_at TIMESTAMPTZ;
ALTER TABLE files ADD COLUMN IF NOT EXISTS unlock_at TIMESTAMPTZ;
ALTER TABLE files ADD COLUMN IF NOT EXISTS modified_at TIMESTAMPTZ;
ALTER TABLE files ADD COLUMN IF NOT EXISTS category TEXT;
ALTER TABLE files ADD COLUMN IF NOT EXISTS visibility_level TEXT;
ALTER TABLE files ADD COLUMN IF NOT EXISTS position INTEGER NOT NULL DEFAULT 1;

-- Canvas attachments.uuid (download verifier). md5 avoids requiring pgcrypto.
ALTER TABLE files ADD COLUMN IF NOT EXISTS uuid TEXT;
UPDATE files
SET uuid = md5(random()::text || clock_timestamp()::text || id::text)
WHERE uuid IS NULL;
ALTER TABLE files ALTER COLUMN uuid SET DEFAULT md5(random()::text || clock_timestamp()::text);

-- Listing indexes: folders_and_files sorts by name/display_name per context,
-- plus the folder tree walk (parent lookup) used by /folders/{id}/all.
CREATE INDEX IF NOT EXISTS idx_files_course_display_name
    ON files(course_id, display_name);
CREATE INDEX IF NOT EXISTS idx_files_folder_display_name
    ON files(folder_id, display_name);
CREATE INDEX IF NOT EXISTS idx_folder_tree_parent_name
    ON folders(parent_folder_id, name);
CREATE INDEX IF NOT EXISTS idx_folder_tree_context_parent
    ON folders(context_type, context_id, parent_folder_id);
