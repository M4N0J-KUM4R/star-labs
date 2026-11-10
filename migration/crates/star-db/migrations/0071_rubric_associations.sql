-- Migration 0071: Canvas rubric associations (star-rubrics parity).
--
-- Canvas ties a rubric to a polymorphic "association object" (assignment,
-- course, or account) through `rubric_associations`. star-lms rubrics were
-- course-scoped only, so this adds the Canvas join table plus the rubric
-- counters Canvas maintains on every association change.
--
-- API scope:
-- - GET    /courses/{courseId}/rubrics/{rubricId}/used_locations
-- - POST   /courses/{courseId}/rubrics/{rubricId}/associations
-- - DELETE /rubric_associations/{id}
--
-- Never drops tables/columns; existing rubric/assessment rows are untouched.

CREATE TABLE IF NOT EXISTS rubric_associations (
    id                    BIGSERIAL PRIMARY KEY,
    rubric_id             BIGINT NOT NULL REFERENCES rubrics(id) ON DELETE CASCADE,
    -- Polymorphic association object (Assignment/Course); no FK because the
    -- target table varies by association_type.
    association_id        BIGINT NOT NULL,
    association_type      TEXT NOT NULL,
    -- Canvas context owning the association; star-lms associations are always
    -- course-scoped, so context_type defaults to 'Course'.
    context_id            BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    context_type          TEXT NOT NULL DEFAULT 'Course',
    context_code          TEXT,
    title                 TEXT,
    use_for_grading       BOOLEAN NOT NULL DEFAULT false,
    hide_score_total      BOOLEAN NOT NULL DEFAULT false,
    hide_points           BOOLEAN NOT NULL DEFAULT false,
    hide_outcome_results  BOOLEAN NOT NULL DEFAULT false,
    purpose               TEXT NOT NULL DEFAULT 'grading',  -- grading | bookmark
    bookmarked            BOOLEAN NOT NULL DEFAULT true,
    url                   TEXT,
    summary_data          JSONB,
    workflow_state        TEXT NOT NULL DEFAULT 'active',
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Canvas `Rubric#update_association_count` keeps these counters on the rubric:
-- `association_count` counts active associations, `read_only` flips once more
-- than one active grading association shares the rubric.
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS association_count INT NOT NULL DEFAULT 0;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS read_only BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX IF NOT EXISTS idx_rubric_associations_rubric
    ON rubric_associations(rubric_id);
CREATE INDEX IF NOT EXISTS idx_rubric_associations_context
    ON rubric_associations(context_id);
-- Serves used_locations (rubric_id + Assignment join) and the Canvas
-- `assert_uniqueness` lookup by association object.
CREATE INDEX IF NOT EXISTS idx_rubric_associations_association
    ON rubric_associations(association_type, association_id);
