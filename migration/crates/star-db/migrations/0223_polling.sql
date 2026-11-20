-- Migration 0223: the polling tables.
--
-- Canvas `Polling::Poll` is a question with its choices, `Polling::PollSession`
-- is one session of a poll in a course, and `Polling::PollSubmission` is the
-- answer of a user.

CREATE TABLE IF NOT EXISTS polls (
    id           BIGSERIAL PRIMARY KEY,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    course_id    BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    question     TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS poll_choices (
    id           BIGSERIAL PRIMARY KEY,
    poll_id      BIGINT NOT NULL REFERENCES polls(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    description  TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS poll_sessions (
    id                 BIGSERIAL PRIMARY KEY,
    poll_id            BIGINT NOT NULL REFERENCES polls(id) ON DELETE CASCADE,
    course_id          BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    course_section_id  BIGINT REFERENCES course_sections(id) ON DELETE SET NULL,
    has_public_results BOOLEAN NOT NULL DEFAULT false,
    is_published       BOOLEAN NOT NULL DEFAULT false,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_poll_sessions_poll ON poll_sessions (poll_id);

CREATE TABLE IF NOT EXISTS poll_submissions (
    id              BIGSERIAL PRIMARY KEY,
    poll_session_id BIGINT NOT NULL REFERENCES poll_sessions(id) ON DELETE CASCADE,
    poll_choice_id  BIGINT NOT NULL REFERENCES poll_choices(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_poll_submissions_session ON poll_submissions (poll_session_id);
