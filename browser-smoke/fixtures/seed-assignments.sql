-- Browser-smoke fixture for the assignments + gradebook journey (issue #71).
-- Applied by the workflow's glob loop BEFORE seed.sql ('-' 0x2D sorts before
-- '.' 0x2E, so seed-assignments.sql runs first), which is why this fragment
-- is FULLY SELF-SUFFICIENT and order-independent:
--   * own sentinel users 992001/992002, course 992101, enrollments
--     992111/992112, assignment groups 992021/992022, assignments
--     992031/992032/992033, submission 992041 (all 9920xx — disjoint from
--     seed.sql's 9900xx and the other journey fragments' 9930xx/9940xx);
--   * deletes-first idempotency for its OWN sentinels only;
--   * own SELECT setval(...) re-basing for every touched sequence, computed
--     from GREATEST(MAX(id), sentinel) so it is safe whether this fragment
--     runs before or after seed.sql.
--
-- Password hash: argon2id PHC string for "SmokeTest-123!" (Rust argon2 crate
-- defaults: v=19, m=19456, t=2, p=1) — identical to seed.sql, so the journey
-- logs in through the normal form path.

BEGIN;

-- Children first for the FKs (submissions -> assignments -> courses;
-- assignment_groups -> courses; enrollments -> courses).
DELETE FROM submissions       WHERE id = 992041;
DELETE FROM assignments       WHERE id IN (992031, 992032, 992033);
DELETE FROM assignment_groups WHERE id IN (992021, 992022);
DELETE FROM enrollments       WHERE id IN (992111, 992112);
DELETE FROM courses           WHERE id = 992101;
DELETE FROM users             WHERE id IN (992001, 992002);

-- Users: one teacher, one student, both enrolled in the course below.
INSERT INTO users (id, name, email, password_hash, role) VALUES
  (992001, 'Assignments Teacher', 'assignments-teacher@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'teacher'),
  (992002, 'Assignments Student', 'assignments-student@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'student');

-- Course + enrollments: the teacher drives the index/gradebook journeys;
-- the student drives the grades page and the text-entry submit flow.
INSERT INTO courses (id, name, course_code, workflow_state) VALUES
  (992101, 'Smoke Assignments Course', 'SMOKE-ASGN-101', 'available');

INSERT INTO enrollments (id, user_id, course_id, role) VALUES
  (992111, 992001, 992101, 'teacher'),
  (992112, 992002, 992101, 'student');

-- Assignment groups: "Essays" (position 1) and "Quizzes" (position 2).
INSERT INTO assignment_groups (id, course_id, name, position) VALUES
  (992021, 992101, 'Smoke Essays', 1),
  (992022, 992101, 'Smoke Quizzes', 2);

-- Three assignments:
--  * Essay One (published, 10 pts, due date, text-entry submission seeded)
--  * Essay Two (published, 20 pts, no due date)
--  * Draft Quiz (UNPUBLISHED — teacher-only styling on the index)
INSERT INTO assignments
  (id, course_id, assignment_group_id, title, description, points_possible,
   due_at, workflow_state) VALUES
  (992031, 992101, 992021, 'Smoke Essay One',
   '<p>Write about the <strong>smoke journey</strong>.</p>', 10,
   '2026-12-01 17:00:00+00', 'published'),
  (992032, 992101, 992021, 'Smoke Essay Two',
   NULL, 20, NULL, 'published'),
  (992033, 992101, 992022, 'Smoke Draft Quiz',
   NULL, 5, NULL, 'unpublished');

-- One submitted + graded text submission for the student on Essay One:
-- grades page shows the score, gradebook cell shows 8.
INSERT INTO submissions
  (id, assignment_id, user_id, body, submission_type, score, workflow_state) VALUES
  (992041, 992031, 992002, 'My smoke essay text', 'online_text_entry', 8, 'graded');

-- Re-base every touched sequence past the sentinel ids so later inserts
-- (the journeys create new assignments and submissions) never collide.
SELECT setval('users_id_seq',             (SELECT GREATEST(MAX(id), 992002) FROM users));
SELECT setval('courses_id_seq',           (SELECT GREATEST(MAX(id), 992101) FROM courses));
SELECT setval('enrollments_id_seq',       (SELECT GREATEST(MAX(id), 992112) FROM enrollments));
SELECT setval('assignment_groups_id_seq', (SELECT GREATEST(MAX(id), 992022) FROM assignment_groups));
SELECT setval('assignments_id_seq',       (SELECT GREATEST(MAX(id), 992033) FROM assignments));
SELECT setval('submissions_id_seq',       (SELECT GREATEST(MAX(id), 992041) FROM submissions));

COMMIT;
