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
//! Single comment + attachments (Canvas SubmissionCommentsApiController):
//! - POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/files
//!   → 201, metadata-only attachment record (no binary upload/storage)
//! - GET /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}
//!   → CommentDetail (also available flat: /submissions/{submissionId}/comments/{commentId})
//! - PUT   same paths → 200, edit the comment text (sets `edited_at` when it changes)
//! - DELETE nested path → 204, soft-delete (existing behavior)
//!
//! Access control (Canvas-style):
//! - POST: course teacher/admin OR the submission's own student
//! - GET:  course teacher/admin OR the submission's own student
//! - DELETE: author of the comment or admin
//! - PUT (edit): author of the comment or admin
//! - read state: same audience as GET (teacher/admin or own student)
//! - comment files upload: same audience as POST (teacher/admin or own student)

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
    /// Set when an existing comment's text is edited (Canvas `edited_at`).
    pub edited_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating a comment.
#[derive(Debug, Deserialize)]
pub struct CreateComment {
    pub comment: String,
    /// Optional metadata-only attachments (from the comments/files upload) to
    /// link to the new comment. Canvas attaches comment files the same way.
    #[serde(default)]
    pub file_ids: Vec<i64>,
}

/// Request body for editing a comment (Canvas permits `comment` only).
#[derive(Debug, Deserialize)]
pub struct UpdateComment {
    pub comment: String,
}

/// Metadata-only comment attachment record.
/// Canvas shapes these as `attachments` inside submission comment JSON.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CommentAttachment {
    pub id: i64,
    pub submission_id: i64,
    pub comment_id: Option<i64>,
    pub filename: String,
    pub display_name: String,
    pub content_type: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for a metadata-only comment file upload.
#[derive(Debug, Deserialize)]
pub struct CreateCommentFile {
    pub filename: String,
    pub display_name: Option<String>,
    pub content_type: Option<String>,
    pub size: Option<i64>,
    /// Attach the upload to an existing comment of this submission.
    pub comment_id: Option<i64>,
}

/// Single-comment payload: Canvas submission_comment_json fields plus the
/// attachment records linked to the comment.
#[derive(Debug, Clone, Serialize)]
pub struct CommentDetail {
    pub id: i64,
    pub submission_id: i64,
    pub author_id: i64,
    pub author_name: Option<String>,
    pub comment: String,
    pub workflow_state: String,
    pub edited_at: Option<DateTime<Utc>>,
    pub attachments: Vec<CommentAttachment>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Row shape for the single-comment join (author name).
#[derive(Debug, Clone, sqlx::FromRow)]
struct CommentRow {
    id: i64,
    submission_id: i64,
    author_id: i64,
    author_name: Option<String>,
    comment: String,
    workflow_state: String,
    edited_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
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
    Ok(owner_id == Some(user_id))
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
        RETURNING id, submission_id, author_id, comment, workflow_state, edited_at, created_at, updated_at
        "#,
    )
    .bind(submission_id)
    .bind(auth.id)
    .bind(&payload.comment)
    .fetch_one(&state.db)
    .await?;

