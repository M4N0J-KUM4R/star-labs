-- Migration 0063: Canvas Scheduler appointment groups parity.
--
-- Mirrors Canvas `appointment_groups`, `appointment_group_contexts` and
-- `appointment_group_sub_contexts`. Like Canvas, appointment time slots and
-- reservations are CalendarEvents: a slot carries `appointment_group_id`
-- (context = AppointmentGroup) and a reservation is a child event carrying
-- `parent_event_id` and the participant as `user_id`.
--
-- Canvas parity:
--   - appointment_groups.workflow_state: 'pending' | 'active' | 'deleted'
--     (publish transitions pending -> active; deletion is soft)
--   - participant_visibility: 'private' | 'protected'
--   - participant_type: 'User' | 'Group' (derived from sub-contexts)
--   - start_at / end_at are cached min/max of the group's time slots
--   - slots are soft-deleted with the group; reservations cascade to the slot
--
-- All statements are additive: no table or column is dropped.

CREATE TABLE IF NOT EXISTS appointment_groups (
    id                               BIGSERIAL PRIMARY KEY,
    title                            TEXT NOT NULL,
    description                      TEXT,
    location_name                    TEXT,
    location_address                 TEXT,
    start_at                         TIMESTAMPTZ,
    end_at                           TIMESTAMPTZ,
    max_appointments_per_participant INTEGER,
    min_appointments_per_participant INTEGER,
    participants_per_appointment     INTEGER,
    participant_visibility           TEXT NOT NULL DEFAULT 'private',
    participant_type                 TEXT NOT NULL DEFAULT 'User',
    workflow_state                   TEXT NOT NULL DEFAULT 'pending',
    allow_observer_signup            BOOLEAN NOT NULL DEFAULT FALSE,
    cancel_reason                    TEXT,
    created_at                       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Course (or other context) membership for an appointment group. `context_code`
-- is the Canvas asset string (`course_42`) used by the API.
CREATE TABLE IF NOT EXISTS appointment_group_contexts (
    id                  BIGSERIAL PRIMARY KEY,
    appointment_group_id BIGINT NOT NULL REFERENCES appointment_groups(id) ON DELETE CASCADE,
    context_type        TEXT NOT NULL DEFAULT 'Course',
    context_id          BIGINT NOT NULL,
    context_code        TEXT NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (appointment_group_id, context_code)
);

-- Section / group-category restrictions (`course_section_12`,
-- `group_category_3`). A group category makes signups group-based
-- (participant_type = 'Group').
CREATE TABLE IF NOT EXISTS appointment_group_sub_contexts (
    id                  BIGSERIAL PRIMARY KEY,
    appointment_group_id BIGINT NOT NULL REFERENCES appointment_groups(id) ON DELETE CASCADE,
    sub_context_type    TEXT NOT NULL,
    sub_context_id      BIGINT NOT NULL,
    sub_context_code    TEXT NOT NULL,
    participant_type    TEXT NOT NULL DEFAULT 'User',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (appointment_group_id, sub_context_code)
);

-- Appointment slots (parent_event_id IS NULL) and reservations
-- (parent_event_id = slot) stay in calendar_events like Canvas.
ALTER TABLE calendar_events
    ADD COLUMN IF NOT EXISTS appointment_group_id BIGINT REFERENCES appointment_groups(id) ON DELETE CASCADE;
ALTER TABLE calendar_events
    ADD COLUMN IF NOT EXISTS parent_event_id BIGINT REFERENCES calendar_events(id) ON DELETE CASCADE;
ALTER TABLE calendar_events
    ADD COLUMN IF NOT EXISTS location_address TEXT;
ALTER TABLE calendar_events
    ADD COLUMN IF NOT EXISTS comments TEXT;

CREATE INDEX IF NOT EXISTS idx_appointment_groups_state
    ON appointment_groups (workflow_state, end_at);

CREATE INDEX IF NOT EXISTS idx_appointment_group_contexts_group
    ON appointment_group_contexts (appointment_group_id);

CREATE INDEX IF NOT EXISTS idx_appointment_group_contexts_code
    ON appointment_group_contexts (context_code);

CREATE INDEX IF NOT EXISTS idx_appointment_group_sub_contexts_group
    ON appointment_group_sub_contexts (appointment_group_id);

CREATE INDEX IF NOT EXISTS idx_calendar_events_appointment_group
    ON calendar_events (appointment_group_id)
    WHERE appointment_group_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_calendar_events_parent_event
    ON calendar_events (parent_event_id)
    WHERE parent_event_id IS NOT NULL;
