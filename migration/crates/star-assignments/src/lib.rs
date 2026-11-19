//! star-assignments: assignments, submissions, and grading for star-lms.
//!
//! Implements the Canvas-style assignments API with enrollment-based access control.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};
use std::collections::{HashMap, HashSet};

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
    include_provisional_grades: bool,
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
            "include" => {
                if value == "provisional_grades" {
                    query.include_provisional_grades = true;
                }
            }
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

    // Canvas: include[]=provisional_grades is honored per submission, and
    // only for moderated assignments.
    let moderated_includes: HashMap<i64, ModeratedAssignment> = if query.include_provisional_grades
    {
        let mut map = HashMap::new();
        for id in &assignment_ids {
            if let Some(assignment) = load_moderated_assignment(&state.db, course_id, *id)
                .await
                .ok()
            {
                if assignment.moderated_grading {
                    map.insert(*id, assignment);
                }
            }
        }
        map
    } else {
        HashMap::new()
    };

    if !query.grouped {
        let mut submissions: Vec<serde_json::Value> = Vec::new();
        for row in rows {
            let files = attachments.remove(&row.id).unwrap_or_default();
            let rendered = CanvasSubmission::from_row(row, course_id, files);
            let mut value =
                serde_json::to_value(&rendered).map_err(|e| ApiError::BadRequest(e.to_string()))?;
            append_provisional_grades_include(
                &state.db,
                &moderated_includes,
                rendered.id,
                course_id,
                auth.id,
                &auth,
                &mut value,
            )
            .await?;
            submissions.push(value);
        }
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

    let mut submissions_by_user: HashMap<i64, Vec<serde_json::Value>> = HashMap::new();
    for row in rows {
        let files = attachments.remove(&row.id).unwrap_or_default();
        let rendered = CanvasSubmission::from_row(row, course_id, files);
        let mut value =
            serde_json::to_value(&rendered).map_err(|e| ApiError::BadRequest(e.to_string()))?;
        append_provisional_grades_include(
            &state.db,
            &moderated_includes,
            rendered.id,
            course_id,
            auth.id,
            &auth,
            &mut value,
        )
        .await?;
        submissions_by_user
            .entry(rendered.user_id)
            .or_default()
            .push(value);
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
    if query.include_provisional_grades {
        // Canvas: the include is only honored for moderated assignments.
        let assignment = load_moderated_assignment(&state.db, course_id, assignment_id).await?;
        if assignment.moderated_grading {
            let submission_ref: ProvisionalSubmissionRef = sqlx::query_as(
                r#"SELECT id, user_id, anonymous_id, submitted_at FROM submissions WHERE id = $1"#,
            )
            .bind(submission_id)
            .fetch_one(&state.db)
            .await?;
            entry["provisional_grades"] = serde_json::json!(
                submission_provisional_grades_json(
                    &state.db,
                    course_id,
                    &assignment,
                    &submission_ref,
                    auth.id,
                    &auth,
                )
                .await?
            );
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
    include_provisional_grades: bool,
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
            "provisional_grades" => includes.include_provisional_grades = true,
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

/// Row shape for the `submission_comments` include (id, author_id, author
/// name, comment, edited_at, created_at).
type SubmissionCommentRow = (
    i64,
    i64,
    Option<String>,
    String,
    Option<DateTime<Utc>>,
    DateTime<Utc>,
);

/// Canvas `submission_comments` include: the active comments on the
/// submission (lib/api/v1/submission_comment.rb fields).
async fn submission_comments_json(
    pool: &PgPool,
    submission_id: i64,
) -> Result<serde_json::Value, ApiError> {
    let rows: Vec<SubmissionCommentRow> = sqlx::query_as(
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

    // Canvas change_read_state stores 'read'/'unread' for the submission's
    // user (the path user), not the caller.
    let new_state = if read { "read" } else { "unread" };
    sqlx::query(
        r#"
        INSERT INTO submission_reads
            (submission_id, user_id, read_state, read_at, source)
        VALUES ($1, $2, $3, now(), 'read_state')
        ON CONFLICT (submission_id, user_id) DO UPDATE
            SET read_state = $3,
                read_at = CASE WHEN $3 = 'read' THEN now() ELSE submission_reads.read_at END,
                source = 'read_state',
                updated_at = now()
        "#,
    )
    .bind(submission_id)
    .bind(user_id)
    .bind(new_state)
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
        "url": String::from("/api/v1/progress/0"),
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
/// submission row, or an `AnonymousUserDisplay` when the caller cannot view
/// student names and `allow_new_anonymous_id` is passed. Requires
/// manage_grades/view_all_grades (teachers/admins in star-lms).
///
/// `can_view_student_names?` parity (spec §6.1): names are hidden when the
/// assignment `anonymize_students?` — anonymous grading on and (for the
/// snapshot's non-moderated assignments) at least one active student has an
/// unposted submission. Canvas models "unposted" as `posted_at IS NULL`; the
/// Star snapshot has no `posted_at`, so an ungraded submission
/// (`graded_at IS NULL`) is the equivalent gate.
async fn gradeable_students(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let assignment = load_moderated_assignment(&state.db, course_id, assignment_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let (sort_by_name, descending, allow_new_anonymous_id, include_provisional_grades) =
        parse_gradeable_students_query(raw_query.as_deref());

    // Canvas: include[]=provisional_grades additionally requires
    // permits_moderation? (render_unauthorized_action otherwise).
    if include_provisional_grades && !permits_moderation(&state.db, &assignment, auth.id).await? {
        return Err(ApiError::Unauthorized);
    }

    // Canvas: can_view_student_names = allow_new_anonymous_id ?
    // assignment.can_view_student_names?(user) : true. A teacher arriving
    // without the flag always sees names.
    let anonymize = allow_new_anonymous_id
        && assignment_anonymize_students_moderated(&state.db, &assignment).await?;

    // Canvas: sort=name orders by sortable_name when names are visible and
    // falls back to anonymous_id when hidden; the default order is user_id
    // when visible and anonymous_id when hidden.
    let direction = if descending { "DESC" } else { "ASC" };
    let order_by = if anonymize {
        format!("s.anonymous_id {direction}")
    } else if sort_by_name {
        // COALESCE mirrors Canvas' tolerance of users without a sortable
        // name; the secondary id keeps the order deterministic.
        format!("COALESCE(u.sortable_name, u.name) {direction}, u.id {direction}")
    } else {
        format!("s.user_id {direction}")
    };

    let sql = format!(
        r#"
        SELECT s.id, s.user_id, s.anonymous_id, u.name
        FROM submissions s
        JOIN assignments a ON a.id = s.assignment_id
        JOIN users u ON u.id = s.user_id
        JOIN enrollments e ON e.user_id = s.user_id AND e.course_id = a.course_id
        WHERE s.assignment_id = $1 AND e.role = 'student' AND e.workflow_state <> 'deleted'
        ORDER BY {order_by}
        "#
    );

    let rows: Vec<(i64, i64, Option<String>, String)> = sqlx::query_as(&sql)
        .bind(assignment_id)
        .fetch_all(&state.db)
        .await?;

    let display = |submission_id: i64, user_id: i64, anonymous_id: &Option<String>, name: &str| {
        if anonymize {
            // Canvas AnonymousUserDisplay (lib/api/v1/user.rb):
            // {anonymous_id, avatar_image_url (default fallback),
            // display_name}. The snapshot has no per-student pseudonym table
            // (spec open question 2), so display_name stays the placeholder
            // "Student" and the fallback avatar is null.
            let _ = (submission_id, name);
            serde_json::json!({
                "id": anonymous_id.clone().unwrap_or_default(),
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
    };

    if !include_provisional_grades {
        let students = rows
            .into_iter()
            .map(|(submission_id, user_id, anonymous_id, name)| {
                display(submission_id, user_id, &anonymous_id, &name)
            })
            .collect();
        return Ok(Json(students));
    }

    // Canvas: in_moderation_set is true when a selection exists that points
    // at one of this submission's provisional grades (the quirk from
    // `submission.provisional_grades.find(&:selection)`); the pg list is
    // role-filtered like every other provisional include.
    let mut enriched: Vec<serde_json::Value> = Vec::new();
    for (submission_id, user_id, anonymous_id, name) in rows {
        let mut json = display(submission_id, user_id, &anonymous_id, &name);

        let submission_ref = ProvisionalSubmissionRef {
            id: submission_id,
            user_id,
            anonymous_id: anonymous_id.clone(),
            submitted_at: None,
        };

        // The selection whose selected pg belongs to this submission.
        let selected: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT sel.selected_provisional_grade_id
            FROM moderated_grading_selections sel
            WHERE sel.assignment_id = $1 AND sel.student_id = $2
              AND sel.selected_provisional_grade_id IN (
                  SELECT id FROM provisional_grades WHERE submission_id = $3
              )
            "#,
        )
        .bind(assignment_id)
        .bind(user_id)
        .bind(submission_id)
        .fetch_optional(&state.db)
        .await?
        .flatten();

        json["in_moderation_set"] = serde_json::json!(selected.is_some());
        json["selected_provisional_grade_id"] = serde_json::json!(selected);
        json["provisional_grades"] = serde_json::json!(
            submission_provisional_grades_json(
                &state.db,
                course_id,
                &assignment,
                &submission_ref,
                auth.id,
                &auth,
            )
            .await?
        );
        enriched.push(json);
    }

    Ok(Json(enriched))
}

/// Parse `sort`, `order`, `allow_new_anonymous_id`, and `include[]` from
/// the query string.
fn parse_gradeable_students_query(raw_query: Option<&str>) -> (bool, bool, bool, bool) {
    let mut sort_by_name = false;
    let mut descending = false;
    let mut allow_new_anonymous_id = false;
    let mut include_provisional_grades = false;

    for pair in raw_query.unwrap_or_default().split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default();
        let value = parts.next().unwrap_or_default();
        match key {
            "sort" => sort_by_name = value == "name",
            "order" => descending = value.eq_ignore_ascii_case("desc"),
            "allow_new_anonymous_id" => allow_new_anonymous_id = value == "true" || value == "1",
            "include[]" | "include" => {
                include_provisional_grades = value == "provisional_grades";
            }
            _ => {}
        }
    }

    (
        sort_by_name,
        descending,
        allow_new_anonymous_id,
        include_provisional_grades,
    )
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

// ---------------------------------------------------------------------------
// Anonymous submissions: section twins (spec p5 slice E, #30)
// ---------------------------------------------------------------------------

/// Minimal section row for the section-scoped anonymous submission routes.
#[derive(Debug, Clone, sqlx::FromRow)]
struct SectionRef {
    course_id: i64,
}

/// Load a section and return its course. Canvas' `get_course_from_section`
/// (`api_find(CourseSection, params[:section_id])`) makes an unknown section a
/// 404 before any course-scoped lookup happens.
async fn load_section_course(pool: &PgPool, section_id: i64) -> Result<i64, ApiError> {
    let section: Option<SectionRef> =
        sqlx::query_as("SELECT course_id FROM course_sections WHERE id = $1")
            .bind(section_id)
            .fetch_optional(pool)
            .await?;
    Ok(section.ok_or(ApiError::NotFound)?.course_id)
}

/// GET /sections/{sectionId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}
/// — Canvas `show_anonymous`, section variant.
///
/// Canvas resolves the section to its course (`get_course_from_section`), then
/// scopes the student lookup to the section via `get_user_considering_section`
/// (`students.where(enrollments: { course_section_id: @section })`), so the
/// submission must belong to a student enrolled in that section.
async fn section_show_anonymous_submission(
    State(state): State<AppState>,
    Path((section_id, assignment_id, anonymous_id)): Path<(i64, i64, String)>,
    auth: AuthUser,
) -> Result<Json<AnonymousSubmission>, ApiError> {
    let course_id = load_section_course(&state.db, section_id).await?;
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    let row: Option<AnonymousSubmissionRow> = sqlx::query_as(&format!(
        "{ANONYMOUS_SUBMISSION_SELECT} \
         WHERE s.assignment_id = $1 AND s.anonymous_id = $2 \
           AND s.user_id IN (SELECT user_id FROM enrollments \
                              WHERE course_section_id = $3 \
                                AND workflow_state <> 'deleted')"
    ))
    .bind(assignment_id)
    .bind(&anonymous_id)
    .bind(section_id)
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

/// PUT /sections/{sectionId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}
/// — Canvas `update_anonymous`, section variant (grade update path).
///
/// Same resolution as the GET twin; Canvas `update_anonymous` funnels into the
/// ordinary `update` action, whose `:grade` right means teachers/admins only.
async fn section_update_anonymous_submission(
    State(state): State<AppState>,
    Path((section_id, assignment_id, anonymous_id)): Path<(i64, i64, String)>,
    auth: AuthUser,
    Json(payload): Json<GradeSubmission>,
) -> Result<Json<AnonymousSubmission>, ApiError> {
    let course_id = load_section_course(&state.db, section_id).await?;
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let submission_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT s.id FROM submissions s
        WHERE s.assignment_id = $1 AND s.anonymous_id = $2
          AND s.user_id IN (SELECT user_id FROM enrollments
                             WHERE course_section_id = $3
                               AND workflow_state <> 'deleted')
        "#,
    )
    .bind(assignment_id)
    .bind(&anonymous_id)
    .bind(section_id)
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
///
/// Canvas `submissions_api#update` moderation params (spec p5 slice F, #31):
/// `submission[provisional]=true` routes the grade into a provisional grade
/// row instead of the submission; `submission[final]` is honored only for
/// moderators; a non-provisional `posted_grade` on a moderated, unpublished
/// assignment is a 401 before any grading happens.
async fn course_grade_submission(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let assignment = load_moderated_assignment(&state.db, course_id, assignment_id).await?;

    let submission_params = payload
        .get("submission")
        .and_then(|value| value.as_object());
    let posted_grade = submission_params.and_then(|params| params.get("posted_grade"));
    let provisional =
        json_value_to_boolean(submission_params.and_then(|params| params.get("provisional")));
    let final_flag =
        json_value_to_boolean(submission_params.and_then(|params| params.get("final")));

    // Canvas rejects a non-provisional posted_grade on a moderated,
    // unpublished assignment (render_unauthorized_action) before resolving
    // the user.
    if let Some(grade) = posted_grade {
        if !grade.is_null()
            && !provisional
            && assignment.moderated_grading
            && !assignment.grades_published()
        {
            return Err(ApiError::Unauthorized);
        }
    }

    ensure_user_in_course(&state.db, course_id, user_id).await?;

    // Canvas requires :grade, i.e. manage_grades: teachers and admins only.
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if provisional {
        return grade_submission_provisional(
            state,
            assignment,
            user_id,
            auth,
            &payload,
            posted_grade,
            final_flag,
        )
        .await;
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

    let value =
        serde_json::to_value(&submission).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    Ok(Json(value))
}

/// The provisional branch of `PUT .../submissions/{userId}` (Canvas
/// `Assignment#grade_student` with `provisional: true`): the submission row
/// is created but never scored; the grade lands in a `provisional_grades`
/// row owned by the scorer (or the single final grade for moderators).
async fn grade_submission_provisional(
    state: AppState,
    assignment: ModeratedAssignment,
    user_id: i64,
    auth: AuthUser,
    payload: &serde_json::Value,
    posted_grade: Option<&serde_json::Value>,
    final_flag: bool,
) -> Result<Json<serde_json::Value>, ApiError> {
    // find_or_create_submission — the row exists afterwards, ungraded.
    let inserted: Option<Submission> = sqlx::query_as(
        r#"
        INSERT INTO submissions (assignment_id, user_id)
        VALUES ($1, $2)
        ON CONFLICT (assignment_id, user_id) DO NOTHING
        RETURNING id, assignment_id, user_id, body, score::float8, submitted_at,
                  created_at, updated_at
        "#,
    )
    .bind(assignment.id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    let submission: Submission = match inserted {
        Some(submission) => submission,
        None => {
            sqlx::query_as(
                r#"
                SELECT id, assignment_id, user_id, body, score::float8, submitted_at,
                       created_at, updated_at
                FROM submissions WHERE assignment_id = $1 AND user_id = $2
                "#,
            )
            .bind(assignment.id)
            .bind(user_id)
            .fetch_one(&state.db)
            .await?
        }
    };
    let submission_id = submission.id;

    // Canvas: submission[:final] = value_to_boolean(final) &&
    // permits_moderation? — silently downgraded without the right.
    let wants_final = final_flag && permits_moderation(&state.db, &assignment, auth.id).await?;

    // The grade itself (posted_grade path); `None` means a comment-only or
    // empty provisional request.
    let score = match posted_grade {
        None => None,
        Some(value) => parse_grade_value(value)?,
    };

    // ensure_grader_can_adjudicate: every provisional grade occupies a grader
    // slot on a moderated assignment (moderation_graders row, slot_taken).
    if assignment.moderated_grading {
        ensure_grader_slot(&state.db, &assignment, auth.id).await?;
    }

    let comment_text = payload
        .get("comment")
        .and_then(|value| value.get("text_comment"))
        .and_then(|value| value.as_str());

    if score.is_some() || comment_text.is_some() {
        if let Some(score_value) = score {
            // find_or_create_provisional_grade!(grader, grade:, score:,
            // final:) — grade defaults to the points-form string (Canvas
            // score_to_grade for grading_type "points").
            let grade_text = format_points_grade(score_value);
            upsert_provisional_grade(
                &state.db,
                &assignment,
                &auth,
                submission_id,
                wants_final,
                Some(score_value),
                Some(grade_text),
            )
            .await?;
        } else {
            // Comment-only provisional: a gradeless provisional row owned by
            // the commenter (graded_at stays NULL), with the comment attached.
            let pg = upsert_provisional_grade(
                &state.db,
                &assignment,
                &auth,
                submission_id,
                false,
                None,
                None,
            )
            .await?;
            if let Some(text) = comment_text {
                sqlx::query(
                    r#"
                    INSERT INTO submission_comments
                        (submission_id, author_id, comment, provisional_grade_id)
                    VALUES ($1, $2, $3, $4)
                    "#,
                )
                .bind(submission_id)
                .bind(auth.id)
                .bind(text)
                .bind(pg.id)
                .execute(&state.db)
                .await?;
            }
        }
        // Canvas touches the submission when a provisional grade is saved.
        sqlx::query("UPDATE submissions SET updated_at = now() WHERE id = $1")
            .bind(submission_id)
            .execute(&state.db)
            .await?;
    }

    // Response: the submission plus provisional_grades when the assignment is
    // moderated (Canvas adds the include when the request was provisional).
    let mut value =
        serde_json::to_value(&submission).map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if assignment.moderated_grading {
        let submission_ref = ProvisionalSubmissionRef {
            id: submission_id,
            user_id,
            anonymous_id: None,
            submitted_at: Some(submission.submitted_at),
        };
        // The anonymous id matters when names are hidden; fetch it.
        let anonymous_id: Option<String> =
            sqlx::query_scalar("SELECT anonymous_id FROM submissions WHERE id = $1")
                .bind(submission_id)
                .fetch_one(&state.db)
                .await?;
        let submission_ref = ProvisionalSubmissionRef {
            anonymous_id,
            ..submission_ref
        };
        value["provisional_grades"] = serde_json::json!(
            submission_provisional_grades_json(
                &state.db,
                assignment.course_id,
                &assignment,
                &submission_ref,
                auth.id,
                &auth,
            )
            .await?
        );
    }
    Ok(Json(value))
}

/// Canvas `Assignment#ensure_grader_can_adjudicate` (provisional path):
/// create or occupy the grader's moderation_graders slot; the grader limit
/// rejects new graders once the slots are full.
async fn ensure_grader_slot(
    pool: &PgPool,
    assignment: &ModeratedAssignment,
    grader_id: i64,
) -> Result<(), ApiError> {
    let existing: Option<(bool,)> = sqlx::query_as(
        "SELECT slot_taken FROM moderation_graders WHERE assignment_id = $1 AND user_id = $2",
    )
    .bind(assignment.id)
    .bind(grader_id)
    .fetch_optional(pool)
    .await?;

    let limit_reached = moderated_grader_limit_reached(pool, assignment).await?;

    match existing {
        None => {
            // create_moderation_grader(occupy_slot: true) with a fresh
            // 5-char anonymous id (Canvas Anonymity.generate_id).
            if limit_reached && assignment.final_grader_id != Some(grader_id) {
                return Err(ApiError::BadRequest(
                    "The maximum number of graders has been reached for this assignment.".into(),
                ));
            }
            insert_moderation_grader_slot(pool, assignment.id, grader_id).await?;
        }
        Some((slot_taken,)) => {
            if !slot_taken {
                if limit_reached && assignment.final_grader_id != Some(grader_id) {
                    return Err(ApiError::BadRequest(
                        "The maximum number of graders has been reached for this assignment."
                            .into(),
                    ));
                }
                sqlx::query(
                    "UPDATE moderation_graders SET slot_taken = TRUE, updated_at = now() \
                     WHERE assignment_id = $1 AND user_id = $2",
                )
                .bind(assignment.id)
                .bind(grader_id)
                .execute(pool)
                .await?;
            }
        }
    }
    Ok(())
}

/// Insert a moderation_graders row with a random 5-char alphanumeric
/// anonymous id (Canvas `Anonymity.generate_id`), retrying on collisions.
async fn insert_moderation_grader_slot(
    pool: &PgPool,
    assignment_id: i64,
    grader_id: i64,
) -> Result<(), ApiError> {
    // Simple deterministic-enough generator without a dependency: mix the
    // assignment/grader ids and time into 5 chars of [A-Za-z0-9].
    for _ in 0..5 {
        let entropy = format!(
            "{}-{}-{}",
            assignment_id,
            grader_id,
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let hash = format!("{:x}", fnv1a_hash(&entropy));
        let candidate: String = hash.chars().take(5).collect();
        if candidate.len() == 5 && candidate.chars().all(char::is_ascii_alphanumeric) {
            let inserted = sqlx::query(
                r#"
                INSERT INTO moderation_graders (assignment_id, user_id, anonymous_id, slot_taken)
                VALUES ($1, $2, $3, TRUE)
                ON CONFLICT (assignment_id, user_id) DO NOTHING
                "#,
            )
            .bind(assignment_id)
            .bind(grader_id)
            .bind(&candidate)
            .execute(pool)
            .await?;
            if inserted.rows_affected() > 0 {
                return Ok(());
            }
        }
    }
    Err(ApiError::Db(
        "could not allocate grader anonymous id".into(),
    ))
}

/// A tiny FNV-1a hash standing in for a random id source.
fn fnv1a_hash(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Canvas `round_if_whole(score).to_s` for the default points grading type:
/// whole numbers lose the decimal ("90"), others keep it ("88.5").
fn format_points_grade(score: f64) -> String {
    if score.trunc() == score && score.abs() < 1e15 {
        format!("{}", score as i64)
    } else {
        format!("{score}")
    }
}

/// Canvas `Submission#find_or_create_provisional_grade!` for the update
/// route: upsert the scorer's non-final grade, or (for moderators) the
/// submission's single final grade, enforcing the Canvas validations.
async fn upsert_provisional_grade(
    pool: &PgPool,
    assignment: &ModeratedAssignment,
    auth: &AuthUser,
    submission_id: i64,
    final_grade: bool,
    score: Option<f64>,
    grade: Option<String>,
) -> Result<ProvisionalGradeRow, ApiError> {
    if final_grade {
        // must_have_non_final_provisional_grade_to_create_final
        let non_final: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM provisional_grades WHERE submission_id = $1 AND NOT final",
        )
        .bind(submission_id)
        .fetch_optional(pool)
        .await?;
        if non_final.is_none() {
            return Err(ApiError::BadRequest(
                "Cannot give a final mark for a student with no other provisional grades".into(),
            ));
        }
    }

    let existing: Option<ProvisionalGradeRow> = if final_grade {
        sqlx::query_as(
            r#"
            SELECT id, submission_id, scorer_id, score::float8, grade, graded_at,
                   final AS is_final, graded_anonymously, created_at
            FROM provisional_grades WHERE submission_id = $1 AND final
            "#,
        )
        .bind(submission_id)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT id, submission_id, scorer_id, score::float8, grade, graded_at,
                   final AS is_final, graded_anonymously, created_at
            FROM provisional_grades WHERE submission_id = $1 AND NOT final AND scorer_id = $2
            "#,
        )
        .bind(submission_id)
        .bind(auth.id)
        .fetch_optional(pool)
        .await?
    };

    // set_graded_at: graded_at is set whenever a grade or score is recorded.
    let graded_at = if score.is_some() || grade.is_some() {
        Some(Utc::now())
    } else {
        None
    };

    let was_existing = existing.is_some();
    let saved: ProvisionalGradeRow = match existing {
        Some(existing) => {
            // update_provisional_grade: a selected provisional grade cannot
            // be modified by its scorer (only the final grader may).
            if score.is_some() || grade.is_some() {
                let selected: Option<(i32,)> = sqlx::query_as(
                    r#"
                    SELECT 1 FROM moderated_grading_selections
                    WHERE selected_provisional_grade_id = $1
                    "#,
                )
                .bind(existing.id)
                .fetch_optional(pool)
                .await?;
                if selected.is_some() && assignment.final_grader_id != Some(auth.id) {
                    return Err(ApiError::BadRequest(
                        "provisional grade has already been selected".into(),
                    ));
                }
            }
            sqlx::query_as(
                r#"
                UPDATE provisional_grades
                SET score = $2, grade = $3, graded_at = $4, final = $5, updated_at = now()
                WHERE id = $1
                RETURNING id, submission_id, scorer_id, score::float8, grade, graded_at,
                          final AS is_final, graded_anonymously, created_at
                "#,
            )
            .bind(existing.id)
            .bind(score)
            .bind(&grade)
            .bind(graded_at)
            .bind(final_grade)
            .fetch_one(pool)
            .await?
        }
        None => {
            // must_be_final_or_student_in_need_of_provisional_grade: a new
            // non-final grade requires a moderated-grader slot for the scorer.
            if !final_grade && !can_be_moderated_grader(pool, assignment, auth.id, auth).await? {
                return Err(ApiError::BadRequest(
                    "Student already has the maximum number of provisional grades".into(),
                ));
            }
            sqlx::query_as(
                r#"
                INSERT INTO provisional_grades
                    (submission_id, scorer_id, score, grade, graded_at, final)
                VALUES ($1, $2, $3, $4, $5, $6)
                RETURNING id, submission_id, scorer_id, score::float8, grade, graded_at,
                          final AS is_final, graded_anonymously, created_at
                "#,
            )
            .bind(submission_id)
            .bind(auth.id)
            .bind(score)
            .bind(&grade)
            .bind(graded_at)
            .bind(final_grade)
            .fetch_one(pool)
            .await?
        }
    };

    let event_type = if was_existing {
        "provisional_grade_updated"
    } else {
        "provisional_grade_created"
    };
    create_provisional_grade_event(
        pool,
        assignment.id,
        auth.id,
        submission_id,
        event_type,
        provisional_grade_event_payload(&saved),
    )
    .await?;

    Ok(saved)
}

// ---------------------------------------------------------------------------
// Moderated / provisional grading (Canvas ModerationSetController,
// ProvisionalGradesController, AnonymousProvisionalGradesController — #31)
// ---------------------------------------------------------------------------

/// Moderation-relevant assignment columns (Canvas `AbstractAssignment`).
#[derive(Debug, Clone)]
struct ModeratedAssignment {
    id: i64,
    course_id: i64,
    final_grader_id: Option<i64>,
    moderated_grading: bool,
    grades_published_at: Option<DateTime<Utc>>,
    grader_count: i32,
    grader_names_visible_to_final_grader: bool,
    grader_comments_visible_to_graders: bool,
    graders_anonymous_to_graders: bool,
    anonymous_grading: bool,
}

impl ModeratedAssignment {
    /// Canvas `AbstractAssignment#grades_published?`: `!moderated_grading? ||
    /// grades_published_at.present?`.
    fn grades_published(&self) -> bool {
        !self.moderated_grading || self.grades_published_at.is_some()
    }
}

/// Load the moderation columns of an assignment inside a course. Canvas
/// resolves the assignment with `api_find(@context.assignments, ...)`, so an
/// unknown assignment (or one from another course) is a 404.
async fn load_moderated_assignment(
    pool: &PgPool,
    course_id: i64,
    assignment_id: i64,
) -> Result<ModeratedAssignment, ApiError> {
    let row = sqlx::query_as::<
        _,
        (
            i64,
            Option<i64>,
            bool,
            Option<DateTime<Utc>>,
            i32,
            bool,
            bool,
            bool,
            bool,
        ),
    >(
        r#"
        SELECT id, final_grader_id, moderated_grading, grades_published_at, grader_count,
               grader_names_visible_to_final_grader, grader_comments_visible_to_graders,
               graders_anonymous_to_graders, anonymous_grading
        FROM assignments WHERE id = $1 AND course_id = $2
        "#,
    )
    .bind(assignment_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    let Some((
        id,
        final_grader_id,
        moderated_grading,
        grades_published_at,
        grader_count,
        grader_names_visible_to_final_grader,
        grader_comments_visible_to_graders,
        graders_anonymous_to_graders,
        anonymous_grading,
    )) = row
    else {
        return Err(ApiError::NotFound);
    };

    Ok(ModeratedAssignment {
        id,
        course_id,
        final_grader_id,
        moderated_grading,
        grades_published_at,
        grader_count,
        grader_names_visible_to_final_grader,
        grader_comments_visible_to_graders,
        graders_anonymous_to_graders,
        anonymous_grading,
    })
}

/// Canvas `Course#account_membership_allows(user, :select_final_grade)`: the
/// user holds an account membership (`account_users`) on the course's account
/// chain. Canvas's default AccountAdmin role carries `select_final_grade`
/// (`config/initializers/permissions_registry.rb`); the snapshot has no
/// account-role override layer yet, so membership alone grants the right.
async fn account_membership_allows(
    pool: &PgPool,
    course_id: i64,
    user_id: i64,
) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE account_chain AS (
            SELECT id, parent_account_id, 0 AS depth
            FROM accounts
            WHERE id = (SELECT account_id FROM courses WHERE id = $2)
              AND workflow_state <> 'deleted'
            UNION ALL
            SELECT parent.id, parent.parent_account_id, chain.depth + 1
            FROM accounts parent
            JOIN account_chain chain ON parent.id = chain.parent_account_id
            WHERE parent.workflow_state <> 'deleted'
              AND chain.depth < 10
        )
        SELECT 1
        FROM account_users au
        JOIN account_chain chain ON chain.id = au.account_id
        WHERE au.user_id = $1
        LIMIT 1
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Canvas `AbstractAssignment#permits_moderation?(user)`: the final grader or
/// an account admin with `:select_final_grade`.
async fn permits_moderation(
    pool: &PgPool,
    assignment: &ModeratedAssignment,
    user_id: i64,
) -> Result<bool, ApiError> {
    if assignment.final_grader_id == Some(user_id) {
        return Ok(true);
    }
    account_membership_allows(pool, assignment.course_id, user_id).await
}

/// Canvas `AbstractAssignment#can_view_other_grader_identities?(user)`.
async fn can_view_other_grader_identities(
    pool: &PgPool,
    assignment: &ModeratedAssignment,
    user_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if !check_teacher_or_admin(pool, user_id, assignment.course_id, auth).await? {
        return Ok(false);
    }
    if !(assignment.moderated_grading && !assignment.grades_published()) {
        return Ok(true);
    }
    if assignment.final_grader_id == Some(user_id) {
        return Ok(assignment.grader_names_visible_to_final_grader);
    }
    if account_membership_allows(pool, assignment.course_id, user_id).await? {
        return Ok(true);
    }
    if !assignment.grader_comments_visible_to_graders {
        return Ok(false);
    }
    Ok(!assignment.graders_anonymous_to_graders)
}

/// Canvas `AbstractAssignment#moderated_grader_limit_reached?`: the count of
/// slot-holding provisional graders (excluding the final grader) has reached
/// `grader_count`.
async fn moderated_grader_limit_reached(
    pool: &PgPool,
    assignment: &ModeratedAssignment,
) -> Result<bool, ApiError> {
    if !assignment.moderated_grading {
        return Ok(false);
    }
    let taken: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM moderation_graders
        WHERE assignment_id = $1 AND slot_taken
          AND ($2::bigint IS NULL OR user_id <> $2)
        "#,
    )
    .bind(assignment.id)
    .bind(assignment.final_grader_id)
    .fetch_one(pool)
    .await?;
    Ok(taken >= i64::from(assignment.grader_count))
}

/// Canvas `AbstractAssignment#anonymize_students?` with the moderated branch:
/// false unless `anonymous_grading`; for moderated assignments only while
/// grades are unpublished; otherwise while any active student has an
/// unposted submission.
async fn assignment_anonymize_students_moderated(
    pool: &PgPool,
    assignment: &ModeratedAssignment,
) -> Result<bool, ApiError> {
    if !assignment.anonymous_grading {
        return Ok(false);
    }
    if assignment.moderated_grading {
        return Ok(!assignment.grades_published());
    }
    // unposted_anonymous_submissions? — same proxy the pre-moderation helper
    // uses: the snapshot has no posted_at, so `graded_at IS NULL` marks the
    // submissions that still need posting.
    let unposted: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM submissions s
        JOIN enrollments e ON e.user_id = s.user_id
        WHERE s.assignment_id = $1 AND s.graded_at IS NULL
          AND e.course_id = $2 AND e.role = 'student'
          AND e.workflow_state NOT IN ('rejected', 'deleted')
        LIMIT 1
        "#,
    )
    .bind(assignment.id)
    .bind(assignment.course_id)
    .fetch_optional(pool)
    .await?;
    Ok(unposted.is_some())
}

/// Canvas `AbstractAssignment#can_view_student_names?(user)`: false when
/// students are anonymized, otherwise course grading rights.
async fn can_view_student_names(
    pool: &PgPool,
    assignment: &ModeratedAssignment,
    user_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if assignment_anonymize_students_moderated(pool, assignment).await? {
        return Ok(false);
    }
    check_teacher_or_admin(pool, user_id, assignment.course_id, auth).await
}

/// Canvas `AbstractAssignment#can_be_moderated_grader?(user)`: a grader with
/// course grading rights, unless the grader limit is reached — then only the
/// final grader or an existing moderation grader.
async fn can_be_moderated_grader(
    pool: &PgPool,
    assignment: &ModeratedAssignment,
    user_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if !check_teacher_or_admin(pool, user_id, assignment.course_id, auth).await? {
        return Ok(false);
    }
    if !moderated_grader_limit_reached(pool, assignment).await? {
        return Ok(true);
    }
    if assignment.final_grader_id == Some(user_id) {
        return Ok(true);
    }
    let slot: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM moderation_graders WHERE assignment_id = $1 AND user_id = $2 AND slot_taken"#,
    )
    .bind(assignment.id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(slot.is_some())
}

/// Canvas `assignment.grader_ids_to_anonymous_ids` (moderation_graders map).
async fn grader_ids_to_anonymous_ids(
    pool: &PgPool,
    assignment_id: i64,
) -> Result<HashMap<i64, String>, ApiError> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT user_id, anonymous_id FROM moderation_graders WHERE assignment_id = $1",
    )
    .bind(assignment_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// One `provisional_grades` row (Canvas `ModeratedGrading::ProvisionalGrade`).
#[derive(Debug, Clone, sqlx::FromRow)]
struct ProvisionalGradeRow {
    id: i64,
    submission_id: i64,
    scorer_id: i64,
    score: Option<f64>,
    grade: Option<String>,
    graded_at: Option<DateTime<Utc>>,
    #[sqlx(rename = "is_final")]
    final_grade: bool,
    graded_anonymously: Option<bool>,
    created_at: DateTime<Utc>,
}

/// Submission facts needed to serialize a provisional grade.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ProvisionalSubmissionRef {
    id: i64,
    user_id: i64,
    anonymous_id: Option<String>,
    submitted_at: Option<DateTime<Utc>>,
}

/// Canvas `value_to_boolean` for JSON payload values: booleans pass through,
/// strings/numbers use Canvas's truthy spellings, anything else is false.
fn json_value_to_boolean(value: Option<&serde_json::Value>) -> bool {
    match value {
        Some(serde_json::Value::Bool(flag)) => *flag,
        Some(serde_json::Value::String(text)) => matches!(
            text.trim().to_ascii_lowercase().as_str(),
            "true" | "t" | "1" | "yes" | "y" | "on"
        ),
        Some(serde_json::Value::Number(number)) => {
            number.as_f64().map(|value| value != 0.0).unwrap_or(false)
        }
        _ => false,
    }
}

/// Canvas `lib/api/v1/submission.rb#provisional_grade_json`:
/// `grade_attributes` (`grade`, `score`, `graded_at`, `scorer_id`, `final`,
/// `graded_anonymously`, `provisional_grade_id`,
/// `grade_matches_current_submission`, `entered_score`, `entered_grade`) plus
/// `speedgrader_url`. When the caller cannot see grader identities,
/// `scorer_id` is replaced by `anonymous_grader_id`.
fn provisional_grade_json(
    course_id: i64,
    assignment_id: i64,
    submission: &ProvisionalSubmissionRef,
    pg: &ProvisionalGradeRow,
    can_view_names: bool,
    can_view_grader_identities: bool,
    grader_anonymous_ids: &HashMap<i64, String>,
) -> serde_json::Value {
    let grade_matches_current_submission = submission.submitted_at.is_none()
        || pg.graded_at.is_none()
        || submission.submitted_at <= pg.graded_at;

    let speedgrader_url = if can_view_names {
        format!(
            "/courses/{course_id}/gradebook/speed_grader?assignment_id={assignment_id}&student_id={}",
            submission.user_id
        )
    } else {
        format!(
            "/courses/{course_id}/gradebook/speed_grader?assignment_id={assignment_id}&anonymous_id={}",
            submission.anonymous_id.as_deref().unwrap_or("")
        )
    };

    let mut json = serde_json::json!({
        "grade": pg.grade,
        "score": pg.score,
        "graded_at": pg.graded_at,
        "final": pg.final_grade,
        "graded_anonymously": pg.graded_anonymously,
        "provisional_grade_id": pg.id,
        "grade_matches_current_submission": grade_matches_current_submission,
        "entered_score": pg.score,
        "entered_grade": pg.grade,
        "speedgrader_url": speedgrader_url,
    });

    if can_view_grader_identities {
        json["scorer_id"] = serde_json::json!(pg.scorer_id);
    } else {
        json["anonymous_grader_id"] = serde_json::json!(grader_anonymous_ids.get(&pg.scorer_id));
    }

    json
}

/// Canvas `submission_provisional_grades_json` (lib/api/v1/submission.rb):
/// moderators see every provisional grade (non-final by `created_at`, final
/// last); other graders only see their own.
async fn submission_provisional_grades_json(
    pool: &PgPool,
    course_id: i64,
    assignment: &ModeratedAssignment,
    submission: &ProvisionalSubmissionRef,
    current_user_id: i64,
    auth: &AuthUser,
) -> Result<Vec<serde_json::Value>, ApiError> {
    let mut rows: Vec<ProvisionalGradeRow> = sqlx::query_as(
        r#"
        SELECT id, submission_id, scorer_id, score::float8, grade, graded_at,
               final AS is_final, graded_anonymously, created_at
        FROM provisional_grades WHERE submission_id = $1
        "#,
    )
    .bind(submission.id)
    .fetch_all(pool)
    .await?;

    let is_moderator = permits_moderation(pool, assignment, current_user_id).await?;
    if is_moderator {
        // sort_by { |pg| pg.final ? CanvasSort::Last : pg.created_at }:
        // created_at ascending with the final grade moved to the end.
        rows.sort_by(|a, b| match (a.final_grade, b.final_grade) {
            (true, false) => std::cmp::Ordering::Greater,
            (false, true) => std::cmp::Ordering::Less,
            _ => a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)),
        });
    } else {
        rows.retain(|pg| pg.scorer_id == current_user_id);
    }

    let can_view_names = can_view_student_names(pool, assignment, current_user_id, auth).await?;
    let can_view_identities =
        can_view_other_grader_identities(pool, assignment, current_user_id, auth).await?;
    let anonymous_ids = if can_view_identities {
        HashMap::new()
    } else {
        grader_ids_to_anonymous_ids(pool, assignment.id).await?
    };

    let list = rows
        .iter()
        .map(|pg| {
            provisional_grade_json(
                course_id,
                assignment.id,
                submission,
                pg,
                can_view_names,
                can_view_identities,
                &anonymous_ids,
            )
        })
        .collect();
    Ok(list)
}

/// The students visible to a course admin, including inactive ones (Canvas
/// `students_visible_to(user, include: :inactive)` over
/// `all_accepted_students`: student enrollments not rejected/deleted).
async fn visible_student_ids(pool: &PgPool, course_id: i64) -> Result<Vec<i64>, ApiError> {
    let ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT e.user_id FROM enrollments e
        WHERE e.course_id = $1 AND e.role = 'student'
          AND e.workflow_state NOT IN ('rejected', 'deleted')
        "#,
    )
    .bind(course_id)
    .fetch_all(pool)
    .await?;
    Ok(ids)
}

/// Attach `provisional_grades` to a serialized submission when the
/// assignment is a moderated one present in the include map (Canvas
/// `submission_json`: the include is only honored for moderated
/// assignments).
async fn append_provisional_grades_include(
    pool: &PgPool,
    moderated_includes: &HashMap<i64, ModeratedAssignment>,
    submission_id: Option<i64>,
    course_id: i64,
    current_user_id: i64,
    auth: &AuthUser,
    value: &mut serde_json::Value,
) -> Result<(), ApiError> {
    let Some(submission_id) = submission_id else {
        return Ok(());
    };
    if moderated_includes.is_empty() {
        return Ok(());
    }
    let submission_ref: Option<ProvisionalSubmissionRef> = sqlx::query_as(
        r#"SELECT id, user_id, anonymous_id, submitted_at FROM submissions WHERE id = $1"#,
    )
    .bind(submission_id)
    .fetch_optional(pool)
    .await?;
    let Some(submission_ref) = submission_ref else {
        return Ok(());
    };
    // The submission's assignment must be one of the moderated ones.
    let assignment_id: Option<i64> =
        sqlx::query_scalar("SELECT assignment_id FROM submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(pool)
            .await?;
    let Some(assignment) = assignment_id.and_then(|id| moderated_includes.get(&id)) else {
        return Ok(());
    };
    value["provisional_grades"] = serde_json::json!(
        submission_provisional_grades_json(
            pool,
            course_id,
            assignment,
            &submission_ref,
            current_user_id,
            auth,
        )
        .await?
    );
    Ok(())
}

/// Parse `student_ids[]` from a JSON or form-encoded moderation-set body
/// (Canvas accepts both; same approach as `parse_submission_id_list`).
fn parse_moderated_student_ids(body: &[u8]) -> Result<Vec<i64>, ApiError> {
    if body.is_empty() {
        return Ok(Vec::new());
    }

    if let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) {
        let Some(object) = json.as_object() else {
            return Ok(Vec::new());
        };
        if let Some(value) = object.get("student_ids") {
            let items: Vec<&serde_json::Value> = match value {
                serde_json::Value::Array(items) => items.iter().collect(),
                other => vec![other],
            };
            return Ok(items.into_iter().filter_map(json_id).collect());
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
        if key != "student_ids" {
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

/// GET /courses/{courseId}/assignments/{assignmentId}/moderated_students —
/// Canvas `ModerationSetController#index`: the students selected for
/// moderation, ordered by id.
async fn moderation_set_index(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let assignment = load_moderated_assignment(&state.db, course_id, assignment_id).await?;
    if !permits_moderation(&state.db, &assignment, auth.id).await? {
        return Err(ApiError::Unauthorized);
    }

    let rows: Vec<(i64, String, Option<String>, String, String, DateTime<Utc>)> = sqlx::query_as(
        r#"
            SELECT u.id, u.name, u.sortable_name, u.email, u.role, u.created_at
            FROM users u
            WHERE u.id IN (
                SELECT student_id FROM moderated_grading_selections WHERE assignment_id = $1
            )
            ORDER BY u.id
            "#,
    )
    .bind(assignment_id)
    .fetch_all(&state.db)
    .await?;

    let users = rows
        .into_iter()
        .map(|(id, name, sortable_name, email, role, created_at)| {
            serde_json::json!({
                "id": id,
                "name": name,
                "sortable_name": sortable_name.unwrap_or(name),
                "email": email,
                "role": role,
                "created_at": created_at,
            })
        })
        .collect();
    Ok(Json(users))
}

/// POST /courses/{courseId}/assignments/{assignmentId}/moderated_students —
/// Canvas `ModerationSetController#create`: add students to the moderation
/// set incrementally; the response lists the requested students that are
/// visible to the caller.
async fn moderation_set_create(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let assignment = load_moderated_assignment(&state.db, course_id, assignment_id).await?;
    if !permits_moderation(&state.db, &assignment, auth.id).await? {
        return Err(ApiError::Unauthorized);
    }

    let student_ids = parse_moderated_student_ids(&body)?;
    if student_ids.is_empty() {
        // Canvas: render json: [], status: :bad_request
        return Ok((StatusCode::BAD_REQUEST, Json(Vec::<i64>::new())).into_response());
    }

    // The visible subset of the requested students (response + creation set).
    let visible: Vec<i64> = visible_student_ids(&state.db, course_id).await?;
    let visible_requested: Vec<i64> = student_ids
        .iter()
        .copied()
        .filter(|id| visible.contains(id))
        .collect();

    // incremental_create: insert selections only for students not yet in the
    // set (existing selections are kept — `ON CONFLICT DO NOTHING`).
    sqlx::query(
        r#"
        INSERT INTO moderated_grading_selections (assignment_id, student_id)
        SELECT $1, unnest($2::bigint[])
        ON CONFLICT (assignment_id, student_id) DO NOTHING
        "#,
    )
    .bind(assignment_id)
    .bind(&visible_requested)
    .execute(&state.db)
    .await?;

    let rows: Vec<(i64, String, Option<String>, String, String, DateTime<Utc>)> = sqlx::query_as(
        r#"
            SELECT u.id, u.name, u.sortable_name, u.email, u.role, u.created_at
            FROM users u
            WHERE u.id = ANY($1)
            ORDER BY u.id
            "#,
    )
    .bind(&visible_requested)
    .fetch_all(&state.db)
    .await?;

    let users: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, name, sortable_name, email, role, created_at)| {
            serde_json::json!({
                "id": id,
                "name": name,
                "sortable_name": sortable_name.unwrap_or(name),
                "email": email,
                "role": role,
                "created_at": created_at,
            })
        })
        .collect();
    Ok(Json(users).into_response())
}

/// Canvas renders specific JSON bodies with non-200 statuses on the
/// moderation routes; `ApiError` stays plain text for generic failures.
fn canvas_message(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "message": message }))).into_response()
}

/// Canvas `ModeratedGrading::Selection#as_json` shape (plus the
/// `anonymous_id` swap when student names are hidden): the response of the
/// `select`/`bulk_select` routes.
fn selection_json(
    assignment_id: i64,
    student_id: i64,
    selected_provisional_grade_id: Option<i64>,
    can_view_names: bool,
    submission_anonymous_id: Option<&str>,
) -> serde_json::Value {
    if can_view_names {
        serde_json::json!({
            "assignment_id": assignment_id,
            "student_id": student_id,
            "selected_provisional_grade_id": selected_provisional_grade_id,
        })
    } else {
        serde_json::json!({
            "assignment_id": assignment_id,
            "anonymous_id": submission_anonymous_id,
            "selected_provisional_grade_id": selected_provisional_grade_id,
        })
    }
}

/// Record a moderation event (Canvas `ModeratedGrading::Selection
/// #create_moderation_event` → `AnonymousOrModerationEvent`).
async fn create_moderation_event(
    pool: &PgPool,
    assignment_id: i64,
    user_id: i64,
    submission_id: Option<i64>,
    selected_provisional_grade_id: Option<i64>,
    student_id: i64,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO anonymous_or_moderation_events
            (assignment_id, user_id, submission_id, event_type, payload)
        VALUES ($1, $2, $3, 'provisional_grade_selected',
                jsonb_build_object('id', $4, 'student_id', $5))
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(submission_id)
    .bind(selected_provisional_grade_id)
    .bind(student_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a provisional-grade audit event (Canvas
/// `ModeratedGrading::ProvisionalGrade` create/update audit events).
async fn create_provisional_grade_event(
    pool: &PgPool,
    assignment_id: i64,
    user_id: i64,
    submission_id: i64,
    event_type: &str,
    payload: serde_json::Value,
) -> Result<(), ApiError> {
    sqlx::query(
        r#"
        INSERT INTO anonymous_or_moderation_events
            (assignment_id, user_id, submission_id, event_type, payload)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(submission_id)
    .bind(event_type)
    .bind(payload)
    .execute(pool)
    .await?;
    Ok(())
}

/// The provisional-grade audit payload (Canvas AUDITABLE_ATTRIBUTES plus the
/// row id).
fn provisional_grade_event_payload(pg: &ProvisionalGradeRow) -> serde_json::Value {
    serde_json::json!({
        "id": pg.id,
        "score": pg.score,
        "grade": pg.grade,
        "graded_at": pg.graded_at,
        "final": pg.final_grade,
        "graded_anonymously": pg.graded_anonymously,
        "scorer_id": pg.scorer_id,
    })
}

/// Select a provisional grade's row, restricted to one assignment (Canvas
/// `@assignment.provisional_grades.find` → 404 on a foreign id).
async fn find_provisional_grade(
    pool: &PgPool,
    assignment_id: i64,
    provisional_grade_id: i64,
) -> Result<Option<ProvisionalGradeRow>, ApiError> {
    let pg: Option<ProvisionalGradeRow> = sqlx::query_as(
        r#"
        SELECT pg.id, pg.submission_id, pg.scorer_id, pg.score::float8, pg.grade,
               pg.graded_at, pg.final AS is_final, pg.graded_anonymously, pg.created_at
        FROM provisional_grades pg
        JOIN submissions s ON s.id = pg.submission_id
        WHERE pg.id = $1 AND s.assignment_id = $2
        "#,
    )
    .bind(provisional_grade_id)
    .bind(assignment_id)
    .fetch_optional(pool)
    .await?;
    Ok(pg)
}

/// PUT /courses/{courseId}/assignments/{assignmentId}/provisional_grades/
/// {provisionalGradeId}/select — Canvas `ProvisionalGradesController#select`.
async fn provisional_grades_select(
    State(state): State<AppState>,
    Path((course_id, assignment_id, provisional_grade_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let assignment = load_moderated_assignment(&state.db, course_id, assignment_id).await?;
    if !permits_moderation(&state.db, &assignment, auth.id).await? {
        return Err(ApiError::Unauthorized);
    }

    let Some(pg) = find_provisional_grade(&state.db, assignment_id, provisional_grade_id).await?
    else {
        return Err(ApiError::NotFound);
    };

    let submission: ProvisionalSubmissionRef = sqlx::query_as(
        r#"SELECT id, user_id, anonymous_id, submitted_at FROM submissions WHERE id = $1"#,
    )
    .bind(pg.submission_id)
    .fetch_one(&state.db)
    .await?;

    let selection: Option<(i64, Option<i64>)> = sqlx::query_as(
        r#"
        SELECT id, selected_provisional_grade_id
        FROM moderated_grading_selections
        WHERE assignment_id = $1 AND student_id = $2
        "#,
    )
    .bind(assignment_id)
    .bind(submission.user_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((selection_id, _)) = selection else {
        return Ok(canvas_message(
            StatusCode::BAD_REQUEST,
            "student not in moderation set",
        ));
    };

    sqlx::query(
        r#"
        UPDATE moderated_grading_selections
        SET selected_provisional_grade_id = $2, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(selection_id)
    .bind(provisional_grade_id)
    .execute(&state.db)
    .await?;

    create_moderation_event(
        &state.db,
        assignment_id,
        auth.id,
        Some(pg.submission_id),
        Some(provisional_grade_id),
        submission.user_id,
    )
    .await?;

    // Canvas touches the submission so SpeedGrader reloads the grades.
    sqlx::query("UPDATE submissions SET updated_at = now() WHERE id = $1")
        .bind(pg.submission_id)
        .execute(&state.db)
        .await?;

    let view_names = can_view_student_names(&state.db, &assignment, auth.id, &auth).await?;
    let json = selection_json(
        assignment_id,
        submission.user_id,
        Some(provisional_grade_id),
        view_names,
        submission.anonymous_id.as_deref(),
    );
    Ok(Json(json).into_response())
}

/// PUT /courses/{courseId}/assignments/{assignmentId}/provisional_grades/
/// bulk_select — Canvas `ProvisionalGradesController#bulk_select`: select a
/// provisional grade for several students at once; only changed selections
/// are returned (and generate moderation events).
async fn provisional_grades_bulk_select(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let assignment = load_moderated_assignment(&state.db, course_id, assignment_id).await?;
    if !permits_moderation(&state.db, &assignment, auth.id).await? {
        return Err(ApiError::Unauthorized);
    }

    let requested_ids = parse_moderated_student_ids(&body)?;
    // Canvas: @assignment.provisional_grades.where(id: ids) — ids from other
    // assignments are simply absent from the map.
    let mut grades: Vec<ProvisionalGradeRow> = Vec::new();
    for requested in &requested_ids {
        if let Some(pg) = find_provisional_grade(&state.db, assignment_id, *requested).await? {
            grades.push(pg);
        }
    }

    let submission_ids: Vec<i64> = grades.iter().map(|pg| pg.submission_id).collect();
    let submissions: Vec<ProvisionalSubmissionRef> = sqlx::query_as(
        r#"SELECT id, user_id, anonymous_id, submitted_at FROM submissions WHERE id = ANY($1)"#,
    )
    .bind(&submission_ids)
    .fetch_all(&state.db)
    .await?;
    let submissions_by_id: HashMap<i64, ProvisionalSubmissionRef> =
        submissions.into_iter().map(|s| (s.id, s)).collect();

    // Canvas builds {student_id => {...}} keyed by first appearance in the
    // request order (later ids for the same student win); the response keeps
    // that order.
    let mut student_order: Vec<i64> = Vec::new();
    let mut chosen: HashMap<i64, (i64, i64)> = HashMap::new(); // student -> (pg id, submission id)
    for requested in &requested_ids {
        let Some(pg) = grades.iter().find(|pg| pg.id == *requested) else {
            continue;
        };
        let Some(submission) = submissions_by_id.get(&pg.submission_id) else {
            continue;
        };
        if !chosen.contains_key(&submission.user_id) {
            student_order.push(submission.user_id);
        }
        chosen.insert(submission.user_id, (pg.id, pg.submission_id));
    }

    let selections: Vec<(i64, i64, Option<i64>)> = sqlx::query_as(
        r#"
        SELECT id, student_id, selected_provisional_grade_id
        FROM moderated_grading_selections
        WHERE assignment_id = $1 AND student_id = ANY($2)
        "#,
    )
    .bind(assignment_id)
    .bind(&student_order)
    .fetch_all(&state.db)
    .await?;
    let selections_by_student: HashMap<i64, (i64, Option<i64>)> = selections
        .into_iter()
        .map(|(id, student_id, selected)| (student_id, (id, selected)))
        .collect();

    // Canvas dereferences the selection inside the transaction; a student
    // without a selection row aborts the whole request (the transaction
    // rolls back and Rails answers 500).
    if let Some(student_id) = student_order
        .iter()
        .find(|student| !selections_by_student.contains_key(student))
    {
        return Err(ApiError::Db(format!(
            "student {student_id} not in moderation set"
        )));
    }

    let view_names = can_view_student_names(&state.db, &assignment, auth.id, &auth).await?;

    let mut json: Vec<serde_json::Value> = Vec::new();
    let mut changed_submission_ids: Vec<i64> = Vec::new();
    for student_id in student_order {
        let (provisional_grade_id, submission_id) = chosen[&student_id];
        let (selection_id, previous) = selections_by_student[&student_id];
        // Only changed selections are saved, returned, and generate events.
        if previous == Some(provisional_grade_id) {
            continue;
        }

        sqlx::query(
            r#"
            UPDATE moderated_grading_selections
            SET selected_provisional_grade_id = $2, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(selection_id)
        .bind(provisional_grade_id)
        .execute(&state.db)
        .await?;

        create_moderation_event(
            &state.db,
            assignment_id,
            auth.id,
            Some(submission_id),
            Some(provisional_grade_id),
            student_id,
        )
        .await?;

        changed_submission_ids.push(submission_id);
        let submission = submissions_by_id[&submission_id].clone();
        json.push(selection_json(
            assignment_id,
            student_id,
            Some(provisional_grade_id),
            view_names,
            submission.anonymous_id.as_deref(),
        ));
    }

    // Canvas: Submission.where(id: changed).touch_all (SpeedGrader reload).
    sqlx::query("UPDATE submissions SET updated_at = now() WHERE id = ANY($1)")
        .bind(&changed_submission_ids)
        .execute(&state.db)
        .await?;

    Ok(Json(json).into_response())
}

/// How a status request addresses the student.
enum StudentRef {
    UserId(i64),
    AnonymousId(String),
}

/// Extract a single query-string value (first occurrence wins).
fn parse_query_value(raw_query: Option<&str>, key: &str) -> Option<String> {
    for pair in raw_query.unwrap_or_default().split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let decoded_key = decode_query_component(raw_key);
        let decoded_key = decoded_key.strip_suffix("[]").unwrap_or(&decoded_key);
        if decoded_key == key {
            return Some(decode_query_component(raw_value));
        }
    }
    None
}

/// GET .../provisional_grades/status and .../anonymous_provisional_grades/
/// status — Canvas `ProvisionalGradesBaseController#status` (the two
/// controllers differ only in how the submission is located).
async fn provisional_grades_status_common(
    state: AppState,
    course_id: i64,
    assignment_id: i64,
    student_ref: StudentRef,
    last_updated_at: Option<String>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let assignment = load_moderated_assignment(&state.db, course_id, assignment_id).await?;

    // Canvas resolves the student's submission first (`find_by!` → 404),
    // before the course permission check.
    let submission: Option<ProvisionalSubmissionRef> = match &student_ref {
        StudentRef::UserId(user_id) => {
            sqlx::query_as(
                r#"SELECT id, user_id, anonymous_id, submitted_at FROM submissions
                   WHERE assignment_id = $1 AND user_id = $2"#,
            )
            .bind(assignment_id)
            .bind(user_id)
            .fetch_optional(&state.db)
            .await?
        }
        StudentRef::AnonymousId(anonymous_id) => {
            sqlx::query_as(
                r#"SELECT id, user_id, anonymous_id, submitted_at FROM submissions
                   WHERE assignment_id = $1 AND anonymous_id = $2"#,
            )
            .bind(assignment_id)
            .bind(anonymous_id)
            .fetch_optional(&state.db)
            .await?
        }
    };
    let Some(submission) = submission else {
        return Err(ApiError::NotFound);
    };

    // authorized_action(@context, [:manage_grades, :view_all_grades])
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Unauthorized);
    }

    if !assignment.moderated_grading {
        return Ok(canvas_message(
            StatusCode::BAD_REQUEST,
            "Assignment does not use moderated grading",
        ));
    }
    if assignment.grades_published() {
        return Ok(canvas_message(
            StatusCode::BAD_REQUEST,
            "Assignment grades have already been published",
        ));
    }

    let mut json = serde_json::json!({
        "needs_provisional_grade":
            can_be_moderated_grader(&state.db, &assignment, auth.id, &auth).await?,
    });

    let Some(last_updated_raw) = last_updated_at else {
        return Ok(Json(json).into_response());
    };

    // Canvas: submission&.updated_at.to_i == Time.zone.parse(...).to_i —
    // second precision; an unparseable timestamp never matches.
    let last_updated_epoch = chrono::DateTime::parse_from_rfc3339(&last_updated_raw)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc).timestamp());
    let submission_updated: Option<chrono::DateTime<Utc>> =
        sqlx::query_scalar("SELECT updated_at FROM submissions WHERE id = $1")
            .bind(submission.id)
            .fetch_one(&state.db)
            .await?;
    if submission_updated.map(|ts| ts.timestamp()) == last_updated_epoch {
        return Ok(Json(json).into_response());
    }
    if !permits_moderation(&state.db, &assignment, auth.id).await? {
        return Err(ApiError::Unauthorized);
    }

    let selection: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT selected_provisional_grade_id
        FROM moderated_grading_selections
        WHERE assignment_id = $1 AND student_id = $2
        "#,
    )
    .bind(assignment_id)
    .bind(submission.user_id)
    .fetch_optional(&state.db)
    .await?
    .flatten();

    let include_scorer_names =
        can_view_other_grader_identities(&state.db, &assignment, auth.id, &auth).await?;
    let view_names = can_view_student_names(&state.db, &assignment, auth.id, &auth).await?;
    let anonymous_ids = if include_scorer_names {
        HashMap::new()
    } else {
        grader_ids_to_anonymous_ids(&state.db, assignment_id).await?
    };

    // Canvas orders by id and splits the final grade into its own key.
    let rows: Vec<ProvisionalGradeRow> = sqlx::query_as(
        r#"
        SELECT id, submission_id, scorer_id, score::float8, grade, graded_at,
               final AS is_final, graded_anonymously, created_at
        FROM provisional_grades WHERE submission_id = $1 ORDER BY id
        "#,
    )
    .bind(submission.id)
    .fetch_all(&state.db)
    .await?;

    let mut provisional_grades: Vec<serde_json::Value> = Vec::new();
    let mut final_provisional_grade: Option<serde_json::Value> = None;
    for pg in &rows {
        let mut pg_json = provisional_grade_json(
            course_id,
            assignment_id,
            &submission,
            pg,
            view_names,
            include_scorer_names,
            &anonymous_ids,
        );
        pg_json["selected"] = serde_json::json!(selection == Some(pg.id));
        pg_json["readonly"] = serde_json::json!(!pg.final_grade && pg.scorer_id != auth.id);
        if include_scorer_names {
            let scorer_name: Option<(String,)> =
                sqlx::query_as("SELECT name FROM users WHERE id = $1")
                    .bind(pg.scorer_id)
                    .fetch_optional(&state.db)
                    .await?;
            if let Some((name,)) = scorer_name {
                pg_json["scorer_name"] = serde_json::json!(name);
            }
        }
        if pg.final_grade {
            final_provisional_grade = Some(pg_json);
        } else {
            provisional_grades.push(pg_json);
        }
    }
    json["provisional_grades"] = serde_json::json!(provisional_grades);
    if let Some(final_grade) = final_provisional_grade {
        json["final_provisional_grade"] = final_grade;
    }

    Ok(Json(json).into_response())
}

/// GET .../provisional_grades/status — Canvas
/// `ProvisionalGradesController#status`.
async fn provisional_grades_status(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    // Canvas find_by!(user_id: params.fetch(:student_id)): a missing or
    // unknown student is a 404.
    let student_id =
        parse_query_value(raw_query.as_deref(), "student_id").and_then(|value| value.parse().ok());
    let Some(student_id) = student_id else {
        return Err(ApiError::NotFound);
    };
    provisional_grades_status_common(
        state,
        course_id,
        assignment_id,
        StudentRef::UserId(student_id),
        parse_query_value(raw_query.as_deref(), "last_updated_at"),
        auth,
    )
    .await
}

/// GET .../anonymous_provisional_grades/status — Canvas
/// `AnonymousProvisionalGradesController#status`.
async fn anonymous_provisional_grades_status(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let Some(anonymous_id) = parse_query_value(raw_query.as_deref(), "anonymous_id") else {
        return Err(ApiError::NotFound);
    };
    provisional_grades_status_common(
        state,
        course_id,
        assignment_id,
        StudentRef::AnonymousId(anonymous_id),
        parse_query_value(raw_query.as_deref(), "last_updated_at"),
        auth,
    )
    .await
}

/// A provisional grade considered for publishing.
#[derive(Debug, Clone, sqlx::FromRow)]
struct PublishGrade {
    id: i64,
    submission_id: i64,
    scorer_id: i64,
    score: Option<f64>,
    grade: Option<String>,
    graded_at: Option<DateTime<Utc>>,
}

impl PublishGrade {
    /// Canvas `ModeratedGrading::ProvisionalGrade.graded`: graded_at is set.
    fn is_graded(&self) -> bool {
        self.graded_at.is_some()
    }
}

/// POST /courses/{courseId}/assignments/{assignmentId}/provisional_grades/
/// publish — Canvas `ProvisionalGradesController#publish`: publish the
/// selected provisional grade of every submission (irreversible).
async fn provisional_grades_publish(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let assignment = load_moderated_assignment(&state.db, course_id, assignment_id).await?;

    // manage_grades AND permits_moderation (Canvas order).
    let can_manage = check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;
    if !can_manage || !permits_moderation(&state.db, &assignment, auth.id).await? {
        return Err(ApiError::Unauthorized);
    }

    if !assignment.moderated_grading {
        return Ok(canvas_message(
            StatusCode::BAD_REQUEST,
            "Assignment does not use moderated grading",
        ));
    }
    if assignment.grades_published() {
        return Ok(canvas_message(
            StatusCode::BAD_REQUEST,
            "Assignment grades have already been published",
        ));
    }

    let visible = visible_student_ids(&state.db, course_id).await?;

    // Submissions of visible students that have at least one provisional
    // grade; the rest are skipped entirely (Canvas `graded_submissions`).
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        r#"
        SELECT s.id, s.user_id
        FROM submissions s
        WHERE s.assignment_id = $1
          AND s.user_id = ANY($2)
          AND EXISTS (SELECT 1 FROM provisional_grades pg WHERE pg.submission_id = s.id)
        ORDER BY s.id
        "#,
    )
    .bind(assignment_id)
    .bind(&visible)
    .fetch_all(&state.db)
    .await?;

    let mut grades_to_publish: Vec<PublishGrade> = Vec::new();
    for (submission_id, user_id) in &rows {
        let pgs: Vec<PublishGrade> = sqlx::query_as(
            r#"
            SELECT id, submission_id, scorer_id, score::float8, grade, graded_at
            FROM provisional_grades WHERE submission_id = $1 ORDER BY id
            "#,
        )
        .bind(submission_id)
        .fetch_all(&state.db)
        .await?;

        // A selection row pointing at one of this submission's grades wins.
        let selected_id: Option<i64> = sqlx::query_scalar(
            r#"
            SELECT selected_provisional_grade_id
            FROM moderated_grading_selections
            WHERE assignment_id = $1 AND student_id = $2
            "#,
        )
        .bind(assignment_id)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?
        .flatten();

        let mut chosen = selected_id.and_then(|id| pgs.iter().find(|pg| pg.id == id).cloned());

        // Else exactly one graded provisional grade.
        if chosen.is_none() {
            let graded: Vec<&PublishGrade> = pgs.iter().filter(|pg| pg.is_graded()).collect();
            if graded.len() == 1 {
                chosen = Some((*graded[0]).clone());
            }
        }

        // Else the first comment-only grade when no graded ones exist.
        if chosen.is_none() && !pgs.iter().any(|pg| pg.is_graded()) {
            chosen = pgs.iter().find(|pg| !pg.is_graded()).cloned();
        }

        let Some(chosen) = chosen else {
            return Ok(canvas_message(
                StatusCode::UNPROCESSABLE_ENTITY,
                "All submissions must have a selected grade",
            ));
        };
        grades_to_publish.push(chosen);
    }

    // publish! per grade (Canvas skips per-submission score recompute and
    // recomputes the course once; the snapshot computes gradebook grades on
    // read, so nothing stored needs recomputing).
    for grade in &grades_to_publish {
        sqlx::query(
            r#"
            UPDATE submissions
            SET score = $2, graded_at = now(), grader_id = $3,
                workflow_state = CASE WHEN $2::float8 IS NOT NULL
                                      THEN 'graded' ELSE workflow_state END,
                updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(grade.submission_id)
        .bind(grade.score)
        .bind(grade.scorer_id)
        .execute(&state.db)
        .await?;

        // publish_submission_comments!: provisional comments are duplicated
        // as published comments (provisional_grade_id cleared).
        sqlx::query(
            r#"
            INSERT INTO submission_comments
                (submission_id, author_id, comment, workflow_state)
            SELECT submission_id, author_id, comment, workflow_state
            FROM submission_comments
            WHERE provisional_grade_id = $1 AND workflow_state <> 'deleted'
            "#,
        )
        .bind(grade.id)
        .execute(&state.db)
        .await?;
    }

    sqlx::query(
        "UPDATE assignments SET grades_published_at = now(), updated_at = now() WHERE id = $1",
    )
    .bind(assignment_id)
    .execute(&state.db)
    .await?;

    Ok(Json(serde_json::json!({ "message": "OK" })).into_response())
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
            let rows: Vec<(i64, i64, String, DateTime<Utc>)> = sqlx::query_as(
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
            let comments: Vec<serde_json::Value> = rows
                .into_iter()
                .map(|(id, author_id, comment, created_at)| {
                    serde_json::json!({
                        "id": id,
                        "author_id": author_id,
                        "comment": comment,
                        "created_at": created_at,
                    })
                })
                .collect();
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
// Assignment overrides — batch + alias routes (Canvas
// AssignmentOverridesController#batch_retrieve / batch_create / batch_update /
// group_alias / section_alias; issue #55)
// ---------------------------------------------------------------------------

/// Canvas `require_context` + `api_find(Course.active, ...)`: a missing or
/// soft-deleted course is a 404 before any permission check runs.
async fn ensure_course_not_deleted(pool: &PgPool, course_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(pool)
            .await?;

    row.ok_or(ApiError::NotFound)?;
    Ok(())
}

/// Ruby `to_i` semantics for JSON parameter values: numbers pass through,
/// strings contribute their leading digits, anything else is 0.
fn ruby_to_i(value: &serde_json::Value) -> i64 {
    match value {
        serde_json::Value::Number(n) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().map(|f| f as i64).unwrap_or(0)),
        serde_json::Value::String(s) => {
            let mut out = String::new();
            for (i, c) in s.trim().chars().enumerate() {
                let keep = c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'));
                if !keep {
                    break;
                }
                out.push(c);
            }
            out.parse::<i64>().unwrap_or(0)
        }
        _ => 0,
    }
}

/// One `assignment_overrides[]` entry from a batch request (Canvas shape).
///
/// Dates keep the Canvas absent / null / value distinction: `None` = key not
/// sent (clears the override on update), `Some(None)` = explicit null or
/// blank string, `Some(Some(dt))` = parsed timestamp.
#[derive(Debug, Default, Clone)]
struct BatchOverrideEntry {
    assignment_id: Option<i64>,
    id: Option<i64>,
    title: Option<String>,
    student_ids: Option<Vec<i64>>,
    group_id: Option<i64>,
    course_section_id: Option<i64>,
    due_at: Option<Option<DateTime<Utc>>>,
    unlock_at: Option<Option<DateTime<Utc>>>,
    lock_at: Option<Option<DateTime<Utc>>>,
}

/// Parse a Canvas timestamp like `Time.zone.parse`: RFC3339 first, then
/// timezone-less ISO strings interpreted as UTC.
fn parse_canvas_datetime(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        return Some(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, format) {
            return Some(naive.and_utc());
        }
    }
    // Date-only values resolve to midnight UTC (Canvas uses the course zone;
    // star-lms is UTC-only).
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .map(|date| date.and_hms_opt(0, 0, 0).unwrap().and_utc())
}

/// Parse one date field of a batch entry. Returns the Canvas error string on
/// unparseable values (`invalid due_at "foo"` — Ruby `inspect` quoting).
fn parse_batch_date(
    value: &serde_json::Value,
    field: &str,
) -> Result<Option<Option<DateTime<Utc>>>, String> {
    match value {
        serde_json::Value::Null => Ok(Some(None)),
        serde_json::Value::String(s) => {
            if s.trim().is_empty() {
                Ok(Some(None))
            } else {
                parse_canvas_datetime(s)
                    .map(|dt| Some(Some(dt)))
                    .ok_or_else(|| format!("invalid {field} \"{s}\""))
            }
        }
        other => Err(format!("invalid {field} {other}")),
    }
}

/// Interpret one JSON entry into a [`BatchOverrideEntry`], mirroring Canvas
/// `interpret_assignment_override_data`'s structure checks. Target semantics
/// (student/group/section validation) run separately because they need the
/// database.
fn parse_batch_entry(value: &serde_json::Value) -> BatchOverrideEntry {
    let mut entry = BatchOverrideEntry::default();
    let Some(map) = value.as_object() else {
        return entry;
    };
    if let Some(raw) = map.get("assignment_id") {
        entry.assignment_id = Some(ruby_to_i(raw));
    }
    if let Some(raw) = map.get("id") {
        entry.id = Some(ruby_to_i(raw));
    }
    if let Some(raw) = map.get("title") {
        entry.title = raw.as_str().map(str::to_string);
    }
    if let Some(raw) = map.get("student_ids") {
        // JSON null is falsy in Canvas and skips the ADHOC branch entirely.
        if !raw.is_null() {
            let ids = raw
                .as_array()
                .map(|items| items.iter().map(ruby_to_i).collect())
                .unwrap_or_default();
            entry.student_ids = Some(ids);
        }
    }
    if let Some(raw) = map.get("group_id") {
        if !raw.is_null() {
            if let Some(number) = raw.as_i64() {
                entry.group_id = Some(number);
            } else if let Some(text) = raw.as_str() {
                if !text.trim().is_empty() {
                    entry.group_id = Some(ruby_to_i(raw));
                }
            } else {
                entry.group_id = Some(ruby_to_i(raw));
            }
        }
    }
    if let Some(raw) = map.get("course_section_id") {
        if !raw.is_null() {
            if let Some(number) = raw.as_i64() {
                entry.course_section_id = Some(number);
            } else if let Some(text) = raw.as_str() {
                if !text.trim().is_empty() {
                    entry.course_section_id = Some(ruby_to_i(raw));
                }
            } else {
                entry.course_section_id = Some(ruby_to_i(raw));
            }
        }
    }
    for (field, slot) in [
        ("due_at", &mut entry.due_at),
        ("unlock_at", &mut entry.unlock_at),
        ("lock_at", &mut entry.lock_at),
    ] {
        if let Some(raw) = map.get(field) {
            // Structure errors surface during validation; keep parsing lenient
            // here so per-entry errors stay aligned with the request.
            if let Ok(parsed) = parse_batch_date(raw, field) {
                *slot = parsed;
            }
        }
    }
    entry
}

/// Outcome of Canvas `deserialize_overrides` for a JSON request.
enum DeserializedOverrides {
    /// Nothing usable was supplied.
    Blank,
    /// A truthy non-array (scalar / malformed map).
    NotAnArray,
    /// An ordered list of entries.
    Entries(Vec<serde_json::Value>),
}

/// Port of Canvas `deserialize_overrides`: a bare array passes through, a map
/// whose keys are exactly "0".."n-1" becomes an array, anything else is blank.
fn deserialize_overrides(raw: &serde_json::Value) -> DeserializedOverrides {
    match raw {
        serde_json::Value::Null => DeserializedOverrides::Blank,
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                DeserializedOverrides::Blank
            } else {
                DeserializedOverrides::Entries(items.clone())
            }
        }
        serde_json::Value::Object(map) => {
            let numeric: Vec<i64> = map
                .keys()
                .map(|key| key.parse::<i64>().ok())
                .collect::<Option<Vec<i64>>>()
                .unwrap_or_default();
            if numeric.len() == map.len() && !numeric.is_empty() && {
                let mut sorted = numeric.clone();
                sorted.sort_unstable();
                sorted == (0..sorted.len() as i64).collect::<Vec<_>>()
            } {
                let ordered: Vec<serde_json::Value> = (0..numeric.len() as i64)
                    .map(|index| map[&index.to_string()].clone())
                    .collect();
                DeserializedOverrides::Entries(ordered)
            } else {
                DeserializedOverrides::Blank
            }
        }
        _ => DeserializedOverrides::NotAnArray,
    }
}

/// Extract the `assignment_overrides` payload from a JSON request body:
/// either a bare array or `{"assignment_overrides": [...]}`.
fn body_override_payload(payload: &serde_json::Value) -> serde_json::Value {
    match payload {
        serde_json::Value::Array(_) => payload.clone(),
        serde_json::Value::Object(map) => map
            .get("assignment_overrides")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        _ => serde_json::Value::Null,
    }
}

/// Canvas error body for the batch endpoints: `{"errors": [...]}` with 400.
fn batch_error_body(errors: serde_json::Value) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "errors": errors })),
    )
}

/// The target set a batch entry resolves to, once validated against the
/// course (Canvas `interpret_assignment_override_data`).
#[derive(Debug, Clone)]
enum ResolvedBatchTarget {
    Adhoc(Vec<i64>),
    Group(i64),
    Section(i64),
}

/// Validate an ad-hoc `student_ids` list against active student enrollments
/// (Canvas `api_find_all(learning_object.context.all_students, ...)`).
async fn validate_batch_student_ids(
    pool: &PgPool,
    course_id: i64,
    student_ids: &[i64],
) -> Result<Result<Vec<i64>, String>, ApiError> {
    let enrolled: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT DISTINCT user_id FROM enrollments
        WHERE course_id = $1 AND user_id = ANY($2)
          AND role = 'student' AND workflow_state <> 'deleted'
        ORDER BY user_id
        "#,
    )
    .bind(course_id)
    .bind(student_ids)
    .fetch_all(pool)
    .await?;

    // Unmatched ids report as `unknown student ids: [...]` (Ruby inspect
    // quoting); duplicate ids in the request are fine (Canvas stores the
    // distinct student set).
    let bad: Vec<String> = student_ids
        .iter()
        .filter(|id| !enrolled.contains(id))
        .map(|id| id.to_string())
        .collect();
    if !bad.is_empty() {
        let inspected = bad
            .iter()
            .map(|id| format!("\"{id}\""))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(Err(format!("unknown student ids: [{inspected}]")));
    }

    Ok(Ok(enrolled))
}

