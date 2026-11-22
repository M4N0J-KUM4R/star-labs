-- 0232: authentication audit events (parity gap —
-- AuthenticationAuditApiController for_login/for_account/for_user).
-- Canvas Auditors::Authentication streams events keyed by pseudonym with
-- created_at/uuid columns; the workspace audit convention (0092
-- grade_change_events) uses plain BIGSERIAL tables with jsonb details.
CREATE TABLE IF NOT EXISTS authentication_events (
    id              BIGSERIAL PRIMARY KEY,
    pseudonym_id    BIGINT NOT NULL,
    user_id         BIGINT NOT NULL,
    account_id      BIGINT,
    event_type      TEXT,          -- 'login' | 'logout' (Canvas pseudonym_id/created_at rows)
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_authentication_events_pseudonym
    ON authentication_events (pseudonym_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_authentication_events_account
    ON authentication_events (account_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_authentication_events_user
    ON authentication_events (user_id, created_at DESC);
