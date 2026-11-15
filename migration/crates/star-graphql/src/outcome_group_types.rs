//! Outcome + Group read types and connections (issue #81).
//!
//! Mirrors the conventions of the merged foundation slice in `lib.rs`:
//! camelCase field names (`displayName`, `vendorGuid`, `joinLevel`),
//! relay-style connections with `edges { cursor node }` and
//! `pageInfo { hasNextPage endCursor }`, opaque hex cursors over the id,
//! `LIMIT first+1` ordered by id ascending.
//!
//! Canvas shape references: `app/graphql/types/learning_outcome_type.rb`,
//! `learning_outcome_group_type.rb`, `group_type.rb`, and
//! `group_membership_type.rb` in canvas-lms.
//!
//! The Outcome types read the same tables the star-outcomes REST handlers
//! serve (`outcome_groups`/`outcome_links` from 0044/0081, plus the
//! `learning_outcomes`/`learning_outcome_results` payload tables from 0022);
//! the Group types read `groups`/`group_memberships` (0018) with
//! `group_categories` (0043/0096) for the `groupCategoryId` field.
//!
//! Authorization (REST parity):
//! - `Course.outcomesConnection` / `Course.groupsConnection` inherit the
//!   `course(id)` gate (enrolled users of any role, or admins).
//! - `Outcome.results` — read-only `LearningOutcomeResultConnection`:
//!     teachers in the owning course and admins see every row, other users
//!     see only their own rows (like `GET /outcomes/{id}/results`).
//! - `OutcomeGroup.childGroupsConnection` / `outcomesConnection` inherit the
//!   owning course's gate (account/global groups resolve only for admins).
//! - `Group.membersConnection` — teacher/admin-gated per the star-groups
//!   `GET /groups/{id}/memberships` handler; null for everyone else.
//! - `Group.membersCount` counts active (`!= 'deleted'`) memberships and is
//!   readable wherever the group itself is (Canvas `members_count`).

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, decode_cursor, encode_cursor, PageInfo, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

