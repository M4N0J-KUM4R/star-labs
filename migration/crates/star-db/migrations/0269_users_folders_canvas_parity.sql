-- Migration 0264: Canvas parity for the `users` and `folders` tables.
--
-- The recorded Canvas corpus is the contract: every key of a recorded body is
-- a field Canvas returns for that table
-- (`migration/tools/parity/schema_debt.py`). This migration closes the gap
-- between the recorded `users`/`folders` bodies and the port's tables.
--
-- Canvas sources
--   users   lib/api/v1/user.rb:26  (API_USER_JSON_OPTS: id, name, created_at,
--                                  sortable_name, short_name)
--           lib/api/v1/user.rb:53  (user_json)
--           lib/api/v1/user.rb:105 (first_name / last_name includes)
--           lib/api/v1/user.rb:138 (locale / effective_locale)
--           lib/api/v1/user.rb:159 (permissions map)
--           lib/api/v1/user.rb:176 (time_zone)
--           app/controllers/users_controller.rb:3227 (api_show_includes)
--           app/controllers/users_controller.rb:2340 (update includes)
--           app/models/user.rb:1014 / :1018 (first_name / last_name)
--           app/models/user.rb:1213 (short_name falls back to the name)
--           app/models/user.rb:3900 (locale)
--   folders lib/api/v1/folders.rb:30 (folder_json), :43-51 (the payload keys)
--
-- The CREATE TABLE blocks re-declare the Canvas-shaped tables (the 0262
-- precedent): IF NOT EXISTS makes them a no-op where the table already exists
-- (0001 owns `users`, 0036 owns `folders`), so they only state the whole shape
-- in one place; the ALTER TABLE statements below are what change an existing
-- database, and they are idempotent.

