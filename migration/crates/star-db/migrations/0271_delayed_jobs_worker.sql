-- 0345: the background-job worker's queue (MIG-04a, migration plan slice 1.5).
--
-- Canvas' background queue is the `delayed_jobs` table, created by
--   canvas-src/db/migrate/20101216224513_create_delayed_jobs.rb:26-77
-- The port already carried a schema-parity copy of that DDL in
-- 0224_canvas_schema_tables_b.sql:30-53, but nothing read or wrote it and
-- its NOT NULL columns (`queue`, `run_at`, `created_at`, `updated_at`) had
-- no defaults, so a row could not be seeded directly.
--
-- This migration
--
--   * makes `delayed_jobs` directly seedable — the defaults are the values
--     Canvas' `Delayed::Job` model sets in Ruby (`queue` = "canvas_queue",
--     `config/initializers/delayed_job.rb:80`; `run_at` = now),
--   * adds Canvas' claim index `get_delayed_jobs_index`
--     (20101216224513_create_delayed_jobs.rb:62-64),
--   * adds the port's projection columns (`workflow_state`, `progress` and
--     the context columns) so the Jobs API can report Canvas' field set from
--     the very rows the worker runs,
--   * adds the `job_queue` read model over both job tables.
--
-- Additive only: no DROP, no TRUNCATE, no renumbering of migration numbers.

-- ---------------------------------------------------------------------------
-- The Canvas queue table (idempotent: 0224 already created this shape).
-- ---------------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS delayed_jobs (
    id                       BIGSERIAL PRIMARY KEY,
    priority                 integer DEFAULT 0,
    attempts                 integer DEFAULT 0,
    handler                  text,
    last_error               text,
    queue                    varchar(255) NOT NULL,
    run_at                   timestamp NOT NULL,
    locked_at                timestamp,
    failed_at                timestamp,
    locked_by                varchar(255),
    created_at               timestamp NOT NULL,
    updated_at               timestamp NOT NULL,
    tag                      varchar(255),
    max_attempts             integer,
    strand                   varchar(255),
    next_in_strand           boolean NOT NULL DEFAULT true,
    shard_id                 bigint,
    source                   varchar(255),
    max_concurrent           integer NOT NULL DEFAULT 1,
    expires_at               timestamp,
    strand_order_override    integer NOT NULL DEFAULT 0,
    singleton                varchar
);

-- The values Canvas' model writes in Ruby, so a row can be seeded directly.
ALTER TABLE delayed_jobs ALTER COLUMN queue      SET DEFAULT 'canvas_queue';
ALTER TABLE delayed_jobs ALTER COLUMN run_at     SET DEFAULT now();
ALTER TABLE delayed_jobs ALTER COLUMN created_at SET DEFAULT now();
ALTER TABLE delayed_jobs ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE delayed_jobs ALTER COLUMN attempts   SET DEFAULT 0;
ALTER TABLE delayed_jobs ALTER COLUMN priority   SET DEFAULT 0;

-- ---------------------------------------------------------------------------
-- Projection columns: the port's Jobs API reports `workflow_state` and
-- `completion_percentage`, which Canvas derives from the queue columns
-- (queued / running / future / failed, see the worker spec) and the port
-- stores explicitly.
-- ---------------------------------------------------------------------------

ALTER TABLE delayed_jobs ADD COLUMN IF NOT EXISTS workflow_state  TEXT NOT NULL DEFAULT 'queued';
ALTER TABLE delayed_jobs ADD COLUMN IF NOT EXISTS progress        INTEGER NOT NULL DEFAULT 0;
ALTER TABLE delayed_jobs ADD COLUMN IF NOT EXISTS user_id         BIGINT;
ALTER TABLE delayed_jobs ADD COLUMN IF NOT EXISTS context_type    TEXT;
ALTER TABLE delayed_jobs ADD COLUMN IF NOT EXISTS context_id      BIGINT;
ALTER TABLE delayed_jobs ADD COLUMN IF NOT EXISTS original_job_id BIGINT;
ALTER TABLE delayed_jobs ADD COLUMN IF NOT EXISTS requeued_job_id BIGINT;

-- The failed-job archive gets the same projection columns so a row copied
-- out of the queue keeps reporting a state the API can show.
ALTER TABLE failed_jobs ADD COLUMN IF NOT EXISTS workflow_state  TEXT NOT NULL DEFAULT 'failed';
ALTER TABLE failed_jobs ADD COLUMN IF NOT EXISTS progress        INTEGER NOT NULL DEFAULT 0;
ALTER TABLE failed_jobs ADD COLUMN IF NOT EXISTS user_id         BIGINT;
ALTER TABLE failed_jobs ADD COLUMN IF NOT EXISTS context_type    TEXT;
ALTER TABLE failed_jobs ADD COLUMN IF NOT EXISTS context_id      BIGINT;
ALTER TABLE failed_jobs ADD COLUMN IF NOT EXISTS next_in_strand   BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE failed_jobs ADD COLUMN IF NOT EXISTS max_concurrent   INTEGER NOT NULL DEFAULT 1;

-- ---------------------------------------------------------------------------
-- Claim index — Canvas' partial index over (priority, run_at, id) for the
-- rows that are due, unlocked and next in their strand
-- (20101216224513_create_delayed_jobs.rb:62-64).
-- ---------------------------------------------------------------------------

CREATE INDEX IF NOT EXISTS get_delayed_jobs_index
    ON delayed_jobs (priority, run_at, id)
    WHERE queue = 'canvas_queue' AND locked_at IS NULL AND next_in_strand;

-- ---------------------------------------------------------------------------
-- `job_queue` — the read model the Jobs API reports from.
--
-- It covers both job tables: the port's own `jobs` table
-- (0033_jobs.sql + 0057_jobs_v2.sql) and Canvas' `delayed_jobs` queue, so
-- `GET /jobs` reports the rows the worker actually runs. `id` is the id of
-- the source table, so the two id spaces stay distinct.
-- ---------------------------------------------------------------------------

CREATE OR REPLACE VIEW job_queue AS
    SELECT id, tag, workflow_state, progress, created_at, updated_at, user_id,
           context_type, context_id, strand, singleton, shard_id, max_concurrent,
           priority, attempts, max_attempts, locked_by, locked_at, run_at,
           handler, failed_at, original_job_id, requeued_job_id, last_error,
           next_in_strand, COALESCE(queue, 'canvas_queue')
      FROM jobs
    UNION ALL
    -- The queue table stores naive timestamps (Canvas' `t.timestamp`) and
    -- leaves the job columns nullable, so they are normalized here: the
    -- timestamps are read in the frame the port writes them in and the
    -- columns the port's `jobs` table declares NOT NULL fall back to the
    -- values Canvas' worker uses.
    SELECT id, COALESCE(tag, ''), workflow_state, progress,
           created_at::timestamptz, updated_at::timestamptz, user_id,
           context_type, context_id, strand, singleton, shard_id, max_concurrent,
           COALESCE(priority, 0), COALESCE(attempts, 0),
           COALESCE(max_attempts, 1), locked_by,
           locked_at::timestamptz, run_at::timestamptz,
           handler, failed_at::timestamptz, original_job_id, requeued_job_id,
           last_error, next_in_strand, queue
      FROM delayed_jobs;
