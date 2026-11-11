-- Migration: 0093_communication_channels_deep
-- Canvas API v1 parity: bounce detail fields surfaced by
-- `communication_channel_json` for users with :read_bounce_details, plus the
-- index backing `accounts/:account_id/bounced_communication_channels`
-- (`CommunicationChannel::BulkActions::ResetBounceCounts`).
--
-- `communication_channels` itself was created in 0045; this only adds the
-- Canvas bounce columns that were missing there.

ALTER TABLE communication_channels
    ADD COLUMN IF NOT EXISTS last_bounce_summary TEXT,
    ADD COLUMN IF NOT EXISTS last_transient_bounce_at TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS last_transient_bounce_summary TEXT,
    ADD COLUMN IF NOT EXISTS last_suppression_bounce_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_communication_channels_bounced
    ON communication_channels (user_id, last_bounce_at)
    WHERE bounce_count > 0;
