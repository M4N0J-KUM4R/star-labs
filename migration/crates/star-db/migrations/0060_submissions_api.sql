-- Migration 0060: Canvas submissions API parity.
--
-- API scope (app/controllers/submissions_api_controller.rb):
-- - GET  /courses/{courseId}/students/submissions            (for_students)
-- - GET  /courses/{courseId}/assignments/{assignmentId}/submissions/{userId} (show)
-- - PUT  /courses/{courseId}/assignments/{assignmentId}/submissions/{userId} (update)
-- - POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/files
-- - PUT  /courses/{courseId}/submissions/bulk_mark_read      (mark_bulk_submissions_as_read)
--
-- The base submissions table (0001) only models body/score. Canvas'
-- submission_json exposes attempt, submission_type, workflow_state, graded_at,
-- grader_id, excused, late_policy_status and points_deducted, so those are
-- added here without disturbing existing columns.

ALTER TABLE submissions ADD COLUMN IF NOT EXISTS submission_type TEXT;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS attempt INTEGER NOT NULL DEFAULT 1;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'submitted';
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS graded_at TIMESTAMPTZ;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS grader_id BIGINT;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS excused BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS late_policy_status TEXT;
ALTER TABLE submissions ADD COLUMN IF NOT EXISTS points_deducted NUMERIC(10,2);

-- Backfill rows created before this migration:
-- text bodies were online_text_entry submissions, graded scores imply graded_at.
UPDATE submissions
SET submission_type = 'online_text_entry'
WHERE submission_type IS NULL AND body IS NOT NULL;

UPDATE submissions
SET workflow_state = 'graded'
WHERE workflow_state = 'submitted' AND score IS NOT NULL;

UPDATE submissions
SET graded_at = updated_at
WHERE graded_at IS NULL AND score IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_submissions_workflow_state
    ON submissions(workflow_state);

-- Per-user submission read markers. Canvas stores these as
-- ContentParticipation/StreamItem rows for "Submission" assets; star-lms keeps
-- one marker per (submission, user), written by bulk_mark_read.
CREATE TABLE IF NOT EXISTS submission_reads (
    id            BIGSERIAL PRIMARY KEY,
    submission_id BIGINT NOT NULL REFERENCES submissions(id) ON DELETE CASCADE,
    user_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    read_state    TEXT NOT NULL DEFAULT 'read',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_submission_reads_submission_user
    ON submission_reads(submission_id, user_id);

CREATE INDEX IF NOT EXISTS idx_submission_reads_user_id
    ON submission_reads(user_id);

-- Metadata-only submission attachments (Canvas Attachment rows whose context
-- is a Submission). No binary upload handling: the API records filename,
-- content type and size, mirroring files/attachments elsewhere in star-lms.
CREATE TABLE IF NOT EXISTS submission_files (
    id            BIGSERIAL PRIMARY KEY,
    submission_id BIGINT NOT NULL REFERENCES submissions(id) ON DELETE CASCADE,
    user_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    filename      TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    content_type  TEXT NOT NULL DEFAULT 'application/octet-stream',
    size          BIGINT NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_submission_files_submission
    ON submission_files(submission_id);

CREATE INDEX IF NOT EXISTS idx_submission_files_user
    ON submission_files(user_id);
