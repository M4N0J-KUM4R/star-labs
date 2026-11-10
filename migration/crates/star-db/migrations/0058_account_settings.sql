-- Migration 0058: account settings — Canvas `accounts.settings` hash.
--
-- Canvas stores per-account configuration in a serialized `settings` hash on
-- the account row (microsoft_sync_*, enable_as_k5_account, course-creation
-- toggles, ...). Mirror it with a JSONB column so account settings can be read
-- and shallow-merged without further schema churn.
--
-- Backs the Phase 3 routes:
--   GET/PUT /api/v1/accounts/:account_id/settings
--   GET     /api/v1/accounts/:account_id/sub_accounts
--   GET     /api/v1/manageable_accounts
--   GET     /api/v1/course_creation_accounts

ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS settings JSONB NOT NULL DEFAULT '{}'::jsonb;
