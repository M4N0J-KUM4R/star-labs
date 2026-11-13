//! star-assignments: assignments, submissions, and grading for star-lms.
//!
//! Implements the Canvas-style assignments API with enrollment-based access control.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};
use std::collections::HashMap;

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

/// Canvas-shaped submission returned by the submissions API endpoints
/// (lib/api/v1/submission.rb SUBMISSION_JSON_FIELDS).
///
/// `url`/`preview_url` are course-relative paths: star-lms does not know its
/// external host, so the Canvas field order is kept without the host prefix.
#[derive(Debug, Clone, Serialize)]
pub struct CanvasSubmission {
    pub id: Option<i64>,
    pub assignment_id: i64,
    pub user_id: i64,
    pub attempt: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grade: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub submitted_at: Option<DateTime<Utc>>,
    pub graded_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grader_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission_type: Option<String>,
    pub workflow_state: String,
    pub late: bool,
    pub missing: bool,
    pub excused: bool,
    pub url: String,
    pub preview_url: String,
    pub grade_matches_current_submission: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub late_policy_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points_deducted: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entered_grade: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entered_score: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SubmissionAttachment>,
}

/// Metadata-only submission attachment (Canvas Attachment under Submission).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct SubmissionAttachment {
    pub id: i64,
    pub filename: String,
    pub display_name: String,
    pub content_type: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct SubmissionAttachmentRow {
    submission_id: i64,
    #[sqlx(flatten)]
    attachment: SubmissionAttachment,
}

/// Raw submission row used to build a [`CanvasSubmission`].
#[derive(Debug, Clone, sqlx::FromRow)]
struct SubmissionRow {
    id: i64,
    assignment_id: i64,
    user_id: i64,
    attempt: i32,
    body: Option<String>,
    grade: Option<String>,
    score: Option<f64>,
    submitted_at: Option<DateTime<Utc>>,
    graded_at: Option<DateTime<Utc>>,
    grader_id: Option<i64>,
    submission_type: Option<String>,
    workflow_state: String,
    late: bool,
    excused: bool,
    late_policy_status: Option<String>,
    points_deducted: Option<f64>,
    entered_grade: Option<String>,
    entered_score: Option<f64>,
}

impl CanvasSubmission {
    /// Canvas `show` returns a synthesized unsubmitted submission when the
    /// student has no row yet (`assignment.submission_for_student`).
    fn unsubmitted(course_id: i64, assignment_id: i64, user_id: i64) -> Self {
        let url = format!("/courses/{course_id}/assignments/{assignment_id}/submissions/{user_id}");
        Self {
            id: None,
            assignment_id,
            user_id,
            attempt: 1,
            body: None,
            grade: None,
            score: None,
            submitted_at: None,
            graded_at: None,
            grader_id: None,
            submission_type: None,
            workflow_state: "unsubmitted".to_string(),
            late: false,
            missing: true,
            excused: false,
            preview_url: format!("{url}?preview=1"),
            url,
            grade_matches_current_submission: true,
            late_policy_status: None,
            points_deducted: None,
            entered_grade: None,
            entered_score: None,
            attachments: Vec::new(),
        }
    }

    fn from_row(
        row: SubmissionRow,
        course_id: i64,
        attachments: Vec<SubmissionAttachment>,
    ) -> Self {
        let url = format!(
            "/courses/{course_id}/assignments/{}/submissions/{}",
            row.assignment_id, row.user_id
        );
        Self {
            id: Some(row.id),
            assignment_id: row.assignment_id,
            user_id: row.user_id,
            attempt: row.attempt,
            body: row.body,
            grade: row.grade,
            score: row.score,
            submitted_at: row.submitted_at,
            graded_at: row.graded_at,
            grader_id: row.grader_id,
            submission_type: row.submission_type,
            workflow_state: row.workflow_state,
            late: row.late,
            missing: false,
            excused: row.excused,
            preview_url: format!("{url}?preview=1"),
            url,
            grade_matches_current_submission: true,
            late_policy_status: row.late_policy_status,
            points_deducted: row.points_deducted,
            entered_grade: row.entered_grade,
            entered_score: row.entered_score,
            attachments,
        }
    }
}

/// Canvas submission SELECT shared by the submissions API handlers.
///
/// `grade`/`entered_grade` use float8 text so they serialize like Canvas'
/// `score.to_s` ("87.5", not NUMERIC's "87.50").
const SUBMISSION_SELECT: &str = r#"
    SELECT s.id,
           s.assignment_id,
           s.user_id,
           s.attempt,
           s.body,
           s.score::float8::text AS grade,
           s.score::float8 AS score,
           s.submitted_at,
           s.graded_at,
           s.grader_id,
           s.submission_type,
           s.workflow_state,
           (a.due_at IS NOT NULL AND s.submitted_at > a.due_at) AS late,
           s.excused,
           s.late_policy_status,
           s.points_deducted::float8 AS points_deducted,
           s.score::float8::text AS entered_grade,
           s.score::float8 AS entered_score
    FROM submissions s
    JOIN assignments a ON a.id = s.assignment_id
"#;

/// Canvas caps `students/submissions` at one page of student ids.
const MAX_STUDENTS_PER_REQUEST: usize = 100;

/// Parsed query string for GET /courses/{courseId}/students/submissions.
#[derive(Debug, Default)]
struct StudentSubmissionsQuery {
    student_ids: Vec<String>,
    assignment_ids: Vec<i64>,
    grouped: bool,
    workflow_state: Option<String>,
    submitted_since: Option<String>,
    graded_since: Option<String>,
    order: Option<String>,
    order_direction: Option<String>,
}

/// Request body for POST .../submissions/{userId}/files (metadata only).
///
/// Canvas' upload preflight sends multipart `name`; star-lms accepts JSON and
/// also recognizes `filename` for symmetry with the files API.
#[derive(Debug, Deserialize, Default)]
struct SubmissionFileRequest {
    name: Option<String>,
    filename: Option<String>,
    display_name: Option<String>,
    content_type: Option<String>,
    size: Option<i64>,
    submit_assignment: Option<bool>,
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

/// Verify the target user has a live enrollment in the course. Canvas'
/// `get_user_considering_section` raises RecordNotFound when it cannot find
/// the user in the course, so mismatches are 404s rather than 403s.
async fn ensure_user_in_course(
    pool: &PgPool,
    course_id: i64,
    user_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM enrollments
        WHERE course_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    row.ok_or(ApiError::NotFound)?;
    Ok(())
}

/// Canvas truthy query/body values (`true`, `1`, `yes`, `on`).
fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// Minimal percent-decoding for raw query/body components (same approach as
/// star-calendar's `context_codes[]` parsing: no new dependency).
fn decode_query_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        decoded.push((high * 16 + low) as u8);
                        index += 3;
                    }
                    _ => {
                        decoded.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Parse the `students/submissions` query string, accepting Canvas array
/// spellings (`student_ids[]=1&student_ids[]=2` or `student_ids=1,2`).
fn parse_student_submissions_query(
    raw_query: Option<&str>,
) -> Result<StudentSubmissionsQuery, ApiError> {
    let mut query = StudentSubmissionsQuery::default();
    let Some(raw_query) = raw_query else {
        return Ok(query);
    };

    for pair in raw_query.split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let key = decode_query_component(raw_key).to_ascii_lowercase();
        let key = key.strip_suffix("[]").unwrap_or(key.as_str()).to_string();
        let value = decode_query_component(raw_value);

        match key.as_str() {
            "student_ids" => query.student_ids.extend(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_string),
            ),
            "assignment_ids" => {
                for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    let id = part
                        .parse::<i64>()
                        .map_err(|_| ApiError::BadRequest("invalid assignment_ids".into()))?;
                    query.assignment_ids.push(id);
                }
            }
            "grouped" => query.grouped = parse_bool(&value),
            "workflow_state" => query.workflow_state = Some(value).filter(|v| !v.is_empty()),
            "submitted_since" => query.submitted_since = Some(value).filter(|v| !v.is_empty()),
            "graded_since" => query.graded_since = Some(value).filter(|v| !v.is_empty()),
            "order" => query.order = Some(value).filter(|v| !v.is_empty()),
            "order_direction" => query.order_direction = Some(value).filter(|v| !v.is_empty()),
            _ => {}
        }
    }

    query.assignment_ids.sort_unstable();
    query.assignment_ids.dedup();
    Ok(query)
}

