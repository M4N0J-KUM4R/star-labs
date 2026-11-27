//! GradingStandard + GradingStandardItem and AccountNotification object
//! types (GraphQL parity gap completion).
//!
//! Mirrors Canvas `app/graphql/types/grading_standard_type.rb` /
//! `grading_standard_item_type.rb` / `account_notification_type.rb` and
//! the `gradingStandards` field on `course_type.rb` plus the
//! `accountNotification` root query (Canvas resolves account
//! announcements for the current user; here it is exposed per-account
//! under `Query` for callers with account visibility).
//!
//! Schema backing:
//! - migration 0027: `grading_standards` (course_id NULL = account
//!   level) + `grading_scheme_entries` (name, min_score 0.0–1.0
//!   percentage, position);
//! - migration 0072 adds the account fields (account_id, points_based,
//!   scaling_factor);
//! - migration 0035: `account_notifications` (+ roles join table).
//!
//! Canvas shape (subset, camelCase):
//! - GradingStandard: `id`, `title`, `pointsBased`, `scalingFactor`,
//!   `createdAt`, `updatedAt`, nested `gradingStandardItemData`
//!   (Canvas exposes the scheme rows through items);
//! - GradingStandardItem: `name`, `value` (the min score fraction);
//! - AccountNotification: `id`, `accountId`, `subject`, `message`,
//!   `startAt`, `endAt`, `icon`, `workflowState`.
//!
//! Authorization: `Course.gradingStandards` inherits the course(id)
//! gate (anyone who can see the course sees its schemes — REST parity
//! with the star-grading-standards list). `Query.accountNotification`
//! requires the caller to be an admin of the account (star-lms has no
//! account-role model; global admin only — documented deviation) and
//! returns the window-valid announcements (start_at <= now < end_at)
//! like Canvas's user-facing resolver.

use async_graphql::{Object, SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::db_err;

// ---------------------------------------------------------------------------
// Database rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct GradingStandardRow {
    id: i64,
    title: String,
    points_based: bool,
    scaling_factor: f64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SchemeEntryRow {
    name: String,
    min_score: f64,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `GradingStandardItem`
/// (`app/graphql/types/grading_standard_item_type.rb`, subset): a
/// scheme row. `value` is the minimum score fraction (0.0–1.0).
#[derive(Debug, Clone, SimpleObject)]
pub struct GradingStandardItem {
    pub name: String,
    pub value: f64,
}

/// Canvas GraphQL `GradingStandard`
/// (`app/graphql/types/grading_standard_type.rb`, subset) with its
/// scheme rows nested (Canvas `gradingStandardItemData`).
#[derive(Debug, Clone)]
pub struct GradingStandard {
    pub id: i64,
    pub title: String,
    pub points_based: bool,
    pub scaling_factor: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub items: Vec<GradingStandardItem>,
}

#[Object]
impl GradingStandard {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn title(&self) -> &str {
        &self.title
    }

    /// Canvas `pointsBased` (Canvas `GradingStandard#points_based`,
    /// 0072 column).
    async fn points_based(&self) -> bool {
        self.points_based
    }

    /// Canvas `scalingFactor` (0072 column).
    async fn scaling_factor(&self) -> f64 {
        self.scaling_factor
    }

    /// The scheme rows, position-ordered (Canvas
    /// `grading_standard_item_data`).
    async fn grading_standard_item_data(&self) -> &[GradingStandardItem] {
        &self.items
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

/// A course's active grading standards with their scheme rows
/// (position-ordered). Account-level fallbacks (course_id NULL on the
/// account) are NOT included — Canvas course `gradingStandards` lists
/// the course's own schemes.
pub(crate) async fn fetch_course_grading_standards(
    pool: &PgPool,
    course_id: i64,
) -> Result<Vec<GradingStandard>, async_graphql::Error> {
    let rows: Vec<GradingStandardRow> = sqlx::query_as(
        r#"
        SELECT id, title, points_based, scaling_factor::float8 AS scaling_factor,
               created_at, updated_at
        FROM grading_standards
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        ORDER BY id
        "#,
    )
    .bind(course_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let entries: Vec<SchemeEntryRow> = sqlx::query_as(
            r#"
            SELECT name, min_score::float8 AS min_score
            FROM grading_scheme_entries
            WHERE grading_standard_id = $1
            ORDER BY position, id
            "#,
        )
        .bind(row.id)
        .fetch_all(pool)
        .await
        .map_err(db_err)?;
        out.push(GradingStandard {
            id: row.id,
            title: row.title,
            points_based: row.points_based,
            scaling_factor: row.scaling_factor,
            created_at: row.created_at,
            updated_at: row.updated_at,
            items: entries
                .into_iter()
                .map(|e| GradingStandardItem {
                    name: e.name,
                    value: e.min_score,
                })
                .collect(),
        });
    }
    Ok(out)
}
