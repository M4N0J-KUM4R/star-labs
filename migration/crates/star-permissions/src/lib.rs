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
//! Phase 3 adds the permission administration endpoints:
//!
//! * `GET/PUT /accounts/{accountId}/role_overrides`
//! * `GET/PUT /courses/{courseId}/role_overrides`
//!
//! [`get_role_overrides`] and [`put_role_overrides`] carry the full endpoint
//! behavior (context lookup, `manage_role_overrides` authorization, payload
//! validation, upsert, Canvas-shaped rows), so the router only has to parse
//! the path context and call them.
//!
//! Note: [`Role`] values are the star-lms role strings stored in
//! `enrollments.role` ("student", "teacher", …); [`Role::canvas_enrollment_type`]
//! returns the Canvas API spelling ("TeacherEnrollment", …).

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
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
        let normalized = raw.trim().to_ascii_lowercase().replace(['_', '-', ' '], "");

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

// ---------------------------------------------------------------------------
// Role override administration endpoints
// ---------------------------------------------------------------------------

/// Polymorphic `role_overrides.context_type` used by the Phase 3 endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleOverrideContext {
    Account,
    Course,
}

impl RoleOverrideContext {
    /// Canvas context class name stored in `role_overrides.context_type`.
    pub fn as_str(self) -> &'static str {
        match self {
            RoleOverrideContext::Account => "Account",
            RoleOverrideContext::Course => "Course",
        }
    }
}

impl fmt::Display for RoleOverrideContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for RoleOverrideContext {
    type Err = ApiError;

    /// Accepts the Canvas context class names case-insensitively.
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "account" => Ok(RoleOverrideContext::Account),
            "course" => Ok(RoleOverrideContext::Course),
            _ => Err(ApiError::BadRequest(format!(
                "unknown role override context: {raw}"
            ))),
        }
    }
}

/// A `role_overrides` row in Canvas `RoleOverride` JSON shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct RoleOverrideView {
    pub id: i64,
    pub permission: String,
    pub enabled: bool,
    pub locked: bool,
    pub context_type: String,
    pub context_id: i64,
    pub role_id: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One Canvas-style `permissions[<name>]` entry of a PUT body.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PermissionUpdate {
    /// New value of `enabled`; ignored when `explicit` is false.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// New value of `locked`.
    #[serde(default)]
    pub locked: Option<bool>,
    /// Canvas `explicit`: only an explicit update changes `enabled`.
    /// Absent is treated as explicit because this endpoint writes the row
    /// directly instead of walking an inheritance chain.
    #[serde(default)]
    pub explicit: Option<bool>,
}

impl PermissionUpdate {
    /// Value to store in `role_overrides.enabled`, if this entry changes it.
    pub fn enabled_change(&self) -> Option<bool> {
        if self.explicit == Some(false) {
            None
        } else {
            self.enabled
        }
    }

    fn validate(&self, permission: &str) -> Result<(), ApiError> {
        if permission.trim().is_empty() {
            return Err(ApiError::BadRequest(
                "permission name must not be blank".to_string(),
            ));
        }
        if self.enabled.is_none() && self.locked.is_none() {
            return Err(ApiError::BadRequest(format!(
                "permission {permission} must set enabled or locked"
            )));
        }
        Ok(())
    }
}

/// PUT body for a context's role overrides.
#[derive(Debug, Clone, Deserialize)]
pub struct RoleOverridesUpdate {
    /// Built-in role the permissions apply to ([`Role::builtin_role_id`]);
    /// sent in the body because the Canvas route carries the role in the path.
    pub role_id: i64,
    /// Canvas-style map of permission name to its update.
    #[serde(default)]
    pub permissions: BTreeMap<String, PermissionUpdate>,
}

/// Validate a PUT body; returns the built-in role it addresses.
pub fn validate_role_overrides_update(update: &RoleOverridesUpdate) -> Result<Role, ApiError> {
    let role = Role::from_builtin_role_id(update.role_id)
        .ok_or_else(|| ApiError::BadRequest(format!("unknown role_id: {}", update.role_id)))?;
    if update.permissions.is_empty() {
        return Err(ApiError::BadRequest(
            "permissions must not be empty".to_string(),
        ));
    }
    for (permission, change) in &update.permissions {
        change.validate(permission)?;
    }
    Ok(role)
}

/// Whether the account/course context exists (not soft-deleted).
pub async fn role_override_context_exists(
    db: &PgPool,
    context: RoleOverrideContext,
    context_id: i64,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = match context {
        RoleOverrideContext::Account => {
            sqlx::query_as("SELECT 1 FROM accounts WHERE id = $1 AND workflow_state <> 'deleted'")
                .bind(context_id)
                .fetch_optional(db)
                .await?
        }
        RoleOverrideContext::Course => {
            sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
                .bind(context_id)
                .fetch_optional(db)
                .await?
        }
    };
    Ok(row.is_some())
}

