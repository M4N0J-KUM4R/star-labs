-- Migration 0005: file metadata tracking (no S3/storage upload)
CREATE TABLE IF NOT EXISTS files (
    id           BIGSERIAL PRIMARY KEY,
    course_id    BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    uploader_id  BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename     TEXT NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
    size_bytes   BIGINT NOT NULL DEFAULT 0,
    folder       TEXT NOT NULL DEFAULT 'unfiled',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_files_course ON files(course_id);
CREATE INDEX idx_files_uploader ON files(uploader_id);
