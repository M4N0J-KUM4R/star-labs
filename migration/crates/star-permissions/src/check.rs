//! The data half of the shared permission layer.
//!
//! [`crate::rules`] holds the decision; this module holds the lookup. Every
//! route in the port asks the same question through this module, so a
//! permission is resolved the same way everywhere:
//!
//! 1. which roles does the acting user hold in this context
//!    (an enrollment, an account membership, a group membership)?
//! 2. what does the permission's registry entry say about each of them?
//! 3. what does a role override say?
//!
//! The Canvas sources for each step:
//!
//! * the roles come from `enrollments` (Canvas `Enrollment#has_permission_to?`)
//!   and from `account_users` (Canvas `AccountUser#enabled_for?`).
//! * the registry entry is `config/initializers/permissions_registry.rb`.
//! * the overrides are `role_overrides`, walked from the root account down to
//!   the context account (Canvas `RoleOverride#uncached_permission_for`).

use sqlx::PgPool;
use star_common::ApiError;

use crate::registry;
use crate::rules::{self, AccountScope, ContextKind, EnrollmentState, Grant, RoleKind};

/// Where a permission is being asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionContext {
    /// An account context (`accounts`, `account_users`).
    Account {
        /// The account the check happens in.
        account_id: i64,
    },
    /// A course context (`courses`, `enrollments`).
    Course {
        /// The course the check happens in.
        course_id: i64,
    },
    /// A group context (`groups`).
    ///
    /// Canvas grants group participation separately from the permission
    /// registry, and the registry names no group role, so a group context
    /// resolves the roles of the course the group belongs to.
    Group {
        /// The group the check happens in.
        group_id: i64,
    },
}

impl PermissionContext {
    /// The kind of context.
    pub fn kind(&self) -> ContextKind {
        match self {
            PermissionContext::Account { .. } => ContextKind::Account,
            PermissionContext::Course { .. } => ContextKind::Course,
            PermissionContext::Group { .. } => ContextKind::Group,
        }
    }

    /// The id of the context.
    pub fn id(&self) -> i64 {
        match self {
            PermissionContext::Account { account_id } => *account_id,
            PermissionContext::Course { course_id } => *course_id,
            PermissionContext::Group { group_id } => *group_id,
        }
    }
}

/// One account in the account chain.
#[derive(Debug, Clone, Copy)]
struct AccountNode {
    id: i64,
}

/// The account chain of `account_id`, root-most first.
///
/// Canvas walks the same chain when it resolves a role override
/// (`RoleOverride#uncached_overrides_for` walks `account.account_chain`).
/// The walk is capped at ten levels, like `star-accounts`, so a circular
/// chain cannot loop forever.
async fn account_chain(db: &PgPool, account_id: i64) -> Result<Vec<AccountNode>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE account_chain AS (
            SELECT id, parent_account_id, 0 AS depth
            FROM accounts
            WHERE id = $1 AND workflow_state <> 'deleted'
            UNION ALL
            SELECT parent.id, parent.parent_account_id, chain.depth + 1
            FROM accounts parent
            JOIN account_chain chain ON parent.id = chain.parent_account_id
            WHERE parent.workflow_state <> 'deleted'
              AND chain.depth < 10
        )
        SELECT id FROM account_chain ORDER BY depth DESC
        "#,
    )
    .bind(account_id)
    .fetch_all(db)
    .await?;

    Ok(rows.into_iter().map(|(id,)| AccountNode { id }).collect())
}

/// Whether the user is a site admin.
///
/// The port marks a site admin with `users.role = 'admin'`. In Canvas the
/// site admin is a member of the site admin account, which sits at the top of
/// every account chain, so a site admin is an admin of every account.
async fn is_site_admin(db: &PgPool, user_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> =
        sqlx::query_as(r#"SELECT 1 FROM users WHERE id = $1 AND role = 'admin'"#)
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

/// The account memberships of the user in the given account chain.
///
/// Canvas `Account#cached_account_users_for`: an admin of a parent account
/// administers every sub-account.
async fn account_memberships(
    db: &PgPool,
    user_id: i64,
    chain: &[AccountNode],
) -> Result<Vec<(i64, RoleKind)>, ApiError> {
    if chain.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = chain.iter().map(|node| node.id).collect();
    let rows: Vec<(i64, String)> = sqlx::query_as(
        r#"
        SELECT account_id, role
        FROM account_users
        WHERE user_id = $1 AND account_id = ANY($2)
        "#,
    )
    .bind(user_id)
    .bind(&ids)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(account_id, role)| {
            let kind = RoleKind::from_port_name(&role).unwrap_or(RoleKind::AccountMembership);
            (account_id, kind)
        })
        .collect())
}

