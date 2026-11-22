-- 0248: gradebook settings + what-if grades (parity gaps —
-- GradebookSettingsController#update and WhatIfGradesApiController
-- update/reset_for_student_course).
-- Canvas stores per-user gradebook preferences (User#get_preference/
-- set_preference) keyed by course global id plus a colors key; the
-- workspace gets a generic user_preferences table. What-if grades write
-- submissions.student_entered_score (Canvas column of the same name).
CREATE TABLE IF NOT EXISTS user_preferences (
    id           BIGSERIAL PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    value        JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_user_preference UNIQUE (user_id, name)
);

ALTER TABLE submissions
    ADD COLUMN IF NOT EXISTS student_entered_score DOUBLE PRECISION;
