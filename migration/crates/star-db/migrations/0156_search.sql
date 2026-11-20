-- Migration 0156: search slice - page_views, page_view_queries, and
-- additional courses / users / groups columns needed by the search routes.
--
-- page_views: mirrors Canvas DynamoDB-backed PageView model for
-- GET /users/{user_id}/page_views. Fields sourced from
-- lib/api/v1/page_view.rb:page_views_json.
--
-- page_view_queries: tracks async query state for
-- POST /users/{user_id}/page_views/query and its batch variant.
--
-- course columns (indexed, is_public, open_enrollment): needed by
-- GET /search/all_courses to apply Canvas course-catalog filters.
--
-- users columns (avatar_url, sortable_name): needed by search#recipients
-- to return user.full_name and avatar_url per conversation_recipients_json.
--
-- groups columns (name): needed for group recipients in search#recipients.
-- Existing groups.name was already present (migration 0018).

-- ─────────────────────────────────────────────────────────────────────────────
-- page_views — mirrors Canvas DynamoDB schema in PostgreSQL
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS page_views (
    id                      UUID PRIMARY KEY,
    user_id                 BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    app_name                TEXT,
    url                     TEXT,
    context_type            TEXT,
    asset_type              TEXT,
    controller              TEXT,
    action                  TEXT,
    contributed            BOOLEAN NOT NULL DEFAULT false,
    interaction_seconds     NUMERIC,
    created_at             TIMESTAMPTZ NOT NULL,
    user_request           BOOLEAN NOT NULL DEFAULT true,
    render_time             NUMERIC,
    participated           BOOLEAN NOT NULL DEFAULT false,
    http_method             TEXT,
    remote_ip               TEXT,
    session_id              UUID,
    developer_key_id        BIGINT,
    asset_user_access_id    BIGINT,
    links_user              BIGINT,
    links_context           BIGINT,
    links_asset             BIGINT,
    links_real_user         BIGINT,
    links_account           BIGINT
);

CREATE INDEX IF NOT EXISTS idx_page_views_user_id      ON page_views(user_id);
CREATE INDEX IF NOT EXISTS idx_page_views_created_at   ON page_views(created_at);

-- ─────────────────────────────────────────────────────────────────────────────
-- page_view_queries — async query state for PV5 page_views export
-- ─────────────────────────────────────────────────────────────────────────────

CREATE TABLE IF NOT EXISTS page_view_queries (
    query_id     UUID PRIMARY KEY,
    user_id      BIGINT NOT NULL,
    start_date   DATE NOT NULL,
    end_date     DATE NOT NULL,
    format       TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'queued'
                  CHECK (status IN ('queued', 'processing', 'finished', 'failed')),
    error_code   TEXT,
    results_data BYTEA,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_page_view_queries_user_id  ON page_view_queries(user_id);
CREATE INDEX IF NOT EXISTS idx_page_view_queries_status  ON page_view_queries(status);

-- ─────────────────────────────────────────────────────────────────────────────
-- courses — additional fields needed by GET /search/all_courses
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE courses ADD COLUMN IF NOT EXISTS indexed          BOOLEAN NOT NULL DEFAULT true;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS is_public       BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE courses ADD COLUMN IF NOT EXISTS open_enrollment BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX IF NOT EXISTS idx_courses_indexed        ON courses(indexed);
CREATE INDEX IF NOT EXISTS idx_courses_is_public      ON courses(is_public);
CREATE INDEX IF NOT EXISTS idx_courses_open_enrollment ON courses(open_enrollment);

-- ─────────────────────────────────────────────────────────────────────────────
-- users — additional fields needed by search#recipients
-- ─────────────────────────────────────────────────────────────────────────────

ALTER TABLE users ADD COLUMN IF NOT EXISTS avatar_url      TEXT;
ALTER TABLE users ADD COLUMN IF NOT EXISTS sortable_name   TEXT;
