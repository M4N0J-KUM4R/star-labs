# Implementation notes for issue #32 gradebook_history routes (star-audit crate)
# Data source decision: star-lms has NO versions/submission_versions writers (tables exist from
# migration 0225 but nothing populates them). grade_change_events (migration 0092, backfilled by
# 0118) IS the audit trail the snapshot keeps. Canvas GradebookHistory reads submission versions;
# Star maps: one event = one graded version. Deviation documented in PR body.
#
# Route shapes (Canvas gradebook_history_api_controller.rb):
# 1. GET /api/v1/courses/:course_id/gradebook_history/days -> days
#    - require_manage_grades on course (401 anon, 403 student, 404 unknown course)
#    - group graded submissions (graded_at NOT NULL) by calendar day (course TZ; Star: UTC date),
#      each day: [{date, graders: [{id, name, assignments: [assignment_json...]}]}]
#    - sorted by date DESC; grader null -> id 0 name "Graded on submission"
#    - Canvas submissions_set: submissions grader_id<=0 counted under grader 0 bucket
# 2. GET /api/v1/courses/:course_id/gradebook_history/:date -> day_details (date %Y-%m-%d, else 400)
#    - same shape, single day, returns grader array directly (not wrapped in day)
# 3. GET /api/v1/courses/:course_id/gradebook_history/:date/graders/:grader_id/assignments/:assignment_id/submissions
#    - group by submission {submission_id, versions: [...]}; only score-changing versions;
#      previous_grade/graded_at/grader nil on first; new_* aliases; newest-first per submission
# 4. GET /api/v1/courses/:course_id/gradebook_history/feed?assignment_id&user_id&ascending
#    - paginated uncollated versions, plain grade/graded_at/grader fields, version_id order
print("notes saved")
