-- Migration 0110: assignments.workflow_state (issue #29, GraphQL Batch A)
--
-- Canvas assignments carry a workflow_state ("published" | "unpublished" |
-- "deleted" among others); the GraphQL create/update mutations map the
-- `state: AssignmentState` input onto it and deletion is a *soft* delete
-- (workflow_state = 'deleted', assignment_base.rb ensure_destroyed).
-- star-lms had no such column — every assignment was implicitly visible —
-- which blocked the GraphQL mutation batch spec'd in specs/p5-gql-mutations.md §4.
--
-- Additive only. Backfill preserves current behavior: every existing row is
-- treated as published so the REST list/get handlers keep returning them.
-- The CHECK constraint restricts the column to the three states this batch
-- uses; widening it later is additive.

ALTER TABLE assignments ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'published'
    CHECK (workflow_state IN ('unpublished', 'published', 'deleted'));

UPDATE assignments SET workflow_state = 'published' WHERE workflow_state IS NULL;
