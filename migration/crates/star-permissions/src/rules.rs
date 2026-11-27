//! The pure half of Canvas' permission model.
//!
//! Canvas does not ask "is this user an admin?" at a route. It asks "does the
//! role this user holds in this context grant this permission?", and the
//! permission's own registry entry decides the answer
//! (`config/initializers/permissions_registry.rb`).
//!
//! This module holds that decision as pure functions over values, so the
//! rules can be tested without a database. [`crate::check`] is the other half:
//! it looks the values up (enrollments, account memberships, role overrides)
//! and hands them to the functions here.
//!
//! The registry rules implemented here, and where they come from:
//!
//! * `true_for` / `false_for` — the default answer for a role
//!   (Canvas `RoleOverride#uncached_permission_for`: `enabled`).
//! * `available_to` — the roles whose value may be *overridden*. A role that
//!   is not listed is locked to the default
//!   (Canvas `RoleOverride#uncached_permission_for`: `locked`).
//! * `applies_to_concluded` — the carve-out that keeps a permission alive for
//!   a concluded enrollment.
//! * `account_only` — `:root` and `:site_admin` restrict a permission to a
//!   role that belongs to the root account or to a site admin.
//! * `restrict_future_enrollments` — a future (invited) enrollment is denied.
//! * `account_allows` — Canvas gates 45 permissions on an account setting or
//!   feature flag; the port does not carry those flags, so they are denied.
//! * `not_for_masquerading` / `deprecated` — carried on the spec so callers
//!   can ask; neither changes the grant decision itself.

use crate::registry::{AccountAllows, AccountOnly, PermissionSpec};

/// A Canvas role, in the spelling the permission registry uses.
///
/// The five enrollment roles come from `enrollments.role`; the two account
/// roles come from `account_users` (Canvas `AccountUser#role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RoleKind {
    StudentEnrollment,
    TeacherEnrollment,
    TaEnrollment,
    DesignerEnrollment,
    ObserverEnrollment,
    AccountAdmin,
    AccountMembership,
}

impl RoleKind {
    /// Every role the registry can name.
    pub const ALL: [RoleKind; 7] = [
        RoleKind::StudentEnrollment,
        RoleKind::TeacherEnrollment,
        RoleKind::TaEnrollment,
        RoleKind::DesignerEnrollment,
        RoleKind::ObserverEnrollment,
        RoleKind::AccountAdmin,
        RoleKind::AccountMembership,
    ];

    /// The Canvas spelling of the role.
    pub fn as_str(self) -> &'static str {
        match self {
            RoleKind::StudentEnrollment => "StudentEnrollment",
            RoleKind::TeacherEnrollment => "TeacherEnrollment",
            RoleKind::TaEnrollment => "TaEnrollment",
            RoleKind::DesignerEnrollment => "DesignerEnrollment",
            RoleKind::ObserverEnrollment => "ObserverEnrollment",
            RoleKind::AccountAdmin => "AccountAdmin",
            RoleKind::AccountMembership => "AccountMembership",
        }
    }

    /// Parse a Canvas role name.
    pub fn from_name(raw: &str) -> Option<RoleKind> {
        RoleKind::ALL
            .iter()
            .copied()
            .find(|role| role.as_str() == raw)
    }

    /// Parse the role spelling the port stores in `enrollments.role`
    /// ("student", "teacher", …), in `account_users.role` ("admin") or in
    /// `roles.base_role_type` ("StudentEnrollment").
    pub fn from_port_name(raw: &str) -> Option<RoleKind> {
        match raw {
            "student" => Some(RoleKind::StudentEnrollment),
            "teacher" => Some(RoleKind::TeacherEnrollment),
            "ta" => Some(RoleKind::TaEnrollment),
            "designer" => Some(RoleKind::DesignerEnrollment),
            "observer" => Some(RoleKind::ObserverEnrollment),
            "admin" => Some(RoleKind::AccountAdmin),
            other => RoleKind::from_name(other),
        }
    }

    /// Whether the role is a course enrollment role.
    pub fn is_enrollment(self) -> bool {
        !self.is_account()
    }

    /// Whether the role is an account role.
    pub fn is_account(self) -> bool {
        matches!(self, RoleKind::AccountAdmin | RoleKind::AccountMembership)
    }

    /// The stable `role_overrides.role_id` of the built-in role.
    ///
    /// The port seeds the built-in roles with fixed ids (see
    /// `star-db/migrations/0099_roles_list.sql`), so `role_overrides.role_id`
    /// can name a role without a `roles` row lookup. Canvas generates the ids
    /// per root account instead.
    pub fn builtin_role_id(self) -> i64 {
        match self {
            RoleKind::StudentEnrollment => 1,
            RoleKind::TeacherEnrollment => 2,
            RoleKind::TaEnrollment => 3,
            RoleKind::DesignerEnrollment => 4,
            RoleKind::ObserverEnrollment => 5,
            RoleKind::AccountAdmin => 6,
            RoleKind::AccountMembership => 7,
        }
    }

    /// Inverse of [`RoleKind::builtin_role_id`].
    pub fn from_builtin_role_id(role_id: i64) -> Option<RoleKind> {
        RoleKind::ALL
            .iter()
            .copied()
            .find(|role| role.builtin_role_id() == role_id)
    }
}

