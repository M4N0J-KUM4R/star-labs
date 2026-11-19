-- Migration 0118: grade change audit tail (issue #53).
--
-- Canvas `Auditors::GradeChange` scoping for the remaining
-- `grade_change_audit_api_controller` entry points:
--
--   GET /audit/grade_change/assignments/{assignmentId}
--   GET /audit/grade_change/students/{studentId}
--   GET /audit/grade_change/graders/{graderId}
--   GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}/graders/{graderId}
--   GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}/students/{studentId}
--   GET /audit/grade_change/courses/{courseId}/graders/{graderId}/students/{studentId}
--   GET /audit/grade_change/courses/{courseId}/assignments/{assignmentId}/graders/{graderId}/students/{studentId}
--
-- Canvas requirements these columns support:
-- - `Auditors::GradeChange.for_root_account_student` /
--   `for_root_account_grader` filter on the event's ROOT account
--   (`auditor_grade_change_records.root_account_id`), so student/grader
--   scoped queries must be restrictable to the request's domain root account.
-- - Course final grade overrides are stored with a NULL assignment
--   (`NULL_PLACEHOLDER` in Cassandra, `submission_id IS NULL` in the
--   ActiveRecord backend) and hidden unless the course allows overrides
--   (`Course#allow_final_grade_override?`).
-- - `render_events` drops or anonymizes events whose assignment is
--   anonymous AND muted (`Assignment#anonymous_grading? && muted?`);
--   `muted` had no Star column yet (`anonymous_grading` landed in 0080).
-- - Canvas `Course#allow_final_grade_override?` is a course column; Star
--   needs it to reproduce the override visibility rule.
-- - `events_visible_to_current_user` hides other sections' students from
--   teachers/TAs whose enrollment carries
--   `Enrollment#limit_privileges_to_course_section`; the Star `enrollments`
--   table had no such flag.

ALTER TABLE grade_change_events
    ADD COLUMN IF NOT EXISTS root_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_grade_change_events_root_account_student
    ON grade_change_events(root_account_id, student_id, created_at DESC)
    WHERE student_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_grade_change_events_root_account_grader
    ON grade_change_events(root_account_id, grader_id, created_at DESC)
    WHERE grader_id IS NOT NULL;

ALTER TABLE assignments ADD COLUMN IF NOT EXISTS muted BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE courses ADD COLUMN IF NOT EXISTS allow_final_grade_override BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE enrollments
    ADD COLUMN IF NOT EXISTS limit_privileges_to_course_section BOOLEAN NOT NULL DEFAULT FALSE;
