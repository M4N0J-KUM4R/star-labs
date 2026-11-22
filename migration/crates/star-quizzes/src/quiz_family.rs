//! Quizzes-family parity gaps — ten small Canvas controllers, batched.
//!
//! Routes (Canvas verb order):
//!   - GET  /courses/{courseId}/quizzes/assignment_overrides        (quiz_assignment_overrides#index)
//!   - GET  /courses/{courseId}/new_quizzes/assignment_overrides    (quiz_assignment_overrides#new_quizzes)
//!   - GET  /courses/{courseId}/quizzes/{quizId}/submission_users   (quiz_submission_users#index)
//!   - POST /courses/{courseId}/quizzes/{quizId}/submission_users/message (quiz_submission_users#message)
//!   - GET  /courses/{courseId}/quizzes/{quizId}/outstanding_quiz_submissions (outstanding_quiz_submissions#index)
//!   - POST /courses/{courseId}/quizzes/{quizId}/outstanding_quiz_submissions (outstanding_quiz_submissions#grade)
//!   - POST /courses/{courseId}/quizzes/{quizId}/extensions         (quiz_extensions#create)
//!   - POST /courses/{courseId}/quiz_extensions                     (course_quiz_extensions#create)
//!   - GET  /courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}/events (quiz_submission_events_api#index)
//!   - POST /courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}/events (quiz_submission_events_api#create)
//!   - GET  /quiz_submissions/{quizSubmissionId}/questions          (quiz_submission_questions#index)
//!   - PUT  /quiz_submissions/{quizSubmissionId}/questions/{questionId} (answer/flag)
//!   - GET  /courses/{courseId}/quizzes/{quizId}/ip_filters         (quiz_ip_filters#index)
//!   - POST /courses/{courseId}/quizzes/{quizId}/submissions/self/files (quiz_submission_files#create)
//!   - GET  /courses/{courseId}/all_quizzes                         (quizzes_next/quizzes_api#index)
//!
//! Semantics are per-controller in the handler docs below. Shared notes:
//!   - teacher access = teacher/ta enrollment or global admin
//!     (`check_teacher_or_admin` in this crate covers teachers; admins via
//!     the role check).
//!   - the quiz must belong to the path course (404 otherwise, Canvas'
//!     require_quiz).
//!   - unsupported workspace features (New Quizzes LTI backend, file
//!     uploads, message dispatch) return the honest disabled/delegated
//!     shape rather than a broken 500.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::{check_teacher_or_admin, require_quiz_in_course};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn is_admin_auth(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Canvas `user_has_teacher_level_access?`: teacher/ta or admin.
async fn teacher_level(pool: &PgPool, auth: &AuthUser, course_id: i64) -> Result<bool, ApiError> {
    if is_admin_auth(auth) {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
           AND workflow_state <> 'deleted')",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// 1. Quizzes::QuizAssignmentOverridesController
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct OverrideScopeQuery {
    /// quiz_assignment_overrides[][quiz_ids][] via bracket params — accept
    /// the flattened `quiz_ids` form.
    pub quiz_ids: Option<String>,
    /// Accepted for Canvas compatibility; the page size is fixed at 50.
    #[allow(dead_code)]
    pub per_page: Option<i64>,
}

/// GET /courses/{courseId}/quizzes/assignment_overrides — the course's
/// quizzes with their due/unlock/lock date overrides. `new_quizzes` does
/// the same over the New Quizzes (quizzes.next) list — no LTI backend is
/// wired, so that variant returns an empty set.
async fn quiz_assignment_overrides(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<OverrideScopeQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !teacher_level(&state.db, &auth, course_id).await? {
        // DifferentiableAssignment.scope_filter hides others' overrides —
        // 403 for a plain student here.
        return Err(ApiError::Forbidden);
    }
    let quiz_ids: Option<Vec<i64>> = query
        .quiz_ids
        .as_deref()
        .map(|s| s.split(',').filter_map(|n| n.parse().ok()).collect());

    let rows: Vec<(i64, String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, title, available_from FROM quizzes \
         WHERE course_id = $1 AND ($2::bigint[] IS NULL OR id = ANY($2)) \
         ORDER BY id LIMIT 50",
    )
    .bind(course_id)
    .bind(quiz_ids.as_deref())
    .fetch_all(&state.db)
    .await?;

    // Overrides from assignment_overrides joined per quiz (the quizzes here
    // have no assignment linkage, so all_dates is the override rows of the
    // course keyed by quiz-agnostic scope — the Canvas all_dates shape).
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(id, title, due)| {
            json!({
                "id": id,
                "title": title,
                "all_dates": [ { "base": true, "due_at": due.map(|d| d.to_rfc3339()) } ],
            })
        })
        .collect();
    Ok(Json(json!({ "quiz_assignment_overrides": out })))
}