/// The kind of context a permission is being asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextKind {
    Account,
    Course,
    Group,
}

/// The state of the enrollment that carries a role.
///
/// Canvas distinguishes these because a concluded course keeps some of its
/// permissions (`applies_to_concluded`) and loses the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentState {
    Active,
    Invited,
    Completed,
    Inactive,
}

impl EnrollmentState {
    /// Map an `enrollments.workflow_state` value.
    pub fn from_workflow_state(raw: &str) -> EnrollmentState {
        match raw {
            "active" => EnrollmentState::Active,
            "invited" | "creation_pending" => EnrollmentState::Invited,
            "completed" => EnrollmentState::Completed,
            _ => EnrollmentState::Inactive,
        }
    }
}

/// Which account the role that carries the permission belongs to.
///
/// `account_only: :root` and `account_only: :site_admin` are answered from
/// here, exactly as Canvas answers them from the *role's* account
/// (`RoleOverride#uncached_permission_for`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountScope {
    /// The account the role belongs to is the root of its account tree.
    ///
    /// Canvas asks the same question of the account the role was created in.
    /// A built-in enrollment role belongs to no account, so Canvas passes no
    /// account at all and the answer is "no".
    pub role_account_is_root: bool,
    /// The account the role belongs to is the site admin account.
    pub role_account_is_site_admin: bool,
}

impl AccountScope {
    /// A scope whose role belongs to no account: the answer Canvas gives for
    /// a built-in enrollment role.
    pub fn no_account() -> Self {
        AccountScope {
            role_account_is_root: false,
            role_account_is_site_admin: false,
        }
    }
}

/// One role the acting user holds in the context.
///
/// `override_value` is the resolved `role_overrides` entry for this role and
/// permission; `None` means no override applies, so the permission's default
/// decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grant {
    /// The role this grant carries.
    pub role: RoleKind,
    /// The `role_overrides.role_id` the override is looked up under.
    pub role_id: i64,
    /// The state of the enrollment (or membership) that carries the role.
    pub state: EnrollmentState,
    /// The resolved role override, if any.
    pub override_value: Option<bool>,
    /// The account the role belongs to.
    pub scope: AccountScope,
}

impl Grant {
    /// A grant with no role override.
    pub fn new(role: RoleKind, state: EnrollmentState) -> Self {
        Grant {
            role,
            role_id: role.builtin_role_id(),
            state,
            override_value: None,
            scope: AccountScope::no_account(),
        }
    }

    /// A grant whose role belongs to an account.
    pub fn with_scope(mut self, scope: AccountScope) -> Self {
        self.scope = scope;
        self
    }

    /// A grant with a resolved role override.
    pub fn with_override(mut self, override_value: Option<bool>) -> Self {
        self.override_value = override_value;
        self
    }
}

/// Whether the permission's value may be overridden for this role.
///
/// A role that is not in `available_to` is *locked* to the default
/// (Canvas `RoleOverride#uncached_permission_for`: `locked`).
pub fn is_overridable(spec: &PermissionSpec, role: RoleKind) -> bool {
    spec.available_to.contains(&role)
}

/// The default answer for a role: the permission's `true_for` / `false_for`.
///
/// Canvas `RoleOverride#uncached_permission_for`: `enabled`.
pub fn default_grant(spec: &PermissionSpec, role: RoleKind) -> bool {
    if spec.false_for.contains(&role) {
        return false;
    }
    spec.true_for.contains(&role)
}

