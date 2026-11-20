-- Migration 0217: password reset tokens.
--
-- Backing store for the star-ui forgot/reset password journey (issue #70;
-- specs/ui-research/canvas-reference.md §2 "Password reset").
--
-- Canvas source of truth (app/controllers/pseudonyms_controller.rb +
-- communication_channel#forgot_password!):
--
--   GET|POST /forgot_password                    -> ask for the account email
--   GET  /pseudonyms/:id/change_password/:nonce  -> reset form
--   POST /pseudonyms/:id/change_password/:nonce  -> set the new password
--
-- Canvas stores the nonce on `communication_channels.confirmation_code`
-- with `confirmation_code_expires_at` and reuses the confirmation flow.
-- star-lms has no communication_channels table, so the journey gets its
-- own dedicated, minimal `password_reset_tokens` table (first
-- password-reset path anywhere in the repo — no star-auth endpoint
-- existed before this; see the PR notes for #70).
--
-- Semantics implemented in star-ui:
--   * token       — 128-bit random hex generated server-side (rand OsRng),
--                   never derived from any user attribute (not guessable).
--   * expires_at  — 2 hours after issue (Canvas default for confirmation
--                   codes; tighter than the 7-day session JWT on purpose).
--   * used_at     — single-use marker; stamped when the password is
--                   actually changed, so a used link cannot replay.
--   * user_id     — ON DELETE CASCADE so deleted users take their tokens.

CREATE TABLE IF NOT EXISTS password_reset_tokens (
    id         BIGSERIAL PRIMARY KEY,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token      TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at    TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Expired/used tokens are looked up by token only; the index on user_id
-- exists so a user's outstanding tokens can be invalidated cheaply.
CREATE INDEX IF NOT EXISTS idx_password_reset_tokens_user
    ON password_reset_tokens (user_id);
