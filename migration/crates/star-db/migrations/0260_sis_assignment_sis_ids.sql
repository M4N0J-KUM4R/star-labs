-- 0260: SIS assignment ids — Canvas `assignments.sis_source_id` and friends
-- (issue #92, REST: SIS imports / sis_assignments / pseudonyms).
--
-- Canvas exposes them through Api::V1::SisAssignment
-- (`sis_assignment_id` is `alias_attribute :sis_assignment_id, :sis_source_id`,
-- app/models/abstract_assignment.rb) and the SIS grade-export feed
-- (`SisApiController#sis_assignments`) plus `disable_post_to_sis`
-- (which flips assignments.post_to_sis, added by 0256).
--
-- Canvas column provenance (db canvas-authored schema):
--   - assignments.sis_source_id / integration_id / integration_data /
--     omit_from_final_grade
--   - course_sections.integration_id
--
-- Additive only (ADD COLUMN IF NOT EXISTS): no DROP, no TRUNCATE, no RENAME.
-- 0231 already added the sis_source_id/integration_data columns of the other
-- Canvas tables (accounts, courses, course_sections.sis_source_id,
-- assignment_groups, ...) — the assignments and course_sections.integration_id
-- columns were the ones this API surface still misses.

ALTER TABLE assignments
    ADD COLUMN IF NOT EXISTS sis_source_id varchar(255);
ALTER TABLE assignments
    ADD COLUMN IF NOT EXISTS integration_id varchar(255);
ALTER TABLE assignments
    ADD COLUMN IF NOT EXISTS integration_data text;
ALTER TABLE assignments
    ADD COLUMN IF NOT EXISTS omit_from_final_grade boolean NOT NULL DEFAULT false;

ALTER TABLE course_sections
    ADD COLUMN IF NOT EXISTS integration_id varchar(255);

-- The grade-export feed looks the assignments up by their SIS id.
CREATE INDEX IF NOT EXISTS idx_assignments_sis_source_id
    ON assignments (sis_source_id)
    WHERE sis_source_id IS NOT NULL;
