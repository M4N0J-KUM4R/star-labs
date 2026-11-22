//! AssignmentGroup object type + `Course.assignmentGroupsConnection`
//! (GraphQL parity gap completion).
//!
//! Mirrors Canvas `app/graphql/types/assignment_group_type.rb` /
//! `assignment_group_rules_type.rb` and the `assignmentGroupsConnection`
//! field on `course_type.rb`. Schema backing is migrations 0025
//! (`assignment_groups`) + 0049 (`assignment_group_rules`: one row per
//! group with `drop_lowest` / `drop_highest` / `never_drop` ids, LEFT
//! JOINed so groups without a rules row resolve `rules` to null, like
//! Canvas's nil `AssignmentGroup#rules`).
//!
//! Conventions are those of `assignment.rs`: camelCase field names,
//! relay-style connection with `edges { cursor node }` +
//! `pageInfo { hasNextPage endCursor }`, opaque cursors, `LIMIT first+1`
//! ordered by position then id (Canvas orders assignment groups by
//! position; id is the stable tiebreaker).
//!
//! Authorization: visibility is inherited from `course(id)` — anyone who
//! can resolve the course sees its assignment groups, matching the
//! star-assignment-groups REST index (groups only have
//! `available|deleted` states and deleted ones are filtered here, like
//! Canvas's `AssignmentGroup.active` scope).

use async_graphql::{Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, decode_cursor, encode_cursor, PageInfo, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

// ---------------------------------------------------------------------------
// Database row (LEFT JOIN: groups without a rules row still surface)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct AssignmentGroupRow {
    id: i64,
    course_id: i64,
    name: String,
    position: i32,
    group_weight: f64,
    drop_lowest: Option<i32>,
    drop_highest: Option<i32>,
    never_drop: Option<Vec<i64>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `AssignmentGroupRules`
/// (`app/graphql/types/assignment_group_rules_type.rb`): `dropLowest`,
/// `dropHighest`, `neverDrop` (id list). Members resolve to null when the
/// group has no `assignment_group_rules` row; a row with default zeroes
/// resolves `dropLowest`/`dropHighest` to 0 (Canvas serializes the raw
/// integers; only the row's absence is null).
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentGroupRules {
    pub drop_lowest: Option<i32>,
    pub drop_highest: Option<i32>,
    pub never_drop: Option<Vec<ID>>,
}

/// Canvas GraphQL `AssignmentGroup`
/// (`app/graphql/types/assignment_group_type.rb`, subset): `id`,
/// `courseId`, `name`, `position`, `groupWeight`, `rules`, `createdAt`,
/// `updatedAt`.
#[derive(Debug, Clone)]
pub struct AssignmentGroup {
    pub id: i64,
    pub course_id: i64,
    pub name: String,
    pub position: i32,
    pub group_weight: f64,
    pub drop_lowest: Option<i32>,
    pub drop_highest: Option<i32>,
    pub never_drop: Option<Vec<i64>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[Object]
impl AssignmentGroup {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn position(&self) -> i32 {
        self.position
    }

    /// Canvas `groupWeight` — a fraction of the final grade
    /// (0 when unweighted, like the column default).
    async fn group_weight(&self) -> f64 {
        self.group_weight
    }

    /// Canvas `rules` — an `AssignmentGroupRules` object, null when the
    /// group has no rules row.
    async fn rules(&self) -> Option<AssignmentGroupRules> {
        if self.drop_lowest.is_none() && self.drop_highest.is_none() && self.never_drop.is_none() {
            return None;
        }
        Some(AssignmentGroupRules {
            drop_lowest: self.drop_lowest,
            drop_highest: self.drop_highest,
            never_drop: self
                .never_drop
                .as_ref()
                .map(|ids| ids.iter().map(|id| ID(id.to_string())).collect::<Vec<ID>>()),
        })
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentGroupEdge {
    pub cursor: String,
    pub node: AssignmentGroup,
}

/// Relay connection for assignment groups.
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentGroupConnection {
    pub edges: Vec<AssignmentGroupEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetch
// ---------------------------------------------------------------------------

/// Fetch one page of a course's assignment groups: `LIMIT first+1` ordered
/// by position ascending then id (Canvas `AssignmentGroup.natural_order`),
/// optionally resuming after a cursor. The extra row drives `hasNextPage`.
pub(crate) async fn fetch_assignment_group_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<AssignmentGroupConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<AssignmentGroupRow> = sqlx::query_as(
        r#"
        SELECT g.id, g.course_id, g.name, g.position,
               g.group_weight::float8 AS group_weight,
               r.drop_lowest, r.drop_highest, r.never_drop,
               g.created_at, g.updated_at
        FROM assignment_groups g
        LEFT JOIN assignment_group_rules r ON r.assignment_group_id = g.id
        WHERE g.course_id = $1
          AND g.workflow_state <> 'deleted'
          AND ($2::bigint IS NULL OR g.id > $2)
        ORDER BY g.position, g.id
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

    let edges: Vec<AssignmentGroupEdge> = rows
        .into_iter()
        .map(|row| {
            let id = row.id;
            AssignmentGroupEdge {
                cursor: encode_cursor(id),
                node: AssignmentGroup {
                    id: row.id,
                    course_id: row.course_id,
                    name: row.name,
                    position: row.position,
                    group_weight: row.group_weight,
                    drop_lowest: row.drop_lowest,
                    drop_highest: row.drop_highest,
                    never_drop: row.never_drop,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
            }
        })
        .collect();

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(AssignmentGroupConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}