    // Link any metadata-only uploads the caller references (Canvas attaches
    // comment files the same way). Unknown/foreign ids are ignored, matching
    // Canvas, which only keeps attachments the author may attach.
    link_attachments(&state.db, submission_id, row.id, &payload.file_ids).await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// Attach unattached comment-file records of this submission to a comment.
async fn link_attachments(
    pool: &PgPool,
    submission_id: i64,
    comment_id: i64,
    file_ids: &[i64],
) -> Result<(), ApiError> {
    if file_ids.is_empty() {
        return Ok(());
    }

    sqlx::query(
        r#"
        UPDATE submission_comment_attachments
        SET comment_id = $1, updated_at = now()
        WHERE id = ANY($2) AND submission_id = $3 AND comment_id IS NULL
        "#,
    )
    .bind(comment_id)
    .bind(file_ids)
    .bind(submission_id)
    .execute(pool)
    .await?;

    Ok(())
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
        SELECT id, submission_id, author_id, comment, workflow_state, edited_at, created_at, updated_at
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
// Single comment read + edit (Canvas SubmissionCommentsApiController)
// ---------------------------------------------------------------------------

/// List the metadata-only attachments linked to a comment.
async fn list_comment_attachments(
    pool: &PgPool,
    comment_id: i64,
) -> Result<Vec<CommentAttachment>, ApiError> {
    let rows = sqlx::query_as::<_, CommentAttachment>(
        r#"
        SELECT id, submission_id, comment_id, filename, display_name, content_type, size, created_at, updated_at
        FROM submission_comment_attachments
        WHERE comment_id = $1
        ORDER BY id ASC
        "#,
    )
    .bind(comment_id)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Load one active comment of a submission in Canvas submission_comment_json
/// shape (author name + attachments). 404 when missing or soft-deleted.
async fn load_comment_detail(
    pool: &PgPool,
    submission_id: i64,
    comment_id: i64,
) -> Result<CommentDetail, ApiError> {
    let row: Option<CommentRow> = sqlx::query_as(
        r#"
        SELECT c.id, c.submission_id, c.author_id, u.name AS author_name,
               c.comment, c.workflow_state, c.edited_at, c.created_at, c.updated_at
        FROM submission_comments c
        LEFT JOIN users u ON u.id = c.author_id
        WHERE c.id = $1 AND c.submission_id = $2 AND c.workflow_state = 'active'
        "#,
    )
    .bind(comment_id)
    .bind(submission_id)
    .fetch_optional(pool)
    .await?;

    let row = row.ok_or(ApiError::NotFound)?;
    let attachments = list_comment_attachments(pool, comment_id).await?;

    Ok(CommentDetail {
        id: row.id,
        submission_id: row.submission_id,
        author_id: row.author_id,
        author_name: row.author_name,
        comment: row.comment,
        workflow_state: row.workflow_state,
        edited_at: row.edited_at,
        attachments,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

/// Shared single-comment read logic for the flat and Canvas-nested routes.
/// Course teacher/admin OR the submission's own student → comment detail.
async fn get_comment_for(
    state: &AppState,
    submission_id: i64,
    comment_id: i64,
    auth: AuthUser,
) -> Result<Json<CommentDetail>, ApiError> {
    authorize_comment_thread(state, submission_id, &auth).await?;
    Ok(Json(
        load_comment_detail(&state.db, submission_id, comment_id).await?,
    ))
}

/// GET /submissions/{submissionId}/comments/{commentId}
async fn get_comment(
    State(state): State<AppState>,
    Path((submission_id, comment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<CommentDetail>, ApiError> {
    get_comment_for(&state, submission_id, comment_id, auth).await
}

/// GET /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}
async fn get_comment_nested(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id, comment_id)): Path<(i64, i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<CommentDetail>, ApiError> {
    let submission_id = resolve_submission_id(&state.db, course_id, assignment_id, user_id).await?;
    get_comment_for(&state, submission_id, comment_id, auth).await
}

/// Shared comment edit logic for the flat and Canvas-nested routes.
/// Canvas only lets the comment author edit it; star-lms also allows admin,
/// matching the crate's existing delete rule. `edited_at` is stamped whenever
/// the text actually changes (Canvas SubmissionComment#set_edited_at).
async fn update_comment_for(
    state: &AppState,
    submission_id: i64,
    comment_id: i64,
    auth: AuthUser,
    payload: UpdateComment,
) -> Result<Json<CommentDetail>, ApiError> {
    authorize_comment_thread(state, submission_id, &auth).await?;

    let row: Option<(i64, String)> = sqlx::query_as(
        r#"SELECT author_id, workflow_state FROM submission_comments WHERE id = $1 AND submission_id = $2"#,
    )
    .bind(comment_id)
    .bind(submission_id)
    .fetch_optional(&state.db)
    .await?;

    let (author_id, workflow_state) = row.ok_or(ApiError::NotFound)?;
    if workflow_state != "active" {
        return Err(ApiError::NotFound);
    }

    if !is_admin(&auth) && author_id != auth.id {
        return Err(ApiError::Forbidden);
    }

    if payload.comment.trim().is_empty() {
        return Err(ApiError::Validation("comment cannot be empty".into()));
    }

    sqlx::query(
        r#"
        UPDATE submission_comments
        SET comment = $3,
            edited_at = CASE WHEN comment IS DISTINCT FROM $3 THEN now() ELSE edited_at END,
            updated_at = now()
        WHERE id = $1 AND submission_id = $2
        "#,
    )
    .bind(comment_id)
    .bind(submission_id)
    .bind(&payload.comment)
    .execute(&state.db)
    .await?;

    Ok(Json(
        load_comment_detail(&state.db, submission_id, comment_id).await?,
    ))
}

/// PUT /submissions/{submissionId}/comments/{commentId}
async fn update_comment(
    State(state): State<AppState>,
    Path((submission_id, comment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateComment>,
) -> Result<Json<CommentDetail>, ApiError> {
    update_comment_for(&state, submission_id, comment_id, auth, payload).await
}

/// PUT /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}
async fn update_comment_nested(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id, comment_id)): Path<(i64, i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateComment>,
) -> Result<Json<CommentDetail>, ApiError> {
    let submission_id = resolve_submission_id(&state.db, course_id, assignment_id, user_id).await?;
    update_comment_for(&state, submission_id, comment_id, auth, payload).await
}

// ---------------------------------------------------------------------------
// Comment attachments (Canvas SubmissionCommentsApiController#create_file)
// ---------------------------------------------------------------------------

/// Create a metadata-only comment attachment record for a submission.
/// Canvas runs a two-step upload here; star-lms records the metadata and
/// returns the attachment shape (no binary upload / storage handling).
///
/// `comment_id`, when given, must be an active comment of this submission.
/// Otherwise the record stays unattached until a comment references it via
/// `file_ids` (Canvas attaches those ids when the comment is written).
async fn upload_comment_file_for(
    state: &AppState,
    submission_id: i64,
    auth: AuthUser,
    payload: CreateCommentFile,
) -> Result<(StatusCode, Json<CommentAttachment>), ApiError> {
    // Same audience as commenting: teacher/admin or the submission's student.
    if !check_can_view_or_comment(&state.db, auth.id, submission_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.filename.trim().is_empty() {
        return Err(ApiError::Validation("filename is required".into()));
    }

    if let Some(comment_id) = payload.comment_id {
        let found: Option<(i32,)> = sqlx::query_as(
            r#"
            SELECT 1 FROM submission_comments
            WHERE id = $1 AND submission_id = $2 AND workflow_state = 'active'
            "#,
        )
        .bind(comment_id)
        .bind(submission_id)
        .fetch_optional(&state.db)
        .await?;
        if found.is_none() {
            return Err(ApiError::NotFound);
        }
    }

    // Canvas infer_display_name: display_name ||= filename.
    let display_name = match payload.display_name {
        Some(ref d) if !d.trim().is_empty() => d.trim().to_string(),
        _ => payload.filename.clone(),
    };
    let content_type = payload
        .content_type
        .unwrap_or_else(|| "application/octet-stream".into());
    let size = payload.size.unwrap_or(0).max(0);

    let row = sqlx::query_as::<_, CommentAttachment>(
        r#"
        INSERT INTO submission_comment_attachments
            (submission_id, comment_id, uploader_id, filename, display_name, content_type, size)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, submission_id, comment_id, filename, display_name, content_type, size, created_at, updated_at
        "#,
    )
    .bind(submission_id)
    .bind(payload.comment_id)
    .bind(auth.id)
    .bind(&payload.filename)
    .bind(&display_name)
    .bind(&content_type)
    .bind(size)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/files
async fn upload_comment_file_nested(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateCommentFile>,
) -> Result<(StatusCode, Json<CommentAttachment>), ApiError> {
    let submission_id = resolve_submission_id(&state.db, course_id, assignment_id, user_id).await?;
    upload_comment_file_for(&state, submission_id, auth, payload).await
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
            get(get_comment),
        )
        .route(
            "/submissions/{submissionId}/comments/{commentId}",
            put(update_comment),
        )
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
        // Metadata-only comment file upload (Canvas SubmissionCommentsApiController#create_file)
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/files",
            post(upload_comment_file_nested),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}",
            get(get_comment_nested),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}",
            put(update_comment_nested),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}",
            delete(delete_comment_nested),
        )
}
