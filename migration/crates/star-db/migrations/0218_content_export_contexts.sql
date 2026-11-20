-- Migration 0218: content export contexts, new-quizzes settings and the
-- epub / web-zip export table.
--
-- Canvas `ContentExport` belongs to a polymorphic context:
--
--   belongs_to :context, polymorphic: [:course, :account, :group, :user]
--
-- and `ContentExportsApiController` serves the same actions for a course, a
-- group and a user.  The port stored the context in `course_id` alone, so the
-- group and user variants could not be served.  The two columns below carry
-- the context for every row; the existing `course_id` column stays as it is,
-- so the course-scoped queries and the rows that already exist keep working.
--
-- The backfill gives every course-scoped row its context.

ALTER TABLE content_exports
    ADD COLUMN IF NOT EXISTS context_type TEXT;

ALTER TABLE content_exports
    ADD COLUMN IF NOT EXISTS context_id BIGINT;

UPDATE content_exports
   SET context_type = 'Course',
       context_id = course_id
 WHERE context_type IS NULL
   AND course_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_content_exports_context
    ON content_exports (context_type, context_id);

-- Canvas keeps the new-quizzes export settings in `export.settings`; the port
-- stores the two fields the API renders as columns.
ALTER TABLE content_exports
    ADD COLUMN IF NOT EXISTS new_quizzes_export_state TEXT;

ALTER TABLE content_exports
    ADD COLUMN IF NOT EXISTS new_quizzes_export_url TEXT;

-- Canvas `EpubExport` and `WebZipExport` share one table (single table
-- inheritance: `WebZipExport < EpubExport`), so a web zip export is a row of
-- `epub_exports` with `type = 'WebZipExport'` and a plain ePub export has
-- `type IS NULL`.
CREATE TABLE IF NOT EXISTS epub_exports (
    id                BIGSERIAL PRIMARY KEY,
    content_export_id BIGINT REFERENCES content_exports(id) ON DELETE SET NULL,
    course_id         BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    user_id           BIGINT REFERENCES users(id) ON DELETE SET NULL,
    workflow_state    TEXT NOT NULL DEFAULT 'created',
    type              TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_epub_exports_course ON epub_exports (course_id);

CREATE INDEX IF NOT EXISTS idx_epub_exports_type ON epub_exports (type);
