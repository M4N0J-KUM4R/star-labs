-- Migration 0011: context_settings table for top_nav_favorites storage.
-- Canvas stores top_nav_favorite_tool_ids in @context.settings (JSON column).
-- This workspace stores them in a dedicated table so they are queryable.

CREATE TABLE IF NOT EXISTS context_settings (
  context_type TEXT NOT NULL,   -- 'Account' or 'Course'
  context_id   BIGINT NOT NULL,
  settings     JSONB DEFAULT '{}',
  PRIMARY KEY (context_type, context_id)
);

CREATE INDEX IF NOT EXISTS idx_context_settings_account
  ON context_settings(context_type, context_id)
  WHERE context_type = 'Account';

CREATE INDEX IF NOT EXISTS idx_context_settings_course
  ON context_settings(context_type, context_id)
  WHERE context_type = 'Course';
