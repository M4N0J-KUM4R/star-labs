-- Migration 0031: Canvas role_overrides — per-permission enable/lock settings
-- stored on an account and inherited by its sub-accounts.
-- Canvas parity: RoleOverride model (`context` is polymorphic; star-lms only
-- stores Account contexts, matching Canvas's account role permissions UI).

CREATE TABLE IF NOT EXISTS role_overrides (
    id           BIGSERIAL PRIMARY KEY,
    permission   TEXT NOT NULL,
    enabled      BOOLEAN NOT NULL DEFAULT TRUE,
    locked       BOOLEAN NOT NULL DEFAULT FALSE,
    context_type TEXT NOT NULL DEFAULT 'Account',
    context_id   BIGINT NOT NULL,
    role_id      BIGINT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Canvas upserts one override per (context, role, permission).
CREATE UNIQUE INDEX IF NOT EXISTS idx_role_overrides_unique
    ON role_overrides (context_type, context_id, role_id, permission);

-- Account-chain walk used when resolving inherited overrides.
CREATE INDEX IF NOT EXISTS idx_role_overrides_context
    ON role_overrides (context_type, context_id);

-- Permission lookup for a role.
CREATE INDEX IF NOT EXISTS idx_role_overrides_role_permission
    ON role_overrides (role_id, permission);
