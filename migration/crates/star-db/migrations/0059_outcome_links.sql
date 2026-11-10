-- Migration 0059: outcome root groups + course link listing (Canvas API v1 parity).
--
-- Adds the schema needed by the Phase 3 outcome routes served by star-outcomes:
--   GET /courses/{courseId}/root_outcome_group   — lazily-created root group
--   GET /courses/{courseId}/outcome_group_links  — all links in the course
--   GET /accounts/{accountId}/outcome_groups     — account-context groups
--   GET /courses/{courseId}/outcome_groups/{id}  — course-scoped group show
--
-- Canvas creates one root outcome group per context (`LearningOutcomeGroup.
-- root_outcome_group`): the active group whose parent is nil. The base schema
-- (0044) has no way to tell a root group apart from a flat, parentless group
-- created through the API, so this migration adds an explicit marker plus a
-- partial unique index that makes lazily creating the root race-safe.

ALTER TABLE outcome_groups
    ADD COLUMN IF NOT EXISTS root_group BOOLEAN NOT NULL DEFAULT FALSE;

-- At most one root group per context. Partial so the many existing parentless
-- (non-root) groups keep working untouched.
CREATE UNIQUE INDEX IF NOT EXISTS idx_outcome_groups_one_root
    ON outcome_groups (context_type, context_id)
    WHERE root_group;

-- Listing every outcome link in a course/account context joins on the
-- denormalized context columns; the group index alone forces a per-group scan.
CREATE INDEX IF NOT EXISTS idx_outcome_links_context
    ON outcome_links (context_type, context_id, workflow_state);

-- Account outcome groups are addressed by (context_type, context_id) with a
-- NULL course_id; keep that lookup cheap.
CREATE INDEX IF NOT EXISTS idx_outcome_groups_account_context
    ON outcome_groups (context_type, context_id, workflow_state)
    WHERE course_id IS NULL;
