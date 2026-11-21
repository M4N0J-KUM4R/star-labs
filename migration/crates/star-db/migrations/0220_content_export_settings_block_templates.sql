-- Migration 0220: the content export settings, the block editor templates and
-- the course settings.
--
-- Canvas `ContentExport#settings` holds the selection of a share or a course
-- copy (`{"selected_content": {"assignments": ["42"]}}`), which the content
-- share API reads back to answer the content type of a share.
--
-- Canvas `BlockEditorTemplate` is the template of the block editor; the
-- table mirrors the columns of the Canvas model.
--
-- Canvas `Course#settings` holds the course scoped settings, among them
-- `enable_offline_web_export`, which the web zip export routes read.

ALTER TABLE content_exports
    ADD COLUMN IF NOT EXISTS settings JSONB NOT NULL DEFAULT '{}'::jsonb;

ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS settings JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE TABLE IF NOT EXISTS block_editor_templates (
    id              BIGSERIAL PRIMARY KEY,
    root_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    context_type    TEXT NOT NULL,
    context_id      BIGINT NOT NULL,
    name            TEXT NOT NULL,
    description     TEXT,
    tags            TEXT[] NOT NULL DEFAULT '{}',
    node_tree       JSONB NOT NULL DEFAULT '{}'::jsonb,
    editor_version  TEXT NOT NULL DEFAULT '0.2',
    template_type   TEXT NOT NULL DEFAULT 'page',
    thumbnail       TEXT,
    workflow_state  TEXT NOT NULL DEFAULT 'unpublished',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_block_editor_templates_context
    ON block_editor_templates (context_type, context_id);
