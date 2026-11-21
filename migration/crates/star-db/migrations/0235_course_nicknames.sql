-- Migration 0235: the course nicknames.
--
-- Canvas keeps a course nickname in the `user_preference_values` rows, under
-- the key `course_nicknames` and the course id as the sub key
-- (`app/models/course_nickname.rb`):
--
--   user.preference_value = {
--     "course_nicknames" => { "42" => "Nickname" }
--   }
--
-- The workspace has no preference-value table, so the nicknames get their
-- own table with one row per user and course.

CREATE TABLE IF NOT EXISTS course_nicknames (
    id         BIGSERIAL PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    course_id  BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    nickname   TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, course_id)
);

CREATE INDEX IF NOT EXISTS idx_course_nicknames_user ON course_nicknames (user_id);
