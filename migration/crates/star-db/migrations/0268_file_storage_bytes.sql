-- File bytes: the columns Canvas' `attachments` table carries and the port's
-- `files` table still owes.
--
-- Canvas (`db/structure.sql`, `app/models/attachment.rb`):
--   * `md5`        — the MD5 hex digest of the bytes (attachment.rb:1097-1109)
--   * `file_state` — 'available' once the bytes are in the store
--   * `encoding`   — the charset Canvas reports on download (attachment.rb:1045)
--   * `word_count` — written back through the attachment JWT
--     (app/controllers/files_controller.rb:1625-1640)
--
-- `storage_key` is the port's own column: Canvas derives the key of an
-- attachment's bytes from the attachment id and the file name
-- (`gems/attachment_fu/lib/attachment_fu/backends/file_system_backend.rb:38-58`,
-- `.../s3_backend.rb:206-229`); the port records the key it used so a read
-- never has to guess where the bytes are.

ALTER TABLE files ADD COLUMN IF NOT EXISTS storage_key TEXT;
ALTER TABLE files ADD COLUMN IF NOT EXISTS md5 TEXT;
ALTER TABLE files ADD COLUMN IF NOT EXISTS file_state TEXT NOT NULL DEFAULT 'available';
ALTER TABLE files ADD COLUMN IF NOT EXISTS encoding TEXT;
ALTER TABLE files ADD COLUMN IF NOT EXISTS word_count INTEGER;

CREATE INDEX IF NOT EXISTS idx_files_storage_key ON files (storage_key);