/// Parse a Canvas ISO8601 `submitted_since` / `graded_since` filter.
fn parse_since(value: Option<&str>, field: &str) -> Result<Option<DateTime<Utc>>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(value)
        .map(|parsed| Some(parsed.with_timezone(&Utc)))
        .map_err(|_| ApiError::BadRequest(format!("Invalid datetime for {field}")))
}

/// Resolve the `student_ids[]` argument against the course.
///
/// Canvas semantics: teachers/admins may request any visible student or
/// "all"; students may only request themselves. Unknown student ids are
/// unauthorized (403) just like `render_unauthorized_action`.
async fn resolve_requested_student_ids(
    pool: &PgPool,
    course_id: i64,
    auth: &AuthUser,
    can_view_all: bool,
    requested: &[String],
) -> Result<Vec<i64>, ApiError> {
    if requested.is_empty() {
        return Ok(vec![auth.id]);
    }

    if requested.iter().any(|id| id.eq_ignore_ascii_case("all")) {
        if !can_view_all {
            return Ok(vec![auth.id]);
        }
        let ids: Vec<i64> = sqlx::query_scalar(
            r#"
            SELECT DISTINCT user_id FROM enrollments
            WHERE course_id = $1 AND role = 'student' AND workflow_state <> 'deleted'
            ORDER BY user_id
            "#,
        )
        .bind(course_id)
        .fetch_all(pool)
        .await?;
        return Ok(ids);
    }

    let mut ids = Vec::with_capacity(requested.len());
    for token in requested {
        let id = token
            .parse::<i64>()
            .map_err(|_| ApiError::BadRequest("invalid student_ids".into()))?;
        ids.push(id);
    }
    ids.sort_unstable();
    ids.dedup();

    if !can_view_all {
        if ids.iter().any(|id| *id != auth.id) {
            return Err(ApiError::Forbidden);
        }
        return Ok(ids);
    }

    let visible: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT user_id) FROM enrollments
        WHERE course_id = $1 AND user_id = ANY($2)
          AND role = 'student' AND workflow_state <> 'deleted'
        "#,
    )
    .bind(course_id)
    .bind(&ids)
    .fetch_one(pool)
    .await?;

    if visible as usize != ids.len() {
        return Err(ApiError::Forbidden);
    }

    Ok(ids)
}

/// Resolve the requested `assignment_ids[]` to the course's assignments.
/// Canvas returns 403 when any requested id is outside the course scope.
async fn resolve_requested_assignment_ids(
    pool: &PgPool,
    course_id: i64,
    requested: &[i64],
) -> Result<Vec<i64>, ApiError> {
    if requested.is_empty() {
        let ids: Vec<i64> =
            sqlx::query_scalar(r#"SELECT id FROM assignments WHERE course_id = $1 ORDER BY id"#)
                .bind(course_id)
                .fetch_all(pool)
                .await?;
        return Ok(ids);
    }

    let found: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM assignments WHERE course_id = $1 AND id = ANY($2)"#,
    )
    .bind(course_id)
    .bind(requested)
    .fetch_one(pool)
    .await?;

    if found as usize != requested.len() {
        return Err(ApiError::Forbidden);
    }

    Ok(requested.to_vec())
}

/// Load submission attachments grouped by submission id.
async fn load_submission_attachments(
    pool: &PgPool,
    submission_ids: &[i64],
) -> Result<HashMap<i64, Vec<SubmissionAttachment>>, ApiError> {
    let mut attachments: HashMap<i64, Vec<SubmissionAttachment>> = HashMap::new();
    if submission_ids.is_empty() {
        return Ok(attachments);
    }

    let rows: Vec<SubmissionAttachmentRow> = sqlx::query_as(
        r#"
        SELECT submission_id, id, filename, display_name, content_type, size,
               created_at, updated_at
        FROM submission_files
        WHERE submission_id = ANY($1)
        ORDER BY id ASC
        "#,
    )
    .bind(submission_ids)
    .fetch_all(pool)
    .await?;

    for row in rows {
        attachments
            .entry(row.submission_id)
            .or_default()
            .push(row.attachment);
    }

    Ok(attachments)
}

/// Interpret a Canvas grade value: numbers and numeric strings are scores,
/// null/blank clears the grade. Letter grades need a grading scheme and are
/// rejected here (star-lms stores points only).
fn parse_grade_value(value: &serde_json::Value) -> Result<Option<f64>, ApiError> {
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(number) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| ApiError::BadRequest("invalid grade".into())),
        serde_json::Value::String(text) if text.trim().is_empty() => Ok(None),
        serde_json::Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map(Some)
            .map_err(|_| ApiError::BadRequest("invalid grade".into())),
        _ => Err(ApiError::BadRequest("invalid grade".into())),
    }
}

/// Resolve the submitted score from either the existing `{score}` payload or
/// Canvas' `{submission: {posted_grade}}` shape. `None` means no grade was
/// supplied; `Some(None)` clears the grade.
fn resolve_grade_payload(payload: &serde_json::Value) -> Result<Option<Option<f64>>, ApiError> {
    let object = payload
        .as_object()
        .ok_or_else(|| ApiError::BadRequest("invalid submission payload".into()))?;

    if let Some(inner) = object.get("submission").and_then(|value| value.as_object()) {
        if inner.get("excuse").and_then(|value| value.as_bool()) == Some(true) {
            return Ok(Some(None));
        }
        for key in ["posted_grade", "grade", "score"] {
            if let Some(value) = inner.get(key) {
                return parse_grade_value(value).map(Some);
            }
        }
    }

    for key in ["posted_grade", "grade", "score"] {
        if let Some(value) = object.get(key) {
            return parse_grade_value(value).map(Some);
        }
    }

    Ok(None)
}

