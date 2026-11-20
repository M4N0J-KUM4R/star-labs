-- Migration 0123: users controller tail — user preferences + user merge ledger (#57).
--
-- Canvas parity:
--   - PUT /users/:id/text_editor_preference          (set_text_editor_preference)
--   - PUT /users/:id/files_ui_version_preference      (set_files_ui_version_preference)
--   - GET /users/:id/new_user_tutorial_statuses       (get_new_user_tutorial_statuses)
--   - PUT /users/:id/new_user_tutorial_statuses/:page (set_new_user_tutorial_status)
--   - PUT /users/:id/merge_into/...                   (merge_into)
--   - POST /users/:id/split                           (split)
--
-- Canvas backs the first four with serialized User preferences
-- (User#set_preference / #get_preference); star-lms has no generic
-- preference store (user_settings is boolean-columns-only), so this adds a
-- per-user JSONB document with the same get/set/null semantics.
--
-- user_merges mirrors Canvas UserMergeData: one row per merge, keyed to the
-- SURVIVING user with a pointer at the merged-away user. Canvas keeps the
-- from_user row soft-deleted with merged_into_user_id set; this schema has
-- no users.workflow_state, so user_merges stores a snapshot (name, email,
-- sortable_name, password hash) that split uses to re-create the user —
-- Canvas does the same through UserMergeDataItem records plus the soft-
-- deleted row, and its 180-day splitable window maps to created_at here.
-- from_user_id carries no FK: the referenced row is deleted by the merge.

CREATE TABLE IF NOT EXISTS user_preferences (
    user_id     BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    preferences JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS user_merges (
    id              BIGSERIAL PRIMARY KEY,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    from_user_id    BIGINT NOT NULL,
    from_name       TEXT,
    from_email      TEXT,
    from_sortable_name TEXT,
    from_password_hash  TEXT,
    workflow_state  TEXT NOT NULL DEFAULT 'active'
        CHECK (workflow_state IN ('active', 'deleted', 'failed')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_user_merges_user
    ON user_merges (user_id, workflow_state);
CREATE INDEX IF NOT EXISTS idx_user_merges_from_user
    ON user_merges (from_user_id);
