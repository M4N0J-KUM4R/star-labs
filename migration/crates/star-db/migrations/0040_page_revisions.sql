-- Migration 0040: Wiki page revisions (Canvas simply_versioned parity).
-- page_revisions mirrors Canvas SimplyVersioned::Version rows for a WikiPage:
--   - page_id     the versioned page (cascade with the page)
--   - revision_id the per-page version number, 1-based, newest = MAX
--   - title/body/url  page state captured by that revision
--   - user_id     the editor (`version.model.user` -> `edited_by`)
-- Rows are append-only: page create/update appends a revision; nothing in the
-- API deletes revision history.

CREATE TABLE IF NOT EXISTS page_revisions (
    id          BIGSERIAL PRIMARY KEY,
    page_id     BIGINT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    revision_id INTEGER NOT NULL,
    title       TEXT NOT NULL,
    body        TEXT,
    url         TEXT,
    user_id     BIGINT REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Version numbers are unique per page (Canvas `versions.number`) and the
-- index also serves newest-first history reads for one page.
CREATE UNIQUE INDEX IF NOT EXISTS idx_page_revisions_page_revision
    ON page_revisions(page_id, revision_id);
