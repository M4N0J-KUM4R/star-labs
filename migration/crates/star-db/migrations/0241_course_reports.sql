-- 0241_course_reports.sql — Canvas `reports` rows in a course context
-- (CourseReportsApiController: course_pace_docx). Canvas keeps course
-- reports in the `reports` table with a Course context; star-lms models
-- them as their own `course_reports` table.
--
-- The table itself already exists from the Canvas schema dump (0223),
-- so the CREATE below only covers fresh databases; on every database
-- that ran 0223 the ALTERs add the star-specific columns the API reads
-- and writes (progress; the 0223 columns match the rest).

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

-- 0223's course_reports predates the progress column the API writes;
-- ADD COLUMN IF NOT EXISTS keeps both fresh and 0223-migrated databases
-- on the same shape.
ALTER TABLE course_reports
    ADD COLUMN IF NOT EXISTS progress INTEGER;

CREATE INDEX IF NOT EXISTS idx_course_reports_course_type_user
    ON course_reports (course_id, report_type, user_id);
