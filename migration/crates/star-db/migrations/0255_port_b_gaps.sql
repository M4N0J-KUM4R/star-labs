-- Migration 0255: PORT-B parity gaps (star-users / star-modules / star-files).
--
-- Column additions only; every table already exists:
-- - module_items.url            Canvas ContentTag#url — the target of an
--                              ExternalUrl module item (context_module_items_
--                              api#redirect) and copied by module duplicate.
-- - module_item_completions     widen the state CHECK with 'read' so
--                              mark_read can upsert a read state that does
--                              not clobber a completed one.
-- - files.word_count            Canvas files#update_word_count.
-- - group_files.usage_rights_id / user_files.usage_rights_id
--                              Canvas UsageRightsController#set_usage_rights
--                              links the context's usage_rights row onto its
--                              attachments; the shared usage_rights table is
--                              migration 0084.

ALTER TABLE module_items
    ADD COLUMN IF NOT EXISTS url TEXT;

ALTER TABLE module_item_completions
    DROP CONSTRAINT IF EXISTS module_item_completions_state_check;

ALTER TABLE module_item_completions
    ADD CONSTRAINT module_item_completions_state_check
        CHECK (state IN ('completed', 'incomplete', 'read'));

ALTER TABLE files
    ADD COLUMN IF NOT EXISTS word_count INTEGER;

ALTER TABLE group_files
    ADD COLUMN IF NOT EXISTS usage_rights_id BIGINT
        REFERENCES usage_rights(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_group_files_usage_rights_id
    ON group_files(usage_rights_id);

ALTER TABLE user_files
    ADD COLUMN IF NOT EXISTS usage_rights_id BIGINT
        REFERENCES usage_rights(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_user_files_usage_rights_id
    ON user_files(usage_rights_id);
