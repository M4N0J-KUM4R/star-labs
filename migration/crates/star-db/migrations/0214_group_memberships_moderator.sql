-- Migration 0214: group_memberships.moderator column
--
-- Canvas ground truth (canvas-lms):
--   db/migrate/20101210192618_init_canvas_db.rb:2793  -> t.boolean :moderator
--     (nullable boolean, no default in Canvas)
--   app/models/group_membership.rb:50                  -> scope :moderators,
--     -> { where(moderator: true) }
--   app/controllers/group_memberships_controller.rb:245 ->
--     UPDATABLE_MEMBERSHIP_ATTRIBUTES = %w[workflow_state moderator]
--     (updatable via PUT/PATCH update membership)
--   app/controllers/group_memberships_controller.rb:57-61 (API docs):
--     "moderator": Whether or not the user is a moderator of the group.
--
-- The star-groups crate reads gm.moderator via COALESCE(..., false), so a
-- NOT NULL DEFAULT false column reproduces Canvas's nil-as-false rendering
-- while keeping inserts that omit the column working (0018 inserts only
-- group_id, user_id, workflow_state).
--
-- 0018_groups.sql must NOT be edited (sqlx checksums), hence this new
-- migration.

ALTER TABLE group_memberships ADD COLUMN IF NOT EXISTS moderator BOOLEAN NOT NULL DEFAULT false;

-- Index backing Canvas's `scope :moderators` (WHERE moderator = true) lookups.
CREATE INDEX IF NOT EXISTS idx_group_memberships_moderator
    ON group_memberships (group_id) WHERE moderator;
