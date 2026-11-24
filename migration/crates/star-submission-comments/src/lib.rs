//! star-submission-comments: Canvas API v1 submission comments for star-lms.
//!
//! Endpoints (Canvas API v1 parity, canvas-src/config/routes.rb:1580-1586):
//! - PUT    /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}
//! - DELETE /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}
//!
//! Canvas declares no comments index or create route: the thread is read
//! through `include[]=submission_comments` on the submission and written
//! through `PUT .../submissions/{userId}` (both served by star-assignments),
//! so the flat `/submissions/{submissionId}/comments` GET/POST twins are not
//! registered here either.
//!
//! Single comment attachments (Canvas SubmissionCommentsApiController):
//! - POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/files
//!   → 201, metadata-only attachment record (no binary upload/storage)
//! - PUT    .../comments/{commentId} → 200, edit the comment text (sets
//!   `edited_at` when it changes); Canvas declares only PUT and DELETE there
//!   (routes.rb:1582-1583), so no single-comment GET is served.
//! - DELETE .../comments/{commentId} → 204, soft-delete (existing behavior)
//!
//! Access control (Canvas-style):
//! - DELETE: author of the comment or admin
//! - PUT (edit): author of the comment or admin
//! - comment files upload: teacher/admin or the submission's own student

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    routing::{post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// Server-rendered UI loaders + the shared comment write (star-ui, issue
// #108): pub fns extracted the same way star-conversations did, so the
// SpeedGrader comment feed reuses this crate's validation instead of
// forking it.
pub mod ui;

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

/// Shared delete logic for the Canvas-nested member route.
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
/// Otherwise the record stays unattached: Canvas links the remaining ids when
/// it writes the comment on the submission update, which is star-assignments.
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
// Annotation notification (Canvas SubmissionCommentsApiController
// #annotation_notification; issue #55)
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/annotation_notification
/// — Canvas `annotation_notification`: site-admin broadcast helper for
/// document annotations. Canvas gates on `send_messages` on the Site Admin
/// account (star's global admin role), then resolves the assignment, the
/// annotation author and the submission's user, answering `{}` on 200.
/// Star has no annotation read-state table or notification delivery for
/// this path, so those side effects are intentionally absent — the route's
/// observable contract (gates, 404/400 ordering, empty 200 body) is what
/// ships.
async fn annotation_notification(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    // Canvas require_context: missing or deleted courses are 404.
    let course: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    if course.is_none() {
        return Err(ApiError::NotFound);
    }

    // Site-admin-only broadcast (Canvas: send_messages on Account.site_admin).
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let assignment: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM assignments WHERE id = $1 AND course_id = $2")
            .bind(assignment_id)
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    if assignment.is_none() {
        return Err(ApiError::NotFound);
    }

    // author_id arrives as a query param (Canvas -F form) or a JSON body.
    let mut author_id: Option<i64> = None;
    if let Some(query) = raw_query.as_deref() {
        for pair in query.split('&') {
            let Some((raw_key, raw_value)) = pair.split_once('=') else {
                continue;
            };
            if raw_key == "author_id" {
                author_id = raw_value.parse::<i64>().ok();
            }
        }
    }
    if author_id.is_none() && !body.is_empty() {
        if let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&body) {
            author_id = payload.get("author_id").and_then(|value| match value {
                serde_json::Value::Number(number) => number.as_i64(),
                serde_json::Value::String(text) => text.parse::<i64>().ok(),
                _ => None,
            });
        }
    }
    let Some(author_id) = author_id else {
        // Canvas api_find(User.active, nil) raises RecordNotFound.
        return Err(ApiError::NotFound);
    };
    let author: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(author_id)
        .fetch_optional(&state.db)
        .await?;
    if author.is_none() {
        return Err(ApiError::NotFound);
    }

    // Canvas api_find(@context.all_current_users, params[:user_id]).
    let enrolled: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2 \
         AND workflow_state <> 'deleted' LIMIT 1",
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    if enrolled.is_none() {
        return Err(ApiError::NotFound);
    }

    let submission: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM submissions WHERE assignment_id = $1 AND user_id = $2")
            .bind(assignment_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    if submission.is_none() {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "Couldn't find Submission for user with API id {user_id}"
                )
            })),
        ));
    }

    Ok((StatusCode::OK, Json(serde_json::json!({}))))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // Canvas nested member routes — resolve submission by course/assignment/user.
        // Canvas declares no comments index or create route anywhere in the API
        // route set: the thread is read through `include[]=submission_comments`
        // on the submission and written through `PUT
        // .../submissions/{userId}` (both star-assignments), and only PUT and
        // DELETE exist on the member path (routes.rb:1582-1583).  The
        // Canvas-absent flat twins `/submissions/{submissionId}/comments`
        // (GET/POST) and `/submissions/{submissionId}/comments/{commentId}`
        // are therefore not registered, and neither is a GET on the member path.
        // Metadata-only comment file upload (Canvas SubmissionCommentsApiController#create_file)
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/files",
            post(upload_comment_file_nested),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/comments/{commentId}",
            put(update_comment_nested).delete(delete_comment_nested),
        )
        // Annotation notification (Canvas SubmissionCommentsApiController
        // #annotation_notification, issue #55). Canvas declares this route
        // inside the API route set with an absolute path (no /api/v1
        // prefix); star mounts the API set under /api/v1, matching the
        // landed create_file route.
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/annotation_notification",
            post(annotation_notification),
        )
}
