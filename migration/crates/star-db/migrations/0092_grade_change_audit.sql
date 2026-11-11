-- Migration 0092: Canvas grade change audit log.
--
-- Canvas `Auditors::GradeChange` events serialized by
-- `Api::V1::GradeChangeEvent#grade_change_event_json` and served by
-- `GradeChangeAuditApiController`.
--
--   grade_change_events
--     One row per submission grade change (or course override). `uuid` is the
--     Canvas event id; grade values are stored as the displayed strings and
--     the raw scores separately, mirroring Canvas `grade_before`/`grade_after`.
--
-- Routes served by star-audit:
--   GET /audit/grade_change/courses/{courseId}
--   GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}
--   GET /audit/grade_change/courses/{courseId}/students/{studentId}
--   GET /audit/grade_change/courses/{courseId}/graders/{graderId}
--   GET /audit/grade_change

CREATE TABLE IF NOT EXISTS grade_change_events (
    id                    BIGSERIAL PRIMARY KEY,
    uuid                  TEXT NOT NULL UNIQUE,
    course_id             BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    assignment_id         BIGINT REFERENCES assignments(id) ON DELETE CASCADE,
    student_id            BIGINT REFERENCES users(id) ON DELETE SET NULL,
    grader_id             BIGINT REFERENCES users(id) ON DELETE SET NULL,
    event_type            TEXT NOT NULL DEFAULT 'grade_change',
    grade_before          TEXT,
    grade_after           TEXT,
    score_before          NUMERIC(10,2),
    score_after           NUMERIC(10,2),
    points_possible_before NUMERIC(10,2),
    points_possible_after NUMERIC(10,2),
    excused_before        BOOLEAN NOT NULL DEFAULT FALSE,
    excused_after         BOOLEAN NOT NULL DEFAULT FALSE,
    graded_anonymously    BOOLEAN,
    version_number        INTEGER,
    request_id            TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Every endpoint filters on a subset of these columns and orders by time.
CREATE INDEX IF NOT EXISTS idx_grade_change_events_course
    ON grade_change_events(course_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_grade_change_events_assignment
    ON grade_change_events(assignment_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_grade_change_events_student
    ON grade_change_events(student_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_grade_change_events_grader
    ON grade_change_events(grader_id, created_at DESC);
