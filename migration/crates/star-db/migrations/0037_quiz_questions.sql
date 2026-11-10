-- Migration 0037: quiz questions, question groups and question banks.
-- Canvas parity: Quizzes::QuizQuestion (quiz_questions), Quizzes::QuizGroup
-- (quiz_groups) and AssessmentQuestionBank (assessment_question_banks).
-- `answers` holds the serialized question answers array (Canvas stores the
-- whole question payload as serialized `question_data`).
-- Deleting a group deletes its questions (Canvas has dependent: :destroy).

CREATE TABLE IF NOT EXISTS question_banks (
    id             BIGSERIAL PRIMARY KEY,
    context_type   TEXT,
    context_id     BIGINT,
    title          TEXT,
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS quiz_question_groups (
    id                           BIGSERIAL PRIMARY KEY,
    quiz_id                      BIGINT NOT NULL REFERENCES quizzes(id) ON DELETE CASCADE,
    name                         TEXT,
    pick_count                   INTEGER NOT NULL DEFAULT 0,
    question_points              NUMERIC(10,2),
    assessment_question_bank_id  BIGINT REFERENCES question_banks(id) ON DELETE SET NULL,
    position                     INTEGER NOT NULL DEFAULT 0,
    created_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS quiz_questions (
    id                           BIGSERIAL PRIMARY KEY,
    quiz_id                      BIGINT NOT NULL REFERENCES quizzes(id) ON DELETE CASCADE,
    quiz_group_id                BIGINT REFERENCES quiz_question_groups(id) ON DELETE CASCADE,
    assessment_question_bank_id  BIGINT REFERENCES question_banks(id) ON DELETE SET NULL,
    position                     INTEGER NOT NULL DEFAULT 0,
    question_name                TEXT NOT NULL DEFAULT 'Question',
    question_type                TEXT NOT NULL DEFAULT 'text_only_question',
    question_text                TEXT NOT NULL DEFAULT '',
    points_possible              NUMERIC(10,2) NOT NULL DEFAULT 0,
    correct_comments             TEXT,
    incorrect_comments           TEXT,
    neutral_comments             TEXT,
    answers                      TEXT NOT NULL DEFAULT '[]',
    workflow_state               TEXT NOT NULL DEFAULT 'active',
    created_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_question_banks_context
    ON question_banks(context_type, context_id);
CREATE INDEX IF NOT EXISTS idx_quiz_question_groups_quiz
    ON quiz_question_groups(quiz_id);
CREATE INDEX IF NOT EXISTS idx_quiz_questions_quiz
    ON quiz_questions(quiz_id);
CREATE INDEX IF NOT EXISTS idx_quiz_questions_quiz_group
    ON quiz_questions(quiz_group_id);
