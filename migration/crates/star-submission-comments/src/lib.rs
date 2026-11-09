//! star-submission-comments: Canvas API v1 submission comments for star-lms.
//!
//! Endpoints (Canvas API v1 parity):
//! - POST   /submissions/{submissionId}/comments  {comment} → 201
//! - GET    /submissions/{submissionId}/comments → [comments], active only, ordered by created_at
//! - DELETE /submissions/{submissionId}/comments/{commentId} → 204, soft-delete
//!
//! Access control (Canvas-style):
//! - POST: course teacher/admin OR the submission's own student
//! - GET:  course teacher/admin OR the submission's own student
//! - DELETE: author of the comment or admin

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// SubmissionComment response shape (Canvas API v1 compatible).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SubmissionComment {
    pub id: i64,
    pub submission_id: i64,
    pub author_id: i64,
    pub comment: String,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating a comment.
#[derive(Debug, Deserialize)]
pub struct CreateComment {
    pub comment: String,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Check if user is a teacher in a course or admin.
async fn check_teacher_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

/// Fetch the course_id for a given submission.
/// Returns None if the submission does not exist.
async fn get_course_id_for_submission(
    pool: &PgPool,
    submission_id: i64,
) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT a.course_id
        FROM submissions s
        JOIN assignments a ON a.id = s.assignment_id
        WHERE s.id = $1
        "#,
    )
    .bind(submission_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.0))
}

/// Fetch the submission owner (user_id) for a given submission.
async fn get_submission_owner(pool: &PgPool, submission_id: i64) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT user_id FROM submissions WHERE id = $1"#)
        .bind(submission_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| r.0))
}

/// Check if user can view/submit comments on a submission:
/// - course teacher/admin OR the submission's own student.
async fn check_can_view_or_comment(
    pool: &PgPool,
    user_id: i64,
    submission_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let course_id = match get_course_id_for_submission(pool, submission_id).await? {
        Some(cid) => cid,
        None => return Err(ApiError::NotFound),
    };

    // Teacher in the course?
    if check_teacher_or_admin(pool, user_id, course_id, auth).await? {
        return Ok(true);
    }

    // Or the submission's own student?
    let owner_id = get_submission_owner(pool, submission_id).await?;
    if owner_id == Some(user_id) {
        return Ok(true);
    }

    Ok(false)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /submissions/{submissionId}/comments
/// Course teacher/admin OR the submission's own student → 201
async fn create_comment(
    State(state): State<AppState>,
    Path(submission_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateComment>,
) -> Result<(StatusCode, Json<SubmissionComment>), ApiError> {
    // Verify the submission exists (404 if not)
    let _course_id = get_course_id_for_submission(&state.db, submission_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Access check
    if !check_can_view_or_comment(&state.db, auth.id, submission_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.comment.trim().is_empty() {
        return Err(ApiError::Validation("comment cannot be empty".into()));
    }

    let row: SubmissionComment = sqlx::query_as(
        r#"
        INSERT INTO submission_comments (submission_id, author_id, comment)
        VALUES ($1, $2, $3)
        RETURNING id, submission_id, author_id, comment, workflow_state, created_at, updated_at
        "#,
    )
    .bind(submission_id)
    .bind(auth.id)
    .bind(&payload.comment)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /submissions/{submissionId}/comments
/// Course teacher/admin OR the submission's own student → [comments], active only, ordered by created_at
async fn list_comments(
    State(state): State<AppState>,
    Path(submission_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<SubmissionComment>>, ApiError> {
    // Verify the submission exists (404 if not)
    let _course_id = get_course_id_for_submission(&state.db, submission_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Access check
    if !check_can_view_or_comment(&state.db, auth.id, submission_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<SubmissionComment> = sqlx::query_as(
        r#"
        SELECT id, submission_id, author_id, comment, workflow_state, created_at, updated_at
        FROM submission_comments
        WHERE submission_id = $1 AND workflow_state = 'active'
        ORDER BY created_at ASC
        "#,
    )
    .bind(submission_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// DELETE /submissions/{submissionId}/comments/{commentId}
/// Author or admin → soft-delete (workflow_state = 'deleted'), 204
async fn delete_comment(
    State(state): State<AppState>,
    Path((submission_id, comment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Fetch the comment to verify it exists and get its author
    let row: Option<(i64,)> = sqlx::query_as(
        r#"SELECT author_id FROM submission_comments WHERE id = $1 AND submission_id = $2"#,
    )
    .bind(comment_id)
    .bind(submission_id)
    .fetch_optional(&state.db)
    .await?;

    let (author_id,) = row.ok_or(ApiError::NotFound)?;

    // Access check: author or admin
    if !is_admin(&auth) && author_id != auth.id {
        return Err(ApiError::Forbidden);
    }

    // Soft-delete (Canvas parity)
    sqlx::query(
        r#"UPDATE submission_comments SET workflow_state = 'deleted', updated_at = now() WHERE id = $1"#,
    )
    .bind(comment_id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/submissions/{submissionId}/comments", post(create_comment))
        .route("/submissions/{submissionId}/comments", get(list_comments))
        .route(
            "/submissions/{submissionId}/comments/{commentId}",
            delete(delete_comment),
        )
}
