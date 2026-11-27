-- 0131_smart_search.sql — smart search per-course index tracking (issue #91,
-- spec specs/p5-rest-search-analytics.md §4/§6).
--
-- Canvas tracks per-course vector-index readiness in
-- `SmartSearch.check_course(course)` → (ready, progress) backed by the
-- courses.search_embedding_version column plus per-content embedding tables
-- (wiki_page_embeddings, assignment_embeddings, discussion_topic_embeddings —
-- pgvector tables this port does not create, per the 0223 snapshot header).
-- The API surface is what the spec pins: `index_status` answers
-- {"status": "indexed"|"indexing", "progress": <number>} and `search` adds
-- status/indexing_progress while the index is incomplete.
--
-- star-lms has no embedding backend and no worker (misc-index spec §3:
-- shared job-model convention — "domain row" half only). This migration
-- stores the readiness domain row per course: `status` CHECK-constrained to
-- Canvas' two API states and `progress` as the 0-100 percentage the API
-- reports. A course with no row reads as "indexing, 0" (Canvas' first-touch
-- behavior: check_course queues the index job and reports the live progress
-- — 0 before anything is embedded); the first search/index_status touch
-- marks the course indexed (first-cut approach in spec §4.1: a trivial
-- implementation that "marks courses indexed, progress 100 on first touch
-- and returns keyword-search results computed with SQL ILIKE"). Tests flip
-- status/progress directly in SQL, the same way the content-migration and
-- polling tests do.
--
-- No smart_search_logs table: the `log` route is a Canvas telemetry no-op
-- ("TODO: do something more"); the spec §6 recommends discarding.

CREATE TABLE IF NOT EXISTS smart_search_indexes (
    course_id  BIGINT PRIMARY KEY REFERENCES courses(id) ON DELETE CASCADE,
    status     TEXT NOT NULL DEFAULT 'indexing'
               CHECK (status IN ('indexed', 'indexing')),
    progress   DOUBLE PRECISION NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