/// One enrollment, with the base role type it carries.
type EnrollmentGrantRow = (Option<i64>, Option<String>, String, String, Option<String>);

/// The enrollments of the user in a course, with the role each carries.
async fn course_enrollments(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<Vec<Grant>, ApiError> {
    let rows: Vec<EnrollmentGrantRow> = sqlx::query_as(
        r#"
        SELECT e.role_id, e.type, e.role, e.workflow_state, r.base_role_type
        FROM enrollments e
        LEFT JOIN roles r ON r.id = e.role_id
        WHERE e.user_id = $1 AND e.course_id = $2
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(
            |(role_id, enrollment_type, role, workflow_state, base_role_type)| {
                let kind = base_role_type
                    .as_deref()
                    .and_then(RoleKind::from_name)
                    .or_else(|| enrollment_type.as_deref().and_then(RoleKind::from_name))
                    .or_else(|| RoleKind::from_port_name(&role))?;
                let role_id = role_id.unwrap_or_else(|| kind.builtin_role_id());
                Some(Grant {
                    role: kind,
                    role_id,
                    state: EnrollmentState::from_workflow_state(&workflow_state),
                    override_value: None,
                    // Canvas resolves an enrollment's permission with no account
                    // (`RoleOverride.enabled_for?(course, action, role_id, nil)`),
                    // so an `account_only: :root` permission never reaches a
                    // course role.
                    scope: AccountScope::no_account(),
                })
            },
        )
        .collect())
}

/// The account a course belongs to.
async fn course_account_id(db: &PgPool, course_id: i64) -> Result<Option<i64>, ApiError> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        r#"SELECT account_id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'"#,
    )
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.and_then(|(account_id,)| account_id))
}

/// The course a group belongs to.
async fn group_course_id(db: &PgPool, group_id: i64) -> Result<Option<i64>, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as(r#"SELECT course_id FROM groups WHERE id = $1"#)
            .bind(group_id)
            .fetch_optional(db)
            .await?;
    Ok(row.and_then(|(course_id,)| course_id))
}

/// The role overrides that apply to `role_id` for `permission`.
///
/// The account chain is walked from the root account down to the context
/// account, and a course override — when the context is a course — is the
/// most specific one, so it comes last.
async fn resolve_override(
    db: &PgPool,
    account_id: Option<i64>,
    course_id: Option<i64>,
    role_id: i64,
    permission: &str,
) -> Result<Option<bool>, ApiError> {
    let mut overrides = match account_id {
        Some(account_id) => crate::role_overrides_for(db, account_id, role_id, permission).await?,
        None => Vec::new(),
    };

    if let Some(course_id) = course_id {
        let course_rows: Vec<crate::RoleOverrideRule> = sqlx::query_as(
            r#"
            SELECT id, permission, enabled, locked, context_type, context_id, role_id
            FROM role_overrides
            WHERE context_type = 'Course'
              AND context_id = $1
              AND role_id = $2
              AND permission = $3
            ORDER BY id ASC
            "#,
        )
        .bind(course_id)
        .bind(role_id)
        .bind(permission)
        .fetch_all(db)
        .await?;
        overrides.extend(course_rows);
    }

    // The deepest unlocked override wins; a locked override is final
    // (Canvas `RoleOverride#locked?`).
    let mut resolved = None;
    for rule in &overrides {
        resolved = Some(rule.enabled);
        if rule.locked {
            break;
        }
    }
    Ok(resolved)
}

