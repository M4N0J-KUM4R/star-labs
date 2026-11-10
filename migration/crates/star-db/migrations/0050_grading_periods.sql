-- Migration 0050: grading periods (Canvas API v1 parity)
--   GET    /accounts/{accountId}/grading_periods
--   POST   /accounts/{accountId}/grading_periods
--   GET    /grading_periods/{id}
--   PUT    /grading_periods/{id}
--   DELETE /grading_periods/{id}  -> soft-delete (workflow_state='deleted')
--
-- Mirrors Canvas `GradingPeriodGroup` + `GradingPeriod`. An account-level set
-- groups the periods for an account; each period carries the Canvas serializer
-- fields (id, grading_period_group_id, title, start_date, end_date, close_date,
-- weight) plus the soft-deletable `workflow_state`.

CREATE TABLE IF NOT EXISTS grading_period_groups (
    id             BIGSERIAL PRIMARY KEY,
    account_id     BIGINT REFERENCES accounts(id) ON DELETE CASCADE,
    course_id      BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    title          TEXT,
    weighted       BOOLEAN NOT NULL DEFAULT FALSE,
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_grading_period_groups_account_id
    ON grading_period_groups (account_id)
    WHERE workflow_state = 'active';

CREATE INDEX IF NOT EXISTS idx_grading_period_groups_course_id
    ON grading_period_groups (course_id)
    WHERE workflow_state = 'active';

CREATE TABLE IF NOT EXISTS grading_periods (
    id                      BIGSERIAL PRIMARY KEY,
    grading_period_group_id BIGINT NOT NULL REFERENCES grading_period_groups(id) ON DELETE CASCADE,
    title                   TEXT NOT NULL,
    start_date              TIMESTAMPTZ NOT NULL,
    end_date                TIMESTAMPTZ NOT NULL,
    -- Grades can only be changed before close_date.
    close_date              TIMESTAMPTZ NOT NULL,
    -- Weight within a weighted grading period set (NULL when unweighted).
    weight                  FLOAT8,
    workflow_state          TEXT NOT NULL DEFAULT 'active',
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_grading_periods_group_id
    ON grading_periods (grading_period_group_id, start_date);

CREATE INDEX IF NOT EXISTS idx_grading_periods_active_dates
    ON grading_periods (start_date, end_date)
    WHERE workflow_state = 'active';
