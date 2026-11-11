-- Migration 0083: course content shares (Canvas ContentShare).
-- Mirrors Canvas `content_shares`: a shared copy of exported content sent to
-- another user. Phase 4 courses-deep owns this file.

CREATE TABLE IF NOT EXISTS content_shares (
    id                BIGSERIAL PRIMARY KEY,
    name              TEXT NOT NULL,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    content_export_id BIGINT NOT NULL REFERENCES content_exports(id) ON DELETE CASCADE,
    sender_id         BIGINT REFERENCES users(id) ON DELETE SET NULL,
    read_state        TEXT NOT NULL DEFAULT 'unread',
    type              TEXT NOT NULL,
    root_account_id   BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, content_export_id, sender_id)
);

CREATE INDEX IF NOT EXISTS idx_content_shares_user
    ON content_shares (user_id);
CREATE INDEX IF NOT EXISTS idx_content_shares_content_export
    ON content_shares (content_export_id);
