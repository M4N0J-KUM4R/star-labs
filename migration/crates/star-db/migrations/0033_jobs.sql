-- Migration 0033: jobs table for the Canvas Jobs API parity surface.
--
-- Canvas (`app/controllers/jobs_controller.rb`, Jobs V1 API) exposes queued /
-- running / future / failed Delayed::Job rows to site admins. This workspace
-- keeps the durable record only: a job row is enqueued for a request and an
-- external worker (later phase) is expected to advance workflow_state and
-- progress. No worker is started by star-jobs itself.
--
-- Canvas parity:
--   - workflow_state: 'queued' | 'running' | 'completed' | 'failed' | 'cancelled'
--   - progress is an integer percentage (0..=100)
--   - tag is the human-readable job tag shown in the admin Jobs UI
--   - context_type/context_id identify the Canvas context (Course, User, ...)
--
-- Idempotent (IF NOT EXISTS) so re-applying is safe when the migration is
-- applied out-of-band with psql before sqlx records it.

CREATE TABLE IF NOT EXISTS jobs (
    id             BIGSERIAL PRIMARY KEY,
    tag            TEXT NOT NULL,
    workflow_state TEXT NOT NULL DEFAULT 'queued',
    progress       INTEGER NOT NULL DEFAULT 0,
    user_id        BIGINT REFERENCES users(id) ON DELETE SET NULL,
    context_type   TEXT,
    context_id     BIGINT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT jobs_workflow_state_check
        CHECK (workflow_state IN ('queued', 'running', 'completed', 'failed', 'cancelled')),
    CONSTRAINT jobs_progress_check
        CHECK (progress >= 0 AND progress <= 100)
);

-- Listing/filtering by state (admin Jobs UI buckets: running, queued, failed).
CREATE INDEX IF NOT EXISTS idx_jobs_workflow_state ON jobs (workflow_state);

-- Per-user job history.
CREATE INDEX IF NOT EXISTS idx_jobs_user_id ON jobs (user_id);

-- Context-scoped lookup (jobs for a course/account/user).
CREATE INDEX IF NOT EXISTS idx_jobs_context ON jobs (context_type, context_id);

-- Newest-first pagination for GET /api/v1/jobs.
CREATE INDEX IF NOT EXISTS idx_jobs_created_at ON jobs (created_at DESC);
