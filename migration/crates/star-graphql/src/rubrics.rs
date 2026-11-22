//! Rubric object-type family + `Course.rubricsConnection` (GraphQL parity
//! gap completion).
//!
//! Mirrors Canvas `app/graphql/types/rubric_type.rb`,
//! `rubric_criterion_type.rb`, `rubric_rating_type.rb`,
//! `rubric_assessment_type.rb` and the `rubricsConnection` field on
//! `course_type.rb`. Schema backing is migration 0023: `rubrics`,
//! `rubric_criteria` (ordered by position), `rubric_ratings` (ordered by
//! position within a criterion) and `rubric_assessments` (score + JSONB
//! data keyed by submission).
//!
//! Canvas shape (subset, camelCase):
//! - Rubric: `id`, `courseId`, `title`, `pointsPossible`, `createdAt`,
//!   `updatedAt`, plus nested `criteria { … ratings { … } }`;
//! - RubricCriterion (Canvas `RubricCriterion` is surfaced through the
//!   legacy rubric JSON in some clients; the type exists in the schema
//!   inventory): `id`, `description`, `points`, `position`;
//! - RubricRating: `id`, `description`, `points`, `position`;
//! - RubricAssessment: `id`, `score`, `createdAt`, `updatedAt`.
//!
//! Authorization: visibility is inherited from `course(id)` — parity with
//! the star-search rubrics REST handler (teachers manage, readers read:
//! in star-lms anyone enrolled can list rubrics). Deleted rubrics are
//! filtered (Canvas `Rubric.active`).

use async_graphql::{Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, decode_cursor, encode_cursor, PageInfo, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct RubricRow {
    id: i64,
    course_id: i64,
    title: String,
    points_possible: f64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct CriterionRow {
    id: i64,
    rubric_id: i64,
    description: String,
    points: f64,
    position: i32,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct RatingRow {
    id: i64,
    criterion_id: i64,
    description: String,
    points: f64,
    position: i32,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct AssessmentRow {
    id: i64,
    rubric_id: i64,
    submission_id: i64,
    assessor_id: i64,
    score: Option<f64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `RubricRating`
/// (`app/graphql/types/rubric_rating_type.rb`, subset).
#[derive(Debug, Clone, SimpleObject)]
pub struct RubricRating {
    pub id: ID,
    pub description: String,
    pub points: f64,
    pub position: i32,
}

/// Canvas GraphQL `RubricCriterion`
/// (`app/graphql/types/rubric_criterion_type.rb`, subset) with its
/// nested ratings, position-ordered.
#[derive(Debug, Clone)]
pub struct RubricCriterion {
    pub id: i64,
    pub description: String,
    pub points: f64,
    pub position: i32,
    pub ratings: Vec<RubricRating>,
}

#[Object]
impl RubricCriterion {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn description(&self) -> &str {
        &self.description
    }

    async fn points(&self) -> f64 {
        self.points
    }

    async fn position(&self) -> i32 {
        self.position
    }

    /// The criterion's ratings, position-ordered.
    async fn ratings(&self) -> &[RubricRating] {
        &self.ratings
    }
}

/// Canvas GraphQL `Rubric` (`app/graphql/types/rubric_type.rb`, subset).
#[derive(Debug, Clone)]
pub struct Rubric {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    pub points_possible: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub criteria: Vec<RubricCriterion>,
}

#[Object]
impl Rubric {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    async fn title(&self) -> &str {
        &self.title
    }

    async fn points_possible(&self) -> f64 {
        self.points_possible
    }

    /// The rubric's criteria with nested ratings, position-ordered
    /// (Canvas `Rubric#rubric_criteria` ordering).
    async fn criteria(&self) -> &[RubricCriterion] {
        &self.criteria
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

/// Canvas GraphQL `RubricAssessment`
/// (`app/graphql/types/rubric_assessment_type.rb`, subset over the
/// `rubric_assessments` table: score + assessor, not the JSONB data
/// breakdown).
#[derive(Debug, Clone, SimpleObject)]
pub struct RubricAssessment {
    pub id: ID,
    pub rubric_id: ID,
    pub submission_id: ID,
    pub assessor_id: ID,
    pub score: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct RubricEdge {
    pub cursor: String,
    pub node: Rubric,
}

/// Relay connection for rubrics.
#[derive(Debug, Clone, SimpleObject)]
pub struct RubricConnection {
    pub edges: Vec<RubricEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetch
// ---------------------------------------------------------------------------

/// Hydrate one rubric's criteria + ratings (two queries, position order).
async fn hydrate_criteria(pool: &PgPool, rubric_id: i64) -> Result<Vec<RubricCriterion>, async_graphql::Error> {
    let criteria: Vec<CriterionRow> = sqlx::query_as(
        r#"
        SELECT id, rubric_id, description, points::float8 AS points, position
        FROM rubric_criteria
        WHERE rubric_id = $1
        ORDER BY position, id
        "#,
    )
    .bind(rubric_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let mut out = Vec::with_capacity(criteria.len());
    for c in criteria {
        let ratings: Vec<RatingRow> = sqlx::query_as(
            r#"
            SELECT id, criterion_id, description, points::float8 AS points, position
            FROM rubric_ratings
            WHERE criterion_id = $1
            ORDER BY position, id
            "#,
        )
        .bind(c.id)
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        out.push(RubricCriterion {
            id: c.id,
            description: c.description,
            points: c.points,
            position: c.position,
            ratings: ratings
                .into_iter()
                .map(|r| RubricRating {
                    id: ID(r.id.to_string()),
                    description: r.description,
                    points: r.points,
                    position: r.position,
                })
                .collect(),
        });
    }
    Ok(out)
}

/// Fetch one page of a course's rubrics (with criteria + ratings
/// hydrated): `LIMIT first+1` ordered by id, optionally resuming after a
/// cursor. The extra row drives `hasNextPage`.
pub(crate) async fn fetch_rubric_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<RubricConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<RubricRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, points_possible::float8 AS points_possible,
               created_at, updated_at
        FROM rubrics
        WHERE course_id = $1
          AND workflow_state <> 'deleted'
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

    let mut edges = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row.id;
        let criteria = hydrate_criteria(pool, row.id).await?;
        edges.push(RubricEdge {
            cursor: encode_cursor(id),
            node: Rubric {
                id: row.id,
                course_id: row.course_id,
                title: row.title,
                points_possible: row.points_possible,
                created_at: row.created_at,
                updated_at: row.updated_at,
                criteria,
            },
        });
    }

    let end_cursor = edges.last().map(|e| e.cursor.clone());
    Ok(RubricConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}
