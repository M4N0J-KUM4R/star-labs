//! star-assignments: assignments, submissions, and grading for star-lms.
//!
//! Implements the Canvas-style assignments API with enrollment-based access control.

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

/// Assignment record matching Canvas API v1 shape.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Assignment {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub points_possible: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request to create or update an assignment.
#[derive(Debug, Deserialize)]
pub struct CreateAssignment {
    pub title: String,
    pub description: Option<String>,
    pub points_possible: Option<f64>,
    pub due_at: Option<DateTime<Utc>>,
}

/// Request to update an assignment.
#[derive(Debug, Deserialize)]
pub struct UpdateAssignment {
    pub title: Option<String>,
    pub description: Option<String>,
    pub points_possible: Option<f64>,
    pub due_at: Option<DateTime<Utc>>,
}

/// Submission record matching Canvas API v1 shape.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Submission {
    pub id: i64,
    pub assignment_id: i64,
    pub user_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub submitted_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Submission with user info (for teacher/admin views).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SubmissionWithUser {
    pub id: i64,
    pub assignment_id: i64,
    pub user_id: i64,
    pub user_name: String,
    pub user_email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub submitted_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request to submit an assignment.
#[derive(Debug, Deserialize)]
pub struct CreateSubmission {
    pub body: Option<String>,
}

/// Request to grade a submission.
#[derive(Debug, Deserialize)]
pub struct GradeSubmission {
    pub score: f64,
}

/// A single student's grade row in the gradebook.
#[derive(Debug, Clone, Serialize)]
pub struct GradebookRow {
    pub user: GradebookUser,
    pub submissions: Vec<GradebookSubmission>,
    pub current_grade: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GradebookUser {
    pub id: i64,
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GradebookSubmission {
    pub assignment_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub points_possible: f64,
}

/// The full gradebook response.
#[derive(Debug, Clone, Serialize)]
pub struct Gradebook {
    pub assignment_ids: Vec<i64>,
    pub rows: Vec<GradebookRow>,
}

/// Assignment override matching the Canvas API v1 AssignmentOverride shape.
///
/// Canvas omits target fields that do not apply to the override's set type, so
/// the empty/null variants are skipped during serialization.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AssignmentOverride {
    pub id: i64,
    pub assignment_id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlock_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lock_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub student_ids: Vec<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub course_section_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<i64>,
    pub workflow_state: String,
}

/// Request body for creating or updating an assignment override.
///
/// Matches Canvas `assignment_override[...]` arguments flattened into JSON.
#[derive(Debug, Deserialize)]
pub struct OverrideRequest {
    pub title: Option<String>,
    pub due_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    pub student_ids: Option<Vec<i64>>,
    pub course_section_id: Option<i64>,
    pub group_id: Option<i64>,
}

/// The target set an override applies to (Canvas: ADHOC, Group, CourseSection).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverrideTarget {
    Adhoc(Vec<i64>),
    Section(i64),
    Group(i64),
}

impl OverrideTarget {
    /// Split into the `(student_ids, course_section_id, group_id)` columns.
    fn into_columns(self) -> (Vec<i64>, Option<i64>, Option<i64>) {
        match self {
            OverrideTarget::Adhoc(ids) => (ids, None, None),
            OverrideTarget::Section(id) => (Vec::new(), Some(id), None),
            OverrideTarget::Group(id) => (Vec::new(), None, Some(id)),
        }
    }
}

/// Resolve the target set for an override.
///
/// Canvas picks the most specific target: student_ids first, then group_id,
/// then course_section_id. Returns 400 when no target is supplied or the
/// ad-hoc student list is empty, mirroring Canvas' `bad_request`.
pub(crate) fn resolve_override_target(
    student_ids: Option<Vec<i64>>,
    group_id: Option<i64>,
    course_section_id: Option<i64>,
) -> Result<OverrideTarget, ApiError> {
    if let Some(mut ids) = student_ids {
        ids.sort_unstable();
        ids.dedup();
        if ids.is_empty() || ids.iter().any(|id| *id <= 0) {
            return Err(ApiError::BadRequest("invalid student_ids".into()));
        }
        return Ok(OverrideTarget::Adhoc(ids));
    }

    if let Some(group_id) = group_id {
        return Ok(OverrideTarget::Group(group_id));
    }

    if let Some(course_section_id) = course_section_id {
        return Ok(OverrideTarget::Section(course_section_id));
    }

    Err(ApiError::BadRequest(
        "one of student_ids, group_id, or course_section_id is required".into(),
    ))
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

/// Check if user is admin.
fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Check if user is enrolled in a course (as any role) or is admin.
async fn check_enrolled_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
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

/// Verify that an assignment exists and belongs to the given course.
/// Canvas nested routes treat a mismatched or missing assignment as 404.
async fn ensure_assignment_in_course(
    pool: &PgPool,
    course_id: i64,
    assignment_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i32,)> =
        sqlx::query_as(r#"SELECT 1 FROM assignments WHERE id = $1 AND course_id = $2"#)
            .bind(assignment_id)
            .bind(course_id)
            .fetch_optional(pool)
            .await?;

    row.ok_or(ApiError::NotFound)?;
    Ok(())
}

/// Validate that every ad-hoc student id belongs to an active student
/// enrollment in the course. Canvas returns 400 for unknown student ids.
async fn validate_student_ids(
    pool: &PgPool,
    course_id: i64,
    student_ids: &[i64],
) -> Result<(), ApiError> {
    let enrolled: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT user_id) FROM enrollments
        WHERE course_id = $1 AND user_id = ANY($2)
          AND role = 'student' AND workflow_state <> 'deleted'
        "#,
    )
    .bind(course_id)
    .bind(student_ids)
    .fetch_one(pool)
    .await?;

    if enrolled as usize != student_ids.len() {
        return Err(ApiError::BadRequest("unknown student ids".into()));
    }

    Ok(())
}

