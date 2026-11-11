-- Migration 0079: deep discussion features (Canvas parity).
--
-- Adds the pieces Canvas keeps on DiscussionTopic / DiscussionTopicSummary
-- that the deep discussion endpoints need:
--   - discussion_topics.group_id         Canvas `belongs_to :context, polymorphic: [:course, :group]`
--   - discussion_topics.summary_enabled  Canvas `summary_enabled`, set by find_or_create_summary
--   - discussion_topic_summaries         Canvas `discussion_topic_summaries`
--
-- Canvas stores two rows per generated summary: the "raw" summary and the
-- "refined" summary that points back at it through `parent_id`.
-- `find_summary` only answers when both rows exist.

ALTER TABLE discussion_topics
    ADD COLUMN IF NOT EXISTS group_id BIGINT REFERENCES groups(id) ON DELETE CASCADE;

ALTER TABLE discussion_topics
    ADD COLUMN IF NOT EXISTS summary_enabled BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX IF NOT EXISTS idx_discussion_topics_group_id ON discussion_topics(group_id);

CREATE TABLE IF NOT EXISTS discussion_topic_summaries (
    id                   BIGSERIAL PRIMARY KEY,
    discussion_topic_id  BIGINT NOT NULL REFERENCES discussion_topics(id) ON DELETE CASCADE,
    user_id              BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    llm_config_version   TEXT NOT NULL DEFAULT 'discussion_topic_summary_refined',
    dynamic_content_hash TEXT NOT NULL,
    user_input           TEXT,
    summary              TEXT,
    parent_id            BIGINT REFERENCES discussion_topic_summaries(id) ON DELETE SET NULL,
    locale               TEXT,
    input_tokens         INTEGER,
    output_tokens        INTEGER,
    generation_time      INTEGER,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_discussion_topic_summaries_lookup
    ON discussion_topic_summaries(discussion_topic_id, user_id, llm_config_version, created_at DESC);
