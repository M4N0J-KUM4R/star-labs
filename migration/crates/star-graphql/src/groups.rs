//! Group, group membership and group set (category) read types +
//! `Course.groupsConnection` / `Course.groupSetsConnection`, `Group.members`
/// and `GroupSet.groups` (Canvas issue #82 domain groups half).
//!
//! Mirrors the conventions of the merged slices: camelCase field names,
//! relay-style connections, opaque cursors, `LIMIT first+1` ordered by id.
//!
//! Canvas shape references: `app/graphql/types/group_type.rb`,
//! `group_membership_type.rb` and `group_set_type.rb` in canvas-lms.
//! Fields without a backing star-lms column are omitted and documented per
//! type:
//! - `Group`: `assetString`, `sisId`, `activityStream`, `canMessage`
//!   (star-groups REST has no messaging gate); `membersCount` is a live
//!   COUNT over non-deleted memberships; `state` maps `workflow_state`
//!   (0018: available | deleted) like Canvas `GroupState`.
//! - `GroupMembership`: Canvas exposes `user`, `group`, `state`,
//!   `createdAt`, `updatedAt` (group_membership_type.rb:34-41) — all
//!   backed by 0018.
//! - `GroupSet`: `currentGroup` (no per-user current group column),
//!   `sisId`, `singleTag` (no column); `selfSignup` and `autoLeader` come
//!   from group_categories (0043); `memberLimit` maps `group_limit`.
//!
//! Authorization:
//! - `Course.groupsConnection` / `groupSetsConnection` inherit
//!   `course(id)` visibility (enrolled users, any role, and admins).
//! - `Group.membersConnection` requires the caller to be enrolled in the
//!   group's course (any role) or an admin — the group read gate of
//!   Canvas `group.rb:662` (course-level groups grant read through their
//!   context's :read), which the REST group handlers use too.
//!
//! Authentication itself is enforced before execution by the router's
//! `AuthUser` extractor (HTTP 401), so every resolver here can assume an
//! authenticated caller.

