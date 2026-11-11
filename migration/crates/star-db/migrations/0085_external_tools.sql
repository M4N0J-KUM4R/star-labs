-- Migration 0085: external tools (Canvas ContextExternalTool parity)
--
-- A NEW table, independent from the phase-3 `lti_registrations` table: an
-- external tool is an LTI tool configured for a specific context (course or
-- account).

CREATE TABLE IF NOT EXISTS external_tools (
    id                BIGSERIAL PRIMARY KEY,
    name              TEXT NOT NULL,
    description       TEXT,
    url               TEXT NOT NULL,
    domain            TEXT,
    consumer_id       TEXT,
    consumer_key      TEXT,
    shared_secret     TEXT,
    privacy_level     TEXT NOT NULL DEFAULT 'anonymous',
    workflow_state    TEXT NOT NULL DEFAULT 'active',
    developer_key_id  BIGINT,
    lti_registration_id BIGINT,
    deployment_id     TEXT,
    course_id         BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    account_id        BIGINT REFERENCES accounts(id) ON DELETE CASCADE,
    context_type      TEXT NOT NULL DEFAULT 'Course',
    context_id        BIGINT,
    tool_type         TEXT NOT NULL DEFAULT 'lti',
    lti_version       TEXT NOT NULL DEFAULT '1.3',
    icon_url          TEXT,
    text              TEXT,
    enabled           BOOLEAN NOT NULL DEFAULT TRUE,
    visible           BOOLEAN NOT NULL DEFAULT TRUE,
    open_in_new_tab   BOOLEAN NOT NULL DEFAULT FALSE,
    root_url          TEXT,
    ownership_type    TEXT NOT NULL DEFAULT 'account',
    tool_configuration JSONB NOT NULL DEFAULT '{}'::jsonb,
    custom_fields     JSONB NOT NULL DEFAULT '{}'::jsonb,
    selection_width   INTEGER,
    selection_height  INTEGER,
    not_selectable    BOOLEAN NOT NULL DEFAULT FALSE,
    created_by_id     BIGINT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_external_tools_context ON external_tools(context_type, context_id);
CREATE INDEX IF NOT EXISTS idx_external_tools_course ON external_tools(course_id);
CREATE INDEX IF NOT EXISTS idx_external_tools_account ON external_tools(account_id);

-- RCE / top-nav favorites per context.
CREATE TABLE IF NOT EXISTS external_tool_favorites (
    id           BIGSERIAL PRIMARY KEY,
    context_type TEXT NOT NULL,
    context_id   BIGINT NOT NULL,
    tool_id      BIGINT NOT NULL REFERENCES external_tools(id) ON DELETE CASCADE,
    favorite_type TEXT NOT NULL DEFAULT 'rce',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (context_type, context_id, tool_id, favorite_type)
);

CREATE INDEX IF NOT EXISTS idx_external_tool_favorites_context
    ON external_tool_favorites(context_type, context_id, favorite_type);
