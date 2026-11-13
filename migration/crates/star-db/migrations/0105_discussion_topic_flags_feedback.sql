-- Migration 0105: discussion topic moderation flags + summary feedback.
--
-- Canvas parity for the discussion topics API:
--   - discussion_topics.pinned / locked  Canvas `DiscussionTopic#pinned` and
--     `DiscussionTopic#locked`, set through PUT .../discussion_topics/{id}
--     (`published`, `pinned`, `locked`) and reset to false by `duplicate`.
--   - discussion_topic_summary_feedback  Canvas `DiscussionTopicSummaryFeedback`,
--     written by POST .../summaries/{summaryId}/feedback (`_action` seen /
--     like / dislike / add_comment / reset_like / disable_summary).
--
-- The feedback table keeps one row per (summary, user) like Canvas does: the
-- endpoint upserts the caller's latest like/dislike/comment state.

ALTER TABLE discussion_topics
    ADD COLUMN IF NOT EXISTS pinned BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE discussion_topics
    ADD COLUMN IF NOT EXISTS locked BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE IF NOT EXISTS discussion_topic_summary_feedback (
    id         BIGSERIAL PRIMARY KEY,
    summary_id BIGINT NOT NULL REFERENCES discussion_topic_summaries(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    liked      BOOLEAN NOT NULL DEFAULT FALSE,
    disliked   BOOLEAN NOT NULL DEFAULT FALSE,
    comment    TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (summary_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_discussion_topic_summary_feedback_user
    ON discussion_topic_summary_feedback(user_id);
