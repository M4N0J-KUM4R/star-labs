-- Migration 0078: Canvas LTI 1.3 registrations, account bindings, deployments.
--
-- Canvas models (`app/models/lti/registration.rb`,
-- `app/models/lti/registration_account_binding.rb`,
-- `app/controllers/lti/registrations_controller.rb`,
-- `app/controllers/lti/deployments_controller.rb`):
--
--   lti_registrations
--     A tool installed in an account. The LTI 1.3 tool configuration is kept
--     as JSONB (Canvas stores it on the associated tool configuration /
--     developer key); `overlay` is the admin-defined per-account override.
--   lti_registration_account_bindings
--     Per-account enablement of a registration: on | off | allow.
--   lti_deployments
--     A deployment of a registration into an Account or Course context, with
--     the opaque `deployment_id` string shared with launched tools.
--
-- Routes served by star-lti:
--   GET/POST /accounts/{accountId}/lti_registrations
--   GET/DELETE /lti_registrations/{id}
--   GET/POST /lti_registrations/{id}/deployments
--   plus Canvas-nested aliases under /accounts/{accountId}/lti_registrations.

CREATE TABLE IF NOT EXISTS lti_registrations (
    id                      BIGSERIAL PRIMARY KEY,
    account_id              BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    root_account_id         BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    name                    TEXT NOT NULL,
    admin_nickname          TEXT,
    vendor                  TEXT,
    description             TEXT,
    icon_url                TEXT,
    lti_version             TEXT NOT NULL DEFAULT '1.3', -- 1.1 | 1.3
    dynamic_registration    BOOLEAN NOT NULL DEFAULT FALSE,
    internal_service        BOOLEAN NOT NULL DEFAULT FALSE,
    lock_deploying          BOOLEAN NOT NULL DEFAULT FALSE,
    template_registration_id BIGINT,
    unified_tool_id         TEXT,
    workflow_state          TEXT NOT NULL DEFAULT 'active', -- active | inactive | deleted
    configuration           JSONB NOT NULL DEFAULT '{}'::jsonb,
    overlay                 JSONB,
    created_by_id           BIGINT REFERENCES users(id) ON DELETE SET NULL,
    updated_by_id           BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Listing is account-scoped and hides deleted registrations.
CREATE INDEX IF NOT EXISTS idx_lti_registrations_account_state
    ON lti_registrations(account_id, workflow_state);

-- Root-account rollups (Canvas lists inherited registrations per root).
CREATE INDEX IF NOT EXISTS idx_lti_registrations_root_account
    ON lti_registrations(root_account_id);

-- Canvas analytics identifier; not unique when omitted.
CREATE INDEX IF NOT EXISTS idx_lti_registrations_unified_tool_id
    ON lti_registrations(unified_tool_id);

CREATE TABLE IF NOT EXISTS lti_registration_account_bindings (
    id              BIGSERIAL PRIMARY KEY,
    registration_id BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    account_id      BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    root_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    workflow_state  TEXT NOT NULL DEFAULT 'on', -- on | off | allow | deleted
    created_by_id   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    updated_by_id   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (registration_id, account_id)
);

CREATE INDEX IF NOT EXISTS idx_lti_registration_bindings_account
    ON lti_registration_account_bindings(account_id, workflow_state);

CREATE TABLE IF NOT EXISTS lti_deployments (
    id              BIGSERIAL PRIMARY KEY,
    registration_id BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    deployment_id   TEXT NOT NULL,
    context_type    TEXT NOT NULL DEFAULT 'Account', -- Account | Course
    context_id      BIGINT NOT NULL,
    context_name    TEXT,
    available       BOOLEAN NOT NULL DEFAULT TRUE,
    workflow_state  TEXT NOT NULL DEFAULT 'active', -- active | disabled | deleted
    created_by_id   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The deployment id is the stable identifier handed to the tool.
CREATE UNIQUE INDEX IF NOT EXISTS idx_lti_deployments_deployment_id
    ON lti_deployments(deployment_id);

CREATE INDEX IF NOT EXISTS idx_lti_deployments_registration_state
    ON lti_deployments(registration_id, workflow_state);

CREATE INDEX IF NOT EXISTS idx_lti_deployments_context
    ON lti_deployments(context_type, context_id);
