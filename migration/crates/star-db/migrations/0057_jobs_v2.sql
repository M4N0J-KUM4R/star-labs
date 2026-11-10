-- Migration 0057: Jobs V2 parity surface (Canvas `JobsV2Controller`).
--
-- Canvas (`app/controllers/jobs_v2_controller.rb`) exposes the delayed-job
-- queue as four buckets (queued / running / future / failed), a per-id lookup
-- that also matches `original_job_id`, requeue of failed jobs, and a
-- per-cluster jobs dashboard. This workspace keeps the durable record only
-- (no worker), so the jobs table is extended with the Delayed::Job columns
-- the V2 JSON shape exposes. Nothing is dropped: 0033's columns stay intact
-- and `GET /api/v1/jobs` keeps reading them.
--
-- Existing rows pick up the defaults (run_at = now(), attempts = 0,
-- next_in_strand = true), so the V1 surface is unaffected.

ALTER TABLE jobs ADD COLUMN IF NOT EXISTS strand         TEXT;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS singleton      TEXT;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS shard_id       BIGINT;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS max_concurrent INTEGER NOT NULL DEFAULT 1;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS priority       INTEGER NOT NULL DEFAULT 0;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS attempts       INTEGER NOT NULL DEFAULT 0;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS max_attempts   INTEGER NOT NULL DEFAULT 1;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS locked_by      TEXT;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS locked_at      TIMESTAMPTZ;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS run_at         TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS handler        TEXT;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS failed_at      TIMESTAMPTZ;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS original_job_id BIGINT;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS requeued_job_id BIGINT;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS last_error     TEXT;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS next_in_strand  BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS queue           TEXT;

-- Bucket scans (`queued` filters on run_at, `running` on locked_at,
-- `failed` orders by failed_at).
CREATE INDEX IF NOT EXISTS idx_jobs_jobs2_bucket
    ON jobs (workflow_state, run_at);
CREATE INDEX IF NOT EXISTS idx_jobs_jobs2_failed_at
    ON jobs (failed_at DESC)
    WHERE failed_at IS NOT NULL;

-- Strand/admin filters (PUT /jobs2/manage equivalents) and cluster rollups.
CREATE INDEX IF NOT EXISTS idx_jobs_strand ON jobs (strand);
CREATE INDEX IF NOT EXISTS idx_jobs_shard_id ON jobs (shard_id);
CREATE INDEX IF NOT EXISTS idx_jobs_original_job_id ON jobs (original_job_id);

-- Canvas enumerates job clusters (delayed-jobs shards) for the Jobs dashboard.
-- The flat workspace equivalent records one row per cluster; the endpoint
-- derives `block_stranded_shard_ids` / `jobs_held_shard_ids` from the flags
-- and job `counts` from the jobs carrying the matching `shard_id`.
CREATE TABLE IF NOT EXISTS job_clusters (
    id                 BIGSERIAL PRIMARY KEY,
    database_server_id TEXT NOT NULL,
    domain             TEXT NOT NULL,
    block_stranded     BOOLEAN NOT NULL DEFAULT FALSE,
    jobs_held          BOOLEAN NOT NULL DEFAULT FALSE,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- `database_server_id` is the cluster identity in Canvas; keep it unique so
-- re-applying or re-seeding cannot create duplicates.
CREATE UNIQUE INDEX IF NOT EXISTS idx_job_clusters_database_server_id
    ON job_clusters (database_server_id);