/// Resolve the create-time target of a batch entry: `student_ids` >
/// `group_id` > `course_section_id`, with Canvas' error strings. Dates are
/// parsed separately and only their parse errors are reported here.
async fn resolve_batch_target(
    pool: &PgPool,
    course_id: i64,
    entry: &BatchOverrideEntry,
) -> Result<Result<Option<ResolvedBatchTarget>, String>, ApiError> {
    if let Some(ids) = &entry.student_ids {
        if ids.is_empty() {
            return Ok(Err("invalid student_ids []".to_string()));
        }
        return match validate_batch_student_ids(pool, course_id, ids).await? {
            Ok(valid) => Ok(Ok(Some(ResolvedBatchTarget::Adhoc(valid)))),
            Err(message) => Ok(Err(message)),
        };
    }

    if let Some(group_id) = entry.group_id {
        let row: Option<String> = sqlx::query_scalar(
            "SELECT name FROM groups WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'",
        )
        .bind(group_id)
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
        if row.is_none() {
            return Ok(Err(format!("unknown group id {group_id}")));
        }
        return Ok(Ok(Some(ResolvedBatchTarget::Group(group_id))));
    }

    if let Some(section_id) = entry.course_section_id {
        let row: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM course_sections WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'",
        )
        .bind(section_id)
        .bind(course_id)
        .fetch_optional(pool)
        .await?;
        if row.is_none() {
            return Ok(Err(format!("unknown section id {section_id}")));
        }
        return Ok(Ok(Some(ResolvedBatchTarget::Section(section_id))));
    }

    Ok(Ok(None))
}

