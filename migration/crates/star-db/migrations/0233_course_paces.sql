-- Migration 0233: the course paces, their module items and the progress
-- rows the pacing routes publish.
--
-- Canvas keeps a course pace in `course_paces` (the plan of a course, one
-- of its sections or one of its enrollments) and the shifted module items
-- in `course_pace_module_items`. The columns mirror the Canvas schema
-- (`db/migrate/20101210192618_init_canvas_db.rb`):
--
--   - `workflow_state` is "unpublished", "active" or "deleted";
--   - `selected_days_to_skip` is a text array, Canvas defaults it to
--     ["sat", "sun"];
--   - `assignments_weighting` holds the JSON weighting Canvas stores;
--   - the unique indexes keep one active pace per course / section / user.
--
-- The Canvas-schema baseline (0223) already created `course_paces` and
-- `course_pace_module_items`, and 0227 pointed the module-item foreign
-- key at Canvas' polymorphic `content_tags`. The workspace keeps the
-- course's module items in `module_items` (0002) — this migration
-- repoints the constraint there and aligns the three columns the pacing
-- routes write (`end_date` a timestamptz like the API, the weighting and
-- the calendar-day count NOT NULL with their Canvas defaults).
-- Nothing is dropped: the blackout dates (0091) already exist.

ALTER TABLE course_paces
    ALTER COLUMN end_date TYPE TIMESTAMPTZ USING end_date::timestamptz;

-- The pacing API serializes every timestamp as ISO8601 UTC (DateTime<Utc>);
-- the baseline created the audit columns as `timestamp without time zone`.
ALTER TABLE course_paces
    ALTER COLUMN created_at TYPE TIMESTAMPTZ USING created_at::timestamptz,
    ALTER COLUMN updated_at TYPE TIMESTAMPTZ USING updated_at::timestamptz;

ALTER TABLE course_pace_module_items
    ALTER COLUMN created_at TYPE TIMESTAMPTZ USING created_at::timestamptz,
    ALTER COLUMN updated_at TYPE TIMESTAMPTZ USING updated_at::timestamptz;

ALTER TABLE course_paces
    ALTER COLUMN assignments_weighting SET DEFAULT '[]',
    ALTER COLUMN assignments_weighting SET NOT NULL;

ALTER TABLE course_paces
    ALTER COLUMN time_to_complete_calendar_days SET DEFAULT 0,
    ALTER COLUMN time_to_complete_calendar_days SET NOT NULL;

ALTER TABLE course_pace_module_items
    DROP CONSTRAINT IF EXISTS fk_course_pace_module_items_module_item_id;

ALTER TABLE course_pace_module_items
    ADD CONSTRAINT fk_course_pace_module_items_module_item_id
    FOREIGN KEY (module_item_id) REFERENCES module_items (id) ON DELETE CASCADE;

CREATE UNIQUE INDEX IF NOT EXISTS idx_course_paces_primary_plan
    ON course_paces (course_id)
    WHERE course_section_id IS NULL AND user_id IS NULL AND workflow_state = 'active';

CREATE UNIQUE INDEX IF NOT EXISTS idx_course_paces_section_plan
    ON course_paces (course_section_id)
    WHERE course_section_id IS NOT NULL AND workflow_state = 'active';

CREATE UNIQUE INDEX IF NOT EXISTS idx_course_paces_user_plan
    ON course_paces (course_id, user_id)
    WHERE user_id IS NOT NULL AND workflow_state = 'active';

CREATE INDEX IF NOT EXISTS idx_course_paces_course ON course_paces (course_id);

CREATE INDEX IF NOT EXISTS idx_course_pace_module_items_pace
    ON course_pace_module_items (course_pace_id);
