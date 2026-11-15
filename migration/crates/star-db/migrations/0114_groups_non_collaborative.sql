-- Migration 0114: groups non_collaborative flag (issue #35, spec §4)
--
-- Canvas API v1 parity:
--   GET /accounts/{accountId}/groups?collaboration_state=... (groups#context_index)
--   GET /courses/{courseId}/bulk_user_tags                   (differentiation tags)
--
-- Canvas differentiates collaboration groups from "differentiation tags"
-- (non-collaborative group categories / groups). 0043/0096 already carry
-- non_collaborative on group_categories; this adds the mirrored flag on
-- groups so the collaboration_state filter and the tag bulk routes can
-- filter without joining the category.
--
-- Additive only: existing rows keep the default (collaborative).

ALTER TABLE groups ADD COLUMN IF NOT EXISTS non_collaborative BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX IF NOT EXISTS idx_groups_non_collaborative ON groups(non_collaborative) WHERE non_collaborative;