/// Canvas `title_from_students`: "1 student" / "N students".
fn title_from_student_count(count: usize) -> String {
    if count == 1 {
        "1 student".to_string()
    } else {
        format!("{count} students")
    }
}

/// Overrides of an assignment visible to the caller (Canvas
/// `AssignmentOverrideApplicator.overrides_for_assignment_and_user`):
/// teachers/admins see every active override; students only see overrides
/// that target them (ad-hoc list, their groups, their sections).
async fn visible_override_ids(
    pool: &PgPool,
    course_id: i64,
    assignment_id: i64,
    auth: &AuthUser,
) -> Result<HashSet<i64>, ApiError> {
    if check_teacher_or_admin(pool, auth.id, course_id, auth).await? {
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM assignment_overrides \
             WHERE assignment_id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(assignment_id)
        .fetch_all(pool)
        .await?;
        return Ok(ids.into_iter().collect());
    }

    let ids: Vec<i64> = sqlx::query_scalar(
        r#"
        SELECT o.id FROM assignment_overrides o
        WHERE o.assignment_id = $1 AND o.workflow_state <> 'deleted'
          AND (
            $2 = ANY(o.student_ids)
            OR o.group_id IN (
              SELECT gm.group_id FROM group_memberships gm
              JOIN groups g ON g.id = gm.group_id
              WHERE gm.user_id = $2 AND gm.workflow_state = 'accepted'
                AND g.course_id = $3 AND g.workflow_state <> 'deleted'
            )
            OR o.course_section_id IN (
              SELECT e.course_section_id FROM enrollments e
              WHERE e.user_id = $2 AND e.course_id = $3
                AND e.workflow_state <> 'deleted' AND e.course_section_id IS NOT NULL
            )
          )
        "#,
    )
    .bind(assignment_id)
    .bind(auth.id)
    .bind(course_id)
    .fetch_all(pool)
    .await?;

    Ok(ids.into_iter().collect())
}

