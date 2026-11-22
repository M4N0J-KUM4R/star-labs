-- Browser-smoke fixture for the SpeedGrader grading journey (issue #108).
-- Applied by the workflow's glob loop BEFORE seed.sql ('-' 0x2D sorts before
-- '.' 0x2E, so seed-grading.sql runs first), which is why this fragment
-- is FULLY SELF-SUFFICIENT and order-independent:
--   * own sentinel users 996001/996002, course 996101, enrollments
--     996111/996112, assignment group 996021, assignment 996031,
--     submission 996041, rubric 996051, criteria 996061/996062,
--     ratings 996071..996074, rubric association 996081 (all 9960xx —
--     disjoint from seed.sql's 9900xx and the other journey fragments'
--     9910xx/9920xx/9930xx/9940xx);
--   * deletes-first idempotency for its OWN sentinels only;
--   * own SELECT setval(...) re-basing for every touched sequence, computed
--     from GREATEST(MAX(id), sentinel) so it is safe whether this fragment
--     runs before or after seed.sql.
--
-- Password hash: argon2id PHC string for "SmokeTest-123!" (Rust argon2 crate
-- defaults: v=19, m=19456, t=2, p=1) — identical to seed.sql, so the journey
-- logs in through the normal form path.

BEGIN;

-- Children first for the FKs (rubric_assessments -> submissions;
-- rubric_ratings -> rubric_criteria -> rubrics; submissions -> assignments
-- -> assignment_groups -> courses; enrollments -> courses).
DELETE FROM rubric_assessments  WHERE submission_id = 996041 OR rubric_id = 996051;
DELETE FROM submission_comments WHERE submission_id = 996041;
DELETE FROM submissions         WHERE id = 996041;
DELETE FROM rubric_associations WHERE association_id = 996031 AND rubric_id = 996051;
DELETE FROM rubric_ratings      WHERE criterion_id IN (996061, 996062);
DELETE FROM rubric_criteria     WHERE rubric_id = 996051;
DELETE FROM rubrics             WHERE id = 996051;
DELETE FROM assignments         WHERE id = 996031;
DELETE FROM assignment_groups   WHERE id = 996021;
DELETE FROM enrollments         WHERE id IN (996111, 996112);
DELETE FROM courses             WHERE id = 996101;
DELETE FROM users               WHERE id IN (996001, 996002);

-- Users: one teacher, one student, both enrolled in the course below.
INSERT INTO users (id, name, email, password_hash, role) VALUES
  (996001, 'Grading Smoke Teacher', 'grading-teacher@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'teacher'),
  (996002, 'Grading Smoke Student', 'grading-student@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'student');

-- Course + enrollments: the teacher grades, the student is graded.
INSERT INTO courses (id, name, course_code, workflow_state) VALUES
  (996101, 'Smoke Grading Course', 'SMOKE-GRAD-101', 'available');

INSERT INTO enrollments (id, user_id, course_id, role) VALUES
  (996111, 996001, 996101, 'teacher'),
  (996112, 996002, 996101, 'student');

-- One assignment group + one published 10-pt essay assignment with a
-- submitted-but-ungraded text submission from the student.
INSERT INTO assignment_groups (id, course_id, name, position) VALUES
  (996021, 996101, 'Smoke Grading Essays', 1);

INSERT INTO assignments
  (id, course_id, assignment_group_id, title, description, points_possible,
   workflow_state) VALUES
  (996031, 996101, 996021, 'Smoke Grading Essay',
   '<p>Explain the smoke journey.</p>', 10, 'published');

INSERT INTO submissions
  (id, assignment_id, user_id, body, submission_type, workflow_state) VALUES
  (996041, 996031, 996002, 'My smoke grading essay text', 'online_text_entry',
   'submitted');

-- Rubric (10 pts: two 5-pt criteria, each with Full/Partial ratings),
-- associated with the assignment for grading.
INSERT INTO rubrics (id, course_id, title, points_possible) VALUES
  (996051, 996101, 'Smoke Grading Rubric', 10);

INSERT INTO rubric_criteria (id, rubric_id, description, points, position) VALUES
  (996061, 996051, 'Thesis', 5, 1),
  (996062, 996051, 'Evidence', 5, 2);

INSERT INTO rubric_ratings (id, criterion_id, description, points, position) VALUES
  (996071, 996061, 'Clear thesis', 5, 1),
  (996072, 996061, 'Partial thesis', 3, 2),
  (996073, 996062, 'Strong evidence', 5, 1),
  (996074, 996062, 'Some evidence', 2, 2);

INSERT INTO rubric_associations
  (id, rubric_id, association_id, association_type, context_id, context_type, purpose) VALUES
  (996081, 996051, 996031, 'Assignment', 996101, 'Course', 'grading');

-- Re-base every touched sequence past the sentinel ids so later inserts
-- (the journey writes scores/comments/assessments) never collide.
SELECT setval('users_id_seq',              (SELECT GREATEST(MAX(id), 996002) FROM users));
SELECT setval('courses_id_seq',            (SELECT GREATEST(MAX(id), 996101) FROM courses));
SELECT setval('enrollments_id_seq',        (SELECT GREATEST(MAX(id), 996112) FROM enrollments));
SELECT setval('assignment_groups_id_seq',  (SELECT GREATEST(MAX(id), 996021) FROM assignment_groups));
SELECT setval('assignments_id_seq',        (SELECT GREATEST(MAX(id), 996031) FROM assignments));
SELECT setval('submissions_id_seq',        (SELECT GREATEST(MAX(id), 996041) FROM submissions));
SELECT setval('rubrics_id_seq',            (SELECT GREATEST(MAX(id), 996051) FROM rubrics));
SELECT setval('rubric_criteria_id_seq',    (SELECT GREATEST(MAX(id), 996062) FROM rubric_criteria));
SELECT setval('rubric_ratings_id_seq',     (SELECT GREATEST(MAX(id), 996074) FROM rubric_ratings));
SELECT setval('rubric_associations_id_seq',(SELECT GREATEST(MAX(id), 996081) FROM rubric_associations));
SELECT setval('rubric_assessments_id_seq',
              (SELECT GREATEST(COALESCE(MAX(id), 0), 996091) FROM rubric_assessments));
SELECT setval('submission_comments_id_seq',
              (SELECT GREATEST(COALESCE(MAX(id), 0), 996095) FROM submission_comments));

COMMIT;
