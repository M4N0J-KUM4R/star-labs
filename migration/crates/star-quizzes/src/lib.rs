//! star-quizzes: quiz metadata only — NOT a question engine.
//!
//! Handles: quiz CRUD, quiz submission start (upsert), and submission listing.
//! Question/answer management is out of scope.
//!
//! Canvas parity notes (app/models/quizzes/quiz.rb):
//! - `shuffle_answers` (Canvas field name; our old column was shuffle_questions).
//! - `quiz_type` is validated to Canvas' four values: assignment, practice,
//!   survey, graded_survey.
//! - `published` is Canvas' boolean workflow equivalent; Canvas soft-deletes via
//!   workflow_state='deleted' while keeping submissions — we hard-delete
//!   instead (submissions cascade away with the quiz). Documented difference.
//! - POST /quizzes/{id}/start is Canvas' "take" alias for beginning a
//!   quiz_submission; we keep the /start endpoint name.
//! - Canvas nested aliases live under /courses/{courseId}/quizzes/{id}:
//!   GET/PUT/DELETE (quiz CRUD), POST .../submissions (start), and
//!   GET .../submission (singular; the caller's own submission).

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
// Auth (mirrors star-assignments — reads x-user-id / x-user-role headers)
// ---------------------------------------------------------------------------

