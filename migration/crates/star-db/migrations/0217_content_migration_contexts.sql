-- Migration 0217: polymorphic context for content migrations.
--
-- Canvas `ContentMigration` belongs to a polymorphic context:
--
--   belongs_to :context, polymorphic: [:course, :account, :group, { context_user: "User" }]
--
-- and `ContentMigrationsController` serves the same actions for a course, an
-- account, a group and a user:
--
--   GET  /api/v1/{courses,accounts,groups,users}/:id/content_migrations
--   POST /api/v1/{courses,accounts,groups,users}/:id/content_migrations
--   GET  /api/v1/{courses,accounts,groups,users}/:id/content_migrations/:id
--   PUT  /api/v1/{courses,accounts,groups,users}/:id/content_migrations/:id
--
-- The port stored the context in `course_id` alone, so the account, group and
-- user variants could not be served.  The two columns below carry the context
-- for every row; the existing `course_id` column stays as it is, so the
-- course-scoped queries and the rows that already exist keep working.
--
-- The backfill gives every course-scoped row its context.  The user id
-- column is unrelated: it is the user who started the migration.

ALTER TABLE content_migrations
    ADD COLUMN IF NOT EXISTS context_type TEXT;

ALTER TABLE content_migrations
    ADD COLUMN IF NOT EXISTS context_id BIGINT;

UPDATE content_migrations
   SET context_type = 'Course',
       context_id = course_id
 WHERE context_type IS NULL
   AND course_id IS NOT NULL;

-- A migration without a course still belongs to the user who created it.
UPDATE content_migrations
   SET context_type = 'User',
       context_id = user_id
 WHERE context_type IS NULL
   AND user_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_content_migrations_context
    ON content_migrations (context_type, context_id);