/// Fetch active overrides by id.
async fn fetch_overrides_by_ids(
    pool: &PgPool,
    ids: &[i64],
) -> Result<HashMap<i64, AssignmentOverride>, ApiError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<AssignmentOverride> = sqlx::query_as(
        r#"
        SELECT id, assignment_id, title, due_at, unlock_at, lock_at,
               student_ids, course_section_id, group_id, workflow_state
        FROM assignment_overrides
        WHERE id = ANY($1) AND workflow_state <> 'deleted'
        "#,
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

/// GET /courses/{courseId}/assignments/overrides — Canvas
/// `batch_retrieve`. Query entries arrive in Rails form encoding
/// (`assignment_overrides[][id]=..&assignment_overrides[][assignment_id]=..`
/// or the indexed `assignment_overrides[0][..]` spelling) and are answered
/// in request order with `null` for unknown assignments, unknown overrides
/// and overrides the caller cannot see.
async fn batch_retrieve_assignment_overrides(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    ensure_course_not_deleted(&state.db, course_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Parse the query string the way Rails builds params for the `[][k]`
    // and indexed `[0][k]` spellings.
    let mut append_form: Vec<Vec<(String, i64)>> = Vec::new();
    let mut indexed_form: HashMap<i64, Vec<(String, i64)>> = HashMap::new();
    let mut hash_form = false;
    let mut scalar_form = false;
    if let Some(query) = raw_query.as_deref() {
        for pair in query.split('&') {
            let Some((raw_key, raw_value)) = pair.split_once('=') else {
                continue;
            };
            let key = decode_query_component(raw_key);
            if key == "assignment_overrides" {
                scalar_form = true;
                continue;
            }
            let Some(rest) = key.strip_prefix("assignment_overrides[") else {
                continue;
            };
            // rest is `<middle>][<inner>]` for the array spellings, or
            // `<key>]` for the hash spelling Rails parses as a plain map.
            let Some(brace) = rest.strip_suffix(']') else {
                continue;
            };
            let Some((middle, inner)) = brace.split_once("][") else {
                hash_form = true;
                continue;
            };
            let value = ruby_to_i(&serde_json::Value::String(decode_query_component(
                raw_value,
            )));
            if middle.is_empty() {
                // `[][k]`: a repeated key starts the next entry (Rails).
                match append_form.last_mut() {
                    Some(current) if !current.iter().any(|(existing, _)| existing == inner) => {
                        current.push((inner.to_string(), value));
                    }
                    _ => append_form.push(vec![(inner.to_string(), value)]),
                }
            } else if !middle.is_empty() && middle.chars().all(|c| c.is_ascii_digit()) {
                match middle.parse::<i64>() {
                    Ok(index) if index >= 0 => {
                        indexed_form
                            .entry(index)
                            .or_default()
                            .push((inner.to_string(), value));
                    }
                    _ => hash_form = true,
                }
            } else {
                hash_form = true;
            }
        }
    }
    if append_form.is_empty() && indexed_form.is_empty() {
        // A hash with non-numeric keys deserializes to nil in Canvas
        // ("no ... values present"); a bare scalar param is truthy but not
        // an array ("must specify an array ...").
        if !hash_form && scalar_form {
            return Ok(batch_error_body(serde_json::json!([
                "must specify an array with entry format { id, assignment_id }"
            ])));
        }
        return Ok(batch_error_body(serde_json::json!([
            "no assignment_overrides values present"
        ])));
    }

    // Indexed maps must cover exactly 0..n-1 (Canvas deserialize_overrides).
    let mut entries: Vec<Vec<(String, i64)>> = append_form;
    if !indexed_form.is_empty() {
        let mut indices: Vec<i64> = indexed_form.keys().copied().collect();
        indices.sort_unstable();
        if indices != (0..indices.len() as i64).collect::<Vec<_>>() {
            return Ok(batch_error_body(serde_json::json!([
                "no assignment_overrides values present"
            ])));
        }
        entries = indices
            .into_iter()
            .map(|index| indexed_form.remove(&index).unwrap_or_default())
            .collect();
    }

    // Every entry needs both keys.
    if !entries
        .iter()
        .all(|entry| entry.iter().any(|(k, _)| k == "id"))
        || !entries
            .iter()
            .all(|entry| entry.iter().any(|(k, _)| k == "assignment_id"))
    {
        return Ok(batch_error_body(serde_json::json!([
            "must specify an array with entry format { id, assignment_id }"
        ])));
    }

    // Active assignments of this course (unknown assignment ids silently
    // drop to nulls, Canvas `next unless assignment`).
    let requested_assignment_ids: Vec<i64> = entries
        .iter()
        .filter_map(|entry| {
            entry
                .iter()
                .find(|(k, _)| k == "assignment_id")
                .map(|(_, v)| *v)
        })
        .collect();
    let known_assignments: HashSet<i64> =
        sqlx::query_scalar("SELECT id FROM assignments WHERE course_id = $1 AND id = ANY($2)")
            .bind(course_id)
            .bind(&requested_assignment_ids)
            .fetch_all(&state.db)
            .await?
            .into_iter()
            .collect();

    // Visible override ids per requested assignment.
    let mut visibility: HashMap<i64, HashSet<i64>> = HashMap::new();
    for assignment_id in &known_assignments {
        let ids = visible_override_ids(&state.db, course_id, *assignment_id, &auth).await?;
        visibility.insert(*assignment_id, ids);
    }

    let requested_override_ids: Vec<i64> = entries
        .iter()
        .filter_map(|entry| entry.iter().find(|(k, _)| k == "id").map(|(_, v)| *v))
        .collect();
    let overrides = fetch_overrides_by_ids(&state.db, &requested_override_ids).await?;

    // Response preserves request order; unknown/invisible entries are null.
    let mut ordered: Vec<Option<&AssignmentOverride>> = Vec::with_capacity(entries.len());
    for entry in &entries {
        let assignment_id = entry
            .iter()
            .find(|(k, _)| k == "assignment_id")
            .map(|(_, v)| *v);
        let override_id = entry.iter().find(|(k, _)| k == "id").map(|(_, v)| *v);
        let found = assignment_id
            .filter(|aid| known_assignments.contains(aid))
            .and_then(|aid| {
                let visible = visibility.get(&aid)?;
                let override_row = overrides.get(&override_id?)?;
                if override_row.assignment_id == aid && visible.contains(&override_row.id) {
                    Some(override_row)
                } else {
                    None
                }
            });
        ordered.push(found);
    }

    let values: Vec<serde_json::Value> = ordered
        .into_iter()
        .map(|row| match row {
            Some(override_row) => serde_json::to_value(override_row).unwrap_or_default(),
            None => serde_json::Value::Null,
        })
        .collect();

    Ok((StatusCode::OK, Json(serde_json::Value::Array(values))))
}

/// Shared validation for `batch_create` / `batch_update` (Canvas
/// `interpret_batch_assignment_overrides_data`). Returns per-entry error
/// strings aligned with the request (`None` marks a valid entry), the parsed
/// entries, the resolved targets (create; or an adhoc update's replacement
/// set), and — for updates — the current override rows.
async fn interpret_batch_entries(
    pool: &PgPool,
    course_id: i64,
    raw_entries: &[serde_json::Value],
    for_update: bool,
) -> Result<
    (
        Vec<Option<String>>,
        Vec<BatchOverrideEntry>,
        Vec<Option<ResolvedBatchTarget>>,
        Vec<Option<AssignmentOverride>>,
    ),
    ApiError,
> {
    let parsed: Vec<BatchOverrideEntry> = raw_entries.iter().map(parse_batch_entry).collect();
    let mut errors: Vec<Option<String>> = vec![None; parsed.len()];
    let mut targets: Vec<Option<ResolvedBatchTarget>> = vec![None; parsed.len()];
    let mut existing: Vec<Option<AssignmentOverride>> = vec![None; parsed.len()];

    // Structure checks first (Canvas `check_property`), so a missing id is
    // reported even when the assignment is also unknown.
    for (index, entry) in parsed.iter().enumerate() {
        if entry.assignment_id.is_none() {
            errors[index] = Some("must specify an assignment id".to_string());
        }
        if for_update {
            if entry.id.is_none() && errors[index].is_none() {
                errors[index] = Some("must specify an override id".to_string());
            }
        } else if entry.id.is_some() && errors[index].is_none() {
            errors[index] = Some("may not specify an override id".to_string());
        }
    }
    // Canvas returns before interpreting anything once a structure error
    // exists (`return nil, all_errors unless all_errors.compact.blank?`).
    if errors.iter().any(Option::is_some) {
        return Ok((errors, parsed, targets, existing));
    }

    // Assignments of this course.
    let assignment_ids: Vec<i64> = parsed.iter().filter_map(|e| e.assignment_id).collect();
    let known_assignments: HashSet<i64> = if assignment_ids.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar("SELECT id FROM assignments WHERE course_id = $1 AND id = ANY($2)")
            .bind(course_id)
            .bind(&assignment_ids)
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect()
    };

    for (index, entry) in parsed.iter().enumerate() {
        if errors[index].is_some() {
            continue;
        }
        let assignment_id = entry.assignment_id.unwrap_or(0);
        if !known_assignments.contains(&assignment_id) {
            errors[index] = Some("assignment not found".to_string());
            continue;
        }

        let mut entry_errors: Vec<String> = Vec::new();

        if for_update {
            // The override must exist, be active and belong to the entry's
            // assignment (teacher callers see every active override).
            let row: Option<AssignmentOverride> = sqlx::query_as(
                r#"
                SELECT id, assignment_id, title, due_at, unlock_at, lock_at,
                       student_ids, course_section_id, group_id, workflow_state
                FROM assignment_overrides
                WHERE id = $1 AND assignment_id = $2 AND workflow_state <> 'deleted'
                "#,
            )
            .bind(entry.id.unwrap_or(0))
            .bind(assignment_id)
            .fetch_optional(pool)
            .await?;
            let Some(row) = row else {
                errors[index] = Some("override not found".to_string());
                continue;
            };
            existing[index] = Some(row);
        }

        // Target resolution. Creates need a target; updates only resolve a
        // target when an adhoc override actually supplies a replacement
        // `student_ids` set (Canvas passes the override's set_type, which
        // skips the target branches — an adhoc update without student_ids
        // keeps its set, and non-adhoc overrides keep theirs).
        let needs_target = if for_update {
            existing[index]
                .as_ref()
                .is_some_and(|row| !row.student_ids.is_empty())
                && entry.student_ids.is_some()
        } else {
            true
        };
        if needs_target {
            match resolve_batch_target(pool, course_id, entry).await? {
                Ok(target) => {
                    if let Some(target) = target {
                        targets[index] = Some(target);
                    } else {
                        entry_errors.push(
                            "one of student_ids, group_id, or course_section_id is required"
                                .to_string(),
                        );
                    }
                }
                Err(message) => entry_errors.push(message),
            }
        }

        // Date parse errors (unparseable values were treated as absent while
        // parsing the entry; re-parse the raw value to surface the error).
        for field in ["due_at", "unlock_at", "lock_at"] {
            if let Some(raw) = raw_entries[index].get(field) {
                if let Err(message) = parse_batch_date(raw, field) {
                    entry_errors.push(message);
                }
            }
        }

        if !entry_errors.is_empty() {
            errors[index] = Some(entry_errors.join(", "));
        }
    }

    Ok((errors, parsed, targets, existing))
}

/// POST /courses/{courseId}/assignments/overrides — Canvas `batch_create`.
/// Transactional: every override is created or none are.
async fn batch_create_assignment_overrides(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    ensure_course_not_deleted(&state.db, course_id).await?;

    // Canvas `require_all_assignments_edit`: course-level
    // manage_assignments_edit.
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::BadRequest("invalid JSON body".into()))?;
    let raw = body_override_payload(&payload);
    let entries = match deserialize_overrides(&raw) {
        DeserializedOverrides::Blank => {
            return Ok(batch_error_body(serde_json::json!([
                "no assignment override data present"
            ])));
        }
        DeserializedOverrides::NotAnArray => {
            return Ok(batch_error_body(serde_json::json!([
                "must specify an array of overrides"
            ])));
        }
        DeserializedOverrides::Entries(entries) => entries,
    };

    let (errors, parsed, targets, _) =
        interpret_batch_entries(&state.db, course_id, &entries, false).await?;
    if errors.iter().any(Option::is_some) {
        let listed: Vec<serde_json::Value> = errors
            .into_iter()
            .map(|error| match error {
                Some(message) => serde_json::json!([message]),
                None => serde_json::Value::Null,
            })
            .collect();
        return Ok(batch_error_body(serde_json::Value::Array(listed)));
    }

    let mut tx = state.db.begin().await?;
    let mut created: Vec<AssignmentOverride> = Vec::with_capacity(parsed.len());
    for (index, entry) in parsed.iter().enumerate() {
        let target = targets[index].clone().expect("validated above");
        // Canvas `set_title_if_needed` / `title_from_students`: adhoc titles
        // default to the student count, group/section titles to the set name.
        let (student_ids, course_section_id, group_id, title) = match &target {
            ResolvedBatchTarget::Adhoc(ids) => (
                ids.clone(),
                None,
                None,
                entry
                    .title
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| title_from_student_count(ids.len())),
            ),
            ResolvedBatchTarget::Group(id) => {
                let name: String = sqlx::query_scalar("SELECT name FROM groups WHERE id = $1")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await?;
                (Vec::new(), None, Some(*id), name)
            }
            ResolvedBatchTarget::Section(id) => {
                let name: String =
                    sqlx::query_scalar("SELECT name FROM course_sections WHERE id = $1")
                        .bind(id)
                        .fetch_one(&mut *tx)
                        .await?;
                (Vec::new(), Some(*id), None, name)
            }
        };

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
        .bind(entry.assignment_id.unwrap_or(0))
        .bind(&title)
        .bind(entry.due_at.flatten())
        .bind(entry.unlock_at.flatten())
        .bind(entry.lock_at.flatten())
        .bind(&student_ids)
        .bind(course_section_id)
        .bind(group_id)
        .fetch_one(&mut *tx)
        .await?;
        created.push(row);
    }
    tx.commit().await?;

    let values: Vec<serde_json::Value> = created
        .iter()
        .map(|row| serde_json::to_value(row).unwrap_or_default())
        .collect();
    Ok((StatusCode::OK, Json(serde_json::Value::Array(values))))
}

