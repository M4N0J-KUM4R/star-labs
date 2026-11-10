-- Migration 0075: Course wiki front page marker (Canvas Wiki#front_page_url).
-- Canvas keeps the front page on the course wiki (`front_page_url` +
-- `has_no_front_page`). star-lms has no wikis table, so the designation lives
-- on the page row and GET/PUT /courses/{courseId}/front_page resolves it
-- directly:
--   - front_page  true for the one designated wiki page of a course
-- Soft-deleted front pages are ignored by the API; the partial unique index
-- only covers live rows, so a deleted front page no longer blocks setting a
-- replacement (Canvas unsets the front page on delete).

ALTER TABLE pages ADD COLUMN IF NOT EXISTS front_page BOOLEAN NOT NULL DEFAULT false;

CREATE UNIQUE INDEX IF NOT EXISTS idx_pages_course_front_page
    ON pages (course_id)
    WHERE front_page AND workflow_state <> 'deleted';
