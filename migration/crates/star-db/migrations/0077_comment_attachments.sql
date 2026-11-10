-- Migration 0077: Submission comment attachments + comment edit tracking.
--
-- Canvas: SubmissionComment (app/models/submission_comment.rb)
--   - carries attachment file ids in `attachment_ids` (comment attachments are
--     uploaded first, then referenced when the comment is written)
--   - `edited_at` is set when an existing comment's text changes
--     (SubmissionComment#set_edited_at)
--
-- Canvas API (SubmissionCommentsApiController):
--   POST   /courses/:course_id/assignments/:assignment_id/submissions/:user_id/comments/files
--          create_file — attachment metadata for a submission comment. star-lms
--          records metadata only: no binary upload / storage handling.
--   GET    .../comments/:id  — single comment (star-lms detail route)
--   PUT    .../comments/:id  — edit a comment's text (author only)
--   DELETE .../comments/:id  — soft-delete a comment
--
-- submission_comment_attachments starts unattached (comment_id NULL) right
-- after the metadata upload; it is linked to a comment either by passing
-- `comment_id` on upload or `file_ids` when creating a comment.

ALTER TABLE submission_comments
    ADD COLUMN IF NOT EXISTS edited_at TIMESTAMPTZ;

CREATE TABLE IF NOT EXISTS submission_comment_attachments (
    id           BIGSERIAL PRIMARY KEY,
    submission_id BIGINT NOT NULL REFERENCES submissions(id) ON DELETE CASCADE,
    comment_id   BIGINT REFERENCES submission_comments(id) ON DELETE CASCADE,
    uploader_id  BIGINT NOT NULL,
    filename     TEXT   NOT NULL,
    display_name TEXT   NOT NULL,
    content_type TEXT   NOT NULL DEFAULT 'application/octet-stream',
    size         BIGINT NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- List attachments per submission (including not-yet-linked uploads) ...
CREATE INDEX IF NOT EXISTS idx_submission_comment_attachments_submission
    ON submission_comment_attachments(submission_id);

-- ... and per comment for comment payloads.
CREATE INDEX IF NOT EXISTS idx_submission_comment_attachments_comment
    ON submission_comment_attachments(comment_id);
