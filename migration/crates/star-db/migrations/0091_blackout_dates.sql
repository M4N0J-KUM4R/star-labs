-- Migration 0091: blackout dates (Canvas API v1 parity)
--
--   GET    /courses/{courseId}/blackout_dates
--   POST   /courses/{courseId}/blackout_dates
--   PUT    /courses/{courseId}/blackout_dates   (bulk sync)
--   GET    /accounts/{accountId}/blackout_dates
--   POST   /accounts/{accountId}/blackout_dates
--   DELETE /courses/{courseId}/blackout_dates/{id}
--
-- Mirrors Canvas `BlackoutDate` (app/models/blackout_date.rb,
-- app/controllers/blackout_dates_controller.rb). The Canvas model owns a
-- polymorphic context (Account or Course), so `context_id`/`context_type`
-- are plain columns without a foreign key.

CREATE TABLE IF NOT EXISTS blackout_dates (
    id                        BIGSERIAL PRIMARY KEY,
    context_id                BIGINT NOT NULL,
    context_type              TEXT NOT NULL DEFAULT 'Course', -- Account | Course
    -- Canvas `event_title`.
    title                     TEXT NOT NULL,
    description               TEXT,
    start_date                TIMESTAMPTZ NOT NULL,
    end_date                  TIMESTAMPTZ NOT NULL,
    -- Optional grading period this blackout date belongs to.
    parent_grading_period_id  BIGINT REFERENCES grading_periods(id) ON DELETE SET NULL,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_blackout_dates_context
    ON blackout_dates (context_type, context_id, start_date);
