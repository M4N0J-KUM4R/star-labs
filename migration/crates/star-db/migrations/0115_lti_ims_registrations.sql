-- Migration 0115: LTI IMS Dynamic Registration and Tool Proxy
--
-- Canvas controllers covered:
--   lti/ims/dynamic_registration_controller  — 11 routes
--   lti/tool_proxy_controller               — 4 context-scoped routes
--   lti/ims/tool_proxy_controller           — 3 routes
--
-- Routes implemented:
--   lti/ims/dynamic_registration:
--     GET  /accounts/:account_id/registration_token
--     GET  /accounts/:account_id/registrations/uuid/:registration_uuid
--     GET  /accounts/:account_id/lti_registrations/uuid/:registration_uuid
--     GET  /accounts/:account_id/lti_registration_update_request/uuid/:registration_uuid
--     GET  /accounts/:account_id/registrations/:registration_id
--     PUT  /accounts/:account_id/registrations/:registration_id/overlay
--     GET  /accounts/:account_id/dr_iframe
--     GET  /accounts/:account_id/registrations/:registration_id/view
--     POST /registrations
--     PUT  /registrations/:registration_id
--     GET  /registrations/:registration_id
--   lti/tool_proxy (context-scoped):
--     DELETE /courses/:course_id/tool_proxies/:tool_proxy_id
--     PUT    /courses/:course_id/tool_proxies/:tool_proxy_id
--     DELETE /courses/:course_id/tool_proxies/:tool_proxy_id/update
--     PUT    /courses/:course_id/tool_proxies/:tool_proxy_id/update
--     DELETE /accounts/:account_id/tool_proxies/:tool_proxy_id
--     PUT    /accounts/:account_id/tool_proxies/:tool_proxy_id
--     DELETE /accounts/:account_id/tool_proxies/:tool_proxy_id/update
--     PUT    /accounts/:account_id/tool_proxies/:tool_proxy_id/update
--   lti/ims/tool_proxy:
--     GET  /tool_proxy/:tool_proxy_guid
--     POST /courses/:course_id/tool_proxy
--     POST /accounts/:account_id/tool_proxy

-- lti_ims_registrations: IMS LTI 1.3 registration state (tool configuration).
CREATE TABLE IF NOT EXISTS lti_ims_registrations (
    id                     BIGSERIAL PRIMARY KEY,
    guid                   TEXT NOT NULL UNIQUE,
    developer_key_id       BIGINT,
    root_account_id        BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    unified_tool_id        TEXT,
    registration_url       TEXT,
    initiate_login_uri     TEXT,
    jwks_uri               TEXT,
    client_name            TEXT,
    redirect_uris          TEXT,
    logo_uri               TEXT,
    scopes                 TEXT,
    lti_tool_configuration JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_lti_ims_registrations_root_account
    ON lti_ims_registrations(root_account_id);
CREATE INDEX IF NOT EXISTS idx_lti_ims_registrations_developer_key
    ON lti_ims_registrations(developer_key_id);

-- lti_tool_proxies: LTI 1.3 tool proxies (tool consumer-side state per context).
CREATE TABLE IF NOT EXISTS lti_tool_proxies (
    id              BIGSERIAL PRIMARY KEY,
    guid            TEXT NOT NULL UNIQUE,
    context_type    TEXT NOT NULL DEFAULT 'Account',
    context_id      BIGINT NOT NULL,
    registration_id BIGINT,
    developer_key_id BIGINT,
    raw_data        JSONB NOT NULL DEFAULT '{}'::jsonb,
    workflow_state  TEXT NOT NULL DEFAULT 'active',
    update_payload  JSONB,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_lti_tool_proxies_context
    ON lti_tool_proxies(context_type, context_id);
CREATE INDEX IF NOT EXISTS idx_lti_tool_proxies_registration
    ON lti_tool_proxies(registration_id);
CREATE INDEX IF NOT EXISTS idx_lti_tool_proxies_developer_key
    ON lti_tool_proxies(developer_key_id);

-- lti_registration_update_requests: pending or processed registration updates.
CREATE TABLE IF NOT EXISTS lti_registration_update_requests (
    id                  BIGSERIAL PRIMARY KEY,
    uuid                TEXT NOT NULL UNIQUE,
    lti_registration_id BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    root_account_id     BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    lti_ims_registration JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_by_id       BIGINT,
    accepted_at         TIMESTAMPTZ,
    rejected_at         TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_rur_registration
    ON lti_registration_update_requests(lti_registration_id);
CREATE INDEX IF NOT EXISTS idx_rur_uuid
    ON lti_registration_update_requests(uuid);
