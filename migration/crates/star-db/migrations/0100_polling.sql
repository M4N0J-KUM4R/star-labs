-- 0100_polling.sql — polling domain tables (issue #89, spec specs/p5-rest-polling.md §5).
--
-- Canvas reference tables: polling_polls, polling_poll_choices,
-- polling_poll_sessions, polling_poll_submissions (app/models/polling/*.rb).
-- The canvas-schema snapshot (0225) carries unused Canvas-named copies; this
-- migration creates the star-lms domain tables per the spec, mirroring the
-- one-crate-per-domain layout used by every other feature wave.
--
-- Session state (spec §8 Q1, resolved against upstream): Canvas stores a
-- boolean `is_published` on polling_poll_sessions (`publish!`/`close!` set it
-- true/false; the `opened`/`closed` scopes filter on it), NOT a workflow_state
-- string. `is_published = false` is the initial state (a session must be
-- opened via GET .../open before students may submit).
--
-- poll_submissions uniqueness (spec §8 Q2, resolved): upstream PollSubmission
-- validates uniqueness of user_id scoped to poll_session_id ("can only submit
-- one choice per poll session."), so re-submitting REJECTS with 400 — the
-- UNIQUE constraint below enforces the same at the storage layer.

CREATE TABLE IF NOT EXISTS polls (
    id          BIGSERIAL PRIMARY KEY,
    user_id     BIGINT NOT NULL REFERENCES users(id),
    question    TEXT NOT NULL,
    description TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_polls_user ON polls(user_id);

CREATE TABLE IF NOT EXISTS poll_choices (
    id         BIGSERIAL PRIMARY KEY,
    poll_id    BIGINT NOT NULL REFERENCES polls(id) ON DELETE CASCADE,
    text       TEXT NOT NULL,
    is_correct BOOLEAN NOT NULL DEFAULT false,
    position   INTEGER,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_poll_choices_poll ON poll_choices(poll_id);

CREATE TABLE IF NOT EXISTS poll_sessions (
    id                 BIGSERIAL PRIMARY KEY,
    poll_id            BIGINT NOT NULL REFERENCES polls(id) ON DELETE CASCADE,
    course_id          BIGINT REFERENCES courses(id),
    course_section_id  BIGINT REFERENCES course_sections(id),
    has_public_results BOOLEAN NOT NULL DEFAULT false,
    is_published       BOOLEAN NOT NULL DEFAULT false,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_poll_sessions_poll ON poll_sessions(poll_id);
CREATE INDEX IF NOT EXISTS idx_poll_sessions_course ON poll_sessions(course_id);
CREATE INDEX IF NOT EXISTS idx_poll_sessions_course_section ON poll_sessions(course_section_id);

CREATE TABLE IF NOT EXISTS poll_submissions (
    id              BIGSERIAL PRIMARY KEY,
    poll_id         BIGINT NOT NULL REFERENCES polls(id) ON DELETE CASCADE,
    poll_session_id BIGINT NOT NULL REFERENCES poll_sessions(id) ON DELETE CASCADE,
    poll_choice_id  BIGINT NOT NULL REFERENCES poll_choices(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL REFERENCES users(id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (poll_session_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_poll_submissions_poll ON poll_submissions(poll_id);
CREATE INDEX IF NOT EXISTS idx_poll_submissions_poll_choice ON poll_submissions(poll_choice_id);
