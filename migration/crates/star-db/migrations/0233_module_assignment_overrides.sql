-- 0233: module assignment overrides (parity gap —
-- ModuleAssignmentOverridesController index/bulk_update/
-- convert_tag_overrides_to_adhoc_overrides).
-- Canvas scopes a module's differentiated overrides through
-- assignment_overrides with a context_module_id discriminator; 0038 lacks
-- the column. Differentiation-tag conversion writes adhoc (student_ids)
-- rows from group overrides in place.
ALTER TABLE assignment_overrides
    ADD COLUMN IF NOT EXISTS context_module_id BIGINT;
