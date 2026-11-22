-- Browser-smoke fixture for the course settings + people journey (issue #73).
-- Applied by .github/workflows/browser-smoke.yml in bash glob order, where
-- seed-settings.sql runs BEFORE seed.sql ('-' < '.'): this file is therefore
-- FULLY SELF-SUFFICIENT (own users, own course, own enrollments, own
-- setvals, deletes-first idempotency for ITS OWN 9950xx sentinels only). It
-- does not depend on — and does not modify — seed.sql or the other
-- seed-<topic>.sql fragments.

BEGIN;

-- Own sentinels only (deletes-first idempotency).
DELETE FROM enrollments WHERE user_id IN (995001, 995002, 995003) OR course_id = 995101;
DELETE FROM courses         WHERE id = 995101;
DELETE FROM users           WHERE id IN (995001, 995002, 995003);

-- Users: own teacher + student (enrolled) + extra student (Add People
-- target, deliberately unenrolled), argon2id hash for "SmokeTest-123!"
-- (same PHC string seed.sql uses — Rust argon2 crate defaults).
INSERT INTO users (id, name, email, password_hash, role) VALUES
  (995001, 'Settings Smoke Teacher', 'settings-teacher@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'teacher'),
  (995002, 'Settings Smoke Student', 'settings-student@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'student'),
  (995003, 'Settings Smoke Extra', 'settings-extra@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'student');

-- Own course + enrollments (teacher + student — the journeys need both;
-- the extra user stays unenrolled so the teacher can add them).
INSERT INTO courses (id, name, course_code, workflow_state, syllabus_body) VALUES
  (995101, 'Smoke Settings Course', 'SMOKE-951', 'available',
   '<p>Original syllabus body.</p>');

INSERT INTO enrollments (id, user_id, course_id, role, workflow_state) VALUES
  (995201, 995001, 995101, 'teacher', 'active'),
  (995202, 995002, 995101, 'student', 'active');

-- Re-base every touched sequence past the sentinel ids.
SELECT setval('users_id_seq',       (SELECT GREATEST(MAX(id), 995003) FROM users));
SELECT setval('courses_id_seq',     (SELECT GREATEST(MAX(id), 995101) FROM courses));
SELECT setval('enrollments_id_seq', (SELECT GREATEST(MAX(id), 995202) FROM enrollments));

COMMIT;
