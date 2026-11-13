//! Course and User Canvas field-parity additions (issue #28).
//!
//! Backs the fields added to the in-crate `Course` and `User` types defined
//! in `lib.rs` (the types stay there to keep this slice's lib.rs diff small
//! and contiguous for the hand-merge with the parallel gql-foundation
//! branches):
//!
//! - `Course.account` — Canvas `CourseType#account` (`course_type.rb` has no
//!   guard; visibility is inherited from the `course(id)` gate), backed by
//!   the `accounts` table via `courses.account_id`;
//! - `Course.permissions` — Canvas `CoursePermissionsType`
//!   (`course_permissions_type.rb`: manageGrades / sendMessages /
//!   viewAllGrades / viewAnalytics / becomeUser), computed from the
//!   star-lms role model instead of Canvas's RoleOverride engine;
//! - `Course.usersConnection` — Canvas `CourseType#users_connection`, gated
//!   on roster read rights (`read_roster`/`view_all_grades`/`manage_grades`
//!   translate to global admin or course teacher); everyone else resolves
//!   null, like Canvas — never an error;
//! - row enrichment for `User` (`sortableName` from `users.sortable_name`,
//!   `avatarUrl` from `user_profiles.avatar_url`).
//!
//! Conventions follow `assignment.rs`: relay connections with
//! `edges { cursor node }` + `pageInfo { hasNextPage endCursor }`, opaque
//! hex cursors over the id, `first` clamped to 1..=100 (default 10),
//! `LIMIT first+1` ordered by id ascending, and `db_err` error mapping.
//!
//! Canvas GraphQL orders `users_connection` by sortable name and
//! `sections_connection` by section name; this crate's connections are
//! id-ordered (cursor contract) — documented deviation, same as every
//! other star-graphql connection.
//!
//! Documented omissions (no backing columns; see the issue report):
//! Canvas `Course.term` (`courses.enrollment_term_id` does not exist),
//! `Course.sisId`, `Course.imageUrl`, `Course.applyGroupWeights`,
//! `Course.allowFinalGradeOverride`, `User.uuid`, `User.shortName`,
//! `User.pronouns`, `User.loginId`, `User.sisId`/`integrationId`.

use async_graphql::{Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{
    db_err, decode_cursor, encode_cursor, PageInfo, User, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};

// ---------------------------------------------------------------------------
// Account (subset of Canvas AccountType backing Course.account)
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Account` (subset of `app/graphql/types/account_type.rb`
/// limited to the columns of the `accounts` table). Method names are
/// camelCased by the Object derive: `id`, `name`, `state`, `createdAt`,
/// `updatedAt`.
pub struct Account {
    id: i64,
    name: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl Account {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    /// Canvas renames the DB `workflow_state` to `state` (like `Course`).
    async fn state(&self) -> &str {
        &self.workflow_state
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Row of the `accounts` table (migrations 0024/0028).
#[derive(Debug, Clone, sqlx::FromRow)]
struct AccountRow {
    id: i64,
    name: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<AccountRow> for Account {
    fn from(row: AccountRow) -> Self {
        Account {
            id: row.id,
            name: row.name,
            workflow_state: row.workflow_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Fetch one account by id. Unknown ids resolve to null, like Canvas.
pub(crate) async fn fetch_account(
    pool: &PgPool,
    id: i64,
) -> Result<Option<Account>, async_graphql::Error> {
    let row: Option<AccountRow> = sqlx::query_as(
        r#"
        SELECT id, name, workflow_state, created_at, updated_at
        FROM accounts
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(AccountRow::into))
}

// ---------------------------------------------------------------------------
// CoursePermissions (Canvas CoursePermissionsType)
// ---------------------------------------------------------------------------

/// Canvas GraphQL `CoursePermissions` (subset of
/// `app/graphql/types/course_permissions_type.rb` — every field that type
/// declares). Field names are camelCased by the SimpleObject derive:
/// `manageGrades`, `sendMessages`, `viewAllGrades`, `viewAnalytics`,
/// `becomeUser`.
///
/// Canvas resolves each flag through the RoleOverride engine; star-lms has
/// no per-course permission rows, so the values are computed from the role
/// model the REST handlers use: global admins and course teachers hold the
/// management rights, any course participant may send messages, and
/// `becomeUser` follows the account-admin chain (global admin or
/// account_users row on the course's account or an ancestor), mirroring
/// star-accounts' `can_access_account`.
#[derive(Debug, Clone, SimpleObject)]
pub struct CoursePermissions {
    pub manage_grades: bool,
    pub send_messages: bool,
    pub view_all_grades: bool,
    pub view_analytics: bool,
    pub become_user: bool,
}

// ---------------------------------------------------------------------------
// usersConnection (Canvas CourseType#users_connection)
// ---------------------------------------------------------------------------

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct UserEdge {
    pub cursor: String,
    pub node: User,
}

/// Relay connection for a course's users (Canvas `usersConnection`).
#[derive(Debug, Clone, SimpleObject)]
pub struct UserConnection {
    pub edges: Vec<UserEdge>,
    pub page_info: PageInfo,
}

/// Row of the users × enrollments × user_profiles join behind
/// `usersConnection`. `sortable_name` is coalesced onto `name` like the
/// star-users REST handlers (`COALESCE(sortable_name, name)`).
#[derive(Debug, Clone, sqlx::FromRow)]
struct UserJoinRow {
    id: i64,
    name: String,
    email: String,
    /// COALESCE(sortable_name, name) — never null, like the REST handlers.
    sortable_name: String,
    avatar_url: Option<String>,
    created_at: DateTime<Utc>,
}

impl From<UserJoinRow> for User {
    fn from(row: UserJoinRow) -> Self {
        User {
            id: row.id,
            name: row.name,
            email: row.email,
            sis_user_id: None,
            sortable_name: row.sortable_name,
            avatar_url: row.avatar_url,
            created_at: row.created_at,
        }
    }
}

/// Fetch one page of a course's roster users: active enrollments joined to
/// users, `DISTINCT ON (u.id)` so multi-enrollment users appear once,
/// `LIMIT first+1` ordered by user id ascending, optionally resuming after
/// a cursor. The extra row drives `hasNextPage`.
pub(crate) async fn fetch_user_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<UserConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let rows: Vec<UserJoinRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (u.id)
               u.id, u.name, u.email,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               up.avatar_url,
               u.created_at
        FROM users u
        JOIN enrollments e ON e.user_id = u.id
        LEFT JOIN user_profiles up ON up.user_id = u.id
        WHERE e.course_id = $1
          AND e.workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR u.id > $2)
        ORDER BY u.id
        LIMIT $3
        "#,
    )
    .bind(course_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    let mut rows = rows;
    rows.truncate(page_size);

    let edges: Vec<UserEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            UserEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(UserConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

// ---------------------------------------------------------------------------
// becomeUser helper (account-admin chain, mirroring star-accounts)
// ---------------------------------------------------------------------------

/// Whether the user is an account admin on the given account or any of its
/// ancestors — the same chain star-accounts' `can_access_account` walks.
/// Global admins are handled by the caller.
pub(crate) async fn is_account_admin_in_chain(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
) -> Result<bool, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE chain AS (
            SELECT id, parent_account_id FROM accounts WHERE id = $2
          UNION
            SELECT a.id, a.parent_account_id
            FROM accounts a JOIN chain c ON a.id = c.parent_account_id
        )
        SELECT 1
        FROM account_users au
        JOIN chain c ON c.id = au.account_id
        WHERE au.user_id = $1
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.is_some())
}
