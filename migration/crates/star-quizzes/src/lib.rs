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
//! - Phase 3 additions mirror Canvas `Quizzes::QuizSubmissionsApiController`
//!   (show/update) and `Quizzes::QuizReportsController` (index/create):
//!   GET/PUT `.../quizzes/:quiz_id/submissions/:id` (Canvas wraps the payload
//!   in `{"quiz_submissions": [...]}`; PUT is teacher-only manual scoring) and
//!   GET/POST `.../quizzes/:quiz_id/reports` returning the "QuizReport" shape
//!   (index is a bare array, create a single object).

use axum::{
    extract::{Path, Query, State},
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
use std::collections::HashMap;

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
// Quiz submissions API
// (Canvas Quizzes::QuizSubmissionsApiController — show / update)
// ---------------------------------------------------------------------------

/// Stored quiz submission plus the quiz fields the Canvas serializer exposes.
/// `questions` manual scores land in `submission_data` (Canvas keeps the
/// scored answer payload there, serialized as text).
#[derive(Debug, sqlx::FromRow)]
struct QuizSubmissionRow {
    id: i64,
    quiz_id: i64,
    user_id: i64,
    submission_id: Option<i64>,
    quiz_points_possible: f64,
    attempt: i32,
    extra_attempts: Option<i32>,
    extra_time: Option<i32>,
    manually_unlocked: bool,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
    fudge_points: Option<f64>,
    kept_score: Option<f64>,
    score: Option<f64>,
    score_before_regrade: Option<f64>,
    has_seen_results: bool,
    workflow_state: String,
    allowed_attempts: i32,
    submission_data: String,
}

const SUBMISSION_COLUMNS: &str = "qs.id, qs.quiz_id, qs.user_id, qs.submission_id, \
     q.points_possible::float8 AS quiz_points_possible, \
     qs.attempt, qs.extra_attempts, qs.extra_time, qs.manually_unlocked, \
     qs.started_at, qs.finished_at, qs.end_at, qs.completed_at, \
     qs.fudge_points::float8 AS fudge_points, \
     qs.kept_score::float8 AS kept_score, qs.score::float8 AS score, \
     qs.score_before_regrade::float8 AS score_before_regrade, \
     qs.has_seen_results, qs.workflow_state, qs.submission_data, \
     q.allowed_attempts";

impl QuizSubmissionRow {
    /// Canvas `completed?`: complete or pending review. The legacy
    /// `completed_at` column also marks a submission as turned in.
    fn completed(&self) -> bool {
        matches!(self.workflow_state.as_str(), "complete" | "pending_review")
            || self.completed_at.is_some()
    }

    /// Canvas `finished_at`; the pre-Phase-3 column was `completed_at`.
    fn finished_at(&self) -> Option<DateTime<Utc>> {
        self.finished_at.or(self.completed_at)
    }

    /// Canvas `time_spent`: seconds, including granted extra time.
    fn time_spent(&self) -> Option<i64> {
        self.finished_at().map(|finished| {
            (finished - self.started_at).num_seconds() + i64::from(self.extra_time.unwrap_or(0))
        })
    }

    /// Canvas `attempts_left` (-1 means unlimited attempts).
    fn attempts_left(&self) -> i32 {
        if self.allowed_attempts < 0 {
            return -1;
        }
        (self.allowed_attempts - self.attempt + self.extra_attempts.unwrap_or(0)).max(0)
    }

    /// Canvas `overdue_and_needs_submission?` (timer-autosubmit nuance aside).
    fn overdue_and_needs_submission(&self) -> bool {
        self.workflow_state == "untaken" && self.end_at.is_some_and(|end_at| end_at < Utc::now())
    }

    /// Canvas `quiz_submission_json` (fields Canvas emits, nulls omitted).
    fn to_json(&self, course_id: i64) -> Value {
        let mut out = json!({
            "id": self.id,
            "quiz_id": self.quiz_id,
            "user_id": self.user_id,
            "quiz_points_possible": self.quiz_points_possible,
            "attempt": self.attempt,
            "manually_unlocked": self.manually_unlocked,
            "started_at": self.started_at,
            "has_seen_results": self.has_seen_results,
            "workflow_state": self.workflow_state,
            "attempts_left": self.attempts_left(),
            "overdue_and_needs_submission": self.overdue_and_needs_submission(),
            "html_url": format!(
                "/courses/{course_id}/quizzes/{}/submissions/{}",
                self.quiz_id, self.id
            ),
        });

        if let Some(object) = out.as_object_mut() {
            if let Some(value) = self.submission_id {
                object.insert("submission_id".into(), json!(value));
            }
            if let Some(value) = self.extra_attempts {
                object.insert("extra_attempts".into(), json!(value));
            }
            if let Some(value) = self.extra_time {
                object.insert("extra_time".into(), json!(value));
            }
            if let Some(value) = self.finished_at() {
                object.insert("finished_at".into(), json!(value));
            }
            if let Some(value) = self.end_at {
                object.insert("end_at".into(), json!(value));
            }
            if let Some(value) = self.fudge_points {
                object.insert("fudge_points".into(), json!(value));
            }
            if let Some(value) = self.kept_score {
                object.insert("kept_score".into(), json!(value));
            }
            if let Some(value) = self.score {
                object.insert("score".into(), json!(value));
            }
            if let Some(value) = self.score_before_regrade {
                object.insert("score_before_regrade".into(), json!(value));
            }
            if let Some(value) = self.time_spent() {
                object.insert("time_spent".into(), json!(value));
            }
        }

        out
    }
}

/// Canvas always wraps quiz submissions in `{"quiz_submissions": [...]}`.
fn submissions_envelope(rows: &[QuizSubmissionRow], course_id: i64) -> Value {
    json!({
        "quiz_submissions": rows
            .iter()
            .map(|row| row.to_json(course_id))
            .collect::<Vec<_>>(),
    })
}

/// Load a submission by (quiz, id); an id outside the path quiz is not found.
async fn load_quiz_submission(
    pool: &PgPool,
    quiz_id: i64,
    submission_id: i64,
) -> Result<Option<QuizSubmissionRow>, ApiError> {
    let sql = format!(
        "SELECT {SUBMISSION_COLUMNS} FROM quiz_submissions qs \
         JOIN quizzes q ON q.id = qs.quiz_id \
         WHERE qs.id = $1 AND qs.quiz_id = $2"
    );
    Ok(sqlx::query_as(&sql)
        .bind(submission_id)
        .bind(quiz_id)
        .fetch_optional(pool)
        .await?)
}

/// Canvas `quiz_submissions` manual-scoring payload (`PUT .../submissions/:id`).
/// Everything is kept as raw JSON because Canvas accepts both numbers and
/// numeric strings in the form-encoded payload.
#[derive(Debug, Default, Deserialize)]
struct QuizSubmissionScoreInput {
    #[serde(default)]
    attempt: Option<Value>,
    #[serde(default)]
    fudge_points: Option<Value>,
    #[serde(default)]
    questions: Option<Value>,
}

/// Canvas requires a `quiz_submissions` array; the first element is scored.
fn score_payload(body: &Value) -> Result<QuizSubmissionScoreInput, ApiError> {
    let first = body
        .get("quiz_submissions")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .ok_or_else(|| ApiError::BadRequest("missing required key :quiz_submissions".into()))?;
    serde_json::from_value(first.clone())
        .map_err(|e| ApiError::BadRequest(format!("invalid quiz_submissions payload: {e}")))
}

fn value_to_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Parse an optional points value; empty strings mean "no change" (Canvas
/// treats blank form values as absent).
fn value_to_f64(value: &Value) -> Result<Option<f64>, ApiError> {
    match value {
        Value::Null => Ok(None),
        Value::Number(number) => number
            .as_f64()
            .map(Some)
            .ok_or_else(|| ApiError::BadRequest("points must be a number".into())),
        Value::String(text) if text.trim().is_empty() => Ok(None),
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map(Some)
            .map_err(|_| ApiError::BadRequest("points must be a number".into())),
        _ => Err(ApiError::BadRequest("points must be a number".into())),
    }
}

/// Validate and normalize the per-question `score` / `comment` map.
fn question_scores(input: Option<&Value>) -> Result<Option<Value>, ApiError> {
    let Some(input) = input else {
        return Ok(None);
    };
    let Value::Object(entries) = input else {
        return Err(ApiError::BadRequest(
            "questions must be an object keyed by question id".into(),
        ));
    };

    let mut normalized = serde_json::Map::new();
    for (question_id, data) in entries {
        let Value::Object(fields) = data else {
            return Err(ApiError::BadRequest(
                "invalid question scoring payload".into(),
            ));
        };
        let mut entry = serde_json::Map::new();
        if let Some(score) = fields.get("score") {
            // Blank/null scores mean "no change" in Canvas' form payloads.
            if let Some(points) = value_to_f64(score)? {
                entry.insert("score".into(), json!(points));
            }
        }
        if let Some(comment) = fields.get("comment") {
            entry.insert(
                "comment".into(),
                json!(comment.as_str().unwrap_or_default()),
            );
        }
        normalized.insert(question_id.clone(), Value::Object(entry));
    }

    Ok(Some(Value::Object(normalized)))
}

/// Merge per-question scores into the stored `submission_data` object.
fn merge_submission_data(current: &str, scores: Value) -> String {
    let mut root = serde_json::from_str::<Value>(current)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut questions = root
        .get("questions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Value::Object(scores) = scores {
        for (question_id, entry) in scores {
            questions.insert(question_id, entry);
        }
    }
    root.insert("questions".to_string(), Value::Object(questions));
    serde_json::to_string(&Value::Object(root)).unwrap_or_else(|_| "{}".to_string())
}

/// GET /courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}
///
/// Canvas `show`: grade managers read any submission; a student reads only
/// their own (and must still be enrolled).
async fn get_quiz_submission(
    State(state): State<AppState>,
    Path((course_id, quiz_id, submission_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    let row = load_quiz_submission(&state.db, quiz_id, submission_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let can_manage = check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;
    let is_own = row.user_id == auth.id
        && check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await?;
    if !can_manage && !is_own {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(submissions_envelope(&[row], course_id)))
}

/// PUT /courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}
///
/// Canvas `update` (manual scoring): teacher/admin only. The attempt must be
/// supplied, match the stored attempt and already be complete — otherwise 400,
/// mirroring `QuizSubmissionService#update_scores`.
async fn update_quiz_submission(
    State(state): State<AppState>,
    Path((course_id, quiz_id, submission_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let input = score_payload(&body)?;
    let current = load_quiz_submission(&state.db, quiz_id, submission_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let attempt = input
        .attempt
        .as_ref()
        .and_then(value_to_i64)
        .ok_or_else(|| ApiError::BadRequest("invalid attempt".into()))?;
    if attempt != i64::from(current.attempt) {
        return Err(ApiError::BadRequest("invalid attempt".into()));
    }
    if !current.completed() {
        return Err(ApiError::BadRequest(
            "quiz submission attempt must be complete".into(),
        ));
    }

    let scores = question_scores(input.questions.as_ref())?;
    let fudge = match input.fudge_points.as_ref() {
        Some(value) => value_to_f64(value)?,
        None => None,
    };

    let updated = match (fudge, scores) {
        (Some(fudge_points), scores) => {
            // Canvas tallies fudge points on top of the graded question total.
            let base_score = current.score.unwrap_or(0.0) - current.fudge_points.unwrap_or(0.0);
            let score = base_score + fudge_points;
            let submission_data = match scores {
                Some(scores) => merge_submission_data(&current.submission_data, scores),
                None => current.submission_data.clone(),
            };
            sqlx::query(
                "UPDATE quiz_submissions \
                 SET fudge_points = $2, score = $3, kept_score = $3, \
                     submission_data = $4, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(current.id)
            .bind(fudge_points)
            .bind(score)
            .bind(&submission_data)
            .execute(&state.db)
            .await?;
            true
        }
        (None, Some(scores)) => {
            let submission_data = merge_submission_data(&current.submission_data, scores);
            sqlx::query(
                "UPDATE quiz_submissions \
                 SET submission_data = $2, updated_at = now() \
                 WHERE id = $1",
            )
            .bind(current.id)
            .bind(&submission_data)
            .execute(&state.db)
            .await?;
            true
        }
        (None, None) => false,
    };

    let row = if updated {
        load_quiz_submission(&state.db, quiz_id, submission_id)
            .await?
            .ok_or(ApiError::NotFound)?
    } else {
        current
    };

    Ok(Json(submissions_envelope(&[row], course_id)))
}

// ---------------------------------------------------------------------------
// Quiz reports (Canvas Quizzes::QuizReportsController — index / create)
// ---------------------------------------------------------------------------

/// Canvas `Quizzes::QuizStatistics::REPORTS`.
const QUIZ_REPORT_TYPES: [(&str, &str); 2] = [
    ("student_analysis", "Student Analysis"),
    ("item_analysis", "Item Analysis"),
];

/// Quiz fields the report payload needs (Canvas' `QuizReport` embeds `quiz_id`).
#[derive(Debug, sqlx::FromRow)]
struct QuizReportContext {
    id: i64,
    course_id: i64,
    quiz_type: String,
    /// max(quiz.created_at, latest submission update) — a report older than
    /// this is stale and gets regenerated by Canvas.
    last_activity: DateTime<Utc>,
}

/// Stored row of Canvas' `quiz_statistics` table (API name: QuizReport).
#[derive(Debug, Clone, sqlx::FromRow)]
struct QuizReportRow {
    id: i64,
    quiz_id: i64,
    report_type: String,
    includes_all_versions: bool,
    includes_sis_ids: bool,
    anonymous: bool,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

const QUIZ_REPORT_COLUMNS: &str = "id, quiz_id, report_type, includes_all_versions, \
     includes_sis_ids, anonymous, created_at, updated_at";

fn readable_report_type(report_type: &str) -> &'static str {
    QUIZ_REPORT_TYPES
        .iter()
        .find(|(name, _)| *name == report_type)
        .map(|(_, readable)| *readable)
        .unwrap_or("Student Analysis")
}

/// Canvas: item analysis cannot be generated for surveys, everything else can.
fn report_generatable(quiz_type: &str, report_type: &str) -> bool {
    if report_type == "item_analysis" {
        !matches!(quiz_type, "survey" | "graded_survey")
    } else {
        true
    }
}

/// Canvas `current_statistics_for`: reuse the newest matching report while it
/// is still current, otherwise persist a freshly generated one.
async fn ensure_quiz_report(
    pool: &PgPool,
    quiz: &QuizReportContext,
    report_type: &str,
    includes_all_versions: bool,
) -> Result<QuizReportRow, ApiError> {
    // item_analysis always considers all versions and never SIS ids.
    let (all_versions, sis_ids) = if report_type == "item_analysis" {
        (true, false)
    } else {
        (includes_all_versions, false)
    };

    let existing: Option<QuizReportRow> = sqlx::query_as(&format!(
        "SELECT {QUIZ_REPORT_COLUMNS} FROM quiz_statistics \
         WHERE quiz_id = $1 AND report_type = $2 \
           AND includes_all_versions = $3 AND includes_sis_ids = $4 \
         ORDER BY created_at DESC, id DESC LIMIT 1"
    ))
    .bind(quiz.id)
    .bind(report_type)
    .bind(all_versions)
    .bind(sis_ids)
    .fetch_optional(pool)
    .await?;

    if let Some(report) = existing {
        if report.created_at >= quiz.last_activity {
            return Ok(report);
        }
    }

    let report: QuizReportRow = sqlx::query_as(&format!(
        "INSERT INTO quiz_statistics \
             (quiz_id, report_type, includes_all_versions, includes_sis_ids, anonymous) \
         VALUES ($1, $2, $3, $4, false) \
         RETURNING {QUIZ_REPORT_COLUMNS}"
    ))
    .bind(quiz.id)
    .bind(report_type)
    .bind(all_versions)
    .bind(sis_ids)
    .fetch_one(pool)
    .await?;

    Ok(report)
}

async fn load_report_context(pool: &PgPool, quiz_id: i64) -> Result<QuizReportContext, ApiError> {
    let quiz: QuizReportContext = sqlx::query_as(
        "SELECT q.id, q.course_id, q.quiz_type, \
                GREATEST(q.created_at, \
                    COALESCE((SELECT MAX(qs.updated_at) FROM quiz_submissions qs \
                              WHERE qs.quiz_id = q.id), q.created_at)) AS last_activity \
         FROM quizzes q WHERE q.id = $1",
    )
    .bind(quiz_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)?;
    Ok(quiz)
}

/// Canvas `QuizReportSerializer` (non-JSON-API shape).
fn report_json(report: &QuizReportRow, quiz: &QuizReportContext) -> Value {
    json!({
        "id": report.id,
        "quiz_id": report.quiz_id,
        "report_type": report.report_type,
        "readable_type": readable_report_type(&report.report_type),
        "includes_all_versions": report.includes_all_versions,
        "includes_sis_ids": report.includes_sis_ids,
        "generatable": report_generatable(&quiz.quiz_type, &report.report_type),
        "anonymous": report.anonymous,
        "url": format!(
            "/api/v1/courses/{}/quizzes/{}/reports/{}",
            quiz.course_id, quiz.id, report.id
        ),
        "created_at": report.created_at,
        "updated_at": report.updated_at,
    })
}

fn query_bool(params: &HashMap<String, String>, key: &str) -> bool {
    params
        .get(key)
        .map(|value| matches!(value.as_str(), "true" | "1" | "on" | "yes"))
        .unwrap_or(false)
}

fn json_bool(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_i64().is_some_and(|n| n != 0),
        Some(Value::String(text)) => matches!(text.as_str(), "true" | "1" | "on" | "yes"),
        _ => false,
    }
}

/// Accept Canvas' `quiz_report{...}` object, a `quiz_reports` array (JSON-API)
/// or a flat body.
fn report_payload(body: &Value) -> Value {
    if let Some(report) = body.get("quiz_report").filter(|value| value.is_object()) {
        return report.clone();
    }
    if let Some(first) = body
        .get("quiz_reports")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    {
        return first.clone();
    }
    body.clone()
}

/// GET /courses/{courseId}/quizzes/{quizId}/reports
///
/// Canvas `index`: one report per known type, generated (and persisted) when
/// stale or missing. Returns a bare JSON array like Canvas.
async fn list_quiz_reports(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let quiz = load_report_context(&state.db, quiz_id).await?;
    let all_versions = query_bool(&params, "includes_all_versions");

    let mut reports = Vec::with_capacity(QUIZ_REPORT_TYPES.len());
    for (report_type, _) in QUIZ_REPORT_TYPES {
        let report = ensure_quiz_report(&state.db, &quiz, report_type, all_versions).await?;
        reports.push(report_json(&report, &quiz));
    }

    Ok(Json(Value::Array(reports)))
}

/// POST /courses/{courseId}/quizzes/{quizId}/reports
///
/// Canvas `create`: 400 for an unknown report type, otherwise 200 with the
/// report (an existing current report is returned instead of a duplicate).
async fn create_quiz_report(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let payload = report_payload(&body);
    let report_type = payload
        .get("report_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !QUIZ_REPORT_TYPES
        .iter()
        .any(|(known, _)| *known == report_type)
    {
        return Err(ApiError::BadRequest("invalid quiz report type".into()));
    }

    let quiz = load_report_context(&state.db, quiz_id).await?;
    let all_versions = json_bool(payload.get("includes_all_versions"));
    let report = ensure_quiz_report(&state.db, &quiz, report_type, all_versions).await?;

    Ok(Json(report_json(&report, &quiz)))
}

// ---------------------------------------------------------------------------
// Quiz statistics (Canvas Quizzes::QuizStatisticsController — index)
// ---------------------------------------------------------------------------

/// Canvas QuizStatistics aggregate shape (simplified from Canvas' full stats).
#[derive(Debug, Clone, Serialize)]
pub struct QuizStatistics {
    pub id: i64,
    pub quiz_id: i64,
    pub multiple_attempts_exist: bool,
    pub includes_all_versions: bool,
    pub generated_at: DateTime<Utc>,
    pub url: String,
    pub html_url: String,
    pub question_statistics: Value,
    pub submission_statistics: Value,
}

/// Load aggregate submission stats from completed quiz_submissions.
async fn load_submission_stats(
    pool: &PgPool,
    quiz_id: i64,
    include_all_versions: bool,
) -> Result<Value, ApiError> {
    let where_all = if include_all_versions {
        "".to_string()
    } else {
        format!(
            " AND id IN (SELECT MAX(id) FROM quiz_submissions \
             WHERE quiz_id = $1 AND workflow_state IN ('complete', 'pending_review') \
             GROUP BY user_id)"
        )
    };

    let row: Option<(i64, Option<f64>, Option<f64>, Option<f64>, Option<f64>)> =
        sqlx::query_as(&format!(
            r#"
            SELECT
                COUNT(DISTINCT user_id)::bigint AS unique_count,
                AVG(score)::float8 AS score_average,
                MAX(score)::float8 AS score_high,
                MIN(score)::float8 AS score_low,
                STDDEV(score)::float8 AS score_stdev
            FROM quiz_submissions
            WHERE quiz_id = $1 AND score IS NOT NULL
              AND workflow_state IN ('complete', 'pending_review')
            {where_all}
            "#,
        ))
        .bind(quiz_id)
        .fetch_optional(pool)
        .await?;

    let (unique_count, score_avg, score_high, score_low, score_stdev) =
        row.unwrap_or((0, None, None, None, None));

    Ok(json!({
        "unique_count": unique_count,
        "score_average": score_avg,
        "score_high": score_high,
        "score_low": score_low,
        "score_stdev": score_stdev,
        "scores": {},
    }))
}

/// GET /courses/{courseId}/quizzes/{quizId}/statistics
///
/// Canvas `index`: returns aggregate quiz statistics. Teacher/admin only.
async fn get_quiz_statistics(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    Query(params): Query<HashMap<String, String>>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let quiz: Option<(String,)> =
        sqlx::query_as("SELECT quiz_type FROM quizzes WHERE id = $1")
            .bind(quiz_id)
            .fetch_optional(&state.db)
            .await?;
    let (quiz_type,) = quiz.ok_or(ApiError::NotFound)?;

    let all_versions = query_bool(&params, "all_versions");

    // question_statistics: empty array as we don't store per-question aggregates
    let question_stats: Value = json!([]);

    let submission_stats =
        load_submission_stats(&state.db, quiz_id, all_versions).await?;

    let stats = json!({
        "quiz_statistics": [{
            "id": quiz_id,
            "quiz_id": quiz_id,
            "multiple_attempts_exist": false,
            "includes_all_versions": all_versions,
            "generated_at": chrono::Utc::now(),
            "url": format!("/api/v1/courses/{}/quizzes/{}/statistics", course_id, quiz_id),
            "html_url": format!("/courses/{}/quizzes/{}/statistics", course_id, quiz_id),
            "question_statistics": question_stats,
            "submission_statistics": submission_stats,
        }]
    });

    Ok(Json(stats))
}

// ---------------------------------------------------------------------------
// Quiz reorder & access code validation (Canvas quizzes_controller)
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/quizzes/{quizId}/reorder
///
/// Canvas `reorder`: teacher/admin only. Accepts an `order` array of
/// {type: "question"|"group", id, position}. We persist the order to the
/// `position` column of quiz_questions and quiz_question_groups.
async fn reorder_quiz(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(body): Json<Value>,
) -> Result<StatusCode, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let order = body
        .get("order")
        .and_then(Value::as_array)
        .ok_or_else(|| ApiError::BadRequest("order array is required".into()))?;

    for (idx, item) in order.iter().enumerate() {
        let item_type = item.get("type").and_then(Value::as_str);
        let id = item.get("id").and_then(|v| v.as_i64()).ok_or_else(|| {
            ApiError::BadRequest("each order item needs {type, id}".into())
        })?;
        let position = (idx + 1) as i32;

        match item_type {
            Some("question") => {
                sqlx::query("UPDATE quiz_questions SET position = $2 WHERE id = $1 AND quiz_id = $3")
                    .bind(id)
                    .bind(position)
                    .bind(quiz_id)
                    .execute(&state.db)
                    .await?;
            }
            Some("group") => {
                sqlx::query("UPDATE quiz_question_groups SET position = $2 WHERE id = $1 AND quiz_id = $3")
                    .bind(id)
                    .bind(position)
                    .bind(quiz_id)
                    .execute(&state.db)
                    .await?;
            }
            _ => return Err(ApiError::BadRequest("type must be 'question' or 'group'".into())),
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /courses/{courseId}/quizzes/{quizId}/validate_access_code
///
/// Canvas `validate_access_code`: any enrolled user can call it.
/// Returns a plain boolean `true` if the code matches the quiz's access_code.
#[derive(Debug, Deserialize)]
struct AccessCodeRequest {
    access_code: String,
}

async fn validate_access_code(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<AccessCodeRequest>,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT access_code FROM quizzes WHERE id = $1")
            .bind(quiz_id)
            .fetch_optional(&state.db)
            .await?;
    let (access_code,) = row.ok_or(ApiError::NotFound)?;

    let correct = access_code
        .as_ref()
        .is_some_and(|code| code == &payload.access_code);

    Ok(Json(json!(correct)))
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
        // Quiz submissions API (Canvas quiz_submissions_api show/update)
        .route(
            "/courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}",
            get(get_quiz_submission),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}",
            put(update_quiz_submission),
        )
        // Quiz reports (Canvas quiz_reports index/create)
        .route(
            "/courses/{courseId}/quizzes/{quizId}/reports",
            get(list_quiz_reports),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/reports",
            post(create_quiz_report),
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
        // Quiz reorder (Canvas quizzes_controller#reorder)
        .route(
            "/courses/{courseId}/quizzes/{quizId}/reorder",
            post(reorder_quiz),
        )
        // Quiz access code validation (Canvas quizzes_controller#validate_access_code)
        .route(
            "/courses/{courseId}/quizzes/{quizId}/validate_access_code",
            post(validate_access_code),
        )
        // Quiz statistics (Canvas quiz_statistics_controller#index)
        .route(
            "/courses/{courseId}/quizzes/{quizId}/statistics",
            get(get_quiz_statistics),
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