/// Global star-lms admin (`users.role = 'admin'`).
async fn is_global_admin(db: &PgPool, user_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM users WHERE id = $1 AND role = 'admin'")
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    Ok(row.is_some())
}

/// Whether the user holds Canvas `manage_role_overrides` for the context.
///
/// Canvas grants `manage_role_overrides` to AccountAdmin only
/// (`config/initializers/permissions_registry.rb`), so both endpoint contexts
/// accept a global admin or an account admin of the account/ancestor chain;
/// a course is administered through its `courses.account_id`.
pub async fn can_manage_role_overrides(
    db: &PgPool,
    user_id: i64,
    context: RoleOverrideContext,
    context_id: i64,
) -> Result<bool, ApiError> {
    if is_global_admin(db, user_id).await? {
        return Ok(true);
    }
    match context {
        RoleOverrideContext::Account => is_account_admin(db, user_id, context_id).await,
        RoleOverrideContext::Course => is_course_account_admin(db, user_id, context_id).await,
    }
}

/// [`can_manage_role_overrides`] as a guard: `403 Forbidden` when unauthorized.
pub async fn require_manage_role_overrides(
    db: &PgPool,
    user_id: i64,
    context: RoleOverrideContext,
    context_id: i64,
) -> Result<(), ApiError> {
    if can_manage_role_overrides(db, user_id, context, context_id).await? {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}

async fn fetch_context_role_overrides(
    db: &PgPool,
    context: RoleOverrideContext,
    context_id: i64,
) -> Result<Vec<RoleOverrideView>, ApiError> {
    let rows: Vec<RoleOverrideView> = sqlx::query_as(
        r#"
        SELECT id, permission, enabled, locked, context_type, context_id,
               role_id, created_at, updated_at
        FROM role_overrides
        WHERE context_type = $1 AND context_id = $2
        ORDER BY permission ASC, role_id ASC, id ASC
        "#,
    )
    .bind(context.as_str())
    .bind(context_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// `GET /accounts/{accountId}/role_overrides` and
/// `GET /courses/{courseId}/role_overrides`.
///
/// `404` when the context does not exist, `403` unless the caller holds
/// `manage_role_overrides`; otherwise every override of the context as a bare
/// JSON array ordered by permission then role.
pub async fn get_role_overrides(
    db: &PgPool,
    user_id: i64,
    context: RoleOverrideContext,
    context_id: i64,
) -> Result<Vec<RoleOverrideView>, ApiError> {
    if !role_override_context_exists(db, context, context_id).await? {
        return Err(ApiError::NotFound);
    }
    require_manage_role_overrides(db, user_id, context, context_id).await?;
    fetch_context_role_overrides(db, context, context_id).await
}

/// `PUT /accounts/{accountId}/role_overrides` and
/// `PUT /courses/{courseId}/role_overrides`.
///
/// Upserts the body's permissions for one built-in role and returns the
/// context's overrides (same payload as GET). Existing rows keep their
/// `enabled`/`locked` value when the entry omits it, and `updated_at` moves.
pub async fn put_role_overrides(
    db: &PgPool,
    user_id: i64,
    context: RoleOverrideContext,
    context_id: i64,
    update: &RoleOverridesUpdate,
) -> Result<Vec<RoleOverrideView>, ApiError> {
    if !role_override_context_exists(db, context, context_id).await? {
        return Err(ApiError::NotFound);
    }
    require_manage_role_overrides(db, user_id, context, context_id).await?;

    let role = validate_role_overrides_update(update)?;
    let role_id = role.builtin_role_id();

    let mut tx = db.begin().await?;
    for (permission, change) in &update.permissions {
        sqlx::query(
            r#"
            INSERT INTO role_overrides
                (permission, enabled, locked, context_type, context_id, role_id)
            VALUES ($1, COALESCE($2::boolean, TRUE), COALESCE($3::boolean, FALSE),
                    $4, $5, $6)
            ON CONFLICT (context_type, context_id, role_id, permission)
            DO UPDATE SET
                enabled = COALESCE($2::boolean, role_overrides.enabled),
                locked = COALESCE($3::boolean, role_overrides.locked),
                updated_at = now()
            "#,
        )
        .bind(permission)
        .bind(change.enabled_change())
        .bind(change.locked)
        .bind(context.as_str())
        .bind(context_id)
        .bind(role_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;

    fetch_context_role_overrides(db, context, context_id).await
}

// ---------------------------------------------------------------------------
// Canvas Role API (`accounts/roles_controller`)
// ---------------------------------------------------------------------------

/// A `roles` row in Canvas `Role` JSON shape. `account_id` is `null` for the
/// five built-in enrollment roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct RoleView {
    pub id: i64,
    pub name: String,
    pub label: String,
    pub base_role_type: String,
    pub workflow_state: String,
    pub account_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for create/update; Canvas posts `role[label]=…` (nested), so
/// both the nested and the flat form are accepted.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RoleApiInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub base_role_type: Option<String>,
    #[serde(default)]
    pub workflow_state: Option<String>,
}

/// Canvas create body: `{"role": {...}}` or flat `{...}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum RoleRequest {
    Nested { role: RoleApiInput },
    Flat(RoleApiInput),
}

impl RoleRequest {
    pub fn into_input(self) -> RoleApiInput {
        match self {
            RoleRequest::Nested { role } => role,
            RoleRequest::Flat(input) => input,
        }
    }
}

impl RoleApiInput {
    /// Canvas base role type the custom role extends; `400` when unknown.
    fn base_role(&self) -> Result<Option<Role>, ApiError> {
        match &self.base_role_type {
            None => Ok(None),
            Some(raw) => raw
                .parse::<Role>()
                .map(Some)
                .map_err(|_| ApiError::BadRequest(format!("unknown base_role_type: {raw}"))),
        }
    }

    fn validate_workflow_state(&self) -> Result<(), ApiError> {
        match self.workflow_state.as_deref() {
            None | Some("active") | Some("inactive") | Some("deleted") => Ok(()),
            Some(other) => Err(ApiError::BadRequest(format!(
                "unknown workflow_state: {other}"
            ))),
        }
    }
}

/// `404` when the account does not exist, `403` unless the caller can manage
/// roles on it (reuses the `manage_role_overrides` admin checks).
async fn require_roles_admin(db: &PgPool, user_id: i64, account_id: i64) -> Result<(), ApiError> {
    if !role_override_context_exists(db, RoleOverrideContext::Account, account_id).await? {
        return Err(ApiError::NotFound);
    }
    require_manage_role_overrides(db, user_id, RoleOverrideContext::Account, account_id).await
}

const ROLE_COLUMNS: &str = "id, name, label, base_role_type, workflow_state, account_id, \
                            created_at, updated_at";

/// `GET /accounts/{accountId}/roles` — the account's custom roles and all
/// built-in roles (NULL account_id), oldest first.
pub async fn roles_list(
    db: &PgPool,
    user_id: i64,
    account_id: i64,
) -> Result<Vec<RoleView>, ApiError> {
    require_roles_admin(db, user_id, account_id).await?;

    let mut rows: Vec<RoleView> = sqlx::query_as(&format!(
        "SELECT {ROLE_COLUMNS} FROM roles
         WHERE account_id = $1 AND workflow_state <> 'deleted'
         UNION ALL
         SELECT {ROLE_COLUMNS} FROM roles WHERE account_id IS NULL
         ORDER BY id ASC"
    ))
    .bind(account_id)
    .fetch_all(db)
    .await?;
    // Built-in roles (NULL account_id) must report workflow_state = "built_in".
    for role in rows.iter_mut() {
        if role.account_id.is_none() {
            role.workflow_state = "built_in".to_string();
        }
    }
    Ok(rows)
}

/// `POST /accounts/{accountId}/roles` — creates a custom role.
///
/// `400` when `label` is missing/blank or `base_role_type` is unknown, `409`
/// when the account already has a role with that name.
pub async fn role_create(
    db: &PgPool,
    user_id: i64,
    account_id: i64,
    input: &RoleApiInput,
) -> Result<RoleView, ApiError> {
    require_roles_admin(db, user_id, account_id).await?;

    let label = input
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .ok_or_else(|| ApiError::BadRequest("role label is required".to_string()))?;
    input.validate_workflow_state()?;
    let base_role_type = input
        .base_role()?
        .map(|role| role.canvas_enrollment_type())
        .unwrap_or("StudentEnrollment");
    let name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(label);

    let created: Option<RoleView> = sqlx::query_as(&format!(
        "INSERT INTO roles (account_id, name, label, base_role_type, workflow_state)
         VALUES ($1, $2, $3, $4, COALESCE($5, 'active'))
         ON CONFLICT (account_id, name) DO NOTHING
         RETURNING {ROLE_COLUMNS}"
    ))
    .bind(account_id)
    .bind(name)
    .bind(label)
    .bind(base_role_type)
    .bind(input.workflow_state.as_deref())
    .fetch_optional(db)
    .await?;

    created.ok_or_else(|| ApiError::Conflict(format!("role {name} already exists")))
}

/// `GET /accounts/{accountId}/roles/{id}` — a custom role of the account or
/// any built-in role; `404` otherwise.
pub async fn role_show(
    db: &PgPool,
    user_id: i64,
    account_id: i64,
    role_id: i64,
) -> Result<RoleView, ApiError> {
    require_roles_admin(db, user_id, account_id).await?;
    fetch_role(db, account_id, role_id).await
}

async fn fetch_role(db: &PgPool, account_id: i64, role_id: i64) -> Result<RoleView, ApiError> {
    let role: Option<RoleView> = sqlx::query_as(&format!(
        "SELECT {ROLE_COLUMNS} FROM roles
         WHERE id = $1 AND (account_id = $2 OR account_id IS NULL)
           AND workflow_state <> 'deleted'"
    ))
    .bind(role_id)
    .bind(account_id)
    .fetch_optional(db)
    .await?;
    let mut role = role.ok_or(ApiError::NotFound)?;
    // Built-in roles (NULL account_id) must report workflow_state = "built_in".
    if role.account_id.is_none() {
        role.workflow_state = "built_in".to_string();
    }
    Ok(role)
}

/// `PUT /accounts/{accountId}/roles/{id}` — updates the supplied fields.
/// Built-in roles cannot be modified (`400`).
pub async fn role_update(
    db: &PgPool,
    user_id: i64,
    account_id: i64,
    role_id: i64,
    input: &RoleApiInput,
) -> Result<RoleView, ApiError> {
    require_roles_admin(db, user_id, account_id).await?;
    input.validate_workflow_state()?;
    let base_role_type = input.base_role()?.map(|role| role.canvas_enrollment_type());
    let label = input
        .label
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty());
    let name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());

    let updated: Option<RoleView> = sqlx::query_as(&format!(
        "UPDATE roles SET
             label = COALESCE($3, label),
             name = COALESCE($4, name),
             base_role_type = COALESCE($5, base_role_type),
             workflow_state = COALESCE($6, workflow_state),
             updated_at = now()
         WHERE id = $1 AND account_id = $2 AND workflow_state <> 'deleted'
         RETURNING {ROLE_COLUMNS}"
    ))
    .bind(role_id)
    .bind(account_id)
    .bind(label)
    .bind(name)
    .bind(base_role_type)
    .bind(input.workflow_state.as_deref())
    .fetch_optional(db)
    .await?;

    match updated {
        Some(role) => Ok(role),
        None => Err(reject_builtin_or_missing(db, account_id, role_id).await),
    }
}

/// `DELETE /accounts/{accountId}/roles/{id}` — soft deletes (`workflow_state
/// = 'deleted'`) and returns the role. Built-in roles cannot be deleted
/// (`400`).
pub async fn role_delete(
    db: &PgPool,
    user_id: i64,
    account_id: i64,
    role_id: i64,
) -> Result<RoleView, ApiError> {
    require_roles_admin(db, user_id, account_id).await?;

    let deleted: Option<RoleView> = sqlx::query_as(&format!(
        "UPDATE roles SET workflow_state = 'deleted', updated_at = now()
         WHERE id = $1 AND account_id = $2 AND workflow_state <> 'deleted'
         RETURNING {ROLE_COLUMNS}"
    ))
    .bind(role_id)
    .bind(account_id)
    .fetch_optional(db)
    .await?;

    match deleted {
        Some(role) => Ok(role),
        None => Err(reject_builtin_or_missing(db, account_id, role_id).await),
    }
}

/// `400` for a built-in role, `404` when the account has no such role.
async fn reject_builtin_or_missing(db: &PgPool, account_id: i64, role_id: i64) -> ApiError {
    match Role::from_builtin_role_id(role_id) {
        Some(_) => ApiError::BadRequest("built-in roles cannot be modified".to_string()),
        None => match fetch_role(db, account_id, role_id).await {
            Ok(role) if role.account_id.is_none() => {
                ApiError::BadRequest("built-in roles cannot be modified".to_string())
            }
            _ => ApiError::NotFound,
        },
    }
}

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use star_auth::AuthUser;
use star_common::AppState;

/// Canvas role-override administration routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/accounts/{accountId}/role_overrides",
            get(get_account_overrides).put(put_account_overrides),
        )
        .route(
            "/courses/{courseId}/role_overrides",
            get(get_course_overrides).put(put_course_overrides),
        )
        .route(
            "/accounts/{accountId}/roles",
            get(list_account_roles).post(create_account_role),
        )
        .route(
            "/accounts/{accountId}/roles/{id}",
            get(show_account_role)
                .put(update_account_role)
                .delete(delete_account_role),
        )
}

