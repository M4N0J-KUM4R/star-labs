-- Migration 0046: conversation participant/read-state support for star-lms.
-- Canvas: ConversationParticipant tracks per-user read state for a conversation
-- (workflow_state read/unread plus message counters). This simplified schema
-- keeps the existing active/deleted workflow_state and adds a read timestamp.
--
-- Added:
--   read_at — when this participant last read the conversation (NULL = unread)
ALTER TABLE conversation_participants
    ADD COLUMN IF NOT EXISTS read_at TIMESTAMPTZ;

-- Lookup index for participant/read-state queries on a conversation.
CREATE INDEX IF NOT EXISTS idx_conversation_participants_conv_read
    ON conversation_participants(conversation_id, read_at);
