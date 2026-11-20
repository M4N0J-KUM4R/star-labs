-- Browser-smoke fixture for the star-ui shell (specs/p5-ui-shell.md §4/§6).
-- Run with: psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f browser-smoke/fixtures/seed.sql
--
-- Idempotent: sentinel rows are deleted first and re-inserted with explicit
-- sentinel ids (9900xx), and every touched sequence is re-based afterwards,
-- so local re-runs never collide with rows created between runs.
--
-- Column lists mirror migration/crates/star-ui/tests/ui_integration.rs
-- exactly (same tables, same sentinel-id + setval discipline).

BEGIN;

-- Children first for the FKs (enrollments -> users/courses, pages/modules -> courses).
DELETE FROM enrollments WHERE user_id IN (990001, 990002) OR course_id IN (990101, 990102);
DELETE FROM pages     WHERE course_id IN (990101, 990102);
DELETE FROM modules   WHERE course_id IN (990101, 990102);
DELETE FROM courses   WHERE id IN (990101, 990102);
DELETE FROM users     WHERE id IN (990001, 990002);

-- Users. password_hash is an argon2id PHC string for "SmokeTest-123!" using
-- the Rust argon2 crate defaults (v=19, m=19456, t=2, p=1) — the same
-- parameters star-auth::hash_password produces, so the hash verifies through
-- the normal login path. Seeded via SQL because the public
-- POST /api/v1/auth/register endpoint only allows role=student and the
-- journeys need a teacher.
INSERT INTO users (id, name, email, password_hash, role) VALUES
  (990001, 'Smoke Teacher', 'smoke-teacher@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'teacher'),
  (990002, 'Smoke Student', 'smoke-student@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'student');

-- Courses: both published ('available'). Course 1 gets a front page (wiki
-- branch of course home); course 2 has none, so its home renders the modules
-- branch.
INSERT INTO courses (id, name, course_code, workflow_state) VALUES
  (990101, 'Smoke Course Alpha', 'SMOKE-101', 'available'),
  (990102, 'Smoke Course Beta',  'SMOKE-102', 'available');

-- The teacher is enrolled in both courses (dashboard cards + course access);
-- the student stays deliberately unenrolled everywhere for the
-- access-control journey.
INSERT INTO enrollments (id, user_id, course_id, role) VALUES
  (990201, 990001, 990101, 'teacher'),
  (990202, 990001, 990102, 'teacher');

-- Course 1 front page: the body carries identifiable text the journey asserts.
INSERT INTO pages (id, course_id, title, body, url, front_page) VALUES
  (990301, 990101, 'Welcome',
   '<h2>Welcome to Smoke Course</h2><p>Front page body served by the browser-smoke fixture.</p>',
   'front-page', true);

-- Course 2 module: the modules branch of course home.
INSERT INTO modules (id, course_id, name, position, workflow_state) VALUES
  (990401, 990102, 'Smoke Module One', 1, 'active');

-- Re-base every touched sequence past the sentinel ids so later inserts
-- (registrations, new courses) never collide with the fixture.
SELECT setval('users_id_seq',       (SELECT GREATEST(MAX(id), 990002) FROM users));
SELECT setval('courses_id_seq',     (SELECT GREATEST(MAX(id), 990102) FROM courses));
SELECT setval('enrollments_id_seq', (SELECT GREATEST(MAX(id), 990202) FROM enrollments));
SELECT setval('pages_id_seq',       (SELECT GREATEST(MAX(id), 990301) FROM pages));
SELECT setval('modules_id_seq',     (SELECT GREATEST(MAX(id), 990401) FROM modules));

COMMIT;
