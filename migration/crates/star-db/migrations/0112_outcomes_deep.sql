-- Migration 0112: outcomes deep parity columns (issue #33).
--
-- Supports the outcomes_api routes served by star-outcomes:
--   GET/PUT/DELETE /outcomes/:id — full Canvas Outcome JSON including the
--   embedded rubric criterion (ratings + mastery_points) and the calculation
--   method/int.
--
-- Canvas stores the embedded rubric criterion on the LearningOutcome itself
-- (rubric_criterion: mastery_points, points_possible, ratings[{description,
-- points}]) plus calculation_method/calculation_int/vendor_guid. 0022 only
-- created title/description/display_name, so these columns are additive.
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS mastery_points NUMERIC;
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS calculation_method TEXT;
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS calculation_int INT;
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS vendor_guid TEXT;
ALTER TABLE learning_outcomes ADD COLUMN IF NOT EXISTS ratings JSONB;

-- outcomes_api#show/update filter on workflow_state ('active' vs 'deleted').
CREATE INDEX IF NOT EXISTS idx_learning_outcomes_state ON learning_outcomes(workflow_state);
