//! Quiz, quiz submission and quiz question read types +
//! `Course.quizzesConnection` (issue #78).
//!
//! Mirrors the conventions of the merged slices in `lib.rs` /
//! `assignment.rs` / `terms_sections.rs`: camelCase field names
//! (`courseId`, `pointsPossible`, `quizType`), relay-style connections with
//! `edges { cursor node }` + `pageInfo { hasNextPage endCursor }`, opaque
//! cursors, `LIMIT first+1` ordered by id.
//!
//! Canvas shape references: `app/graphql/types/quiz_type.rb`,
//! `quiz_submission_type.rb` and `quiz_item_type.rb` in canvas-lms. Canvas
//! fields without a backing star-lms column are omitted (documented per type
//! below): `anonymousSubmissions` (no column), `quizPointsPossible` /
//! `quizVersion` / `versionNumber` / `manuallyScored` / `extraAttempts` /
//! `extraTime` on QuizSubmission are exposed where columns exist (0068) and
//! omitted where they do not (`quizVersion`, `versionNumber`,
//! `manuallyScored`); Canvas's `QuizItem` is exposed as `QuizQuestion`
//! because star-lms models question rows directly (migration 0037), not
//! quiz items.
//!
//! Authorization:
//! - `Course.quizzesConnection`: visibility is inherited from
//!   `course(id)` — enrolled users (any role) and admins; everyone else gets
//!   a null course and never reaches this field (same gate as
//!   `assignmentsConnection`).
//! - `Quiz.submissionsConnection`: self-or-teacher-or-admin, like
//!   `User.enrollmentsConnection` — a student sees only their own
//!   submissions (scoped to their user id), teachers of the course and
//!   admins see all of them, unenrolled non-admins get null.
//! - `Quiz.questionsConnection`: quiz content is visible to anyone who can
//!   resolve the quiz (enrolled users of the course and admins), mirroring
//!   the star-quizzes REST handler.
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
// -----------------------------------------------------------------------------

/// Row of the `quizzes` table (migrations 0006 + 0015). `points_possible`
/// is NUMERIC(10,2) and cast to float8 in the queries, like `assignment.rs`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct QuizRow {
    id: i64,
    course_id: i64,
    title: String,
    description: Option<String>,
    quiz_type: String,
    points_possible: f64,
    time_limit: Option<i32>,
    shuffle_answers: bool,
    allowed_attempts: i32,
    published: bool,
    available_from: Option<DateTime<Utc>>,
    available_until: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Row of the `quiz_submissions` table (migrations 0006 + 0068).
#[derive(Debug, Clone, sqlx::FromRow)]
struct QuizSubmissionRow {
    id: i64,
    quiz_id: i64,
    user_id: i64,
    attempt: i32,
    score: Option<f64>,
    kept_score: Option<f64>,
    fudge_points: Option<f64>,
    workflow_state: String,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// Row of the `quiz_questions` table (migration 0037).
#[derive(Debug, Clone, sqlx::FromRow)]
struct QuizQuestionRow {
    id: i64,
    quiz_id: i64,
    position: i32,
    question_name: String,
    question_type: String,
    question_text: String,
    points_possible: f64,
    answers: String,
    workflow_state: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// GraphQL types
// -----------------------------------------------------------------------------

/// Canvas GraphQL `Quiz` (subset of `app/graphql/types/quiz_type.rb`).
/// Method names are camelCased by the Object derive: `id`, `courseId`,
/// `title`, `description`, `quizType`, `pointsPossible`, `timeLimit`,
/// `shuffleAnswers`, `allowedAttempts`, `published`, `availableFrom`,
/// `availableUntil`, `createdAt`, `updatedAt`, `submissionsConnection`,
/// `questionsConnection`. Omitted Canvas fields without backing columns:
/// `anonymousSubmissions` and the node/module-item interfaces.
#[derive(Debug, Clone)]
pub struct Quiz {
    id: i64,
    course_id: i64,
    title: String,
    description: Option<String>,
    quiz_type: String,
    points_possible: f64,
    time_limit: Option<i32>,
    shuffle_answers: bool,
    allowed_attempts: i32,
    published: bool,
    available_from: Option<DateTime<Utc>>,
    available_until: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[Object]
impl Quiz {
    async fn id(&self) -> ID {
        ID(self.id.to_string())
    }

    async fn course_id(&self) -> ID {
        ID(self.course_id.to_string())
    }

    async fn title(&self) -> &str {
        &self.title
    }

    async fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Canvas `Quiz.quizType`: 'quiz' for regular quizzes, 'assignment' for
    /// LTI quiz assignments.
    async fn quiz_type(&self) -> &str {
        &self.quiz_type
    }

    async fn points_possible(&self) -> f64 {
        self.points_possible
    }

    async fn time_limit(&self) -> Option<i32> {
        self.time_limit
    }

    async fn shuffle_answers(&self) -> bool {
        self.shuffle_answers
    }

