-- Migration 0006: quiz metadata (NOT a question engine).
-- star-quizzes: quiz records + one submission row per (quiz, user).

CREATE TABLE IF NOT EXISTS quizzes (
    id               BIGSERIAL PRIMARY KEY,
    course_id        BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    title            TEXT NOT NULL,
    description      TEXT,
    quiz_type        TEXT NOT NULL DEFAULT 'assignment',
    time_limit       INTEGER,
    shuffle_questions BOOLEAN NOT NULL DEFAULT false,
    available_from   TIMESTAMPTZ,
    available_until  TIMESTAMPTZ,
    points_possible  NUMERIC(10,2) NOT NULL DEFAULT 0,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS quiz_submissions (
    id           BIGSERIAL PRIMARY KEY,
    quiz_id      BIGINT NOT NULL REFERENCES quizzes(id) ON DELETE CASCADE,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    score        NUMERIC(10,2),
    completed_at TIMESTAMPTZ,
    started_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (quiz_id, user_id)
);

CREATE INDEX idx_quizzes_course ON quizzes(course_id);
CREATE INDEX idx_quiz_submissions_quiz ON quiz_submissions(quiz_id);
CREATE INDEX idx_quiz_submissions_user ON quiz_submissions(user_id);
