-- Migration 0099: Canvas Role API (`accounts/roles_controller`).
-- Canvas keeps both the five built-in enrollment roles and account-defined
-- custom roles in `roles`; `role_overrides.role_id` points at a row here.
-- Built-in roles are seeded with the fixed ids already used by
-- `Role::builtin_role_id` (1..5) and carry a NULL `account_id`.

CREATE TABLE IF NOT EXISTS roles (
    id              BIGSERIAL PRIMARY KEY,
    account_id      BIGINT REFERENCES accounts(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    label           TEXT NOT NULL,
    base_role_type  TEXT NOT NULL,
    workflow_state  TEXT NOT NULL DEFAULT 'active',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Canvas role names are unique per account; built-in roles share NULL account.
CREATE UNIQUE INDEX IF NOT EXISTS idx_roles_account_name
    ON roles (account_id, name);

CREATE INDEX IF NOT EXISTS idx_roles_account
    ON roles (account_id, workflow_state);

INSERT INTO roles (id, account_id, name, label, base_role_type, workflow_state)
VALUES
    (1, NULL, 'StudentEnrollment', 'Student', 'StudentEnrollment', 'active'),
    (2, NULL, 'TeacherEnrollment', 'Teacher', 'TeacherEnrollment', 'active'),
    (3, NULL, 'TaEnrollment', 'TA', 'TaEnrollment', 'active'),
    (4, NULL, 'DesignerEnrollment', 'Designer', 'DesignerEnrollment', 'active'),
    (5, NULL, 'ObserverEnrollment', 'Observer', 'ObserverEnrollment', 'active')
ON CONFLICT DO NOTHING;

-- Keep the BIGSERIAL sequence past the seeded built-in ids.
SELECT setval(
    pg_get_serial_sequence('roles', 'id'),
    GREATEST((SELECT COALESCE(MAX(id), 1) FROM roles), 5)
);