/// PUT /courses/{courseId}/assignments/overrides — Canvas `batch_update`.
/// Transactional: every override is updated or none are. Omitted date fields
/// clear the override (Canvas `clear_*_override`); an adhoc override without
/// `student_ids` keeps its target set.
async fn batch_update_assignment_overrides(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    body: Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    ensure_course_not_deleted(&state.db, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let payload: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::BadRequest("invalid JSON body".into()))?;
    let raw = body_override_payload(&payload);
    let entries = match deserialize_overrides(&raw) {
        DeserializedOverrides::Blank => {
            return Ok(batch_error_body(serde_json::json!([
                "no assignment override data present"
            ])));
        }
        DeserializedOverrides::NotAnArray => {
            return Ok(batch_error_body(serde_json::json!([
                "must specify an array of overrides"
            ])));
        }
        DeserializedOverrides::Entries(entries) => entries,
    };

    let (errors, parsed, targets, existing) =
        interpret_batch_entries(&state.db, course_id, &entries, true).await?;
    if errors.iter().any(Option::is_some) {
        let listed: Vec<serde_json::Value> = errors
            .into_iter()
            .map(|error| match error {
                Some(message) => serde_json::json!([message]),
                None => serde_json::Value::Null,
            })
            .collect();
        return Ok(batch_error_body(serde_json::Value::Array(listed)));
    }

    let mut tx = state.db.begin().await?;
    let mut updated: Vec<AssignmentOverride> = Vec::with_capacity(parsed.len());
    for (index, entry) in parsed.iter().enumerate() {
        let current = existing[index].clone().expect("validated above");
        let is_adhoc = !current.student_ids.is_empty();

        // Target sets of group/section overrides cannot change; an adhoc
        // override without student_ids keeps its set. A resolved target on
        // an update entry is the adhoc replacement set (validated above).
        let (student_ids, course_section_id, group_id) = if is_adhoc {
            match &targets[index] {
                Some(ResolvedBatchTarget::Adhoc(ids)) => (ids.clone(), None, None),
                _ => (current.student_ids.clone(), None, None),
            }
        } else {
            (Vec::new(), current.course_section_id, current.group_id)
        };

        let title = if is_adhoc {
            entry
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| current.title.clone())
        } else {
            current.title.clone()
        };

        let row: AssignmentOverride = sqlx::query_as(
            r#"
            UPDATE assignment_overrides
            SET title = $3, due_at = $4, unlock_at = $5, lock_at = $6,
                student_ids = $7, course_section_id = $8, group_id = $9,
                updated_at = now()
            WHERE id = $1 AND assignment_id = $2 AND workflow_state <> 'deleted'
            RETURNING id, assignment_id, title, due_at, unlock_at, lock_at,
                      student_ids, course_section_id, group_id, workflow_state
            "#,
        )
        .bind(current.id)
        .bind(current.assignment_id)
        .bind(&title)
        .bind(entry.due_at.flatten())
        .bind(entry.unlock_at.flatten())
        .bind(entry.lock_at.flatten())
        .bind(&student_ids)
        .bind(course_section_id)
        .bind(group_id)
        .fetch_one(&mut *tx)
        .await?;
        updated.push(row);
    }
    tx.commit().await?;

    let values: Vec<serde_json::Value> = updated
        .iter()
        .map(|row| serde_json::to_value(row).unwrap_or_default())
        .collect();
    Ok((StatusCode::OK, Json(serde_json::Value::Array(values))))
}

