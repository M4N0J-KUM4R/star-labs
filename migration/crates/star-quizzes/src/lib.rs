//! star-quizzes: quiz metadata, quiz questions and question groups.
//!
//! Handles: quiz CRUD, quiz submission start (upsert), submission listing,
//! and Canvas-parity CRUD for quiz questions + question groups (and the
//! `question_banks` table they can reference).
//!
//! Canvas parity notes (app/models/quizzes/quiz.rb,
//! app/controllers/quizzes/quiz_questions_controller.rb,
//! app/controllers/quizzes/quiz_groups_controller.rb):
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
//! - Question routes are teacher/admin-only (Canvas authorizes `:update` even
//!   for the question index); groups are readable by any enrolled user.
//!   Question create returns 200, group create 201, deletes 204.
//! - Group responses keep Canvas' compound shape `{"quiz_groups": [...]}`;
//!   question responses are plain objects/arrays. Request bodies accept
//!   Canvas' `question[...]` and `quiz_groups[][...]` wrappers (flat JSON also
//!   works).
//! - Deleting a question is Canvas' soft delete (`workflow_state='deleted'`);
//!   deleting a group destroys its questions.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
// Quiz questions & question groups
// (Canvas Quizzes::QuizQuestionsController / Quizzes::QuizGroupsController)
// ---------------------------------------------------------------------------

