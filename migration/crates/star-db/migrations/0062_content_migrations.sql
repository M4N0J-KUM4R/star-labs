-- Migration 0062: Canvas content migrations parity
-- (app/controllers/content_migrations_controller.rb, lib/api/v1/content_migration.rb).
--
-- Canvas ContentMigration belongs_to a context (Course/Account/Group/User) and
-- to the user who started it, carries migration_type, a serialized
-- migration_settings hash, source_course_id for course copies, and has_many
-- migration_issues. This workspace exposes the course-scoped API plus a flat
-- show alias:
-- - GET  /api/v1/courses/{courseId}/content_migrations
-- - POST /api/v1/courses/{courseId}/content_migrations
-- - GET  /api/v1/courses/{courseId}/content_migrations/{id}
-- - GET  /api/v1/courses/{courseId}/content_migrations/migrators
-- - GET  /api/v1/content_migrations/{id}
-- - GET  /api/v1/content_migrations/migrators
--
-- workflow_state mirrors the Canvas model states:
--   created | queued | pre_processing | pre_processed | pre_process_error |
--   exporting | exported | importing | imported | failed
-- The API layer maps them to the Canvas JSON states (see content_migrations.rs).
--
-- migration_settings is stored as JSON text rather than JSONB: the workspace
-- sqlx feature set does not enable json decoding, matching other crates that
-- keep serialized blobs as TEXT.
--
-- Idempotent (IF NOT EXISTS) so re-applying with psql is safe before sqlx
-- records the migration.

CREATE TABLE IF NOT EXISTS content_migrations (
    id                 BIGSERIAL PRIMARY KEY,
    course_id          BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    user_id            BIGINT REFERENCES users(id) ON DELETE SET NULL,
    migration_type     TEXT NOT NULL,
    workflow_state     TEXT NOT NULL DEFAULT 'created',
    migration_settings TEXT NOT NULL DEFAULT '{}',
    source_course_id   BIGINT REFERENCES courses(id) ON DELETE SET NULL,
    -- Metadata-only stand-in for Canvas' attachment association: the workspace
    -- stores no file bytes, but keeps the uploaded package name/size so a
    -- future worker can pick the migration up.
    attachment_name    TEXT,
    attachment_size    BIGINT,
    started_at         TIMESTAMPTZ,
    finished_at        TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT content_migrations_workflow_state_check CHECK (
        workflow_state IN (
            'created', 'queued', 'pre_processing', 'pre_processed',
            'pre_process_error', 'exporting', 'exported', 'importing',
            'imported', 'failed'
        )
    )
);

-- Index lists newest-first per course (Canvas: order(id: :desc)).
CREATE INDEX IF NOT EXISTS idx_content_migrations_course_id
    ON content_migrations(course_id, id DESC);

-- Per-user history (Canvas scopes migrations by user for non-admins).
CREATE INDEX IF NOT EXISTS idx_content_migrations_user_id
    ON content_migrations(user_id);

-- Course copy lookups by source course.
CREATE INDEX IF NOT EXISTS idx_content_migrations_source_course_id
    ON content_migrations(source_course_id);

-- Migration issues (Canvas MigrationIssue): surfaced through
-- migration_issues_count and created when a pre_processing upload times out.
-- Canvas columns: content_migration_id, description, workflow_state
-- (active|resolved), fix_issue_html_url, issue_type (todo|warning|error),
-- error_message. error_report_id is omitted (no error_reports table).
CREATE TABLE IF NOT EXISTS migration_issues (
    id                   BIGSERIAL PRIMARY KEY,
    content_migration_id BIGINT NOT NULL REFERENCES content_migrations(id) ON DELETE CASCADE,
    description          TEXT,
    workflow_state       TEXT NOT NULL DEFAULT 'active',
    fix_issue_html_url   TEXT,
    issue_type           TEXT NOT NULL DEFAULT 'error',
    error_message        TEXT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT migration_issues_state_check CHECK (
        workflow_state IN ('active', 'resolved')
    ),
    CONSTRAINT migration_issues_type_check CHECK (
        issue_type IN ('todo', 'warning', 'error')
    )
);

CREATE INDEX IF NOT EXISTS idx_migration_issues_content_migration_id
    ON migration_issues(content_migration_id);
