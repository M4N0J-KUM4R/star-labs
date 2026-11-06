-- Migration 0026: Canvas submission_comments parity
-- Canvas: SubmissionComment belongs_to submission, author (User),
-- carries comment TEXT, workflow_state ('active'), timestamps.
--
-- API scope:
-- - POST   /submissions/{submissionId}/comments  {comment} → 201
-- - GET    /submissions/{submissionId}/comments  → [comments], ordered by created_at
-- - DELETE /submissions/{submissionId}/comments/{commentId} → 204, soft-delete
CREATE TABLE IF NOT EXISTS submission_comments (
    id              BIGSERIAL PRIMARY KEY,
    submission_id   BIGINT NOT NULL REFERENCES submissions(id),
    author_id       BIGINT NOT NULL,
    comment         TEXT   NOT NULL,
    workflow_state  TEXT   NOT NULL DEFAULT 'active',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Index for fast lookup by submission + active rows
CREATE INDEX IF NOT EXISTS idx_submission_comments_submission_id
    ON submission_comments(submission_id)
    WHERE workflow_state = 'active';
