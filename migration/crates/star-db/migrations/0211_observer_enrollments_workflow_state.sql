-- Migration 0211: observer_enrollments_workflow_state
--
-- Adds workflow_state to observer_enrollments so the observee listing
-- queries can return enrollment state consistently with other enrollment
-- tables.

ALTER TABLE observer_enrollments
    ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'active';
