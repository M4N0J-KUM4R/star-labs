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

use async_graphql::SimpleObject;
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
    workflow_state: String,
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
    description: String,
    points: f64,
    position: i32,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

// NOTE: the `Rubric`, `RubricCriterion` and `RubricRating` objects live in
// `root_types.rs` — they are the only ones in the schema (they also answer
// `Query.rubric` and the `Node` interface), and they carry the Canvas
// fields (`workflowState`, `buttonDisplay`, `ratingOrder`, ...).

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct RubricEdge {
    pub cursor: String,
    pub node: crate::root_types::Rubric,
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
async fn hydrate_criteria(
    pool: &PgPool,
    rubric_id: i64,
) -> Result<Vec<crate::root_types::RubricCriterion>, async_graphql::Error> {
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
            SELECT id, description, points::float8 AS points, position
            FROM rubric_ratings
            WHERE criterion_id = $1
            ORDER BY position, id
            "#,
        )
        .bind(c.id)
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        out.push(crate::root_types::RubricCriterion {
            id: c.id,
            rubric_id: c.rubric_id,
            description: c.description,
            points: c.points,
            position: c.position,
            ratings: ratings
                .into_iter()
                .map(|r| crate::root_types::RubricRating {
                    id: r.id,
                    rubric_id,
                    criterion_id: c.id,
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
               workflow_state, created_at, updated_at
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
            node: crate::root_types::Rubric {
                id: row.id,
                course_id: row.course_id,
                title: row.title,
                points_possible: row.points_possible,
                workflow_state: row.workflow_state,
                criteria,
                created_at: row.created_at,
                updated_at: row.updated_at,
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
