-- Migration 0234: make the LTI context controls usable.
--
-- The Canvas schema (`db/migrate/20101210192618_init_canvas_db.rb`) models
-- the owner of a context control as a polymorphic reference:
--
--   t.references :context, polymorphic: %i[account course], null: false
--
-- The workspace schema translated that into two columns that are both
-- `NOT NULL` plus a check constraint that requires exactly one of them to
-- be NULL:
--
--   account_id bigint NOT NULL,
--   course_id  bigint NOT NULL,
--   CONSTRAINT chk_require_context_4 CHECK (((account_id IS NOT NULL)::int
--                                            + (course_id IS NOT NULL)::int) = 1)
--
-- That constraint can never be satisfied, so no row could ever be inserted.
-- This migration makes the two columns nullable and drops the constraint;
-- the "exactly one owner" rule is enforced by the routes, exactly like the
-- Canvas check constraint did.
--
-- The deployment of a control is an LTI Advantage deployment
-- (`lti_deployments`), not a Canvas 1.x external tool, so the foreign key
-- is repointed at the table the workspace keeps the deployments in.

ALTER TABLE lti_context_controls
    DROP CONSTRAINT IF EXISTS chk_require_context_4;

ALTER TABLE lti_context_controls
    ALTER COLUMN account_id DROP NOT NULL,
    ALTER COLUMN course_id DROP NOT NULL;

ALTER TABLE lti_context_controls
    DROP CONSTRAINT IF EXISTS fk_lti_context_controls_deployment_id;

ALTER TABLE lti_context_controls
    ADD CONSTRAINT fk_lti_context_controls_deployment_id
    FOREIGN KEY (deployment_id) REFERENCES lti_deployments (id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS index_lti_context_controls_on_root_account_id
    ON lti_context_controls (root_account_id);