/// Canvas QuizQuestion shape. `answers` is the raw answers array, stored
/// serialized in `quiz_questions.answers` (Canvas keeps `question_data` JSON).
#[derive(Debug, Clone, Serialize)]
pub struct QuizQuestion {
    pub id: i64,
    pub quiz_id: i64,
    pub quiz_group_id: Option<i64>,
    pub assessment_question_bank_id: Option<i64>,
    pub position: i32,
    pub question_name: String,
    pub question_type: String,
    pub question_text: String,
    pub points_possible: f64,
    pub correct_comments: Option<String>,
    pub incorrect_comments: Option<String>,
    pub neutral_comments: Option<String>,
    pub answers: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Canvas QuizGroup shape (API_ALLOWED_QUIZ_GROUP_OUTPUT_FIELDS).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct QuizQuestionGroup {
    pub id: i64,
    pub quiz_id: i64,
    pub name: Option<String>,
    pub pick_count: i32,
    pub question_points: Option<f64>,
    pub assessment_question_bank_id: Option<i64>,
    pub position: i32,
}

/// Canvas wraps group responses in `{ "quiz_groups": [...] }`.
#[derive(Debug, Serialize)]
pub struct QuizGroupsEnvelope {
    pub quiz_groups: Vec<QuizQuestionGroup>,
}

/// Stored row; `answers` is the serialized JSON array.
#[derive(Debug, sqlx::FromRow)]
struct QuizQuestionRow {
    id: i64,
    quiz_id: i64,
    quiz_group_id: Option<i64>,
    assessment_question_bank_id: Option<i64>,
    position: i32,
    question_name: String,
    question_type: String,
    question_text: String,
    points_possible: f64,
    correct_comments: Option<String>,
    incorrect_comments: Option<String>,
    neutral_comments: Option<String>,
    answers: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl QuizQuestionRow {
    fn into_api(self) -> QuizQuestion {
        let answers = serde_json::from_str(&self.answers).unwrap_or_else(|_| json!([]));
        QuizQuestion {
            id: self.id,
            quiz_id: self.quiz_id,
            quiz_group_id: self.quiz_group_id,
            assessment_question_bank_id: self.assessment_question_bank_id,
            position: self.position,
            question_name: self.question_name,
            question_type: self.question_type,
            question_text: self.question_text,
            points_possible: self.points_possible,
            correct_comments: self.correct_comments,
            incorrect_comments: self.incorrect_comments,
            neutral_comments: self.neutral_comments,
            answers,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Canvas question create/update fields (`question[...]`).
#[derive(Debug, Default, Deserialize)]
pub struct QuizQuestionInput {
    pub question_name: Option<String>,
    pub question_type: Option<String>,
    pub question_text: Option<String>,
    pub position: Option<i32>,
    pub points_possible: Option<f64>,
    pub correct_comments: Option<String>,
    pub incorrect_comments: Option<String>,
    pub neutral_comments: Option<String>,
    pub answers: Option<Value>,
    pub quiz_group_id: Option<i64>,
    pub assessment_question_bank_id: Option<i64>,
}

/// Canvas group create/update fields (`quiz_groups[][...]`).
#[derive(Debug, Default, Deserialize)]
pub struct QuizGroupInput {
    pub name: Option<String>,
    pub pick_count: Option<i32>,
    pub question_points: Option<f64>,
    pub assessment_question_bank_id: Option<i64>,
    pub position: Option<i32>,
}

const QUESTION_COLUMNS: &str =
    "id, quiz_id, quiz_group_id, assessment_question_bank_id, position, \
     question_name, question_type, question_text, \
     points_possible::float8 AS points_possible, \
     correct_comments, incorrect_comments, neutral_comments, \
     answers, created_at, updated_at";

const GROUP_COLUMNS: &str = "id, quiz_id, name, pick_count, \
     question_points::float8 AS question_points, \
     assessment_question_bank_id, position";

/// Accept either Canvas' wrapped body (`{"question": {...}}`) or a flat object.
fn question_payload(body: Value) -> Result<QuizQuestionInput, ApiError> {
    let inner = match body.get("question") {
        Some(value) if value.is_object() => value.clone(),
        _ => body.clone(),
    };
    serde_json::from_value(inner)
        .map_err(|e| ApiError::BadRequest(format!("invalid question payload: {e}")))
}

/// Accept either Canvas' wrapped body (`{"quiz_groups": [{...}]}`) or a flat
/// object (the first element wins when an array is supplied).
fn group_payload(body: Value) -> Result<QuizGroupInput, ApiError> {
    if let Some(groups) = body.get("quiz_groups") {
        let first = groups
            .as_array()
            .and_then(|items| items.first())
            .cloned()
            .ok_or_else(|| ApiError::Validation("quiz_groups is required".into()))?;
        return serde_json::from_value(first)
            .map_err(|e| ApiError::BadRequest(format!("invalid quiz_groups payload: {e}")));
    }
    serde_json::from_value(body.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid quiz_groups payload: {e}")))
}

/// Validate a question bank reference (Canvas' find_bank → 404).
async fn require_question_bank(pool: &PgPool, bank_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM question_banks WHERE id = $1")
        .bind(bank_id)
        .fetch_optional(pool)
        .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// A group id outside the path quiz is not found (Canvas nested find).
async fn require_group_in_quiz(pool: &PgPool, group_id: i64, quiz_id: i64) -> Result<(), ApiError> {
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM quiz_question_groups WHERE id = $1 AND quiz_id = $2")
            .bind(group_id)
            .bind(quiz_id)
            .fetch_optional(pool)
            .await?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

/// Mirror Canvas positions: group questions append inside their group, root
/// questions/groups append after the quiz's root entries.
async fn next_question_position(
    pool: &PgPool,
    quiz_id: i64,
    group_id: Option<i64>,
) -> Result<i32, ApiError> {
    if let Some(group_id) = group_id {
        let (max,): (Option<i32>,) = sqlx::query_as(
            "SELECT MAX(position) FROM quiz_questions \
             WHERE quiz_id = $1 AND quiz_group_id = $2 AND workflow_state <> 'deleted'",
        )
        .bind(quiz_id)
        .bind(group_id)
        .fetch_one(pool)
        .await?;
        return Ok(max.unwrap_or(0) + 1);
    }

    let (max,): (i32,) = sqlx::query_as(
        r#"
        SELECT GREATEST(
            COALESCE((SELECT MAX(position) FROM quiz_questions
                      WHERE quiz_id = $1 AND quiz_group_id IS NULL
                        AND workflow_state <> 'deleted'), 0),
            COALESCE((SELECT MAX(position) FROM quiz_question_groups
                      WHERE quiz_id = $1), 0)
        )
        "#,
    )
    .bind(quiz_id)
    .fetch_one(pool)
    .await?;
    Ok(max + 1)
}

/// Serialize an answers payload, rejecting non-arrays.
fn answers_json(input: Option<&Value>, current: Option<&str>) -> Result<String, ApiError> {
    match input {
        Some(value) if value.is_array() => serde_json::to_string(value)
            .map_err(|e| ApiError::Validation(format!("invalid answers: {e}"))),
        Some(_) => Err(ApiError::Validation("answers must be an array".into())),
        None => Ok(current.unwrap_or("[]").to_string()),
    }
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
// Quiz question handlers (Canvas: all question CRUD requires :update)
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/quizzes/{quizId}/questions — Canvas list (array).
async fn list_quiz_questions(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<QuizQuestion>>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<QuizQuestionRow> = sqlx::query_as(&format!(
        "SELECT {} FROM quiz_questions
         WHERE quiz_id = $1 AND workflow_state <> 'deleted'
         ORDER BY position, id",
        QUESTION_COLUMNS
    ))
    .bind(quiz_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(
        rows.into_iter().map(QuizQuestionRow::into_api).collect(),
    ))
}

/// GET /courses/{courseId}/quizzes/{quizId}/questions/{id}
async fn get_quiz_question(
    State(state): State<AppState>,
    Path((course_id, quiz_id, question_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<QuizQuestion>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<QuizQuestionRow> = sqlx::query_as(&format!(
        "SELECT {} FROM quiz_questions
         WHERE id = $1 AND quiz_id = $2 AND workflow_state <> 'deleted'",
        QUESTION_COLUMNS
    ))
    .bind(question_id)
    .bind(quiz_id)
    .fetch_optional(&state.db)
    .await?;

    Ok(Json(row.ok_or(ApiError::NotFound)?.into_api()))
}

/// POST /courses/{courseId}/quizzes/{quizId}/questions (Canvas returns 200).
async fn create_quiz_question(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<QuizQuestion>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let input = question_payload(body)?;

    if let Some(group_id) = input.quiz_group_id {
        require_group_in_quiz(&state.db, group_id, quiz_id).await?;
    }
    if let Some(bank_id) = input.assessment_question_bank_id {
        require_question_bank(&state.db, bank_id).await?;
    }

    let answers = answers_json(input.answers.as_ref(), None)?;
    let position = match input.position {
        Some(position) => position,
        None => next_question_position(&state.db, quiz_id, input.quiz_group_id).await?,
    };

    let row: QuizQuestionRow = sqlx::query_as(&format!(
        "INSERT INTO quiz_questions
             (quiz_id, quiz_group_id, assessment_question_bank_id, position,
              question_name, question_type, question_text, points_possible,
              correct_comments, incorrect_comments, neutral_comments, answers)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
         RETURNING {}",
        QUESTION_COLUMNS
    ))
    .bind(quiz_id)
    .bind(input.quiz_group_id)
    .bind(input.assessment_question_bank_id)
    .bind(position)
    .bind(input.question_name.as_deref().unwrap_or("Question"))
    .bind(
        input
            .question_type
            .as_deref()
            .unwrap_or("text_only_question"),
    )
    .bind(input.question_text.as_deref().unwrap_or(""))
    .bind(input.points_possible.unwrap_or(0.0))
    .bind(&input.correct_comments)
    .bind(&input.incorrect_comments)
    .bind(&input.neutral_comments)
    .bind(&answers)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(row.into_api()))
}

/// PUT /courses/{courseId}/quizzes/{quizId}/questions/{id}
async fn update_quiz_question(
    State(state): State<AppState>,
    Path((course_id, quiz_id, question_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<QuizQuestion>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let input = question_payload(body)?;

    let current: Option<QuizQuestionRow> = sqlx::query_as(&format!(
        "SELECT {} FROM quiz_questions
         WHERE id = $1 AND quiz_id = $2 AND workflow_state <> 'deleted'",
        QUESTION_COLUMNS
    ))
    .bind(question_id)
    .bind(quiz_id)
    .fetch_optional(&state.db)
    .await?;

    let current = current.ok_or(ApiError::NotFound)?;

    if let Some(bank_id) = input.assessment_question_bank_id {
        require_question_bank(&state.db, bank_id).await?;
    }
    let group_id = match input.quiz_group_id {
        Some(group_id) => {
            require_group_in_quiz(&state.db, group_id, quiz_id).await?;
            Some(group_id)
        }
        None => current.quiz_group_id,
    };

    let answers = answers_json(input.answers.as_ref(), Some(&current.answers))?;
    // Canvas moves a question to the end of the target group when reassigned.
    let position = if group_id != current.quiz_group_id {
        next_question_position(&state.db, quiz_id, group_id).await?
    } else {
        input.position.unwrap_or(current.position)
    };

    let question_name = input.question_name.unwrap_or(current.question_name);
    let question_type = input.question_type.unwrap_or(current.question_type);
    let question_text = input.question_text.unwrap_or(current.question_text);
    let points_possible = input.points_possible.unwrap_or(current.points_possible);
    let correct_comments = input.correct_comments.or(current.correct_comments);
    let incorrect_comments = input.incorrect_comments.or(current.incorrect_comments);
    let neutral_comments = input.neutral_comments.or(current.neutral_comments);
    let bank_id = input
        .assessment_question_bank_id
        .or(current.assessment_question_bank_id);

    let row: QuizQuestionRow = sqlx::query_as(&format!(
        "UPDATE quiz_questions
         SET quiz_group_id = $2, assessment_question_bank_id = $3, position = $4,
             question_name = $5, question_type = $6, question_text = $7,
             points_possible = $8, correct_comments = $9, incorrect_comments = $10,
             neutral_comments = $11, answers = $12, updated_at = now()
         WHERE id = $1 AND quiz_id = $13 AND workflow_state <> 'deleted'
         RETURNING {}",
        QUESTION_COLUMNS
    ))
    .bind(question_id)
    .bind(group_id)
    .bind(bank_id)
    .bind(position)
    .bind(&question_name)
    .bind(&question_type)
    .bind(&question_text)
    .bind(points_possible)
    .bind(&correct_comments)
    .bind(&incorrect_comments)
    .bind(&neutral_comments)
    .bind(&answers)
    .bind(quiz_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(row.into_api()))
}

/// DELETE /courses/{courseId}/quizzes/{quizId}/questions/{id} → 204
///
/// Canvas soft-deletes questions (`workflow_state='deleted'`); mirror that so
/// the row disappears from every endpoint but is kept for history.
async fn delete_quiz_question(
    State(state): State<AppState>,
    Path((course_id, quiz_id, question_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        "UPDATE quiz_questions
         SET workflow_state = 'deleted', updated_at = now()
         WHERE id = $1 AND quiz_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(question_id)
    .bind(quiz_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Quiz question group handlers
// (read is enrollment-level; mutations require :update)
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/quizzes/{quizId}/groups
async fn list_quiz_groups(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<QuizGroupsEnvelope>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let quiz_groups: Vec<QuizQuestionGroup> = sqlx::query_as(&format!(
        "SELECT {} FROM quiz_question_groups WHERE quiz_id = $1 ORDER BY position, id",
        GROUP_COLUMNS
    ))
    .bind(quiz_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(QuizGroupsEnvelope { quiz_groups }))
}

/// GET /courses/{courseId}/quizzes/{quizId}/groups/{id}
async fn get_quiz_group(
    State(state): State<AppState>,
    Path((course_id, quiz_id, group_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<QuizQuestionGroup>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let group: Option<QuizQuestionGroup> = sqlx::query_as(&format!(
        "SELECT {} FROM quiz_question_groups WHERE id = $1 AND quiz_id = $2",
        GROUP_COLUMNS
    ))
    .bind(group_id)
    .bind(quiz_id)
    .fetch_optional(&state.db)
    .await?;

    Ok(Json(group.ok_or(ApiError::NotFound)?))
}

/// POST /courses/{courseId}/quizzes/{quizId}/groups → 201, wrapped response.
async fn create_quiz_group(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<QuizGroupsEnvelope>), ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let input = group_payload(body)?;

    if let Some(bank_id) = input.assessment_question_bank_id {
        require_question_bank(&state.db, bank_id).await?;
    }

    // Canvas appends groups after the quiz's root entries.
    let position = match input.position {
        Some(position) => position,
        None => next_question_position(&state.db, quiz_id, None).await?,
    };

    let group: QuizQuestionGroup = sqlx::query_as(&format!(
        "INSERT INTO quiz_question_groups
             (quiz_id, name, pick_count, question_points,
              assessment_question_bank_id, position)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING {}",
        GROUP_COLUMNS
    ))
    .bind(quiz_id)
    .bind(&input.name)
    .bind(input.pick_count.unwrap_or(0))
    .bind(input.question_points)
    .bind(input.assessment_question_bank_id)
    .bind(position)
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(QuizGroupsEnvelope {
            quiz_groups: vec![group],
        }),
    ))
}

/// PUT /courses/{courseId}/quizzes/{quizId}/groups/{id}
async fn update_quiz_group(
    State(state): State<AppState>,
    Path((course_id, quiz_id, group_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<QuizGroupsEnvelope>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let input = group_payload(body)?;

    let current: Option<QuizQuestionGroup> = sqlx::query_as(&format!(
        "SELECT {} FROM quiz_question_groups WHERE id = $1 AND quiz_id = $2",
        GROUP_COLUMNS
    ))
    .bind(group_id)
    .bind(quiz_id)
    .fetch_optional(&state.db)
    .await?;

    let current = current.ok_or(ApiError::NotFound)?;

    // Canvas only permits name/pick_count/question_points on update.
    let name = input.name.or(current.name);
    let pick_count = input.pick_count.unwrap_or(current.pick_count);
    let question_points = input.question_points.or(current.question_points);

    let group: QuizQuestionGroup = sqlx::query_as(&format!(
        "UPDATE quiz_question_groups
         SET name = $2, pick_count = $3, question_points = $4, updated_at = now()
         WHERE id = $1 AND quiz_id = $5
         RETURNING {}",
        GROUP_COLUMNS
    ))
    .bind(group_id)
    .bind(&name)
    .bind(pick_count)
    .bind(question_points)
    .bind(quiz_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(QuizGroupsEnvelope {
        quiz_groups: vec![group],
    }))
}

/// DELETE /courses/{courseId}/quizzes/{quizId}/groups/{id} → 204
///
/// Canvas destroys the group and its questions; the FK cascade does the same.
async fn delete_quiz_group(
    State(state): State<AppState>,
    Path((course_id, quiz_id, group_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query("DELETE FROM quiz_question_groups WHERE id = $1 AND quiz_id = $2")
        .bind(group_id)
        .bind(quiz_id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
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
        // Quiz questions CRUD (Canvas quizzes/quiz_questions)
        .route(
            "/courses/{courseId}/quizzes/{quizId}/questions",
            get(list_quiz_questions),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/questions",
            post(create_quiz_question),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/questions/{questionId}",
            get(get_quiz_question),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/questions/{questionId}",
            put(update_quiz_question),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/questions/{questionId}",
            delete(delete_quiz_question),
        )
        // Quiz question groups CRUD (Canvas quizzes/quiz_groups)
        .route(
            "/courses/{courseId}/quizzes/{quizId}/groups",
            get(list_quiz_groups),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/groups",
            post(create_quiz_group),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/groups/{groupId}",
            get(get_quiz_group),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/groups/{groupId}",
            put(update_quiz_group),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/groups/{groupId}",
            delete(delete_quiz_group),
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
