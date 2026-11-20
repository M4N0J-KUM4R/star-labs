-- Migration 0151: Canvas rubric imports parity.
--
-- Canvas `RubricImport` (app/models/rubric_import.rb) tracks CSV rubric upload
-- jobs. The import is created with attachment + progress=0, scheduled via
-- `schedule`, and processed asynchronously (states: initializing → created →
-- importing → succeeded / succeeded_with_errors / failed).
--
-- API scope (app/controllers/rubrics_api_controller.rb):
-- - POST   /courses/{courseId}/rubrics/upload               → creates import, returns 201
-- - GET    /courses/{courseId}/rubrics/upload_template       → returns CSV template (already impl)
-- - GET    /courses/{courseId}/rubrics/upload_status          → latest or by id
-- - GET    /courses/{courseId}/rubrics/rubrics_by_import_id/{id}
--
-- Unlike Canvas (which stores rubric data as a JSON `data` column on rubrics),
-- star-lms stores criteria and ratings as normalized rows, so the CSV import
-- is parsed and converted to INSERTs on the same rubrics/rubric_criteria/
-- rubric_ratings tables already created by migrations 0023 and 0094.

CREATE TABLE IF NOT EXISTS rubric_imports (
    id              BIGSERIAL PRIMARY KEY,
    -- Polymorphic context (Account or Course).
    context_id      BIGINT NOT NULL,
    context_type    TEXT NOT NULL,  -- 'Account' | 'Course'
    -- The uploaded CSV attachment id. star-lms has no attachments table yet, so this is a
    -- plain id column with no foreign key (Canvas: rubric_imports.attachment_id).
    attachment_id   BIGINT,
    -- The user who initiated the import.
    user_id         BIGINT REFERENCES users(id) ON DELETE SET NULL,
    -- Processing state; mirrors Canvas workflow states.
    workflow_state  TEXT NOT NULL DEFAULT 'initializing',
    -- 0–100 progress percentage updated during processing.
    progress        INT NOT NULL DEFAULT 0,
    -- Counts of warnings collected during import.
    error_count     INT NOT NULL DEFAULT 0,
    error_data      JSONB NOT NULL DEFAULT '[]',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Index for finding imports by context (course or account).
CREATE INDEX IF NOT EXISTS idx_rubric_imports_context
    ON rubric_imports(context_id, context_type);

-- Index for rubrics_by_import_id lookup.
CREATE INDEX IF NOT EXISTS idx_rubric_imports_user
    ON rubric_imports(user_id);

-- star-lms stores rubric_imports_id on the created rubrics so they can be
-- enumerated via rubrics_by_import_id. Ensure column exists (Canvas also
-- stores it; star-lms rubrics may have been created before this migration).
ALTER TABLE rubrics ADD COLUMN IF NOT EXISTS rubric_imports_id BIGINT
    REFERENCES rubric_imports(id) ON DELETE SET NULL;
