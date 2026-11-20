-- Migration 0127: progress table for Canvas Progress parity.
--
-- Canvas group_categories#import_tags (config/routes.rb:2600) starts a CSV
-- tag import and answers with a Progress object, persisted in Canvas's
-- `progresses` table and served at /api/v1/progress/:id. star-groups
-- persists a minimal progress row so the import_tags response carries a
-- real id (tag 'course_tag_import', workflow_state 'running') instead of a
-- fabricated one.
--
-- Columns mirror the subset of Canvas Progress fields the API renders:
--   - tag, workflow_state (queued | running | completed | failed),
--     completion (0..=100), context_type/context_id (Course here),
--     user_id, message, url are filled in by the endpoint layer.
--
-- No endpoint lists this table yet, so only a context index is added.
CREATE TABLE IF NOT EXISTS progress (
    id              BIGSERIAL PRIMARY KEY,
    tag             TEXT NOT NULL,
    workflow_state  TEXT NOT NULL DEFAULT 'running'
                    CHECK (workflow_state IN ('queued', 'running', 'completed', 'failed')),
    completion      INTEGER NOT NULL DEFAULT 0
                    CHECK (completion >= 0 AND completion <= 100),
    context_type    TEXT,
    context_id      BIGINT,
    user_id         BIGINT,
    message         TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS progress_context_idx ON progress (context_type, context_id);
