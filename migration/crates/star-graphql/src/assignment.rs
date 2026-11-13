//! Assignment object type + `Course.assignmentsConnection` (issue #26), plus
//! the `Assignment.submissionsConnection` field (issue #48) whose Submission
//! type and page fetch live in `submission.rs`.
//!
//! Mirrors the conventions of the merged foundation slice in `lib.rs`:
//! camelCase field names via the SimpleObject derive (`courseId`,
//! `pointsPossible`, `dueAt`), relay-style connection with
//! `edges { cursor node }` + `pageInfo { hasNextPage endCursor }`, opaque
//! cursors, `LIMIT first+1` ordered by id.
//!
//! Authorization: visibility of the connection is inherited from
//! `course(id)` (enrolled users of any role, or admins; everyone else gets
//! a null course and never reaches this field). Inside the connection the
//! REST list handler's parity applies: star-assignments has no
//! published/unpublished filter on its list route because the schema has
//! no `workflow_state` on assignments, so the connection returns every
//! assignment of the course. Deviation documented in
//! `specs/gql/assignments.md`.

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::submission::{self, SubmissionConnection};
use crate::{
    db_err, decode_cursor, encode_cursor, GraphQLContext, PageInfo, DEFAULT_PAGE_SIZE,
    MAX_PAGE_SIZE,
};

// ---------------------------------------------------------------------------
// Database row (runtime sqlx::query_as + FromRow, like every other crate)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct AssignmentRow {
    id: i64,
    course_id: i64,
    name: String,
    description: Option<String>,
    points_possible: f64,
    due_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Assignment` (subset of `app/graphql/types/assignment_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `courseId`,
/// `name`, `description`, `pointsPossible`, `dueAt`, `createdAt`, `updatedAt`,
/// `submissionsConnection`.
#[derive(Debug, Clone)]
pub struct Assignment {
    pub id: i64,
    pub course_id: i64,
    pub name: String,
    pub description: Option<String>,
    pub points_possible: f64,
    pub due_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[Object]
impl Assignment {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    async fn points_possible(&self) -> f64 {
        self.points_possible
    }

    async fn due_at(&self) -> Option<DateTime<Utc>> {
        self.due_at
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Paginated submissions for this assignment (LIMIT first+1, ORDER BY
    /// id). Roster parity with `Course.enrollmentsConnection`: admins and
    /// course teachers see every submission, anyone else sees only their own.
    /// A student with no submission gets empty edges — star-lms stores no
    /// unsubmitted placeholder rows.
    async fn submissions_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<SubmissionConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            submission::fetch_visible_submission_page(
                &gctx.pool,
                gctx.auth_user.id,
                gctx.is_admin(),
                self.id,
                self.course_id,
                first,
                after.as_deref(),
            )
            .await?,
        ))
    }
}

impl From<AssignmentRow> for Assignment {
    fn from(row: AssignmentRow) -> Self {
        Assignment {
            id: row.id,
            course_id: row.course_id,
            name: row.name,
            description: row.description,
            points_possible: row.points_possible,
            due_at: row.due_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentEdge {
    pub cursor: String,
    pub node: Assignment,
}

/// Relay connection for assignments.
#[derive(Debug, Clone, SimpleObject)]
pub struct AssignmentConnection {
    pub edges: Vec<AssignmentEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetch (same pagination shape as fetch_enrollment_page)
// ---------------------------------------------------------------------------

/// Fetch one page of a course's assignments: `LIMIT first+1` ordered by id
/// ascending, optionally resuming after a cursor. The extra row drives
/// `hasNextPage`.
pub(crate) async fn fetch_assignment_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<AssignmentConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<AssignmentRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title AS name, description,
               points_possible::float8 AS points_possible,
               due_at, created_at, updated_at
        FROM assignments
        WHERE course_id = $1
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

    let edges: Vec<AssignmentEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            AssignmentEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(AssignmentConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}
