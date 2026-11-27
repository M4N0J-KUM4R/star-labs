//! Module and module item read types + `Course.modulesConnection` and the
//! root `moduleItem(id)` query (Canvas issue #79 domain).
//!
//! Mirrors the conventions of the merged slices (`lib.rs`, `quiz.rs`,
//! `terms_sections.rs`): camelCase field names via the `#[Object]` derive,
//! relay-style connections with `edges { cursor node }` + `pageInfo`
//! `{ hasNextPage endCursor }`, opaque cursors over the row id,
//! `LIMIT first+1` ordered by id.
//!
//! Canvas shape references: `app/graphql/types/module_type.rb` and
//! `module_item_type.rb` in canvas-lms. Fields without a backing star-lms
//! column are omitted and documented per type:
//! - `Module`: `prerequisites` (modules table has no prerequisite column),
//!   `completionRequirements` / `requirementCount` (module_items has no
//!   requirement columns), `requireSequentialProgress`, `unlockAt`,
//!   `estimatedDuration`, `submissionStatistics`, `progression`,
//!   `hasActiveOverrides` — none have columns in star-db
//!   (0001/0012/0065); `published` maps to `workflow_state == 'active'`
//!   like the REST module handlers.
//! - `ModuleItem`: `content` (the ModuleItemInterface union), `url` /
//!   `moduleItemUrl` (no external_url column), `next` / `previous` +
//!   their connections, `masterCourseRestrictions`, `masteryPaths`,
//!   `estimatedDuration` — no columns; `published` maps to
//!   `workflow_state == 'active'` (0065 states: active | unpublished |
//!   deleted); `title` falls back to the linked page/assignment title via
//!   LEFT JOINs, like Canvas's `ContentTag#title` fallback.
//!
//! Authorization:
//! - `Course.modulesConnection` inherits `course(id)` visibility (enrolled
//!   users of the course, any role, and admins) — same gate as
//!   `assignmentsConnection` / `quizzesConnection`.
//! - `moduleItem(id)` requires the caller to resolve the item's module →
//!   course chain and be enrolled there (any role) or be an admin; unknown
//!   ids resolve to null, like Canvas's GraphQLNodeLoader behavior.
//!
//! Authentication itself is enforced before execution by the router's
//! `AuthUser` extractor (HTTP 401), so every resolver here can assume an
//! authenticated caller.

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{
    db_err, decode_cursor, encode_cursor, GraphQLContext, PageInfo, DEFAULT_PAGE_SIZE,
    MAX_PAGE_SIZE,
};

// ---------------------------------------------------------------------------
// Database rows (runtime sqlx::query_as + FromRow, like every other crate)
// ---------------------------------------------------------------------------

/// Row of the `modules` table (migrations 0001 + 0012).
#[derive(Debug, Clone, sqlx::FromRow)]
struct ModuleRow {
    id: i64,
    name: String,
    position: i32,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Row of the `module_items` table (migrations 0002 + 0065), with the
/// linked content titles resolved by LEFT JOIN like Canvas's
/// `ContentTag#title` fallback chain.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ModuleItemRow {
    id: i64,
    module_id: i64,
    position: i32,
    title: Option<String>,
    indent: i32,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// SQL fragment resolving a module item's display title: the explicit
/// module_items.title override, else the linked page or assignment title.
const MODULE_ITEM_TITLE_SQL: &str = "COALESCE(mi.title, p.title, a.title) AS title";

/// SQL fragment LEFT JOINing the linked content for the title fallback.
const MODULE_ITEM_JOIN_SQL: &str = "FROM module_items mi \
     LEFT JOIN pages p ON mi.item_type = 'Page' AND p.id = mi.item_id \
     LEFT JOIN assignments a ON mi.item_type = 'Assignment' AND a.id = mi.item_id";

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Module` (subset of `app/graphql/types/module_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `_id`, `name`,
/// `position`, `published`, `moduleItems`, `moduleItemsConnection`,
/// `moduleItemsTotalCount`, `createdAt`, `updatedAt`. Omitted Canvas fields
/// without backing columns: see the module doc above.
#[derive(Debug, Clone)]
pub struct Module {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) position: i32,
    pub(crate) workflow_state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl Module {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `Module._id`: the legacy canvas id (module_type.rb:119).
    #[graphql(name = "_id")]
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn position(&self) -> i32 {
        self.position
    }

    /// Canvas `Module.published` is `published?` (module_type.rb:142);
    /// star-lms stores publish state in `workflow_state` ('active' is
    /// published), like the REST module handlers.
    async fn published(&self) -> bool {
        self.workflow_state == "active"
    }

    async fn module_items(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Option<Vec<ModuleItem>>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let items = fetch_module_items(&gctx.pool, self.id, None, None).await?;
        Ok(Some(items.into_iter().map(|edge| edge.node).collect()))
    }

    async fn module_items_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<ModuleItemConnection, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_module_item_page(&gctx.pool, self.id, first, after.as_deref()).await
    }

