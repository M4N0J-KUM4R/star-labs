-- Migration 0016: Canvas attachment parity — display_name + size field rename.
--
-- Canvas attachment fields (Canvas API v1, attachment_json in lib/api/v1/attachment.rb):
--   filename   — internal stored name (unencoded)
--   display_name — user-facing name; defaults to filename if not provided (Canvas: infer_display_name)
--   content_type — MIME type
--   size        — file size in bytes (renamed from size_bytes for Canvas parity)
--   folder      — full folder path, e.g. 'course files/subfolder' (Canvas root: 'course files')
--
-- On create, display_name defaults to filename if not provided.
-- Responses use display_name prominently (Canvas behavior).
--
-- NOTE: Actual file upload / S3 storage is out of scope.
--       POST /courses/{id}/files records metadata only.

-- Rename size_bytes -> size (Canvas field name)
ALTER TABLE files RENAME COLUMN size_bytes TO size;

-- Add display_name: user-facing filename, defaults to filename if not provided
ALTER TABLE files ADD COLUMN display_name TEXT;
UPDATE files SET display_name = filename WHERE display_name IS NULL;
ALTER TABLE files ALTER COLUMN display_name SET NOT NULL;