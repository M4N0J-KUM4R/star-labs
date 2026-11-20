-- Migration 0221: the LTI tool settings.
--
-- Canvas `ToolSetting` holds the settings a tool consumer reads for a
-- context: the tool proxy itself, a binding in a course or an account, and
-- the settings of a resource link inside it.  The `custom` column carries the
-- settings the tool reads.

CREATE TABLE IF NOT EXISTS tool_settings (
    id                BIGSERIAL PRIMARY KEY,
    tool_proxy_id     BIGINT NOT NULL REFERENCES lti_tool_proxies(id) ON DELETE CASCADE,
    context_type      TEXT,
    context_id        BIGINT,
    resource_link_id  TEXT,
    custom            JSONB NOT NULL DEFAULT '{}'::jsonb,
    custom_parameters JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_tool_settings_lookup
    ON tool_settings (tool_proxy_id, context_type, context_id, resource_link_id);