/// Validate that a group override target is an active group in the course.
async fn validate_group_in_course(
    pool: &PgPool,
    course_id: i64,
    group_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM groups
        WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    row.ok_or_else(|| ApiError::BadRequest("group_id is not valid".into()))?;
    Ok(())
}

/// Resolve and validate an override target against the course.
async fn resolve_and_validate_target(
    pool: &PgPool,
    course_id: i64,
    student_ids: Option<Vec<i64>>,
    group_id: Option<i64>,
    course_section_id: Option<i64>,
) -> Result<OverrideTarget, ApiError> {
    let target = resolve_override_target(student_ids, group_id, course_section_id)?;

    match &target {
        OverrideTarget::Adhoc(ids) => validate_student_ids(pool, course_id, ids).await?,
        OverrideTarget::Group(id) => validate_group_in_course(pool, course_id, *id).await?,
        // Sections belong to the courses/terms workstream; there is no base
        // table to validate against yet, so only reject impossible ids.
        OverrideTarget::Section(id) if *id <= 0 => {
            return Err(ApiError::BadRequest(
                "course_section_id is not valid".into(),
            ))
        }
        OverrideTarget::Section(_) => {}
    }

    Ok(target)
}

/// Fetch a non-deleted override or 404.
async fn fetch_override(pool: &PgPool, override_id: i64) -> Result<AssignmentOverride, ApiError> {
    let row: Option<AssignmentOverride> = sqlx::query_as(
        r#"
        SELECT id, assignment_id, title, due_at, unlock_at, lock_at,
               student_ids, course_section_id, group_id, workflow_state
        FROM assignment_overrides
        WHERE id = $1 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(override_id)
    .fetch_optional(pool)
    .await?;

    row.ok_or(ApiError::NotFound)
}

/// Course that owns the assignment behind an override.
async fn course_id_for_assignment(pool: &PgPool, assignment_id: i64) -> Result<i64, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM assignments WHERE id = $1"#)
        .bind(assignment_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.ok_or(ApiError::NotFound)?.0)
}

/// Fetch an override and enforce read (enrolled) or write (teacher/admin)
/// access on its course. Returns the existing row.
async fn require_override_access(
    pool: &PgPool,
    override_id: i64,
    auth: &AuthUser,
    write: bool,
) -> Result<AssignmentOverride, ApiError> {
    let existing = fetch_override(pool, override_id).await?;
    let course_id = course_id_for_assignment(pool, existing.assignment_id).await?;

    let allowed = if write {
        check_teacher_or_admin(pool, auth.id, course_id, auth).await?
    } else {
        check_enrolled_or_admin(pool, auth.id, course_id, auth).await?
    };

    if !allowed {
        return Err(ApiError::Forbidden);
    }

    Ok(existing)
}

/// Ensure the override belongs to the assignment in the nested Canvas route.
async fn ensure_override_in_assignment(
    pool: &PgPool,
    assignment_id: i64,
    override_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM assignment_overrides
        WHERE id = $1 AND assignment_id = $2 AND workflow_state <> 'deleted'
        "#,
    )
    .bind(override_id)
    .bind(assignment_id)
    .fetch_optional(pool)
    .await?;

    row.ok_or(ApiError::NotFound)?;
    Ok(())
}

/// Canvas defaults a missing or blank override title to "No Title".
fn normalize_override_title(title: Option<String>) -> String {
    title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "No Title".to_string())
}

// ---------------------------------------------------------------------------
// Grade computation (pure functions — unit tested)
// ---------------------------------------------------------------------------

/// Compute the current grade percentage from scored submissions.
///
/// Formula: sum(scores) / sum(points_possible) * 100.0
/// Only includes assignments that have a graded submission (score IS NOT NULL).
/// Returns None if no graded submissions exist.
pub fn compute_grade_percent(scores: &[f64], points_possible: &[f64]) -> Option<f64> {
    let mut sum_score = 0.0;
    let mut sum_possible = 0.0;

    for (&score, &possible) in scores.iter().zip(points_possible.iter()) {
        // Only include graded assignments (score is not NaN/default)
        if score >= 0.0 && possible > 0.0 {
            sum_score += score;
            sum_possible += possible;
        }
    }

    if sum_possible > 0.0 {
        Some((sum_score / sum_possible) * 100.0)
    } else {
        None
    }
}