    async fn module_items_total_count(
        &self,
        ctx: &Context<'_>,
    ) -> Result<i64, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let (count,): (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*) FROM module_items
               WHERE module_id = $1 AND workflow_state != 'deleted'"#,
        )
        .bind(self.id)
        .fetch_one(&gctx.pool)
        .await
        .map_err(db_err)?;
        Ok(count)
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Canvas GraphQL `ModuleItem` (subset of
/// `app/graphql/types/module_item_type.rb`). Method names are camelCased by
/// the Object derive: `id`, `_id`, `module`, `title`, `indent`, `position`,
/// `published`, `createdAt`, `updatedAt`. Omitted Canvas fields without
/// backing columns: see the module doc above.
#[derive(Debug, Clone)]
pub struct ModuleItem {
    pub(crate) id: i64,
    pub(crate) module_id: i64,
    pub(crate) title: Option<String>,
    pub(crate) indent: i32,
    pub(crate) position: i32,
    pub(crate) workflow_state: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
}

#[Object]
impl ModuleItem {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `ModuleItem._id`: the legacy canvas id (module_item_type.rb:54).
    #[graphql(name = "_id")]
    async fn _id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `ModuleItem.module` (module_item_type.rb:56).
    async fn module(&self, ctx: &Context<'_>) -> Result<Option<Module>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_module(&gctx.pool, self.module_id).await
    }

    /// Canvas `ModuleItem.title` with the ContentTag title fallback
    /// (explicit title, else the linked content title).
    async fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    async fn indent(&self) -> i32 {
        self.indent
    }

    async fn position(&self) -> i32 {
        self.position
    }

    /// Canvas `ModuleItem.published` (module_item_type.rb:76); star-lms
    /// stores publish state in `workflow_state` (0065).
    async fn published(&self) -> bool {
        self.workflow_state == "active"
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleEdge {
    pub cursor: String,
    pub node: Module,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleConnection {
    pub edges: Vec<ModuleEdge>,
    pub page_info: PageInfo,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleItemEdge {
    pub cursor: String,
    pub node: ModuleItem,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct ModuleItemConnection {
    pub edges: Vec<ModuleItemEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetches (same pagination shape as fetch_quiz_page)
// ---------------------------------------------------------------------------

/// Fetch one page of a course's modules: `LIMIT first+1` ordered by id
/// ascending, optionally resuming after a cursor. The extra row drives
/// `hasNextPage`. Deleted modules are excluded, like the REST index.
pub(crate) async fn fetch_module_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<ModuleConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<ModuleRow> = sqlx::query_as(
        r#"
        SELECT id, name, position, workflow_state, created_at, updated_at
        FROM modules
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

    let edges: Vec<ModuleEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            ModuleEdge {
                cursor,
                node: Module {
                    id: row.id,
                    name: row.name,
                    position: row.position,
                    workflow_state: row.workflow_state,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(ModuleConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch one page of a module's items (all of them when `first` is None —
/// the plain `moduleItems` field is unpaginated in Canvas).
async fn fetch_module_items(
    pool: &PgPool,
    module_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<Vec<ModuleItemEdge>, async_graphql::Error> {
    let page_size = match first {
        None => usize::MAX,
        Some(n) => n.clamp(1, MAX_PAGE_SIZE) as usize,
    };
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let rows: Vec<ModuleItemRow> = sqlx::query_as(&format!(
        r#"
        SELECT mi.id, mi.module_id, mi.position,
               mi.indent, mi.workflow_state, mi.created_at, mi.updated_at,
               {MODULE_ITEM_TITLE_SQL}
        {MODULE_ITEM_JOIN_SQL}
        WHERE mi.module_id = $1
          AND mi.workflow_state != 'deleted'
          AND ($2::bigint IS NULL OR mi.id > $2)
        ORDER BY mi.id
        LIMIT $3
        "#
    ))
    .bind(module_id)
    .bind(after_id)
    .bind(page_size.saturating_add(1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let edges: Vec<ModuleItemEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            ModuleItemEdge {
                cursor,
                node: ModuleItem {
                    id: row.id,
                    module_id: row.module_id,
                    title: row.title,
                    indent: row.indent,
                    position: row.position,
                    workflow_state: row.workflow_state,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
            }
        })
        .collect();
    Ok(edges)
}

/// Paginated variant driving `Module.moduleItemsConnection` (first+1 read).
async fn fetch_module_item_page(
    pool: &PgPool,
    module_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<ModuleItemConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let mut edges = fetch_module_items(pool, module_id, Some(page_size as i32), after).await?;
    // fetch_module_items read first+1 rows worth of headroom via LIMIT;
    // recompute has_next_page from the raw row count.
    let has_next_page = edges.len() > page_size;
    edges.truncate(page_size);
    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(ModuleItemConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch a single module by id (used by `ModuleItem.module`).
pub(crate) async fn fetch_module(
    pool: &PgPool,
    module_id: i64,
) -> Result<Option<Module>, async_graphql::Error> {
    let row: Option<ModuleRow> = sqlx::query_as(
        r#"
        SELECT id, name, position, workflow_state, created_at, updated_at
        FROM modules
        WHERE id = $1 AND workflow_state != 'deleted'
        "#,
    )
    .bind(module_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| Module {
        id: row.id,
        name: row.name,
        position: row.position,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

/// `moduleItem(id: ID!)` — resolve the item, then require the caller to be
/// enrolled in the item's course (any role) or an admin; null otherwise.
pub(crate) async fn fetch_module_item(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    item_id: i64,
) -> Result<Option<ModuleItem>, async_graphql::Error> {
    let row: Option<ModuleItemRow> = sqlx::query_as(&format!(
        r#"
        SELECT mi.id, mi.module_id, mi.position,
               mi.indent, mi.workflow_state, mi.created_at, mi.updated_at,
               {MODULE_ITEM_TITLE_SQL}
        {MODULE_ITEM_JOIN_SQL}
        WHERE mi.id = $1 AND mi.workflow_state != 'deleted'
        "#
    ))
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Ok(None);
    };

    // Visibility flows through the module's course.
    let course: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM modules WHERE id = $1 AND workflow_state != 'deleted'",
    )
    .bind(row.module_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    let Some((course_id,)) = course else {
        return Ok(None);
    };

    if !is_admin && !user_enrolled_in_course(pool, user_id, course_id).await? {
        return Ok(None);
    }

    Ok(Some(ModuleItem {
        id: row.id,
        module_id: row.module_id,
        title: row.title,
        indent: row.indent,
        position: row.position,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

// ---------------------------------------------------------------------------
// Permission helper (copied from lib.rs to keep the slice self-contained,
// like terms_sections.rs)
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

// ---------------------------------------------------------------------------
// Single-record fetch (Canvas `Query.node` / `Query.legacyNode`)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Single-record fetch (Canvas `Query.node` / `Query.legacyNode`)
// ---------------------------------------------------------------------------

/// Canvas `Query.node(type: ModuleItem)` — a module item by legacy id.
///
/// Unlike [`fetch_module_item`] this does not apply the course visibility
/// gate; the `Node` resolver applies the permission check itself.
pub(crate) async fn fetch_module_item_unchecked(
    pool: &PgPool,
    id: i64,
) -> Result<Option<ModuleItem>, async_graphql::Error> {
    let row: Option<ModuleItemRow> = sqlx::query_as(&format!(
        r#"
        SELECT mi.id, mi.module_id, mi.position,
               mi.indent, mi.workflow_state, mi.created_at, mi.updated_at,
               {MODULE_ITEM_TITLE_SQL}
        {MODULE_ITEM_JOIN_SQL}
        WHERE mi.id = $1
        "#
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| ModuleItem {
        id: row.id,
        module_id: row.module_id,
        title: row.title,
        indent: row.indent,
        position: row.position,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}
