-- Migration 0068: Canvas quiz submission fields and quiz report records.
-- Canvas parity:
-- - Quizzes::QuizSubmission (quiz_submissions): attempt bookkeeping, timing,
--   manual scoring (fudge_points / kept_score), workflow_state and the
--   serialized submission_data payload (Canvas stores text).
-- - Quizzes::QuizStatistics (quiz_statistics): the row behind the
--   "QuizReport" API (report_type student_analysis | item_analysis).
-- Existing rows keep working: every new column is nullable or defaulted.

ALTER TABLE quiz_submissions
    ADD COLUMN IF NOT EXISTS submission_id       BIGINT,
    ADD COLUMN IF NOT EXISTS finished_at         TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS end_at              TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS attempt             INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS extra_attempts      INTEGER,
    ADD COLUMN IF NOT EXISTS extra_time          INTEGER,
    ADD COLUMN IF NOT EXISTS manually_unlocked   BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS time_spent          INTEGER,
    ADD COLUMN IF NOT EXISTS fudge_points        NUMERIC(10,2),
    ADD COLUMN IF NOT EXISTS kept_score          NUMERIC(10,2),
    ADD COLUMN IF NOT EXISTS score_before_regrade NUMERIC(10,2),
    ADD COLUMN IF NOT EXISTS has_seen_results    BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS workflow_state      TEXT NOT NULL DEFAULT 'untaken',
    ADD COLUMN IF NOT EXISTS submission_data     TEXT NOT NULL DEFAULT '{}';

CREATE TABLE IF NOT EXISTS quiz_statistics (
    id                    BIGSERIAL PRIMARY KEY,
    quiz_id               BIGINT NOT NULL REFERENCES quizzes(id) ON DELETE CASCADE,
    report_type           TEXT NOT NULL,
    includes_all_versions BOOLEAN NOT NULL DEFAULT false,
    includes_sis_ids      BOOLEAN NOT NULL DEFAULT false,
    anonymous             BOOLEAN NOT NULL DEFAULT false,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_quiz_statistics_quiz_report
    ON quiz_statistics(quiz_id, report_type);
