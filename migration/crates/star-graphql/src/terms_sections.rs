//! Enrollment term and course section read types + root queries (issue #27).
//!
//! Mirrors the conventions of the foundation slice in `lib.rs`: camelCase
//! field names via the `#[Object]` derive (`startAt`, `endAt`, `courseId`,
//! `createdAt`, `updatedAt`), the Canvas `state` renaming of the DB
//! `workflow_state` column (like `Course.state`), and null on unknown or
//! unauthorized ids.
//!
//! Canvas shape references: `app/graphql/types/term_type.rb` and
//! `section_type.rb` in canvas-lms. SIS fields (`sisTermId`, `sisId`) and
//! roster aggregates (`userCount`, `students`, `allStudents`, `gradesPosted`,
//! `gradesPresent`) are omitted — the star-lms schema has no columns for
//! them (documented in `specs/gql/terms-sections.md`).
//!
//! Authorization:
//! - `enrollmentTerm(id)` is readable by any authenticated caller (Canvas's
//!   GraphQL API serves terms to authenticated users; the REST route is
//!   account-admin-only — documented deviation, see the spec). Unknown ids
//!   resolve to null.
//! - `courseSection(id)` mirrors `course(id)`: enrolled users of the course
//!   (any role), teachers of the course, and admins; null otherwise.
//!
//! Authentication itself is enforced before execution by the router's
//! `AuthUser` extractor (HTTP 401), so every resolver here can assume an
//! authenticated caller.

use async_graphql::{Object, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

// ---------------------------------------------------------------------------
// Database rows (runtime sqlx::query_as + FromRow, like every other crate)
// ---------------------------------------------------------------------------

/// Row of the `terms` table (migration 0034). The table is named `terms`, not
/// `enrollment_terms`; `start_at`/`end_at` are nullable.
#[derive(Debug, Clone, sqlx::FromRow)]
struct TermRow {
    id: i64,
    name: String,
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Row of the `course_sections` table (migrations 0034/0047).
#[derive(Debug, Clone, sqlx::FromRow)]
struct SectionRow {
    id: i64,
    course_id: i64,
    name: String,
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `EnrollmentTerm` (subset of `app/graphql/types/term_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `name`, `startAt`,
/// `endAt`, `state`, `createdAt`, `updatedAt`.
pub struct EnrollmentTerm {
    id: i64,
    name: String,
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl EnrollmentTerm {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn start_at(&self) -> Option<DateTime<Utc>> {
        self.start_at
    }

    async fn end_at(&self) -> Option<DateTime<Utc>> {
        self.end_at
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
}

/// Canvas GraphQL `Section` (subset of `app/graphql/types/section_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `name`, `courseId`,
/// `startAt`, `endAt`, `state`, `createdAt`, `updatedAt`.
pub struct CourseSection {
    id: i64,
    course_id: i64,
    name: String,
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl CourseSection {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn name(&self) -> &str {
        &self.name
    }

    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    async fn start_at(&self) -> Option<DateTime<Utc>> {
        self.start_at
    }

    async fn end_at(&self) -> Option<DateTime<Utc>> {
        self.end_at
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
}

// ---------------------------------------------------------------------------
// Query resolvers (called from the Query root in lib.rs)
// ---------------------------------------------------------------------------

/// `enrollmentTerm(id: ID!)` — any authenticated caller may read the term
/// shape; no role check. Unknown ids resolve to null, like Canvas.
pub(crate) async fn fetch_term(
    pool: &PgPool,
    id: i64,
) -> Result<Option<EnrollmentTerm>, async_graphql::Error> {
    let row: Option<TermRow> = sqlx::query_as(
        r#"
        SELECT id, name, start_at, end_at, workflow_state, created_at, updated_at
        FROM terms
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|row| EnrollmentTerm {
        id: row.id,
        name: row.name,
        start_at: row.start_at,
        end_at: row.end_at,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

/// `courseSection(id: ID!)` — visible to users enrolled in the section's
/// course (any role), teachers of the course, and admins; everyone else
/// gets null, mirroring `course(id)` visibility.
pub(crate) async fn fetch_section(
    pool: &PgPool,
    user_id: i64,
    is_admin: bool,
    id: i64,
) -> Result<Option<CourseSection>, async_graphql::Error> {
    let row: Option<SectionRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, name, start_at, end_at, workflow_state, created_at, updated_at
        FROM course_sections
        WHERE id = $1
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else {
        return Ok(None);
    };

    if !is_admin
        && !user_enrolled_in_course(pool, user_id, row.course_id).await?
        && !user_teaches_course(pool, user_id, row.course_id).await?
    {
        return Ok(None);
    }

    Ok(Some(CourseSection {
        id: row.id,
        course_id: row.course_id,
        name: row.name,
        start_at: row.start_at,
        end_at: row.end_at,
        workflow_state: row.workflow_state,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// Map a database failure to a GraphQL error. Same body as `db_err` in
/// `lib.rs`; kept as a private copy so this slice does not depend on lib.rs
/// visibility churn from the parallel gql-foundation branches.
fn db_err(e: sqlx::Error) -> async_graphql::Error {
    eprintln!("database error: {e}");
    async_graphql::Error::new("database error")
}

// Copied verbatim from lib.rs (user_enrolled_in_course / user_teaches_course)
// to keep the lib.rs diff of this slice minimal across the parallel
// gql-foundation branches.
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
