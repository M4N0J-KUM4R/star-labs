-- Browser-smoke fixture for the discussions + inbox journey (issue #72).
-- Applied by the workflow's glob loop BEFORE seed.sql ('-' 0x2D sorts before
-- '.' 0x2E, so seed-discussions.sql runs first), which is why this fragment
-- is FULLY SELF-SUFFICIENT and order-independent:
--   * own sentinel users 993001/993002, course 993101, enrollment 993201,
--     topic 993301, entries 993401/993402, conversation 993501,
--     conversation_participants 993601/993602, conversation_message 993701
--     (all 9930xx — disjoint from seed.sql's 9900xx);
--   * deletes-first idempotency for its OWN sentinels only;
--   * own SELECT setval(...) re-basing for every touched sequence, computed
--     from GREATEST(MAX(id), sentinel) so it is safe whether this fragment
--     runs before or after seed.sql.
--
-- Password hash: argon2id PHC string for "SmokeTest-123!" (Rust argon2 crate
-- defaults: v=19, m=19456, t=2, p=1) — identical to seed.sql, so the journey
-- logs in through the normal form path.

BEGIN;

-- Children first for the FKs.
DELETE FROM conversation_messages         WHERE id = 993701;
DELETE FROM conversation_participants     WHERE id IN (993601, 993602);
DELETE FROM conversations                 WHERE id = 993501;
DELETE FROM discussion_entries            WHERE id IN (993401, 993402);
DELETE FROM discussion_topic_reads        WHERE topic_id = 993301;
DELETE FROM discussion_topics             WHERE id = 993301;
DELETE FROM enrollments                   WHERE id = 993201;
DELETE FROM courses                       WHERE id = 993101;
DELETE FROM users                         WHERE id IN (993001, 993002);

-- Users: one teacher, one student, both enrolled in the course below.
INSERT INTO users (id, name, email, password_hash, role) VALUES
  (993001, 'Discussions Teacher', 'discussions-teacher@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'teacher'),
  (993002, 'Discussions Student', 'discussions-student@test.local',
   '$argon2id$v=19$m=19456,t=2,p=1$MDEyMzQ1Njc4OWFiY2RlZg$IJx+mNaWh2ohMfrjZVCHC6My98jfK4Bqd3fagqJ4JEM',
   'student');

-- Course + enrollments: teacher starts the discussion; the student reads it.
INSERT INTO courses (id, name, course_code, workflow_state) VALUES
  (993101, 'Smoke Discussions Course', 'SMOKE-DISC-101', 'available');

INSERT INTO enrollments (id, user_id, course_id, role) VALUES
  (993201, 993001, 993101, 'teacher'),
  (993202, 993002, 993101, 'student');

-- Discussion topic authored by the teacher, with two entries (one root by
-- the teacher, one reply by the student). No discussion_topic_reads rows:
-- the topic starts UNREAD for the student so the journey can assert the
-- unread marker before opening it.
INSERT INTO discussion_topics
  (id, course_id, title, message, discussion_type, workflow_state, pinned, locked, user_id)
VALUES
  (993301, 993101, 'Welcome to the course discussion',
   '<p>Say hello and introduce yourself.</p>', 'side_comment', 'active', FALSE, FALSE, 993001);

INSERT INTO discussion_entries (id, topic_id, parent_id, user_id, message) VALUES
  (993401, 993301, NULL,     993001, 'Teacher: welcome aboard, reply here!'),
  (993402, 993301, 993401,   993002, 'Student: happy to be here.');

-- Teacher <-> student conversation, UNREAD for the teacher (read_at NULL)
-- and read for the student (author of the message). The journey opens it as
-- the teacher (marker clears), replies, and later composes a new one.
INSERT INTO conversations (id, subject, workflow_state) VALUES
  (993501, 'Smoke Inbox Subject', 'active');

INSERT INTO conversation_participants
  (id, conversation_id, user_id, workflow_state, last_message_at, read_at) VALUES
  (993601, 993501, 993001, 'active', now(), NULL),
  (993602, 993501, 993002, 'active', now(), now());

INSERT INTO conversation_messages (id, conversation_id, author_id, body) VALUES
  (993701, 993501, 993002, 'Student to teacher: do we need the textbook?');

-- Re-base every touched sequence past the sentinel ids so later inserts
-- (the journeys create new topics/entries/conversations) never collide.
SELECT setval('users_id_seq',                     (SELECT GREATEST(MAX(id), 993002) FROM users));
SELECT setval('courses_id_seq',                   (SELECT GREATEST(MAX(id), 993101) FROM courses));
SELECT setval('enrollments_id_seq',               (SELECT GREATEST(MAX(id), 993202) FROM enrollments));
SELECT setval('discussion_topics_id_seq',         (SELECT GREATEST(MAX(id), 993301) FROM discussion_topics));
SELECT setval('discussion_entries_id_seq',        (SELECT GREATEST(MAX(id), 993402) FROM discussion_entries));
SELECT setval('discussion_topic_reads_id_seq',    (SELECT GREATEST(MAX(id), 1)     FROM discussion_topic_reads));
SELECT setval('conversations_id_seq',             (SELECT GREATEST(MAX(id), 993501) FROM conversations));
SELECT setval('conversation_participants_id_seq', (SELECT GREATEST(MAX(id), 993602) FROM conversation_participants));
SELECT setval('conversation_messages_id_seq',     (SELECT GREATEST(MAX(id), 993701) FROM conversation_messages));

COMMIT;
