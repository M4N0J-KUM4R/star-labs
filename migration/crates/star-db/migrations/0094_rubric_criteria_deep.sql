-- Migration 0094: rubric criteria deep fields for Canvas CSV import/export.
--
-- Canvas `RubricCriterion` carries `long_description`, `criterion_use_range`
-- and `ignore_for_scoring`; `RubricRating` carries `long_description`. These
-- columns are exported verbatim by `RubricsApiController#download_rubrics`.

ALTER TABLE rubric_criteria ADD COLUMN IF NOT EXISTS long_description TEXT;
ALTER TABLE rubric_criteria
    ADD COLUMN IF NOT EXISTS criterion_use_range BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE rubric_criteria
    ADD COLUMN IF NOT EXISTS ignore_for_scoring BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE rubric_criteria ADD COLUMN IF NOT EXISTS learning_outcome_id BIGINT;

ALTER TABLE rubric_ratings ADD COLUMN IF NOT EXISTS long_description TEXT;

-- Export/read paths order criteria and ratings by position within a rubric.
CREATE INDEX IF NOT EXISTS idx_rubric_criteria_rubric_position
    ON rubric_criteria(rubric_id, position);
CREATE INDEX IF NOT EXISTS idx_rubric_ratings_criterion_position
    ON rubric_ratings(criterion_id, position);
