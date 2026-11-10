-- Migration 0041: per-user discussion read state and entry ratings.
-- Mirrors Canvas' discussion_topic_participants / discussion_entry_participants
-- read columns (workflow_state, forced_read_state, read_at) and the participant
-- `rating` column, split into dedicated tables:
--   - discussion_topic_reads   (topic_id, user_id, workflow_state, forced_read_state, read_at)
--   - discussion_entry_reads   (entry_id, user_id, workflow_state, forced_read_state, read_at)
--   - discussion_entry_ratings (entry_id, user_id, rating: 0 = cleared, 1 = liked)

CREATE TABLE IF NOT EXISTS discussion_topic_reads (
    id                BIGSERIAL PRIMARY KEY,
    topic_id          BIGINT NOT NULL REFERENCES discussion_topics(id) ON DELETE CASCADE,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workflow_state    TEXT NOT NULL DEFAULT 'read'
                      CHECK (workflow_state IN ('read','unread')),
    forced_read_state BOOLEAN,
    read_at           TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_discussion_topic_reads_topic_user
    ON discussion_topic_reads(topic_id, user_id);
CREATE INDEX IF NOT EXISTS idx_discussion_topic_reads_user_id
    ON discussion_topic_reads(user_id);

CREATE TABLE IF NOT EXISTS discussion_entry_reads (
    id                BIGSERIAL PRIMARY KEY,
    entry_id          BIGINT NOT NULL REFERENCES discussion_entries(id) ON DELETE CASCADE,
    user_id           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workflow_state    TEXT NOT NULL DEFAULT 'read'
                      CHECK (workflow_state IN ('read','unread')),
    forced_read_state BOOLEAN,
    read_at           TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_discussion_entry_reads_entry_user
    ON discussion_entry_reads(entry_id, user_id);
CREATE INDEX IF NOT EXISTS idx_discussion_entry_reads_user_id
    ON discussion_entry_reads(user_id);

CREATE TABLE IF NOT EXISTS discussion_entry_ratings (
    id         BIGSERIAL PRIMARY KEY,
    entry_id   BIGINT NOT NULL REFERENCES discussion_entries(id) ON DELETE CASCADE,
    user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    rating     INTEGER NOT NULL DEFAULT 0 CHECK (rating IN (0, 1)),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_discussion_entry_ratings_entry_user
    ON discussion_entry_ratings(entry_id, user_id);
CREATE INDEX IF NOT EXISTS idx_discussion_entry_ratings_user_id
    ON discussion_entry_ratings(user_id);
