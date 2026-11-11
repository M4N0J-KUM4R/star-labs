-- Migration 0081: deep outcome group parity (Canvas API v1).
--
-- Supports the account/course-scoped outcome group routes served by
-- star-outcomes:
--   GET|PUT|DELETE /accounts/{accountId}/outcome_groups/{id}
--   GET            /accounts/{accountId}/outcome_groups/{id}/subgroups
--   PUT|DELETE     /courses/{courseId}/outcome_groups/{id}
--
-- Canvas `LearningOutcomeGroup` can record the global group it was imported
-- from (source_outcome_group_id), so imported group trees keep their provenance.
ALTER TABLE outcome_groups
    ADD COLUMN IF NOT EXISTS source_outcome_group_id BIGINT REFERENCES outcome_groups(id) ON DELETE SET NULL;

-- `#subgroups` lists immediate active children ordered by title; that walk
-- filters on parent_outcome_group_id and workflow_state.
CREATE INDEX IF NOT EXISTS idx_outcome_groups_parent_state
    ON outcome_groups (parent_outcome_group_id, workflow_state);
