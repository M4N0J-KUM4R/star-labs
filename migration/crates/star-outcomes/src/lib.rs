//! star-outcomes — Canvas learning outcomes, groups, links, and results.
//!
//! Implements the following Canvas API v1 endpoints:
//! - `POST /courses/{courseId}/outcome_groups` — create a group (teacher)
//! - `GET  /courses/{courseId}/outcome_groups` — list groups (enrolled)
//! - `DELETE /outcome_groups/{id}` — soft-delete a group (teacher)
//! - `POST /courses/{courseId}/outcomes` — create an outcome (teacher)
//! - `GET  /courses/{courseId}/outcomes` — list outcomes, excludes deleted (enrolled)
//! - `POST /courses/{courseId}/outcome_groups/{groupId}/outcomes` — link outcome to group (teacher)
//! - `GET  /courses/{courseId}/outcome_groups/{groupId}/outcomes` — list outcomes in a group (enrolled)
//! - `POST /outcomes/{outcomeId}/results` — record a result (teacher)
//! - `GET  /outcomes/{outcomeId}/results` — list results (teacher or the user themself)

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
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

/// A learning outcome. Title is required; description and display_name are optional.
/// Workflow_state is always active|archived|retired|deleted.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LearningOutcome {
    pub id: i64,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating an outcome.
#[derive(Debug, Deserialize)]
pub struct CreateOutcome {
    pub title: String,
    pub description: Option<String>,
    pub display_name: Option<String>,
}

/// A learning outcome group, optionally tied to a course.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LearningOutcomeGroup {
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub course_id: Option<i64>,
    pub title: String,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for creating an outcome group.
#[derive(Debug, Deserialize)]
pub struct CreateOutcomeGroup {
    pub title: String,
}

/// A link between an outcome and a group.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LearningOutcomeLink {
    pub id: i64,
    pub outcome_id: i64,
    pub outcome_group_id: i64,
    pub workflow_state: String,
}

/// Request body for linking an outcome to a group.
#[derive(Debug, Deserialize)]
pub struct LinkOutcomeToGroup {
    pub outcome_id: i64,
}

/// A result recording a user's score on an outcome.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LearningOutcomeResult {
    pub id: i64,
    pub outcome_id: i64,
    pub user_id: i64,
    pub score: f64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Request body for recording a result.
#[derive(Debug, Deserialize)]
pub struct CreateOutcomeResult {
    pub user_id: i64,
    pub score: f64,
}

