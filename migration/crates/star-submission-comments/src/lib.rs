//! star-submission-comments: Canvas API v1 submission comments for star-lms.
//!
//! Endpoints (Canvas API v1 parity):
//! - POST   /submissions/{submissionId}/comments  {comment} → 201
//! - GET    /submissions/{submissionId}/comments → [comments], active only, ordered by created_at
//! - DELETE /submissions/{submissionId}/comments/{commentId} → 204, soft-delete
//!
//! Canvas nested aliases (resolve submission by course/assignment/user):
//! - GET, POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments
//! - DELETE    /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}
//!
//! Comment read state (Canvas mark-read parity):
//! - PUT /submissions/{submissionId}/comments/read → 204, marks the thread read
//! - GET /submissions/{submissionId}/comments/unread_count → { "unread_count": n }
//!
//! Access control (Canvas-style):
//! - POST: course teacher/admin OR the submission's own student
//! - GET:  course teacher/admin OR the submission's own student
//! - DELETE: author of the comment or admin
//! - read state: same audience as GET (teacher/admin or own student)

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
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

/// Unread comment count response (Canvas `unread_count` shape).
#[derive(Debug, Clone, Serialize)]
pub struct UnreadCount {
    pub unread_count: i64,
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

/// Resolve a submission id from Canvas's nested path context
/// (course, assignment, user). Returns 404 when the submission is missing,
/// or when its assignment does not belong to the given course.
async fn resolve_submission_id(
    pool: &PgPool,
    course_id: i64,
    assignment_id: i64,
    user_id: i64,
) -> Result<i64, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT s.id
        FROM submissions s
        JOIN assignments a ON a.id = s.assignment_id
        WHERE s.assignment_id = $1 AND s.user_id = $2 AND a.course_id = $3
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    row.map(|r| r.0).ok_or(ApiError::NotFound)
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

/// Shared create logic for the flat and Canvas-nested routes.
/// Course teacher/admin OR the submission's own student → 201
async fn create_comment_for(
    state: &AppState,
    submission_id: i64,
    auth: AuthUser,
    payload: CreateComment,
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

/// Verify the submission exists and the caller may read/post its comments.
/// 404 when the submission is missing, 403 when access is denied.
async fn authorize_comment_thread(
    state: &AppState,
    submission_id: i64,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    // Verify the submission exists (404 if not)
    let _course_id = get_course_id_for_submission(&state.db, submission_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // Access check
    if !check_can_view_or_comment(&state.db, auth.id, submission_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    Ok(())
}

/// POST /submissions/{submissionId}/comments
async fn create_comment(
    State(state): State<AppState>,
    Path(submission_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateComment>,
) -> Result<(StatusCode, Json<SubmissionComment>), ApiError> {
    create_comment_for(&state, submission_id, auth, payload).await
}

/// POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments
async fn create_comment_nested(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateComment>,
) -> Result<(StatusCode, Json<SubmissionComment>), ApiError> {
    let submission_id = resolve_submission_id(&state.db, course_id, assignment_id, user_id).await?;
    create_comment_for(&state, submission_id, auth, payload).await
}

/// Shared list logic for the flat and Canvas-nested routes.
/// Course teacher/admin OR the submission's own student → [comments], active only, ordered by created_at
async fn list_comments_for(
    state: &AppState,
    submission_id: i64,
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

/// GET /submissions/{submissionId}/comments
async fn list_comments(
    State(state): State<AppState>,
    Path(submission_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<SubmissionComment>>, ApiError> {
    list_comments_for(&state, submission_id, auth).await
}

/// GET /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments
async fn list_comments_nested(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<SubmissionComment>>, ApiError> {
    let submission_id = resolve_submission_id(&state.db, course_id, assignment_id, user_id).await?;
    list_comments_for(&state, submission_id, auth).await
}

/// Shared delete logic for the flat and Canvas-nested routes.
/// Author or admin → soft-delete (workflow_state = 'deleted'), 204
async fn delete_comment_for(
    state: &AppState,
    submission_id: i64,
    comment_id: i64,
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

/// DELETE /submissions/{submissionId}/comments/{commentId}
async fn delete_comment(
    State(state): State<AppState>,
    Path((submission_id, comment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    delete_comment_for(&state, submission_id, comment_id, auth).await
}

/// DELETE /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}
async fn delete_comment_nested(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id, comment_id)): Path<(i64, i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let submission_id = resolve_submission_id(&state.db, course_id, assignment_id, user_id).await?;
    delete_comment_for(&state, submission_id, comment_id, auth).await
}

// ---------------------------------------------------------------------------
// Comment read state
// ---------------------------------------------------------------------------

/// Mark the whole comment thread read for the caller (Canvas mark-read parity).
/// Stores the highest active comment id at mark time, so later comments count
/// as unread again.
async fn mark_comments_read_for(
    state: &AppState,
    submission_id: i64,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    authorize_comment_thread(state, submission_id, &auth).await?;

    sqlx::query(
        r#"
        INSERT INTO submission_comment_reads
            (submission_id, user_id, last_read_comment_id, read_at, updated_at)
        VALUES (
            $1,
            $2,
            (SELECT MAX(id) FROM submission_comments
              WHERE submission_id = $1 AND workflow_state = 'active'),
            now(),
            now()
        )
        ON CONFLICT (submission_id, user_id) DO UPDATE
        SET last_read_comment_id = EXCLUDED.last_read_comment_id,
            read_at = now(),
            updated_at = now()
        "#,
    )
    .bind(submission_id)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// PUT /submissions/{submissionId}/comments/read
async fn mark_comments_read(
    State(state): State<AppState>,
    Path(submission_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    mark_comments_read_for(&state, submission_id, auth).await
}

/// Count unread comments for the caller: active comments authored by someone
/// else with an id greater than the last read marker (all when never read).
/// Comments the caller wrote never count as unread (Canvas parity).
async fn unread_count_for(
    state: &AppState,
    submission_id: i64,
    auth: &AuthUser,
) -> Result<Json<UnreadCount>, ApiError> {
    authorize_comment_thread(state, submission_id, auth).await?;

    let (unread_count,): (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)::BIGINT
        FROM submission_comments c
        WHERE c.submission_id = $1
          AND c.workflow_state = 'active'
          AND c.author_id <> $2
          AND c.id > COALESCE(
              (SELECT r.last_read_comment_id
                 FROM submission_comment_reads r
                WHERE r.submission_id = $1 AND r.user_id = $2),
              0
          )
        "#,
    )
    .bind(submission_id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(UnreadCount { unread_count }))
}

/// GET /submissions/{submissionId}/comments/unread_count
async fn comments_unread_count(
    State(state): State<AppState>,
    Path(submission_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<UnreadCount>, ApiError> {
    unread_count_for(&state, submission_id, &auth).await
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
        // Comment read state
        .route(
            "/submissions/{submissionId}/comments/read",
            put(mark_comments_read),
        )
        .route(
            "/submissions/{submissionId}/comments/unread_count",
            get(comments_unread_count),
        )
        // Canvas nested aliases — resolve submission by course/assignment/user
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments",
            post(create_comment_nested),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments",
            get(list_comments_nested),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}",
            delete(delete_comment_nested),
        )
}
