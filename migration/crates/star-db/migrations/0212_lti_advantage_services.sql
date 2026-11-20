-- Migration 0212: LTI Advantage services (line items, data services, tool proxy).
--
-- Canvas: app/controllers/lti/line_items_controller.rb,
--         app/controllers/lti/data_services_controller.rb,
--         app/controllers/lti/tool_proxy_controller.rb.
-- Column lists mirror the *_COLUMNS constants in star-lti.

CREATE TABLE IF NOT EXISTS lti_line_items (
    id              BIGSERIAL PRIMARY KEY,
    course_id       BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    assignment_id   BIGINT REFERENCES assignments(id) ON DELETE SET NULL,
    label           TEXT NOT NULL,
    resource_id     TEXT,
    tag             TEXT,
    score_maximum   DOUBLE PRECISION NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    workflow_state  TEXT NOT NULL DEFAULT 'active'
);

CREATE INDEX IF NOT EXISTS idx_lti_line_items_course
    ON lti_line_items(course_id);

CREATE TABLE IF NOT EXISTS lti_data_service_subscriptions (
    id                  BIGSERIAL PRIMARY KEY,
    account_id          BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    context_type        TEXT NOT NULL,
    context_id          TEXT NOT NULL,
    event_types         JSONB NOT NULL DEFAULT '[]',
    format              TEXT NOT NULL,
    transport_type      TEXT NOT NULL,
    transport_metadata  JSONB NOT NULL DEFAULT '{}',
    owner_id            TEXT,
    owner_type          TEXT NOT NULL DEFAULT 'person',
    workflow_state      TEXT NOT NULL DEFAULT 'active',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_lti_data_service_subscriptions_account
    ON lti_data_service_subscriptions(account_id);

CREATE TABLE IF NOT EXISTS lti_tool_proxies (
    id              BIGSERIAL PRIMARY KEY,
    guid            TEXT NOT NULL,
    context_type    TEXT,
    context_id      BIGINT,
    account_id      BIGINT REFERENCES accounts(id) ON DELETE CASCADE,
    raw_data        JSONB NOT NULL DEFAULT '{}',
    workflow_state  TEXT NOT NULL DEFAULT 'active',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_lti_tool_proxies_guid
    ON lti_tool_proxies(guid);