/// Shared alias implementation (Canvas `group_alias` / `section_alias`):
/// resolve the set's course, require course read + assignment, then 302 to
/// the canonical override URL — 404 when no visible override targets the set.
async fn assignment_override_alias(
    pool: &PgPool,
    course_id: i64,
    assignment_id: i64,
    group_id: Option<i64>,
    section_id: Option<i64>,
    auth: &AuthUser,
) -> Result<(StatusCode, [(axum::http::HeaderName, String); 1]), ApiError> {
    ensure_course_not_deleted(pool, course_id).await?;

    if !check_enrolled_or_admin(pool, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM assignments WHERE id = $1 AND course_id = $2")
            .bind(assignment_id)
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    row.ok_or(ApiError::NotFound)?;

    let visible = visible_override_ids(pool, course_id, assignment_id, auth).await?;
    let override_id: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT id FROM assignment_overrides
        WHERE assignment_id = $1 AND workflow_state <> 'deleted'
          AND (($2::bigint IS NOT NULL AND group_id = $2)
            OR ($3::bigint IS NOT NULL AND course_section_id = $3))
        ORDER BY id ASC
        LIMIT 1
        "#,
    )
    .bind(assignment_id)
    .bind(group_id)
    .bind(section_id)
    .fetch_optional(pool)
    .await?;

    let Some(override_id) = override_id.filter(|id| visible.contains(id)) else {
        return Err(ApiError::NotFound);
    };

    let location =
        format!("/api/v1/courses/{course_id}/assignments/{assignment_id}/overrides/{override_id}");
    Ok((
        StatusCode::FOUND,
        [(axum::http::header::LOCATION, location)],
    ))
}

