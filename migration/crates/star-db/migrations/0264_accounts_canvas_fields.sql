-- Migration 0240: accounts — the Canvas `accounts` table fields the account
-- API returns (app/models/account.rb, lib/api/v1/account.rb,
-- app/controllers/accounts_controller.rb).
--
-- The `accounts` table itself is created by migration 0024; the canonical
-- definition below documents the complete column set. `schema_debt.py` only
-- reads `CREATE TABLE` statements, so a table that grew through `ALTER
-- TABLE` looks empty to it. On an existing database the statement is a
-- no-op — the ALTERs below are what add the new columns.
--
-- New columns (Canvas source -> JSON key):
--
--   sis_source_id          TEXT    Canvas `accounts.sis_source_id`
--                                   -> `sis_account_id`
--   sis_batch_id           BIGINT  Canvas `accounts.sis_batch_id`
--                                   -> `sis_import_id`
--   integration_id         TEXT    Canvas `accounts.integration_id`
--                                   -> `integration_id`
--   course_template_id     BIGINT  Canvas `accounts.course_template_id`
--                                   -> `course_template_id`
--   self_registration_type TEXT    Canvas `Account#self_registration_type`
--                                   -> `self_registration_type`
--   terms_type             TEXT    Canvas `terms_of_services.terms_type`
--                                   -> `terms_type`
--   passive                BOOLEAN Canvas `terms_of_services.passive`
--                                   -> `passive`
--   content                TEXT    Canvas `terms_of_service_contents.content`
--                                   -> `content`
--   account_id             BIGINT  Canvas `terms_of_services.account_id`
--                                   -> `account_id`
--
-- Canvas keeps the terms of service in `terms_of_services` /
-- `terms_of_service_contents`; the port mirrors those fields on the account
-- row so GET /accounts/:id/terms_of_service can answer with the account's
-- own policy. Canvas resolves the *root* account's policy, so the handler
-- reads the root account's row.
--
-- `root_account_id` is deliberately not a column: the port computes the root
-- account from the parent chain, so a stored copy could not go stale when an
-- account is re-parented. The JSON key is rendered from that computation.

CREATE TABLE IF NOT EXISTS accounts (
    id                             BIGSERIAL PRIMARY KEY,
    name                           TEXT NOT NULL,
    parent_account_id              BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    workflow_state                 TEXT NOT NULL DEFAULT 'active',
    created_at                     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                     TIMESTAMPTZ NOT NULL DEFAULT now(),
    settings                       JSONB NOT NULL DEFAULT '{}'::jsonb,
    uuid                           TEXT,
    default_time_zone              TEXT NOT NULL DEFAULT 'UTC',
    default_storage_quota_mb       INTEGER,
    default_user_storage_quota_mb  INTEGER,
    default_group_storage_quota_mb INTEGER,
    domain                         TEXT,
    login_handle_name              TEXT,
    change_password_url            TEXT,
    auth_discovery_url             TEXT,
    unknown_user_url               TEXT,
    login_help_url                 TEXT,
    discovery_page_active          BOOLEAN,
    account_calendar_visible       BOOLEAN NOT NULL DEFAULT false,
    account_calendar_subscription_type TEXT NOT NULL DEFAULT 'manual',
    sis_source_id                  TEXT,
    sis_batch_id                   BIGINT,
    integration_id                 TEXT,
    course_template_id             BIGINT,
    self_registration_type         TEXT NOT NULL DEFAULT 'none',
    terms_type                     TEXT NOT NULL DEFAULT 'default',
    passive                        BOOLEAN NOT NULL DEFAULT true,
    content                        TEXT,
    account_id                     BIGINT REFERENCES accounts(id) ON DELETE SET NULL
);

-- Canvas `accounts.sis_source_id` -> JSON `sis_account_id`.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS sis_source_id TEXT;

-- Canvas `accounts.sis_batch_id` -> JSON `sis_import_id`.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS sis_batch_id BIGINT;

-- Canvas `accounts.integration_id` -> JSON `integration_id`.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS integration_id TEXT;

-- Canvas `accounts.course_template_id` -> JSON `course_template_id`.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS course_template_id BIGINT;

-- Canvas `Account#self_registration_type` — the account's registration type
-- ("none", "all", "observer", ...). Canvas reads it from the account's
-- canvas authentication provider; the port stores it on the account row.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS self_registration_type TEXT NOT NULL DEFAULT 'none';

-- Canvas `terms_of_services.terms_type` — "default", "custom", "no_terms".
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS terms_type TEXT NOT NULL DEFAULT 'default';

-- Canvas `terms_of_services.passive` — true when the policy is not enforced.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS passive BOOLEAN NOT NULL DEFAULT true;

-- Canvas `terms_of_service_contents.content` — the policy body.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS content TEXT;

-- Canvas `terms_of_services.account_id` — the account that owns the policy.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL;

-- Canvas `terms_of_services.passive` on an explicitly stored policy. A policy
-- an admin configured is enforced by default; the account's own default
-- policy (the `accounts.passive` column above) is passive, like Canvas'
-- column default.
ALTER TABLE account_terms_of_service
    ADD COLUMN IF NOT EXISTS passive BOOLEAN NOT NULL DEFAULT false;
