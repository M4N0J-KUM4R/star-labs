-- Migration 0119: assignment_annotations table.
--
-- Backs the read-state routes added alongside migration 0122
-- (0122_rubric_annotation_read_states.sql), whose
-- `document_annotation_reads.document_annotation_id` foreign key references
-- `assignment_annotations(id)`. 0122 was pushed before this table existed,
-- so fresh databases abort at migration 122 with 42P01 `relation
-- "assignment_annotations" does not exist`; 0122 is already applied on CI
-- databases and is checksummed, so this migration adds the missing table
-- with a lower number instead of editing 0122.
--
-- Canvas ground truth: there is no `assignment_annotations` table in Canvas.
-- Annotation read state is a per-user preference
-- (app/models/user.rb:2096-2107, `unread_submission_annotations?` /
-- `mark_submission_annotations_read!` backed by the
-- `unread_submission_annotations` preference keyed on submission global id),
-- and the only annotation-shaped model is `CanvadocsAnnotationContext`
-- (app/models/canvadocs_annotation_context.rb; db/migrate
-- /20101210192618_init_canvas_db.rb:1357-1372: attachment_id, submission_id,
-- root_account_id, launch_id, submission_attempt). The star-lms
-- read-state handlers model an annotation as a course-scoped asset owned by
-- a student (star-assignments `mark_document_annotation_read`:
-- `SELECT user_id FROM assignment_annotations WHERE id = $1 AND course_id =
-- $2`), so the table carries exactly the columns those handlers query:
-- course + owning user, plus the Canvas context columns for parity with
-- canvadocs_annotation_contexts.

CREATE TABLE IF NOT EXISTS assignment_annotations (
    id                BIGSERIAL PRIMARY KEY,
    course_id         BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    submission_id     BIGINT REFERENCES submissions(id) ON DELETE CASCADE,
    attachment_id     BIGINT REFERENCES files(id) ON DELETE SET NULL,
    root_account_id   BIGINT,
    launch_id         TEXT,
    submission_attempt INTEGER,
    workflow_state    TEXT NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_assignment_annotations_course
    ON assignment_annotations(course_id);

CREATE INDEX IF NOT EXISTS idx_assignment_annotations_user
    ON assignment_annotations(user_id);
