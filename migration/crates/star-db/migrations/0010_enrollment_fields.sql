-- Canvas parity: enrollment workflow_state (active | invited | completed | deleted | inactive)
ALTER TABLE enrollments ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'active';
CREATE INDEX IF NOT EXISTS idx_enrollments_workflow_state ON enrollments(workflow_state);
