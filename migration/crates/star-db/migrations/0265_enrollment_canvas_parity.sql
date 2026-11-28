-- Migration 0262: Canvas enrollment parity — every field the enrollments API
-- returns.
--
-- Canvas table: `enrollments` (db/migrate/20101210192618_init_canvas_db.rb,
-- "create_table :enrollments"). Canvas model: app/models/enrollment.rb.
-- Canvas API shape: `Api::V1::User#enrollment_json` (lib/api/v1/user.rb:292),
-- which app/controllers/enrollments_api_controller.rb renders from its #index
-- and #show actions.
--
-- 0001_core_schema.sql created `enrollments` with six columns (id, user_id,
-- course_id, role, created_at, updated_at) and later migrations added the
-- workflow_state, course_section, lifecycle and temporary-enrollment columns.
-- Canvas carries more: the enrollment type, the role id, the root account,
-- the associated user of an observer enrollment and the SIS/integration ids.
-- Those are the columns this migration adds.
--
-- The CREATE TABLE block re-declares the Canvas-shaped table. IF NOT EXISTS
-- makes it a no-op on the databases that already have the table (0001 owns
-- the creation), so it only states the whole shape in one place; the ALTER
-- TABLE statements below are what actually change an existing database, and
-- they are idempotent.

CREATE TABLE IF NOT EXISTS enrollments (
    id                                BIGSERIAL PRIMARY KEY,
    user_id                           BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    course_id                         BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE,
    -- Canvas stores the enrollment type on the row (StudentViewEnrollment for
    -- a student, TeacherEnrollment for a teacher, TaEnrollment for a TA,
    -- ObserverEnrollment for an observer, DesignerEnrollment for a designer).
    type                              TEXT,
    role                              TEXT NOT NULL DEFAULT 'student',
    role_id                           BIGINT REFERENCES roles(id),
    workflow_state                    TEXT NOT NULL DEFAULT 'active',
    associated_user_id                BIGINT REFERENCES users(id) ON DELETE SET NULL,
    course_section_id                 BIGINT REFERENCES course_sections(id) ON DELETE SET NULL,
    root_account_id                   BIGINT REFERENCES accounts(id),
    limit_privileges_to_course_section BOOLEAN NOT NULL DEFAULT FALSE,
    start_at                          TIMESTAMPTZ,
    end_at                            TIMESTAMPTZ,
    last_activity_at                  TIMESTAMPTZ,
    last_attended_at                  TIMESTAMPTZ,
    total_activity_time               INTEGER NOT NULL DEFAULT 0,
    sis_account_id                    TEXT,
    sis_course_id                     TEXT,
    course_integration_id             TEXT,
    sis_section_id                    TEXT,
    section_integration_id            TEXT,
    sis_user_id                       TEXT,
    temporary_enrollment_source_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    -- Canvas renames workflow_state to enrollment_state in the API payload
    -- (`json[:enrollment_state] = json.delete("workflow_state")`), so the
    -- state is kept in one place and mirrored here.
    enrollment_state                  TEXT GENERATED ALWAYS AS (workflow_state) STORED,
    -- The two nested payloads of the enrollment JSON. Canvas computes them
    -- per request (the grades hash from the enrollment's score rollups, the
    -- user from the joined user row), so these columns carry no value; they
    -- are declared because the enrollments API returns them as part of the
    -- enrollment payload.
    grades                            JSONB,
    "user"                            JSONB,
    -- Canvas builds the enrollment URL from the course and the user
    -- (`course_user_url(enrollment.course_id, enrollment.user_id)`).
    html_url                          TEXT GENERATED ALWAYS AS ('/courses/' || course_id::text || '/users/' || user_id::text) STORED,
    created_at                        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, course_id, role)
);

-- The columns that 0001 and the later migrations did not have.
ALTER TABLE enrollments
    ADD COLUMN IF NOT EXISTS type TEXT,
    ADD COLUMN IF NOT EXISTS role_id BIGINT REFERENCES roles(id),
    ADD COLUMN IF NOT EXISTS associated_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS root_account_id BIGINT REFERENCES accounts(id),
    ADD COLUMN IF NOT EXISTS sis_account_id TEXT,
    ADD COLUMN IF NOT EXISTS sis_course_id TEXT,
    ADD COLUMN IF NOT EXISTS course_integration_id TEXT,
    ADD COLUMN IF NOT EXISTS sis_section_id TEXT,
    ADD COLUMN IF NOT EXISTS section_integration_id TEXT,
    ADD COLUMN IF NOT EXISTS sis_user_id TEXT,
    ADD COLUMN IF NOT EXISTS enrollment_state TEXT GENERATED ALWAYS AS (workflow_state) STORED,
    ADD COLUMN IF NOT EXISTS grades JSONB,
    ADD COLUMN IF NOT EXISTS "user" JSONB,
    ADD COLUMN IF NOT EXISTS html_url TEXT GENERATED ALWAYS AS ('/courses/' || course_id::text || '/users/' || user_id::text) STORED;

-- The type of the rows that predate this migration: Canvas keeps the
-- enrollment type on the row, the port derives it from the role.
UPDATE enrollments
SET type = CASE role
        WHEN 'student' THEN 'StudentViewEnrollment'
        WHEN 'teacher' THEN 'TeacherEnrollment'
        WHEN 'ta' THEN 'TaEnrollment'
        WHEN 'observer' THEN 'ObserverEnrollment'
        WHEN 'designer' THEN 'DesignerEnrollment'
        ELSE type
    END
WHERE type IS NULL;

-- The role id: Canvas points at the role row of the account the enrollment
-- belongs to. The built-in roles (id 1..5, seeded by 0099_roles_list.sql)
-- carry the Canvas role names.
UPDATE enrollments e
SET role_id = r.id
FROM roles r
WHERE e.role_id IS NULL
  AND r.account_id IS NULL
  AND r.name = CASE e.role
        WHEN 'student' THEN 'StudentEnrollment'
        WHEN 'teacher' THEN 'TeacherEnrollment'
        WHEN 'ta' THEN 'TaEnrollment'
        WHEN 'observer' THEN 'ObserverEnrollment'
        WHEN 'designer' THEN 'DesignerEnrollment'
    END;

-- The root account: Canvas stores the account the enrollment belongs to.
UPDATE enrollments e
SET root_account_id = c.account_id
FROM courses c
WHERE e.root_account_id IS NULL
  AND c.id = e.course_id;

-- The role id is looked up by name on every read.
CREATE INDEX IF NOT EXISTS idx_enrollments_role_id
    ON enrollments(role_id)
    WHERE role_id IS NOT NULL;
