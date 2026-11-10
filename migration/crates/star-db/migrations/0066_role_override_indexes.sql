-- Migration 0066: role override indexes for the account/course
-- role_overrides endpoints (Phase 3).
--
-- GET /accounts/{accountId}/role_overrides and
-- GET /courses/{courseId}/role_overrides list one context's overrides ordered
-- by permission; PUT upserts them on (context, role, permission), which the
-- existing idx_role_overrides_unique already covers. These are additive
-- indexes only: no table or column changes.

-- Ordered listing for a single context (Account or Course).
CREATE INDEX IF NOT EXISTS idx_role_overrides_context_permission
    ON role_overrides (context_type, context_id, permission, role_id);

-- Deterministic "changed since" reads for the same listing.
CREATE INDEX IF NOT EXISTS idx_role_overrides_context_updated_at
    ON role_overrides (context_type, context_id, updated_at DESC);
