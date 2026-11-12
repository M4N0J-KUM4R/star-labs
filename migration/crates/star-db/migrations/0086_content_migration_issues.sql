-- Migration 0086: Canvas content migration issues deep parity
-- (app/controllers/migration_issues_controller.rb, lib/api/v1/content_migration.rb).
--
-- 0062 already created content_migrations and migration_issues. Canvas'
-- MigrationIssue carries a fix_issue_html_url, an optional link to the source
-- object, and the two "fixable" flags surfaced by the selective import UI.
-- This migration only adds the missing columns (no drops, no rewrites):
-- - issue_link            — link to the source object the issue refers to
-- - fixable_by_web_upload — Canvas `fixable_by_web_upload?`
-- - fixable_by_admin      — Canvas `fixable_by_admin?`
--
-- `unresolved` is derived from workflow_state ('active') at serialization
-- time, matching Canvas' `unresolved?` helper.
--
-- Idempotent (IF NOT EXISTS) so re-applying with psql is safe before sqlx
-- records the migration.

ALTER TABLE migration_issues
    ADD COLUMN IF NOT EXISTS issue_link TEXT,
    ADD COLUMN IF NOT EXISTS fixable_by_web_upload BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS fixable_by_admin BOOLEAN NOT NULL DEFAULT false;

-- Canvas lists issues per migration ordered by created_at (by_created_at).
CREATE INDEX IF NOT EXISTS idx_migration_issues_created_at
    ON migration_issues(content_migration_id, created_at, id);
