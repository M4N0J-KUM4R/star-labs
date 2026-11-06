-- Migration 0018: Canvas Groups + Group Memberships
--
-- Canvas API v1 parity:
--   POST /courses/{courseId}/groups        -> create group
--   GET  /courses/{courseId}/groups        -> list course groups (excludes deleted)
--   GET  /groups/{id}                     -> get single group
--   PUT  /groups/{id}                     -> update group (teacher/admin)
--   DELETE /groups/{id}                   -> soft-delete group (teacher/admin)
--   POST /groups/{groupId}/memberships    -> add user to group (teacher/admin)
--   GET  /groups/{groupId}/memberships    -> list memberships (teacher/admin, excludes deleted)
--   DELETE /groups/{groupId}/memberships/{membershipId} -> soft-delete membership
--
-- Group join_level: invitation_only | parent_context_auto_join | parent_context_request
-- Group workflow_state: available | deleted
-- Membership workflow_state: accepted | deleted

-- Groups table
CREATE TABLE IF NOT EXISTS groups (
    id               BIGSERIAL PRIMARY KEY,
    course_id        BIGINT REFERENCES courses(id) ON DELETE SET NULL,
    -- account_id is out of scope; NULL course_id = account-level group (not implemented)
    name             TEXT NOT NULL,
    description      TEXT,
    workflow_state    TEXT NOT NULL DEFAULT 'available',
    join_level       TEXT NOT NULL DEFAULT 'invitation_only',
    group_category_id BIGINT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_groups_course_id ON groups(course_id);
CREATE INDEX IF NOT EXISTS idx_groups_workflow_state ON groups(workflow_state);

-- Group memberships table
CREATE TABLE IF NOT EXISTS group_memberships (
    id            BIGSERIAL PRIMARY KEY,
    group_id      BIGINT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id       BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workflow_state TEXT NOT NULL DEFAULT 'accepted',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (group_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_group_memberships_group_id ON group_memberships(group_id);
CREATE INDEX IF NOT EXISTS idx_group_memberships_user_id ON group_memberships(user_id);
CREATE INDEX IF NOT EXISTS idx_group_memberships_workflow_state ON group_memberships(workflow_state);
