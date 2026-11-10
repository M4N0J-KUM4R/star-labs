-- Migration 0072: account grading schemes (Canvas GradingStandard parity)
--   GET    /accounts/{accountId}/grading_schemes
--   POST   /accounts/{accountId}/grading_schemes
--   GET    /grading_schemes/{id}
--   PUT    /grading_schemes/{id}
--   DELETE /grading_schemes/{id}  -> soft-delete (workflow_state='deleted')
--
-- Extends the 0027 grading_standards table. Canvas GradingStandard belongs to
-- either an Account or a Course context (course_id already exists; account
-- schemes keep course_id NULL and set account_id) and carries the points-based
-- fields exposed by GradingSchemeSerializer.

ALTER TABLE grading_standards
    ADD COLUMN IF NOT EXISTS account_id BIGINT REFERENCES accounts(id) ON DELETE CASCADE;

ALTER TABLE grading_standards
    ADD COLUMN IF NOT EXISTS points_based BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE grading_standards
    ADD COLUMN IF NOT EXISTS scaling_factor FLOAT8 NOT NULL DEFAULT 1.0;

CREATE INDEX IF NOT EXISTS idx_grading_standards_account_id
    ON grading_standards (account_id)
    WHERE workflow_state = 'active';

-- Account scheme listing (Canvas detail_list) orders by title within account.
CREATE INDEX IF NOT EXISTS idx_grading_standards_account_listing
    ON grading_standards (account_id, lower(title))
    WHERE workflow_state <> 'deleted';
