-- 0231: outcomes_api parity gap (OutcomesApiController show/update/destroy
-- on /outcomes/{id}). Canvas LearningOutcome carries mastery/criterion and
-- calculation attributes the 0022 snapshot lacks; the update action writes
-- them directly (process_params: title, display_name, description,
-- vendor_guid, calculation_method, calculation_int; mastery_points/ratings
-- via update_outcome_criterion).
ALTER TABLE learning_outcomes
    ADD COLUMN IF NOT EXISTS vendor_guid TEXT,
    ADD COLUMN IF NOT EXISTS mastery_points DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS calculation_method VARCHAR,
    ADD COLUMN IF NOT EXISTS calculation_int INTEGER;
