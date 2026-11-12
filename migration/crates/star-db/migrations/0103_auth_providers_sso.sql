-- Migration 0103: authentication_providers + account SSO settings (Canvas parity)

CREATE TABLE IF NOT EXISTS authentication_providers (
    id             BIGSERIAL PRIMARY KEY,
    account_id     BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    auth_type      TEXT NOT NULL,
    position       INTEGER,
    settings       JSONB NOT NULL DEFAULT '{}',
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_auth_providers_account ON authentication_providers(account_id);

ALTER TABLE accounts ADD COLUMN IF NOT EXISTS login_handle_name TEXT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS change_password_url TEXT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS auth_discovery_url TEXT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS unknown_user_url TEXT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS login_help_url TEXT;
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS discovery_page_active BOOLEAN;
