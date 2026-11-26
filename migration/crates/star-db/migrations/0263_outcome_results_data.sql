-- 0263: outcome results / rollups runtime columns (issue #86).
--
-- The outcome results domain routes (OutcomeResultsController:
-- GET /courses/:course_id/outcome_results, GET /courses/:course_id/outcome_rollups,
-- GET /courses/:course_id/outcome_mastery_distribution,
-- GET /courses/:course_id/outcomes/:outcome_id/contributing_scores,
-- POST /courses/:course_id/assign_outcome_order and
-- POST /enqueue_outcome_rollup_calculation) read:
--   - learning_outcomes.data      — the rubric criterion JSON
--                                   (points_possible / mastery_points / ratings)
--                                   that rollup scores and the mastery
--                                   distribution buckets are derived from;
--   - learning_outcome_results.*  — the Canvas LearningOutcomeResult columns
--                                   (context_code, the alignment link, the
--                                   hidden flag, possible/percent, mastery,
--                                   workflow_state and the timestamps);
--   - users.uuid                  — POST /enqueue_outcome_rollup_calculation
--                                   addresses a student by uuid.
--
-- All three were documented in the outcomes-api DDL block but had never landed
-- in a migration, so the routes 500'd against a migrations-only database (the
-- integration tests applied the same statements ad hoc). Additive only.

ALTER TABLE learning_outcomes
    ADD COLUMN IF NOT EXISTS data JSONB;

ALTER TABLE learning_outcome_results
    ALTER COLUMN score DROP NOT NULL,
    ADD COLUMN IF NOT EXISTS context_code TEXT,
    ADD COLUMN IF NOT EXISTS content_tag_id BIGINT,
    ADD COLUMN IF NOT EXISTS association_type TEXT,
    ADD COLUMN IF NOT EXISTS association_id BIGINT,
    ADD COLUMN IF NOT EXISTS hidden BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'active',
    ADD COLUMN IF NOT EXISTS possible DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS percent DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS mastery BOOLEAN,
    ADD COLUMN IF NOT EXISTS hide_points BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN IF NOT EXISTS title TEXT,
    ADD COLUMN IF NOT EXISTS submitted_at TIMESTAMP,
    ADD COLUMN IF NOT EXISTS assessed_at TIMESTAMP;

ALTER TABLE users
    ADD COLUMN IF NOT EXISTS uuid TEXT;

CREATE INDEX IF NOT EXISTS idx_learning_outcome_results_context
    ON learning_outcome_results (context_code, user_id);

CREATE UNIQUE INDEX IF NOT EXISTS idx_users_uuid
    ON users (uuid) WHERE uuid IS NOT NULL;
