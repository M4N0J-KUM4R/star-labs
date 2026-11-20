-- Migration 0231: the LTI registration history, overlays, bindings and
-- deployments, and the page views.
--
-- Canvas keeps the edit history of an LTI registration in
-- `lti_registration_history_entries`, the versions of its overlay in
-- `lti_overlay_versions`, the accounts a registration is shared with in
-- `lti_registration_account_bindings` and the contexts it is deployed to in
-- `lti_deployments`.
--
-- `page_views` records the page views of a user; `page_view_queries` the
-- queries the calendar and the admin run over them.

CREATE TABLE IF NOT EXISTS lti_registration_history_entries (
    id                    BIGSERIAL PRIMARY KEY,
    root_account_id       BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    lti_registration_id   BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    created_by_id         BIGINT REFERENCES users(id) ON DELETE SET NULL,
    diff                  JSONB NOT NULL DEFAULT '{}'::jsonb,
    update_type           TEXT NOT NULL DEFAULT 'update',
    comment               TEXT,
    old_configuration     JSONB,
    new_configuration     JSONB,
    old_context_controls  JSONB,
    new_context_controls  JSONB,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_lti_registration_history_entries_registration
    ON lti_registration_history_entries (lti_registration_id);

CREATE TABLE IF NOT EXISTS lti_overlay_versions (
    id                BIGSERIAL PRIMARY KEY,
    account_id        BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    root_account_id   BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    lti_overlay_id    BIGINT,
    diff              JSONB NOT NULL DEFAULT '{}'::jsonb,
    caused_by_reset   BOOLEAN NOT NULL DEFAULT false,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_lti_overlay_versions_overlay ON lti_overlay_versions (lti_overlay_id);

CREATE TABLE IF NOT EXISTS lti_registration_account_bindings (
    id                BIGSERIAL PRIMARY KEY,
    registration_id   BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    account_id        BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    root_account_id   BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    workflow_state    TEXT NOT NULL DEFAULT 'active',
    created_by_id     BIGINT REFERENCES users(id) ON DELETE SET NULL,
    updated_by_id     BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (registration_id, account_id)
);

CREATE TABLE IF NOT EXISTS lti_deployments (
    id                BIGSERIAL PRIMARY KEY,
    registration_id   BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    context_type      TEXT NOT NULL,
    context_id        BIGINT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_lti_deployments_registration ON lti_deployments (registration_id);

CREATE TABLE IF NOT EXISTS page_views (
    request_id           TEXT PRIMARY KEY,
    session_id           TEXT,
    user_id              BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    real_user_id         BIGINT REFERENCES users(id) ON DELETE SET NULL,
    url                  TEXT,
    context_id           BIGINT,
    context_type         TEXT,
    asset_id             BIGINT,
    asset_type           TEXT,
    controller           TEXT,
    action               TEXT,
    interaction_seconds  DOUBLE PRECISION,
    participated         BOOLEAN NOT NULL DEFAULT false,
    summarized           BOOLEAN NOT NULL DEFAULT false,
    developer_key_id     BIGINT REFERENCES developer_keys(id) ON DELETE SET NULL,
    user_request         BOOLEAN,
    render_time          DOUBLE PRECISION,
    user_agent           TEXT,
    asset_user_access_id BIGINT,
    account_id           BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    http_method          TEXT,
    remote_ip            TEXT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_page_views_user ON page_views (user_id, created_at DESC);

CREATE TABLE IF NOT EXISTS page_view_queries (
    id             UUID PRIMARY KEY,
    requestor_id   BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    user_id        BIGINT REFERENCES users(id) ON DELETE CASCADE,
    user_ids       BIGINT[] NOT NULL DEFAULT '{}',
    start_date     DATE NOT NULL,
    end_date       DATE NOT NULL,
    results_format TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'finished',
    error_code     TEXT,
    warnings       JSONB,
    content        BYTEA,
    filename       TEXT,
    content_type   TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at   TIMESTAMPTZ
);
