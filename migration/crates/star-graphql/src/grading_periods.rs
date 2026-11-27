//! GradingPeriod + GradingPeriodGroup object types and the
//! `Course.gradingPeriods` field (GraphQL parity gap completion).
//!
//! Mirrors Canvas `app/graphql/types/grading_period_type.rb` /
//! `grading_period_group_type.rb` and the `gradingPeriods` field on
//! `course_type.rb` (Canvas resolves a course's grading periods through
//! its `GradingPeriodGroup`, sorted by start date). Schema backing is
//! migration 0050: `grading_period_groups` (account_id or course_id —
//! for a course lookup the course_id row is authoritative, mirroring
//! `GradingPeriodGroup.for_course`) and `grading_periods` (cascade).
//!
//! Canvas shape (subset, camelCase): GradingPeriodGroup carries `id`,
//! `title`, `weighted`, `createdAt`, `updatedAt` and its periods;
//! GradingPeriod carries `id`, `title`, `startDate`, `endDate`,
//! `closeDate`, `weight`, `createdAt`, `updatedAt`.
//!
//! Authorization: inherited from `course(id)` — anyone who can resolve
//! the course sees its grading periods (parity with the
//! star-grading-standards REST list, which gates on course visibility,
//! not a grading role; deleted groups/periods are filtered like
//! Canvas's `active` scopes).

use async_graphql::{Context, Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, GraphQLContext};

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct GradingPeriodGroupRow {
    id: i64,
    title: Option<String>,
    weighted: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct GradingPeriodRow {
    id: i64,
    title: String,
    start_date: DateTime<Utc>,
    end_date: DateTime<Utc>,
    close_date: DateTime<Utc>,
    weight: Option<f64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `GradingPeriod`
/// (`app/graphql/types/grading_period_type.rb`, subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct GradingPeriod {
    pub id: ID,
    pub title: String,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub close_date: DateTime<Utc>,
    /// Weight within a weighted set (null when unweighted — Canvas
    /// serializes the nil column value the same way).
    pub weight: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Canvas GraphQL `GradingPeriodGroup`
/// (`app/graphql/types/grading_period_group_type.rb`, subset): the
/// course's period set plus its periods, sorted by start date.
#[derive(Debug, Clone)]
pub struct GradingPeriodGroup {
    pub id: i64,
    pub title: Option<String>,
    pub weighted: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[Object]
impl GradingPeriodGroup {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    /// Canvas `title` — nullable on the column and in Canvas's
    /// serializer for legacy un-titled sets.
    async fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Canvas `weighted` — whether the set applies period weights.
    async fn weighted(&self) -> bool {
        self.weighted
    }

    /// The group's periods, sorted by start date (Canvas
    /// `GradingPeriodGroup.grading_periods` ordering).
    async fn grading_periods(
        &self,
        ctx: &Context<'_>,
    ) -> Result<Vec<GradingPeriod>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        fetch_periods_for_group(&gctx.pool, self.id).await
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

// ---------------------------------------------------------------------------
// Fetchers
// ---------------------------------------------------------------------------

/// All active periods of a group, ordered by start date.
async fn fetch_periods_for_group(
    pool: &PgPool,
    group_id: i64,
) -> Result<Vec<GradingPeriod>, async_graphql::Error> {
    let rows: Vec<GradingPeriodRow> = sqlx::query_as(
        r#"
        SELECT id, title, start_date, end_date,
               close_date, weight, created_at, updated_at
        FROM grading_periods
        WHERE grading_period_group_id = $1 AND workflow_state <> 'deleted'
        ORDER BY start_date, id
        "#,
    )
    .bind(group_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    Ok(rows
        .into_iter()
        .map(|r| GradingPeriod {
            id: ID(r.id.to_string()),
            title: r.title,
            start_date: r.start_date,
            end_date: r.end_date,
            close_date: r.close_date,
            weight: r.weight,
            created_at: r.created_at,
            updated_at: r.updated_at,
        })
        .collect())
}

/// One grading period by id — Canvas `Grades.gradingPeriod`.
pub(crate) async fn fetch_grading_period(
    pool: &PgPool,
    id: i64,
) -> Result<Option<GradingPeriod>, async_graphql::Error> {
    let row: Option<GradingPeriodRow> = sqlx::query_as(
        r#"
        SELECT id, title, start_date, end_date,
               close_date, weight, created_at, updated_at
        FROM grading_periods
        WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|r| GradingPeriod {
        id: ID(r.id.to_string()),
        title: r.title,
        start_date: r.start_date,
        end_date: r.end_date,
        close_date: r.close_date,
        weight: r.weight,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}

/// The course's grading period group (course_id row, active), or null when
/// the course has none — Canvas `course.gradingPeriods` resolves through
/// `GradingPeriodGroup.for_course(course)`.
pub(crate) async fn fetch_course_grading_period_group(
    pool: &PgPool,
    course_id: i64,
) -> Result<Option<GradingPeriodGroup>, async_graphql::Error> {
    let row: Option<GradingPeriodGroupRow> = sqlx::query_as(
        r#"
        SELECT id, title, weighted, created_at, updated_at
        FROM grading_period_groups
        WHERE course_id = $1 AND workflow_state = 'active'
        ORDER BY id
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;

    Ok(row.map(|r| GradingPeriodGroup {
        id: r.id,
        title: r.title,
        weighted: r.weighted,
        created_at: r.created_at,
        updated_at: r.updated_at,
    }))
}
