-- Migration 0015: Canvas quiz parity (app/models/quizzes/quiz.rb).
-- - rename shuffle_questions -> shuffle_answers (Canvas field name)
-- - allowed_attempts: number of allowed attempts (Canvas default -1 = unlimited)
-- - published: Canvas exposes published? as a boolean workflow equivalent
ALTER TABLE quizzes RENAME COLUMN shuffle_questions TO shuffle_answers;
ALTER TABLE quizzes
    ADD COLUMN IF NOT EXISTS allowed_attempts INTEGER NOT NULL DEFAULT 1,
    ADD COLUMN IF NOT EXISTS published BOOLEAN NOT NULL DEFAULT false;
