-- Migration 0238: the nonces of the LTI 1.1 requests.
--
-- The LTI 1.1 spec asks the tool provider to reject a request whose nonce
-- was used before; Canvas keeps the nonces of the last 90 minutes
-- (`app/controllers/lti_api_controller.rb`, `Lti::Security.check_and_store_nonce`).
--
-- The workspace keeps the nonces in their own table so the check survives
-- the restart of the server.

CREATE TABLE IF NOT EXISTS lti_nonces (
    consumer_key TEXT NOT NULL,
    nonce        TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (consumer_key, nonce)
);

CREATE INDEX IF NOT EXISTS idx_lti_nonces_created_at ON lti_nonces (created_at);
