-- 0243: accounts.allow_sis_import — the column sis_imports_api reads
-- (its module doc expected this in a migration; it never landed, so the
-- errors index 500s on "column allow_sis_import does not exist").
-- Canvas enables SIS imports on every root account (Account#
-- enable_sis_imports, app/models/account.rb), so the backfill flips
-- every existing root account on.
ALTER TABLE accounts
    ADD COLUMN IF NOT EXISTS allow_sis_import BOOLEAN NOT NULL DEFAULT false;

UPDATE accounts SET allow_sis_import = true WHERE parent_account_id IS NULL;
