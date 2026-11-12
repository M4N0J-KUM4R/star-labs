-- Migration 0089: account admin deep — Canvas terms of service.
--
-- Canvas renders GET /acceptable_use_policy from the root account's
-- TermsOfService (`{content}` or `{redirectUrl}`). Store the per-account
-- policy so that route has a real source of truth instead of a hardcoded
-- response.
--
-- Backs the Phase 4 routes:
--   PUT    /api/v1/accounts/:id
--   GET    /api/v1/manually_created_courses_account
--   DELETE /api/v1/accounts/:account_id/users/:user_id
--   GET    /api/v1/acceptable_use_policy

CREATE TABLE IF NOT EXISTS account_terms_of_service (
    id           BIGSERIAL PRIMARY KEY,
    account_id   BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    terms_type   TEXT,
    content      TEXT,
    external_url TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id)
);