// ---------------------------------------------------------------------------
// Database rows (runtime sqlx::query_as + FromRow, like every other crate)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct OutcomeRow {
    id: i64,
    title: String,
    description: Option<String>,
    display_name: Option<String>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct OutcomeResultRow {
    id: i64,
    outcome_id: i64,
    user_id: i64,
    score: f64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct OutcomeGroupRow {
    id: i64,
    course_id: Option<i64>,
    context_type: String,
    context_id: Option<i64>,
    parent_outcome_group_id: Option<i64>,
    title: String,
    description: Option<String>,
    vendor_guid: Option<String>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct GroupRow {
    id: i64,
    course_id: Option<i64>,
    name: String,
    description: Option<String>,
    workflow_state: String,
    join_level: String,
    group_category_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Membership row joined with the member's user columns (Canvas
/// `GroupMembership.user`), like the star-groups `MembershipWithUser` query.
#[derive(Debug, Clone, sqlx::FromRow)]
struct MembershipRow {
    id: i64,
    group_id: i64,
    user_id: i64,
    user_name: String,
    user_email: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types: Outcome + results connection
// ---------------------------------------------------------------------------

/// Canvas GraphQL `LearningOutcome` (subset of
/// `app/graphql/types/learning_outcome_type.rb`). Method names are
/// camelCased by the Object derive: `id`, `title`, `displayName`,
/// `description`, `vendorGuid` (null — no column), `state`, `createdAt`,
/// `updatedAt`, `results`.
#[derive(Debug, Clone)]
pub struct Outcome {
    id: i64,
    title: String,
    description: Option<String>,
    display_name: Option<String>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl Outcome {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn title(&self) -> &str {
        &self.title
    }

    async fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Canvas exposes `vendorGuid`; star-lms has no `vendor_guid` column on
    /// `learning_outcomes`, so this is always null (shape parity).
    async fn vendor_guid(&self) -> Option<&str> {
        None
    }

    /// Canvas renames the DB `workflow_state` to `state`.
    async fn state(&self) -> &str {
        &self.workflow_state
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Read-only results connection (`learning_outcome_results`). Teachers in
    /// the owning course and admins see every row; everyone else sees only
    /// their own — parity with `GET /outcomes/{outcomeId}/results`.
    async fn results(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<LearningOutcomeResultConnection, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;

        let viewer_sees_all = if gctx.is_admin() {
            true
        } else {
            match outcome_course_id(&gctx.pool, self.id).await? {
                Some(course_id) => {
                    user_teaches_course(&gctx.pool, gctx.auth_user.id, course_id).await?
                }
                None => false,
            }
        };

        fetch_outcome_result_page(
            &gctx.pool,
            self.id,
            if viewer_sees_all {
                None
            } else {
                Some(gctx.auth_user.id)
            },
            first,
            after.as_deref(),
        )
        .await
    }
}

/// Canvas GraphQL `LearningOutcomeResult` — a user's recorded score on an
/// outcome. Field names are camelCased by the SimpleObject derive: `id`,
/// `outcomeId`, `userId`, `score`, `createdAt`, `updatedAt`.
#[derive(Debug, Clone, SimpleObject)]
pub struct LearningOutcomeResult {
    pub id: ID,
    pub outcome_id: ID,
    pub user_id: ID,
    pub score: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<OutcomeResultRow> for LearningOutcomeResult {
    fn from(row: OutcomeResultRow) -> Self {
        LearningOutcomeResult {
            id: ID(row.id.to_string()),
            outcome_id: ID(row.outcome_id.to_string()),
            user_id: ID(row.user_id.to_string()),
            score: row.score,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct LearningOutcomeResultEdge {
    pub cursor: String,
    pub node: LearningOutcomeResult,
}

/// Relay connection for outcome results.
#[derive(Debug, Clone, SimpleObject)]
pub struct LearningOutcomeResultConnection {
    pub edges: Vec<LearningOutcomeResultEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// GraphQL types: OutcomeGroup + child/outcome connections
// ---------------------------------------------------------------------------

/// Canvas GraphQL `LearningOutcomeGroup` (subset of
/// `app/graphql/types/learning_outcome_group_type.rb`). Method names are
/// camelCased by the Object derive: `id`, `contextType`, `contextId`,
/// `parentOutcomeGroupId`, `title`, `description`, `vendorGuid`, `state`,
/// `createdAt`, `updatedAt`, `childGroupsConnection`, `outcomesConnection`.
#[derive(Debug, Clone)]
pub struct OutcomeGroup {
    id: i64,
    course_id: Option<i64>,
    context_type: String,
    context_id: Option<i64>,
    parent_outcome_group_id: Option<i64>,
    title: String,
    description: Option<String>,
    vendor_guid: Option<String>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl OutcomeGroup {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn context_type(&self) -> &str {
        &self.context_type
    }

    async fn context_id(&self) -> Option<ID> {
        self.context_id.map(|id| ID(id.to_string()))
    }

    async fn parent_outcome_group_id(&self) -> Option<ID> {
        self.parent_outcome_group_id.map(|id| ID(id.to_string()))
    }

    async fn title(&self) -> &str {
        &self.title
    }

    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    async fn vendor_guid(&self) -> Option<&str> {
        self.vendor_guid.as_deref()
    }

    /// Canvas renames the DB `workflow_state` to `state`.
    async fn state(&self) -> &str {
        &self.workflow_state
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Immediate active child groups (`#subgroups` parity: filters on
    /// `parent_outcome_group_id` and active state), cursor-paginated by id.
    async fn child_groups_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<OutcomeGroupConnection>, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        Ok(Some(
            fetch_child_outcome_group_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    /// Active outcomes linked into this group via `outcome_links`, ordered
    /// by outcome id (the REST `GET /outcome_groups/{id}/outcomes` shape).
    async fn outcomes_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<OutcomeConnection>, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        Ok(Some(
            fetch_group_outcome_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }
}

impl From<OutcomeGroupRow> for OutcomeGroup {
    fn from(row: OutcomeGroupRow) -> Self {
        OutcomeGroup {
            id: row.id,
            course_id: row.course_id,
            context_type: row.context_type,
            context_id: row.context_id,
            parent_outcome_group_id: row.parent_outcome_group_id,
            title: row.title,
            description: row.description,
            vendor_guid: row.vendor_guid,
            workflow_state: row.workflow_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Relay connection edge for outcome groups.
#[derive(Debug, Clone, SimpleObject)]
pub struct OutcomeGroupEdge {
    pub cursor: String,
    pub node: OutcomeGroup,
}

/// Relay connection for outcome groups.
#[derive(Debug, Clone, SimpleObject)]
pub struct OutcomeGroupConnection {
    pub edges: Vec<OutcomeGroupEdge>,
    pub page_info: PageInfo,
}

/// Relay connection edge for outcomes.
#[derive(Debug, Clone, SimpleObject)]
pub struct OutcomeEdge {
    pub cursor: String,
    pub node: Outcome,
}

/// Relay connection for outcomes.
#[derive(Debug, Clone, SimpleObject)]
pub struct OutcomeConnection {
    pub edges: Vec<OutcomeEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// GraphQL types: Group + memberships connection + GroupMembership
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Group` (subset of `app/graphql/types/group_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `courseId`,
/// `name`, `description`, `joinLevel`, `groupCategoryId`, `state`,
/// `createdAt`, `updatedAt`, `membersConnection`, `membersCount`.
#[derive(Debug, Clone)]
pub struct Group {
    id: i64,
    course_id: Option<i64>,
    name: String,
    description: Option<String>,
    workflow_state: String,
    join_level: String,
    group_category_id: Option<i64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl Group {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn course_id(&self) -> Option<ID> {
        self.course_id.map(|id| ID(id.to_string()))
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    async fn join_level(&self) -> &str {
        &self.join_level
    }

    async fn group_category_id(&self) -> Option<ID> {
        self.group_category_id.map(|id| ID(id.to_string()))
    }

    /// Canvas renames the DB `workflow_state` to `state`.
    async fn state(&self) -> &str {
        &self.workflow_state
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Canvas gates `membersConnection` on `read_roster`; star-groups serves
    /// `GET /groups/{groupId}/memberships` to teachers/admins only, so the
    /// connection resolves to null for everyone else (REST parity).
    async fn members_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<GroupMembershipConnection>, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;

        let allowed = if gctx.is_admin() {
            true
        } else {
            match self.course_id {
                Some(course_id) => {
                    user_teaches_course(&gctx.pool, gctx.auth_user.id, course_id).await?
                }
                None => false,
            }
        };

        if !allowed {
            return Ok(None);
        }

        Ok(Some(
            fetch_group_membership_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    /// Count of active (`!= 'deleted'`) memberships. Canvas's `members_count`
    /// is readable wherever the group is; no separate gate.
    async fn members_count(&self, ctx: &Context<'_>) -> Result<i64, async_graphql::Error> {
        let gctx = ctx.data::<crate::GraphQLContext>()?;
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*) FROM group_memberships
            WHERE group_id = $1 AND workflow_state != 'deleted'
            "#,
        )
        .bind(self.id)
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(count)
    }
}

/// Canvas GraphQL `GroupMembership` (subset of
/// `app/graphql/types/group_membership_type.rb`) with the embedded `user`.
/// Field names are camelCased by the SimpleObject derive: `id`, `groupId`,
/// `userId`, `state`, `createdAt`, `updatedAt`, `user`.
#[derive(Debug, Clone, SimpleObject)]
pub struct GroupMembership {
    pub id: ID,
    pub group_id: ID,
    pub user_id: ID,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Minimal Canvas `User` shape of the member (like `user_json` in
    /// star-groups: id, name, email).
    pub user: Option<GroupMembershipUser>,
}

/// Embedded member user shape (`GroupMembership.user` in Canvas).
#[derive(Debug, Clone, SimpleObject)]
pub struct GroupMembershipUser {
    pub id: ID,
    pub name: String,
    pub email: String,
}

impl From<MembershipRow> for GroupMembership {
    fn from(row: MembershipRow) -> Self {
        GroupMembership {
            id: ID(row.id.to_string()),
            group_id: ID(row.group_id.to_string()),
            user_id: ID(row.user_id.to_string()),
            state: row.workflow_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
            user: Some(GroupMembershipUser {
                id: ID(row.user_id.to_string()),
                name: row.user_name,
                email: row.user_email,
            }),
        }
    }
}

/// Relay connection edge for group memberships.
#[derive(Debug, Clone, SimpleObject)]
pub struct GroupMembershipEdge {
    pub cursor: String,
    pub node: GroupMembership,
}

/// Relay connection for group memberships.
#[derive(Debug, Clone, SimpleObject)]
pub struct GroupMembershipConnection {
    pub edges: Vec<GroupMembershipEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Entry points called from the Course resolvers in lib.rs
// ---------------------------------------------------------------------------

/// `Course.outcomesConnection` — every active outcome linked into the course
/// through active `outcome_links` + active `outcome_groups` rows of the
/// course (same join as the star-outcomes `GET /courses/{id}/outcomes`
/// handler), cursor-paginated by outcome id.
pub(crate) async fn fetch_course_outcome_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<OutcomeConnection, async_graphql::Error> {
    let page_size = page_size_of(first);
    let after_id = decode_after(after)?;

    let mut rows: Vec<OutcomeRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT lo.id, lo.title, lo.description, lo.display_name,
               lo.workflow_state, lo.created_at, lo.updated_at
        FROM learning_outcomes lo
        JOIN outcome_links ol ON ol.outcome_id = lo.id
        JOIN outcome_groups og ON og.id = ol.outcome_group_id
        WHERE og.course_id = $1
          AND og.workflow_state = 'active'
          AND ol.workflow_state = 'active'
          AND lo.workflow_state = 'active'
          AND ($2::bigint IS NULL OR lo.id > $2)
        ORDER BY lo.id
        LIMIT $3
        "#,
    )
    .bind(course_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(finish_outcome_page(rows, page_size))
}

/// `Course.groupsConnection` — the course's non-deleted groups
/// (`GET /courses/{id}/groups` parity), cursor-paginated by group id.
pub(crate) async fn fetch_course_group_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<GroupConnection, async_graphql::Error> {
    let page_size = page_size_of(first);
    let after_id = decode_after(after)?;

    let mut rows: Vec<GroupRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, description, workflow_state, join_level,
               group_category_id, created_at, updated_at
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
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(GroupConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

// ---------------------------------------------------------------------------
// Connection fetches (same pagination shape as fetch_enrollment_page)
// ---------------------------------------------------------------------------

/// One page of an outcome's results, optionally scoped to one user.
async fn fetch_outcome_result_page(
    pool: &PgPool,
    outcome_id: i64,
    user_id: Option<i64>,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<LearningOutcomeResultConnection, async_graphql::Error> {
    let page_size = page_size_of(first);
    let after_id = decode_after(after)?;

    let mut rows: Vec<OutcomeResultRow> = sqlx::query_as(
        r#"
        SELECT id, outcome_id, user_id, score, created_at, updated_at
        FROM learning_outcome_results
        WHERE outcome_id = $1
          AND ($2::bigint IS NULL OR user_id = $2)
          AND ($3::bigint IS NULL OR id > $3)
        ORDER BY id
        LIMIT $4
        "#,
    )
    .bind(outcome_id)
    .bind(user_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<LearningOutcomeResultEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            LearningOutcomeResultEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(LearningOutcomeResultConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// One page of an outcome group's immediate active child groups.
async fn fetch_child_outcome_group_page(
    pool: &PgPool,
    parent_group_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<OutcomeGroupConnection, async_graphql::Error> {
    let page_size = page_size_of(first);
    let after_id = decode_after(after)?;

    let mut rows: Vec<OutcomeGroupRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, context_type, context_id, parent_outcome_group_id,
               title, description, vendor_guid, workflow_state, created_at, updated_at
        FROM outcome_groups
        WHERE parent_outcome_group_id = $1
          AND workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(parent_group_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<OutcomeGroupEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            OutcomeGroupEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(OutcomeGroupConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// One page of the active outcomes linked into an outcome group.
async fn fetch_group_outcome_page(
    pool: &PgPool,
    outcome_group_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<OutcomeConnection, async_graphql::Error> {
    let page_size = page_size_of(first);
    let after_id = decode_after(after)?;

    let mut rows: Vec<OutcomeRow> = sqlx::query_as(
        r#"
        SELECT lo.id, lo.title, lo.description, lo.display_name,
               lo.workflow_state, lo.created_at, lo.updated_at
        FROM learning_outcomes lo
        JOIN outcome_links ol ON ol.outcome_id = lo.id
        WHERE ol.outcome_group_id = $1
          AND ol.workflow_state = 'active'
          AND lo.workflow_state = 'active'
          AND ($2::bigint IS NULL OR lo.id > $2)
        ORDER BY lo.id
        LIMIT $3
        "#,
    )
    .bind(outcome_group_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(finish_outcome_page(rows, page_size))
}

/// One page of a group's active memberships joined with the member user.
async fn fetch_group_membership_page(
    pool: &PgPool,
    group_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<GroupMembershipConnection, async_graphql::Error> {
    let page_size = page_size_of(first);
    let after_id = decode_after(after)?;

    let mut rows: Vec<MembershipRow> = sqlx::query_as(
        r#"
        SELECT gm.id, gm.group_id, gm.user_id,
               u.name AS user_name, u.email AS user_email,
               gm.workflow_state, gm.created_at, gm.updated_at
        FROM group_memberships gm
        JOIN users u ON u.id = gm.user_id
        WHERE gm.group_id = $1
          AND gm.workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR gm.id > $2)
        ORDER BY gm.id
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
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(GroupMembershipConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Relay connection edge for groups.
#[derive(Debug, Clone, SimpleObject)]
pub struct GroupEdge {
    pub cursor: String,
    pub node: Group,
}

/// Relay connection for groups.
#[derive(Debug, Clone, SimpleObject)]
pub struct GroupConnection {
    pub edges: Vec<GroupEdge>,
    pub page_info: PageInfo,
}

impl From<GroupRow> for Group {
    fn from(row: GroupRow) -> Self {
        Group {
            id: row.id,
            course_id: row.course_id,
            name: row.name,
            description: row.description,
            workflow_state: row.workflow_state,
            join_level: row.join_level,
            group_category_id: row.group_category_id,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

// ---------------------------------------------------------------------------
// Shared pagination helpers
// ---------------------------------------------------------------------------

/// Clamp a `first` argument the same way every other connection does.
fn page_size_of(first: Option<i32>) -> usize {
    first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize
}

/// Decode an `after` cursor into a row id.
fn decode_after(after: Option<&str>) -> Result<Option<i64>, async_graphql::Error> {
    match after {
        None => Ok(None),
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    }
}

/// Truncate a fetched `first+1` page and build the outcome connection.
fn finish_outcome_page(mut rows: Vec<OutcomeRow>, page_size: usize) -> OutcomeConnection {
    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<OutcomeEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            OutcomeEdge {
                cursor,
                node: Outcome {
                    id: row.id,
                    title: row.title,
                    description: row.description,
                    display_name: row.display_name,
                    workflow_state: row.workflow_state,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    OutcomeConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    }
}

// ---------------------------------------------------------------------------
// Permission helpers (private copies from lib.rs / star-outcomes, to keep
// the lib.rs diff of this slice minimal across the parallel gql-foundation
// branches — wave-1 decision, kept)
// ---------------------------------------------------------------------------

/// The course an outcome is linked into (via active links + active groups),
/// or None for global outcomes. Same join as the star-outcomes results
/// handlers.
async fn outcome_course_id(
    pool: &PgPool,
    outcome_id: i64,
) -> Result<Option<i64>, async_graphql::Error> {
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        r#"
        SELECT og.course_id
        FROM learning_outcomes lo
        JOIN outcome_links ol ON ol.outcome_id = lo.id
        JOIN outcome_groups og ON og.id = ol.outcome_group_id
        WHERE lo.id = $1 AND lo.workflow_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(outcome_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.map(|(course_id,)| course_id))
}

async fn user_teaches_course(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
) -> Result<bool, async_graphql::Error> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
             AND workflow_state != 'deleted'
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    Ok(row.is_some())
}
