-- Migration 0042: planner_notes table for Canvas Planner Notes parity.
--
-- Mirrors Canvas `planner_notes` (app/models/planner_note.rb):
--   - a planner note belongs to exactly one user
--   - course_id is optional (None = personal note, not tied to a course)
--   - todo_date is the planner day the note appears on (TIMESTAMPTZ, Canvas
--     stores a timestamp, not a date)
--   - workflow_state: 'active' | 'deleted' (Canvas::SoftDeletable; DELETE
--     soft-deletes and list endpoints exclude deleted notes)
--
-- The Canvas linked_object_* columns are not part of this migration; the
-- exposed API shape is id, title, details, user_id, course_id, todo_date,
-- workflow_state, created_at, updated_at.

CREATE TABLE IF NOT EXISTS planner_notes (
    id             BIGSERIAL PRIMARY KEY,
    user_id        BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    course_id      BIGINT REFERENCES courses(id) ON DELETE SET NULL,
    title          TEXT NOT NULL,
    details        TEXT,
    todo_date      TIMESTAMPTZ NOT NULL,
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Primary access path: a user's active notes ordered by planner day.
CREATE INDEX IF NOT EXISTS idx_planner_notes_user_todo
    ON planner_notes (user_id, todo_date)
    WHERE workflow_state <> 'deleted';

-- Filtering a user's notes by associated course (context_codes[]).
CREATE INDEX IF NOT EXISTS idx_planner_notes_course_id
    ON planner_notes (course_id);
