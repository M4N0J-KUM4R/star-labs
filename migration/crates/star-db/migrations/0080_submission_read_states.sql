-- Migration 0080: submission read states + anonymous submissions parity.
--
-- API scope:
-- - PUT /courses/{courseId}/submissions/{id}/clear_unread
--   (Canvas submissions_api_controller#submissions_clear_unread)
-- - GET /courses/{courseId}/assignments/{assignmentId}/gradeable_students
--   (Canvas submissions_api_controller#gradeable_students)
-- - GET/PUT /courses/{courseId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}
--   (Canvas submissions_api_controller#show_anonymous / #update_anonymous)
--
-- Canvas stores the anonymous id on the submission row (Submission#anonymous_id)
-- and an assignment-level anonymous id / anonymous_grading flag on assignments.
-- star-lms had neither, so both are added here. The per-(submission, user)
-- read marker table (submission_reads) already exists from 0060; it only
-- needed the read timestamp and the source marker used by clear_unread.

ALTER TABLE submissions ADD COLUMN IF NOT EXISTS anonymous_id TEXT;

CREATE INDEX IF NOT EXISTS idx_submissions_anonymous_id
    ON submissions(anonymous_id);

ALTER TABLE assignments ADD COLUMN IF NOT EXISTS anonymous_id TEXT;

ALTER TABLE assignments ADD COLUMN IF NOT EXISTS anonymous_grading BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE submission_reads ADD COLUMN IF NOT EXISTS read_at TIMESTAMPTZ NOT NULL DEFAULT now();

ALTER TABLE submission_reads ADD COLUMN IF NOT EXISTS source TEXT NOT NULL DEFAULT 'bulk_mark_read';

CREATE INDEX IF NOT EXISTS idx_submission_reads_read_at
    ON submission_reads(read_at);
