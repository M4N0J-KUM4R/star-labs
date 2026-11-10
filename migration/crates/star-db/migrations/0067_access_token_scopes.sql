-- Migration 0067: Canvas personal access tokens (GET/POST/DELETE /api/v1/tokens).
--
-- Manually generated personal access tokens live in `access_tokens`
-- (migration 0032) and are told apart from OAuth tokens by their NULL
-- `developer_key_id`. The `scopes` column already exists there; this
-- migration only adds the Canvas Token metadata used by the API:
--
-- * token_hint      first 5 characters of the token, used for lookup and for
--                   the masked `visible_token` returned after creation.
-- * remember_access Canvas Token JSON parity field (always FALSE for
--                   manually created tokens).
-- * real_user_id    id of the real user when the token was created while
--                   masquerading (NULL otherwise).
--
-- The indexes back the per-user token listing and token_hint lookups.

ALTER TABLE access_tokens
    ADD COLUMN IF NOT EXISTS token_hint TEXT;

ALTER TABLE access_tokens
    ADD COLUMN IF NOT EXISTS remember_access BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE access_tokens
    ADD COLUMN IF NOT EXISTS real_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_access_tokens_user_created
    ON access_tokens(user_id, created_at, id);

CREATE INDEX IF NOT EXISTS idx_access_tokens_user_token_hint
    ON access_tokens(user_id, token_hint) WHERE token_hint IS NOT NULL;
