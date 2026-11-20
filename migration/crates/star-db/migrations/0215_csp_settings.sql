-- Migration 0215: Content Security Policy settings.
--
-- Canvas `CspSettingsController` (app/controllers/csp_settings_controller.rb)
-- with `Csp::AccountHelper` / `Csp::CourseHelper`:
--
--   GET    /api/v1/courses/:course_id/csp_settings
--   GET    /api/v1/accounts/:account_id/csp_settings
--   PUT    /api/v1/courses/:course_id/csp_settings
--   PUT    /api/v1/accounts/:account_id/csp_settings
--   PUT    /api/v1/accounts/:account_id/csp_settings/lock
--   POST   /api/v1/accounts/:account_id/csp_settings/domains
--   POST   /api/v1/accounts/:account_id/csp_settings/domains/batch_create
--   DELETE /api/v1/accounts/:account_id/csp_settings/domains
--
-- Canvas stores the account state in the `settings` hash under
-- `csp_inherited_data`:
--
--   {"value": [enabled, account_id], "locked": true|false}
--
-- where `value[0]` says whether CSP is on and `value[1]` is the account that
-- set it.  The setting is inheritable, so a sub-account without its own
-- entry resolves to the nearest ancestor that has one; `csp_inherited?` is
-- true when the account that set the value is not the account itself.
-- `accounts.settings` already exists (migration 0058), so no column is
-- added for the account state.
--
-- A course keeps only a `csp_disabled` flag: the course is CSP-enabled when
-- its account is enabled and the course has not disabled CSP, and the course
-- inherits the account's lock.
--
-- Allowed domains live in `csp_domains`, one row per account and domain.
-- Canvas soft deletes them (`Canvas::SoftDeletable`), and re-adding a
-- deleted domain revives the row, so the unique index spans the whole table
-- and the upsert reactivates instead of inserting a second row.

ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS csp_disabled BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE IF NOT EXISTS csp_domains (
    id             BIGSERIAL PRIMARY KEY,
    account_id     BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    domain         TEXT NOT NULL,
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_csp_domains_account_domain
    ON csp_domains (account_id, domain);

CREATE INDEX IF NOT EXISTS idx_csp_domains_account_state
    ON csp_domains (account_id, workflow_state);
