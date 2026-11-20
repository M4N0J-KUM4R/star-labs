-- Migration 0223: LTI registrations deep (issue #69)
--
-- Canvas controllers covered:
--   lti/registrations_controller   — history, overlay_history, reset, bind,
--                                     unbind, install_from_template,
--                                     update_requests, context_search
--   lti/context_controls_controller — controls CRUD + bulk
--   lti/deployments_controller      — deployment show/destroy/controls
--
-- Canvas models mirrored:
--   Lti::ContextControl (app/models/lti/context_control.rb) — per-context
--     availability switch for a registration deployment.
--   Lti::OverlayVersion (app/models/lti/overlay_version.rb) — overlay edit
--     history backing GET .../lti_registrations/:id/overlay_history.
--   Lti::RegistrationHistoryEntry (app/models/lti/registration_history_entry.rb)
--     — changelog backing GET .../lti_registrations/:id/history.
--
-- The workspace keeps the overlay itself on lti_registrations.overlay
-- (Canvas stores it on a separate Lti::Overlay row); the versions and history
-- tables below record who changed what, which is what the endpoints return.

CREATE TABLE IF NOT EXISTS lti_context_controls (
    id              BIGSERIAL PRIMARY KEY,
    registration_id BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    deployment_id   BIGINT NOT NULL REFERENCES lti_deployments(id) ON DELETE CASCADE,
    account_id      BIGINT REFERENCES accounts(id) ON DELETE CASCADE,
    course_id       BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    root_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    available       BOOLEAN NOT NULL DEFAULT TRUE,
    workflow_state  TEXT NOT NULL DEFAULT 'active', -- active | deleted
    created_by_id   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    updated_by_id   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- Canvas enforces one active control per (deployment, context)
    -- (Lti::ContextControlService.unique_check_attrs).
    CONSTRAINT lti_context_controls_one_context CHECK (
        (account_id IS NULL) <> (course_id IS NULL)
    )
);

CREATE INDEX IF NOT EXISTS idx_lti_context_controls_registration
    ON lti_context_controls(registration_id);
CREATE INDEX IF NOT EXISTS idx_lti_context_controls_deployment
    ON lti_context_controls(deployment_id);
CREATE UNIQUE INDEX IF NOT EXISTS uq_lti_context_controls_context
    ON lti_context_controls(deployment_id, account_id, course_id) NULLS NOT DISTINCT
    WHERE workflow_state <> 'deleted';

CREATE TABLE IF NOT EXISTS lti_overlay_versions (
    id              BIGSERIAL PRIMARY KEY,
    registration_id BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    account_id      BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    root_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    overlay         JSONB NOT NULL DEFAULT '{}'::jsonb,
    diff            JSONB,
    caused_by_reset BOOLEAN NOT NULL DEFAULT FALSE,
    created_by_id   BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_lti_overlay_versions_registration
    ON lti_overlay_versions(registration_id, account_id);

CREATE TABLE IF NOT EXISTS lti_registration_history_entries (
    id                   BIGSERIAL PRIMARY KEY,
    lti_registration_id BIGINT NOT NULL REFERENCES lti_registrations(id) ON DELETE CASCADE,
    root_account_id      BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    update_type          TEXT NOT NULL,
    diff                 JSONB,
    comment              TEXT,
    old_configuration    JSONB,
    new_configuration    JSONB,
    created_by_id        BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_lti_registration_history_entries_registration
    ON lti_registration_history_entries(lti_registration_id, created_at DESC);
