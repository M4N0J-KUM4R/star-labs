-- Migration 0024: accounts — Canvas-style account hierarchy + account admins
-- Canvas API v1 multi-tenancy model.

-- accounts table: hierarchical organization structure
CREATE TABLE IF NOT EXISTS accounts (
    id                BIGSERIAL PRIMARY KEY,
    name              TEXT NOT NULL,
    parent_account_id BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    workflow_state    TEXT NOT NULL DEFAULT 'active',
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- account_users table: maps users to accounts with a role (admin by default)
CREATE TABLE IF NOT EXISTS account_users (
    id           BIGSERIAL PRIMARY KEY,
    account_id   BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    user_id      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role         TEXT NOT NULL DEFAULT 'admin',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, user_id)
);

-- Add account_id to courses if it doesn't already exist
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'courses' AND column_name = 'account_id'
    ) THEN
        ALTER TABLE courses ADD COLUMN account_id BIGINT REFERENCES accounts(id) DEFAULT 1;
    END IF;
END $$;

-- Index for fast account lookups
CREATE INDEX IF NOT EXISTS idx_accounts_parent ON accounts(parent_account_id);
CREATE INDEX IF NOT EXISTS idx_account_users_account ON account_users(account_id);
CREATE INDEX IF NOT EXISTS idx_account_users_user ON account_users(user_id);

-- Seed: Canvas always has a root account with id = 1 named "Root Account"
-- Use ON CONFLICT to be idempotent (handles re-runs)
INSERT INTO accounts (id, name, workflow_state)
VALUES (1, 'Root Account', 'active')
ON CONFLICT (id) DO NOTHING;
