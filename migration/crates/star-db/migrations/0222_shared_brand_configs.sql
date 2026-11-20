-- Migration 0221: the shared brand configs.
--
-- Canvas `SharedBrandConfig` shares a brand config (a theme) with an
-- account: it gives the brand config a name and makes it available to the
-- other users of the account.

CREATE TABLE IF NOT EXISTS shared_brand_configs (
    id                BIGSERIAL PRIMARY KEY,
    account_id        BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    brand_config_md5  TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_shared_brand_configs_account
    ON shared_brand_configs (account_id);
