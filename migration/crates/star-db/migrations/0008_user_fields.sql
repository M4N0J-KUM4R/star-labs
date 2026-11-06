-- Add sortable_name field to users table.
-- sortable_name format: "Last, First" (computed from name at registration)
ALTER TABLE users ADD COLUMN IF NOT EXISTS sortable_name TEXT;

-- Backfill existing users: parse "First ... Last" -> "Last, First"
-- SPLIT_PART(name, ' ', -1) = last word = last name
-- LEFT(name, LENGTH(name) - LENGTH(SPLIT_PART(name, ' ', -1)) - 1) = everything before last word = first name(s)
UPDATE users
SET sortable_name = TRIM(
    SPLIT_PART(name, ' ', -1) || ', ' ||
    LEFT(name, LENGTH(name) - LENGTH(SPLIT_PART(name, ' ', -1)) - 1)
)
WHERE sortable_name IS NULL;
