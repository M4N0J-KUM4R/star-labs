-- Migration 0025: Assignment Groups
-- Canvas-compatible assignment group for organizing assignments by category/weight

CREATE TABLE assignment_groups (
    id BIGSERIAL PRIMARY KEY,
    course_id BIGINT NOT NULL REFERENCES courses(id),
    name TEXT NOT NULL,
    position INT NOT NULL DEFAULT 1,
    group_weight DOUBLE PRECISION NOT NULL DEFAULT 0,
    workflow_state TEXT NOT NULL DEFAULT 'available',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Index for course lookups
CREATE INDEX idx_assignment_groups_course_id ON assignment_groups(course_id);

-- Index for ordering by position within course
CREATE INDEX idx_assignment_groups_course_position ON assignment_groups(course_id, position);

-- Soft-delete scope: excludes 'deleted' workflow_state
-- Position ordering is handled via the position column (acts_as_list-like)

-- Add assignment_group_id to assignments table (allows NULL for existing assignments)
ALTER TABLE assignments ADD COLUMN IF NOT EXISTS assignment_group_id BIGINT REFERENCES assignment_groups;

-- Index for assignment -> group lookups
CREATE INDEX IF NOT EXISTS idx_assignments_assignment_group_id ON assignments(assignment_group_id);
