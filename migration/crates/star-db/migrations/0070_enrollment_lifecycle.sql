-- Migration 0070: Canvas enrollment lifecycle + user-scoped listing.
--
-- API scope:
-- - POST /courses/{courseId}/enrollments/{id}/accept     → { success: true }
-- - POST /courses/{courseId}/enrollments/{id}/reject     → { success: true }
-- - PUT  /courses/{courseId}/enrollments/{id}/reactivate  → Enrollment
-- - GET  /users/{userId}/enrollments                      → [Enrollment]
--
-- The lifecycle itself rides on the existing `enrollments.workflow_state`
-- column (invited → active/rejected, inactive → active). Canvas also returns
-- enrollment dates and activity counters from enrollment_json, so mirror those
-- columns here; NULL start_at/end_at keeps an enrollment on the course dates.

ALTER TABLE enrollments
    ADD COLUMN IF NOT EXISTS start_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS end_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS last_activity_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS last_attended_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS total_activity_time INTEGER NOT NULL DEFAULT 0;

-- GET /users/{userId}/enrollments filters one user's enrollments by state.
CREATE INDEX IF NOT EXISTS idx_enrollments_user_workflow_state
    ON enrollments(user_id, workflow_state);

-- Lifecycle transitions look up one enrollment within its course.
CREATE INDEX IF NOT EXISTS idx_enrollments_course_workflow_state
    ON enrollments(course_id, workflow_state);