/// Extract a single id from a JSON number or numeric string.
fn json_id(value: &serde_json::Value) -> Option<i64> {
    match value {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Parse `submissionIds[]` from a JSON or form-encoded
/// `bulk_mark_read` body (Canvas sends both shapes).
fn parse_submission_id_list(body: &[u8]) -> Result<Vec<i64>, ApiError> {
    if body.is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) {
        let Some(object) = json.as_object() else {
            return Ok(Vec::new());
        };
        for key in ["submissionIds", "submission_ids"] {
            if let Some(value) = object.get(key) {
                let items: Vec<&serde_json::Value> = match value {
                    serde_json::Value::Array(items) => items.iter().collect(),
                    other => vec![other],
                };
                return Ok(items.into_iter().filter_map(json_id).collect());
            }
        }
        return Ok(Vec::new());
    }

    let body = String::from_utf8_lossy(body);
    let mut ids = Vec::new();
    for pair in body.split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let key = decode_query_component(raw_key).to_ascii_lowercase();
        let key = key.strip_suffix("[]").unwrap_or(key.as_str()).to_string();
        if key != "submissionids" && key != "submission_ids" {
            continue;
        }
        for part in decode_query_component(raw_value).split(',') {
            if let Ok(id) = part.trim().parse::<i64>() {
                ids.push(id);
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
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
        INSERT INTO submissions (assignment_id, user_id, body, submission_type, anonymous_id)
        VALUES ($1, $2, $3, 'online_text_entry', gen_random_uuid()::text)
        ON CONFLICT (assignment_id, user_id) DO UPDATE
        SET body = $3, submission_type = 'online_text_entry', submitted_at = now(),
            anonymous_id = COALESCE(submissions.anonymous_id, gen_random_uuid()::text)
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
        SET score = $2,
            graded_at = now(),
            grader_id = $3,
            workflow_state = 'graded',
            updated_at = now()
        WHERE id = $1
        RETURNING id, assignment_id, user_id, body, score::float8, submitted_at, created_at, updated_at
        "#,
    )
    .bind(submission_id)
    .bind(payload.score)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

// ---------------------------------------------------------------------------
// Canvas submissions API (SubmissionsApiController)
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/students/submissions — Canvas `for_students`.
///
/// Returns a flat list of existing submissions by default; `grouped=true`
/// returns one entry per enrolled student with a (possibly empty)
/// `submissions` array.
async fn list_student_submissions(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let can_view_all = check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let query = parse_student_submissions_query(raw_query.as_deref())?;
    let student_ids = resolve_requested_student_ids(
        &state.db,
        course_id,
        &auth,
        can_view_all,
        &query.student_ids,
    )
    .await?;
    if student_ids.len() > MAX_STUDENTS_PER_REQUEST {
        return Err(ApiError::BadRequest("too many students".into()));
    }
    let assignment_ids =
        resolve_requested_assignment_ids(&state.db, course_id, &query.assignment_ids).await?;

    let submitted_since = parse_since(query.submitted_since.as_deref(), "submitted_since")?;
    let graded_since = parse_since(query.graded_since.as_deref(), "graded_since")?;

    // Canvas: order=graded_at else id; descending sorts nulls last.
    let order_direction = if query
        .order_direction
        .as_deref()
        .is_some_and(|direction| direction.eq_ignore_ascii_case("descending"))
    {
        "DESC NULLS LAST"
    } else {
        "ASC"
    };
    let order_by = match query.order.as_deref() {
        Some("graded_at") => format!("s.graded_at {order_direction}, s.id {order_direction}"),
        _ => format!("s.id {order_direction}"),
    };

    let sql = format!(
        "{SUBMISSION_SELECT}
         WHERE s.user_id = ANY($1)
           AND s.assignment_id = ANY($2)
           AND ($3::text IS NULL OR s.workflow_state = $3)
           AND ($4::timestamptz IS NULL OR s.submitted_at > $4)
           AND ($5::timestamptz IS NULL OR s.graded_at > $5)
         ORDER BY {order_by}"
    );

    let rows: Vec<SubmissionRow> = sqlx::query_as(&sql)
        .bind(&student_ids)
        .bind(&assignment_ids)
        .bind(query.workflow_state.as_deref())
        .bind(submitted_since)
        .bind(graded_since)
        .fetch_all(&state.db)
        .await?;

    let row_ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
    let mut attachments = load_submission_attachments(&state.db, &row_ids).await?;

    if !query.grouped {
        let submissions: Vec<CanvasSubmission> = rows
            .into_iter()
            .map(|row| {
                let files = attachments.remove(&row.id).unwrap_or_default();
                CanvasSubmission::from_row(row, course_id, files)
            })
            .collect();
        return Ok(Json(serde_json::json!(submissions)));
    }

    // Grouped mode ignores order/order_direction (Canvas) and includes
    // enrolled students who have not submitted anything.
    let grouped_student_ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT u.id
        FROM users u
        JOIN enrollments e ON e.user_id = u.id
        WHERE e.course_id = $1 AND e.role = 'student'
          AND e.workflow_state <> 'deleted'
          AND u.id = ANY($2)
        ORDER BY u.id
        "#,
    )
    .bind(course_id)
    .bind(&student_ids)
    .fetch_all(&state.db)
    .await?;

    let mut submissions_by_user: HashMap<i64, Vec<CanvasSubmission>> = HashMap::new();
    for row in rows {
        let files = attachments.remove(&row.id).unwrap_or_default();
        submissions_by_user
            .entry(row.user_id)
            .or_default()
            .push(CanvasSubmission::from_row(row, course_id, files));
    }

    let grouped: Vec<serde_json::Value> = grouped_student_ids
        .into_iter()
        .map(|user_id| {
            serde_json::json!({
                "user_id": user_id,
                "section_id": serde_json::Value::Null,
                "submissions": submissions_by_user.remove(&user_id).unwrap_or_default(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!(grouped)))
}

/// GET /courses/{courseId}/assignments/{assignmentId}/submissions/{userId} —
/// Canvas `show`. Students may only read their own submission; missing
/// submissions come back as a synthesized unsubmitted record.
///
/// `include[]` accepts `submission_history`, `submission_comments`,
/// `rubric_assessment`, `user`, and `read_status`. Like Canvas, including
/// `read_status` reports the current state and then marks the submission
/// read for the caller.
async fn get_student_submission(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    ensure_user_in_course(&state.db, course_id, user_id).await?;

    if auth.id != user_id && !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let query = parse_submission_includes(raw_query.as_deref());

    let row: Option<SubmissionRow> = sqlx::query_as(&format!(
        "{SUBMISSION_SELECT} WHERE s.assignment_id = $1 AND s.user_id = $2"
    ))
    .bind(assignment_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(row) = row else {
        let mut unsubmitted = serde_json::to_value(CanvasSubmission::unsubmitted(
            course_id,
            assignment_id,
            user_id,
        ))
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
        if query.include_user {
            unsubmitted["user"] = user_display(&state.db, user_id).await?;
        }
        if query.include_read_status {
            unsubmitted["read_state"] = serde_json::json!("read");
        }
        return Ok(Json(unsubmitted));
    };

    let mut attachments = load_submission_attachments(&state.db, &[row.id]).await?;
    let files = attachments.remove(&row.id).unwrap_or_default();
    let submission_id = row.id;
    let mut entry = serde_json::to_value(CanvasSubmission::from_row(row, course_id, files))
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;

    if query.include_submission_history {
        entry["submission_history"] = submission_history_json(&state.db, submission_id).await?;
    }
    if query.include_submission_comments {
        entry["submission_comments"] = submission_comments_json(&state.db, submission_id).await?;
    }
    if query.include_rubric_assessment {
        entry["rubric_assessment"] = rubric_assessment_json(&state.db, submission_id).await?;
    }
    if query.include_user {
        entry["user"] = user_display(&state.db, user_id).await?;
    }
    if query.include_read_status {
        // Canvas reports the state, then marks the submission read.
        let read_state: String = sqlx::query_scalar(
            r#"
            SELECT COALESCE(
                (SELECT read_state FROM submission_reads
                 WHERE submission_id = $1 AND user_id = $2),
                CASE WHEN EXISTS (
                    SELECT 1 FROM submission_comments c
                    WHERE c.submission_id = $1 AND c.author_id <> $2
                      AND c.workflow_state <> 'deleted'
                ) OR EXISTS (
                    SELECT 1 FROM submissions s WHERE s.id = $1 AND s.score IS NOT NULL
                ) THEN 'unread' ELSE 'read' END
            )
            "#,
        )
        .bind(submission_id)
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;
        entry["read_state"] = serde_json::json!(read_state);
        if read_state == "unread" {
            sqlx::query(
                r#"
                INSERT INTO submission_reads
                    (submission_id, user_id, read_state, read_at, source)
                VALUES ($1, $2, 'read', now(), 'read_status')
                ON CONFLICT (submission_id, user_id) DO UPDATE
                SET read_state = 'read', read_at = now(), source = 'read_status',
                    updated_at = now()
                "#,
            )
            .bind(submission_id)
            .bind(auth.id)
            .execute(&state.db)
            .await?;
        }
    }

    Ok(Json(entry))
}

/// `include[]` values accepted by the single-submission `show` route.
#[derive(Debug, Default)]
struct SubmissionIncludes {
    include_submission_history: bool,
    include_submission_comments: bool,
    include_rubric_assessment: bool,
    include_user: bool,
    include_read_status: bool,
}

/// Parse `include[]` from the raw query string (Canvas array spelling).
fn parse_submission_includes(raw_query: Option<&str>) -> SubmissionIncludes {
    let mut includes = SubmissionIncludes::default();
    for pair in raw_query.unwrap_or_default().split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let key = decode_query_component(raw_key).to_ascii_lowercase();
        let key = key.strip_suffix("[]").unwrap_or(key.as_str());
        if key != "include" {
            continue;
        }
        match decode_query_component(raw_value).as_str() {
            "submission_history" => includes.include_submission_history = true,
            "submission_comments" => includes.include_submission_comments = true,
            "rubric_assessment" => includes.include_rubric_assessment = true,
            "user" => includes.include_user = true,
            "read_status" => includes.include_read_status = true,
            _ => {}
        }
    }
    includes
}

/// Canvas `submission_history`: one attempt-shaped entry per submission
/// version. star-lms does not version submissions, so the current row is the
/// single history entry, like a one-attempt Canvas submission.
async fn submission_history_json(
    pool: &PgPool,
    submission_id: i64,
) -> Result<serde_json::Value, ApiError> {
    let row: Option<SubmissionRow> =
        sqlx::query_as(&format!("{SUBMISSION_SELECT} WHERE s.id = $1"))
            .bind(submission_id)
            .fetch_optional(pool)
            .await?;
    let Some(row) = row else {
        return Ok(serde_json::json!([]));
    };
    let mut attachments = load_submission_attachments(pool, &[row.id]).await?;
    let files = attachments.remove(&row.id).unwrap_or_default();
    // Canvas keys history entries by the attempt they represent; the course id
    // only shapes `url`, which is reconstructed from the row's own ids.
    let course_id: i64 = sqlx::query_scalar(
        "SELECT a.course_id FROM submissions s JOIN assignments a ON a.id = s.assignment_id WHERE s.id = $1",
    )
    .bind(submission_id)
    .fetch_one(pool)
    .await?;
    let entry = serde_json::to_value(CanvasSubmission::from_row(row, course_id, files))
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(serde_json::json!([entry]))
}

/// Canvas `submission_comments` include: the active comments on the
/// submission (lib/api/v1/submission_comment.rb fields).
async fn submission_comments_json(
    pool: &PgPool,
    submission_id: i64,
) -> Result<serde_json::Value, ApiError> {
    let rows: Vec<(
        i64,
        i64,
        Option<String>,
        String,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
    )> = sqlx::query_as(
        r#"
            SELECT c.id, c.author_id, u.name, c.comment, c.edited_at, c.created_at
            FROM submission_comments c
            LEFT JOIN users u ON u.id = c.author_id
            WHERE c.submission_id = $1 AND c.workflow_state <> 'deleted'
            ORDER BY c.created_at ASC, c.id ASC
            "#,
    )
    .bind(submission_id)
    .fetch_all(pool)
    .await?;
    let comments: Vec<serde_json::Value> = rows
        .into_iter()
        .map(
            |(id, author_id, author_name, comment, edited_at, created_at)| {
                serde_json::json!({
                    "id": id,
                    "author_id": author_id,
                    "author_name": author_name,
                    "comment": comment,
                    "edited_at": edited_at,
                    "created_at": created_at,
                })
            },
        )
        .collect();
    Ok(serde_json::json!(comments))
}

/// Canvas `rubric_assessment` include: the grading assessment for the
/// submission as `{criterion_id: {rating_id, comments, points}}`
/// (indexed_rubric_assessment_json). `null` when there is none.
async fn rubric_assessment_json(
    pool: &PgPool,
    submission_id: i64,
) -> Result<serde_json::Value, ApiError> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        r#"
        SELECT ra.data::text FROM rubric_assessments ra
        WHERE ra.submission_id = $1 AND ra.assessment_type = 'grading'
          AND ra.workflow_state = 'active'
        ORDER BY ra.id DESC
        LIMIT 1
        "#,
    )
    .bind(submission_id)
    .fetch_optional(pool)
    .await?;
    let Some((Some(data),)) = row else {
        return Ok(serde_json::Value::Null);
    };
    let data: serde_json::Value = serde_json::from_str(&data)
        .map_err(|e| ApiError::BadRequest(format!("invalid rubric assessment data: {e}")))?;
    // rubric_assessments.data is a JSONB array of criterion assessments
    // [{criterion_id, rating_id, points, comments}]; Canvas indexes it by
    // criterion id, keeping only rating_id/comments/points.
    let mut indexed = serde_json::Map::new();
    if let Some(items) = data.as_array() {
        for item in items {
            let Some(criterion_id) = item.get("criterion_id") else {
                continue;
            };
            let mut entry = serde_json::Map::new();
            if let Some(rating_id) = item.get("rating_id") {
                if !rating_id.is_null() {
                    entry.insert("rating_id".to_string(), rating_id.clone());
                }
            }
            if let Some(comments) = item.get("comments") {
                if !comments.is_null() {
                    entry.insert("comments".to_string(), comments.clone());
                }
            }
            if let Some(points) = item.get("points") {
                if !points.is_null() {
                    entry.insert("points".to_string(), points.clone());
                }
            }
            indexed.insert(criterion_id.to_string(), serde_json::Value::Object(entry));
        }
    }
    Ok(serde_json::Value::Object(indexed))
}

/// GET /courses/{courseId}/assignments/{assignmentId}/submission_summary —
/// Canvas `submission_summary` (teacher-only): {graded, ungraded,
/// not_submitted} counts over the course's active students. Canvas's
/// needs_grading = submission_type NOT NULL AND not excused AND
/// (workflow_state='pending_review' OR (state IN submitted,graded AND score
/// IS NULL)); graded = score NOT NULL AND state='graded' OR excused.
async fn submission_summary(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let (students, graded, ungraded): (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
            (SELECT COUNT(DISTINCT e.user_id)
             FROM enrollments e
             WHERE e.course_id = $1 AND e.role = 'student'
               AND e.workflow_state <> 'deleted') AS students,
            (SELECT COUNT(*) FROM submissions s
             WHERE s.assignment_id = $2
               AND ((s.score IS NOT NULL AND s.workflow_state = 'graded')
                    OR s.excused = TRUE)) AS graded,
            (SELECT COUNT(*) FROM submissions s
             WHERE s.assignment_id = $2
               AND s.submission_type IS NOT NULL
               AND s.excused = FALSE
               AND (s.workflow_state = 'pending_review'
                    OR (s.workflow_state IN ('submitted', 'graded')
                        AND s.score IS NULL))) AS ungraded
        "#,
    )
    .bind(course_id)
    .bind(assignment_id)
    .fetch_one(&state.db)
    .await?;

    let not_submitted = (students - graded - ungraded).max(0);
    Ok(Json(serde_json::json!({
        "graded": graded,
        "ungraded": ungraded,
        "not_submitted": not_submitted,
    })))
}

/// PUT/DELETE /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/read
/// — Canvas mark_submission_read / mark_submission_unread. The owner of the
/// submission or a teacher/admin may flip the state; 204 on success.
async fn mark_submission_read_state(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    read: bool,
) -> Result<StatusCode, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    ensure_user_in_course(&state.db, course_id, user_id).await?;

    if auth.id != user_id && !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas find_or_create_submission: the marker can exist even when the
    // submission row does not yet, but a submission that never existed is a
    // 404 (assignment-scoped lookup).
    let submission_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM submissions WHERE assignment_id = $1 AND user_id = $2")
            .bind(assignment_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?;
    let submission_id = submission_id.ok_or(ApiError::NotFound)?;

    sqlx::query(
        r#"
        INSERT INTO submission_reads
            (submission_id, user_id, read_state, read_at, source)
        VALUES ($1, $2, $3, now(), 'read_state')
        ON CONFLICT (submission_id, user_id) DO UPDATE
        SET read_state = $3,
            read_at = CASE WHEN $3 THEN now() ELSE submission_reads.read_at END,
            source = 'read_state',
            updated_at = now()
        "#,
    )
    .bind(submission_id)
    .bind(auth.id)
    .bind(read)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// PUT .../submissions/{userId}/read — mark read.
async fn mark_submission_read(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    mark_submission_read_state(
        State(state),
        Path((course_id, assignment_id, user_id)),
        auth,
        true,
    )
    .await
}

/// DELETE .../submissions/{userId}/read — mark unread.
async fn mark_submission_unread(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    mark_submission_read_state(
        State(state),
        Path((course_id, assignment_id, user_id)),
        auth,
        false,
    )
    .await
}

/// POST /courses/{courseId}/assignments/{assignmentId}/submissions/update_grades
/// — Canvas `bulk_update` (grade_data[<user_id>][posted_grade]).
///
/// Deviation: Canvas enqueues the work and returns a pending Progress
/// (tag "submissions_update"); star-lms has no progresses table, so the
/// grades are applied synchronously and a completed Progress-shaped body is
/// returned ({id: 0, context_id, context_type, tag, completion: 100,
/// workflow_state: "completed"}).
async fn bulk_update_grades(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::BadRequest("'grade_data' parameter required".into()))?;
    let grade_data = payload
        .get("grade_data")
        .and_then(|value| value.as_object())
        .ok_or_else(|| ApiError::BadRequest("'grade_data' parameter required".into()))?;
    if grade_data.is_empty() {
        return Err(ApiError::BadRequest(
            "'grade_data' parameter required".into(),
        ));
    }

    for (raw_user_id, entry) in grade_data {
        let user_id: i64 = raw_user_id
            .parse()
            .map_err(|_| ApiError::BadRequest("invalid grade_data user id".into()))?;
        ensure_user_in_course(&state.db, course_id, user_id).await?;

        let score = match entry.get("posted_grade") {
            Some(value) => parse_grade_value(value)?,
            None => match entry.get("excuse").and_then(|value| value.as_bool()) {
                Some(true) => None,
                _ => continue,
            },
        };

        let graded_at = if score.is_some() {
            Some(Utc::now())
        } else {
            None
        };
        let workflow_state = if score.is_some() {
            "graded"
        } else {
            "submitted"
        };
        sqlx::query(
            r#"
            INSERT INTO submissions
                (assignment_id, user_id, score, graded_at, grader_id, workflow_state)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (assignment_id, user_id) DO UPDATE
            SET score = EXCLUDED.score,
                graded_at = EXCLUDED.graded_at,
                grader_id = EXCLUDED.grader_id,
                workflow_state = EXCLUDED.workflow_state,
                updated_at = now()
            "#,
        )
        .bind(assignment_id)
        .bind(user_id)
        .bind(score)
        .bind(graded_at)
        .bind(auth.id)
        .bind(workflow_state)
        .execute(&state.db)
        .await?;
    }

    // Simplified synchronous Progress (see doc comment): the work is already
    // done when the response is returned.
    Ok(Json(serde_json::json!({
        "id": 0,
        "context_id": course_id,
        "context_type": "Course",
        "user_id": auth.id,
        "tag": "submissions_update",
        "completion": 100,
        "workflow_state": "completed",
        "created_at": Utc::now(),
        "updated_at": Utc::now(),
        "url": format!("/api/v1/progress/0"),
    })))
}

/// PUT /courses/{courseId}/submissions/bulk_mark_read — marks the caller's
/// own submissions in the course as read. Always 204 (Canvas ignores ids that
/// belong to other users or other courses).
async fn bulk_mark_read(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let submission_ids = parse_submission_id_list(&body)?;

    if !submission_ids.is_empty() {
        sqlx::query(
            r#"
            INSERT INTO submission_reads (submission_id, user_id, read_state)
            SELECT s.id, $2, 'read'
            FROM submissions s
            JOIN assignments a ON a.id = s.assignment_id
            WHERE s.id = ANY($1) AND s.user_id = $2 AND a.course_id = $3
            ON CONFLICT (submission_id, user_id) DO UPDATE
            SET read_state = 'read', updated_at = now()
            "#,
        )
        .bind(&submission_ids)
        .bind(auth.id)
        .bind(course_id)
        .execute(&state.db)
        .await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Submission read states + anonymous submissions (Phase 4)
// ---------------------------------------------------------------------------

/// Canvas `AnonymousSubmission`: a submission serialized with the student
/// identity replaced by the assignment-scoped `anonymous_id`.
#[derive(Debug, Clone, Serialize)]
pub struct AnonymousSubmission {
    pub id: i64,
    pub assignment_id: i64,
    pub anonymous_id: String,
    pub attempt: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grade: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    pub submitted_at: DateTime<Utc>,
    pub workflow_state: String,
    pub url: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct AnonymousSubmissionRow {
    id: i64,
    assignment_id: i64,
    user_id: i64,
    anonymous_id: String,
    attempt: i32,
    body: Option<String>,
    grade: Option<String>,
    score: Option<f64>,
    submitted_at: DateTime<Utc>,
    workflow_state: String,
}

const ANONYMOUS_SUBMISSION_SELECT: &str = r#"
    SELECT s.id,
           s.assignment_id,
           s.user_id,
           s.anonymous_id,
           s.attempt,
           s.body,
           s.score::float8::text AS grade,
           s.score::float8 AS score,
           s.submitted_at,
           s.workflow_state
    FROM submissions s
"#;

impl AnonymousSubmission {
    fn from_row(row: AnonymousSubmissionRow, course_id: i64) -> Self {
        Self {
            url: format!(
                "/courses/{course_id}/assignments/{}/anonymous_submissions/{}",
                row.assignment_id, row.anonymous_id
            ),
            id: row.id,
            assignment_id: row.assignment_id,
            anonymous_id: row.anonymous_id,
            attempt: row.attempt,
            body: row.body,
            grade: row.grade,
            score: row.score,
            submitted_at: row.submitted_at,
            workflow_state: row.workflow_state,
        }
    }
}

/// PUT /courses/{courseId}/submissions/{id}/clear_unread — marks the
/// submission read for the caller. Always 204, like Canvas' `clear_unread`
/// (ids outside the course are silently ignored).
async fn clear_unread(
    State(state): State<AppState>,
    Path((course_id, submission_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    sqlx::query(
        r#"
        INSERT INTO submission_reads (submission_id, user_id, read_state, read_at, source)
        SELECT s.id, $2, 'read', now(), 'clear_unread'
        FROM submissions s
        JOIN assignments a ON a.id = s.assignment_id
        WHERE s.id = $1 AND a.course_id = $3
        ON CONFLICT (submission_id, user_id) DO UPDATE
        SET read_state = 'read', read_at = now(), source = 'clear_unread', updated_at = now()
        "#,
    )
    .bind(submission_id)
    .bind(auth.id)
    .bind(course_id)
    .execute(&state.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /courses/{courseId}/assignments/{assignmentId}/gradeable_students —
/// Canvas `gradeable_students`: one `UserDisplay` per student with a
/// submission row, or an `AnonymousUserDisplay` when anonymous grading is on
/// and `allow_new_anonymous_id` is passed. Requires view-all-grades.
async fn gradeable_students(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let (sort_by_name, descending, allow_new_anonymous_id) =
        parse_gradeable_students_query(raw_query.as_deref());

    let anonymous_grading: bool =
        sqlx::query_scalar("SELECT anonymous_grading FROM assignments WHERE id = $1")
            .bind(assignment_id)
            .fetch_one(&state.db)
            .await?;
    let anonymize = allow_new_anonymous_id && anonymous_grading;

    let direction = if descending { "DESC" } else { "ASC" };
    let order_by = if anonymize {
        format!("s.anonymous_id {direction}")
    } else if sort_by_name {
        format!("u.name {direction}, u.id {direction}")
    } else {
        format!("s.user_id {direction}")
    };

    let sql = format!(
        r#"
        SELECT s.user_id, s.anonymous_id, u.name
        FROM submissions s
        JOIN assignments a ON a.id = s.assignment_id
        JOIN users u ON u.id = s.user_id
        JOIN enrollments e ON e.user_id = s.user_id AND e.course_id = a.course_id
        WHERE s.assignment_id = $1 AND e.role = 'student' AND e.workflow_state <> 'deleted'
        ORDER BY {order_by}
        "#
    );

    let rows: Vec<(i64, Option<String>, String)> = sqlx::query_as(&sql)
        .bind(assignment_id)
        .fetch_all(&state.db)
        .await?;

    let students = rows
        .into_iter()
        .map(|(user_id, anonymous_id, name)| {
            if anonymize {
                serde_json::json!({
                    "id": anonymous_id.unwrap_or_default(),
                    "display_name": "Student",
                    "avatar_image_url": serde_json::Value::Null,
                    "html_url": serde_json::Value::Null,
                    "pronouns": serde_json::Value::Null,
                })
            } else {
                serde_json::json!({
                    "id": user_id,
                    "display_name": name,
                    "avatar_image_url": serde_json::Value::Null,
                    "html_url": format!("/courses/{course_id}/users/{user_id}"),
                    "pronouns": serde_json::Value::Null,
                })
            }
        })
        .collect();

    Ok(Json(students))
}

/// Parse `sort`, `order`, and `allow_new_anonymous_id` from the query string.
fn parse_gradeable_students_query(raw_query: Option<&str>) -> (bool, bool, bool) {
    let mut sort_by_name = false;
    let mut descending = false;
    let mut allow_new_anonymous_id = false;

    for pair in raw_query.unwrap_or_default().split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default();
        match key {
            "sort" => sort_by_name = value == "name",
            "order" => descending = value.eq_ignore_ascii_case("desc"),
            "allow_new_anonymous_id" => allow_new_anonymous_id = value == "true" || value == "1",
            _ => {}
        }
    }

    (sort_by_name, descending, allow_new_anonymous_id)
}

/// GET /courses/{courseId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}
async fn show_anonymous_submission(
    State(state): State<AppState>,
    Path((course_id, assignment_id, anonymous_id)): Path<(i64, i64, String)>,
    auth: AuthUser,
) -> Result<Json<AnonymousSubmission>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    let row: Option<AnonymousSubmissionRow> = sqlx::query_as(&format!(
        "{ANONYMOUS_SUBMISSION_SELECT} WHERE s.assignment_id = $1 AND s.anonymous_id = $2"
    ))
    .bind(assignment_id)
    .bind(&anonymous_id)
    .fetch_optional(&state.db)
    .await?;

    let row = row.ok_or(ApiError::NotFound)?;

    if auth.id != row.user_id
        && !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?
    {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(AnonymousSubmission::from_row(row, course_id)))
}

/// PUT /courses/{courseId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId} —
/// grades the submission found by its anonymous id (Canvas `update_anonymous`).
async fn update_anonymous_submission(
    State(state): State<AppState>,
    Path((course_id, assignment_id, anonymous_id)): Path<(i64, i64, String)>,
    auth: AuthUser,
    Json(payload): Json<GradeSubmission>,
) -> Result<Json<AnonymousSubmission>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let submission_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM submissions WHERE assignment_id = $1 AND anonymous_id = $2",
    )
    .bind(assignment_id)
    .bind(&anonymous_id)
    .fetch_optional(&state.db)
    .await?;
    let submission_id = submission_id.ok_or(ApiError::NotFound)?;

    sqlx::query(
        r#"
        UPDATE submissions
        SET score = $2, graded_at = now(), grader_id = $3,
            workflow_state = 'graded', updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(submission_id)
    .bind(payload.score)
    .bind(auth.id)
    .execute(&state.db)
    .await?;

    let row: AnonymousSubmissionRow =
        sqlx::query_as(&format!("{ANONYMOUS_SUBMISSION_SELECT} WHERE s.id = $1"))
            .bind(submission_id)
            .fetch_one(&state.db)
            .await?;

    Ok(Json(AnonymousSubmission::from_row(row, course_id)))
}

/// POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/files
///
/// Metadata-only attachment record (Canvas upload preflight + finalize in one
/// call). Students upload for themselves; teachers/admins may upload on the
/// student's behalf. Creates the submission row lazily like Canvas'
/// `find_or_create_submission`.
async fn create_submission_file(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<SubmissionFileRequest>,
) -> Result<(StatusCode, Json<SubmissionAttachment>), ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    ensure_user_in_course(&state.db, course_id, user_id).await?;

    if auth.id != user_id && !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let filename = payload
        .filename
        .or(payload.name)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| ApiError::Validation("filename is required".into()))?;
    let display_name = payload
        .display_name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| filename.clone());
    let content_type = payload
        .content_type
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
    let size = payload.size.unwrap_or(0).max(0);

    let submission_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO submissions (assignment_id, user_id)
        VALUES ($1, $2)
        ON CONFLICT (assignment_id, user_id)
        DO UPDATE SET updated_at = submissions.updated_at
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .fetch_one(&state.db)
    .await?;

    let attachment: SubmissionAttachment = sqlx::query_as(
        r#"
        INSERT INTO submission_files
            (submission_id, user_id, filename, display_name, content_type, size)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id, filename, display_name, content_type, size, created_at, updated_at
        "#,
    )
    .bind(submission_id)
    .bind(user_id)
    .bind(&filename)
    .bind(&display_name)
    .bind(&content_type)
    .bind(size)
    .fetch_one(&state.db)
    .await?;

    // Canvas' upload preflight submits the assignment by default.
    if payload.submit_assignment.unwrap_or(true) {
        sqlx::query(
            r#"
            UPDATE submissions
            SET submission_type = 'online_upload',
                submitted_at = now(),
                workflow_state = CASE WHEN score IS NULL THEN 'submitted' ELSE 'graded' END,
                updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(submission_id)
        .execute(&state.db)
        .await?;
    }

    Ok((StatusCode::CREATED, Json(attachment)))
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
/// Canvas's per-user grading route. Accepts both the existing star-lms
/// `{score}` payload and Canvas' `{submission: {posted_grade}}` shape, and
/// creates the submission row when the student has not submitted yet
/// (Canvas `find_or_create_submission`).
async fn course_grade_submission(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<Submission>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    ensure_user_in_course(&state.db, course_id, user_id).await?;

    // Canvas requires :grade, i.e. manage_grades: teachers and admins only.
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let score = resolve_grade_payload(&payload)?
        .ok_or_else(|| ApiError::Validation("score is required".into()))?;
    let graded_at = score.map(|_| Utc::now());
    let workflow_state = if score.is_some() {
        "graded"
    } else {
        "submitted"
    };

    let submission: Submission = sqlx::query_as(
        r#"
        INSERT INTO submissions
            (assignment_id, user_id, score, graded_at, grader_id, workflow_state)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (assignment_id, user_id) DO UPDATE
        SET score = EXCLUDED.score,
            graded_at = EXCLUDED.graded_at,
            grader_id = EXCLUDED.grader_id,
            workflow_state = EXCLUDED.workflow_state,
            updated_at = now()
        RETURNING id, assignment_id, user_id, body, score::float8, submitted_at,
                  created_at, updated_at
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(score)
    .bind(graded_at)
    .bind(auth.id)
    .bind(workflow_state)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(submission))
}

// ---------------------------------------------------------------------------
// Peer reviews (Canvas PeerReviewsApiController)
// ---------------------------------------------------------------------------

/// Canvas `AssessmentRequest` peer review
/// (lib/api/v1/assessment_request.rb): the reviewed submission id is exposed
/// as `asset_id` with `asset_type` always "Submission".
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct PeerReview {
    pub id: i64,
    pub user_id: i64,
    pub assessor_id: i64,
    pub asset_id: i64,
    pub asset_type: String,
    pub workflow_state: String,
}

/// `include[]` values accepted by the peer review routes.
#[derive(Debug, Default)]
struct PeerReviewQuery {
    submission_id: Option<i64>,
    include_user: bool,
    include_submission_comments: bool,
}

/// Parse `include[]` and `submission_id` from the raw query string.
fn parse_peer_review_query(raw_query: Option<&str>) -> PeerReviewQuery {
    let mut query = PeerReviewQuery::default();
    for pair in raw_query.unwrap_or_default().split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let key = decode_query_component(raw_key).to_ascii_lowercase();
        let key = key.strip_suffix("[]").unwrap_or(key.as_str()).to_string();
        let value = decode_query_component(raw_value);
        match key.as_str() {
            "submission_id" => query.submission_id = value.parse::<i64>().ok(),
            "include" => {
                if value == "user" {
                    query.include_user = true;
                } else if value == "submission_comments" {
                    query.include_submission_comments = true;
                }
            }
            _ => {}
        }
    }
    query
}

/// Canvas `user_display_json` (id + display_name) for the include[]=user case.
async fn user_display(pool: &PgPool, user_id: i64) -> Result<serde_json::Value, ApiError> {
    let row: Option<(String,)> = sqlx::query_as(r#"SELECT name FROM users WHERE id = $1"#)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row
        .map(|(name,)| serde_json::json!({ "id": user_id, "display_name": name }))
        .unwrap_or(serde_json::Value::Null))
}

/// Fetch the submission, verify it belongs to the assignment, and return it.
/// Canvas `peer_review_assets` raises RecordNotFound on a mismatch (404).
async fn ensure_submission_in_assignment(
    pool: &PgPool,
    assignment_id: i64,
    submission_id: i64,
) -> Result<(i64, i64), ApiError> {
    let row: Option<(i64, i64)> =
        sqlx::query_as(r#"SELECT user_id, assignment_id FROM submissions WHERE id = $1"#)
            .bind(submission_id)
            .fetch_optional(pool)
            .await?;
    let (user_id, submission_assignment_id) = row.ok_or(ApiError::NotFound)?;
    if submission_assignment_id != assignment_id {
        return Err(ApiError::NotFound);
    }
    Ok((user_id, submission_id))
}

/// Select all peer reviews for an assignment, optionally narrowed to one
/// submission (Canvas `for_asset`).
async fn select_peer_reviews(
    pool: &PgPool,
    assignment_id: i64,
    submission_id: Option<i64>,
) -> Result<Vec<PeerReview>, ApiError> {
    let sql = r#"
        SELECT ar.id, ar.user_id, ar.assessor_id, ar.asset_id, ar.asset_type, ar.workflow_state
        FROM assessment_requests ar
        JOIN submissions s ON s.id = ar.asset_id
        WHERE s.assignment_id = $1
          AND ($2::bigint IS NULL OR ar.asset_id = $2)
        ORDER BY ar.id ASC
    "#;
    let rows: Vec<PeerReview> = sqlx::query_as(sql)
        .bind(assignment_id)
        .bind(submission_id)
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// GET /courses/{courseId}/assignments/{assignmentId}/peer_reviews and
/// GET .../submissions/{submissionId}/peer_reviews — Canvas `index`.
/// Teachers/admins see every review; students only see reviews of their own
/// submissions (`for_assessee`).
async fn list_peer_reviews(
    State(state): State<AppState>,
    Path(path): Path<Vec<i64>>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // Two captures on the assignment route, three on the submission route
    // (Canvas renders both with the same `index` action).
    let (course_id, assignment_id, submission_id) = match path.as_slice() {
        [course_id, assignment_id] => (*course_id, *assignment_id, None),
        [course_id, assignment_id, submission_id] => {
            (*course_id, *assignment_id, Some(*submission_id))
        }
        _ => return Err(ApiError::NotFound),
    };

    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    let can_grade = check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;
    if !can_grade && !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let query = parse_peer_review_query(raw_query.as_deref());
    let submission_id = query.submission_id.or(submission_id);
    let mut reviews = select_peer_reviews(&state.db, assignment_id, submission_id).await?;
    if !can_grade {
        reviews.retain(|review| review.user_id == auth.id);
    }

    let mut entries = Vec::with_capacity(reviews.len());
    for review in reviews {
        let mut entry = serde_json::json!({
            "id": review.id,
            "user_id": review.user_id,
            "assessor_id": review.assessor_id,
            "asset_id": review.asset_id,
            "asset_type": review.asset_type,
            "workflow_state": review.workflow_state,
        });
        if query.include_user {
            entry["user"] = user_display(&state.db, review.user_id).await?;
            entry["assessor"] = user_display(&state.db, review.assessor_id).await?;
        }
        if query.include_submission_comments {
            let comments: Vec<serde_json::Value> = sqlx::query_as(
                r#"
                SELECT id, author_id, comment, created_at
                FROM submission_comments
                WHERE submission_id = $1 AND workflow_state <> 'deleted'
                ORDER BY id ASC
                "#,
            )
            .bind(review.asset_id)
            .fetch_all(&state.db)
            .await?;
            entry["submission_comments"] = serde_json::json!(comments);
        }
        entries.push(entry);
    }

    Ok(Json(entries))
}

/// POST /courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews
/// — Canvas `create`. The submission is lazily created for its owner like
/// Canvas' `find_or_create_submission`; the reviewer (user_id param) must be
/// an active student in the course. 400 "Create failed" when the reviewer is
/// the submission owner.
async fn create_peer_review(
    State(state): State<AppState>,
    Path((course_id, assignment_id, submission_id)): Path<(i64, i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    let (student_id, _) =
        ensure_submission_in_assignment(&state.db, assignment_id, submission_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let reviewer_id: i64 = parse_peer_review_user_id(&body)?;

    // Canvas `peer_review_assets` resolves the reviewer among visible
    // students; an unknown or non-student reviewer is a 404.
    let reviewer_is_student: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM enrollments
            WHERE course_id = $1 AND user_id = $2
              AND role = 'student' AND workflow_state <> 'deleted'
        )
        "#,
    )
    .bind(course_id)
    .bind(reviewer_id)
    .fetch_one(&state.db)
    .await?;
    if !reviewer_is_student {
        return Err(ApiError::NotFound);
    }

    if reviewer_id == student_id {
        return Err(ApiError::BadRequest("Create failed".into()));
    }

    // Canvas `assign_peer_review` also ensures the reviewer has a submission.
    sqlx::query(
        r#"
        INSERT INTO submissions (assignment_id, user_id)
        VALUES ($1, $2)
        ON CONFLICT (assignment_id, user_id)
        DO UPDATE SET updated_at = submissions.updated_at
        "#,
    )
    .bind(assignment_id)
    .bind(reviewer_id)
    .execute(&state.db)
    .await?;

    let review: PeerReview = sqlx::query_as(
        r#"
        INSERT INTO assessment_requests
            (user_id, assessor_id, asset_id, asset_type, workflow_state)
        VALUES ($1, $2, $3, 'Submission', 'assigned')
        ON CONFLICT (asset_id, assessor_id) DO UPDATE
        SET updated_at = now()
        RETURNING id, user_id, assessor_id, asset_id, asset_type, workflow_state
        "#,
    )
    .bind(student_id)
    .bind(reviewer_id)
    .bind(submission_id)
    .fetch_one(&state.db)
    .await?;

    // Canvas keeps the per-submission assessment_request_count bumped on the
    // submission row; star-lms mirrors it on the assignment's
    // peer_review_count for the whole set of assigned reviews.
    sqlx::query(
        r#"
        UPDATE assignments
        SET peer_review_count = (
            SELECT COUNT(*) FROM assessment_requests ar
            JOIN submissions s ON s.id = ar.asset_id
            WHERE s.assignment_id = $1
        ), updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(assignment_id)
    .execute(&state.db)
    .await?;

    let mut entry = serde_json::json!({
        "id": review.id,
        "user_id": review.user_id,
        "assessor_id": review.assessor_id,
        "asset_id": review.asset_id,
        "asset_type": review.asset_type,
        "workflow_state": review.workflow_state,
    });
    let query = parse_peer_review_query(raw_query.as_deref());
    if query.include_user {
        entry["user"] = user_display(&state.db, review.user_id).await?;
        entry["assessor"] = user_display(&state.db, review.assessor_id).await?;
    }

    Ok((StatusCode::CREATED, Json(entry)))
}

/// Extract `user_id` from a JSON or form-encoded peer review body.
fn parse_peer_review_user_id(body: &[u8]) -> Result<i64, ApiError> {
    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(id) = json.get("user_id").and_then(json_id) {
            return Ok(id);
        }
    }
    let body = String::from_utf8_lossy(body);
    for pair in body.split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let key = decode_query_component(raw_key).to_ascii_lowercase();
        if key == "user_id" {
            if let Ok(id) = decode_query_component(raw_value).trim().parse::<i64>() {
                return Ok(id);
            }
        }
    }
    Err(ApiError::BadRequest("user_id is required".into()))
}

/// DELETE /courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews
/// — Canvas `destroy`. 400 "Delete failed" when no review matches the
/// (submission, reviewer, assessee) triple.
async fn destroy_peer_review(
    State(state): State<AppState>,
    Path((course_id, assignment_id, submission_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;
    let (student_id, _) =
        ensure_submission_in_assignment(&state.db, assignment_id, submission_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let reviewer_id = parse_peer_review_user_id(&body)?;

    let review: Option<PeerReview> = sqlx::query_as(
        r#"
        DELETE FROM assessment_requests
        WHERE asset_id = $1 AND assessor_id = $2 AND user_id = $3
        RETURNING id, user_id, assessor_id, asset_id, asset_type, workflow_state
        "#,
    )
    .bind(submission_id)
    .bind(reviewer_id)
    .bind(student_id)
    .fetch_optional(&state.db)
    .await?;

    let review = review.ok_or_else(|| ApiError::BadRequest("Delete failed".into()))?;

    sqlx::query(
        r#"
        UPDATE assignments
        SET peer_review_count = (
            SELECT COUNT(*) FROM assessment_requests ar
            JOIN submissions s ON s.id = ar.asset_id
            WHERE s.assignment_id = $1
        ), updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(assignment_id)
    .execute(&state.db)
    .await?;

    Ok(Json(serde_json::json!({
        "id": review.id,
        "user_id": review.user_id,
        "assessor_id": review.assessor_id,
        "asset_id": review.asset_id,
        "asset_type": review.asset_type,
        "workflow_state": review.workflow_state,
    })))
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
        // Canvas submissions API (SubmissionsApiController)
        .route(
            "/courses/{courseId}/students/submissions",
            get(list_student_submissions),
        )
        .route(
            "/courses/{courseId}/submissions/bulk_mark_read",
            put(bulk_mark_read),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}",
            get(get_student_submission),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/files",
            post(create_submission_file),
        )
        // Canvas submissions API extras (Phase 5)
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submission_summary",
            get(submission_summary),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/read",
            put(mark_submission_read),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/read",
            delete(mark_submission_unread),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/update_grades",
            post(bulk_update_grades),
        )
        // Submission read states + anonymous submissions (Phase 4)
        .route(
            "/courses/{courseId}/submissions/{id}/clear_unread",
            put(clear_unread),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/gradeable_students",
            get(gradeable_students),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}",
            get(show_anonymous_submission),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}",
            put(update_anonymous_submission),
        )
        // Canvas peer reviews API (PeerReviewsApiController)
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/peer_reviews",
            get(list_peer_reviews),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews",
            get(list_peer_reviews),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews",
            post(create_peer_review),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews",
            delete(destroy_peer_review),
        )
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
