-- Migration 0130: Context polymorphism for content_migrations and content_exports
--
-- Canvas ContentMigration and ContentExport are context-polymorphic (belong to
-- Course/Account/Group/User). This workspace previously only supported Course
-- contexts. This migration adds the additive columns needed for the full fan-out:
--
-- Routes enabled by this migration (content_migrations):
--   GET/POST        /accounts/:account_id/content_migrations
--   GET/PUT         /accounts/:account_id/content_migrations/:id
--   GET             /accounts/:account_id/content_migrations/:id/selective_data
--   GET             /accounts/:account_id/content_migrations/migrators
--   GET/POST        /groups/:group_id/content_migrations
--   GET/PUT         /groups/:group_id/content_migrations/:id
--   GET             /groups/:group_id/content_migrations/:id/selective_data
--   GET             /groups/:group_id/content_migrations/migrators
--   GET/POST        /users/:user_id/content_migrations
--   GET/PUT         /users/:user_id/content_migrations/:id
--   GET             /users/:user_id/content_migrations/:id/selective_data
--   GET             /users/:user_id/content_migrations/migrators
--
-- Routes enabled by this migration (content_exports):
--   GET/POST        /groups/:group_id/content_exports
--   GET             /groups/:group_id/content_exports/:id
--   PUT             /groups/:group_id/content_exports/:id/fail
--   GET/POST        /users/:user_id/content_exports
--   GET             /users/:user_id/content_exports/:id
--   PUT             /users/:user_id/content_exports/:id/fail
--
-- Also adds asset_id_mapping JSONB to content_migrations for course-copy
-- source→destination id mapping (GET .../asset_id_mapping).

-- Add context polymorphism to content_migrations.
-- course_id remains nullable for back-compat during migration; NULL means
-- the migration is scoped to the context_type/context_id pair.
ALTER TABLE content_migrations
    ADD COLUMN IF NOT EXISTS context_type TEXT NOT NULL DEFAULT 'Course',
    ADD COLUMN IF NOT EXISTS context_id   BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS asset_id_mapping JSONB NOT NULL DEFAULT '{}';

-- Prevent new rows from having NULL context values (protect against re-runs).
UPDATE content_migrations
   SET context_type = 'Course',
       context_id   = COALESCE(course_id, 0)
 WHERE context_type IS NULL OR context_id = 0;

ALTER TABLE content_migrations
    ALTER COLUMN context_type SET DEFAULT 'Course',
    ALTER COLUMN context_id   SET DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_content_migrations_context
    ON content_migrations(context_type, context_id);

-- Add context polymorphism to content_exports (groups and users fan-out).
-- course_id remains nullable for back-compat; NULL = context_id/context_type scope.
ALTER TABLE content_exports
    ADD COLUMN IF NOT EXISTS context_type TEXT NOT NULL DEFAULT 'Course',
    ADD COLUMN IF NOT EXISTS context_id   BIGINT NOT NULL DEFAULT 0;

UPDATE content_exports
   SET context_type = 'Course',
       context_id   = COALESCE(course_id, 0)
 WHERE context_type IS NULL OR context_id = 0;

ALTER TABLE content_exports
    ALTER COLUMN context_type SET DEFAULT 'Course',
    ALTER COLUMN context_id   SET DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_content_exports_context
    ON content_exports(context_type, context_id);

-- Migration issues: add context scoping (mirrors content_migrations).
-- Issues already have content_migration_id FK; adding context allows fan-out
-- without re-reading content_migrations.
ALTER TABLE migration_issues
    ADD COLUMN IF NOT EXISTS context_type TEXT NOT NULL DEFAULT 'Course',
    ADD COLUMN IF NOT EXISTS context_id   BIGINT NOT NULL DEFAULT 0;

UPDATE migration_issues
   SET context_type = (
           SELECT context_type FROM content_migrations WHERE id = content_migration_id
       ),
       context_id   = (
           SELECT context_id FROM content_migrations WHERE id = content_migration_id
       )
 WHERE context_type IS NULL OR context_id = 0;

ALTER TABLE migration_issues
    ALTER COLUMN context_type SET DEFAULT 'Course',
    ALTER COLUMN context_id   SET DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_migration_issues_context
    ON migration_issues(context_type, context_id);