// (AuthUser is now imported from star_auth)

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Quiz {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub quiz_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_limit: Option<i32>,
    pub shuffle_answers: bool,
    pub allowed_attempts: i32,
    pub published: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_from: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_until: Option<DateTime<Utc>>,
    pub points_possible: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateQuiz {
    pub title: String,
    pub description: Option<String>,
    pub quiz_type: Option<String>,
    pub time_limit: Option<i32>,
    pub shuffle_answers: Option<bool>,
    pub allowed_attempts: Option<i32>,
    pub published: Option<bool>,
    pub available_from: Option<DateTime<Utc>>,
    pub available_until: Option<DateTime<Utc>>,
    pub points_possible: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateQuiz {
    pub title: Option<String>,
    pub description: Option<String>,
    pub quiz_type: Option<String>,
    pub time_limit: Option<i32>,
    pub shuffle_answers: Option<bool>,
    pub allowed_attempts: Option<i32>,
    pub published: Option<bool>,
    pub available_from: Option<DateTime<Utc>>,
    pub available_until: Option<DateTime<Utc>>,
    pub points_possible: Option<f64>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct QuizSubmission {
    pub id: i64,
    pub quiz_id: i64,
    pub user_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct QuizSubmissionWithUser {
    pub id: i64,
    pub quiz_id: i64,
    pub user_id: i64,
    pub user_name: String,
    pub user_email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

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

/// Require that `quiz_id` exists and belongs to `course_id`; otherwise 404.
/// Canvas nested routes treat a quiz outside the path course as not found.
async fn require_quiz_in_course(
    pool: &PgPool,
    quiz_id: i64,
    course_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM quizzes WHERE id = $1"#)
        .bind(quiz_id)
        .fetch_optional(pool)
        .await?;

    match row {
        Some((quiz_course_id,)) if quiz_course_id == course_id => Ok(()),
        _ => Err(ApiError::NotFound),
    }
}

// ---------------------------------------------------------------------------
// Quiz type validation (Canvas parity: app/models/quizzes/quiz.rb)
// ---------------------------------------------------------------------------

/// Canvas quiz_type values: assignment | practice | survey | graded_survey.
const CANVAS_QUIZ_TYPES: [&str; 4] = ["assignment", "practice", "survey", "graded_survey"];

fn valid_quiz_type(quiz_type: &str) -> bool {
    CANVAS_QUIZ_TYPES.contains(&quiz_type)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/quizzes
async fn list_course_quizzes(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Quiz>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<Quiz> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, quiz_type,
               time_limit, shuffle_answers, allowed_attempts, published,
               available_from, available_until,
               points_possible::float8, created_at, updated_at
        FROM quizzes
        WHERE course_id = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// POST /courses/{courseId}/quizzes  → 201
async fn create_quiz(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateQuiz>,
) -> Result<(StatusCode, Json<Quiz>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }

    let quiz_type = payload.quiz_type.as_deref().unwrap_or("assignment");
    if !valid_quiz_type(quiz_type) {
        return Err(ApiError::Validation(format!(
            "invalid quiz_type: must be one of {}",
            CANVAS_QUIZ_TYPES.join(", ")
        )));
    }
    let points_possible = payload.points_possible.unwrap_or(0.0);

    let row: Quiz = sqlx::query_as(
        r#"
        INSERT INTO quizzes
            (course_id, title, description, quiz_type, time_limit,
             shuffle_answers, allowed_attempts, published,
             available_from, available_until, points_possible)
        VALUES ($1, $2, $3, $4, $5, COALESCE($6, false), COALESCE($7, 1), COALESCE($8, false),
                $9, $10, $11)
        RETURNING id, course_id, title, description, quiz_type,
                  time_limit, shuffle_answers, allowed_attempts, published,
                  available_from, available_until,
                  points_possible::float8, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(quiz_type)
    .bind(payload.time_limit)
    .bind(payload.shuffle_answers)
    .bind(payload.allowed_attempts)
    .bind(payload.published)
    .bind(payload.available_from)
    .bind(payload.available_until)
    .bind(points_possible)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /quizzes/{id}
async fn get_quiz(
    State(state): State<AppState>,
    Path(quiz_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Quiz>, ApiError> {
    let row: Option<Quiz> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, quiz_type,
               time_limit, shuffle_answers, allowed_attempts, published,
               available_from, available_until,
               points_possible::float8, created_at, updated_at
        FROM quizzes WHERE id = $1
        "#,
    )
    .bind(quiz_id)
    .fetch_optional(&state.db)
    .await?;

    let quiz = row.ok_or(ApiError::NotFound)?;

    if !check_enrolled_or_admin(&state.db, auth.id, quiz.course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(quiz))
}

/// PUT /quizzes/{id}
async fn update_quiz(
    State(state): State<AppState>,
    Path(quiz_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateQuiz>,
) -> Result<Json<Quiz>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM quizzes WHERE id = $1"#)
        .bind(quiz_id)
        .fetch_optional(&state.db)
        .await?;

    let (course_id,) = row.ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Fetch current values for partial updates
    let current: Quiz = sqlx::query_as(
        r#"
        SELECT id, course_id, title, description, quiz_type,
               time_limit, shuffle_answers, allowed_attempts, published,
               available_from, available_until,
               points_possible::float8, created_at, updated_at
        FROM quizzes WHERE id = $1
        "#,
    )
    .bind(quiz_id)
    .fetch_one(&state.db)
    .await?;

    let title = match payload.title {
        Some(title) => {
            if title.trim().is_empty() {
                return Err(ApiError::Validation("title is required".into()));
            }
            title
        }
        None => current.title,
    };
    let description = payload.description.or(current.description);
    let quiz_type = payload.quiz_type.unwrap_or(current.quiz_type);
    if !valid_quiz_type(&quiz_type) {
        return Err(ApiError::Validation(format!(
            "invalid quiz_type: must be one of {}",
            CANVAS_QUIZ_TYPES.join(", ")
        )));
    }
    let time_limit = payload.time_limit.or(current.time_limit);
    let shuffle_answers = payload.shuffle_answers.unwrap_or(current.shuffle_answers);
    let allowed_attempts = payload.allowed_attempts.unwrap_or(current.allowed_attempts);
    let published = payload.published.unwrap_or(current.published);
    let available_from = payload.available_from.or(current.available_from);
    let available_until = payload.available_until.or(current.available_until);
    let points_possible = payload.points_possible.unwrap_or(current.points_possible);

    let updated: Quiz = sqlx::query_as(
        r#"
        UPDATE quizzes
        SET title = $2, description = $3, quiz_type = $4, time_limit = $5,
            shuffle_answers = $6, allowed_attempts = $7, published = $8,
            available_from = $9, available_until = $10,
            points_possible = $11, updated_at = now()
        WHERE id = $1
        RETURNING id, course_id, title, description, quiz_type,
                  time_limit, shuffle_answers, allowed_attempts, published,
                  available_from, available_until,
                  points_possible::float8, created_at, updated_at
        "#,
    )
    .bind(quiz_id)
    .bind(&title)
    .bind(&description)
    .bind(&quiz_type)
    .bind(time_limit)
    .bind(shuffle_answers)
    .bind(allowed_attempts)
    .bind(published)
    .bind(available_from)
    .bind(available_until)
    .bind(points_possible)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /quizzes/{id}  → 204
///
/// Canvas parity note: Canvas soft-deletes quizzes (workflow_state='deleted')
/// while keeping quiz_submissions. We hard-delete instead — submissions
/// cascade away with the quiz. Documented intentional difference.
async fn delete_quiz(
    State(state): State<AppState>,
    Path(quiz_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM quizzes WHERE id = $1"#)
        .bind(quiz_id)
        .fetch_optional(&state.db)
        .await?;

    let (course_id,) = row.ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(r#"DELETE FROM quizzes WHERE id = $1"#)
        .bind(quiz_id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /quizzes/{id}/start — upsert quiz_submission started_at=now()  → 201
/// Returns 409 Conflict if already started.
///
/// Canvas parity note: Canvas' API calls this "take" (create a quiz
/// submission). We keep the /start endpoint name as the alias.
async fn start_quiz(
    State(state): State<AppState>,
    Path(quiz_id): Path<i64>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<QuizSubmission>), ApiError> {
    // Verify quiz exists and the caller is a student in its course
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM quizzes WHERE id = $1"#)
        .bind(quiz_id)
        .fetch_optional(&state.db)
        .await?;

    let (course_id,) = row.ok_or(ApiError::NotFound)?;

    // Canvas only lets students take quizzes: an enrollment row with
    // role='student' is required (admins get no bypass).
    let enrolled: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'student' LIMIT 1"#,
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    if enrolled.is_none() {
        return Err(ApiError::Forbidden);
    }

    // Upsert: INSERT ... ON CONFLICT (quiz_id, user_id) DO NOTHING ... RETURNING
    // We want: if already exists, return Conflict; if new, insert with started_at=now()
    // Check for existing first to return 409 cleanly
    let existing: Option<(i64,)> =
        sqlx::query_as(r#"SELECT id FROM quiz_submissions WHERE quiz_id = $1 AND user_id = $2"#)
            .bind(quiz_id)
            .bind(auth.id)
            .fetch_optional(&state.db)
            .await?;

    if existing.is_some() {
        return Err(ApiError::Conflict("quiz already started".into()));
    }

    let submission: QuizSubmission = sqlx::query_as(
        r#"
        INSERT INTO quiz_submissions (quiz_id, user_id, started_at)
        VALUES ($1, $2, now())
        RETURNING id, quiz_id, user_id, score, completed_at, started_at, created_at, updated_at
        "#,
    )
    .bind(quiz_id)
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(submission)))
}

/// GET /quizzes/{id}/submissions/me
async fn get_my_submission(
    State(state): State<AppState>,
    Path(quiz_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<QuizSubmission>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM quizzes WHERE id = $1"#)
        .bind(quiz_id)
        .fetch_optional(&state.db)
        .await?;

    let (course_id,) = row.ok_or(ApiError::NotFound)?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let submission: Option<QuizSubmission> = sqlx::query_as(
        r#"
        SELECT id, quiz_id, user_id, score, completed_at, started_at, created_at, updated_at
        FROM quiz_submissions
        WHERE quiz_id = $1 AND user_id = $2
        "#,
    )
    .bind(quiz_id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;

    submission.map(Json).ok_or(ApiError::NotFound)
}

/// GET /courses/{courseId}/quizzes/{id}/submissions — all submissions (teacher/admin)
async fn list_quiz_submissions(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<QuizSubmissionWithUser>>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM quizzes WHERE id = $1"#)
        .bind(quiz_id)
        .fetch_optional(&state.db)
        .await?;

    let (quiz_course_id,) = row.ok_or(ApiError::NotFound)?;

    if quiz_course_id != course_id {
        return Err(ApiError::NotFound);
    }

    if !check_teacher_or_admin(&state.db, auth.id, quiz_course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<QuizSubmissionWithUser> = sqlx::query_as(
        r#"
        SELECT qs.id, qs.quiz_id, qs.user_id,
               u.name as user_name, u.email as user_email,
               qs.score, qs.completed_at, qs.started_at, qs.created_at, qs.updated_at
        FROM quiz_submissions qs
        JOIN users u ON u.id = qs.user_id
        WHERE qs.quiz_id = $1
        ORDER BY qs.started_at DESC
        "#,
    )
    .bind(quiz_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// Canvas nested aliases (/courses/{courseId}/quizzes/{id})
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/quizzes/{id} — alias for GET /quizzes/{id}.
async fn get_course_quiz(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Quiz>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    get_quiz(State(state), Path::<i64>(quiz_id), auth).await
}

/// PUT /courses/{courseId}/quizzes/{id} — alias for PUT /quizzes/{id}.
async fn update_course_quiz(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateQuiz>,
) -> Result<Json<Quiz>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    update_quiz(State(state), Path::<i64>(quiz_id), auth, Json(payload)).await
}

/// DELETE /courses/{courseId}/quizzes/{id} — alias for DELETE /quizzes/{id}.
async fn delete_course_quiz(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    delete_quiz(State(state), Path::<i64>(quiz_id), auth).await
}

/// POST /courses/{courseId}/quizzes/{id}/submissions — Canvas start alias
/// (delegates to POST /quizzes/{id}/start).
async fn start_course_quiz_submission(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<QuizSubmission>), ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    start_quiz(State(state), Path::<i64>(quiz_id), auth).await
}

/// GET /courses/{courseId}/quizzes/{id}/submission (singular) — Canvas alias
/// for GET /quizzes/{id}/submissions/me, returning the caller's submission.
async fn get_course_my_submission(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<QuizSubmission>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    get_my_submission(State(state), Path::<i64>(quiz_id), auth).await
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the quizzes router.
pub fn router() -> Router<AppState> {
    Router::new()
        // Course-level
        .route("/courses/{courseId}/quizzes", get(list_course_quizzes))
        .route("/courses/{courseId}/quizzes", post(create_quiz))
        .route(
            "/courses/{courseId}/quizzes/{id}/submissions",
            get(list_quiz_submissions),
        )
        // Quiz-level
        .route("/quizzes/{id}", get(get_quiz))
        .route("/quizzes/{id}", put(update_quiz))
        .route("/quizzes/{id}", delete(delete_quiz))
        .route("/quizzes/{id}/start", post(start_quiz))
        .route("/quizzes/{id}/submissions/me", get(get_my_submission))
        // Canvas nested aliases (delegate to the flat handlers)
        .route("/courses/{courseId}/quizzes/{id}", get(get_course_quiz))
        .route("/courses/{courseId}/quizzes/{id}", put(update_course_quiz))
        .route(
            "/courses/{courseId}/quizzes/{id}",
            delete(delete_course_quiz),
        )
        .route(
            "/courses/{courseId}/quizzes/{id}/submissions",
            post(start_course_quiz_submission),
        )
        .route(
            "/courses/{courseId}/quizzes/{id}/submission",
            get(get_course_my_submission),
        )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::router;

    /// Verify the router builds without panicking.
    /// (Router::routes() introspection is not available in axum 0.8.9;
    /// build-only verification is sufficient per project conventions.)
    #[test]
    fn router_builds() {
        let _ = router();
    }
}
