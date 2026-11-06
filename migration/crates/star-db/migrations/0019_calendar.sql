-- Migration 0019: calendar_events table for Canvas API parity.
--
-- Calendar events can be course-scoped (course_id set, user_id null) or
-- personal (user_id set, course_id null).  The full Canvas model supports
-- many context types (course, user, group, appointment_group, course_section,
-- account), but this migration covers the two primary cases used by the
-- course-scoped and user-navigable calendar endpoints.
--
-- Canvas parity:
--   - workflow_state: 'active' | 'locked' | 'deleted'  (soft delete)
--   - start_at / end_at stored as TIMESTAMPTZ (ISO 8601 / RFC 3339)
--   - all DELETE operations soft-delete (set workflow_state='deleted')
--   - list endpoints exclude workflow_state='deleted'

CREATE TABLE calendar_events (
    id               BIGSERIAL PRIMARY KEY,
    course_id        BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    user_id          BIGINT REFERENCES users(id)  ON DELETE CASCADE,
    title            TEXT NOT NULL,
    description      TEXT,
    start_at         TIMESTAMPTZ NOT NULL,
    end_at           TIMESTAMPTZ,
    workflow_state   TEXT NOT NULL DEFAULT 'active',
    location_name    TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Index for course-scoped event lookups (list + create)
CREATE INDEX idx_calendar_events_course_id ON calendar_events (course_id);

-- Index for personal (user) calendar lookups
CREATE INDEX idx_calendar_events_user_id ON calendar_events (user_id);

-- Index for date-range queries on the user's enrolled courses
CREATE INDEX idx_calendar_events_start_at ON calendar_events (start_at);

-- Composite index for listing course events (exclude deleted)
CREATE INDEX idx_calendar_events_course_active
    ON calendar_events (course_id, start_at)
    WHERE workflow_state <> 'deleted';
