-- 0244: quizzes-family parity gaps (quiz_extensions, quiz_ip_filters).
-- Canvas Quizzes::QuizExtension (extend_attempts / extend_from /
-- extend_to per user+quiz) and the quiz IP filters used by
-- Quizzes::QuizIpFiltersController. quiz_submission_events already exists
-- (0225); quizzes lack ip_filter / access_code columns and quiz_submissions
-- lack attempts / manually_scored / override columns these routes need.
CREATE TABLE IF NOT EXISTS quiz_extensions (
    id               BIGSERIAL PRIMARY KEY,
    quiz_id          BIGINT NOT NULL REFERENCES quizzes(id) ON DELETE CASCADE,
    user_id          BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    extra_attempts   INTEGER NOT NULL DEFAULT 0,
    extra_time       INTEGER NOT NULL DEFAULT 0,
    extend_from      TIMESTAMPTZ,
    extend_to        TIMESTAMPTZ,
    manually_unlocked BOOLEAN NOT NULL DEFAULT FALSE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_quiz_extension UNIQUE (quiz_id, user_id)
);

CREATE TABLE IF NOT EXISTS quiz_ip_filters (
    id          BIGSERIAL PRIMARY KEY,
    quiz_id     BIGINT NOT NULL REFERENCES quizzes(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    filter      TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_quiz_ip_filters_quiz ON quiz_ip_filters (quiz_id);

ALTER TABLE quizzes
    ADD COLUMN IF NOT EXISTS ip_filter TEXT,
    ADD COLUMN IF NOT EXISTS access_code TEXT,
    ADD COLUMN IF NOT EXISTS one_question_at_a_time BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS could_be_locked BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE quiz_submissions
    ADD COLUMN IF NOT EXISTS attempt INTEGER NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'untaken',
    ADD COLUMN IF NOT EXISTS manually_scored BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS score_locked BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS overdue_and_unlocked BOOLEAN NOT NULL DEFAULT FALSE;
