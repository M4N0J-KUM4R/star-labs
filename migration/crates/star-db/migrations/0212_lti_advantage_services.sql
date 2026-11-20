-- Migration 0212: LTI Advantage services tables (line items + data services).
--
-- Canvas: app/controllers/lti/line_items_controller.rb,
--         app/controllers/lti/data_services_controller.rb.
-- Column lists mirror LINE_ITEM_COLUMNS (star-lti/src/line_items.rs) and
-- SUBSCRIPTION_COLUMNS (star-lti/src/data_services.rs).
--
-- lti_tool_proxies is intentionally NOT created here: PR #163 (star-lti
-- tool_proxy routes) owns it via 0115_lti_ims_registrations.sql.

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
