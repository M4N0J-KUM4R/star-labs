-- Feature flags (Canvas FeatureFlagsController parity).
--
-- One row per (context, feature) override. Contexts are Account, Course, or
-- User. Absent rows fall back to the crate's default catalog state
-- ('disabled'). `locking_account_id` mirrors the deprecated Canvas field and
-- is always null for the contexts this API writes.

CREATE TABLE IF NOT EXISTS feature_flags (
    id                 BIGSERIAL PRIMARY KEY,
    context_type       TEXT NOT NULL,
    context_id         BIGINT NOT NULL,
    feature            TEXT NOT NULL,
    display_name       TEXT NOT NULL,
    state              TEXT NOT NULL DEFAULT 'disabled',
    locking_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (context_type, context_id, feature)
);

CREATE INDEX IF NOT EXISTS idx_feature_flags_context
    ON feature_flags (context_type, context_id);
