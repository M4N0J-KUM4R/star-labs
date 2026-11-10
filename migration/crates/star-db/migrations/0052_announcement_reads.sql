-- Migration 0052: Canvas announcement read-state parity
--
-- Canvas tracks per-user read state for announcements through
-- DiscussionTopicRead (Announcement subclasses DiscussionTopic): a row exists
-- for each (topic, user) pair the user has read. Row existence means "read";
-- marking unread clears the read state. `state` is kept explicit so the same
-- table can back both PUT read and DELETE read idempotently.
--
-- API scope:
-- - PUT    /courses/{courseId}/announcements/{id}/read      -> 204
-- - DELETE /courses/{courseId}/announcements/{id}/read      -> 204
-- - GET    /courses/{courseId}/announcements/unread_count   -> {"unread_count": N}
CREATE TABLE IF NOT EXISTS announcement_reads (
    id              BIGSERIAL PRIMARY KEY,
    announcement_id BIGINT NOT NULL REFERENCES announcements(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL,
    state           TEXT   NOT NULL DEFAULT 'read'
        CHECK (state IN ('read', 'unread')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (announcement_id, user_id)
);

-- Unread-count and per-user read lookups scan by user.
CREATE INDEX IF NOT EXISTS idx_announcement_reads_user_id
    ON announcement_reads (user_id);

-- Course unread counts join announcements -> reads per user.
CREATE INDEX IF NOT EXISTS idx_announcement_reads_user_state
    ON announcement_reads (user_id, state);
