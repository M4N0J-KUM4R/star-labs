-- Migration 0069: per-user discussion topic subscriptions.
-- Mirrors Canvas' DiscussionTopicParticipant `subscribed` column (boolean,
-- nullable there; explicit rows only here) split into a dedicated table:
--   - discussion_subscriptions (topic_id, user_id, subscribed)
-- PUT    .../discussion_topics/:topic_id/subscribed upserts subscribed = true
-- DELETE .../discussion_topics/:topic_id/subscribed upserts subscribed = false
-- (only when the current state differs, matching Canvas' no-op early return).

CREATE TABLE IF NOT EXISTS discussion_subscriptions (
    id         BIGSERIAL PRIMARY KEY,
    topic_id   BIGINT NOT NULL REFERENCES discussion_topics(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    subscribed BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_discussion_subscriptions_topic_user
    ON discussion_subscriptions(topic_id, user_id);
CREATE INDEX IF NOT EXISTS idx_discussion_subscriptions_user_id
    ON discussion_subscriptions(user_id);
