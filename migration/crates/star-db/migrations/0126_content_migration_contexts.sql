-- Migration 0126: content migration context polymorphism for the
-- migration_issues API fan-out (issue #64).
--
-- Canvas ContentMigration belongs_to a polymorphic :context
-- (Course/Account/Group/User; app/models/content_migration.rb). 0062 modeled
-- only the course context through content_migrations.course_id. Canvas'
-- migration_issues routes fan out over accounts, courses, groups and users
-- (config/routes.rb `scope(controller: :migration_issues)`), and
-- MigrationIssuesController#require_content_migration resolves the migration
-- through the context association:
--     @context.content_migrations.find(params[:content_migration_id])
-- This migration adds the polymorphic columns so account/group/user contexts
-- can own content migrations. Course contexts keep using the 0062 course_id
-- column (no data rewrite; additive only).
--
-- context_type stores the Canvas class names ('Account', 'Group', 'User');
-- NULL means the migration is course-scoped via course_id.
--
-- Idempotent (IF NOT EXISTS) so re-applying with psql is safe before sqlx
-- records the migration.

ALTER TABLE content_migrations
    ADD COLUMN IF NOT EXISTS context_type TEXT,
    ADD COLUMN IF NOT EXISTS context_id BIGINT;

-- Canvas: content migrations are looked up per context
-- (@context.content_migrations.find); only non-course rows carry the
-- polymorphic columns.
CREATE INDEX IF NOT EXISTS idx_content_migrations_context
    ON content_migrations(context_type, context_id) WHERE context_type IS NOT NULL;
