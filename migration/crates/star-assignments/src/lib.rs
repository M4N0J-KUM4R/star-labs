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