/// The port's site admin account.
///
/// Canvas keeps the site admin in its own account (`Account.site_admin`,
/// `app/models/account.rb:2155-2157`); the port models it as the top level
/// account, the one without a parent.
pub async fn site_admin_account_id(db: &PgPool) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id LIMIT 1"#,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(id,)| id))
}

/// The roles the acting user holds in the context, with the role overrides
/// that apply to `permission` folded in.
pub async fn grants_for(
    db: &PgPool,
    user_id: i64,
    context: &PermissionContext,
    permission: &str,
) -> Result<Vec<Grant>, ApiError> {
    let (account_id, course_id) = match context {
        PermissionContext::Account { account_id } => (Some(*account_id), None),
        PermissionContext::Course { course_id } => {
            (course_account_id(db, *course_id).await?, Some(*course_id))
        }
        PermissionContext::Group { group_id } => match group_course_id(db, *group_id).await? {
            Some(course_id) => (course_account_id(db, course_id).await?, Some(course_id)),
            None => (None, None),
        },
    };

    let chain = match account_id {
        Some(account_id) => account_chain(db, account_id).await?,
        None => Vec::new(),
    };
    let root_account_id = chain.first().map(|node| node.id);
    let site_admin = is_site_admin(db, user_id).await?;

    let mut grants: Vec<Grant> = Vec::new();

    if let Some(course_id) = course_id {
        grants.extend(course_enrollments(db, user_id, course_id).await?);
    }

    for (membership_account_id, role) in account_memberships(db, user_id, &chain).await? {
        let scope = AccountScope {
            role_account_is_root: Some(membership_account_id) == root_account_id,
            role_account_is_site_admin: site_admin
                && Some(membership_account_id) == root_account_id,
        };
        grants.push(Grant {
            role,
            role_id: role.builtin_role_id(),
            state: EnrollmentState::Active,
            override_value: None,
            scope,
        });
    }

    // A site admin is a member of the site admin account, which sits at the
    // top of every account chain, so the site admin holds the account
    // permissions in every context.
    if site_admin {
        grants.push(Grant {
            role: RoleKind::AccountAdmin,
            role_id: RoleKind::AccountAdmin.builtin_role_id(),
            state: EnrollmentState::Active,
            override_value: None,
            scope: AccountScope {
                role_account_is_root: true,
                role_account_is_site_admin: true,
            },
        });
    }

    for grant in grants.iter_mut() {
        grant.override_value =
            resolve_override(db, account_id, course_id, grant.role_id, permission).await?;
    }

    Ok(grants)
}

/// Is `permission` granted to `user_id` in `context`?
///
/// The permission must be one Canvas declares; an unknown name is a wiring
/// bug and fails loudly instead of quietly denying.
pub async fn can(
    db: &PgPool,
    user_id: i64,
    context: &PermissionContext,
    permission: &str,
) -> Result<bool, ApiError> {
    let spec = registry::lookup(permission)
        .ok_or_else(|| ApiError::Db(format!("unknown permission: {permission}")))?;
    let grants = grants_for(db, user_id, context, permission).await?;
    // Canvas gates 45 permissions on `account_allows`, a lambda over the
    // account's feature flags. The port does not carry those flags, so they
    // are denied rather than allowed.
    Ok(rules::evaluate(spec, &grants, false))
}

