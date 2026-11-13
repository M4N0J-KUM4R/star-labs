-- Migration 0101: Canvas temporary enrollment status + last attended.
--
-- API scope:
-- - GET /users/{userId}/temporary_enrollment_status      → { is_provider, is_recipient, can_provide }
-- - GET /temporary_enrollment_status                     → { "<user_id>": { ... } }
-- - PUT /courses/{courseId}/users/{userId}/last_attended → { date }
--
-- Canvas TemporaryEnrollmentPairing creates a plain enrollment carrying
-- `temporary_enrollment_source_user_id` (the provider's user id). The status
-- endpoints only need that link, so add it to the existing enrollments table;
-- NULL keeps normal enrollments untouched.

ALTER TABLE enrollments
    ADD COLUMN IF NOT EXISTS temporary_enrollment_source_user_id BIGINT;

-- Provider lookups scan by provider; recipient lookups scan by recipient.
CREATE INDEX IF NOT EXISTS idx_enrollments_temp_source_user
    ON enrollments(temporary_enrollment_source_user_id)
    WHERE temporary_enrollment_source_user_id IS NOT NULL;

-- last_attended updates every enrollment of one user in one course.
CREATE INDEX IF NOT EXISTS idx_enrollments_course_user
    ON enrollments(course_id, user_id);
