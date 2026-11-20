-- Migration 0213: course_paces + course_pace_module_items
--
--   GET    /courses/{courseId}/course_pacing
--   POST   /courses/{courseId}/course_pacing
--   GET    /courses/{courseId}/course_pacing/{id}
--   PUT    /courses/{courseId}/course_pacing/{id}
--   DELETE /courses/{courseId}/course_pacing/{id}
--   POST   /courses/{courseId}/course_pacing/{id}/publish
--   POST   /courses/{courseId}/course_pacing/compress_dates
--
-- Mirrors Canvas `CoursePace` (app/models/course_pace.rb) and
-- `CoursePaceModuleItem` (app/models/course_pace_module_item.rb).
-- course_pace_module_items links a pace to module_items (content_tags).
--
-- Workflow states: unpublished | active | deleted
-- Context can be a Course (no section/user), CourseSection, or User (StudentEnrollment).

CREATE TABLE IF NOT EXISTS course_paces (
    id                          BIGSERIAL PRIMARY KEY,
    course_id                   BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    -- Exactly one of course_section_id or user_id may be set; both null = primary course pace.
    course_section_id           BIGINT REFERENCES course_sections(id) ON DELETE CASCADE,
    user_id                     BIGINT REFERENCES users(id) ON DELETE CASCADE,
    root_account_id             BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    -- published (active) | unpublished | deleted
    workflow_state              TEXT NOT NULL DEFAULT 'unpublished',
    -- exclude_weekends and selected_days_to_skip are mutually exclusive at the feature level
    -- but stored as plain columns here (application logic enforces the choice).
    exclude_weekends            BOOLEAN NOT NULL DEFAULT FALSE,
    -- Canvas serialises this as a JSON array of day-name strings, e.g. ["sun","sat"].
    -- We store as JSONB for direct Postgres array operations.
    selected_days_to_skip       JSONB DEFAULT NULL,  -- e.g. ["sun","sat"] or NULL
    hard_end_dates              BOOLEAN NOT NULL DEFAULT FALSE,
    end_date                    TIMESTAMPTZ,
    published_at                TIMESTAMPTZ,
    -- Canvas `assignments_weighting` is a serialised Hash; stored as JSONB.
    assignments_weighting        JSONB DEFAULT NULL,
    -- Canvas: `time_to_complete_calendar_days` (integer, nullable)
    time_to_complete_calendar_days INTEGER DEFAULT NULL,
    -- Standard timestamps
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Ensure exactly one of section/user is set (application-level invariant)
-- course_pace_module_items rows reference module_items (content_tags)

CREATE TABLE IF NOT EXISTS course_pace_module_items (
    id                  BIGSERIAL PRIMARY KEY,
    course_pace_id      BIGINT NOT NULL REFERENCES course_paces(id) ON DELETE CASCADE,
    -- Canvas: `module_item_id` is a ContentTag id
    module_item_id      BIGINT NOT NULL REFERENCES content_tags(id) ON DELETE CASCADE,
    root_account_id     BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    -- Duration in days for this pace item (Canvas: `duration` integer, NOT NULL)
    duration            INTEGER NOT NULL DEFAULT 1,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- A course_pace_module_item must reference an assignment content tag (application-level check)
    UNIQUE (course_pace_id, module_item_id)
);

-- Indexes for lookups
CREATE INDEX IF NOT EXISTS idx_course_paces_course_id
    ON course_paces (course_id, workflow_state);

CREATE INDEX IF NOT EXISTS idx_course_paces_section
    ON course_paces (course_section_id) WHERE course_section_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_course_paces_user
    ON course_paces (user_id) WHERE user_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_course_pace_module_items_pace
    ON course_pace_module_items (course_pace_id);

CREATE INDEX IF NOT EXISTS idx_course_pace_module_items_module_item
    ON course_pace_module_items (module_item_id);

-- Trigger to auto-update updated_at
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE TRIGGER trg_course_paces_updated_at
    BEFORE UPDATE ON course_paces
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE OR REPLACE TRIGGER trg_course_pace_module_items_updated_at
    BEFORE UPDATE ON course_pace_module_items
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