/// GET /groups/{groupId}/assignments/{assignmentId}/override — Canvas
/// `group_alias`: 302 to the override for that group, 404 otherwise.
async fn group_assignment_override_alias(
    State(state): State<AppState>,
    Path((group_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, [(axum::http::HeaderName, String); 1]), ApiError> {
    let course_id: Option<i64> = sqlx::query_scalar(
        "SELECT course_id FROM groups WHERE id = $1 AND course_id IS NOT NULL AND workflow_state <> 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;
    let course_id = course_id.ok_or(ApiError::NotFound)?;

    assignment_override_alias(
        &state.db,
        course_id,
        assignment_id,
        Some(group_id),
        None,
        &auth,
    )
    .await
}

/// GET /sections/{sectionId}/assignments/{assignmentId}/override — Canvas
/// `section_alias`: 302 to the override for that section, 404 otherwise.
async fn section_assignment_override_alias(
    State(state): State<AppState>,
    Path((section_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, [(axum::http::HeaderName, String); 1]), ApiError> {
    let course_id: Option<i64> = sqlx::query_scalar(
        "SELECT course_id FROM course_sections WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(section_id)
    .fetch_optional(&state.db)
    .await?;
    let course_id = course_id.ok_or(ApiError::NotFound)?;

    assignment_override_alias(
        &state.db,
        course_id,
        assignment_id,
        None,
        Some(section_id),
        &auth,
    )
    .await
}

// ---------------------------------------------------------------------------
// Assignments API tail (Canvas AssignmentsApiController#user_index /
// student_group_members / duplicate / bulk_update / retry_alignment_clone /
// accessibility_scan / accessibility_queue_scan; issue #55)
// ---------------------------------------------------------------------------

/// GET /users/{userId}/courses/{courseId}/assignments — Canvas `user_index`:
/// the course's assignments as visible to the target user. Canvas
/// `require_user_visibility` allows the caller when they are the target
/// ("self" or matching id), a teacher/admin of the course viewing one of its
/// students, or a global admin. (Observer pairings have no Star equivalent —
/// the enrollments schema carries no `associated_user_id`.) The target user
/// must then be able to read the course (Canvas
/// `authorized_action(@context, @user, :read)`).
async fn user_index_assignments(
    State(state): State<AppState>,
    Path((user_id, course_id)): Path<(String, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<Assignment>>, ApiError> {
    let course: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    if course.is_none() {
        return Err(ApiError::NotFound);
    }

    // Resolve the target user ("self" or an id).
    let target_id = if user_id == "self" {
        auth.id
    } else {
        user_id.parse::<i64>().map_err(|_| ApiError::NotFound)?
    };
    let target: Option<(String,)> = sqlx::query_as("SELECT role FROM users WHERE id = $1")
        .bind(target_id)
        .fetch_optional(&state.db)
        .await?;
    let Some((target_role,)) = target else {
        return Err(ApiError::NotFound);
    };

    // Caller visibility: self, teacher of the course (viewing a student), or
    // global admin.
    let caller_is_teacher = check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;
    if target_id != auth.id && !is_admin(&auth) {
        if !caller_is_teacher {
            return Err(ApiError::Forbidden);
        }
        let student_row: Option<(i32,)> = sqlx::query_as(
            r#"
            SELECT 1 FROM enrollments
            WHERE course_id = $1 AND user_id = $2
              AND role = 'student' AND workflow_state <> 'deleted'
            LIMIT 1
            "#,
        )
        .bind(course_id)
        .bind(target_id)
        .fetch_optional(&state.db)
        .await?;
        if student_row.is_none() {
            return Err(ApiError::Forbidden);
        }
    }

    // The target user must be able to read the course (Canvas checks the
    // target's rights, not the caller's).
    let target_can_read = target_role == "admin"
        || sqlx::query_as::<_, (i32,)>(
            r#"
            SELECT 1 FROM enrollments
            WHERE course_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'
            LIMIT 1
            "#,
        )
        .bind(course_id)
        .bind(target_id)
        .fetch_optional(&state.db)
        .await?
        .is_some();
    if !target_can_read {
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

/// GET /courses/{courseId}/assignments/{assignmentId}/users/{userId}/group_members
/// — Canvas `student_group_members`: `{id, name}` BasicUser entries for the
/// group the student belongs to. Star assignments carry no
/// `group_category_id`, so Canvas' `group_students` always falls to the
/// `[student]` branch (rspec
/// "returns an array with the student only if not a group assignment").
async fn student_group_members(
    State(state): State<AppState>,
    Path((course_id, assignment_id, user_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    ensure_assignment_in_course(&state.db, course_id, assignment_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas `@context.students.find(params[:user_id])`: the user must be an
    // active student of the course, else 404.
    let student: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT u.name FROM enrollments e
        JOIN users u ON u.id = e.user_id
        WHERE e.course_id = $1 AND e.user_id = $2
          AND e.role = 'student' AND e.workflow_state <> 'deleted'
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((name,)) = student else {
        return Err(ApiError::NotFound);
    };

    // Canvas serializes BasicUser.id with `user.id.to_s`.
    Ok(Json(vec![serde_json::json!({
        "id": user_id.to_string(),
        "name": name,
    })]))
}

/// Canvas `get_copy_title(entity, "Copy", title)` against the course's
/// conflicting titles: "Essay" -> "Essay Copy", "Essay Copy" -> "Essay Copy 2".
fn duplicate_copy_title(title: &str, course_conflicts: &[String]) -> String {
    let normalize = |value: &str| value.replace(' ', "-").to_lowercase();
    let normalized = normalize(title);
    // Ruby /copy-[0-9]*$/ — the digit run may be empty.
    let is_multiple = normalized
        .rsplit_once("copy-")
        .is_some_and(|(_, digits)| digits.chars().all(|c| c.is_ascii_digit()));
    let ends_with_copy = normalized.ends_with("copy");

    let (potential, mut number) = if ends_with_copy || is_multiple {
        let trailing = normalized
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect::<Vec<_>>();
        let number = if trailing.is_empty() {
            2
        } else {
            trailing
                .iter()
                .rev()
                .collect::<String>()
                .parse::<u32>()
                .unwrap_or(2)
                + 1
        };
        (title.to_string(), number)
    } else {
        (format!("{title} Copy"), 2)
    };

    let title_base = if is_multiple {
        // Strip the trailing digits for the base ("Essay Copy 2" -> "Essay Copy ").
        let trimmed = title.trim_end_matches(|c: char| c.is_ascii_digit());
        format!("{trimmed} ")
    } else {
        format!("{potential} ")
    };

    let conflicts: HashSet<String> = course_conflicts.iter().map(|c| normalize(c)).collect();
    if !conflicts.contains(&normalize(&potential)) {
        return potential;
    }
    loop {
        let attempt = format!("{title_base}{number}");
        number += 1;
        if !conflicts.contains(&normalize(&attempt)) {
            return attempt;
        }
    }
}

/// POST /courses/{courseId}/assignments/{assignmentId}/duplicate — Canvas
/// `duplicate`: copies the assignment inside its course (or, with
/// `target_course_id`, into another course the caller may add assignments
/// to), answering the new assignment plus `new_positions` for its group.
/// Star assignments have no `position` column, so positions are derived from
/// id order within the group (Canvas moves the copy below the target
/// assignment; the response keeps the documented shape).
async fn duplicate_assignment(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let course: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    if course.is_none() {
        return Err(ApiError::NotFound);
    }

    // Canvas looks the source up with find_by and answers 400 when missing.
    let source: Option<Assignment> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        FROM assignments WHERE id = $1 AND course_id = $2
        "#,
    )
    .bind(assignment_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let Some(source) = source else {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "assignment does not exist" })),
        ));
    };

    // Canvas authorized_action(source, :create).
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Optional cross-course duplicate target.
    let mut target_course_id = course_id;
    let mut target_assignment_id = Some(source.id);
    if let Some(query) = raw_query.as_deref() {
        for pair in query.split('&') {
            let Some((raw_key, raw_value)) = pair.split_once('=') else {
                continue;
            };
            let key = decode_query_component(raw_key);
            let value = decode_query_component(raw_value);
            if key == "target_course_id" {
                if let Ok(parsed) = value.parse::<i64>() {
                    target_course_id = parsed;
                }
            } else if key == "target_assignment_id" {
                target_assignment_id = value.parse::<i64>().ok();
            }
        }
    }
    if target_course_id != course_id {
        let target_course: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
                .bind(target_course_id)
                .fetch_optional(&state.db)
                .await?;
        if target_course.is_none() {
            return Err(ApiError::NotFound);
        }
        // Canvas authorized_action(target_course, :manage_assignments_add).
        if !check_teacher_or_admin(&state.db, auth.id, target_course_id, &auth).await? {
            return Err(ApiError::Forbidden);
        }
    }

    // The target assignment positions the copy and (on retry) names it.
    let target_assignment: Option<Assignment> = match target_assignment_id {
        Some(id) if id != source.id => {
            sqlx::query_as(
                r#"
                SELECT id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
                FROM assignments WHERE id = $1 AND course_id = $2
                "#,
            )
            .bind(id)
            .bind(target_course_id)
            .fetch_optional(&state.db)
            .await?
        }
        _ => Some(source.clone()),
    };
    let Some(target_assignment) = target_assignment else {
        return Err(ApiError::NotFound);
    };

    // Title: on a failure retry Canvas reuses the failed copy's title.
    let copy_title = if target_assignment.id != source.id {
        target_assignment.title.clone()
    } else {
        let conflicts: Vec<String> =
            sqlx::query_scalar("SELECT title FROM assignments WHERE course_id = $1")
                .bind(course_id)
                .fetch_all(&state.db)
                .await?;
        duplicate_copy_title(&source.title, &conflicts)
    };

    // Canvas keeps the source's assignment group on a plain duplicate and
    // switches to the target assignment's group on a cross-course retry;
    // `clone` also copies the anonymous grading and peer review flags while
    // zeroing the peer review count (allocation feature off) and clearing
    // unique identifiers (star's anonymous_id).
    let (source_group, anonymous_grading, peer_reviews): (Option<i64>, bool, bool) = sqlx::query_as(
        "SELECT assignment_group_id, anonymous_grading, peer_reviews FROM assignments WHERE id = $1",
    )
    .bind(source.id)
    .fetch_one(&state.db)
    .await?;
    let group_id = if target_course_id != course_id {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT assignment_group_id FROM assignments WHERE id = $1",
        )
        .bind(target_assignment.id)
        .fetch_one(&state.db)
        .await?
    } else {
        source_group
    };

    let mut tx = state.db.begin().await?;

    let copy: Assignment = sqlx::query_as(
        r#"
        INSERT INTO assignments
            (course_id, title, description, points_possible, due_at,
             assignment_group_id, anonymous_grading, peer_reviews,
             peer_review_count, peer_reviews_assign_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, NULL)
        RETURNING id, course_id, title, description, points_possible::float8, due_at, created_at, updated_at
        "#,
    )
    .bind(target_course_id)
    .bind(&copy_title)
    .bind(&source.description)
    .bind(source.points_possible)
    .bind(source.due_at)
    .bind(group_id)
    .bind(anonymous_grading)
    .bind(peer_reviews)
    .fetch_one(&mut *tx)
    .await?;

    // new_positions: every assignment of the copy's group keyed by id.
    // Star has no stored positions, so they are derived from id order
    // (Canvas moves the copy below the target assignment; the response
    // keeps the documented shape).
    let mut new_positions: HashMap<String, i64> = HashMap::new();
    if let Some(group_id) = group_id {
        let positions: Vec<(i64, i64)> = sqlx::query_as(
            r#"
            SELECT id, ROW_NUMBER() OVER (ORDER BY id)::bigint FROM assignments
            WHERE assignment_group_id = $1
            "#,
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await?;
        for (id, position) in positions {
            new_positions.insert(id.to_string(), position);
        }
    }
    tx.commit().await?;

    let mut value = serde_json::to_value(&copy).unwrap_or_default();
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "new_positions".to_string(),
            serde_json::to_value(&new_positions).unwrap_or_default(),
        );
    }
    Ok((StatusCode::OK, Json(value)))
}

/// PUT /courses/{courseId}/assignments/bulk_update — Canvas `bulk_update`:
/// date updates for a list of assignments, applied synchronously (Star has
/// no job worker) and answered with the Canvas Progress shape — the same
/// convention as the landed `update_grades` route. `all_dates` entries
/// update the base assignment (`due_at`; Star assignments carry no
/// unlock_at/lock_at columns) or the named override (full date triple).
async fn bulk_update_assignments(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    let course: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    if course.is_none() {
        return Err(ApiError::NotFound);
    }

    // Canvas manage_assignments_edit on the course.
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let payload: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| ApiError::BadRequest("expected array".into()))?;
    let entries = payload
        .as_array()
        .ok_or_else(|| ApiError::BadRequest("expected array".into()))?;
    if entries.is_empty() || !entries.iter().all(|entry| entry.get("id").is_some()) {
        return Err(ApiError::BadRequest("missing assignment id".into()));
    }

    let requested: Vec<i64> = entries
        .iter()
        .map(|entry| entry.get("id").map(ruby_to_i).unwrap_or(0))
        .collect();

    let found: Vec<(i64,)> =
        sqlx::query_as("SELECT id FROM assignments WHERE course_id = $1 AND id = ANY($2)")
            .bind(course_id)
            .bind(&requested)
            .fetch_all(&state.db)
            .await?;
    // Canvas raises RecordNotFound when any requested id (or a duplicate
    // entry) does not resolve to an active assignment of the course.
    if found.len() != requested.len() {
        return Err(ApiError::NotFound);
    }

    let mut tx = state.db.begin().await?;
    let mut updated_count = 0i64;
    for entry in entries {
        let id = entry.get("id").map(ruby_to_i).unwrap_or(0);
        let Some(dates) = entry.get("all_dates").and_then(serde_json::Value::as_array) else {
            continue;
        };
        let mut changed = false;
        for date in dates {
            if date.get("base").and_then(serde_json::Value::as_bool) == Some(true) {
                // Base date: update the assignment's due_at (Star assignments
                // carry no unlock_at/lock_at columns).
                let due_at = match date.get("due_at") {
                    Some(serde_json::Value::Null) => None,
                    Some(value) => value.as_str().and_then(parse_canvas_datetime),
                    None => continue,
                };
                changed = true;
                sqlx::query("UPDATE assignments SET due_at = $2, updated_at = now() WHERE id = $1")
                    .bind(id)
                    .bind(due_at)
                    .execute(&mut *tx)
                    .await?;
            } else {
                // Override date: update the named override's date triple;
                // keys absent from the entry clear the override value
                // (Canvas sets a value only when the key is present).
                let Some(override_id) = date.get("id").map(ruby_to_i) else {
                    continue;
                };
                let override_row: Option<(i64,)> = sqlx::query_as(
                    "SELECT id FROM assignment_overrides \
                     WHERE id = $1 AND assignment_id = $2 AND workflow_state <> 'deleted'",
                )
                .bind(override_id)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
                if override_row.is_none() {
                    // Canvas fails the queued progress without applying any
                    // change; the open transaction is dropped (rolled back).
                    let message =
                        format!("invalid assignment override id {override_id} for assignment {id}");
                    return Ok(Json(serde_json::json!({
                        "id": 0,
                        "context_id": course_id,
                        "context_type": "Course",
                        "user_id": auth.id,
                        "tag": "assignment_bulk_update",
                        "completion": 0,
                        "workflow_state": "failed",
                        "created_at": Utc::now(),
                        "updated_at": Utc::now(),
                        "url": "/api/v1/progress/0",
                        "results": { "message": message },
                    })));
                }
                let read_date = |field: &str| -> Option<DateTime<Utc>> {
                    match date.get(field) {
                        Some(serde_json::Value::Null) => None,
                        Some(value) => value.as_str().and_then(parse_canvas_datetime),
                        None => None,
                    }
                };
                changed = true;
                sqlx::query(
                    r#"
                    UPDATE assignment_overrides
                    SET due_at = $2, unlock_at = $3, lock_at = $4, updated_at = now()
                    WHERE id = $1
                    "#,
                )
                .bind(override_id)
                .bind(read_date("due_at"))
                .bind(read_date("unlock_at"))
                .bind(read_date("lock_at"))
                .execute(&mut *tx)
                .await?;
            }
        }
        if changed {
            updated_count += 1;
        }
    }
    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "id": 0,
        "context_id": course_id,
        "context_type": "Course",
        "user_id": auth.id,
        "tag": "assignment_bulk_update",
        "completion": 100,
        "workflow_state": "completed",
        "created_at": Utc::now(),
        "updated_at": Utc::now(),
        "url": "/api/v1/progress/0",
        "results": { "updated_count": updated_count },
    })))
}

/// POST /courses/{courseId}/assignments/{assignmentId}/retry_alignment_clone
/// — Canvas `retry_alignment_clone` only proceeds when the target assignment
/// sits in the `failed_to_clone_outcome_alignment` duplication state. Star
/// assignments have no duplication workflow states (and no Outcomes service
/// to re-clone alignments), so the state guard rejects every representable
/// request with Canvas' exact error body.
async fn retry_alignment_clone(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    _auth: AuthUser,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let course: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    if course.is_none() {
        return Err(ApiError::NotFound);
    }

    // Resolve the target assignment (Canvas: the assignment itself unless a
    // target_assignment_id is given).
    let mut target_id = assignment_id;
    if let Some(query) = raw_query.as_deref() {
        for pair in query.split('&') {
            let Some((raw_key, raw_value)) = pair.split_once('=') else {
                continue;
            };
            if decode_query_component(raw_key) == "target_assignment_id" {
                if let Ok(parsed) = decode_query_component(raw_value).parse::<i64>() {
                    target_id = parsed;
                }
            }
        }
    }
    let target: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM assignments WHERE id = $1 AND course_id = $2")
            .bind(target_id)
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    if target.is_none() {
        return Err(ApiError::NotFound);
    }

    // No duplication workflow states exist in the Star schema, so the
    // state guard can never pass.
    Ok((
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": "invalid status" })),
    ))
}

/// Shared accessibility-scan implementation (Canvas
/// `accessibility_scan` / `accessibility_queue_scan`): course-content
/// management rights, then the account's a11y checker flags, then the
/// assignment — the same gate order as Canvas. Star has no a11y rule
/// engine, so the completed scan reports zero issues (shape parity with
/// `accessibility_resource_scan_json`; the scan id mirrors Canvas'
/// one-scan-per-resource record).
async fn assignment_accessibility_scan(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    let course: Option<(i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, account_id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((_course, account_id)) = course else {
        return Err(ApiError::NotFound);
    };

    // Granular manage-course-content rights decompose the teacher's
    // manage_content in Canvas; teachers/admins hold them in star-lms.
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Course#a11y_checker_enabled?: the GA1 account flag, or the a11y_checker
    // account flag plus the course's EAP flag.
    let checker_enabled = match account_id {
        Some(account) => {
            let flags: Vec<(String, String)> = sqlx::query_as(
                r#"
                SELECT feature, state FROM feature_flags
                WHERE context_type = 'Account' AND context_id = $1
                  AND feature IN ('a11y_checker', 'a11y_checker_ga1')
                "#,
            )
            .bind(account)
            .fetch_all(&state.db)
            .await?;
            let account_flag = |name: &str| {
                flags
                    .iter()
                    .any(|(feature, state)| feature == name && state == "enabled")
            };
            let ga1 = account_flag("a11y_checker_ga1");
            let base = account_flag("a11y_checker");
            let eap = if base {
                sqlx::query_as::<_, (String, String)>(
                    r#"
                    SELECT feature, state FROM feature_flags
                    WHERE context_type = 'Course' AND context_id = $1
                      AND feature = 'a11y_checker_eap'
                    "#,
                )
                .bind(course_id)
                .fetch_optional(&state.db)
                .await?
                .is_some_and(|(_, state)| state == "enabled")
            } else {
                false
            };
            ga1 || (base && eap)
        }
        None => false,
    };
    if !checker_enabled {
        return Err(ApiError::Forbidden);
    }

    let assignment: Option<(i64, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, title, updated_at FROM assignments WHERE id = $1 AND course_id = $2",
    )
    .bind(assignment_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((id, title, updated_at)) = assignment else {
        return Err(ApiError::NotFound);
    };

    Ok(Json(serde_json::json!({
        "id": id,
        "resource_id": id,
        "resource_type": "Assignment",
        "resource_name": title,
        "resource_workflow_state": "published",
        "resource_updated_at": updated_at.to_rfc3339(),
        "resource_url": format!("/courses/{course_id}/assignments/{id}"),
        "resource_scan_path": serde_json::Value::Null,
        "workflow_state": "completed",
        "error_message": "",
        "issue_count": 0,
        "issues": [],
    })))
}

/// POST /courses/{courseId}/assignments/{assignmentId}/accessibility/scan
/// — Canvas `accessibility_scan` (synchronous scan).
async fn assignment_accessibility_scan_sync(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    assignment_accessibility_scan(State(state), Path((course_id, assignment_id)), auth).await
}

/// POST /courses/{courseId}/assignments/{assignmentId}/accessibility/queue_scan
/// — Canvas `accessibility_queue_scan` (queued scan; Star applies it
/// synchronously, like every other Progress-returning route).
async fn assignment_accessibility_queue_scan(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    assignment_accessibility_scan(State(state), Path((course_id, assignment_id)), auth).await
}

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
        // Anonymous submissions: section twins (spec p5 slice E, #30)
        .route(
            "/sections/{sectionId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}",
            get(section_show_anonymous_submission),
        )
        .route(
            "/sections/{sectionId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}",
            put(section_update_anonymous_submission),
        )
        // Moderated / provisional grading (Canvas ModerationSetController,
        // ProvisionalGradesController, AnonymousProvisionalGradesController,
        // spec p5 slice F, #31)
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/moderated_students",
            get(moderation_set_index),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/moderated_students",
            post(moderation_set_create),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/provisional_grades/status",
            get(provisional_grades_status),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/provisional_grades/bulk_select",
            put(provisional_grades_bulk_select),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/provisional_grades/publish",
            post(provisional_grades_publish),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/provisional_grades/{provisionalGradeId}/select",
            put(provisional_grades_select),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/anonymous_provisional_grades/status",
            get(anonymous_provisional_grades_status),
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
        // Assignment overrides — batch routes (Canvas
        // AssignmentOverridesController#batch_*, issue #55)
        .route(
            "/courses/{courseId}/assignments/overrides",
            get(batch_retrieve_assignment_overrides),
        )
        .route(
            "/courses/{courseId}/assignments/overrides",
            post(batch_create_assignment_overrides),
        )
        .route(
            "/courses/{courseId}/assignments/overrides",
            put(batch_update_assignment_overrides),
        )
        // Assignment overrides — group/section aliases (#55)
        .route(
            "/groups/{groupId}/assignments/{assignmentId}/override",
            get(group_assignment_override_alias),
        )
        .route(
            "/sections/{sectionId}/assignments/{assignmentId}/override",
            get(section_assignment_override_alias),
        )
        // Assignments API tail (Canvas AssignmentsApiController, issue #55)
        .route(
            "/users/{userId}/courses/{courseId}/assignments",
            get(user_index_assignments),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/users/{userId}/group_members",
            get(student_group_members),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/duplicate",
            post(duplicate_assignment),
        )
        .route(
            "/courses/{courseId}/assignments/bulk_update",
            put(bulk_update_assignments),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/retry_alignment_clone",
            post(retry_alignment_clone),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/accessibility/scan",
            post(assignment_accessibility_scan_sync),
        )
        .route(
            "/courses/{courseId}/assignments/{assignmentId}/accessibility/queue_scan",
            post(assignment_accessibility_queue_scan),
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
