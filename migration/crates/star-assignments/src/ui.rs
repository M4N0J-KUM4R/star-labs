//! Pub data loaders for the SpeedGrader-style grading page (star-ui,
//! issue #108).
//!
//! Same extraction pattern as `star_conversations::ui`: the grading page
//! needs student names joined with submission state in one query — a shape
//! the REST API does not serve (`gradeable_students` lists only students
//! WITH a submission; the shell renders every enrolled student, Canvas
//! SpeedGrader order) — so the query lives here as a `pub` fn instead of
//! being duplicated in the shell crate.
//!
//! The grade write itself goes through
//! [`grade_submission_for_user`], which reuses the exact validation and
//! upsert shape of the REST `course_grade_submission` handler (Canvas
//! `submissions_api#update`): score required, graded_at/grader_id stamped,
//! workflow_state flipped to `graded`, submission row created on conflict.
//! Authorization stays with the caller (the shell gates through
//! `star_courses::check_teacher_or_admin` first).

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use star_common::ApiError;

use crate::Submission;

/// One student row of the SpeedGrader queue: every enrolled student of the
/// course (Canvas lists all enrollments; a missing submission renders as
/// "not submitted") with their submission state for the selected assignment.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GradingStudentRow {
    pub user_id: i64,
    pub user_name: String,
    /// `submissions.id` when the student has one for this assignment.
    pub submission_id: Option<i64>,
    /// "submitted" | "graded" | NULL (no submissions row).
    pub submission_state: Option<String>,
    /// The student's score, when graded.
    pub score: Option<f64>,
}

/// Load the SpeedGrader student list for one assignment — every non-deleted
/// enrollment LEFT JOINed with the (assignment, user) submission, ordered by
/// student name then id (Canvas SpeedGrader's default student order).
///
/// Teachers appear in the list too (Canvas gradebook lists every enrollment;
/// the teacher's own row simply has no submission).
pub async fn load_grading_students(
    db: &PgPool,
    course_id: i64,
    assignment_id: i64,
) -> Result<Vec<GradingStudentRow>, ApiError> {
    let rows: Vec<GradingStudentRow> = sqlx::query_as(
        r#"
        SELECT u.id AS user_id, u.name AS user_name,
               s.id AS submission_id, s.workflow_state AS submission_state,
               s.score::float8 AS score
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        LEFT JOIN submissions s ON s.assignment_id = $2 AND s.user_id = u.id
        WHERE e.course_id = $1 AND e.workflow_state <> 'deleted'
        ORDER BY u.name ASC, u.id ASC
        "#,
    )
    .bind(course_id)
    .bind(assignment_id)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Grade (or re-grade) one student's submission for an assignment — the
/// shell's write path for the SpeedGrader form.
///
/// Same INSERT ... ON CONFLICT shape as the REST `course_grade_submission`
/// handler (Canvas `find_or_create_submission` + `grade_student`). Score
/// parsing/validation happens in the caller (the REST handler parses the
/// posted grade there too); the numeric guard below only rejects the two
/// values Postgres cannot round-trip (NaN/inf), which the REST path's
/// `parse_grade_value` also rejects at the JSON layer. Returns the
/// persisted submission so the shell can re-render the exact stored state.
pub async fn grade_submission_for_user(
    db: &PgPool,
    assignment_id: i64,
    user_id: i64,
    grader_id: i64,
    score: f64,
) -> Result<Submission, ApiError> {
    if !score.is_finite() {
        return Err(ApiError::Validation("score must be a number".into()));
    }

    let submission: Submission = sqlx::query_as(
        r#"
        INSERT INTO submissions
            (assignment_id, user_id, score, graded_at, grader_id, workflow_state)
        VALUES ($1, $2, $3, now(), $4, 'graded')
        ON CONFLICT (assignment_id, user_id) DO UPDATE
        SET score = EXCLUDED.score,
            graded_at = now(),
            grader_id = EXCLUDED.grader_id,
            workflow_state = 'graded',
            updated_at = now()
        RETURNING id, assignment_id, user_id, body, score::float8, submitted_at,
                  created_at, updated_at
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(score)
    .bind(grader_id)
    .fetch_one(db)
    .await?;
    Ok(submission)
}

/// Load one student's submission for an assignment (with its attempt info),
/// or `None` when the student has not submitted.
pub async fn load_submission_for(
    db: &PgPool,
    assignment_id: i64,
    user_id: i64,
) -> Result<Option<GradingSubmissionView>, ApiError> {
    let row: Option<GradingSubmissionView> = sqlx::query_as(
        r#"
        SELECT id, body, score::float8, submission_type, workflow_state,
               attempt, submitted_at, graded_at, grader_id
        FROM submissions
        WHERE assignment_id = $1 AND user_id = $2
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// The submission columns the SpeedGrader page renders.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GradingSubmissionView {
    pub id: i64,
    pub body: Option<String>,
    pub score: Option<f64>,
    pub submission_type: Option<String>,
    pub workflow_state: String,
    pub attempt: i32,
    pub submitted_at: DateTime<Utc>,
    pub graded_at: Option<DateTime<Utc>>,
    pub grader_id: Option<i64>,
}
