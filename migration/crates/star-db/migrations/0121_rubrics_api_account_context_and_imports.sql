-- Migration 0121: rubrics_api remainder — account-context rubrics + rubric imports (#51).
--
-- Canvas rubrics are owned by a polymorphic context (`app/models/rubric.rb`:
-- `belongs_to :context, polymorphic: [:course, :account]`, `lib/rubric_context.rb`:
-- `has_many :rubrics, as: :context`), associations live on that same context,
-- and CSV imports are tracked by `RubricImport` (`app/models/rubric_import.rb`),
-- whose `#process_rubrics` stamps `rubric_imports_id` onto the rubrics it builds.
-- Star had no way to express any of that, so the account-context
-- `rubrics_api` routes had no data model.
--
-- Additive only:
-- * the legacy course columns stay in place and keep serving the course-scoped
--   paths — `rubrics.course_id`, `rubric_associations.context_id` and
--   `files.course_id` merely lose their NOT NULL so account-scoped rows (which
--   have no course) can be inserted;
-- * `*_context_id` holds an Account id when `context_type = 'Account'` and is
--   NULL for course-scoped rows (mirroring Canvas, where `Rubric#set_root_account_id`
--   and the polymorphic context id are only meaningful for one context type);
-- * `context_type` keeps its 'Course' default, so every pre-existing row and
--   every existing INSERT statement stays valid without touching history;
-- * no DROP / TRUNCATE / renumbering.
--
-- API scope unlocked by this file:
--   GET  /api/v1/accounts/{account_id}/rubrics
--   GET  /api/v1/accounts/{account_id}/rubrics/{id}
--   GET  /api/v1/accounts/{account_id}/rubrics/{id}/used_locations
--   POST /api/v1/courses/{course_id}/rubrics/upload
--   POST /api/v1/accounts/{account_id}/rubrics/upload
--   GET  /api/v1/courses/{course_id}/rubrics/upload/{id}
--   GET  /api/v1/accounts/{account_id}/rubrics/upload/{id}
--   GET  /api/v1/courses/{course_id}/rubrics/upload/{id}/rubrics
--   GET  /api/v1/accounts/{account_id}/rubrics/upload/{id}/rubrics

-- ---------------------------------------------------------------------------
-- rubrics
-- ---------------------------------------------------------------------------

ALTER TABLE rubrics ALTER COLUMN course_id DROP NOT NULL;

ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS context_type TEXT NOT NULL DEFAULT 'Course';
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS account_context_id BIGINT REFERENCES accounts(id) ON DELETE CASCADE;

-- Canvas defaults (app/models/rubric.rb + RubricCSVImporter imports).
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS reusable BOOLEAN NOT NULL DEFAULT true;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS public BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS free_form_criterion_comments BOOLEAN NOT NULL DEFAULT true;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS hide_score_total BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS hide_points BOOLEAN;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS rating_order TEXT NOT NULL DEFAULT 'descending';
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS button_display TEXT NOT NULL DEFAULT 'points';
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS description TEXT;
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS user_id BIGINT REFERENCES users(id) ON DELETE SET NULL;

-- ---------------------------------------------------------------------------
-- criteria / ratings: Canvas's rubric `data` hash carries synthetic item ids
-- (`<rubric_id>_<n>` for criteria, `<criterion_id>_<n>` for ratings — see
-- `Rubric#unique_item_id` and `RubricImport#process_rubrics`), returned
-- verbatim by the API. Star's rows are normalized, so the ids are persisted.
-- ---------------------------------------------------------------------------

ALTER TABLE rubric_criteria ADD COLUMN IF NOT EXISTS item_id TEXT;
ALTER TABLE rubric_ratings  ADD COLUMN IF NOT EXISTS item_id TEXT;

-- ---------------------------------------------------------------------------
-- rubric_associations: same polymorphic-context treatment (Canvas resolves
-- account-context `show`/`used_locations` through
-- `@context.rubric_associations.bookmarked`).
-- ---------------------------------------------------------------------------

ALTER TABLE rubric_associations ALTER COLUMN context_id DROP NOT NULL;

ALTER TABLE rubric_associations ADD COLUMN IF NOT EXISTS account_context_id BIGINT REFERENCES accounts(id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS idx_rubrics_account_context
    ON rubrics(account_context_id);
CREATE INDEX IF NOT EXISTS idx_rubrics_rubric_imports
    ON rubrics(rubric_imports_id);
CREATE INDEX IF NOT EXISTS idx_rubric_associations_account_context
    ON rubric_associations(account_context_id);

-- ---------------------------------------------------------------------------
-- rubric_imports (Canvas `RubricImport`)
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS rubric_imports (
    id             BIGSERIAL PRIMARY KEY,
    -- Canvas: belongs_to :context, polymorphic: [:account, :course],
    -- separate_columns: true → exactly one of these is set.
    course_id      BIGINT REFERENCES courses(id) ON DELETE CASCADE,
    account_id     BIGINT REFERENCES accounts(id) ON DELETE CASCADE,
    root_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    user_id        BIGINT REFERENCES users(id) ON DELETE SET NULL,
    attachment_id  BIGINT REFERENCES files(id) ON DELETE SET NULL,
    -- Canvas Workflow states: initializing → created → importing →
    -- succeeded | succeeded_with_errors | failed.
    workflow_state TEXT NOT NULL DEFAULT 'initializing',
    progress       INT NOT NULL DEFAULT 0,
    error_count    INT NOT NULL DEFAULT 0,
    error_data     JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT rubric_imports_progress_check CHECK (progress >= 0 AND progress <= 100),
    CONSTRAINT rubric_imports_context_check
        CHECK (num_nonnulls(course_id, account_id) = 1),
    CONSTRAINT rubric_imports_workflow_state_check
        CHECK (workflow_state IN ('initializing', 'created', 'importing',
                                  'succeeded', 'succeeded_with_errors', 'failed'))
);

CREATE INDEX IF NOT EXISTS idx_rubric_imports_course ON rubric_imports(course_id, id);
CREATE INDEX IF NOT EXISTS idx_rubric_imports_account ON rubric_imports(account_id, id);
CREATE INDEX IF NOT EXISTS idx_rubric_imports_root_account ON rubric_imports(root_account_id);

-- The rubric -> import link (Canvas `RubricImport#process_rubrics` builds
-- `context.rubrics.build(rubric_imports_id: id)`), which
-- `rubrics_by_import_id` filters on. Added here because rubric_imports has to
-- exist before it can be referenced.
ALTER TABLE rubrics
    ADD COLUMN IF NOT EXISTS rubric_imports_id BIGINT
        REFERENCES rubric_imports(id) ON DELETE SET NULL;

-- ---------------------------------------------------------------------------
-- files: the uploaded CSV becomes an Attachment (Canvas
-- `Attachment.create_data_attachment(import, file, "rubric_upload_<id>.csv")`).
-- Account-scoped uploads have no course, so files.course_id becomes nullable,
-- and the uploaded bytes are kept so an import row can expose its attachment.
-- ---------------------------------------------------------------------------

ALTER TABLE files ALTER COLUMN course_id DROP NOT NULL;
ALTER TABLE files ADD COLUMN IF NOT EXISTS data BYTEA;
