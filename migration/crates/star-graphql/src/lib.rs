//! star-graphql — GraphQL foundation for star-lms.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`, mounted by star-http
//! at `/api/graphql` (Canvas serves its GraphQL API there, outside `/api/v1`).
//!
//! The vertical slice covers Canvas-shaped read queries over the existing
//! schema: `course(id)` and `user(id)` with `enrollmentsConnection` cursor
//! pagination, mirroring the connection conventions of Canvas's own GraphQL
//! schema (`app/graphql/types/*_type.rb` in canvas-lms):
//! camelCase field names (`sisUserId`, `createdAt`), relay-style connections
//! with `edges { cursor node }` and `pageInfo { hasNextPage endCursor }`, and
//! opaque cursors.
//!
//! Authorization parity with the REST endpoints:
//! - unauthenticated requests are rejected with HTTP 401 *before* any GraphQL
//!   execution (via the shared `AuthUser` extractor, not a GraphQL error);
//! - `course(id)` requires an enrollment in the course (any role) or admin;
//! - `Course.enrollmentsConnection` shows every enrollment to admins and
//!   course teachers, but only the caller's own enrollment to anyone else;
//! - `User.enrollmentsConnection` is self-or-admin (null for others).

use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema, SimpleObject, ID};
use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
use axum::{
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

mod assignment;
mod discussion_topic;
mod submission;
mod terms_sections;

use assignment::AssignmentConnection;
use discussion_topic::DiscussionTopicConnection;
use terms_sections::{CourseSection, EnrollmentTerm};

// ---------------------------------------------------------------------------
// GraphQL context
// ---------------------------------------------------------------------------

/// Per-request execution context: the database pool plus the authenticated
/// user. The user is resolved by the `AuthUser` extractor before the schema
/// runs, so the role is always fresh from the database (the JWT role claim is
/// never trusted for authorization, matching the REST extractors).
pub struct GraphQLContext {
    pub(crate) pool: PgPool,
    pub(crate) auth_user: AuthUser,
}

impl GraphQLContext {
    pub(crate) fn is_admin(&self) -> bool {
        self.auth_user.role == "admin"
    }
}

/// Map a database failure to a GraphQL error. Resolvers never panic on DB
/// trouble; failures surface as normal GraphQL `errors` entries.
pub(crate) fn db_err(e: sqlx::Error) -> async_graphql::Error {
    eprintln!("database error: {e}");
    async_graphql::Error::new("database error")
}

// ---------------------------------------------------------------------------
// Opaque cursors
// ---------------------------------------------------------------------------

/// Encode an enrollment id as an opaque pagination cursor (fixed-width hex).
/// Clients treat cursors as opaque tokens; `decode_cursor` is the only reader.
pub(crate) fn encode_cursor(id: i64) -> String {
    format!("{id:016x}")
}

pub(crate) fn decode_cursor(cursor: &str) -> Option<i64> {
    i64::from_str_radix(cursor, 16).ok()
}

// ---------------------------------------------------------------------------
// Database rows (runtime sqlx::query_as + FromRow, like every other crate)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct CourseRow {
    id: i64,
    name: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct UserRow {
    id: i64,
    name: String,
    email: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct EnrollmentRow {
    id: i64,
    user_id: i64,
    course_id: i64,
    role: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Course` (subset of `app/graphql/types/course_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `name`, `state`,
/// `createdAt`, `updatedAt`, `enrollmentsConnection`.
pub struct Course {
    id: i64,
    name: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl Course {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
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

    /// Relay-style connection with cursor pagination: LIMIT first+1,
    /// ORDER BY id. Roster parity: admins and course teachers see every
    /// enrollment; other enrolled users (students) see only their own.
    async fn enrollments_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<EnrollmentConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;

        let scoped_to_self = !gctx.is_admin()
            && !user_teaches_course(&gctx.pool, gctx.auth_user.id, self.id).await?;
        let user_id = if scoped_to_self {
            Some(gctx.auth_user.id)
        } else {
            None
        };

        Ok(Some(
            fetch_enrollment_page(&gctx.pool, Some(self.id), user_id, first, after.as_deref())
                .await?,
        ))
    }

    /// Paginated assignments in this course (LIMIT first+1, ORDER BY id).
    /// Everyone who can resolve the course sees its assignments — parity
    /// with the star-assignments REST list handler; the schema has no
    /// `workflow_state` on assignments yet, so there is no
    /// published/unpublished split to enforce (documented in
    /// `specs/gql/assignments.md`).
    async fn assignments_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<AssignmentConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            assignment::fetch_assignment_page(&gctx.pool, self.id, first, after.as_deref()).await?,
        ))
    }

    /// Paginated, non-deleted discussion topics in this course (LIMIT first+1,
    /// ORDER BY id — Canvas orders regular discussions by `id: asc`).
    /// Everyone who can resolve the course sees the same set: parity with the
    /// star-discussions REST list handler and Canvas's `DiscussionTopic.active`
    /// scope (`workflow_state <> 'deleted'`). Canvas's per-viewer
    /// section/availability scoping is a documented deviation
    /// (`specs/gql/submissions-discussions.md`).
    async fn discussions_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<DiscussionTopicConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(Some(
            discussion_topic::fetch_discussion_topic_page(
                &gctx.pool,
                self.id,
                first,
                after.as_deref(),
            )
            .await?,
        ))
    }
}

