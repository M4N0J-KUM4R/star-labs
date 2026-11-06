-- Migration 0027: grading_standards + grading_scheme_entries
-- Canvas API v1 parity:
--   POST /courses/{courseId}/grading_standards
--   GET  /courses/{courseId}/grading_standards
--   GET  /grading_standards/{id}
--   DELETE /grading_standards/{id} (soft-delete -> workflow_state='deleted')

CREATE TABLE grading_standards (
    id            BIGSERIAL PRIMARY KEY,
    course_id     BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    -- course_id IS NULL means it's an account-level standard (not in scope here)
    title         TEXT NOT NULL,
    workflow_state TEXT NOT NULL NOT NULL DEFAULT 'active',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Index for fast lookups by course
CREATE INDEX idx_grading_standards_course_id ON grading_standards(course_id) WHERE workflow_state = 'active';
-- Index for global listing (account-level, not used in this scope)
CREATE INDEX idx_grading_standards_no_course ON grading_standards(id) WHERE course_id IS NULL AND workflow_state = 'active';

CREATE TABLE grading_scheme_entries (
    id                   BIGSERIAL PRIMARY KEY,
    grading_standard_id  BIGINT NOT NULL REFERENCES grading_standards(id) ON DELETE CASCADE,
    name                 TEXT NOT NULL,
    min_score            FLOAT8 NOT NULL,
    -- min_score is a percentage expressed as 0.0–1.0 (e.g. 0.90 = A)
    position             INT    NOT NULL DEFAULT 0
);

CREATE INDEX idx_grading_scheme_entries_standard_id ON grading_scheme_entries(grading_standard_id, position);

-- Only one active grading standard per course (enforced at app level)