/// Whether the permission survives a concluded enrollment.
///
/// Canvas `RoleOverride#uncached_permission_for` keeps a permission for a
/// concluded enrollment only when `applies_to_concluded` says so.
pub fn applies_to_concluded(spec: &PermissionSpec, role: RoleKind) -> bool {
    match spec.applies_to_concluded {
        Concluded::Never => false,
        Concluded::Any => true,
        Concluded::Roles(roles) => roles.contains(&role),
    }
}

/// Whether the account the role belongs to satisfies `account_only`.
///
/// `account_only: true` only restricts where the permission can be *managed*
/// (Canvas `RoleOverride#uncached_permission_for` leaves it enabled at
/// runtime), so it does not gate the grant.
pub fn account_only_allows(spec: &PermissionSpec, scope: &AccountScope) -> bool {
    match spec.account_only {
        AccountOnly::None | AccountOnly::Account => true,
        AccountOnly::Root => scope.role_account_is_root,
        AccountOnly::SiteAdmin => scope.role_account_is_site_admin,
    }
}

/// Whether the account allows the permission at all.
///
/// Canvas gates 45 permissions on `account_allows`, a lambda over the
/// account's settings and feature flags. The port does not carry Canvas'
/// feature flags, so the layer denies them: allowing what Canvas may deny is
/// worse than refusing a route Canvas would have allowed.
pub fn account_allows(spec: &PermissionSpec, allowed: bool) -> bool {
    match spec.account_allows {
        AccountAllows::Always => true,
        AccountAllows::Required => allowed,
    }
}

/// Whether one grant holds the permission.
pub fn grant_holds(spec: &PermissionSpec, grant: &Grant, account_allows_granted: bool) -> bool {
    if !account_allows(spec, account_allows_granted) {
        return false;
    }
    if !account_only_allows(spec, &grant.scope) {
        return false;
    }
    match grant.state {
        EnrollmentState::Completed => {
            if !applies_to_concluded(spec, grant.role) {
                return false;
            }
        }
        EnrollmentState::Invited => {
            if spec.restrict_future_enrollments {
                return false;
            }
        }
        EnrollmentState::Inactive => return false,
        EnrollmentState::Active => {}
    }
    let default = default_grant(spec, grant.role);
    match grant.override_value {
        // A locked permission keeps its default; an unlocked one takes the
        // override.
        Some(value) if is_overridable(spec, grant.role) => value,
        _ => default,
    }
}

/// Whether any of the user's grants holds the permission.
pub fn evaluate(spec: &PermissionSpec, grants: &[Grant], account_allows_granted: bool) -> bool {
    grants
        .iter()
        .any(|grant| grant_holds(spec, grant, account_allows_granted))
}

/// Whether the permission may be used while masquerading as another user.
///
/// Canvas `Permissions.non_masquerading_permissions`.
pub fn allows_masquerading(spec: &PermissionSpec) -> bool {
    !spec.not_for_masquerading
}

/// Whether the permission is deprecated and hidden from the permission UI.
pub fn is_deprecated(spec: &PermissionSpec) -> bool {
    spec.deprecated
}

