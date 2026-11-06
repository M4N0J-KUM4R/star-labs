-- Migration 0022: learning outcomes, groups, links, and results.
-- Canvas parity: learning_outcomes, learning_outcome_groups, learning_outcome_links,
-- learning_outcome_results tables.

CREATE TABLE IF NOT EXISTS learning_outcomes (
    id              BIGSERIAL PRIMARY KEY,
    title           TEXT NOT NULL,
    description     TEXT,
    display_name    TEXT,
    workflow_state  TEXT NOT NULL DEFAULT 'active',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS learning_outcome_groups (
    id              BIGSERIAL PRIMARY KEY,
    course_id       BIGINT REFERENCES courses(id) ON DELETE SET NULL,
    title           TEXT NOT NULL,
    workflow_state  TEXT NOT NULL DEFAULT 'active',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- outcome can be linked to a group; unique constraint prevents duplicate links
CREATE TABLE IF NOT EXISTS learning_outcome_links (
    id               BIGSERIAL PRIMARY KEY,
    outcome_id       BIGINT NOT NULL REFERENCES learning_outcomes(id) ON DELETE CASCADE,
    outcome_group_id BIGINT NOT NULL REFERENCES learning_outcome_groups(id) ON DELETE CASCADE,
    workflow_state   TEXT NOT NULL DEFAULT 'active',
    UNIQUE (outcome_id, outcome_group_id)
);

CREATE TABLE IF NOT EXISTS learning_outcome_results (
    id          BIGSERIAL PRIMARY KEY,
    outcome_id  BIGINT NOT NULL REFERENCES learning_outcomes(id) ON DELETE CASCADE,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    score       FLOAT8 NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_learning_outcome_groups_course ON learning_outcome_groups(course_id);
CREATE INDEX idx_learning_outcome_links_outcome ON learning_outcome_links(outcome_id);
CREATE INDEX idx_learning_outcome_links_group  ON learning_outcome_links(outcome_group_id);
CREATE INDEX idx_learning_outcome_results_outcome ON learning_outcome_results(outcome_id);
CREATE INDEX idx_learning_outcome_results_user  ON learning_outcome_results(user_id);
