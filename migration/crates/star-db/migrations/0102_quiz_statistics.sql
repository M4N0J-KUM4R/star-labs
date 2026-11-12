-- Migration 0102: quiz statistics deep parity
-- - Adds access_code column to quizzes table (Canvas has quiz-level access code)
-- - Creates quiz_statistics table for quiz statistics reports

ALTER TABLE quizzes ADD COLUMN IF NOT EXISTS access_code TEXT;

-- Canvas' quiz_statistics table (stores pre-computed aggregate reports)
CREATE TABLE IF NOT EXISTS quiz_statistics (
    id               BIGSERIAL PRIMARY KEY,
    quiz_id          BIGINT NOT NULL REFERENCES quizzes(id) ON DELETE CASCADE,
    report_type      TEXT NOT NULL,  -- 'student_analysis' | 'item_analysis'
    includes_all_versions BOOLEAN NOT NULL DEFAULT false,
    includes_sis_ids    BOOLEAN NOT NULL DEFAULT false,
    anonymous        BOOLEAN NOT NULL DEFAULT false,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_quiz_statistics_quiz ON quiz_statistics(quiz_id);
CREATE INDEX IF NOT EXISTS idx_quiz_statistics_quiz_type ON quiz_statistics(quiz_id, report_type);
