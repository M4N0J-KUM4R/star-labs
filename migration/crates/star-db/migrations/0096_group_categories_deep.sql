-- Migration 0096: Group categories deep parity
--
-- Canvas API v1 parity:
--   GET   /accounts/{accountId}/group_categories   -> list (admin, collaboration_state)
--   POST  /accounts/{accountId}/group_categories   -> create account category (admin)
--   PATCH /group_categories/{id}                   -> partial update
--   POST  /group_categories/{id}/assign_unassigned_members -> distribute students
--   POST  /groups                                  -> create a group from its category context
--
-- Canvas Group belongs to a Course OR an Account context. Account-scoped groups
-- (created through POST /groups with an account group category) record their
-- account here; course-scoped rows keep course_id and a NULL account_id.

ALTER TABLE groups ADD COLUMN IF NOT EXISTS account_id BIGINT
    REFERENCES accounts(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_groups_account_id
    ON groups(account_id);

-- GET /accounts/{accountId}/group_categories filters active categories by
-- collaboration state.
CREATE INDEX IF NOT EXISTS idx_group_categories_account_active
    ON group_categories(account_id, non_collaborative)
    WHERE workflow_state != 'deleted';
