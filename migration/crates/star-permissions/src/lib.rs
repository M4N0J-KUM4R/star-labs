//! Canvas-compatible permission model: course roles, account admins, and role
//! overrides.
//!
//! Canvas resolves a user's rights in layers, all represented here:
//!
//! 1. the course role from `enrollments` (`StudentEnrollment` …),
//! 2. account membership from `account_users`, which is inherited by the
//!    account's descendants (an admin of a parent account administers every
//!    sub-account),
//! 3. `role_overrides`, which enable or lock a permission for one role on an
//!    account and are inherited down the account chain until an override is
//!    locked.
//!
//! This crate is a library only — there is no axum router. Domain crates and
//! `star-http` call these helpers so the rules live in one place.
//!
//! Note: [`Role`] values are the star-lms role strings stored in
//! `enrollments.role` ("student", "teacher", …); [`Role::canvas_enrollment_type`]
//! returns the Canvas API spelling ("TeacherEnrollment", …).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sqlx::PgPool;
use star_common::ApiError;

/// Canvas enrollment roles understood by star-lms.
///
/// Serializes as the star-lms role string ("teacher") so JSON responses keep
/// the shape of the existing enrollment endpoints; use
/// [`Role::canvas_enrollment_type`] for the Canvas `Enrollment#type` spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Student,
    Teacher,
    Ta,
    Designer,
    Observer,
}

impl Role {
    /// Every role, strongest first (Canvas grants the broadest role when a
    /// user holds several enrollments in the same course).
    pub const ALL: [Role; 5] = [
        Role::Teacher,
        Role::Ta,
        Role::Designer,
        Role::Observer,
        Role::Student,
    ];

