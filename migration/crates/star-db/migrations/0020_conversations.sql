-- Migration 0020: conversations (inbox/DM) for star-lms.
-- Canvas API v1 parity: simplified inbox.
--
-- conversations: holds conversation metadata (subject, workflow_state).
-- conversation_messages: individual messages belonging to a conversation.
-- conversation_participants: which users are in which conversation (active/deleted).

CREATE TABLE IF NOT EXISTS conversations (
    id            BIGSERIAL PRIMARY KEY,
    subject       TEXT NOT NULL,
    workflow_state TEXT NOT NULL DEFAULT 'active',  -- active | deleted
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS conversation_messages (
    id              BIGSERIAL PRIMARY KEY,
    conversation_id BIGINT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    author_id       BIGINT NOT NULL,
    body            TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS conversation_participants (
    id              BIGSERIAL PRIMARY KEY,
    conversation_id BIGINT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL,
    workflow_state   TEXT NOT NULL DEFAULT 'active',  -- active | deleted
    last_message_at TIMESTAMPTZ,
    UNIQUE (conversation_id, user_id)
);

-- Indexes for common queries
CREATE INDEX idx_conversation_messages_conv ON conversation_messages(conversation_id);
CREATE INDEX idx_conversation_participants_user ON conversation_participants(user_id);
CREATE INDEX idx_conversation_participants_conv_user ON conversation_participants(conversation_id, user_id);
