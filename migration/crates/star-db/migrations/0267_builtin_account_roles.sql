-- Migration 0243: the built-in account roles.
--
-- Canvas keeps its two built-in account roles (`AccountAdmin` and
-- `AccountMembership`) in the same `roles` table as the five built-in
-- course roles, with `workflow_state = 'built_in'`
-- (`app/models/role.rb:127-145`, `Role.built_in_roles`).
--
-- The permission layer needs them: a Canvas permission is granted to a
-- *role*, and `role_overrides.role_id` names that role. Without the rows an
-- account role could never hold an override.
--
-- The ids continue the range the five built-in course roles already use
-- (`Role::builtin_role_id`): `AccountAdmin` is 6 and `AccountMembership`
-- is 7. Canvas generates the ids per root account instead; the port keeps
-- them fixed so a role can be named without a lookup.
--
-- Canvas hides `AccountMembership` from an account's role list
-- (`Role#visible?`, `app/models/role.rb:166-168`), so `roles_list` keeps
-- that rule.

INSERT INTO roles (id, account_id, name, label, base_role_type, workflow_state)
VALUES
    (6, NULL, 'AccountAdmin', 'Account Admin', 'AccountAdmin', 'built_in'),
    (7, NULL, 'AccountMembership', 'Account Membership', 'AccountMembership', 'built_in')
ON CONFLICT DO NOTHING;

-- Keep the BIGSERIAL sequence past the seeded built-in ids.
SELECT setval(
    pg_get_serial_sequence('roles', 'id'),
    GREATEST((SELECT COALESCE(MAX(id), 1) FROM roles), 7)
);