use async_graphql::{Context, Enum, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{
    db_err, decode_cursor, encode_cursor, GraphQLContext, PageInfo, DEFAULT_PAGE_SIZE,
    MAX_PAGE_SIZE,
};

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

/// Row of the `groups` table (migration 0018 + 0096).
#[derive(Debug, Clone, sqlx::FromRow)]
struct GroupRow {
    id: i64,
    course_id: Option<i64>,
    group_category_id: Option<i64>,
    name: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Row of the `group_memberships` table (migration 0018).
#[derive(Debug, Clone, sqlx::FromRow)]
struct GroupMembershipRow {
    id: i64,
    group_id: i64,
    user_id: i64,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Row of the `group_categories` table (migration 0043).
#[derive(Debug, Clone, sqlx::FromRow)]
struct GroupSetRow {
    id: i64,
    course_id: Option<i64>,
    account_id: Option<i64>,
    name: String,
    self_signup: Option<String>,
    group_limit: Option<i32>,
    auto_leader: Option<String>,
    non_collaborative: bool,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas `GroupState` enum (group_type.rb:45 references GroupStateType).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
enum GroupState {
    #[graphql(name = "available")]
    Available,
    #[graphql(name = "deleted")]
    Deleted,
}

/// Canvas `GroupMembershipState` (group_membership_type.rb:34).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
enum GroupMembershipState {
    #[graphql(name = "accepted")]
    Accepted,
    #[graphql(name = "invited")]
    Invited,
    #[graphql(name = "requested")]
    Requested,
    #[graphql(name = "rejected")]
    Rejected,
    #[graphql(name = "deleted")]
    Deleted,
}

/// Canvas `SelfSignupPolicy` (group_set_type.rb:35).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
enum SelfSignupPolicy {
    #[graphql(name = "enabled")]
    Enabled,
    #[graphql(name = "restricted")]
    Restricted,
    #[graphql(name = "disabled")]
    Disabled,
}

/// Canvas `AutoLeaderPolicy` (group_set_type.rb:57).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
enum AutoLeaderPolicy {
    #[graphql(name = "first")]
    First,
    #[graphql(name = "random")]
    Random,
}

/// Map a membership workflow_state to the Canvas enum. 0018 stores
/// accepted/deleted; Canvas's other states map onto accepted for parity of
/// the non-null enum.
fn membership_state(workflow_state: &str) -> GroupMembershipState {
    match workflow_state {
        "deleted" => GroupMembershipState::Deleted,
        "invited" => GroupMembershipState::Invited,
        "requested" => GroupMembershipState::Requested,
        "rejected" => GroupMembershipState::Rejected,
        _ => GroupMembershipState::Accepted,
    }
}

/// Canvas GraphQL `Group` (subset of `app/graphql/types/group_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `_id`, `name`,
/// `membersCount`, `state`, `groupCategory`, `membersConnection`,
/// `createdAt`, `updatedAt`. Omitted Canvas fields without backing
/// columns: see the module doc above.
#[derive(Debug, Clone)]
pub struct Group {
    id: i64,
    course_id: Option<i64>,
    group_category_id: Option<i64>,
    name: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl Group {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `Group._id`: the legacy canvas id (group_type.rb:37).
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    /// Canvas `Group.membersCount` (group_type.rb:41): live COUNT of
    /// non-deleted memberships.
    async fn members_count(&self, ctx: &Context<'_>) -> Result<Option<i64>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let (count,): (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*) FROM group_memberships
               WHERE group_id = $1 AND workflow_state != 'deleted'"#,
        )
        .bind(self.id)
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(Some(count))
    }

    /// Canvas `Group.state` renames workflow_state (group_type.rb:45).
    async fn state(&self) -> GroupState {
        match self.workflow_state.as_str() {
            "deleted" => GroupState::Deleted,
            _ => GroupState::Available,
        }
    }

    /// Canvas `Group.groupCategory` (group_type.rb:93).
    async fn group_category(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<GroupSet>, async_graphql::Error> {
        let Some(category_id) = self.group_category_id else {
            return Ok(None);
        };
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_group_set(&gctx.pool, category_id).await
    }

    /// Canvas `Group.membersConnection` (group_type.rb:52) — visible to
    /// enrolled users of the group's course (any role) and admins.
    async fn members_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<GroupMembershipConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        if let Some(course_id) = self.course_id {
            if !gctx.is_admin()
                && !user_enrolled_in_course(&gctx.pool, gctx.auth_user.id, course_id).await?
            {
                return Ok(None);
            }
        } else if !gctx.is_admin() {
            return Ok(None);
        }
        let conn =
            fetch_membership_page(&gctx.pool, self.id, first, after.as_deref()).await?;
        Ok(Some(conn))
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Canvas GraphQL `GroupMembership` (subset of
/// `app/graphql/types/group_membership_type.rb`).
#[derive(Debug, Clone)]
pub struct GroupMembership {
    id: i64,
    group_id: i64,
    user_id: i64,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl GroupMembership {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `GroupMembership._id` (group_membership_type.rb:25).
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `GroupMembership.state` renames workflow_state
    /// (group_membership_type.rb:34).
    async fn state(&self) -> GroupMembershipState {
        membership_state(&self.workflow_state)
    }

    /// Canvas `GroupMembership.user` (group_membership_type.rb:36).
    async fn user(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<crate::User>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        crate::fetch_user(&gctx.pool, self.user_id).await
    }

    /// Canvas `GroupMembership.group` (group_membership_type.rb:41).
    async fn group(&self, ctx: &Context<'_>) -> Result<Group, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_group(&gctx.pool, self.group_id)
            .await?
            .ok_or_else(|| async_graphql::Error::new("group not found"))
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Canvas GraphQL `GroupSet` (subset of
/// `app/graphql/types/group_set_type.rb`). star-lms models Canvas's
/// GroupCategory as the group_categories table (0043).
#[derive(Debug, Clone)]
pub struct GroupSet {
    id: i64,
    course_id: Option<i64>,
    account_id: Option<i64>,
    name: String,
    self_signup: Option<String>,
    group_limit: Option<i32>,
    auto_leader: Option<String>,
    non_collaborative: bool,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl GroupSet {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `GroupSet._id` (group_set_type.rb:30).
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    /// Canvas `GroupSet.memberLimit` maps group_limit (group_set_type.rb:46).
    async fn member_limit(&self) -> Option<i32> {
        self.group_limit
    }

    /// Canvas `GroupSet.selfSignup` (group_set_type.rb:51). Canvas values:
    /// enabled | restricted | null → disabled; star-lms stores the raw
    /// string (0043).
    async fn self_signup(&self) -> SelfSignupPolicy {
        match self.self_signup.as_deref() {
            Some("enabled") => SelfSignupPolicy::Enabled,
            Some("restricted") => SelfSignupPolicy::Restricted,
            _ => SelfSignupPolicy::Disabled,
        }
    }

    /// Canvas `GroupSet.autoLeader` (group_set_type.rb:64).
    async fn auto_leader(&self) -> Option<AutoLeaderPolicy> {
        match self.auto_leader.as_deref() {
            Some("first") => Some(AutoLeaderPolicy::First),
            Some("random") => Some(AutoLeaderPolicy::Random),
            _ => None,
        }
    }

    /// Canvas `GroupSet.nonCollaborative` (group_set_type.rb:68).
    async fn non_collaborative(&self) -> bool {
        self.non_collaborative
    }

    /// Canvas `GroupSet.groups` (group_set_type.rb:84) — plain list.
    async fn groups(&self, ctx: &Context<'_>) -> Result<Vec<Group>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let rows: Vec<GroupRow> = sqlx::query_as(
            r#"
            SELECT id, course_id, group_category_id, name, workflow_state, created_at, updated_at
            FROM groups
            WHERE group_category_id = $1 AND workflow_state != 'deleted'
            ORDER BY id
            "#,
        )
        .bind(self.id)
        .fetch_all(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(group_from_row).collect())
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GroupEdge {
    pub cursor: String,
    pub node: Group,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GroupConnection {
    pub edges: Vec<GroupEdge>,
    pub page_info: PageInfo,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GroupMembershipEdge {
    pub cursor: String,
    pub node: GroupMembership,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GroupMembershipConnection {
    pub edges: Vec<GroupMembershipEdge>,
    pub page_info: PageInfo,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GroupSetEdge {
    pub cursor: String,
    pub node: GroupSet,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct GroupSetConnection {
    pub edges: Vec<GroupSetEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn group_from_row(row: GroupRow) -> Group {
    Group {
        id: row.id,
        course_id: row.course_id,
        group_category_id: row.group_category_id,
        name: row.name,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

fn group_set_from_row(row: GroupSetRow) -> GroupSet {
    GroupSet {
        id: row.id,
        course_id: row.course_id,
        account_id: row.account_id,
        name: row.name,
        self_signup: row.self_signup,
        group_limit: row.group_limit,
        auto_leader: row.auto_leader,
        non_collaborative: row.non_collaborative,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

// ---------------------------------------------------------------------------
// Connection fetches
// ---------------------------------------------------------------------------

/// Fetch one page of a course's groups: `LIMIT first+1` ordered by id.
pub(crate) async fn fetch_group_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<GroupConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<GroupRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, group_category_id, name, workflow_state, created_at, updated_at
        FROM groups
        WHERE course_id = $1
          AND workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
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
    rows.truncate(page_size);

    let edges: Vec<GroupEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            GroupEdge {
                cursor,
                node: group_from_row(row),
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(GroupConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch one page of a group's memberships: `LIMIT first+1` ordered by id.
async fn fetch_membership_page(
    pool: &PgPool,
    group_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<GroupMembershipConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<GroupMembershipRow> = sqlx::query_as(
        r#"
        SELECT id, group_id, user_id, workflow_state, created_at, updated_at
        FROM group_memberships
        WHERE group_id = $1
          AND workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(group_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<GroupMembershipEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            GroupMembershipEdge {
                cursor,
                node: GroupMembership {
                    id: row.id,
                    group_id: row.group_id,
                    user_id: row.user_id,
                    workflow_state: row.workflow_state,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(GroupMembershipConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch one page of a course's group sets: `LIMIT first+1` ordered by id.
pub(crate) async fn fetch_group_set_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<GroupSetConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<GroupSetRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, account_id, name, self_signup, group_limit,
               auto_leader, non_collaborative, workflow_state, created_at, updated_at
        FROM group_categories
        WHERE course_id = $1
          AND workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
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
    rows.truncate(page_size);

    let edges: Vec<GroupSetEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            GroupSetEdge {
                cursor,
                node: group_set_from_row(row),
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(GroupSetConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch a single group by id, deleted groups excluded.
async fn fetch_group(
    pool: &PgPool,
    group_id: i64,
) -> Result<Option<Group>, async_graphql::Error> {
    let row: Option<GroupRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, group_category_id, name, workflow_state, created_at, updated_at
        FROM groups
        WHERE id = $1 AND workflow_state != 'deleted'
        "#,
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(group_from_row))
}

/// Fetch a single group set by id, deleted categories excluded.
async fn fetch_group_set(
    pool: &PgPool,
    category_id: i64,
) -> Result<Option<GroupSet>, async_graphql::Error> {
    let row: Option<GroupSetRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, account_id, name, self_signup, group_limit,
               auto_leader, non_collaborative, workflow_state, created_at, updated_at
        FROM group_categories
        WHERE id = $1 AND workflow_state != 'deleted'
        "#,
    )
    .bind(category_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(group_set_from_row))
}

// ---------------------------------------------------------------------------
// Permission helper (copied from lib.rs to keep the slice self-contained)
// ---------------------------------------------------------------------------

async fn user_enrolled_in_course(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<bool, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND workflow_state != 'deleted'
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.is_some())
}