CREATE TABLE IF NOT EXISTS users (
    id                        BIGSERIAL PRIMARY KEY,
    name                      TEXT NOT NULL,
    email                     TEXT NOT NULL UNIQUE,
    password_hash             TEXT NOT NULL,
    role                      TEXT NOT NULL DEFAULT 'student',
    workflow_state            TEXT NOT NULL DEFAULT 'registered',
    sortable_name             TEXT,
    -- Canvas `User#short_name` is `super || name` (app/models/user.rb:1213).
    short_name                TEXT,
    -- Canvas derives the two name parts from sortable_name
    -- (app/models/user.rb:1014 and :1018 call User.name_parts).
    first_name                TEXT,
    last_name                 TEXT,
    -- Canvas keeps the locale on the row (app/models/user.rb:3900).
    locale                    TEXT,
    -- Canvas answers `effective_locale` with the request locale
    -- (lib/api/v1/user.rb:135); the port keeps the account default on the row.
    effective_locale          TEXT DEFAULT 'en',
    -- Canvas keeps the time zone name on the row (lib/api/v1/user.rb:176).
    time_zone                 TEXT,
    -- The permissions map Canvas computes per request
    -- (lib/api/v1/user.rb:159). The column carries the stored map when one
    -- was written; the handler falls back to the computed default.
    permissions               JSONB,
    -- The payload keys the user endpoints answer with. The values live in
    -- `user_colors`, `dashboard_positions` and `user_preferences`; the row
    -- carries the Canvas key set of the users family.
    custom_colors             JSONB,
    dashboard_positions       JSONB,
    new_user_tutorial_statuses JSONB,
    -- The single `message` key Canvas renders for the flash responses of the
    -- user endpoints (users_controller.rb:1900, :2086 and :2835).
    message                   TEXT,
    last_logged_out           TIMESTAMPTZ,
    last_seen_release_note    TIMESTAMPTZ,
    created_at                TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS folders (
    id               BIGSERIAL PRIMARY KEY,
    name             TEXT NOT NULL,
    full_name        TEXT NOT NULL DEFAULT '',
    context_type     TEXT NOT NULL DEFAULT 'Course',
    context_id       BIGINT NOT NULL,
    parent_folder_id BIGINT REFERENCES folders(id) ON DELETE CASCADE,
    workflow_state   TEXT NOT NULL DEFAULT 'visible',
    position         INTEGER NOT NULL DEFAULT 1,
    locked           BOOLEAN NOT NULL DEFAULT FALSE,
    lock_at          TIMESTAMPTZ,
    unlock_at        TIMESTAMPTZ,
    for_submissions  BOOLEAN NOT NULL DEFAULT FALSE,
    deleted_at       TIMESTAMPTZ,
    -- The remaining keys of the Canvas folder payload (lib/api/v1/folders.rb:30).
    -- Canvas computes them per request; the columns carry the stored value when
    -- one was written, the handlers fall back to the computed one.
    hidden           BOOLEAN,
    hidden_for_user  BOOLEAN,
    locked_for_user  BOOLEAN,
    can_upload       BOOLEAN,
    files_count      BIGINT,
    folders_count    BIGINT,
    files_url        TEXT,
    folders_url      TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- The columns the recorded `users` body returns and the port's users table
-- did not have.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS short_name TEXT,
    ADD COLUMN IF NOT EXISTS first_name TEXT,
    ADD COLUMN IF NOT EXISTS last_name TEXT,
    ADD COLUMN IF NOT EXISTS locale TEXT,
    ADD COLUMN IF NOT EXISTS effective_locale TEXT DEFAULT 'en',
    ADD COLUMN IF NOT EXISTS time_zone TEXT,
    ADD COLUMN IF NOT EXISTS permissions JSONB,
    ADD COLUMN IF NOT EXISTS custom_colors JSONB,
    ADD COLUMN IF NOT EXISTS dashboard_positions JSONB,
    ADD COLUMN IF NOT EXISTS new_user_tutorial_statuses JSONB,
    ADD COLUMN IF NOT EXISTS message TEXT;

-- The columns the recorded `folders` body returns and the port's folders table
-- did not have.
ALTER TABLE folders
    ADD COLUMN IF NOT EXISTS hidden BOOLEAN,
    ADD COLUMN IF NOT EXISTS hidden_for_user BOOLEAN,
    ADD COLUMN IF NOT EXISTS locked_for_user BOOLEAN,
    ADD COLUMN IF NOT EXISTS can_upload BOOLEAN,
    ADD COLUMN IF NOT EXISTS files_count BIGINT,
    ADD COLUMN IF NOT EXISTS folders_count BIGINT,
    ADD COLUMN IF NOT EXISTS files_url TEXT,
    ADD COLUMN IF NOT EXISTS folders_url TEXT;

-- Canvas keeps the short name on the row and falls back to the name
-- (app/models/user.rb:1213).
UPDATE users
SET short_name = name
WHERE short_name IS NULL;

-- Canvas derives the two name parts from the sortable name
-- (app/models/user.rb:1014 and :1018): "Surname, Given" splits on the comma,
-- "Given Surname" puts the last word in the surname.
UPDATE users u
SET first_name = CASE
        WHEN n.s LIKE '%,%' THEN btrim(split_part(n.s, ',', 2))
        WHEN n.s LIKE '% %'  THEN btrim(regexp_replace(n.s, '\s+\S+$', ''))
        ELSE n.s END,
    last_name = CASE
        WHEN n.s LIKE '%,%' THEN btrim(split_part(n.s, ',', 1))
        WHEN n.s LIKE '% %'  THEN btrim(regexp_replace(n.s, '^.*\s+', ''))
        ELSE NULL END
FROM (
    SELECT id, COALESCE(NULLIF(btrim(sortable_name), ''), btrim(name)) AS s
    FROM users
) n
WHERE u.id = n.id
  AND (u.first_name IS NULL OR u.last_name IS NULL);

-- The folder urls are stable per row (lib/api/v1/folders.rb:43).
UPDATE folders
SET files_url = '/api/v1/folders/' || id::text || '/files',
    folders_url = '/api/v1/folders/' || id::text || '/folders'
WHERE files_url IS NULL OR folders_url IS NULL;
