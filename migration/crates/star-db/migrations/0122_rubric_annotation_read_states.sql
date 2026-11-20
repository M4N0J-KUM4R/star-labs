-- Migration 0122: rubric assessment + document annotation read-state tables.
--
-- API scope (spec p5-rest-assessments.md §6.1, routes from lib.rs router):
-- - PUT /courses/{courseId}/rubric_assessments/{id}/read
--   (mark_rubric_assessment_read)
-- - DELETE /courses/{courseId}/rubric_assessments/{id}/read
--   (mark_rubric_assessment_unread)
-- - PUT /courses/{courseId}/assignment_annotations/{id}/read
--   (mark_document_annotation_read)
-- - DELETE /courses/{courseId}/assignment_annotations/{id}/read
--   (mark_document_annotation_unread)
-- - PUT /sections/{sectionId}/rubric_assessments/{id}/read
--   (section_mark_rubric_assessment_read)
-- - DELETE /sections/{sectionId}/rubric_assessments/{id}/read
--   (section_mark_rubric_assessment_unread)
-- - PUT /sections/{sectionId}/assignment_annotations/{id}/read
--   (section_mark_document_annotation_read)
-- - DELETE /sections/{sectionId}/assignment_annotations/{id}/read
--   (section_mark_document_annotation_unread)
--
-- Canvas stores rubric_assessment read state via Submission#last_rubric_assessment_read
-- and document_annotation read state via Submission#last_document_annotation_read
-- (both per-user, per-asset). We model them as independent per-user read markers.
--
-- Canvas: mark_rubric_assessments_read / mark_document_annotations_read require
-- the caller to be the submission owner (student), a teacher, or an admin.
-- A teacher/admin can mark as read/unread for any user.

CREATE TABLE IF NOT EXISTS rubric_assessment_reads (
    id                   BIGSERIAL PRIMARY KEY,
    rubric_assessment_id BIGINT NOT NULL REFERENCES rubric_assessments(id) ON DELETE CASCADE,
    user_id              BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    read_state           TEXT NOT NULL DEFAULT 'read',
    read_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_rubric_assessment_reads_assessment_user
    ON rubric_assessment_reads(rubric_assessment_id, user_id);

CREATE INDEX IF NOT EXISTS idx_rubric_assessment_reads_user_id
    ON rubric_assessment_reads(user_id);

CREATE TABLE IF NOT EXISTS document_annotation_reads (
    id                   BIGSERIAL PRIMARY KEY,
    document_annotation_id BIGINT NOT NULL REFERENCES assignment_annotations(id) ON DELETE CASCADE,
    user_id              BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    read_state           TEXT NOT NULL DEFAULT 'read',
    read_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_document_annotation_reads_annotation_user
    ON document_annotation_reads(document_annotation_id, user_id);

CREATE INDEX IF NOT EXISTS idx_document_annotation_reads_user_id
    ON document_annotation_reads(user_id);
