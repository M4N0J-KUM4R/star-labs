-- Migration 0028: sync the accounts id sequence after the explicit root seed.
-- Migration 0024 inserts accounts.id = 1 directly, which leaves the BIGSERIAL
-- sequence at 1. On a fresh database the first sequence-allocated insert then
-- collides with the root account (accounts_pkey). Advance the sequence past
-- the largest existing id so inserts continue safely.

SELECT setval(
    pg_get_serial_sequence('accounts', 'id')::regclass,
    GREATEST((SELECT COALESCE(MAX(id), 1) FROM accounts), 1)
);
