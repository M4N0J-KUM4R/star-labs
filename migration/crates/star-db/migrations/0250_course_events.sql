-- 0250: course audit events (parity gap — CourseAuditApiController
-- for_course/for_account). Mirrors the authentication_events convention
-- (0232): plain BIGSERIAL table, jsonb payload, per-scope indexes.
CREATE TABLE IF NOT EXISTS course_events (
    id           BIGSERIAL PRIMARY KEY,
    course_id    BIGINT,
    account_id   BIGINT,
    event_type   TEXT,
    payload      JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_course_events_course
    ON course_events (course_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_course_events_account
    ON course_events (account_id, created_at DESC);