    /// star-lms role string stored in `enrollments.role`.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Student => "student",
            Role::Teacher => "teacher",
            Role::Ta => "ta",
            Role::Designer => "designer",
            Role::Observer => "observer",
        }
    }

    /// Canvas enrollment type used by the API (`TeacherEnrollment`, …).
    pub fn canvas_enrollment_type(self) -> &'static str {
        match self {
            Role::Student => "StudentEnrollment",
            Role::Teacher => "TeacherEnrollment",
            Role::Ta => "TaEnrollment",
            Role::Designer => "DesignerEnrollment",
            Role::Observer => "ObserverEnrollment",
        }
    }

    /// Fixed id for Canvas's built-in enrollment roles.
    ///
    /// Canvas keeps built-in roles in a `roles` table with generated ids;
    /// star-lms has no `roles` table, so the built-in roles use these stable
    /// ids and `role_overrides.role_id` refers to them.
    pub fn builtin_role_id(self) -> i64 {
        match self {
            Role::Student => 1,
            Role::Teacher => 2,
            Role::Ta => 3,
            Role::Designer => 4,
            Role::Observer => 5,
        }
    }

    /// Inverse of [`Role::builtin_role_id`].
    pub fn from_builtin_role_id(role_id: i64) -> Option<Role> {
        Role::ALL
            .iter()
            .copied()
            .find(|role| role.builtin_role_id() == role_id)
    }

    /// Precedence used when one user holds multiple enrollments in a course.
    fn priority(self) -> u8 {
        match self {
            Role::Teacher => 0,
            Role::Ta => 1,
            Role::Designer => 2,
            Role::Observer => 3,
            Role::Student => 4,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Role {
    type Err = ApiError;

    /// Accepts both the star-lms role strings and Canvas enrollment types,
    /// case-insensitively and ignoring separators ("TaEnrollment", "ta",
    /// "Designer_Enrollment").
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let normalized = raw
            .trim()
            .to_ascii_lowercase()
            .replace('_', "")
            .replace('-', "")
            .replace(' ', "");

        match normalized.as_str() {
            "student" | "studentenrollment" => Ok(Role::Student),
            "teacher" | "teacherenrollment" => Ok(Role::Teacher),
            "ta" | "taenrollment" | "teachingassistant" => Ok(Role::Ta),
            "designer" | "designerenrollment" => Ok(Role::Designer),
            "observer" | "observerenrollment" => Ok(Role::Observer),
            _ => Err(ApiError::BadRequest(format!("unknown course role: {raw}"))),
        }
    }
}

impl Serialize for Role {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// Pick the strongest role from raw `enrollments.role` values. Unknown values
/// (including "admin", which is an account role, not a course role) are
/// ignored.
fn strongest_role<S: AsRef<str>>(roles: impl IntoIterator<Item = S>) -> Option<Role> {
    roles
        .into_iter()
        .filter_map(|role| role.as_ref().parse::<Role>().ok())
        .min_by_key(|role| role.priority())
}

// ---------------------------------------------------------------------------
// Course roles
// ---------------------------------------------------------------------------

/// Look up a user's course role from `enrollments`.
///
/// Mirrors Canvas's enrollment lookup: only `active` and `invited`
/// enrollments grant rights; `completed`, `inactive`, and `deleted`
/// enrollments do not. When a user holds several roles in the course the
/// strongest one wins: teacher > ta > designer > observer > student.
pub async fn course_role(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<Option<Role>, ApiError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT role
        FROM enrollments
        WHERE user_id = $1
          AND course_id = $2
          AND workflow_state IN ('active', 'invited')
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_all(db)
    .await?;

    Ok(strongest_role(rows.into_iter().map(|(role,)| role)))
}

/// [`course_role`] for guards: `403 Forbidden` when the user has no active
/// enrollment in the course (Canvas `authorized_action` with a course role).
pub async fn require_course_role(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<Role, ApiError> {
    course_role(db, user_id, course_id)
        .await?
        .ok_or(ApiError::Forbidden)
}

// ---------------------------------------------------------------------------
// Account admins
// ---------------------------------------------------------------------------

/// Whether the user administers `account_id` or any ancestor account.
///
/// Canvas `Account#account_users` includes every account membership; admins of
/// a parent account administer all of its children. The recursive CTE walks
/// the `accounts.parent_account_id` chain (capped at 10 levels, like
/// `star-accounts`, so a circular chain cannot loop forever).
pub async fn is_account_admin(
    db: &PgPool,
    user_id: i64,
    account_id: i64,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE account_chain AS (
            SELECT id, parent_account_id, 0 AS depth
            FROM accounts
            WHERE id = $2 AND workflow_state <> 'deleted'
            UNION ALL
            SELECT parent.id, parent.parent_account_id, chain.depth + 1
            FROM accounts parent
            JOIN account_chain chain ON parent.id = chain.parent_account_id
            WHERE parent.workflow_state <> 'deleted'
              AND chain.depth < 10
        )
        SELECT 1
        FROM account_users au
        JOIN account_chain chain ON chain.id = au.account_id
        WHERE au.user_id = $1
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_optional(db)
    .await?;

    Ok(row.is_some())
}

/// Whether the user administers the course's account (`courses.account_id`)
/// or any ancestor account. `404 Not Found` when the course does not exist;
/// `false` when the course has no account.
pub async fn is_course_account_admin(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<bool, ApiError> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        r#"SELECT account_id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'"#,
    )
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    match row {
        None => Err(ApiError::NotFound),
        Some((None,)) => Ok(false),
        Some((Some(account_id),)) => is_account_admin(db, user_id, account_id).await,
    }
}

// ---------------------------------------------------------------------------
// Role overrides
// ---------------------------------------------------------------------------

/// One row from `role_overrides` (Canvas `RoleOverride`): a permission is
/// switched on or off for a role on an account, optionally locked so that
/// descendant accounts cannot change it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct RoleOverrideRule {
    pub id: i64,
    pub permission: String,
    pub enabled: bool,
    pub locked: bool,
    pub context_type: String,
    pub context_id: i64,
    pub role_id: i64,
}

/// Every override that applies to `role_id` for `permission` on `account_id`
/// and its ancestors, ordered root-most first so they can be folded.
pub async fn role_overrides_for(
    db: &PgPool,
    account_id: i64,
    role_id: i64,
    permission: &str,
) -> Result<Vec<RoleOverrideRule>, ApiError> {
    let overrides: Vec<RoleOverrideRule> = sqlx::query_as(
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
        SELECT ro.id, ro.permission, ro.enabled, ro.locked,
               ro.context_type, ro.context_id, ro.role_id
        FROM role_overrides ro
        JOIN account_chain chain
          ON ro.context_type = 'Account' AND ro.context_id = chain.id
        WHERE ro.role_id = $2 AND ro.permission = $3
        ORDER BY chain.depth DESC, ro.id ASC
        "#,
    )
    .bind(account_id)
    .bind(role_id)
    .bind(permission)
    .fetch_all(db)
    .await?;

    Ok(overrides)
}

/// Resolve an override chain: the deepest unlocked override wins, and a locked
/// override is final because descendant accounts cannot change it (Canvas
/// `RoleOverride#locked?`). `None` means no override applies, so the caller
/// should use the permission's default.
pub async fn resolve_role_override(
    db: &PgPool,
    account_id: i64,
    role_id: i64,
    permission: &str,
) -> Result<Option<bool>, ApiError> {
    let overrides = role_overrides_for(db, account_id, role_id, permission).await?;
    Ok(fold_overrides(&overrides))
}

/// Fold overrides walked from the root account down to the target account.
fn fold_overrides(overrides: &[RoleOverrideRule]) -> Option<bool> {
    let mut resolved = None;
    for rule in overrides {
        resolved = Some(rule.enabled);
        if rule.locked {
            break;
        }
    }
    resolved
}

/// Resolve a permission for a user in a course through all three layers
/// (course role → course account → inherited overrides).
///
/// `Ok(None)` means the user is not enrolled or no override applies; callers
/// then apply the permission's default. This is the entry point later phases
/// should use instead of re-implementing role checks.
pub async fn course_permission_enabled(
    db: &PgPool,
    user_id: i64,
    course_id: i64,
    permission: &str,
) -> Result<Option<bool>, ApiError> {
    let Some(role) = course_role(db, user_id, course_id).await? else {
        return Ok(None);
    };

    let row: Option<(Option<i64>,)> = sqlx::query_as(
        r#"SELECT account_id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'"#,
    )
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    let Some((Some(account_id),)) = row else {
        return Ok(None);
    };

    resolve_role_override(db, account_id, role.builtin_role_id(), permission).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_canvas_enrollment_types_and_short_names() {
        assert_eq!("TeacherEnrollment".parse::<Role>().unwrap(), Role::Teacher);
        assert_eq!("ta".parse::<Role>().unwrap(), Role::Ta);
        assert_eq!(
            "Designer_Enrollment".parse::<Role>().unwrap(),
            Role::Designer
        );
        assert_eq!("observer".parse::<Role>().unwrap(), Role::Observer);
        assert_eq!("StudentEnrollment".parse::<Role>().unwrap(), Role::Student);
        assert!("admin".parse::<Role>().is_err());
    }

    #[test]
    fn strongest_course_role_wins() {
        assert_eq!(strongest_role(["student", "TaEnrollment"]), Some(Role::Ta));
        assert_eq!(
            strongest_role(["ObserverEnrollment", "teacher", "StudentEnrollment"]),
            Some(Role::Teacher)
        );
        assert_eq!(strongest_role(["admin", "garbage"]), None);
        assert_eq!(strongest_role(std::iter::empty::<String>()), None);
    }

    #[test]
    fn deepest_override_wins_until_locked() {
        let root = rule(1, false, false, 1);
        let leaf = rule(2, true, false, 2);
        assert_eq!(fold_overrides(&[root, leaf]), Some(true));

        let locked_root = rule(3, false, true, 1);
        let locked = rule(4, true, false, 2);
        assert_eq!(fold_overrides(&[locked_root, locked]), Some(false));
        assert_eq!(fold_overrides(&[]), None);
    }

    #[test]
    fn builtin_role_ids_round_trip() {
        for role in Role::ALL {
            assert_eq!(
                Role::from_builtin_role_id(role.builtin_role_id()),
                Some(role)
            );
        }
        assert_eq!(Role::Ta.canvas_enrollment_type(), "TaEnrollment");
        assert_eq!(Role::Student.as_str(), "student");
        assert_eq!(Role::from_builtin_role_id(999), None);
    }

    fn rule(id: i64, enabled: bool, locked: bool, context_id: i64) -> RoleOverrideRule {
        RoleOverrideRule {
            id,
            permission: "read".to_string(),
            enabled,
            locked,
            context_type: "Account".to_string(),
            context_id,
            role_id: Role::Teacher.builtin_role_id(),
        }
    }
}