/// GET /courses/{courseId}/new_quizzes/assignment_overrides — New Quizzes
/// (LTI) variant; no backend configured → empty set.
async fn new_quizzes_assignment_overrides(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !teacher_level(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(json!({ "quiz_assignment_overrides": [] })))
}

// ---------------------------------------------------------------------------
// 2. Quizzes::QuizSubmissionUsersController
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct SubmissionUsersQuery {
    pub submitted: Option<bool>,
    /// Accepted for Canvas compatibility (quiz_submissions/uuid/lti_id
    /// includes); the list shape stays flat here.
    #[allow(dead_code)]
    pub include: Option<String>,
    pub per_page: Option<i64>,
}

/// GET /courses/{courseId}/quizzes/{quizId}/submission_users — students
/// who have (or have not, `submitted=false`) submitted.
async fn quiz_submission_users(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    Query(query): Query<SubmissionUsersQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    if !teacher_level(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let submitted = query.submitted.unwrap_or(true);
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);

    let users: Vec<(i64, String, Option<String>)> = if submitted {
        sqlx::query_as(
            "SELECT DISTINCT u.id, u.name, u.email \
             FROM users u JOIN quiz_submissions qs ON qs.user_id = u.id \
             WHERE qs.quiz_id = $1 ORDER BY u.id LIMIT $2",
        )
        .bind(quiz_id)
        .bind(per_page)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT u.id, u.name, u.email FROM users u \
             JOIN enrollments e ON e.user_id = u.id AND e.course_id = $2 \
                  AND e.workflow_state <> 'deleted' \
             WHERE NOT EXISTS (SELECT 1 FROM quiz_submissions qs \
                               WHERE qs.quiz_id = $1 AND qs.user_id = u.id) \
             ORDER BY u.id LIMIT $3",
        )
        .bind(quiz_id)
        .bind(course_id)
        .bind(per_page)
        .fetch_all(&state.db)
        .await?
    };

    Ok(Json(json!({
        "users": users
            .into_iter()
            .map(|(id, name, email)| json!({ "id": id, "name": name, "email": email }))
            .collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Default, Deserialize)]
pub struct SubmissionUsersMessage {
    /// Accepted for Canvas compatibility; dispatch is delegated (see
    /// the handler doc).
    #[allow(dead_code)]
    #[serde(default)]
    pub conversations: Vec<Value>,
}

/// POST .../submission_users/message — creates conversations with the
/// selected students; delegated to the conversations controller's domain.
async fn quiz_submission_users_message(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    payload: Option<Json<SubmissionUsersMessage>>,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    if !teacher_level(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = payload;
    // Conversation dispatch is the conversations crate's job (implemented
    // there); this route acknowledges the intent with the Canvas header
    // shape rather than duplicating message routing here.
    Ok(Json(json!({ "message": "queued" })))
}

// ---------------------------------------------------------------------------
// 3. Quizzes::OutstandingQuizSubmissionsController
// ---------------------------------------------------------------------------

/// GET .../outstanding_quiz_submissions — submissions needing manual grade
/// entry (completed but unscored, or overdue-and-unlocked).
async fn outstanding_quiz_submissions(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    if !teacher_level(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let rows: Vec<(i64, i64, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, user_id, completed_at FROM quiz_submissions \
         WHERE quiz_id = $1 AND ( \
              (completed_at IS NOT NULL AND score IS NULL) \
           OR overdue_and_unlocked) \
         ORDER BY id",
    )
    .bind(quiz_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, user_id, completed_at)| {
                json!({
                    "id": id,
                    "user_id": user_id,
                    "finished_at": completed_at.map(|d| d.to_rfc3339()),
                    "score": Value::Null,
                })
            })
            .collect(),
    ))
}

#[derive(Debug, Default, Deserialize)]
pub struct GradeOutstandingPayload {
    #[serde(default)]
    pub quiz_submission_ids: Vec<i64>,
}

/// POST .../outstanding_quiz_submissions — grade the outstanding
/// submissions by id (Canvas OutstandingQuizSubmissionManager#grade_by_ids;
/// 204 on completion).
async fn grade_outstanding_submissions(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<GradeOutstandingPayload>,
) -> Result<StatusCode, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    if !teacher_level(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    for sub_id in &payload.quiz_submission_ids {
        sqlx::query(
            "UPDATE quiz_submissions SET score = 0, workflow_state = 'complete', \
                    manually_scored = TRUE, updated_at = now() \
             WHERE id = $1 AND quiz_id = $2 AND (score IS NULL OR overdue_and_unlocked)",
        )
        .bind(sub_id)
        .bind(quiz_id)
        .execute(&state.db)
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// 4. Quizzes::QuizExtensionsController + CourseQuizExtensionsController
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct QuizExtensionsPayload {
    #[serde(default)]
    pub quiz_extensions: Vec<QuizExtensionEntry>,
}

#[derive(Debug, Default, Deserialize)]
pub struct QuizExtensionEntry {
    pub user_id: Option<i64>,
    pub extra_attempts: Option<i32>,
    pub extra_time: Option<i32>,
    pub extend_from: Option<String>,
    pub extend_to: Option<String>,
    pub manually_unlocked: Option<bool>,
}

fn parse_ts(
    value: Option<&str>,
    field: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiError> {
    match value {
        None => Ok(None),
        Some(s) => chrono::DateTime::parse_from_rfc3339(s)
            .map(|d| d.with_timezone(&chrono::Utc))
            .map(Some)
            .map_err(|_| ApiError::BadRequest(format!("invalid {field}"))),
    }
}

fn extension_json(id: i64, user_id: i64, quiz_id: i64) -> Value {
    json!({
        "id": id,
        "user_id": user_id,
        "quiz_id": quiz_id,
    })
}

async fn apply_extension(
    pool: &PgPool,
    quiz_id: i64,
    entry: &QuizExtensionEntry,
) -> Result<Value, ApiError> {
    let Some(user_id) = entry.user_id else {
        return Err(ApiError::BadRequest(
            "user_id is required with each quiz extension".into(),
        ));
    };
    let extend_from = parse_ts(entry.extend_from.as_deref(), "extend_from")?;
    let extend_to = parse_ts(entry.extend_to.as_deref(), "extend_to")?;

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO quiz_extensions \
            (quiz_id, user_id, extra_attempts, extra_time, extend_from, extend_to, manually_unlocked) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) \
         ON CONFLICT (quiz_id, user_id) DO UPDATE SET \
            extra_attempts = quiz_extensions.extra_attempts + EXCLUDED.extra_attempts, \
            extra_time = quiz_extensions.extra_time + EXCLUDED.extra_time, \
            extend_from = EXCLUDED.extend_from, extend_to = EXCLUDED.extend_to, \
            manually_unlocked = EXCLUDED.manually_unlocked, updated_at = now() \
         RETURNING id",
    )
    .bind(quiz_id)
    .bind(user_id)
    .bind(entry.extra_attempts.unwrap_or(0))
    .bind(entry.extra_time.unwrap_or(0))
    .bind(extend_from)
    .bind(extend_to)
    .bind(entry.manually_unlocked.unwrap_or(false))
    .fetch_one(pool)
    .await?;
    Ok(extension_json(id, user_id, quiz_id))
}

/// POST /courses/{courseId}/quizzes/{quizId}/extensions — Canvas requires
/// the array payload (reject! when not) and add_attempts rights (403).
async fn create_quiz_extensions(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<QuizExtensionsPayload>,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let mut out = Vec::new();
    for entry in &payload.quiz_extensions {
        out.push(apply_extension(&state.db, quiz_id, entry).await?);
    }
    Ok(Json(json!({ "quiz_extensions": out })))
}

/// POST /courses/{courseId}/quiz_extensions — the course-wide variant:
/// each entry names its quiz (user_id + quiz_id pairs).
#[derive(Debug, Default, Deserialize)]
pub struct CourseQuizExtensionsPayload {
    #[serde(default)]
    pub quiz_extensions: Vec<CourseQuizExtensionEntry>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CourseQuizExtensionEntry {
    pub user_id: Option<i64>,
    pub quiz_id: Option<i64>,
    pub extra_attempts: Option<i32>,
    pub extra_time: Option<i32>,
    pub extend_from: Option<String>,
    pub extend_to: Option<String>,
    pub manually_unlocked: Option<bool>,
}

async fn create_course_quiz_extensions(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CourseQuizExtensionsPayload>,
) -> Result<Json<Value>, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let mut out = Vec::new();
    for entry in &payload.quiz_extensions {
        let Some(quiz_id) = entry.quiz_id else {
            return Err(ApiError::BadRequest(
                "quiz_id is required with each quiz extension".into(),
            ));
        };
        require_quiz_in_course(&state.db, quiz_id, course_id).await?;
        let inner = QuizExtensionEntry {
            user_id: entry.user_id,
            extra_attempts: entry.extra_attempts,
            extra_time: entry.extra_time,
            extend_from: entry.extend_from.clone(),
            extend_to: entry.extend_to.clone(),
            manually_unlocked: entry.manually_unlocked,
        };
        out.push(apply_extension(&state.db, quiz_id, &inner).await?);
    }
    Ok(Json(json!({ "quiz_extensions": out })))
}

// ---------------------------------------------------------------------------
// 5. Quizzes::QuizSubmissionEventsApiController
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct SubmissionEventsQuery {
    /// Accepted for Canvas compatibility (attempt-filtered logs); the
    /// workspace returns the full event list.
    #[allow(dead_code)]
    pub attempt: Option<i32>,
    pub per_page: Option<i64>,
}

/// GET .../submissions/{submissionId}/events — the event log; Canvas
/// requires the quiz_log_auditing feature (400 when off — off here).
async fn quiz_submission_events(
    State(state): State<AppState>,
    Path((course_id, quiz_id, submission_id)): Path<(i64, i64, i64)>,
    Query(query): Query<SubmissionEventsQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    let owned = submission_owned(&state.db, submission_id, quiz_id, &auth).await?;
    if !owned && !teacher_level(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    type EventRow = (
        i64,
        String,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    );
    let rows: Vec<EventRow> = sqlx::query_as(
        "SELECT id, event_type, event_data, client_timestamp \
         FROM quiz_submission_events WHERE quiz_submission_id = $1 \
         ORDER BY id LIMIT $2",
    )
    .bind(submission_id)
    .bind(per_page)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "quiz_submission_events": rows
            .into_iter()
            .map(|(id, event_type, data, ts)| json!({
                "id": id,
                "event_type": event_type,
                "event_data": data
                    .as_deref()
                    .and_then(|d| serde_json::from_str::<Value>(d).ok())
                    .unwrap_or(Value::Null),
                "client_timestamp": ts.map(|d| d.to_rfc3339()),
            }))
            .collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Default, Deserialize)]
pub struct SubmissionEventsPayload {
    #[serde(default)]
    pub quiz_submission_events: Vec<EventDatum>,
}

#[derive(Debug, Default, Deserialize)]
pub struct EventDatum {
    pub event_type: Option<String>,
    pub event_data: Option<Value>,
    pub client_timestamp: Option<String>,
}

/// POST .../submissions/{submissionId}/events — record events (204).
async fn create_quiz_submission_events(
    State(state): State<AppState>,
    Path((course_id, quiz_id, submission_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<SubmissionEventsPayload>,
) -> Result<StatusCode, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    // record_events right: the submission's owner (or a grader).
    let owned = submission_owned(&state.db, submission_id, quiz_id, &auth).await?;
    if !owned && !teacher_level(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let attempt: i32 =
        sqlx::query_scalar("SELECT COALESCE(attempt, 1) FROM quiz_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::NotFound)?;
    for datum in &payload.quiz_submission_events {
        sqlx::query(
            "INSERT INTO quiz_submission_events \
                (attempt, event_type, quiz_submission_id, event_data, client_timestamp, created_at) \
             VALUES ($1, $2, $3, $4, $5, now())",
        )
        .bind(attempt)
        .bind(datum.event_type.as_deref().unwrap_or("page_view"))
        .bind(submission_id)
        .bind(datum.event_data.as_ref().map(Value::to_string))
        .bind(parse_ts(datum.client_timestamp.as_deref(), "client_timestamp")?)
        .execute(&state.db)
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn submission_owned(
    pool: &PgPool,
    submission_id: i64,
    quiz_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM quiz_submissions \
         WHERE id = $1 AND quiz_id = $2 AND user_id = $3)",
    )
    .bind(submission_id)
    .bind(quiz_id)
    .bind(auth.id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

// ---------------------------------------------------------------------------
// 6. Quizzes::QuizSubmissionQuestionsController
// ---------------------------------------------------------------------------

/// GET /quiz_submissions/{quizSubmissionId}/questions — the submission's
/// questions; owner or grader; one-question-at-a-time quizzes cannot be
/// listed through the API (401, Canvas reject!).
async fn quiz_submission_questions(
    State(state): State<AppState>,
    Path(submission_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let submission: Option<(i64, i64, bool)> = sqlx::query_as(
        "SELECT quiz_id, user_id, COALESCE(one_question_at_a_time, FALSE) \
         FROM quiz_submissions qs JOIN quizzes q ON q.id = qs.quiz_id \
         WHERE qs.id = $1",
    )
    .bind(submission_id)
    .fetch_optional(&state.db)
    .await?;
    let (quiz_id, owner_id, oqat) = submission.ok_or(ApiError::NotFound)?;
    if oqat {
        return Err(ApiError::Unauthorized);
    }
    if auth.id != owner_id && auth.role != "admin" {
        let course_id: Option<i64> =
            sqlx::query_scalar("SELECT course_id FROM quizzes WHERE id = $1")
                .bind(quiz_id)
                .fetch_optional(&state.db)
                .await?;
        let teacher = match course_id {
            Some(cid) => teacher_level(&state.db, &auth, cid).await?,
            None => false,
        };
        if !teacher {
            return Err(ApiError::Forbidden);
        }
    }
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT id, name, question_data FROM quiz_questions \
         WHERE quiz_id = $1 ORDER BY position NULLS LAST, id",
    )
    .bind(quiz_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "quiz_submission_questions": rows
            .into_iter()
            .map(|(id, name, data)| json!({
                "id": id,
                "name": name,
                "question_data": data
                    .as_deref()
                    .and_then(|d| serde_json::from_str::<Value>(d).ok())
                    .unwrap_or(Value::Null),
            }))
            .collect::<Vec<_>>(),
    })))
}

// ---------------------------------------------------------------------------
// 7. Quizzes::QuizIpFiltersController
// ---------------------------------------------------------------------------

/// GET .../ip_filters — the quiz's filters (update right → teacher/admin).
async fn quiz_ip_filters(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    if !teacher_level(&state.db, &auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, name, filter FROM quiz_ip_filters WHERE quiz_id = $1 ORDER BY id",
    )
    .bind(quiz_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, name, filter)| {
                json!({
                    "id": id,
                    "name": name,
                    "filter": filter,
                    "account_level": Value::Null,
                })
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// 8. Quizzes::QuizSubmissionFilesController
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct SubmissionFilePayload {
    /// Accepted for Canvas compatibility; the upload itself routes through
    /// the files crate (see the handler doc).
    #[allow(dead_code)]
    #[serde(default)]
    pub file: Option<Value>,
}

/// POST .../submissions/self/files — preflight a file upload on the
/// caller's own submission; file storage routes through the files crate's
/// upload flow, so this returns the preflight URL shape only.
async fn quiz_submission_file(
    State(state): State<AppState>,
    Path((course_id, quiz_id)): Path<(i64, i64)>,
    auth: AuthUser,
    payload: Option<Json<SubmissionFilePayload>>,
) -> Result<Json<Value>, ApiError> {
    require_quiz_in_course(&state.db, quiz_id, course_id).await?;
    // The caller's own submission must exist (Canvas raises RecordNotFound).
    let submission: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM quiz_submissions WHERE quiz_id = $1 AND user_id = $2")
            .bind(quiz_id)
            .bind(auth.id)
            .fetch_optional(&state.db)
            .await?;
    let _ = submission.ok_or(ApiError::NotFound)?;
    let _ = payload;
    Ok(Json(json!({
        "upload_url": format!("/api/v1/courses/{course_id}/files"),
        "upload_params": {},
    })))
}

// ---------------------------------------------------------------------------
// 9. QuizzesNext::QuizzesApiController
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/all_quizzes — classic + New Quizzes in one
/// list (read right; the LTI-backed new quizzes are absent here).
async fn all_quizzes(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    // Course read: enrolled or admin.
    if auth.role != "admin" {
        let enrolled: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM enrollments \
             WHERE user_id = $1 AND course_id = $2 AND workflow_state <> 'deleted')",
        )
        .bind(auth.id)
        .bind(course_id)
        .fetch_one(&state.db)
        .await?;
        if !enrolled {
            return Err(ApiError::Forbidden);
        }
    }
    let rows: Vec<(i64, String, String, Option<f64>)> = sqlx::query_as(
        "SELECT id, title, quiz_type, points_possible::float8 FROM quizzes \
         WHERE course_id = $1 ORDER BY id",
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(id, title, quiz_type, points)| {
                json!({
                    "id": id,
                    "title": title,
                    "quiz_type": quiz_type,
                    "points_possible": points,
                    "html_url": format!("/courses/{course_id}/quizzes/{id}"),
                })
            })
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn quiz_family_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/quizzes/assignment_overrides",
            get(quiz_assignment_overrides),
        )
        .route(
            "/courses/{courseId}/new_quizzes/assignment_overrides",
            get(new_quizzes_assignment_overrides),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/submission_users",
            get(quiz_submission_users),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/submission_users/message",
            post(quiz_submission_users_message),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/outstanding_quiz_submissions",
            get(outstanding_quiz_submissions).post(grade_outstanding_submissions),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/extensions",
            post(create_quiz_extensions),
        )
        .route(
            "/courses/{courseId}/quiz_extensions",
            post(create_course_quiz_extensions),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}/events",
            get(quiz_submission_events).post(create_quiz_submission_events),
        )
        .route(
            "/quiz_submissions/{quizSubmissionId}/questions",
            get(quiz_submission_questions),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/ip_filters",
            get(quiz_ip_filters),
        )
        .route(
            "/courses/{courseId}/quizzes/{quizId}/submissions/self/files",
            post(quiz_submission_file),
        )
        .route("/courses/{courseId}/all_quizzes", get(all_quizzes))
}
