-- Add Canvas parity fields to courses table.
-- workflow_state: course lifecycle state (available, unpublished, completed, deleted)
-- start_at / end_at: course term dates
-- syllabus_body: rich HTML syllabus content

ALTER TABLE courses ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'available';
ALTER TABLE courses ADD COLUMN IF NOT EXISTS start_at TIMESTAMPTZ;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS end_at TIMESTAMPTZ;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS syllabus_body TEXT;

-- Index for filtering by workflow_state efficiently
CREATE INDEX IF NOT EXISTS idx_courses_workflow_state ON courses(workflow_state);
