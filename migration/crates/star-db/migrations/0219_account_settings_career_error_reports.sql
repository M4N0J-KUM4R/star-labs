-- Migration 0219: the account settings columns, the career experience tables
-- and the error report table.
--
-- Canvas `Account` carries a uuid and a default time zone, and
-- `Account#settings` holds the settings the new account routes read
-- (`horizon_account_ids` for the career feature, `ip_filters` for the quiz IP
-- filters).

ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS uuid TEXT;

ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS default_time_zone TEXT NOT NULL DEFAULT 'UTC';

-- The storage quotas of `account_json`.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS default_storage_quota_mb INTEGER;

ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS default_user_storage_quota_mb INTEGER;

ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS default_group_storage_quota_mb INTEGER;

-- Canvas tracks the state of an account membership.
ALTER TABLE account_users
    ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'active';

-- Canvas assigns a course to an enrollment term.
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS enrollment_term_id BIGINT;

-- Canvas tracks the deletion of a user in `workflow_state`; the port only
-- knew the registration.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'registered';

-- Canvas `AccessibilityCourseStatistics`: the issue counters of the
-- accessibility checker, one row per course.
CREATE TABLE IF NOT EXISTS accessibility_course_statistics (
    id                   BIGSERIAL PRIMARY KEY,
    course_id            BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    workflow_state       TEXT NOT NULL DEFAULT 'active',
    active_issue_count   INTEGER NOT NULL DEFAULT 0,
    resolved_issue_count INTEGER NOT NULL DEFAULT 0,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (course_id)
);

-- Canvas `CanvasCareer::UserExperience`: the career experience a user
-- created for a root account.
CREATE TABLE IF NOT EXISTS canvas_career_user_experiences (
    id               BIGSERIAL PRIMARY KEY,
    user_id          BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    root_account_id  BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    workflow_state   TEXT NOT NULL DEFAULT 'active',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, root_account_id)
);

-- Canvas `ErrorReport`: a report of a problem a user ran into.
CREATE TABLE IF NOT EXISTS error_reports (
    id             BIGSERIAL PRIMARY KEY,
    user_id        BIGINT REFERENCES users(id) ON DELETE SET NULL,
    account_id     BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    subject        TEXT,
    url            TEXT,
    email          TEXT,
    comments       TEXT,
    description    TEXT,
    backtrace      TEXT,
    user_agent     TEXT,
    http_env       JSONB,
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_error_reports_user ON error_reports (user_id);