/// Query params for filtering outcome results by user.
#[derive(Debug, Deserialize)]
pub struct OutcomeResultsQuery {
    pub user_id: Option<i64>,
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
    let row: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND workflow_state = 'active' LIMIT 1")
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
        "SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' AND workflow_state = 'active' LIMIT 1",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

async fn check_teacher_or_admin_for_group(
    pool: &PgPool,
    user_id: i64,
    group_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    // Get the course_id from the group
    let row: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT course_id FROM learning_outcome_groups WHERE id = $1",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?;
    let (course_id,) = row.ok_or(ApiError::NotFound)?;
    match course_id {
        Some(cid) => check_teacher_or_admin(pool, user_id, cid, auth).await,
        None => Ok(false), // global groups require admin
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

// --- Outcome Groups ---

/// POST /courses/{courseId}/outcome_groups → 201
async fn create_outcome_group(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateOutcomeGroup>,
) -> Result<(StatusCode, Json<LearningOutcomeGroup>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }

    let row: LearningOutcomeGroup = sqlx::query_as(
        r#"
        INSERT INTO learning_outcome_groups (course_id, title)
        VALUES ($1, $2)
        RETURNING id, course_id, title, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/outcome_groups (enrolled)
async fn list_outcome_groups(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<LearningOutcomeGroup>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<LearningOutcomeGroup> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, workflow_state, created_at, updated_at
        FROM learning_outcome_groups
        WHERE course_id = $1 AND workflow_state = 'active'
        ORDER BY created_at ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// DELETE /outcome_groups/{id} (teacher) — soft-delete
async fn delete_outcome_group(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !check_teacher_or_admin_for_group(&state.db, auth.id, group_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        "UPDATE learning_outcome_groups SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(group_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

// --- Outcomes ---

/// POST /courses/{courseId}/outcomes → 201
async fn create_outcome(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateOutcome>,
) -> Result<(StatusCode, Json<LearningOutcome>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }

    let row: LearningOutcome = sqlx::query_as(
        r#"
        INSERT INTO learning_outcomes (title, description, display_name)
        VALUES ($1, $2, $3)
        RETURNING id, title, description, display_name, workflow_state, created_at, updated_at
        "#,
    )
    .bind(&payload.title)
    .bind(&payload.description)
    .bind(&payload.display_name)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/outcomes (enrolled) — excludes deleted
async fn list_outcomes(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<LearningOutcome>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Outcomes are linked to courses through outcome_groups that have that course_id.
    // We return outcomes that are linked via active links to groups belonging to this course.
    let rows: Vec<LearningOutcome> = sqlx::query_as(
        r#"
        SELECT DISTINCT lo.id, lo.title, lo.description, lo.display_name, lo.workflow_state,
               lo.created_at, lo.updated_at
        FROM learning_outcomes lo
        JOIN learning_outcome_links lol ON lol.outcome_id = lo.id
        JOIN learning_outcome_groups log ON log.id = lol.outcome_group_id
        WHERE log.course_id = $1
          AND log.workflow_state = 'active'
          AND lol.workflow_state = 'active'
          AND lo.workflow_state = 'active'
        ORDER BY lo.created_at ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

// --- Outcome Links ---

/// POST /courses/{courseId}/outcome_groups/{groupId}/outcomes {outcome_id} → 201
async fn link_outcome_to_group(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<LinkOutcomeToGroup>,
) -> Result<(StatusCode, Json<LearningOutcomeLink>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify group belongs to this course
    let group_row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM learning_outcome_groups WHERE id = $1 AND course_id = $2 AND workflow_state = 'active'",
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    if group_row.is_none() {
        return Err(ApiError::NotFound);
    }

    let row: LearningOutcomeLink = sqlx::query_as(
        r#"
        INSERT INTO learning_outcome_links (outcome_id, outcome_group_id)
        VALUES ($1, $2)
        ON CONFLICT (outcome_id, outcome_group_id) DO UPDATE SET workflow_state = 'active'
        RETURNING id, outcome_id, outcome_group_id, workflow_state
        "#,
    )
    .bind(payload.outcome_id)
    .bind(group_id)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /courses/{courseId}/outcome_groups/{groupId}/outcomes (enrolled)
async fn list_group_outcomes(
    State(state): State<AppState>,
    Path((course_id, group_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Vec<LearningOutcome>>, ApiError> {
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify group belongs to this course
    let group_row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM learning_outcome_groups WHERE id = $1 AND course_id = $2 AND workflow_state = 'active'",
    )
    .bind(group_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    if group_row.is_none() {
        return Err(ApiError::NotFound);
    }

    let rows: Vec<LearningOutcome> = sqlx::query_as(
        r#"
        SELECT lo.id, lo.title, lo.description, lo.display_name, lo.workflow_state,
               lo.created_at, lo.updated_at
        FROM learning_outcomes lo
        JOIN learning_outcome_links lol ON lol.outcome_id = lo.id
        WHERE lol.outcome_group_id = $1
          AND lol.workflow_state = 'active'
          AND lo.workflow_state = 'active'
        ORDER BY lo.created_at ASC
        "#,
    )
    .bind(group_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

// --- Results ---

/// POST /outcomes/{outcomeId}/results {user_id, score} → 201
async fn create_outcome_result(
    State(state): State<AppState>,
    Path(outcome_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateOutcomeResult>,
) -> Result<(StatusCode, Json<LearningOutcomeResult>), ApiError> {
    // Only teachers or admins can record results
    // We need to verify the user is a teacher in the course that owns this outcome
    let outcome_row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT log.course_id
        FROM learning_outcomes lo
        JOIN learning_outcome_links lol ON lol.outcome_id = lo.id
        JOIN learning_outcome_groups log ON log.id = lol.outcome_group_id
        WHERE lo.id = $1 AND lo.workflow_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(outcome_id)
    .fetch_optional(&state.db)
    .await?;

    let course_id = match outcome_row {
        Some((cid,)) => cid,
        None => return Err(ApiError::NotFound),
    };

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: LearningOutcomeResult = sqlx::query_as(
        r#"
        INSERT INTO learning_outcome_results (outcome_id, user_id, score)
        VALUES ($1, $2, $3)
        RETURNING id, outcome_id, user_id, score, created_at, updated_at
        "#,
    )
    .bind(outcome_id)
    .bind(payload.user_id)
    .bind(payload.score)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row)))
}

/// GET /outcomes/{outcomeId}/results?user_id= (teacher or the user themself)
async fn list_outcome_results(
    State(state): State<AppState>,
    Path(outcome_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<OutcomeResultsQuery>,
) -> Result<Json<Vec<LearningOutcomeResult>>, ApiError> {
    // Verify outcome exists
    let outcome_row: Option<(i64,)> = sqlx::query_as(
        r#"
        SELECT log.course_id
        FROM learning_outcomes lo
        JOIN learning_outcome_links lol ON lol.outcome_id = lo.id
        JOIN learning_outcome_groups log ON log.id = lol.outcome_group_id
        WHERE lo.id = $1 AND lo.workflow_state = 'active'
        LIMIT 1
        "#,
    )
    .bind(outcome_id)
    .fetch_optional(&state.db)
    .await?;

    let (course_id,) = outcome_row.ok_or(ApiError::NotFound)?;

    // If filtering by user_id, check permissions
    if let Some(uid) = params.user_id {
        if uid != auth.id && !is_admin(&auth) {
            // Check if the requesting user is a teacher in the course
            if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
                return Err(ApiError::Forbidden);
            }
        }
        let rows: Vec<LearningOutcomeResult> = sqlx::query_as(
            r#"
            SELECT id, outcome_id, user_id, score, created_at, updated_at
            FROM learning_outcome_results
            WHERE outcome_id = $1 AND user_id = $2
            ORDER BY created_at ASC
            "#,
        )
        .bind(outcome_id)
        .bind(uid)
        .fetch_all(&state.db)
        .await?;
        Ok(Json(rows))
    } else {
        // No user_id filter: require teacher/admin
        if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
            return Err(ApiError::Forbidden);
        }
        let rows: Vec<LearningOutcomeResult> = sqlx::query_as(
            r#"
            SELECT id, outcome_id, user_id, score, created_at, updated_at
            FROM learning_outcome_results
            WHERE outcome_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(outcome_id)
        .fetch_all(&state.db)
        .await?;
        Ok(Json(rows))
    }
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        // outcome groups
        .route("/courses/{courseId}/outcome_groups", post(create_outcome_group))
        .route("/courses/{courseId}/outcome_groups", get(list_outcome_groups))
        .route("/outcome_groups/{id}", delete(delete_outcome_group))
        // outcomes
        .route("/courses/{courseId}/outcomes", post(create_outcome))
        .route("/courses/{courseId}/outcomes", get(list_outcomes))
        // linking outcomes to groups
        .route(
            "/courses/{courseId}/outcome_groups/{groupId}/outcomes",
            post(link_outcome_to_group),
        )
        .route(
            "/courses/{courseId}/outcome_groups/{groupId}/outcomes",
            get(list_group_outcomes),
        )
        // results
        .route("/outcomes/{outcomeId}/results", post(create_outcome_result))
        .route("/outcomes/{outcomeId}/results", get(list_outcome_results))
}
