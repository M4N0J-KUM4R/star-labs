-- Migration 0273: the LTI children an account takes with it
--
-- Canvas destroys an account's LTI children when it destroys the account
-- (`app/models/account.rb:92-95`):
--
--   has_many :lti_registrations, dependent: :destroy
--   has_many :lti_overlays, dependent: :destroy
--   has_many :lti_overlay_versions, dependent: :destroy
--   has_many :lti_notice_handlers, dependent: :destroy
--
-- The port's foreign keys pointed at the account without a delete rule, so
-- removing an account that had an LTI registration failed with
--
--   update or delete on table accounts violates foreign key constraint
--   fk_lti_overlays_account_id
--
-- and the rows stayed behind. The nightly's coverage lane failed on it, and
-- the leftover rows collide with the next run's seed.
--
-- The constraints are replaced with the rule Canvas' model layer applies.
-- The columns, the rows and the ids are unchanged; only the delete rule is.
-- `lti_context_controls` is included because Canvas removes an account's
-- context controls with it too (`app/models/account.rb:778-781`).

ALTER TABLE lti_overlays
    DROP CONSTRAINT IF EXISTS fk_lti_overlays_account_id;
ALTER TABLE lti_overlays
    ADD CONSTRAINT fk_lti_overlays_account_id
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE;

ALTER TABLE lti_overlay_versions
    DROP CONSTRAINT IF EXISTS fk_lti_overlay_versions_account_id;
ALTER TABLE lti_overlay_versions
    ADD CONSTRAINT fk_lti_overlay_versions_account_id
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE;

ALTER TABLE lti_notice_handlers
    DROP CONSTRAINT IF EXISTS fk_lti_notice_handlers_account_id;
ALTER TABLE lti_notice_handlers
    ADD CONSTRAINT fk_lti_notice_handlers_account_id
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE;

ALTER TABLE lti_context_controls
    DROP CONSTRAINT IF EXISTS fk_lti_context_controls_account_id;
ALTER TABLE lti_context_controls
    ADD CONSTRAINT fk_lti_context_controls_account_id
    FOREIGN KEY (account_id) REFERENCES accounts (id) ON DELETE CASCADE;

-- The same rows also point at the root account, so the root has to be able
-- to go with them.

ALTER TABLE lti_overlays
    DROP CONSTRAINT IF EXISTS fk_lti_overlays_root_account_id;
ALTER TABLE lti_overlays
    ADD CONSTRAINT fk_lti_overlays_root_account_id
    FOREIGN KEY (root_account_id) REFERENCES accounts (id) ON DELETE CASCADE;

ALTER TABLE lti_overlay_versions
    DROP CONSTRAINT IF EXISTS fk_lti_overlay_versions_root_account_id;
ALTER TABLE lti_overlay_versions
    ADD CONSTRAINT fk_lti_overlay_versions_root_account_id
    FOREIGN KEY (root_account_id) REFERENCES accounts (id) ON DELETE CASCADE;

ALTER TABLE lti_notice_handlers
    DROP CONSTRAINT IF EXISTS fk_lti_notice_handlers_root_account_id;
ALTER TABLE lti_notice_handlers
    ADD CONSTRAINT fk_lti_notice_handlers_root_account_id
    FOREIGN KEY (root_account_id) REFERENCES accounts (id) ON DELETE CASCADE;

ALTER TABLE lti_context_controls
    DROP CONSTRAINT IF EXISTS fk_lti_context_controls_root_account_id;
ALTER TABLE lti_context_controls
    ADD CONSTRAINT fk_lti_context_controls_root_account_id
    FOREIGN KEY (root_account_id) REFERENCES accounts (id) ON DELETE CASCADE;
