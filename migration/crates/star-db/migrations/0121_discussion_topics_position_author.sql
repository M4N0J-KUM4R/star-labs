-- Migration 0121: discussion topic ordering position + author reference.
--
-- Canvas parity for the discussion_topics controller routes (issue #56):
--   - discussion_topics.position  Canvas DiscussionTopic#position, managed by
--     acts_as_list within the {context, pinned: true} scope: `reorder`
--     (POST .../discussion_topics/reorder) rewrites it for the pinned topics
--     of a course or group, the reorder response orders by it
--     (`by_position`: position ASC, created_at DESC, id DESC) and the topic
--     index defaults to `by_position_legacy` (position DESC, created_at DESC,
--     id DESC; Postgres sorts NULL positions first on DESC, matching Canvas).
--     Pinned topics created (or re-pinned) get max(position of the context's
--     pinned topics) + 1, like acts_as_list's add_to_list_bottom.
--   - discussion_topics.user_id   Canvas DiscussionTopic belongs_to :user
--     (the topic author; create sets it to the current user).
--     DELETE .../discussion_topics/{id} renders the destroyed topic as_json
--     including the author's name (discussion_topics_controller.rb destroy:
--     `render json: @topic.as_json(include: { user: { only: :name } })`).

ALTER TABLE discussion_topics
    ADD COLUMN IF NOT EXISTS position BIGINT;

ALTER TABLE discussion_topics
    ADD COLUMN IF NOT EXISTS user_id BIGINT REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_discussion_topics_position ON discussion_topics(position);
CREATE INDEX IF NOT EXISTS idx_discussion_topics_user_id ON discussion_topics(user_id);
