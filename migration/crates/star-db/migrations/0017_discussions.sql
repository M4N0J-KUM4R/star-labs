-- Migration 0017: Discussion topics and threaded entries.
-- discussion_topics mirrors Canvas DiscussionTopic:
--   - id, course_id, title, message, workflow_state ('active'|'deleted'),
--   - discussion_type ('side_comment'|'threaded'|'flat'|'not_threaded')
--   - created_at/updated_at
-- discussion_entries mirrors Canvas DiscussionEntry:
--   - id, topic_id FK, parent_id FK (NULL for root entries),
--   - user_id, message, workflow_state ('active'|'deleted'), created_at/updated_at

CREATE TABLE IF NOT EXISTS discussion_topics (
    id               BIGSERIAL PRIMARY KEY,
    course_id        BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    title            TEXT NOT NULL,
    message          TEXT,
    workflow_state   TEXT NOT NULL DEFAULT 'active',
    discussion_type TEXT NOT NULL DEFAULT 'side_comment'
                     CHECK (discussion_type IN ('side_comment','threaded','flat','not_threaded')),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_discussion_topics_course_id ON discussion_topics(course_id);
CREATE INDEX IF NOT EXISTS idx_discussion_topics_workflow_state ON discussion_topics(workflow_state);

CREATE TABLE IF NOT EXISTS discussion_entries (
    id            BIGSERIAL PRIMARY KEY,
    topic_id      BIGINT NOT NULL REFERENCES discussion_topics(id) ON DELETE CASCADE,
    parent_id     BIGINT REFERENCES discussion_entries(id) ON DELETE CASCADE,
    user_id       BIGINT REFERENCES users(id) ON DELETE SET NULL,
    message       TEXT,
    workflow_state TEXT NOT NULL DEFAULT 'active',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_discussion_entries_topic_id ON discussion_entries(topic_id);
CREATE INDEX IF NOT EXISTS idx_discussion_entries_parent_id ON discussion_entries(parent_id);
CREATE INDEX IF NOT EXISTS idx_discussion_entries_workflow_state ON discussion_entries(workflow_state);
