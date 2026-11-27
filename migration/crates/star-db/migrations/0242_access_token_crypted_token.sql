-- Migration 0263: the access-token lookup key.
--
-- Canvas never stores an access token in the clear. The `token=` setter writes
-- `crypted_token = AccessToken.hashed_token(token)` — an HMAC-SHA1 of the token
-- keyed with the app secret (app/models/access_token.rb:176-180 and
-- app/models/access_token.rb:312-315) — and `AccessToken.authenticate` looks
-- the row up by that hash (app/models/access_token.rb:151-168).
--
-- The port resolves the caller from the `access_tokens` row the same way, so
-- the row has to carry the hash. `token` keeps the token value itself: the
-- port's own listing and hint lookup read that column, and it is the value
-- the owner sees once, when the token is created.

ALTER TABLE access_tokens
    ADD COLUMN IF NOT EXISTS crypted_token TEXT;

CREATE UNIQUE INDEX IF NOT EXISTS idx_access_tokens_crypted_token
    ON access_tokens(crypted_token) WHERE crypted_token IS NOT NULL;
