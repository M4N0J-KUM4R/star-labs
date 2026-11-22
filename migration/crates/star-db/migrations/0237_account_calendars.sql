-- Migration 0237: the account calendar settings.
--
-- Canvas keeps the visibility of an account calendar and the way the users
-- subscribe to it on the account itself
-- (`app/models/account/calendar.rb`):
--
--   account_calendar_visible         boolean default false
--   account_calendar_subscription_type string default "manual"
--
-- The `subscription_type` is "auto" when the users see the events of the
-- account without adding the calendar themselves, and "manual" otherwise.

ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS account_calendar_visible BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS account_calendar_subscription_type TEXT NOT NULL DEFAULT 'manual';

CREATE INDEX IF NOT EXISTS idx_accounts_calendar_visible
    ON accounts (account_calendar_visible);
