-- Migration 0053: Canvas content_exports parity (content_exports_api_controller).
--
-- Canvas ContentExport belongs_to context (Course/Group/User) and to the user
-- who requested it, and carries export_type, workflow_state and progress.
-- This workspace only exposes the course-scoped API:
-- - GET  /api/v1/courses/{courseId}/content_exports
-- - POST /api/v1/courses/{courseId}/content_exports  { export_type }
-- - GET  /api/v1/courses/{courseId}/content_exports/{id}
-- - GET  /api/v1/content_exports/{id}
--
-- workflow_state mirrors Canvas: created | exporting | exported | failed | deleted.
CREATE TABLE IF NOT EXISTS content_exports (
    id             BIGSERIAL PRIMARY KEY,
    course_id      BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    user_id        BIGINT REFERENCES users(id) ON DELETE SET NULL,
    export_type    TEXT    NOT NULL DEFAULT 'common_cartridge',
    workflow_state TEXT    NOT NULL DEFAULT 'created',
    progress       INTEGER NOT NULL DEFAULT 0,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Course exports are listed newest-first per course.
CREATE INDEX IF NOT EXISTS idx_content_exports_course_id
    ON content_exports(course_id);

-- content_exports_visible_to filters non-admin rows by user_id.
CREATE INDEX IF NOT EXISTS idx_content_exports_user_id
    ON content_exports(user_id);

-- index/create look up the active export (workflow_state <> 'deleted').
CREATE INDEX IF NOT EXISTS idx_content_exports_course_state
    ON content_exports(course_id, workflow_state);
