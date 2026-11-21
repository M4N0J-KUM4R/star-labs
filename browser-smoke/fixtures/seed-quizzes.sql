-- Browser-smoke fixture for the quiz-taking journey (issue #109).
-- Applied by .github/workflows/browser-smoke.yml in bash glob order, where
-- seed-quizzes.sql runs BEFORE seed.sql ('-' < '.'): this file is therefore
-- FULLY SELF-SUFFICIENT (own users, own course, own enrollment, own quiz +
-- questions, own setvals, deletes-first idempotency for ITS OWN 9940xx
-- sentinels only). It does not depend on — and does not modify — seed.sql.
--
-- Column lists mirror migration/crates/star-db/migrations/0037_quiz_questions.sql
-- (answers: TEXT JSON array of {id, answer_text, answer_weight}, weight 100
-- marks the correct option) and the quiz_submissions columns of 0006/0068.

BEGIN;

-- Own sentinels only (deletes-first idempotency).
DELETE FROM quiz_submissions WHERE quiz_id IN (SELECT id FROM quizzes WHERE course_id = 994101);
DELETE FROM quiz_questions  WHERE quiz_id IN (SELECT id FROM quizzes WHERE course_id = 994101);
DELETE FROM quizzes         WHERE course_id = 994101;
DELETE FROM enrollments     WHERE user_id IN (994001, 994002) OR course_id = 994101;
DELETE FROM courses         WHERE id = 994101;
DELETE FROM users           WHERE id IN (994001, 994002);

-- Users: own teacher + student, argon2id hash for "SmokeTest-123!" (same
-- PHC string seed.sql uses — Rust argon2 crate defaults).
INSERT INTO users (id, name, email, password_hash, role) VALUES
  (994001, 'Quiz Smoke Teacher', 'quiz-teacher@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'teacher'),
  (994002, 'Quiz Smoke Student', 'quiz-student@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'student');

-- Own course + enrollment (teacher AND student — the journeys need both).
INSERT INTO courses (id, name, course_code, workflow_state) VALUES
  (994101, 'Smoke Quiz Course', 'SMOKE-941', 'available');

INSERT INTO enrollments (id, user_id, course_id, role) VALUES
  (994201, 994001, 994101, 'teacher'),
  (994202, 994002, 994101, 'student');

-- Published quiz with 3 questions: multiple_choice (4 answers, ids 1-4,
-- id 1 correct), true_false (ids 11/12, 11 correct), essay (no answers).
-- Quiz points (30) = sum of question points (10 each).
INSERT INTO quizzes (id, course_id, title, description, quiz_type, points_possible, published)
VALUES (994301, 994101, 'Smoke Journey Quiz',
        '<p>Journey quiz instructions.</p>', 'assignment', 30, true);

INSERT INTO quiz_questions
  (id, quiz_id, position, question_name, question_type, question_text, points_possible, answers) VALUES
  (994401, 994301, 1, 'MC', 'multiple_choice_question',
   '<p>Which answer is correct?</p>', 10,
   '[{"id": 1, "answer_text": "The correct one", "answer_weight": 100},
     {"id": 2, "answer_text": "Wrong A", "answer_weight": 0},
     {"id": 3, "answer_text": "Wrong B", "answer_weight": 0},
     {"id": 4, "answer_text": "Wrong C", "answer_weight": 0}]'),
  (994402, 994301, 2, 'TF', 'true_false_question',
   '<p>The sky is blue.</p>', 10,
   '[{"id": 11, "answer_text": "True", "answer_weight": 100},
     {"id": 12, "answer_text": "False", "answer_weight": 0}]'),
  (994403, 994301, 3, 'Essay', 'essay_question',
   '<p>Explain your reasoning.</p>', 10, '[]');

-- Re-base every touched sequence past the sentinel ids.
SELECT setval('users_id_seq',          (SELECT GREATEST(MAX(id), 994002) FROM users));
SELECT setval('courses_id_seq',        (SELECT GREATEST(MAX(id), 994101) FROM courses));
SELECT setval('enrollments_id_seq',    (SELECT GREATEST(MAX(id), 994202) FROM enrollments));
SELECT setval('quizzes_id_seq',        (SELECT GREATEST(MAX(id), 994301) FROM quizzes));
SELECT setval('quiz_questions_id_seq', (SELECT GREATEST(MAX(id), 994403) FROM quiz_questions));
SELECT setval('quiz_submissions_id_seq',
              (SELECT GREATEST(COALESCE(MAX(id), 0), 994501) FROM quiz_submissions));

COMMIT;
