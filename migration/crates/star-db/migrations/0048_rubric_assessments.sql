-- Migration 0048: Canvas-shaped rubric assessments.
--
-- Migration 0023 already created `rubric_assessments` for submission-based
-- assessments. Canvas also associates an assessment with a polymorphic
-- `artifact` (submission or provisional grade) and records the assessed user,
-- so this migration widens the existing table in place. It never drops columns
-- or rows; older rows are backfilled with their course/user through the
-- submission they reference.

CREATE TABLE IF NOT EXISTS rubric_assessments (
    id                     BIGSERIAL PRIMARY KEY,
    rubric_id              BIGINT NOT NULL REFERENCES rubrics(id) ON DELETE CASCADE,
    course_id              BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    rubric_association_id  BIGINT,
    submission_id          BIGINT REFERENCES submissions(id) ON DELETE CASCADE,
    artifact_id            BIGINT,
    artifact_type          TEXT,
    assessment_type        TEXT NOT NULL DEFAULT 'grading',
    user_id                BIGINT REFERENCES users(id) ON DELETE SET NULL,
    assessor_id            BIGINT NOT NULL REFERENCES users(id),
    score                  FLOAT8,
    data                   JSONB,
    comments               TEXT,
    workflow_state         TEXT NOT NULL DEFAULT 'active',
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE rubric_assessments
    ADD COLUMN IF NOT EXISTS course_id BIGINT REFERENCES courses(id) ON DELETE CASCADE;
ALTER TABLE rubric_assessments ADD COLUMN IF NOT EXISTS rubric_association_id BIGINT;
ALTER TABLE rubric_assessments ADD COLUMN IF NOT EXISTS artifact_id BIGINT;
ALTER TABLE rubric_assessments ADD COLUMN IF NOT EXISTS artifact_type TEXT;
ALTER TABLE rubric_assessments
    ADD COLUMN IF NOT EXISTS assessment_type TEXT NOT NULL DEFAULT 'grading';
ALTER TABLE rubric_assessments
    ADD COLUMN IF NOT EXISTS user_id BIGINT REFERENCES users(id) ON DELETE SET NULL;
ALTER TABLE rubric_assessments ADD COLUMN IF NOT EXISTS comments TEXT;

-- Artifact-only assessments (e.g. provisional grades) have no submission row.
ALTER TABLE rubric_assessments ALTER COLUMN submission_id DROP NOT NULL;

-- Backfill Canvas fields for assessments created before this migration.
UPDATE rubric_assessments ra
SET course_id = a.course_id
FROM submissions s
JOIN assignments a ON a.id = s.assignment_id
WHERE ra.submission_id = s.id
  AND ra.course_id IS NULL;

UPDATE rubric_assessments ra
SET user_id = s.user_id
FROM submissions s
WHERE ra.submission_id = s.id
  AND ra.user_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_rubric_assessments_course ON rubric_assessments(course_id);
CREATE INDEX IF NOT EXISTS idx_rubric_assessments_assessor ON rubric_assessments(assessor_id);
CREATE INDEX IF NOT EXISTS idx_rubric_assessments_user ON rubric_assessments(user_id);