/// [`can`] as a guard: `403 Forbidden` when the permission is not granted.
pub async fn require(
    db: &PgPool,
    user_id: i64,
    context: &PermissionContext,
    permission: &str,
) -> Result<(), ApiError> {
    if can(db, user_id, context, permission).await? {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// [`require`] for a set of permissions, any of which grants access.
///
/// Canvas `grants_any_right?`.
pub async fn require_any(
    db: &PgPool,
    user_id: i64,
    context: &PermissionContext,
    permissions: &[&str],
) -> Result<(), ApiError> {
    for permission in permissions {
        if can(db, user_id, context, permission).await? {
            return Ok(());
        }
    }
    Err(ApiError::Forbidden)
}

/// Whether the user may read the account.
///
/// Canvas' Account `:read` right is not a registry permission
/// (`app/models/account.rb:1726-1736`). It is granted to:
///
/// * anyone with an account membership in the account or an ancestor,
/// * a site admin, and
/// * anyone with an admin enrollment (teacher, TA or designer) in one of the
///   account's own courses (`app/models/account.rb:1756-1758`).
///
/// Canvas' `enrollments.active` scope is `workflow_state <> 'deleted'`
/// (`app/models/enrollment.rb:293`), so a concluded enrollment still counts.
pub async fn can_read_account(
    db: &PgPool,
    user_id: i64,
    account_id: i64,
) -> Result<bool, ApiError> {
    let chain = account_chain(db, account_id).await?;
    if chain.is_empty() {
        // The account does not exist; the caller decides whether that is a
        // 404. Canvas loads the account before it checks the right.
        return Ok(false);
    }

    if is_site_admin(db, user_id).await? {
        return Ok(true);
    }

    if !account_memberships(db, user_id, &chain).await?.is_empty() {
        return Ok(true);
    }

    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1
        FROM enrollments e
        JOIN courses c ON c.id = e.course_id
        WHERE e.user_id = $1
          AND c.account_id = $2
          AND e.workflow_state <> 'deleted'
          AND e.role IN ('teacher', 'ta', 'designer')
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_optional(db)
    .await?;

    Ok(row.is_some())
}

/// [`can_read_account`] as a guard: `403 Forbidden` when the account may not
/// be read.
pub async fn require_read_account(
    db: &PgPool,
    user_id: i64,
    account_id: i64,
) -> Result<(), ApiError> {
    if can_read_account(db, user_id, account_id).await? {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

/// Whether the user may conclude or delete an enrollment.
///
/// Canvas `Enrollment#can_be_concluded_by`
/// (`app/models/enrollment.rb:1010-1015`): a student enrollment is concluded
/// with `manage_students`; every other enrollment needs
/// `allow_course_admin_actions`.
pub async fn can_conclude_enrollment(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    enrollment_role: RoleKind,
) -> Result<bool, ApiError> {
    let context = PermissionContext::Course { course_id };

    if enrollment_role == RoleKind::StudentEnrollment
        && can(db, user_id, &context, "manage_students").await?
    {
        return Ok(true);
    }

    can(db, user_id, &context, "allow_course_admin_actions").await
}

/// Whether the user may delete an enrollment.
///
/// Canvas `Enrollment#can_be_deleted_by`
/// (`app/models/enrollment.rb:1024-1030`): the granular `remove_*` permission
/// for the enrollment's own role, and — for an account admin — the right to
/// remove anybody. Removing *your own* enrollment additionally needs
/// `allow_course_admin_actions` on the course's account, because Canvas will
/// not let a course admin remove themselves.
pub async fn can_delete_enrollment(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    enrolled_user_id: i64,
    enrollment_role: RoleKind,
) -> Result<bool, ApiError> {
    let context = PermissionContext::Course { course_id };
    let permission = match enrollment_role {
        RoleKind::TeacherEnrollment => "remove_teacher_from_course",
        RoleKind::TaEnrollment => "remove_ta_from_course",
        RoleKind::DesignerEnrollment => "remove_designer_from_course",
        RoleKind::ObserverEnrollment => "remove_observer_from_course",
        _ => "remove_student_from_course",
    };

    let mut can_remove = can(db, user_id, &context, permission).await?;

    if can_remove && enrolled_user_id == user_id {
        let account_id = course_account_id(db, course_id).await?;
        if let Some(account_id) = account_id {
            let account_context = PermissionContext::Account { account_id };
            can_remove = can(db, user_id, &account_context, "allow_course_admin_actions").await?;
        }
    }

    Ok(can_remove)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_context_reports_its_kind_and_id() {
        let account = PermissionContext::Account { account_id: 7 };
        assert_eq!(account.kind(), ContextKind::Account);
        assert_eq!(account.id(), 7);

        let course = PermissionContext::Course { course_id: 9 };
        assert_eq!(course.kind(), ContextKind::Course);
        assert_eq!(course.id(), 9);

        let group = PermissionContext::Group { group_id: 11 };
        assert_eq!(group.kind(), ContextKind::Group);
        assert_eq!(group.id(), 11);
    }
}

#[cfg(test)]
mod corpus_tests {
    use super::*;
    use crate::rules::RoleKind;
    use std::sync::atomic::{AtomicI64, Ordering};

    /// The recorded corpus is the contract. These tests pin the decisions the
    /// recorded Canvas made for the core families, expressed against the
    /// shared layer.
    ///
    /// The shape is the one `tools/parity/seed_replay_db.py` writes: an
    /// account, a course in it, and a user enrolled as a teacher in that
    /// course. A teacher is not an account admin, so the account's write
    /// rights are denied, while the account's read rights — which Canvas
    /// grants to a course admin of one of the account's courses
    /// (`app/models/account.rb:1756-1758`) — are not.
    struct Scenario {
        pool: PgPool,
        account_id: i64,
        course_id: i64,
        user_id: i64,
    }

    static NEXT_ID: AtomicI64 = AtomicI64::new(0);

    impl Scenario {
        /// Seed the scenario, or `None` when there is no database to seed.
        async fn seed(pool: PgPool) -> Option<Self> {
            let base = 8_000_000_000_000i64 + NEXT_ID.fetch_add(1, Ordering::SeqCst) * 10;
            let account_id = base;
            let course_id = base + 1;
            let user_id = base + 2;

            sqlx::query(
                "INSERT INTO accounts (id, name, parent_account_id, workflow_state) \
                 VALUES ($1, $2, NULL, 'registered')",
            )
            .bind(account_id)
            .bind(format!("permission-layer account {account_id}"))
            .execute(&pool)
            .await
            .expect("seed account");

            sqlx::query(
                "INSERT INTO users (id, name, sortable_name, email, password_hash, role) \
                 VALUES ($1, $2, $2, $3, 'not-a-real-hash', 'teacher')",
            )
            .bind(user_id)
            .bind(format!("permission-layer user {user_id}"))
            .bind(format!("permission-layer-{user_id}@example.com"))
            .execute(&pool)
            .await
            .expect("seed user");

            sqlx::query(
                "INSERT INTO courses (id, name, course_code, account_id) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(course_id)
            .bind(format!("permission-layer course {course_id}"))
            .bind(format!("PLC{course_id}"))
            .bind(account_id)
            .execute(&pool)
            .await
            .expect("seed course");

            sqlx::query(
                "INSERT INTO enrollments (id, user_id, course_id, role, workflow_state) \
                 VALUES ($1, $2, $3, 'teacher', 'active')",
            )
            .bind(base + 3)
            .bind(user_id)
            .bind(course_id)
            .execute(&pool)
            .await
            .expect("seed enrollment");

            Some(Scenario {
                pool,
                account_id,
                course_id,
                user_id,
            })
        }

        async fn cleanup(&self) {
            let _ = sqlx::query("DELETE FROM enrollments WHERE course_id = $1")
                .bind(self.course_id)
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("DELETE FROM courses WHERE id = $1")
                .bind(self.course_id)
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("DELETE FROM users WHERE id = $1")
                .bind(self.user_id)
                .execute(&self.pool)
                .await;
            let _ = sqlx::query("DELETE FROM accounts WHERE id = $1")
                .bind(self.account_id)
                .execute(&self.pool)
                .await;
        }
    }

    /// A migrated database, or `None` when there is none.
    async fn pool() -> Option<PgPool> {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            println!("SKIP: DATABASE_URL not set");
            return None;
        };
        let pool = match sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
        {
            Ok(pool) => pool,
            Err(err) => {
                println!("SKIP: cannot connect to Postgres: {err}");
                return None;
            }
        };
        let ready: Option<(bool,)> = sqlx::query_as(
            r#"
            SELECT to_regclass('public.users') IS NOT NULL
               AND to_regclass('public.accounts') IS NOT NULL
               AND to_regclass('public.enrollments') IS NOT NULL
               AND to_regclass('public.role_overrides') IS NOT NULL
            "#,
        )
        .fetch_optional(&pool)
        .await
        .expect("probe schema");
        match ready {
            Some((true,)) => Some(pool),
            _ => {
                println!("SKIP: schema not migrated");
                None
            }
        }
    }

    #[tokio::test]
    async fn a_course_admin_may_read_the_account_but_not_manage_it() {
        let Some(pool) = pool().await else {
            return;
        };
        let Some(scenario) = Scenario::seed(pool).await else {
            return;
        };
        let account = PermissionContext::Account {
            account_id: scenario.account_id,
        };

        // Canvas answered 200 for GET /api/v1/accounts/6.
        assert!(
            can_read_account(&scenario.pool, scenario.user_id, scenario.account_id)
                .await
                .expect("read account")
        );

        // Canvas answered 403 for the account's settings and its course list.
        assert!(!can(
            &scenario.pool,
            scenario.user_id,
            &account,
            "manage_account_settings"
        )
        .await
        .expect("manage_account_settings"));
        assert!(!can(
            &scenario.pool,
            scenario.user_id,
            &account,
            "read_course_list"
        )
        .await
        .expect("read_course_list"));
        assert!(!can(
            &scenario.pool,
            scenario.user_id,
            &account,
            "manage_account_memberships"
        )
        .await
        .expect("manage_account_memberships"));
        // `manage_users_in_bulk` is gated on an account feature Canvas had
        // switched off, so the layer denies it.
        assert!(!can(
            &scenario.pool,
            scenario.user_id,
            &account,
            "manage_users_in_bulk"
        )
        .await
        .expect("manage_users_in_bulk"));

        scenario.cleanup().await;
    }

    #[tokio::test]
    async fn a_course_admin_holds_the_course_rights_canvas_granted() {
        let Some(pool) = pool().await else {
            return;
        };
        let Some(scenario) = Scenario::seed(pool).await else {
            return;
        };
        let course = PermissionContext::Course {
            course_id: scenario.course_id,
        };

        // Canvas answered 200 for the enrollment list.
        assert!(
            can(&scenario.pool, scenario.user_id, &course, "read_roster")
                .await
                .expect("read_roster")
        );
        assert!(require_any(
            &scenario.pool,
            scenario.user_id,
            &course,
            &["read_roster", "view_all_grades", "manage_grades"]
        )
        .await
        .is_ok());
        // The teacher may add a student and conclude their own enrollment.
        assert!(can(
            &scenario.pool,
            scenario.user_id,
            &course,
            "add_student_to_course"
        )
        .await
        .expect("add_student_to_course"));
        assert!(can(
            &scenario.pool,
            scenario.user_id,
            &course,
            "allow_course_admin_actions"
        )
        .await
        .expect("allow_course_admin_actions"));
        // ... but not the account-only rights.
        assert!(
            !can(&scenario.pool, scenario.user_id, &course, "manage_sis")
                .await
                .expect("manage_sis")
        );

        scenario.cleanup().await;
    }

    #[tokio::test]
    async fn an_enrollment_of_ones_own_is_not_deletable_by_its_own_course_admin() {
        let Some(pool) = pool().await else {
            return;
        };
        let Some(scenario) = Scenario::seed(pool).await else {
            return;
        };
        // Canvas answered 403 for PUT /api/v1/courses/1/enrollments/1/reactivate
        // and 200 for DELETE /api/v1/courses/1/enrollments/1.
        assert!(!can_delete_enrollment(
            &scenario.pool,
            scenario.user_id,
            scenario.course_id,
            scenario.user_id,
            RoleKind::TeacherEnrollment
        )
        .await
        .expect("delete own enrollment"));
        assert!(can_conclude_enrollment(
            &scenario.pool,
            scenario.user_id,
            scenario.course_id,
            RoleKind::TeacherEnrollment
        )
        .await
        .expect("conclude own enrollment"));

        scenario.cleanup().await;
    }

    #[tokio::test]
    async fn an_unknown_permission_fails_loudly() {
        let Some(pool) = pool().await else {
            return;
        };
        let course = PermissionContext::Course { course_id: 1 };
        let err = can(&pool, 1, &course, "not_a_canvas_permission")
            .await
            .expect_err("unknown permission must not resolve");
        assert!(matches!(err, ApiError::Db(_)));
    }
}
