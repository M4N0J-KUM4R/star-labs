-- Migration 0038: Assignment Overrides
-- Canvas API v1 parity:
--   GET/POST /courses/{courseId}/assignments/{assignmentId}/overrides
--   GET/PUT/DELETE /courses/{courseId}/assignments/{assignmentId}/overrides/{id}
--   GET/PUT/DELETE /assignment_overrides/{id}
--
-- An override targets exactly one set, in Canvas priority order:
-- student_ids (ad-hoc) > group_id > course_section_id.
-- Canvas stores ad-hoc students in assignment_override_students; here the ids
-- live in a bigint[] column so the Canvas-shaped student_ids array can be
-- served directly.

CREATE TABLE IF NOT EXISTS assignment_overrides (
    id                BIGSERIAL PRIMARY KEY,
    assignment_id     BIGINT NOT NULL REFERENCES assignments(id) ON DELETE CASCADE,
    title             TEXT NOT NULL DEFAULT 'No Title',
    due_at            TIMESTAMPTZ,
    unlock_at         TIMESTAMPTZ,
    lock_at           TIMESTAMPTZ,
    student_ids       BIGINT[] NOT NULL DEFAULT '{}',
    course_section_id BIGINT,
    group_id          BIGINT REFERENCES groups(id) ON DELETE SET NULL,
    workflow_state    TEXT NOT NULL DEFAULT 'active',  -- active | deleted
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Active overrides for an assignment (the only rows the API lists).
CREATE INDEX IF NOT EXISTS idx_assignment_overrides_assignment_id
    ON assignment_overrides(assignment_id)
    WHERE workflow_state <> 'deleted';

-- Target lookups (section / group overrides).
CREATE INDEX IF NOT EXISTS idx_assignment_overrides_course_section_id
    ON assignment_overrides(course_section_id)
    WHERE course_section_id IS NOT NULL AND workflow_state <> 'deleted';
CREATE INDEX IF NOT EXISTS idx_assignment_overrides_group_id
    ON assignment_overrides(group_id)
    WHERE group_id IS NOT NULL AND workflow_state <> 'deleted';

-- Ad-hoc target lookup (student id inside the array).
CREATE INDEX IF NOT EXISTS idx_assignment_overrides_student_ids
    ON assignment_overrides USING GIN (student_ids);
