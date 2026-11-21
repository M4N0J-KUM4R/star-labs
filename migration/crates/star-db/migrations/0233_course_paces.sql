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
-- Nothing is dropped: the blackout dates (0091), the module items and the
-- progress rows (0127) already exist and are referenced from here.

CREATE TABLE IF NOT EXISTS course_paces (
    id                              BIGSERIAL PRIMARY KEY,
    course_id                       BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    course_section_id               BIGINT REFERENCES course_sections(id) ON DELETE CASCADE,
    user_id                         BIGINT REFERENCES users(id) ON DELETE CASCADE,
    root_account_id                 BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    workflow_state                  TEXT NOT NULL DEFAULT 'unpublished',
    exclude_weekends                BOOLEAN NOT NULL DEFAULT TRUE,
    selected_days_to_skip           TEXT[] NOT NULL DEFAULT '{sat,sun}',
    hard_end_dates                  BOOLEAN NOT NULL DEFAULT FALSE,
    end_date                        TIMESTAMPTZ,
    published_at                    TIMESTAMPTZ,
    assignments_weighting           TEXT NOT NULL DEFAULT '[]',
    time_to_complete_calendar_days  INTEGER NOT NULL DEFAULT 0,
    created_at                      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                      TIMESTAMPTZ NOT NULL DEFAULT now()
);

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

CREATE TABLE IF NOT EXISTS course_pace_module_items (
    id              BIGSERIAL PRIMARY KEY,
    course_pace_id  BIGINT NOT NULL REFERENCES course_paces(id) ON DELETE CASCADE,
    module_item_id  BIGINT NOT NULL REFERENCES module_items(id) ON DELETE CASCADE,
    root_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    duration        INTEGER NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_course_pace_module_items_pace
    ON course_pace_module_items (course_pace_id);
