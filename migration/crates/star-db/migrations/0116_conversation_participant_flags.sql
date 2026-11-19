-- Migration 0116: participant inbox flags for the conversations tail (#60).
-- Canvas ConversationParticipant carries `starred` and `subscribed` next to
-- the read/unread and active/archived/deleted workflow states the snapshot
-- already models (0020 + 0046). Additive only.
ALTER TABLE conversation_participants
    ADD COLUMN IF NOT EXISTS starred BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS subscribed BOOLEAN NOT NULL DEFAULT true;
