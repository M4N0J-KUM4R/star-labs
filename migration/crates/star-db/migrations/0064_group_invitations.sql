-- Migration 0064: Canvas group invitations / memberships extras / group files
--
-- Canvas API v1 parity:
--   GET    /groups/{groupId}/users        -> group users (groups#users)
--   POST   /groups/{groupId}/users        -> add membership (group_memberships#create)
--   DELETE /groups/{groupId}/users        -> bulk remove (destroy_bulk)
--   DELETE /groups/{groupId}/users/{id}   -> leave/remove (destroy)
--   POST   /groups/{groupId}/invite       -> invite by email/user id (groups#invite)
--   GET    /groups/{groupId}/files        -> group file metadata (files#api_index)
--   GET    /users/self/groups             -> caller's active groups (groups#index)
--
-- Canvas group_memberships carries a unique uuid (invitation token) and a
-- moderator flag; both are added here without touching existing rows' state.
-- Group files are metadata-only (no upload/storage), scoped to a group so they
-- never leak into course file listings.

-- Canvas GroupMembership#uuid: NOT NULL, unique.
ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS uuid TEXT;
UPDATE group_memberships
SET uuid = md5(random()::text || clock_timestamp()::text || id::text)
WHERE uuid IS NULL;
ALTER TABLE group_memberships
    ALTER COLUMN uuid SET DEFAULT md5(random()::text || clock_timestamp()::text);
ALTER TABLE group_memberships ALTER COLUMN uuid SET NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_group_memberships_uuid
    ON group_memberships(uuid);

-- Canvas GroupMembership#moderator.
ALTER TABLE group_memberships
    ADD COLUMN IF NOT EXISTS moderator BOOLEAN NOT NULL DEFAULT FALSE;

-- Invitations issued through POST /groups/{groupId}/invite. Matched invitees
-- also get a group_memberships row; unmatched emails stay pending here.
CREATE TABLE IF NOT EXISTS group_invitations (
    id             BIGSERIAL PRIMARY KEY,
    group_id       BIGINT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    email          TEXT NOT NULL,
    user_id        BIGINT REFERENCES users(id) ON DELETE SET NULL,
    invited_by_id  BIGINT REFERENCES users(id) ON DELETE SET NULL,
    workflow_state TEXT NOT NULL DEFAULT 'pending',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_group_invitations_group_id
    ON group_invitations(group_id);
CREATE INDEX IF NOT EXISTS idx_group_invitations_email
    ON group_invitations(lower(email));

-- Group-scoped file metadata (Canvas attachments with a Group context).
CREATE TABLE IF NOT EXISTS group_files (
    id           BIGSERIAL PRIMARY KEY,
    group_id     BIGINT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    uploader_id  BIGINT REFERENCES users(id) ON DELETE SET NULL,
    folder       TEXT NOT NULL DEFAULT 'group files',
    filename     TEXT NOT NULL,
    display_name TEXT,
    content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
    size         BIGINT NOT NULL DEFAULT 0,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_group_files_group_id
    ON group_files(group_id);
