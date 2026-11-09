-- Migration 0030: scope the courses.account_id existence check to the current schema.
-- Migration 0024's information_schema lookup was not schema-qualified, so when
-- the same database already had courses.account_id in another schema (e.g. a
-- Supabase database hosting both star_lms and a test schema), the column was
-- skipped for the schema being migrated.

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = current_schema()
          AND table_name = 'courses'
          AND column_name = 'account_id'
    ) THEN
        ALTER TABLE courses ADD COLUMN account_id BIGINT REFERENCES accounts(id) DEFAULT 1;
    END IF;
END $$;
