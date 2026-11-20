-- Browser-smoke fixture for the star-ui auth journey (issue #70).
-- Idempotent: sentinel rows are deleted first and re-inserted with explicit
-- sentinel ids (9910xx — a disjoint range from seed.sql's 9900xx), and every
-- touched sequence is re-based afterwards. Does NOT touch seed.sql's rows.

BEGIN;

-- Children first for the FK (password_reset_tokens -> users).
DELETE FROM password_reset_tokens WHERE user_id = 991001;
DELETE FROM users WHERE id = 991001;

-- One user for the auth journey: hash for "SmokeTest-123!" (argon2id,
-- v=19, m=19456, t=2, p=1 — same parameters star-auth::hash_password
-- produces, so it verifies through the normal login path).
INSERT INTO users (id, name, email, password_hash, role) VALUES
  (991001, 'Auth Smoke User', 'auth-smoke@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'student');

SELECT setval('users_id_seq', (SELECT GREATEST(MAX(id), 991001) FROM users));

COMMIT;
