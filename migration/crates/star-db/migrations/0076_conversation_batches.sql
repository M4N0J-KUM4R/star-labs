-- Migration 0076: Canvas conversation batches and per-user message removal.
--
-- Canvas creates a ConversationBatch when a bulk private message is sent
-- asynchronously (POST /conversations with mode=async). GET
-- /api/v1/conversations/batches then lists the caller's in-progress batches
-- (workflow_state created | sending) until the send job finishes.
--
-- Canvas also lets a participant remove individual messages from their own
-- view (POST /api/v1/conversations/{id}/remove_messages) through
-- ConversationMessageParticipant rows; this simplified schema keeps one
-- removal marker per (message, user) instead.
--
-- API scope for this migration:
-- - GET  /api/v1/conversations/batches
-- - POST /api/v1/conversations/{id}/remove_messages
-- - GET  /api/v1/conversations/unread_count

CREATE TABLE IF NOT EXISTS conversation_batches (
    id                           BIGSERIAL PRIMARY KEY,
    user_id                      BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    subject                      TEXT,
    -- created | sending | sent | error (Canvas Workflow states)
    workflow_state               TEXT NOT NULL DEFAULT 'created',
    root_conversation_message_id BIGINT REFERENCES conversation_messages(id) ON DELETE SET NULL,
    -- Canvas stores these two as comma-separated strings; arrays are cleaner.
    recipient_ids                BIGINT[] NOT NULL DEFAULT '{}',
    conversation_message_ids     BIGINT[] NOT NULL DEFAULT '{}',
    tags                         TEXT[] NOT NULL DEFAULT '{}',
    created_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Batches are always listed per sender, filtered to in-progress states and
-- ordered by id (oldest first), matching Api.paginate(...order(:id)).
CREATE INDEX IF NOT EXISTS idx_conversation_batches_user_state
    ON conversation_batches(user_id, workflow_state, id);

-- Per-user soft removal of messages: the message stays in the conversation
-- for everyone else. Rows are keyed by (message, user) so repeated removals
-- are idempotent.
CREATE TABLE IF NOT EXISTS conversation_message_removals (
    id              BIGSERIAL PRIMARY KEY,
    conversation_id BIGINT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    message_id      BIGINT NOT NULL REFERENCES conversation_messages(id) ON DELETE CASCADE,
    user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    deleted_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (message_id, user_id)
);

-- GET /conversations/{id} and the inbox preview exclude removed messages for
-- the current user; this index backs those NOT EXISTS filters.
CREATE INDEX IF NOT EXISTS idx_conversation_message_removals_user_conv
    ON conversation_message_removals(user_id, conversation_id);

-- GET /conversations/unread_count counts active conversations whose
-- participant row has not been read yet (read_at IS NULL).
CREATE INDEX IF NOT EXISTS idx_conversation_participants_user_state_read
    ON conversation_participants(user_id, workflow_state, read_at);