/// How a permission behaves for a concluded enrollment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Concluded {
    /// The permission does not survive a concluded enrollment.
    Never,
    /// The permission survives a concluded enrollment for every role.
    Any,
    /// The permission survives a concluded enrollment for these roles only.
    Roles(&'static [RoleKind]),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry;

    fn spec(name: &str) -> &'static PermissionSpec {
        registry::lookup(name).expect("permission in the registry")
    }

    fn teacher() -> Grant {
        Grant::new(RoleKind::TeacherEnrollment, EnrollmentState::Active)
    }

    #[test]
    fn teacher_grants_manage_students_but_not_manage_sis() {
        let manage_students = spec("manage_students");
        assert!(default_grant(manage_students, RoleKind::TeacherEnrollment));
        assert!(!default_grant(manage_students, RoleKind::StudentEnrollment));

        let manage_sis = spec("manage_sis");
        assert!(!default_grant(manage_sis, RoleKind::TeacherEnrollment));
        assert!(default_grant(manage_sis, RoleKind::AccountAdmin));
    }

    #[test]
    fn available_to_locks_the_value_but_does_not_gate_the_default() {
        // `manage_account_settings` is `true_for` AccountAdmin but only
        // `available_to` AccountMembership: an account admin holds it and
        // cannot have it switched off.
        let manage_account_settings = spec("manage_account_settings");
        assert!(default_grant(
            manage_account_settings,
            RoleKind::AccountAdmin
        ));
        assert!(!is_overridable(
            manage_account_settings,
            RoleKind::AccountAdmin
        ));
        assert!(is_overridable(
            manage_account_settings,
            RoleKind::AccountMembership
        ));

        // `read_course_list` is not `true_for` a teacher, and a teacher cannot
        // hold it at all.
        let read_course_list = spec("read_course_list");
        assert!(!default_grant(
            read_course_list,
            RoleKind::TeacherEnrollment
        ));
        assert!(!is_overridable(
            read_course_list,
            RoleKind::TeacherEnrollment
        ));
    }

    #[test]
    fn a_concluded_teacher_keeps_read_roster_but_loses_manage_students() {
        let read_roster = spec("read_roster");
        assert!(applies_to_concluded(
            read_roster,
            RoleKind::TeacherEnrollment
        ));

        let manage_students = spec("manage_students");
        assert!(!applies_to_concluded(
            manage_students,
            RoleKind::TeacherEnrollment
        ));

        let concluded = Grant::new(RoleKind::TeacherEnrollment, EnrollmentState::Completed);
        assert!(grant_holds(read_roster, &concluded, true));
        assert!(!grant_holds(manage_students, &concluded, true));
    }

    #[test]
    fn a_concluded_student_loses_read_roster() {
        let read_roster = spec("read_roster");
        let concluded = Grant::new(RoleKind::StudentEnrollment, EnrollmentState::Completed);
        assert!(!grant_holds(read_roster, &concluded, true));
    }

    #[test]
    fn a_role_override_beats_the_default_only_for_an_overridable_role() {
        let manage_students = spec("manage_students");
        let grant = teacher().with_override(Some(false));
        assert!(!grant_holds(manage_students, &grant, true));

        let grant = teacher().with_override(Some(true));
        assert!(grant_holds(manage_students, &grant, true));

        // The permission is locked for an account admin, so an override that
        // switches it off is ignored.
        let manage_account_settings = spec("manage_account_settings");
        let grant =
            Grant::new(RoleKind::AccountAdmin, EnrollmentState::Active).with_override(Some(false));
        assert!(grant_holds(manage_account_settings, &grant, true));
    }

    #[test]
    fn account_only_gates_root_and_site_admin_permissions() {
        let manage_sis = spec("manage_sis");
        let root = AccountScope {
            role_account_is_root: true,
            role_account_is_site_admin: false,
        };
        let site_admin = AccountScope {
            role_account_is_root: false,
            role_account_is_site_admin: true,
        };
        assert!(account_only_allows(manage_sis, &root));
        assert!(!account_only_allows(manage_sis, &site_admin));
        assert!(!account_only_allows(
            manage_sis,
            &AccountScope::no_account()
        ));

        let manage_jobs = spec("manage_jobs");
        assert!(account_only_allows(manage_jobs, &site_admin));
        assert!(!account_only_allows(manage_jobs, &root));
    }

    #[test]
    fn an_inactive_enrollment_grants_nothing() {
        let manage_students = spec("manage_students");
        let inactive = Grant::new(RoleKind::TeacherEnrollment, EnrollmentState::Inactive);
        assert!(!grant_holds(manage_students, &inactive, true));
    }

    #[test]
    fn a_future_enrollment_loses_restricted_permissions() {
        let create_forum = spec("create_forum");
        assert!(create_forum.restrict_future_enrollments);
        let invited = Grant::new(RoleKind::TeacherEnrollment, EnrollmentState::Invited);
        assert!(!grant_holds(create_forum, &invited, true));
    }

    #[test]
    fn a_permission_gated_on_an_account_setting_is_denied_by_default() {
        let manage_users_in_bulk = spec("manage_users_in_bulk");
        assert_eq!(manage_users_in_bulk.account_allows, AccountAllows::Required);
        let grant = Grant::new(RoleKind::AccountAdmin, EnrollmentState::Active);
        assert!(!grant_holds(manage_users_in_bulk, &grant, false));
        assert!(grant_holds(manage_users_in_bulk, &grant, true));
    }
}
