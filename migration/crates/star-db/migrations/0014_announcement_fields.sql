-- Migration 0014: Canvas announcement parity (announcement.rb subclasses
-- DiscussionTopic, which carries workflow_state; delayed_post_at/pasted... ).
-- - workflow_state: 'active' | 'deleted' (soft delete, Canvas-style)
-- - posted_at: when the announcement was posted (created_at proxy here)
ALTER TABLE announcements
    ADD COLUMN IF NOT EXISTS workflow_state TEXT NOT NULL DEFAULT 'active',
    ADD COLUMN IF NOT EXISTS posted_at TIMESTAMPTZ;
