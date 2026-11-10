-- Migration 0032: developer keys + OAuth access tokens (Canvas API parity).
--
-- developer_keys are the OAuth2 client registrations managed through
-- /api/v1/developer_keys (Canvas "Developer Keys" API). api_key is the client
-- secret used by the client_credentials / refresh_token grants.
--
-- access_tokens stores the opaque bearer tokens issued by
-- POST /login/oauth2/token and the matching refresh token (same row, rotated
-- in place by the refresh_token grant). Tokens are scoped to a developer key;
-- the user is NULL for service (client-credentials) tokens.

CREATE TABLE IF NOT EXISTS developer_keys (
    id                          BIGSERIAL PRIMARY KEY,
    name                        TEXT,
    email                       TEXT,
    icon_url                    TEXT,
    notes                       TEXT,
    workflow_state              TEXT NOT NULL DEFAULT 'active',
    api_key                     TEXT NOT NULL,
    redirect_uris               TEXT[] NOT NULL DEFAULT '{}',
    scopes                      TEXT[] NOT NULL DEFAULT '{}',
    require_scopes              BOOLEAN NOT NULL DEFAULT FALSE,
    allow_includes              BOOLEAN NOT NULL DEFAULT FALSE,
    visible                     BOOLEAN NOT NULL DEFAULT TRUE,
    vendor_code                 TEXT,
    auto_expire_tokens          BOOLEAN NOT NULL DEFAULT FALSE,
    client_credentials_audience TEXT,
    user_id                     BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_developer_keys_api_key
    ON developer_keys(api_key);
CREATE INDEX IF NOT EXISTS idx_developer_keys_workflow_state
    ON developer_keys(workflow_state);

CREATE TABLE IF NOT EXISTS access_tokens (
    id                  BIGSERIAL PRIMARY KEY,
    user_id             BIGINT REFERENCES users(id) ON DELETE CASCADE,
    developer_key_id    BIGINT REFERENCES developer_keys(id) ON DELETE CASCADE,
    token               TEXT NOT NULL,
    refresh_token       TEXT,
    scopes              TEXT[] NOT NULL DEFAULT '{}',
    purpose             TEXT,
    workflow_state      TEXT NOT NULL DEFAULT 'active',
    expires_at          TIMESTAMPTZ,
    permanent_expires_at TIMESTAMPTZ,
    last_used_at        TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_access_tokens_token
    ON access_tokens(token);
CREATE UNIQUE INDEX IF NOT EXISTS idx_access_tokens_refresh_token
    ON access_tokens(refresh_token) WHERE refresh_token IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_access_tokens_user_id
    ON access_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_access_tokens_developer_key_id
    ON access_tokens(developer_key_id);
