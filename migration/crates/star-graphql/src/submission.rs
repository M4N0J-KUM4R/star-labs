//! Submission object type + `Assignment.submissionsConnection` (issue #48).
//!
//! Mirrors the conventions of the merged foundation slice in `lib.rs` and the
//! assignment slice in `assignment.rs`: camelCase field names via the
//! SimpleObject derive (`assignmentId`, `userId`, `submittedAt`, `gradedAt`,
//! `latePolicyStatus`, `deductedPoints`), relay-style connection with
//! `edges { cursor node }` + `pageInfo { hasNextPage endCursor }`, opaque
//! cursors, `LIMIT first+1` ordered by id.
//!
//! Canvas shape reference: `app/graphql/types/submission_type.rb` +
//! `interfaces/submission_interface.rb`. Omitted for missing columns and
//! documented in `specs/gql/submissions-discussions.md`: `grade` /
//! `enteredScore` / `enteredGrade` (no columns; `score` is the grade),
//! `posted` / `postedAt`, `url` / `externalToolUrl` / `previewUrl` (no `url`
//! column), `readState` (needs a `submission_reads` join), Canvas-computed
//! fields (`late`, `missing`, `secondsLate`, …), object refs (`user`,
//! `assignment`) and the comment/history connections (later slices).
//! `submissions.grader_id` exists but Canvas's Submission type has no
//! `graderId` field, so it is not exposed.
//!
//! Authorization: the connection is only reachable through
//! `course(id) → assignmentsConnection` (enrolled users of any role, or
//! admins). Inside, the `enrollmentsConnection` roster rule applies: admins
//! and course teachers see every submission of the assignment, anyone else
//! (enrolled students, TAs) sees only their own. A student with no submission
//! gets empty edges — star-lms has no unsubmitted placeholder rows.

use async_graphql::{SimpleObject, ID};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

use crate::{db_err, decode_cursor, encode_cursor, PageInfo, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};

// ---------------------------------------------------------------------------
// Database row (runtime sqlx::query_as + FromRow, like every other crate)
// ---------------------------------------------------------------------------

/// Row of the `submissions` table (migrations 0001 + 0060).
#[derive(Debug, Clone, sqlx::FromRow)]
struct SubmissionRow {
    id: i64,
    assignment_id: i64,
    user_id: i64,
    attempt: i32,
    body: Option<String>,
    score: Option<f64>,
    submission_type: Option<String>,
    workflow_state: String,
    submitted_at: DateTime<Utc>,
    graded_at: Option<DateTime<Utc>>,
    excused: bool,
    late_policy_status: Option<String>,
    points_deducted: Option<f64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// ---------------------------------------------------------------------------

/// Canvas GraphQL `Submission` (subset of `submission_type.rb` /
/// `submission_interface.rb`). Field names are camelCased by the SimpleObject
/// derive: `id`, `assignmentId`, `userId`, `attempt`, `body`, `score`,
/// `state`, `submissionType`, `submittedAt`, `gradedAt`, `excused`,
/// `latePolicyStatus`, `deductedPoints`, `createdAt`, `updatedAt`.
#[derive(Debug, Clone, SimpleObject)]
pub struct Submission {
    pub id: ID,
    pub assignment_id: ID,
    pub user_id: ID,
    pub attempt: i32,
    pub body: Option<String>,
    /// Canvas exposes the grade separately; star-lms has only `score`
    /// (NUMERIC → float8), which doubles as the grade like the REST handlers.
    pub score: Option<f64>,
    /// Canvas renames the DB `workflow_state` to `state`
    /// (Canvas `SubmissionStateType`: submitted | pending_review | graded).
    pub state: String,
    pub submission_type: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub graded_at: Option<DateTime<Utc>>,
    pub excused: bool,
    pub late_policy_status: Option<String>,
    /// Canvas `deductedPoints` — the DB `points_deducted` late-policy column.
    pub deducted_points: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<SubmissionRow> for Submission {
    fn from(row: SubmissionRow) -> Self {
        Submission {
            id: ID(row.id.to_string()),
            assignment_id: ID(row.assignment_id.to_string()),
            user_id: ID(row.user_id.to_string()),
            attempt: row.attempt,
            body: row.body,
            score: row.score,
            state: row.workflow_state,
            submission_type: row.submission_type,
            submitted_at: row.submitted_at,
            graded_at: row.graded_at,
            excused: row.excused,
            late_policy_status: row.late_policy_status,
            deducted_points: row.points_deducted,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionEdge {
    pub cursor: String,
    pub node: Submission,
}

/// Relay connection for submissions.
#[derive(Debug, Clone, SimpleObject)]
pub struct SubmissionConnection {
    pub edges: Vec<SubmissionEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetch (same pagination shape as fetch_assignment_page)
// ---------------------------------------------------------------------------

/// Resolve one page of an assignment's submissions for a viewer, applying the
/// `enrollmentsConnection` roster rule: admins and course teachers see every
/// submission, anyone else sees only their own.
pub(crate) async fn fetch_visible_submission_page(
    pool: &PgPool,
    viewer_id: i64,
    viewer_is_admin: bool,
    assignment_id: i64,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<SubmissionConnection, async_graphql::Error> {
    let scoped_to_self =
        !viewer_is_admin && !user_teaches_course(pool, viewer_id, course_id).await?;
    let user_id = if scoped_to_self {
        Some(viewer_id)
    } else {
        None
    };

    fetch_submission_page(pool, assignment_id, user_id, first, after).await
}

/// Fetch one page of an assignment's submissions: `LIMIT first+1` ordered by
/// id ascending, optionally scoped to one user, optionally resuming after a
/// cursor. The extra row drives `hasNextPage`. No `workflow_state` filter:
/// Canvas's default states exist to exclude unsubmitted placeholders, which
/// star-lms does not store (REST parity with the star-assignments handlers).
pub(crate) async fn fetch_submission_page(
    pool: &PgPool,
    assignment_id: i64,
    user_id: Option<i64>,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<SubmissionConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<SubmissionRow> = sqlx::query_as(
        r#"
        SELECT id, assignment_id, user_id, attempt, body, score::float8 AS score,
               submission_type, workflow_state, submitted_at, graded_at, excused,
               late_policy_status, points_deducted::float8 AS points_deducted,
               created_at, updated_at
        FROM submissions
        WHERE assignment_id = $1
          AND ($2::bigint IS NULL OR user_id = $2)
          AND ($3::bigint IS NULL OR id > $3)
        ORDER BY id
        LIMIT $4
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<SubmissionEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            SubmissionEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(SubmissionConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

// Copied verbatim from lib.rs (user_teaches_course) to keep the lib.rs diff
// of this slice minimal across the parallel gql-foundation branches, the same
// approach as terms_sections.rs.
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
