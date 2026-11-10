-- Migration 0049: assignment_group_rules
-- Canvas grading rules for assignment groups (group weighting and drop rules).
-- Rules are stored one row per group; the `group_weight` itself stays on
-- assignment_groups (Canvas AssignmentGroup#group_weight).

CREATE TABLE IF NOT EXISTS assignment_group_rules (
    id                  BIGSERIAL PRIMARY KEY,
    assignment_group_id BIGINT NOT NULL UNIQUE REFERENCES assignment_groups(id) ON DELETE CASCADE,
    drop_lowest         INT NOT NULL DEFAULT 0,
    drop_highest        INT NOT NULL DEFAULT 0,
    never_drop          BIGINT[] NOT NULL DEFAULT ARRAY[]::BIGINT[],
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The UNIQUE constraint on assignment_group_id already provides the lookup index.