    async fn allowed_attempts(&self) -> i32 {
        self.allowed_attempts
    }

    async fn published(&self) -> bool {
        self.published
    }

    async fn available_from(&self) -> Option<DateTime<Utc>> {
        self.available_from
    }

    async fn available_until(&self) -> Option<DateTime<Utc>> {
        self.available_until
    }

    async fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    async fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    /// Submissions for this quiz: self-or-teacher-or-admin, like
    /// `User.enrollmentsConnection`. A student sees only their own
    /// submissions; teachers of the course and admins see all; unenrolled
    /// non-admins get null instead of the connection.
    async fn submissions_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<Option<QuizSubmissionConnection>, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;

        let is_admin = gctx.is_admin();
        let teaches = user_teaches_course(&gctx.pool, gctx.auth_user.id, self.course_id).await?;
        if !is_admin
            && !teaches
            && !user_enrolled_in_course(&gctx.pool, gctx.auth_user.id, self.course_id).await?
        {
            return Ok(None);
        }

        let user_id = if is_admin || teaches {
            None
        } else {
            Some(gctx.auth_user.id)
        };

        Ok(Some(
            fetch_quiz_submission_page(&gctx.pool, self.id, user_id, first, after.as_deref())
                .await?,
        ))
    }

    /// Questions of this quiz (LIMIT first+1, ORDER BY id). Quiz content is
    /// visible to anyone who can resolve the quiz (enrolled users of the
    /// course and admins), mirroring the star-quizzes REST handler; there is
    /// no published/hidden split to enforce because question visibility is
    /// driven by the enclosing quiz.
    async fn questions_connection(
        &self,
        ctx: &Context<'_>,
        first: Option<i32>,
        after: Option<String>,
    ) -> Result<QuizQuestionConnection, async_graphql::Error> {
        let gctx = ctx.data::<GraphQLContext>()?;
        Ok(fetch_quiz_question_page(&gctx.pool, self.id, first, after.as_deref()).await?)
    }
}

/// Canvas GraphQL `QuizSubmission` (subset of
/// `app/graphql/types/quiz_submission_type.rb`). Field names are camelCased
/// by the SimpleObject derive: `id`, `quizId`, `userId`, `attempt`, `score`,
/// `keptScore`, `fudgePoints`, `workflowState`, `startedAt`, `finishedAt`,
/// `completedAt`, `createdAt`, `updatedAt`. Omitted Canvas fields without
/// backing columns: `quizPointsPossible`, `quizVersion`, `versionNumber`,
/// `manuallyScored`; `extraAttempts`/`extraTime` (0068) and `manuallyUnlocked`
/// exist in the schema but are not exposed by this read-only slice.
#[derive(Debug, Clone, SimpleObject)]
pub struct QuizSubmission {
    pub id: ID,
    pub quiz_id: ID,
    pub user_id: ID,
    pub attempt: i32,
    pub score: Option<f64>,
    pub kept_score: Option<f64>,
    pub fudge_points: Option<f64>,
    pub workflow_state: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<QuizSubmissionRow> for QuizSubmission {
    fn from(row: QuizSubmissionRow) -> Self {
        QuizSubmission {
            id: ID(row.id.to_string()),
            quiz_id: ID(row.quiz_id.to_string()),
            user_id: ID(row.user_id.to_string()),
            attempt: row.attempt,
            score: row.score,
            kept_score: row.kept_score,
            fudge_points: row.fudge_points,
            workflow_state: row.workflow_state,
            started_at: row.started_at,
            finished_at: row.finished_at,
            completed_at: row.completed_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Canvas GraphQL `QuizItem` equivalent: star-lms models question rows
/// directly (`quiz_questions`, migration 0037), so the type is named
/// `QuizQuestion`. Field names are camelCased by the SimpleObject derive:
/// `id`, `quizId`, `position`, `questionName`, `questionType`,
/// `questionText`, `pointsPossible`, `answers`, `workflowState`,
/// `createdAt`, `updatedAt`. `answers` is the raw JSON payload Canvas also
/// stores as text.
#[derive(Debug, Clone, SimpleObject)]
pub struct QuizQuestion {
    pub id: ID,
    pub quiz_id: ID,
    pub position: i32,
    pub question_name: String,
    pub question_type: String,
    pub question_text: String,
    pub points_possible: f64,
    pub answers: String,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<QuizQuestionRow> for QuizQuestion {
    fn from(row: QuizQuestionRow) -> Self {
        QuizQuestion {
            id: ID(row.id.to_string()),
            quiz_id: ID(row.quiz_id.to_string()),
            position: row.position,
            question_name: row.question_name,
            question_type: row.question_type,
            question_text: row.question_text,
            points_possible: row.points_possible,
            answers: row.answers,
            workflow_state: row.workflow_state,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Relay connection edge: `edges { cursor node }`.
#[derive(Debug, Clone, SimpleObject)]
pub struct QuizEdge {
    pub cursor: String,
    pub node: Quiz,
}

/// Relay connection for quizzes.
#[derive(Debug, Clone, SimpleObject)]
pub struct QuizConnection {
    pub edges: Vec<QuizEdge>,
    pub page_info: PageInfo,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct QuizSubmissionEdge {
    pub cursor: String,
    pub node: QuizSubmission,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct QuizSubmissionConnection {
    pub edges: Vec<QuizSubmissionEdge>,
    pub page_info: PageInfo,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct QuizQuestionEdge {
    pub cursor: String,
    pub node: QuizQuestion,
}

#[derive(Debug, Clone, SimpleObject)]
pub struct QuizQuestionConnection {
    pub edges: Vec<QuizQuestionEdge>,
    pub page_info: PageInfo,
}

// ---------------------------------------------------------------------------
// Connection fetches (same pagination shape as fetch_assignment_page)
// -----------------------------------------------------------------------------

/// Fetch one page of a course's quizzes: `LIMIT first+1` ordered by id
/// ascending, optionally resuming after a cursor. The extra row drives
/// `hasNextPage`.
pub(crate) async fn fetch_quiz_page(
    pool: &PgPool,
    course_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<QuizConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<QuizRow> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, quiz_type,
               points_possible::float8 AS points_possible,
               time_limit, shuffle_answers, allowed_attempts, published,
               available_from, available_until, created_at, updated_at
        FROM quizzes
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

    let edges: Vec<QuizEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            QuizEdge {
                cursor,
                node: Quiz {
                    id: row.id,
                    course_id: row.course_id,
                    title: row.title,
                    description: row.description,
                    quiz_type: row.quiz_type,
                    points_possible: row.points_possible,
                    time_limit: row.time_limit,
                    shuffle_answers: row.shuffle_answers,
                    allowed_attempts: row.allowed_attempts,
                    published: row.published,
                    available_from: row.available_from,
                    available_until: row.available_until,
                    created_at: row.created_at,
                    updated_at: row.updated_at,
                },
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(QuizConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch one page of a quiz's submissions, optionally scoped to one user
/// (the self-only scope for students). `quiz_submissions` has
/// `UNIQUE (quiz_id, user_id)`, so the scoped connection holds at most one
/// edge; the connection shape is kept for Canvas parity.
async fn fetch_quiz_submission_page(
    pool: &PgPool,
    quiz_id: i64,
    user_id: Option<i64>,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<QuizSubmissionConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<QuizSubmissionRow> = sqlx::query_as(
        r#"
        SELECT id, quiz_id, user_id, attempt,
               score::float8 AS score,
               kept_score::float8 AS kept_score,
               fudge_points::float8 AS fudge_points,
               workflow_state, started_at, finished_at, completed_at,
               created_at, updated_at
        FROM quiz_submissions
        WHERE quiz_id = $1
          AND ($2::bigint IS NULL OR user_id = $2)
          AND ($3::bigint IS NULL OR id > $3)
        ORDER BY id
        LIMIT $4
        "#,
    )
    .bind(quiz_id)
    .bind(user_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<QuizSubmissionEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            QuizSubmissionEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(QuizSubmissionConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

/// Fetch one page of a quiz's questions: `LIMIT first+1` ordered by id
/// ascending (position order is applied client-side by consumers; the id
/// ordering matches the other connections).
async fn fetch_quiz_question_page(
    pool: &PgPool,
    quiz_id: i64,
    first: Option<i32>,
    after: Option<&str>,
) -> Result<QuizQuestionConnection, async_graphql::Error> {
    let page_size = first.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize;
    let after_id = match after {
        None => None,
        Some(cursor) => {
            Some(decode_cursor(cursor).ok_or_else(|| async_graphql::Error::new("invalid cursor"))?)
        }
    };

    let mut rows: Vec<QuizQuestionRow> = sqlx::query_as(
        r#"
        SELECT id, quiz_id, position, question_name, question_type,
               question_text, points_possible::float8 AS points_possible,
               answers, workflow_state, created_at, updated_at
        FROM quiz_questions
        WHERE quiz_id = $1
          AND ($2::bigint IS NULL OR id > $2)
        ORDER BY id
        LIMIT $3
        "#,
    )
    .bind(quiz_id)
    .bind(after_id)
    .bind((page_size + 1) as i64)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    let has_next_page = rows.len() > page_size;
    rows.truncate(page_size);

    let edges: Vec<QuizQuestionEdge> = rows
        .into_iter()
        .map(|row| {
            let cursor = encode_cursor(row.id);
            QuizQuestionEdge {
                cursor,
                node: row.into(),
            }
        })
        .collect();
    let end_cursor = edges.last().map(|edge| edge.cursor.clone());

    Ok(QuizQuestionConnection {
        edges,
        page_info: PageInfo {
            has_next_page,
            end_cursor,
        },
    })
}

// ---------------------------------------------------------------------------
// Permission helpers
// -----------------------------------------------------------------------------

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
