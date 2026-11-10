-- Migration 0055: Canvas course settings + course tab configuration.
--   GET/PUT /courses/{courseId}/settings
--   GET     /courses/{courseId}/users, /courses/{courseId}/users/{userId}
--   GET     /courses/{courseId}/tabs
--
-- Canvas keeps course settings serialized on the course row
-- (Course#settings) and tab configuration as an ordered array
-- (Course#tab_configuration). Mirror that with one JSON document per
-- course and one row per configured tab.

CREATE TABLE IF NOT EXISTS course_settings (
    course_id  BIGINT PRIMARY KEY REFERENCES courses(id) ON DELETE CASCADE,
    -- JSON object of Canvas course_settings_json keys (effective values).
    settings   TEXT NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS course_tabs (
    id         BIGSERIAL PRIMARY KEY,
    course_id  BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    tab_id     TEXT NOT NULL,
    position   INTEGER NOT NULL DEFAULT 0,
    hidden     BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (course_id, tab_id)
);

CREATE INDEX IF NOT EXISTS idx_course_tabs_course
    ON course_tabs(course_id, position);
