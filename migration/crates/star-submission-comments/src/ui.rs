//! Pub data loaders for the server-rendered grading page (star-ui, issue
//! #108).
//!
//! Same extraction pattern as `star_conversations::ui`: the SpeedGrader
//! comment feed needs author names joined with the comment rows — the REST
//! API's `SubmissionComment` shape only carries ids — so the query lives
//! here as a `pub` fn. The comment write reuses the REST create path's
//! exact validation ("comment cannot be empty") and INSERT shape via
//! [`create_comment_record`], so the shell cannot fork it.
//!
//! Authorization stays with the caller: the shell gates the whole grading
//! page through `star_courses::check_teacher_or_admin` before any of these
//! run (the REST handlers' teacher-or-own-student rule is therefore
//! strictly narrowed to teachers for the grading UI).

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use star_common::ApiError;

/// One comment row for the grading page feed, with the author's display
/// name (the REST `SubmissionComment` shape carries ids only).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CommentFeedRow {
    pub id: i64,
    pub submission_id: i64,
    pub author_id: i64,
    /// `users.name` of the author (NULL for a deleted user).
    pub author_name: Option<String>,
    pub comment: String,
    pub created_at: DateTime<Utc>,
}

/// Load the active comment feed of a submission, oldest first — the ordering
/// Canvas' `include[]=submission_comments` feed uses.
pub async fn load_submission_comment_feed(
    db: &PgPool,
    submission_id: i64,
) -> Result<Vec<CommentFeedRow>, ApiError> {
    let rows: Vec<CommentFeedRow> = sqlx::query_as(
        r#"
        SELECT c.id, c.submission_id, c.author_id, u.name AS author_name,
               c.comment, c.created_at
        FROM submission_comments c
        LEFT JOIN users u ON u.id = c.author_id
        WHERE c.submission_id = $1 AND c.workflow_state = 'active'
        ORDER BY c.created_at ASC
        "#,
    )
    .bind(submission_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Create a comment on a submission — the shell's write path for the
/// SpeedGrader comment box.
///
/// Same validation ("comment cannot be empty") and same INSERT shape as the
/// shared comment write in star-assignments (issue #55 crate); file attachments
/// are not part of the form UI, so `file_ids` is not modeled here.
pub async fn create_comment_record(
    db: &PgPool,
    submission_id: i64,
    author_id: i64,
    comment: &str,
) -> Result<i64, ApiError> {
    if comment.trim().is_empty() {
        return Err(ApiError::Validation("comment cannot be empty".into()));
    }

    let (id,): (i64,) = sqlx::query_as(
        r#"
        INSERT INTO submission_comments (submission_id, author_id, comment)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(submission_id)
    .bind(author_id)
    .bind(comment.trim())
    .fetch_one(db)
    .await?;
    Ok(id)
}