/// Canvas GraphQL `User` (subset of `app/graphql/types/user_type.rb`).
pub struct User {
    id: i64,
    name: String,
    email: String,
    /// Canvas `User.sisId`. star-lms has no SIS columns yet, so this is null
    /// for now; the field is kept for shape parity.
    sis_user_id: Option<String>,
    created_at: DateTime<Utc>,
}

#[Object]
impl User {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn email(&self) -> &str {
        &self.email
    }

    async fn sis_user_id(&self) -> Option<&str> {
        self.sis_user_id.as_deref()
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Self-or-admin, like Canvas's user enrollments visibility: other users
    /// get null instead of the connection.
    async fn enrollments_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<EnrollmentConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;

        if !(gctx.is_admin() || gctx.auth_user.id == self.id) {
            return Ok(None);
        }

        Ok(Some(
            fetch_enrollment_page(&gctx.pool, None, Some(self.id), first, after.as_deref()).await?,
        ))
    }
}

/// Canvas GraphQL `Enrollment` (subset of `enrollment_type.rb`). Field names
/// are camelCased by the SimpleObject derive: `id`, `userId`, `courseId`,
/// `role`, `state`, `createdAt`, `updatedAt`.
#[derive(Debug, Clone, SimpleObject)]
pub struct Enrollment {
    pub id: ID,
    pub user_id: ID,
    pub course_id: ID,
    pub role: String,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<EnrollmentRow> for Enrollment {
    fn from(row: EnrollmentRow) -> Self {
        Enrollment {
            id: ID(row.id.to_string()),
            user_id: ID(row.user_id.to_string()),
            course_id: ID(row.course_id.to_string()),
            role: row.role,
            state: row.workflow_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct EnrollmentEdge {
    pub cursor: String,
    pub node: Enrollment,
}

/// Relay page info: `pageInfo { hasNextPage endCursor }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

/// Relay connection for enrollments.
#[derive(Debug, Clone, SimpleObject)]
pub struct EnrollmentConnection {
    pub edges: Vec<EnrollmentEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Query root
// ---------------------------------------------------------------------------

pub struct Query;

#[Object]
impl Query {
    /// `course(id: ID!)` — visible to enrolled users (any role) and admins;
    /// unknown or unauthorized ids resolve to null, like Canvas.
    async fn course(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<Course>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let course_id: i64 =
            id.0.parse()
                .map_err(|_| async_graphql::Error::new("invalid course id"))?;

        let row: Option<CourseRow> = sqlx::query_as(
            "SELECT id, name, workflow_state, created_at, updated_at FROM courses WHERE id = $1",
        )
        .bind(course_id)
        .fetch_optional(&gctx.pool)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Ok(None);
        };

        if !gctx.is_admin()
            && !user_enrolled_in_course(&gctx.pool, gctx.auth_user.id, course_id).await?
        {
            return Ok(None);
        }

        Ok(Some(Course {
            id: row.id,
            name: row.name,
            workflow_state: row.workflow_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }))
    }

    /// `user(id: ID!)` — any authenticated caller may read the basic profile
    /// shape; the `enrollmentsConnection` field enforces self-or-admin.
    async fn user(&self, ctx: &Context<'_>, id: ID) -> Result<Option<User>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let user_id: i64 =
            id.0.parse()
                .map_err(|_| async_graphql::Error::new("invalid user id"))?;

        let row: Option<UserRow> =
            sqlx::query_as("SELECT id, name, email, created_at FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(&gctx.pool)
                .await
                .map_err(db_err)?;

        Ok(row.map(|row| User {
            id: row.id,
            name: row.name,
            email: row.email,
            sis_user_id: None,
            created_at: row.created_at,
        }))
    }

    /// `enrollmentTerm(id: ID!)` — any authenticated caller may read the
    /// basic term shape; unknown ids resolve to null, like Canvas.
    async fn enrollment_term(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<EnrollmentTerm>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let term_id: i64 =
            id.0.parse()
                .map_err(|_| async_graphql::Error::new("invalid term id"))?;

        terms_sections::fetch_term(&gctx.pool, term_id).await
    }

    /// `courseSection(id: ID!)` — visible to enrolled users of the course
    /// (any role), teachers of the course, and admins; everyone else gets
    /// null, mirroring `course(id)` visibility.
    async fn course_section(
        &self,
        ctx: &Context<'_>,
        id: ID,
    ) -> Result<Option<CourseSection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        let section_id: i64 =
            id.0.parse()
                .map_err(|_| async_graphql::Error::new("invalid section id"))?;

        terms_sections::fetch_section(&gctx.pool, gctx.auth_user.id, gctx.is_admin(), section_id)
            .await
    }
}

// ---------------------------------------------------------------------------
// Permission helpers (same DB checks as the star-enrollments REST handlers)
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

// ---------------------------------------------------------------------------
// Cursor pagination
// ---------------------------------------------------------------------------

/// Default page size for connections when `first` is omitted.
pub(crate) const DEFAULT_PAGE_SIZE: i32 = 10;

/// Maximum accepted page size for connections.
pub(crate) const MAX_PAGE_SIZE: i32 = 100;

/// Fetch one page of enrollments: `LIMIT first+1` ordered by id ascending,
/// optionally scoped to one course and/or one user, optionally resuming
/// after a cursor. The extra row drives `hasNextPage`.
async fn fetch_enrollment_page(
    pool: &PgPool,
    course_id: Option<i64>,
    user_id: Option<i64>,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<EnrollmentConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<EnrollmentRow> = sqlx::query_as(
        r#"
        SELECT e.id, e.user_id, e.course_id, e.role, e.workflow_state,
               e.created_at, e.updated_at
        FROM enrollments e
        WHERE e.workflow_state != 'deleted'
          AND ($1::bigint IS NULL OR e.course_id = $1)
          AND ($2::bigint IS NULL OR e.user_id = $2)
          AND ($3::bigint IS NULL OR e.id > $3)
        ORDER BY e.id
        LIMIT $4
        "#,
    )
    .bind(course_id)
    .bind(user_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<EnrollmentEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            EnrollmentEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(EnrollmentConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

// ---------------------------------------------------------------------------
// Router + handler
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new().route("/", get(graphql_handler).post(graphql_handler))
}

/// GET/POST `/api/graphql` — the Canvas GraphQL endpoint.
///
/// Authentication happens before execution via the shared `AuthUser`
/// extractor: unauthenticated requests get HTTP 401 (never a GraphQL error),
/// and database failures during auth surface with their real status. The
/// schema itself is stateless, so it is built per request around the pool and
/// the authenticated user.
async fn graphql_handler(
    State(state): State<AppState>,
    auth: Result<AuthUser, ApiError>,
    req: GraphQLRequest,
) -> Response {
    let auth_user = match auth {
        Ok(user) => user,
        Err(e) => return e.into_response(),
    };

    let schema = Schema::build(Query, EmptyMutation, EmptySubscription)
        .data(GraphQLContext {
            pool: state.db.clone(),
            auth_user,
        })
        .finish();

    let resp = schema.execute(req.into_inner()).await;
    GraphQLResponse::from(resp).into_response()
}