/// Compute grade from assignment records and submission records.
/// Returns (scores, points_possible) arrays aligned with assignments.
pub fn grade_arrays_from_submissions(
    assignments: &[Assignment],
    submissions: &[Submission],
) -> (Vec<f64>, Vec<f64>) {
    let mut scores = Vec::with_capacity(assignments.len());
    let mut points_possible = Vec::with_capacity(assignments.len());

    for a in assignments {
        let pts = a.points_possible;
        // Find submission for this assignment (if any)
        let score = submissions
            .iter()
            .find(|s| s.assignment_id == a.id)
            .and_then(|s| s.score);
        points_possible.push(pts);
        scores.push(score.unwrap_or(-1.0)); // -1 sentinel for ungraded
    }

    (scores, points_possible)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/assignments
async fn list_course_assignments(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Assignment>>, ApiError> {
    // Check enrolled or admin
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<Assignment> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        FROM assignments
        WHERE course_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// POST /courses/{courseId}/assignments
async fn create_assignment(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateAssignment>,
) -> Result<(StatusCode, Json<Assignment>), ApiError> {
    // Check teacher or admin
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }

    // Validate points_possible >= 0 if provided
    if let Some(pp) = payload.points_possible {
        if pp < 0.0 {
            return Err(ApiError::Validation("points_possible must be >= 0".into()));
        }
    }

    let row: Assignment = sqlx::query_as(
        r#"
        INSERT INTO assignments (course_id, title, description, points_possible, due_at)
        VALUES ($1, $2, $3, COALESCE($4, 0), $5)
        RETURNING id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(payload.points_possible)
    .bind(payload.due_at)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /assignments/{id}
async fn get_assignment(
    State(state): State<AppState>,
    Path(assignment_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Assignment>, ApiError> {
    // Fetch assignment to get course_id
    let row: Option<Assignment> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        FROM assignments WHERE id = $1
        "#,
    )
    .bind(assignment_id)
    .fetch_optional(&state.db)
    .await?;

    let assignment = row.ok_or(ApiError::NotFound)?;

    // Check enrolled or admin
    if !check_enrolled_or_admin(&state.db, auth.id, assignment.course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(assignment))
}

/// PUT /assignments/{id}
async fn update_assignment(
    State(state): State<AppState>,
    Path(assignment_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateAssignment>,
) -> Result<Json<Assignment>, ApiError> {
    // Fetch assignment to get course_id
    let row: Option<Assignment> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        FROM assignments WHERE id = $1
        "#,
    )
    .bind(assignment_id)
    .fetch_optional(&state.db)
    .await?;

    let existing = row.ok_or(ApiError::NotFound)?;

    // Check teacher or admin
    if !check_teacher_or_admin(&state.db, auth.id, existing.course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let title = payload.title.unwrap_or(existing.title);
    let description = payload.description.or(existing.description);
    let points_possible = payload.points_possible.unwrap_or(existing.points_possible);
    let due_at = payload.due_at.or(existing.due_at);

    let updated: Assignment = sqlx::query_as(
        r#"
        UPDATE assignments
        SET title = $2, description = $3, points_possible = $4, due_at = $5, updated_at = now()
        WHERE id = $1
        RETURNING id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        "#,
    )
    .bind(assignment_id)
    .bind(&title)
    .bind(&description)
    .bind(points_possible)
    .bind(due_at)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /assignments/{id}
async fn delete_assignment(
    State(state): State<AppState>,
    Path(assignment_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    // Fetch assignment to get course_id
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM assignments WHERE id = $1"#)
        .bind(assignment_id)
        .fetch_optional(&state.db)
        .await?;

    let course_id = row.ok_or(ApiError::NotFound)?.0;

    // Check teacher or admin
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(r#"DELETE FROM assignments WHERE id = $1"#)
        .bind(assignment_id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /assignments/{id}/submissions — upsert own submission
async fn create_submission(
    State(state): State<AppState>,
    Path(assignment_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateSubmission>,
) -> Result<(StatusCode, Json<Submission>), ApiError> {
    // Only students can submit
    if auth.role != "student" && auth.role != "teacher" && auth.role != "admin" {
        return Err(ApiError::Forbidden);
    }

    // Fetch assignment to get course_id and check enrollment
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM assignments WHERE id = $1"#)
        .bind(assignment_id)
        .fetch_optional(&state.db)
        .await?;

    let course_id = row.ok_or(ApiError::NotFound)?.0;

    // Check enrolled (students and teachers)
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Upsert: INSERT ... ON CONFLICT UPDATE
    let submission: Submission = sqlx::query_as(
        r#"
        INSERT INTO submissions (assignment_id, user_id, body)
        VALUES ($1, $2, $3)
        ON CONFLICT (assignment_id, user_id) DO UPDATE SET body = $3, submitted_at = now()
        RETURNING id, assignment_id, user_id, body, score::float8, submitted_at, created_at, updated_at
        "#,
    )
    .bind(assignment_id)
    .bind(auth.id)
    .bind(&payload.body)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(submission)))
}

/// GET /assignments/{id}/submissions — all submissions (teacher/admin)
async fn list_submissions(
    State(state): State<AppState>,
    Path(assignment_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<SubmissionWithUser>>, ApiError> {
    // Fetch assignment to get course_id
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM assignments WHERE id = $1"#)
        .bind(assignment_id)
        .fetch_optional(&state.db)
        .await?;

    let course_id = row.ok_or(ApiError::NotFound)?.0;

    // Check teacher or admin
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<SubmissionWithUser> = sqlx::query_as(
        r#"
        SELECT s.id, s.assignment_id, s.user_id, u.name as user_name, u.email as user_email,
               s.body, s.score::float8, s.submitted_at, s.created_at, s.updated_at
        FROM submissions s
        JOIN users u ON u.id = s.user_id
        WHERE s.assignment_id = $1
        ORDER BY s.submitted_at DESC
        "#,
    )
    .bind(assignment_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /assignments/{id}/submissions/me — own submission
async fn get_my_submission(
    State(state): State<AppState>,
    Path(assignment_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Submission>, ApiError> {
    // Fetch assignment to get course_id
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM assignments WHERE id = $1"#)
        .bind(assignment_id)
        .fetch_optional(&state.db)
        .await?;

    let course_id = row.ok_or(ApiError::NotFound)?.0;

    // Check enrolled
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<Submission> = sqlx::query_as(
        r#"
        SELECT id, assignment_id, user_id, body, score::float8, submitted_at, created_at, updated_at
        FROM submissions
        WHERE assignment_id = $1 AND user_id = $2
        "#,
    )
    .bind(assignment_id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;

    row.map(Json).ok_or(ApiError::NotFound)
}

/// PUT /submissions/{id}/grade — set score
async fn grade_submission(
    State(state): State<AppState>,
    Path(submission_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<GradeSubmission>,
) -> Result<Json<Submission>, ApiError> {
    // Fetch submission to get assignment/course
    let row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT a.course_id FROM submissions s
        JOIN assignments a ON a.id = s.assignment_id
        WHERE s.id = $1
        "#,
    )
    .bind(submission_id)
    .fetch_optional(&state.db)
    .await?;

    let course_id = row.ok_or(ApiError::NotFound)?.0;

    // Check teacher or admin
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let updated: Submission = sqlx::query_as(
        r#"
        UPDATE submissions
        SET score = $2, updated_at = now()
        WHERE id = $1
        RETURNING id, assignment_id, user_id, body, score::float8, submitted_at, created_at, updated_at
        "#,
    )
    .bind(submission_id)
    .bind(payload.score)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// GET /courses/{courseId}/gradebook
async fn gradebook(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Gradebook>, ApiError> {
    // Check teacher or admin
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Get assignments for course
    let assignments: Vec<Assignment> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        FROM assignments
        WHERE course_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    if assignments.is_empty() {
        return Ok(Json(Gradebook {
            assignment_ids: vec![],
            rows: vec![],
        }));
    }

    // Get all students enrolled in the course
    let students: Vec<(i64, String, String)> = sqlx::query_as(
        r#"
        SELECT u.id, u.name, u.email
        FROM users u
        JOIN enrollments e ON e.user_id = u.id
        WHERE e.course_id = $1 AND e.role = 'student'
        ORDER BY u.name ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    // Get all submissions for these assignments
    let assignment_ids: Vec<i64> = assignments.iter().map(|a| a.id).collect();
    let submissions: Vec<Submission> = sqlx::query_as(
        r#"
        SELECT id, assignment_id, user_id, body, score::float8, submitted_at, created_at, updated_at
        FROM submissions
        WHERE assignment_id = ANY($1)
        "#,
    )
    .bind(&assignment_ids)
    .fetch_all(&state.db)
    .await?;

    // Build gradebook rows
    let mut rows = Vec::with_capacity(students.len());
    for (user_id, user_name, user_email) in students {
        let mut submissions_list = Vec::with_capacity(assignments.len());
        let mut sum_score = 0.0;
        let mut sum_possible = 0.0;

        for assignment in &assignments {
            let score = submissions
                .iter()
                .find(|s| s.assignment_id == assignment.id && s.user_id == user_id)
                .and_then(|s| s.score);

            if let Some(s) = score {
                sum_score += s;
                sum_possible += assignment.points_possible;
            }

            submissions_list.push(GradebookSubmission {
                assignment_id: assignment.id,
                score,
                points_possible: assignment.points_possible,
            });
        }

        let current_grade = if sum_possible > 0.0 {
            Some((sum_score / sum_possible) * 100.0)
        } else {
            None
        };

        rows.push(GradebookRow {
            user: GradebookUser {
                id: user_id,
                name: user_name,
                email: user_email,
            },
            submissions: submissions_list,
            current_grade,
        });
    }

    let assignment_ids_out: Vec<i64> = assignments.iter().map(|a| a.id).collect();

    Ok(Json(Gradebook {
        assignment_ids: assignment_ids_out,
        rows,
    }))
}

// ---------------------------------------------------------------------------
// Canvas nested route aliases
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/assignments/{id} — flat GET alias.
async fn course_get_assignment(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Assignment>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    get_assignment(State(state), Path(assignment_id), auth).await
}

/// PUT /courses/{courseId}/assignments/{id} — flat PUT alias.
async fn course_update_assignment(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateAssignment>,
) -> Result<Json<Assignment>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    update_assignment(State(state), Path(assignment_id), auth, Json(payload)).await
}

/// DELETE /courses/{courseId}/assignments/{id} — flat DELETE alias.
async fn course_delete_assignment(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    delete_assignment(State(state), Path(assignment_id), auth).await
}

/// GET /courses/{courseId}/assignments/{id}/submissions — flat list alias.
async fn course_list_submissions(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<SubmissionWithUser>>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    list_submissions(State(state), Path(assignment_id), auth).await
}

/// POST /courses/{courseId}/assignments/{id}/submissions — flat submit alias.
async fn course_create_submission(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateSubmission>,
) -> Result<(StatusCode, Json<Submission>), ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    create_submission(State(state), Path(assignment_id), auth, Json(payload)).await
}

/// GET /courses/{courseId}/assignments/{id}/submissions/self — Canvas's
/// name for the flat `/assignments/{id}/submissions/me` route.
async fn course_get_my_submission(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Submission>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    get_my_submission(State(state), Path(assignment_id), auth).await
}

/// PUT /courses/{courseId}/assignments/{assignmentId}/submissions/{userId} —
/// Canvas's per-user grading route. Resolves the user's submission for the
/// assignment, then delegates to the flat grade handler.
async fn course_grade_submission(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<GradeSubmission>,
) -> Result<Json<Submission>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    let row: Option<(i64,)> =
        sqlx::query_as(r#"SELECT id FROM submissions WHERE assignment_id = $1 AND user_id = $2"#)
            .bind(assignment_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;

    let submission_id = row.ok_or(ApiError::NotFound)?.0;
    grade_submission(State(state), Path(submission_id), auth, Json(payload)).await
}

// ---------------------------------------------------------------------------
// Assignment overrides (Canvas AssignmentOverridesController)
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/assignments/{assignmentId}/overrides
async fn list_assignment_overrides(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<AssignmentOverride>>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<AssignmentOverride> = sqlx::query_as(
        r#"
        SELECT id, assignment_id, title, due_at, unlock_at, lock_at,
               student_ids, course_section_id, group_id, workflow_state
        FROM assignment_overrides
        WHERE assignment_id = $1 AND workflow_state <> 'deleted'
        ORDER BY id ASC
        "#,
    )
    .bind(assignment_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// POST /courses/{courseId}/assignments/{assignmentId}/overrides
async fn create_assignment_override(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<OverrideRequest>,
) -> Result<(StatusCode, Json<AssignmentOverride>), ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let target = resolve_and_validate_target(
        &state.db,
        course_id,
        payload.student_ids,
        payload.group_id,
        payload.course_section_id,
    )
    .await?;
    let (student_ids, course_section_id, group_id) = target.into_columns();
    let title = normalize_override_title(payload.title);

    let row: AssignmentOverride = sqlx::query_as(
        r#"
        INSERT INTO assignment_overrides
            (assignment_id, title, due_at, unlock_at, lock_at,
             student_ids, course_section_id, group_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        RETURNING id, assignment_id, title, due_at, unlock_at, lock_at,
                  student_ids, course_section_id, group_id, workflow_state
        "#,
    )
    .bind(assignment_id)
    .bind(&title)
    .bind(payload.due_at)
    .bind(payload.unlock_at)
    .bind(payload.lock_at)
    .bind(&student_ids)
    .bind(course_section_id)
    .bind(group_id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /assignment_overrides/{id}
async fn get_assignment_override(
    State(state): State<AppState>,
    Path(override_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<AssignmentOverride>, ApiError> {
    let row = require_override_access(&state.db, override_id, &auth, false).await?;
    Ok(Json(row))
}

/// PUT /assignment_overrides/{id}
async fn update_assignment_override(
    State(state): State<AppState>,
    Path(override_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<OverrideRequest>,
) -> Result<Json<AssignmentOverride>, ApiError> {
    let existing = require_override_access(&state.db, override_id, &auth, true).await?;
    let course_id = course_id_for_assignment(&state.db, existing.assignment_id).await?;

    // Canvas: target sets of group/section overrides cannot be changed. An
    // ad-hoc override accepts a new student_ids set; when omitted the
    // existing targets are kept.
    let (student_ids, course_section_id, group_id): (Vec<i64>, Option<i64>, Option<i64>) =
        if existing.student_ids.is_empty() {
            (Vec::new(), existing.course_section_id, existing.group_id)
        } else if let Some(ids) = payload.student_ids {
            resolve_and_validate_target(&state.db, course_id, Some(ids), None, None)
                .await?
                .into_columns()
        } else {
            (existing.student_ids.clone(), None, None)
        };

    let title = match payload.title {
        Some(title) => normalize_override_title(Some(title)),
        None => existing.title,
    };

    let updated: AssignmentOverride = sqlx::query_as(
        r#"
        UPDATE assignment_overrides
        SET title = $2, due_at = $3, unlock_at = $4, lock_at = $5,
            student_ids = $6, course_section_id = $7, group_id = $8,
            updated_at = now()
        WHERE id = $1 AND workflow_state <> 'deleted'
        RETURNING id, assignment_id, title, due_at, unlock_at, lock_at,
                  student_ids, course_section_id, group_id, workflow_state
        "#,
    )
    .bind(override_id)
    .bind(&title)
    .bind(payload.due_at)
    .bind(payload.unlock_at)
    .bind(payload.lock_at)
    .bind(&student_ids)
    .bind(course_section_id)
    .bind(group_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /assignment_overrides/{id} — Canvas soft-deletes and returns the
/// former override details with a 200.
async fn delete_assignment_override(
    State(state): State<AppState>,
    Path(override_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<AssignmentOverride>, ApiError> {
    require_override_access(&state.db, override_id, &auth, true).await?;

    let row: Option<AssignmentOverride> = sqlx::query_as(
        r#"
        UPDATE assignment_overrides
        SET workflow_state = 'deleted', updated_at = now()
        WHERE id = $1 AND workflow_state <> 'deleted'
        RETURNING id, assignment_id, title, due_at, unlock_at, lock_at,
                  student_ids, course_section_id, group_id, workflow_state
        "#,
    )
    .bind(override_id)
    .fetch_optional(&state.db)
    .await?;

    row.map(Json).ok_or(ApiError::NotFound)
}

// --- Canvas nested aliases (course + assignment scoped) --------------------

/// GET /courses/{courseId}/assignments/{assignmentId}/overrides/{id}
async fn course_get_assignment_override(
    State(state): State<AppState>,
    Path((course_id, assignment_id, override_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<AssignmentOverride>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    ensure_override_in_assignment(&state.db, assignment_id, override_id).await?;
    get_assignment_override(State(state), Path(override_id), auth).await
}

/// PUT /courses/{courseId}/assignments/{assignmentId}/overrides/{id}
async fn course_update_assignment_override(
    State(state): State<AppState>,
    Path((course_id, assignment_id, override_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<OverrideRequest>,
) -> Result<Json<AssignmentOverride>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    ensure_override_in_assignment(&state.db, assignment_id, override_id).await?;
    update_assignment_override(State(state), Path(override_id), auth, Json(payload)).await
}

/// DELETE /courses/{courseId}/assignments/{assignmentId}/overrides/{id}
async fn course_delete_assignment_override(
    State(state): State<AppState>,
    Path((course_id, assignment_id, override_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<AssignmentOverride>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    ensure_override_in_assignment(&state.db, assignment_id, override_id).await?;
    delete_assignment_override(State(state), Path(override_id), auth).await
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the assignments router.
pub fn router() -> Router<AppState> {
    Router::new()
        // Course-level assignment routes
        .route(
            "/courses/{courseId}/assignments",
            get(list_course_assignments),
        )
        .route("/courses/{courseId}/assignments", post(create_assignment))
        .route("/courses/{courseId}/gradebook", get(gradebook))
        // Assignment-level routes
        .route("/assignments/{id}", get(get_assignment))
        .route("/assignments/{id}", put(update_assignment))
        .route("/assignments/{id}", delete(delete_assignment))
        .route("/assignments/{id}/submissions", post(create_submission))
        .route("/assignments/{id}/submissions", get(list_submissions))
        .route("/assignments/{id}/submissions/me", get(get_my_submission))
        // Submission-level routes
        .route("/submissions/{id}/grade", put(grade_submission))
        // Canvas v1 nested aliases (same handlers, course-scoped paths)
        .route(
            "/courses/{courseId}/assignments/{id}",
            get(course_get_assignment),
        )
        .route(
            "/courses/{courseId}/assignments/{id}",
            put(course_update_assignment),
        )
        .route(
            "/courses/{courseId}/assignments/{id}",
            delete(course_delete_assignment),
        )
        .route(
            "/courses/{courseId}/assignments/{id}/submissions",
            get(course_list_submissions),
        )
        .route(
            "/courses/{courseId}/assignments/{id}/submissions",
            post(course_create_submission),
        )
        .route(
            "/courses/{courseId}/assignments/{id}/submissions/self",
            get(course_get_my_submission),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}",
            put(course_grade_submission),
        )
        // Assignment overrides — Canvas nested paths
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/overrides",
            get(list_assignment_overrides),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/overrides",
            post(create_assignment_override),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/overrides/{id}",
            get(course_get_assignment_override),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/overrides/{id}",
            put(course_update_assignment_override),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/overrides/{id}",
            delete(course_delete_assignment_override),
        )
        // Assignment overrides — flat routes
        .route("/assignment_overrides/{id}", get(get_assignment_override))
        .route(
            "/assignment_overrides/{id}",
            put(update_assignment_override),
        )
        .route(
            "/assignment_overrides/{id}",
            delete(delete_assignment_override),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_grade_percent_empty() {
        let scores: Vec<f64> = vec![];
        let points: Vec<f64> = vec![];
        assert!(compute_grade_percent(&scores, &points).is_none());
    }

    #[test]
    fn test_compute_grade_percent_no_graded() {
        // Ungraded assignments have sentinel -1
        let scores = vec![-1.0, -1.0];
        let points = vec![10.0, 20.0];
        assert!(compute_grade_percent(&scores, &points).is_none());
    }

    #[test]
    fn test_compute_grade_percent_partial_graded() {
        let scores = vec![8.0, -1.0, 18.0]; // 8/10 and 18/20 graded
        let points = vec![10.0, 15.0, 20.0];
        let result = compute_grade_percent(&scores, &points);
        assert!(result.is_some());
        let pct = result.unwrap();
        // (8+18)/(10+20) * 100 = 26/30 * 100 ≈ 86.67
        assert!((pct - 86.666666).abs() < 0.01);
    }

    #[test]
    fn test_compute_grade_percent_all_graded() {
        let scores = vec![9.0, 18.0, 45.0];
        let points = vec![10.0, 20.0, 50.0];
        let result = compute_grade_percent(&scores, &points);
        assert!(result.is_some());
        let pct = result.unwrap();
        // (9+18+45)/(10+20+50) * 100 = 72/80 * 100 = 90.0
        assert!((pct - 90.0).abs() < 0.001);
    }

    #[test]
    fn test_compute_grade_percent_perfect_score() {
        let scores = vec![10.0, 20.0];
        let points = vec![10.0, 20.0];
        let result = compute_grade_percent(&scores, &points);
        assert!(result.is_some());
        assert!((result.unwrap() - 100.0).abs() < 0.001);
    }

    #[test]
    fn test_grade_arrays_from_submissions() {
        let assignments = vec![
            Assignment {
                id: 1,
                course_id: 1,
                title: "A1".into(),
                description: None,
                points_possible: 10.0,
                due_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            Assignment {
                id: 2,
                course_id: 1,
                title: "A2".into(),
                description: None,
                points_possible: 20.0,
                due_at: None,
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
        ];

        let submissions = vec![
            Submission {
                id: 1,
                assignment_id: 1,
                user_id: 1,
                body: Some("answer".into()),
                score: Some(8.0),
                submitted_at: Utc::now(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            },
            // No submission for assignment 2
        ];

        let (scores, points) = grade_arrays_from_submissions(&assignments, &submissions);
        assert_eq!(points, vec![10.0, 20.0]);
        assert_eq!(scores, vec![8.0, -1.0]); // -1 sentinel for ungraded
    }

    #[test]
    fn test_grade_arrays_no_submissions() {
        let assignments = vec![Assignment {
            id: 1,
            course_id: 1,
            title: "A1".into(),
            description: None,
            points_possible: 10.0,
            due_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }];

        let submissions: Vec<Submission> = vec![];
        let (scores, points) = grade_arrays_from_submissions(&assignments, &submissions);
        assert_eq!(points, vec![10.0]);
        assert_eq!(scores, vec![-1.0]);
    }

    #[test]
    fn test_resolve_override_target_prefers_students() {
        // Canvas priority: student_ids > group_id > course_section_id, and
        // duplicate student ids are collapsed.
        let target = resolve_override_target(Some(vec![7, 3, 7]), Some(11), Some(22)).unwrap();
        assert_eq!(target, OverrideTarget::Adhoc(vec![3, 7]));
    }

    #[test]
    fn test_resolve_override_target_requires_target() {
        assert!(resolve_override_target(None, None, None).is_err());
        assert!(resolve_override_target(Some(vec![]), None, None).is_err());

        assert_eq!(
            resolve_override_target(None, None, Some(5)).unwrap(),
            OverrideTarget::Section(5)
        );
        assert_eq!(
            resolve_override_target(None, Some(9), Some(5)).unwrap(),
            OverrideTarget::Group(9)
        );
    }
}

#[cfg(test)]
mod db_tests {
    //! DB integration tests — skipped if DATABASE_URL is not set.
    //!
    //! Run with: DATABASE_URL=postgres://... cargo test -p star-assignments db_tests

    use super::*;
    use sqlx::PgPool;
    use std::env;

    fn get_db_url() -> Option<String> {
        env::var("DATABASE_URL").ok()
    }

    #[tokio::test]
    async fn test_create_and_list_assignment() {
        let db_url = match get_db_url() {
            Some(url) => url,
            None => {
                println!("SKIP: DATABASE_URL not set");
                return;
            }
        };
        let pool = PgPool::connect(&db_url).await.unwrap();

        // Run pending migrations so the tables exist.
        star_db::MIGRATOR.run(&pool).await.unwrap();

        // Create a user and course so we can create a real assignment.
        let teacher_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO users (name, email, password_hash, role)
            VALUES ('Teacher', 'teacher@db-test.local', 'x', 'teacher')
            ON CONFLICT (email) DO UPDATE SET name = EXCLUDED.name
            RETURNING id
            "#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let course_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO courses (name, course_code)
            VALUES ('DB Test Course', 'DB101')
            ON CONFLICT DO NOTHING
            RETURNING id
            "#,
        )
        .fetch_optional(&pool)
        .await
        .unwrap()
        .unwrap();

        // Enroll the teacher
        sqlx::query(
            r#"
            INSERT INTO enrollments (user_id, course_id, role)
            VALUES ($1, $2, 'teacher')
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(teacher_id)
        .bind(course_id)
        .execute(&pool)
        .await
        .unwrap();

        // Create an assignment (simulate what the handler does).
        let assignment: Assignment = sqlx::query_as(
            r#"
            INSERT INTO assignments (course_id, title, description, points_possible)
            VALUES ($1, 'DB Test Assignment', 'Test description', 25.0)
            RETURNING id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
            "#,
        )
        .bind(course_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(assignment.title, "DB Test Assignment");
        assert_eq!(assignment.points_possible, 25.0);

        // List assignments for the course.
        let list: Vec<Assignment> = sqlx::query_as(
            r#"
            SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
            FROM assignments WHERE course_id = $1
            "#,
        )
        .bind(course_id)
        .fetch_all(&pool)
        .await
        .unwrap();

        assert!(list.iter().any(|a| a.id == assignment.id));

        // Clean up.
        let _ = sqlx::query("DELETE FROM assignments WHERE id = $1")
            .bind(assignment.id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM enrollments WHERE user_id = $1 AND course_id = $2")
            .bind(teacher_id)
            .bind(course_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM courses WHERE id = $1")
            .bind(course_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(teacher_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn test_upsert_submission() {
        let db_url = match get_db_url() {
            Some(url) => url,
            None => {
                println!("SKIP: DATABASE_URL not set");
                return;
            }
        };
        let pool = PgPool::connect(&db_url).await.unwrap();
        star_db::MIGRATOR.run(&pool).await.unwrap();

        // Create fixture: user, course, assignment, enrollment.
        let user_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO users (name, email, password_hash, role)
            VALUES ('Student', 'student@db-test.local', 'x', 'student')
            ON CONFLICT (email) DO UPDATE SET name = EXCLUDED.name
            RETURNING id
            "#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let course_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO courses (name, course_code) VALUES ('Course', 'C1')
            ON CONFLICT DO NOTHING RETURNING id
            "#,
        )
        .fetch_optional(&pool)
        .await
        .unwrap()
        .unwrap();

        sqlx::query(
            r#"INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student') ON CONFLICT DO NOTHING"#,
        )
        .bind(user_id)
        .bind(course_id)
        .execute(&pool)
        .await
        .unwrap();

        let assignment_id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, 'A1', 10.0)
            RETURNING id
            "#,
        )
        .bind(course_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        // First submission.
        let sub1: (i64,) = sqlx::query_as(
            r#"
            INSERT INTO submissions (assignment_id, user_id, body)
            VALUES ($1, $2, 'My first answer')
            ON CONFLICT (assignment_id, user_id) DO UPDATE SET body = EXCLUDED.body, submitted_at = now()
            RETURNING id
            "#,
        )
        .bind(assignment_id)
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        // Second submission (upsert — same user, same assignment).
        let sub2: (i64,) = sqlx::query_as(
            r#"
            INSERT INTO submissions (assignment_id, user_id, body)
            VALUES ($1, $2, 'My updated answer')
            ON CONFLICT (assignment_id, user_id) DO UPDATE SET body = EXCLUDED.body, submitted_at = now()
            RETURNING id
            "#,
        )
        .bind(assignment_id)
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        // IDs should be the same (upsert, not insert).
        assert_eq!(sub1.0, sub2.0);

        // Clean up.
        let _ = sqlx::query("DELETE FROM submissions WHERE id = $1")
            .bind(sub1.0)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM assignments WHERE id = $1")
            .bind(assignment_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM enrollments WHERE user_id = $1 AND course_id = $2")
            .bind(user_id)
            .bind(course_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM courses WHERE id = $1")
            .bind(course_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn test_grade_submission() {
        let db_url = match get_db_url() {
            Some(url) => url,
            None => {
                println!("SKIP: DATABASE_URL not set");
                return;
            }
        };
        let pool = PgPool::connect(&db_url).await.unwrap();
        star_db::MIGRATOR.run(&pool).await.unwrap();

        // Fixture setup.
        let user_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO users (name, email, password_hash, role) VALUES ('S', 'grade@db.local', 'x', 'student') ON CONFLICT (email) DO UPDATE SET name=EXCLUDED.name RETURNING id"#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let course_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO courses (name, course_code) VALUES ('G', 'G1') ON CONFLICT DO NOTHING RETURNING id"#,
        )
        .fetch_optional(&pool)
        .await
        .unwrap()
        .unwrap();

        let assignment_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, 'Graded A', 100.0) RETURNING id"#,
        )
        .bind(course_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        let submission_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO submissions (assignment_id, user_id, body) VALUES ($1, $2, 'answer') RETURNING id"#,
        )
        .bind(assignment_id)
        .bind(user_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        // Grade the submission.
        let graded: Submission = sqlx::query_as(
            r#"UPDATE submissions SET score = $2, updated_at = now() WHERE id = $1 RETURNING id, assignment_id, user_id, body, score::float8, submitted_at, created_at, updated_at"#,
        )
        .bind(submission_id)
        .bind(87.5)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(graded.score, Some(87.5));

        // Clean up.
        let _ = sqlx::query("DELETE FROM submissions WHERE id = $1")
            .bind(submission_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM assignments WHERE id = $1")
            .bind(assignment_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM courses WHERE id = $1")
            .bind(course_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(&pool)
            .await;
    }

    #[tokio::test]
    async fn test_gradebook_computes_correctly() {
        let db_url = match get_db_url() {
            Some(url) => url,
            None => {
                println!("SKIP: DATABASE_URL not set");
                return;
            }
        };
        let pool = PgPool::connect(&db_url).await.unwrap();
        star_db::MIGRATOR.run(&pool).await.unwrap();

        // Create two assignments (10 pts and 20 pts) and one student.
        let student_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO users (name, email, password_hash, role) VALUES ('GB Student', 'gb@db.local', 'x', 'student') ON CONFLICT (email) DO UPDATE SET name=EXCLUDED.name RETURNING id"#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let course_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO courses (name, course_code) VALUES ('Grade Course', 'GC1') ON CONFLICT DO NOTHING RETURNING id"#,
        )
        .fetch_optional(&pool)
        .await
        .unwrap()
        .unwrap();

        sqlx::query(r#"INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student') ON CONFLICT DO NOTHING"#)
            .bind(student_id)
            .bind(course_id)
            .execute(&pool)
            .await
            .unwrap();

        let a1_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, 'A1', 10.0) RETURNING id"#,
        )
        .bind(course_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        let a2_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, 'A2', 20.0) RETURNING id"#,
        )
        .bind(course_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        let _sub1_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO submissions (assignment_id, user_id, body, score) VALUES ($1, $2, 'ans', 8.25) RETURNING id"#,
        )
        .bind(a1_id)
        .bind(student_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        let _sub2_id: i64 = sqlx::query_scalar(
            r#"INSERT INTO submissions (assignment_id, user_id, body) VALUES ($1, $2, 'ans') RETURNING id"#,
        )
        .bind(a2_id)
        .bind(student_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        // Compute grade: 8.25/10 = 82.5 (only graded count; NUMERIC score cast to FLOAT8).
        let sum_score: (f64,) = sqlx::query_as(
            r#"SELECT COALESCE(SUM(s.score::float8), 0) FROM submissions s JOIN assignments a ON a.id = s.assignment_id WHERE s.user_id = $1 AND s.score IS NOT NULL"#,
        )
        .bind(student_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        let sum_pts: (f64,) = sqlx::query_as(
            r#"SELECT COALESCE(SUM(a.points_possible::float8), 0) FROM submissions s JOIN assignments a ON a.id = s.assignment_id WHERE s.user_id = $1 AND s.score IS NOT NULL"#,
        )
        .bind(student_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        let grade_pct = (sum_score.0 / sum_pts.0) * 100.0;
        assert!((grade_pct - 82.5).abs() < 0.001);

        // Clean up.
        let _ = sqlx::query("DELETE FROM submissions WHERE user_id = $1")
            .bind(student_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM assignments WHERE course_id = $1")
            .bind(course_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM enrollments WHERE user_id = $1")
            .bind(student_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM courses WHERE id = $1")
            .bind(course_id)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(student_id)
            .execute(&pool)
            .await;
    }
}
