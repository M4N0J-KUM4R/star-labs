-- 0256: port-C parity gaps (star-courses).
--
-- Canvas sections carry original_course_id while they are crosslisted away
-- from their birth course (Canvas Section#crosslist_to_course keeps the
-- originating course id); it is emitted by the section JSON only while set.
-- Canvas assignments.post_to_sis backs the disable_post_to_sis API
-- (DisablePostToSisApiController bulk-clears the flag per course).
--
-- The other port-C tables already exist on this branch and are NOT recreated:
--   - microsoft_sync_groups           (0225 canvas schema snapshot)
--   - live_assessments_assessments /
--     live_assessments_results        (0224 canvas schema snapshot)
--   - planner_items, plugins,
--     courses.settings,
--     progresses.completion_source     (0252 misc gaps)

ALTER TABLE course_sections
    ADD COLUMN IF NOT EXISTS original_course_id BIGINT REFERENCES courses(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_course_sections_original_course
    ON course_sections (original_course_id);

ALTER TABLE assignments
    ADD COLUMN IF NOT EXISTS post_to_sis BOOLEAN NOT NULL DEFAULT FALSE;