async fn get_account_overrides(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<RoleOverrideView>>, ApiError> {
    get_role_overrides(&state.db, auth.id, RoleOverrideContext::Account, account_id)
        .await
        .map(Json)
}

async fn put_account_overrides(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(update): Json<RoleOverridesUpdate>,
) -> Result<Json<Vec<RoleOverrideView>>, ApiError> {
    put_role_overrides(
        &state.db,
        auth.id,
        RoleOverrideContext::Account,
        account_id,
        &update,
    )
    .await
    .map(Json)
}

async fn get_course_overrides(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<RoleOverrideView>>, ApiError> {
    get_role_overrides(&state.db, auth.id, RoleOverrideContext::Course, course_id)
        .await
        .map(Json)
}

async fn put_course_overrides(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(update): Json<RoleOverridesUpdate>,
) -> Result<Json<Vec<RoleOverrideView>>, ApiError> {
    put_role_overrides(
        &state.db,
        auth.id,
        RoleOverrideContext::Course,
        course_id,
        &update,
    )
    .await
    .map(Json)
}

async fn list_account_roles(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<RoleView>>, ApiError> {
    roles_list(&state.db, auth.id, account_id).await.map(Json)
}

async fn create_account_role(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(body): Json<RoleRequest>,
) -> Result<(axum::http::StatusCode, Json<RoleView>), ApiError> {
    role_create(&state.db, auth.id, account_id, &body.into_input())
        .await
        .map(|view| (axum::http::StatusCode::CREATED, Json(view)))
}

async fn show_account_role(
    State(state): State<AppState>,
    Path((account_id, role_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<RoleView>, ApiError> {
    role_show(&state.db, auth.id, account_id, role_id)
        .await
        .map(Json)
}

async fn update_account_role(
    State(state): State<AppState>,
    Path((account_id, role_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<RoleRequest>,
) -> Result<Json<RoleView>, ApiError> {
    role_update(&state.db, auth.id, account_id, role_id, &body.into_input())
        .await
        .map(Json)
}

async fn delete_account_role(
    State(state): State<AppState>,
    Path((account_id, role_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<RoleView>, ApiError> {
    role_delete(&state.db, auth.id, account_id, role_id)
        .await
        .map(Json)
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

    #[test]
    fn role_override_context_parses_canvas_contexts() {
        assert_eq!(
            "Account".parse::<RoleOverrideContext>().unwrap(),
            RoleOverrideContext::Account
        );
        assert_eq!(
            "course".parse::<RoleOverrideContext>().unwrap(),
            RoleOverrideContext::Course
        );
        assert!("site_admin".parse::<RoleOverrideContext>().is_err());
        assert_eq!(RoleOverrideContext::Course.as_str(), "Course");
    }

    #[test]
    fn role_overrides_update_requires_role_and_permissions() {
        let mut body = update_body(&[("read_reports", change(Some(true), Some(true), Some(true)))]);
        assert_eq!(
            validate_role_overrides_update(&body).unwrap(),
            Role::Teacher
        );

        body.role_id = 999;
        assert!(validate_role_overrides_update(&body).is_err());

        body.role_id = Role::Teacher.builtin_role_id();
        body.permissions = BTreeMap::new();
        assert!(validate_role_overrides_update(&body).is_err());

        let blank = update_body(&[("   ", change(Some(true), None, None))]);
        assert!(validate_role_overrides_update(&blank).is_err());

        let empty = update_body(&[("read_reports", PermissionUpdate::default())]);
        assert!(validate_role_overrides_update(&empty).is_err());
    }

    #[test]
    fn permission_update_follows_canvas_explicit_semantics() {
        assert_eq!(
            change(Some(true), Some(false), Some(true)).enabled_change(),
            Some(true)
        );
        assert_eq!(change(Some(true), None, Some(false)).enabled_change(), None);
        assert_eq!(
            change(Some(false), None, None).enabled_change(),
            Some(false)
        );
        assert_eq!(change(None, Some(true), None).enabled_change(), None);
    }

    /// DB round trip for the Phase 3 role_overrides endpoints. Skips cleanly
    /// when DATABASE_URL is unset or the base schema has not been migrated;
    /// rows use a unique per-run prefix and are removed at the end.
    #[tokio::test]
    async fn role_overrides_put_and_get_round_trip() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            println!("SKIP: DATABASE_URL not set");
            return;
        };
        let pool = match sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
        {
            Ok(pool) => pool,
            Err(err) => {
                println!("SKIP: cannot connect to Postgres: {err}");
                return;
            }
        };

        let schema: Option<(bool,)> = sqlx::query_as(
            r#"
            SELECT to_regclass('public.users') IS NOT NULL
               AND to_regclass('public.accounts') IS NOT NULL
               AND to_regclass('public.account_users') IS NOT NULL
               AND to_regclass('public.courses') IS NOT NULL
               AND to_regclass('public.role_overrides') IS NOT NULL
            "#,
        )
        .fetch_optional(&pool)
        .await
        .expect("probe base schema");
        let Some((schema_ready,)) = schema else {
            println!("SKIP: base schema not migrated");
            return;
        };
        if !schema_ready {
            println!("SKIP: base schema not migrated");
            return;
        }

        let prefix = format!(
            "perm-p3-{}-{}",
            std::process::id(),
            Utc::now().timestamp_millis()
        );

        let admin_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO users (name, email, password_hash, role)
            VALUES ($1, $2, 'not-used', 'admin')
            RETURNING id
            "#,
        )
        .bind(format!("{prefix}-admin"))
        .bind(format!("{prefix}-admin@test.local"))
        .fetch_one(&pool)
        .await
        .expect("seed admin");

        let student_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO users (name, email, password_hash, role)
            VALUES ($1, $2, 'not-used', 'student')
            RETURNING id
            "#,
        )
        .bind(format!("{prefix}-student"))
        .bind(format!("{prefix}-student@test.local"))
        .fetch_one(&pool)
        .await
        .expect("seed student");

        let account_id: i64 = sqlx::query_scalar(
            "INSERT INTO accounts (name, workflow_state) VALUES ($1, 'active') RETURNING id",
        )
        .bind(format!("{prefix}-account"))
        .fetch_one(&pool)
        .await
        .expect("seed account");

        sqlx::query(
            "INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')",
        )
        .bind(account_id)
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("seed account admin");

        let course_id: i64 = sqlx::query_scalar(
            "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(format!("{prefix}-course"))
        .bind(format!("{prefix}-code"))
        .bind(account_id)
        .fetch_one(&pool)
        .await
        .expect("seed course");

        let body = update_body(&[
            ("read_reports", change(Some(true), Some(true), Some(true))),
            ("send_messages", change(Some(false), None, None)),
        ]);
        let rows = put_role_overrides(
            &pool,
            admin_id,
            RoleOverrideContext::Account,
            account_id,
            &body,
        )
        .await
        .expect("account put");
        assert_eq!(rows.len(), 2);
        let reports = rows
            .iter()
            .find(|row| row.permission == "read_reports")
            .expect("read_reports row");
        assert!(reports.enabled);
        assert!(reports.locked);
        assert_eq!(reports.context_type, "Account");
        assert_eq!(reports.context_id, account_id);
        assert_eq!(reports.role_id, Role::Teacher.builtin_role_id());

        // Repeating the PUT updates in place instead of duplicating the row.
        let body = update_body(&[("read_reports", change(Some(false), None, Some(true)))]);
        let rows = put_role_overrides(
            &pool,
            admin_id,
            RoleOverrideContext::Account,
            account_id,
            &body,
        )
        .await
        .expect("account re-put");
        assert_eq!(rows.len(), 2);
        let reports = rows
            .iter()
            .find(|row| row.permission == "read_reports")
            .expect("read_reports row");
        assert!(!reports.enabled);
        assert!(reports.locked, "omitted locked keeps the stored value");

        let listed = get_role_overrides(&pool, admin_id, RoleOverrideContext::Account, account_id)
            .await
            .expect("account get");
        assert_eq!(listed, rows);

        // Course-context overrides are a separate scope.
        let body = update_body(&[("read_reports", change(Some(true), None, Some(true)))]);
        let course_rows = put_role_overrides(
            &pool,
            admin_id,
            RoleOverrideContext::Course,
            course_id,
            &body,
        )
        .await
        .expect("course put");
        assert_eq!(course_rows.len(), 1);
        assert_eq!(course_rows[0].context_type, "Course");
        assert_eq!(course_rows[0].context_id, course_id);

        let denied =
            get_role_overrides(&pool, student_id, RoleOverrideContext::Account, account_id).await;
        assert!(matches!(denied, Err(ApiError::Forbidden)));

        sqlx::query(
            "DELETE FROM role_overrides WHERE (context_type = 'Account' AND context_id = $1)
                OR (context_type = 'Course' AND context_id = $2)",
        )
        .bind(account_id)
        .bind(course_id)
        .execute(&pool)
        .await
        .expect("cleanup overrides");
        sqlx::query("DELETE FROM account_users WHERE account_id = $1")
            .bind(account_id)
            .execute(&pool)
            .await
            .expect("cleanup account users");
        sqlx::query("DELETE FROM courses WHERE id = $1")
            .bind(course_id)
            .execute(&pool)
            .await
            .expect("cleanup course");
        sqlx::query("DELETE FROM accounts WHERE id = $1")
            .bind(account_id)
            .execute(&pool)
            .await
            .expect("cleanup account");
        for user_id in [admin_id, student_id] {
            sqlx::query("DELETE FROM users WHERE id = $1")
                .bind(user_id)
                .execute(&pool)
                .await
                .expect("cleanup user");
        }
    }

    #[test]
    fn role_input_validates_base_role_type_and_state() {
        let input = RoleApiInput {
            base_role_type: Some("TaEnrollment".to_string()),
            workflow_state: Some("active".to_string()),
            ..RoleApiInput::default()
        };
        assert_eq!(input.base_role().unwrap(), Some(Role::Ta));
        assert!(input.validate_workflow_state().is_ok());

        let unknown_base = RoleApiInput {
            base_role_type: Some("Wizard".to_string()),
            ..RoleApiInput::default()
        };
        assert!(matches!(
            unknown_base.base_role(),
            Err(ApiError::BadRequest(_))
        ));

        let bad_state = RoleApiInput {
            workflow_state: Some("zombie".to_string()),
            ..RoleApiInput::default()
        };
        assert!(matches!(
            bad_state.validate_workflow_state(),
            Err(ApiError::BadRequest(_))
        ));
        assert!(RoleApiInput::default().base_role().unwrap().is_none());
    }

    #[test]
    fn role_request_accepts_nested_and_flat_bodies() {
        let nested = RoleRequest::Nested {
            role: RoleApiInput {
                label: Some("Grader".to_string()),
                ..RoleApiInput::default()
            },
        };
        assert_eq!(nested.into_input().label.as_deref(), Some("Grader"));

        let flat = RoleRequest::Flat(RoleApiInput {
            name: Some("grader".to_string()),
            ..RoleApiInput::default()
        });
        assert_eq!(flat.into_input().name.as_deref(), Some("grader"));
    }

    /// DB round trip for the Phase 4 roles endpoints. Skips cleanly when
    /// DATABASE_URL is unset or the schema has not been migrated; rows use a
    /// unique per-run prefix and are removed at the end.
    #[tokio::test]
    async fn roles_api_create_show_update_delete_round_trip() {
        let Ok(database_url) = std::env::var("DATABASE_URL") else {
            println!("SKIP: DATABASE_URL not set");
            return;
        };
        let pool = match sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await
        {
            Ok(pool) => pool,
            Err(err) => {
                println!("SKIP: cannot connect to Postgres: {err}");
                return;
            }
        };

        let schema: Option<(bool,)> = sqlx::query_as(
            r#"
            SELECT to_regclass('public.users') IS NOT NULL
               AND to_regclass('public.accounts') IS NOT NULL
               AND to_regclass('public.account_users') IS NOT NULL
               AND to_regclass('public.roles') IS NOT NULL
            "#,
        )
        .fetch_optional(&pool)
        .await
        .expect("probe base schema");
        let Some((schema_ready,)) = schema else {
            println!("SKIP: base schema not migrated");
            return;
        };
        if !schema_ready {
            println!("SKIP: base schema not migrated");
            return;
        }

        let prefix = format!(
            "perm-p4-{}-{}",
            std::process::id(),
            Utc::now().timestamp_millis()
        );

        let admin_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO users (name, email, password_hash, role)
            VALUES ($1, $2, 'not-used', 'admin')
            RETURNING id
            "#,
        )
        .bind(format!("{prefix}-admin"))
        .bind(format!("{prefix}-admin@test.local"))
        .fetch_one(&pool)
        .await
        .expect("seed admin");

        let student_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO users (name, email, password_hash, role)
            VALUES ($1, $2, 'not-used', 'student')
            RETURNING id
            "#,
        )
        .bind(format!("{prefix}-student"))
        .bind(format!("{prefix}-student@test.local"))
        .fetch_one(&pool)
        .await
        .expect("seed student");

        let account_id: i64 = sqlx::query_scalar(
            "INSERT INTO accounts (name, workflow_state) VALUES ($1, 'active') RETURNING id",
        )
        .bind(format!("{prefix}-account"))
        .fetch_one(&pool)
        .await
        .expect("seed account");

        sqlx::query(
            "INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')",
        )
        .bind(account_id)
        .bind(admin_id)
        .execute(&pool)
        .await
        .expect("seed account admin");

        // A fresh account starts with no custom roles (built-ins are global).
        let listed = roles_list(&pool, admin_id, account_id)
            .await
            .expect("empty list");
        assert!(listed.iter().all(|r| r.account_id.is_none()));

        let input = RoleApiInput {
            label: Some("Grader".to_string()),
            base_role_type: Some("ta".to_string()),
            ..RoleApiInput::default()
        };
        let created = role_create(&pool, admin_id, account_id, &input)
            .await
            .expect("create role");
        assert_eq!(created.label, "Grader");
        assert_eq!(created.base_role_type, "TaEnrollment");
        assert_eq!(created.workflow_state, "active");
        assert_eq!(created.account_id, Some(account_id));

        let duplicate = role_create(&pool, admin_id, account_id, &input).await;
        assert!(matches!(duplicate, Err(ApiError::Conflict(_))));

        let blank_label = role_create(
            &pool,
            admin_id,
            account_id,
            &RoleApiInput {
                label: Some("  ".to_string()),
                ..RoleApiInput::default()
            },
        )
        .await;
        assert!(matches!(blank_label, Err(ApiError::BadRequest(_))));

        let listed = roles_list(&pool, admin_id, account_id)
            .await
            .expect("list roles");
        let custom: Vec<_> = listed
            .iter()
            .filter(|r| r.account_id == Some(account_id))
            .cloned()
            .collect();
        assert_eq!(custom, vec![created.clone()]);

        let shown = role_show(&pool, admin_id, account_id, created.id)
            .await
            .expect("show role");
        assert_eq!(shown, created);

        // Built-in roles are visible but immutable.
        let builtin = role_show(&pool, admin_id, account_id, Role::Teacher.builtin_role_id())
            .await
            .expect("show builtin role");
        assert_eq!(builtin.account_id, None);
        let builtin_update = role_update(
            &pool,
            admin_id,
            account_id,
            Role::Teacher.builtin_role_id(),
            &RoleApiInput {
                label: Some("Nope".to_string()),
                ..RoleApiInput::default()
            },
        )
        .await;
        assert!(matches!(builtin_update, Err(ApiError::BadRequest(_))));

        let updated = role_update(
            &pool,
            admin_id,
            account_id,
            created.id,
            &RoleApiInput {
                label: Some("Head Grader".to_string()),
                ..RoleApiInput::default()
            },
        )
        .await
        .expect("update role");
        assert_eq!(updated.label, "Head Grader");
        assert_eq!(updated.name, created.name);

        let missing = role_update(
            &pool,
            admin_id,
            account_id,
            99_999_999,
            &RoleApiInput::default(),
        )
        .await;
        assert!(matches!(missing, Err(ApiError::NotFound)));

        let forbidden = roles_list(&pool, student_id, account_id).await;
        assert!(matches!(forbidden, Err(ApiError::Forbidden)));

        let deleted = role_delete(&pool, admin_id, account_id, created.id)
            .await
            .expect("delete role");
        assert_eq!(deleted.workflow_state, "deleted");
        let gone = role_show(&pool, admin_id, account_id, created.id).await;
        assert!(matches!(gone, Err(ApiError::NotFound)));
        let empty = roles_list(&pool, admin_id, account_id)
            .await
            .expect("list after delete");
        assert!(empty.iter().all(|r| r.account_id.is_none()));

        let builtin_delete =
            role_delete(&pool, admin_id, account_id, Role::Teacher.builtin_role_id()).await;
        assert!(matches!(builtin_delete, Err(ApiError::BadRequest(_))));

        let missing_account = roles_list(&pool, admin_id, 99_999_999).await;
        assert!(matches!(missing_account, Err(ApiError::NotFound)));

        sqlx::query("DELETE FROM roles WHERE account_id = $1")
            .bind(account_id)
            .execute(&pool)
            .await
            .expect("cleanup roles");
        sqlx::query("DELETE FROM account_users WHERE account_id = $1")
            .bind(account_id)
            .execute(&pool)
            .await
            .expect("cleanup account users");
        sqlx::query("DELETE FROM accounts WHERE id = $1")
            .bind(account_id)
            .execute(&pool)
            .await
            .expect("cleanup account");
        for user_id in [admin_id, student_id] {
            sqlx::query("DELETE FROM users WHERE id = $1")
                .bind(user_id)
                .execute(&pool)
                .await
                .expect("cleanup user");
        }
    }

    fn change(
        enabled: Option<bool>,
        locked: Option<bool>,
        explicit: Option<bool>,
    ) -> PermissionUpdate {
        PermissionUpdate {
            enabled,
            locked,
            explicit,
        }
    }

    fn update_body(entries: &[(&str, PermissionUpdate)]) -> RoleOverridesUpdate {
        RoleOverridesUpdate {
            role_id: Role::Teacher.builtin_role_id(),
            permissions: entries
                .iter()
                .map(|(permission, change)| (permission.to_string(), change.clone()))
                .collect(),
        }
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
