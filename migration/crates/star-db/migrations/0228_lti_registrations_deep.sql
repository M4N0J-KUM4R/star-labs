-- Migration 0228: LTI registrations deep (issue #69)
--
-- Canvas controllers covered:
--   lti/registrations_controller   — history, overlay_history, reset, bind,
--                                     unbind, install_from_template,
--                                     update_requests, context_search
--   lti/context_controls_controller — controls CRUD + bulk
--   lti/deployments_controller      — deployment show/destroy/controls
--
-- Migrations 0223-0227 (PR #200, full Canvas schema) already created
-- lti_context_controls, lti_overlay_versions and
-- lti_registration_history_entries straight from the Canvas schema dump.
-- This migration adapts them for the registrations-deep handlers:
--
--   * The dump marks BOTH account_id and course_id NOT NULL while its own
--     chk_require_context_4 CHECK demands exactly one non-NULL — a Rails
--     schema.rb artifact that makes every insert fail. Drop the NOT NULLs
--     so exactly one context can be stored.
--   * The dump's bare `timestamp` columns become timestamptz with now()
--     defaults, matching the workspace convention (migration/CLAUDE.md:
--     every table gets timestamptz default now()).
--   * lti_overlay_versions gains registration_id (the workspace keeps the
--     live overlay on lti_registrations.overlay and uses the registration
--     id where Canvas stores an lti_overlays row id) and the overlay
--     snapshot column for the overlay_history response.
--   * The one-active-control-per-(deployment, context) unique index
--     (Canvas Lti::ContextControlService.unique_check_attrs).

ALTER TABLE lti_context_controls ALTER COLUMN account_id DROP NOT NULL;
ALTER TABLE lti_context_controls ALTER COLUMN course_id DROP NOT NULL;
ALTER TABLE lti_context_controls ALTER COLUMN created_at TYPE timestamptz;
ALTER TABLE lti_context_controls ALTER COLUMN updated_at TYPE timestamptz;
ALTER TABLE lti_context_controls ALTER COLUMN created_at SET DEFAULT now();
ALTER TABLE lti_context_controls ALTER COLUMN updated_at SET DEFAULT now();

CREATE UNIQUE INDEX IF NOT EXISTS uq_lti_context_controls_context
    ON lti_context_controls(deployment_id, account_id, course_id) NULLS NOT DISTINCT
    WHERE workflow_state <> 'deleted';

ALTER TABLE lti_overlay_versions ALTER COLUMN created_at TYPE timestamptz;
ALTER TABLE lti_overlay_versions ALTER COLUMN updated_at TYPE timestamptz;
ALTER TABLE lti_overlay_versions ALTER COLUMN created_at SET DEFAULT now();
ALTER TABLE lti_overlay_versions ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE lti_overlay_versions
    ADD COLUMN IF NOT EXISTS registration_id BIGINT
    REFERENCES lti_registrations(id) ON DELETE CASCADE;
ALTER TABLE lti_overlay_versions
    ADD COLUMN IF NOT EXISTS overlay JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE INDEX IF NOT EXISTS idx_lti_overlay_versions_registration
    ON lti_overlay_versions(registration_id, account_id);

ALTER TABLE lti_registration_history_entries ALTER COLUMN created_at TYPE timestamptz;
ALTER TABLE lti_registration_history_entries ALTER COLUMN updated_at TYPE timestamptz;
ALTER TABLE lti_registration_history_entries ALTER COLUMN created_at SET DEFAULT now();
ALTER TABLE lti_registration_history_entries ALTER COLUMN updated_at SET DEFAULT now();

CREATE INDEX IF NOT EXISTS idx_lti_registration_history_entries_registration
    ON lti_registration_history_entries(lti_registration_id, created_at DESC);
