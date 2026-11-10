-- Migration 0087: page duplication provenance (Canvas WikiPage#duplicate).
-- Canvas `POST /courses/:course_id/pages/:url_or_id/duplicate` builds a new
-- unpublished wiki page from an existing one. star-lms stores the copy as a
-- regular row in `pages`; this column records which page it was copied from
-- so the relationship survives updates of the original.

ALTER TABLE pages ADD COLUMN IF NOT EXISTS duplicated_from_page_id BIGINT;

CREATE INDEX IF NOT EXISTS idx_pages_duplicated_from_page_id
    ON pages (duplicated_from_page_id)
    WHERE duplicated_from_page_id IS NOT NULL;
