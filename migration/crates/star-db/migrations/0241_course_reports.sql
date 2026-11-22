-- 0241_course_reports.sql — Canvas `reports` rows in a course context
-- (CourseReportsApiController: course_pace_docx). Canvas keeps course
-- reports in the `reports` table with a Course context; star-lms models
-- them as their own `course_reports` table with the Canvas columns the
-- API reads and writes.

CREATE TABLE IF NOT EXISTS course_reports (
    id              BIGSERIAL PRIMARY KEY,
    course_id       BIGINT NOT NULL,
    user_id         BIGINT NOT NULL,
    report_type     VARCHAR(255) NOT NULL,
    workflow_state  VARCHAR(255) NOT NULL DEFAULT 'created',
    parameters      TEXT,
    progress        INTEGER,
    attachment_id   BIGINT,
    start_at        TIMESTAMP,
    end_at          TIMESTAMP,
    created_at      TIMESTAMP NOT NULL DEFAULT now(),
    updated_at      TIMESTAMP NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_course_reports_course_type_user
    ON course_reports (course_id, report_type, user_id);
