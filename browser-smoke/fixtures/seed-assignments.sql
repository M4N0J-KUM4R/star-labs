-- Browser-smoke fixture for the assignments + gradebook journey (issue #71).
-- Idempotent: sentinel rows are deleted first and re-inserted with explicit
-- sentinel ids (9920xx — a disjoint range from seed.sql's 9900xx), and every
-- touched sequence is re-based afterwards. Does NOT touch seed.sql's rows
-- (it only ADDS the student enrollment in course 990101, which seed.sql
-- deliberately leaves unenrolled for its own access-control journey).

BEGIN;

-- Children first for the FKs (submissions -> assignments -> courses;
-- assignment_groups -> courses; enrollments -> courses).
DELETE FROM submissions      WHERE assignment_id IN (992031, 992032, 992033);
DELETE FROM assignments      WHERE id IN (992031, 992032, 992033);
DELETE FROM assignment_groups WHERE id IN (992021, 992022);
DELETE FROM enrollments      WHERE id = 992011;

-- Assignment groups: "Essays" (position 1) and "Quizzes" (position 2) in
-- seed.sql's published course 990101.
INSERT INTO assignment_groups (id, course_id, name, position) VALUES
  (992021, 990101, 'Smoke Essays', 1),
  (992022, 990101, 'Smoke Quizzes', 2);

-- Three assignments:
--  * Essay One (published, 10 pts, due date, text-entry submission seeded)
--  * Essay Two (published, 20 pts, no due date)
--  * Draft Quiz (UNPUBLISHED — teacher-only styling on the index)
INSERT INTO assignments
  (id, course_id, assignment_group_id, title, description, points_possible,
   due_at, workflow_state) VALUES
  (992031, 990101, 992021, 'Smoke Essay One',
   '<p>Write about the <strong>smoke journey</strong>.</p>', 10,
   '2026-12-01 17:00:00+00', 'published'),
  (992032, 990101, 992021, 'Smoke Essay Two',
   NULL, 20, NULL, 'published'),
  (992033, 990101, 992022, 'Smoke Draft Quiz',
   NULL, 5, NULL, 'unpublished');

-- The student (990002) joins course 990101 as a student: seed.sql keeps her
-- unenrolled everywhere for the access-control journey; this journey needs
-- an enrollment (submissions + grades).
INSERT INTO enrollments (id, user_id, course_id, role) VALUES
  (992011, 990002, 990101, 'student');

-- One submitted + graded text submission for the student on Essay One:
-- grades page shows the score, gradebook cell shows 8.
INSERT INTO submissions
  (id, assignment_id, user_id, body, submission_type, score, workflow_state) VALUES
  (992041, 992031, 990002, 'My smoke essay text', 'online_text_entry', 8, 'graded');

-- Re-base every touched sequence past the sentinels.
SELECT setval('assignment_groups_id_seq', (SELECT GREATEST(MAX(id), 992022) FROM assignment_groups));
SELECT setval('assignments_id_seq',       (SELECT GREATEST(MAX(id), 992033) FROM assignments));
SELECT setval('enrollments_id_seq',       (SELECT GREATEST(MAX(id), 992011) FROM enrollments));
SELECT setval('submissions_id_seq',       (SELECT GREATEST(MAX(id), 992041) FROM submissions));

COMMIT;
