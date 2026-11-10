-- Migration 0039: per-user module item completion state.
-- Backs Canvas context_module_items_api#mark_as_done (PUT .../done) and
-- #mark_as_not_done (DELETE .../done). Canvas keeps a ContextModuleProgression
-- row and flips the requirement, so completions are upserted to
-- 'completed'/'incomplete' rather than hard-deleted.

CREATE TABLE IF NOT EXISTS module_item_completions (
    id          BIGSERIAL PRIMARY KEY,
    module_id   BIGINT NOT NULL REFERENCES modules(id) ON DELETE CASCADE,
    item_id     BIGINT NOT NULL REFERENCES module_items(id) ON DELETE CASCADE,
    user_id     BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    state       TEXT NOT NULL DEFAULT 'completed'
                CHECK (state IN ('completed', 'incomplete')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, item_id)
);

CREATE INDEX IF NOT EXISTS idx_module_item_completions_module
    ON module_item_completions(module_id);
CREATE INDEX IF NOT EXISTS idx_module_item_completions_user
    ON module_item_completions(user_id);
