-- Migration 0139: section cross-list support (nonxlist_course_id).
--   POST /sections/{id}/crosslist/{newCourseId}
--   DELETE /sections/{id}/crosslist
--
-- When a section is cross-listed, Canvas stores the original course id in
-- nonxlist_course_id before moving it to the new course. This column is
-- needed to support uncrosslisting (restore original course) and to
-- detect whether a section is currently cross-listed.

ALTER TABLE course_sections ADD COLUMN IF NOT EXISTS nonxlist_course_id BIGINT REFERENCES courses(id);
