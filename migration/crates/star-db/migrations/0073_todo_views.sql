-- Migration 0073: dashboard todo / upcoming events / dashboard cards support.
--
-- Canvas parity:
--   - GET /users/self/todo            (UsersController#todo_items)
--   - GET /users/self/upcoming_events (UsersController#upcoming_events)
--   - GET /dashboard/dashboard_cards  (UsersController#dashboard_cards)
--
-- todo_ignores mirrors Canvas user_todo_ignore preferences referenced by the
-- `ignore` / `ignore_permanently` URLs in todo item payloads. `permanent`
-- distinguishes "hide until the item changes" from "hide forever".
--
-- dashboard_positions mirrors Canvas user dashboard positions
-- (User#dashboard_positions), which order the dashboard cards.
--
-- The indexes back the new read paths; no existing table/column is dropped.

CREATE TABLE IF NOT EXISTS todo_ignores (
    id          BIGSERIAL PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    asset_type  TEXT NOT NULL,                     -- assignment | quiz
    asset_id    BIGINT NOT NULL,
    purpose     TEXT NOT NULL,                     -- grading | submitting
    permanent   BOOLEAN NOT NULL DEFAULT false,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, asset_type, asset_id, purpose, permanent)
);

CREATE INDEX IF NOT EXISTS idx_todo_ignores_user
    ON todo_ignores (user_id, permanent);

CREATE TABLE IF NOT EXISTS dashboard_positions (
    id          BIGSERIAL PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    course_id   BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, course_id)
);

CREATE INDEX IF NOT EXISTS idx_dashboard_positions_user
    ON dashboard_positions (user_id, position);

-- Todo/upcoming course + due-date scans (assignments already has an index on
-- course_id alone from 0001).
CREATE INDEX IF NOT EXISTS idx_assignments_course_due
    ON assignments (course_id, due_at);

-- Needs-grading probe: submitted work without a score, per assignment.
CREATE INDEX IF NOT EXISTS idx_submissions_ungraded
    ON submissions (assignment_id)
    WHERE score IS NULL AND body IS NOT NULL;

-- Upcoming personal-event window scan (course window reuses 0019 indexes).
CREATE INDEX IF NOT EXISTS idx_calendar_events_user_start
    ON calendar_events (user_id, start_at)
    WHERE workflow_state <> 'deleted';
