-- Migration 0248: courses + groups — the Canvas columns the recorded
-- responses still owe the port.
--
-- Sources of truth:
--   canvas-src/db/migrate/20101210192618_init_canvas_db.rb:176  (create_table :courses)
--   canvas-src/db/migrate/20101210192618_init_canvas_db.rb:2753 (create_table :groups)
--   canvas-src/app/models/course.rb                            (Course.clonable_attributes,
--                                                                add_setting defaults)
--   canvas-src/lib/api/v1/course_json.rb:21                    (CourseJson::BASE_ATTRIBUTES)
--   canvas-src/lib/api/v1/course_json.rb:66                    (methods_to_send)
--   canvas-src/app/models/group.rb                             (Group#members_count,
--                                                                Group#storage_quota_mb,
--                                                                Group#submission?)
--   canvas-src/lib/api/v1/group.rb:41                          (group_json)
--
-- Both tables already exist (0001 for courses, 0018 for groups) and grew
-- through `ALTER TABLE`. The `CREATE TABLE IF NOT EXISTS` below only
-- documents the canonical column set — `schema_debt.py` reads `CREATE TABLE`
-- statements, so a table that grew through `ALTER TABLE` looks empty to it.
-- On an existing database the statement is a no-op; the `ALTER TABLE`s are
-- what add the new columns.
--
-- New columns — groups (Canvas column -> JSON key):
--
--   is_public            BOOLEAN  Canvas `groups.is_public`
--                                 -> `is_public`
--   max_membership       BIGINT   Canvas `groups.max_membership`
--                                 -> `max_membership`
--   members_count        BIGINT   Canvas `Group#members_count`
--                                 -> `members_count`
--   storage_quota_mb     BIGINT   Canvas `groups.storage_quota` /
--                                 `Group#storage_quota_mb` -> `storage_quota_mb`
--   context_type         TEXT     Canvas `groups.context_type`
--                                 -> `context_type`
--   context_name         TEXT     Canvas `group.context.name`
--                                 -> `context_name`
--   avatar_url           TEXT     Canvas `group.avatar_attachment` thumbnail
--                                 -> `avatar_url`
--   role                 TEXT     Canvas `group.group_category.role`
--                                 -> `role`
--   leader               BIGINT   Canvas `group.leader` (`groups.leader_id`)
--                                 -> `leader`
--   has_submission       BOOLEAN  Canvas `Group#submission?`
--                                 -> `has_submission`
--   concluded            BOOLEAN  Canvas `group.context.concluded?`
--                                 -> `concluded`
--   non_collaborative    BOOLEAN  Canvas `groups.non_collaborative`
--                                 -> `non_collaborative`
--
-- Canvas derives `members_count`, `context_name`, `avatar_url`, `role`,
-- `leader`, `has_submission` and `concluded` from other tables
-- (`group_memberships`, the context, the avatar attachment, the group
-- category). The port mirrors them on the group row so the group JSON can
-- be read in one query, the same way migration 0240 mirrors the terms of
-- service on the account row.
--
-- New columns — courses (Canvas column -> JSON key):
--
--   uuid                 TEXT     Canvas `courses.uuid`         -> `uuid`
--   default_view         TEXT     Canvas `courses.default_view` -> `default_view`
--   is_public            BOOLEAN  Canvas `courses.is_public`    -> `is_public`
--   grading_standard_id  BIGINT   Canvas `courses.grading_standard_id`
--                                 -> `grading_standard_id`
--   license              TEXT     Canvas `courses.license`     -> `license`
--   grade_passback_setting TEXT   Canvas `courses.grade_passback_setting`
--                                 -> `grade_passback_setting`
--   locale               TEXT     Canvas `courses.locale`      -> `locale`
--   template             BOOLEAN  Canvas `courses.template`    -> `template`
--   homeroom_course      BOOLEAN  Canvas `courses.homeroom_course`
--                                 -> `homeroom_course`
--   is_public_to_auth_users BOOLEAN
--                           Canvas `Course#is_public_to_auth_users`
--                                 -> `is_public_to_auth_users`
--   public_syllabus      BOOLEAN  Canvas `Course#public_syllabus`
--                                 -> `public_syllabus`
--   public_syllabus_to_auth BOOLEAN
--                           Canvas `Course#public_syllabus_to_auth`
--                                 -> `public_syllabus_to_auth`
--
-- Canvas keeps the rest of the course settings in the `courses.settings`
-- JSON document (`app/models/course.rb:84`, `serialize :settings`), which
-- this port already mirrors in `course_settings.settings`; the columns above
-- are the ones the course row itself renders
-- (`lib/api/v1/course_json.rb:21` BASE_ATTRIBUTES and `:66` methods_to_send).

CREATE TABLE IF NOT EXISTS courses (
    id                      BIGSERIAL PRIMARY KEY,
    name                    TEXT NOT NULL,
    course_code             TEXT NOT NULL,
    description             TEXT,
    workflow_state          TEXT NOT NULL DEFAULT 'available',
    start_at                TIMESTAMPTZ,
    end_at                  TIMESTAMPTZ,
    syllabus_body           TEXT,
    account_id              BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
    storage_quota           BIGINT,
    allow_final_grade_override BOOLEAN NOT NULL DEFAULT FALSE,
    csp_disabled            BOOLEAN NOT NULL DEFAULT false,
    enrollment_term_id      BIGINT,
    horizon_course          BOOLEAN NOT NULL DEFAULT false,
    settings                JSONB NOT NULL DEFAULT '{}'::jsonb,
    uuid                    TEXT,
    default_view            TEXT,
    is_public               BOOLEAN NOT NULL DEFAULT false,
    grading_standard_id     BIGINT,
    license                 TEXT,
    grade_passback_setting  TEXT,
    locale                  TEXT,
    template                BOOLEAN NOT NULL DEFAULT false,
    homeroom_course         BOOLEAN NOT NULL DEFAULT false,
    is_public_to_auth_users BOOLEAN NOT NULL DEFAULT false,
    public_syllabus         BOOLEAN NOT NULL DEFAULT false,
    public_syllabus_to_auth BOOLEAN NOT NULL DEFAULT false,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Canvas `courses.uuid` (init_canvas_db.rb:181).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS uuid TEXT;

-- Canvas `courses.default_view` (init_canvas_db.rb:195).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS default_view TEXT;

-- Canvas `courses.is_public` (init_canvas_db.rb:185).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS is_public BOOLEAN NOT NULL DEFAULT false;

-- Canvas `courses.grading_standard_id` (init_canvas_db.rb:184).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS grading_standard_id BIGINT;

-- Canvas `courses.license` (init_canvas_db.rb:207).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS license TEXT;

-- Canvas `courses.grade_passback_setting` (init_canvas_db.rb:227).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS grade_passback_setting TEXT;

-- Canvas `courses.locale` (init_canvas_db.rb:211).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS locale TEXT;

-- Canvas `courses.template` (init_canvas_db.rb:228).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS template BOOLEAN NOT NULL DEFAULT false;

-- Canvas `courses.homeroom_course` (init_canvas_db.rb:229).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS homeroom_course BOOLEAN NOT NULL DEFAULT false;

-- Canvas `Course#is_public_to_auth_users` — the course is visible to
-- authenticated users of the account (course.rb:4138).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS is_public_to_auth_users BOOLEAN NOT NULL DEFAULT false;

-- Canvas `Course#public_syllabus` — the syllabus is visible to everyone.
-- Canvas defaults it to `is_public` (course.rb:4793).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS public_syllabus BOOLEAN NOT NULL DEFAULT false;

-- Canvas `Course#public_syllabus_to_auth` — the syllabus is visible to
-- authenticated users. Canvas defaults it to `is_public_to_auth_users`
-- (course.rb:4794).
ALTER TABLE courses
    ADD COLUMN IF NOT EXISTS public_syllabus_to_auth BOOLEAN NOT NULL DEFAULT false;

CREATE TABLE IF NOT EXISTS groups (
    id               BIGSERIAL PRIMARY KEY,
    course_id        BIGINT REFERENCES courses(id) ON DELETE SET NULL,
    name             TEXT NOT NULL,
    description      TEXT,
    workflow_state   TEXT NOT NULL DEFAULT 'available',
    join_level       TEXT NOT NULL DEFAULT 'invitation_only',
    group_category_id BIGINT,
    account_id       BIGINT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_public            BOOLEAN NOT NULL DEFAULT false,
    max_membership       BIGINT,
    members_count        BIGINT NOT NULL DEFAULT 0,
    storage_quota_mb     BIGINT,
    context_type         TEXT,
    context_name         TEXT,
    avatar_url           TEXT,
    role                 TEXT,
    leader               BIGINT,
    has_submission       BOOLEAN NOT NULL DEFAULT false,
    concluded            BOOLEAN NOT NULL DEFAULT false,
    non_collaborative    BOOLEAN NOT NULL DEFAULT false
);

-- Canvas `groups.is_public` (init_canvas_db.rb:2760).
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS is_public BOOLEAN NOT NULL DEFAULT false;

-- Canvas `groups.max_membership` (init_canvas_db.rb:2759).
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS max_membership BIGINT;

-- Canvas `Group#members_count` — the number of participating memberships
-- (group.rb:935). The port keeps it on the group row and refreshes it
-- whenever a membership changes.
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS members_count BIGINT NOT NULL DEFAULT 0;

-- Canvas `Group#storage_quota_mb` — the group quota in megabytes
-- (group.rb:947); Canvas stores the byte count in `groups.storage_quota`
-- (init_canvas_db.rb:2767).
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS storage_quota_mb BIGINT;

-- Canvas `groups.context_type` (init_canvas_db.rb:2758) — "Course" or
-- "Account".
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS context_type TEXT;

-- Canvas `group.context.name` (lib/api/v1/group.rb:48).
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS context_name TEXT;

-- Canvas `group.avatar_attachment` — the group avatar, rendered as a
-- thumbnail URL (lib/api/v1/group.rb:49-50).
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS avatar_url TEXT;

-- Canvas `group.group_category.role` (lib/api/v1/group.rb:51).
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS role TEXT;

-- Canvas `group.leader` (lib/api/v1/group.rb:53); Canvas stores the id in
-- `groups.leader_id` (init_canvas_db.rb:2776).
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS leader BIGINT;

-- Canvas `Group#submission?` (group.rb:279) — the group has a submission
-- for an assignment in its group category.
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS has_submission BOOLEAN NOT NULL DEFAULT false;

-- Canvas `group.context.concluded?` (lib/api/v1/group.rb:101) — the
-- context of the group is concluded or deleted.
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS concluded BOOLEAN NOT NULL DEFAULT false;

-- Canvas `groups.non_collaborative` (init_canvas_db.rb:2778).
ALTER TABLE groups
    ADD COLUMN IF NOT EXISTS non_collaborative BOOLEAN NOT NULL DEFAULT false;

-- The `users` keys the recorded course roster bodies return that the port's
-- users table does not carry yet. Canvas keeps all three on the pseudonym
-- (`lib/api/v1/user.rb:75`, :76 and :84):
--
--   sis_user_id     TEXT   Canvas `pseudonyms.sis_user_id`     -> `sis_user_id`
--   integration_id  TEXT   Canvas `pseudonyms.integration_id` -> `integration_id`
--   login_id        TEXT   Canvas `pseudonyms.unique_id`      -> `login_id`
--
-- The columns mirror the user's pseudonym on the user row, the same way
-- migration 0245 mirrors `User#short_name` there.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS sis_user_id TEXT,
    ADD COLUMN IF NOT EXISTS integration_id TEXT,
    ADD COLUMN IF NOT EXISTS login_id TEXT;

-- Fill the mirrored values from the pseudonym the user owns.
UPDATE users u
SET sis_user_id = p.sis_user_id,
    integration_id = p.integration_id,
    login_id = p.unique_id
FROM (
    SELECT DISTINCT ON (user_id) user_id, sis_user_id, integration_id, unique_id
    FROM pseudonyms
    WHERE workflow_state = 'active'
    ORDER BY user_id, id
) p
WHERE p.user_id = u.id
  AND (u.sis_user_id IS NULL
       OR u.integration_id IS NULL
       OR u.login_id IS NULL);
