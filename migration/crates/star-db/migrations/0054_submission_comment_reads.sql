-- Migration 0054: Submission comment read state (Canvas parity)
-- Canvas tracks per-user comment read state via ViewedSubmissionComment rows;
-- star-lms keeps one marker per (submission, user): the highest active comment
-- id the user has seen, plus when they last marked the thread read.
--
-- API scope:
-- - PUT /submissions/{submissionId}/comments/read        → 204
-- - GET /submissions/{submissionId}/comments/unread_count → { unread_count }
CREATE TABLE IF NOT EXISTS submission_comment_reads (
    id                    BIGSERIAL PRIMARY KEY,
    submission_id         BIGINT NOT NULL REFERENCES submissions(id),
    user_id               BIGINT NOT NULL,
    last_read_comment_id  BIGINT,
    read_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One read marker per user per submission; also serves lookups by submission.
CREATE UNIQUE INDEX IF NOT EXISTS idx_submission_comment_reads_submission_user
    ON submission_comment_reads(submission_id, user_id);

CREATE INDEX IF NOT EXISTS idx_submission_comment_reads_user_id
    ON submission_comment_reads(user_id);
