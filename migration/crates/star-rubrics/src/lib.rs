//! star-rubrics: rubrics, criteria, ratings, and rubric assessments.
//!
//! Implements Canvas-style rubrics API with teacher-based access control.

use axum::{
    extract::{Path, Query, State},
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

/// Rubric record.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Rubric {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    pub points_possible: f64,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Rubric criterion record.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RubricCriterion {
    pub id: i64,
    pub rubric_id: i64,
    pub description: String,
    pub points: f64,
    pub position: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Rubric rating record.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RubricRating {
    pub id: i64,
    pub criterion_id: i64,
    pub description: String,
    pub points: f64,
    pub position: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Rubric assessment record.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RubricAssessment {
    pub id: i64,
    pub rubric_id: i64,
    pub submission_id: i64,
    pub assessor_id: i64,
    pub score: Option<f64>,
    pub data: Option<serde_json::Value>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Rating within a criterion.
#[derive(Debug, Deserialize)]
pub struct CreateRating {
    pub description: String,
    pub points: f64,
}

/// Criterion within a rubric creation request.
#[derive(Debug, Deserialize)]
pub struct CreateCriterion {
    pub description: String,
    pub points: f64,
    pub ratings: Option<Vec<CreateRating>>,
}

/// Request to create a rubric.
#[derive(Debug, Deserialize)]
pub struct CreateRubric {
    pub title: String,
    pub points_possible: Option<f64>,
    pub criteria: Vec<CreateCriterion>,
}

/// Request to update a rubric.
#[derive(Debug, Deserialize)]
pub struct UpdateRubric {
    pub title: Option<String>,
    pub points_possible: Option<f64>,
}

/// A single criterion assessment within an assessment data array.
#[derive(Debug, Deserialize, serde::Serialize)]
pub struct CriterionAssessment {
    pub criterion_id: i64,
    pub rating_id: Option<i64>,
    pub points: f64,
    #[serde(default)]
    pub comments: Option<String>,
}

/// Request to create a rubric assessment.
#[derive(Debug, Deserialize)]
pub struct CreateAssessment {
    pub submission_id: i64,
    pub score: Option<f64>,
    pub data: Vec<CriterionAssessment>,
}

// ---------------------------------------------------------------------------
// Nested response types (criteria + ratings)
// ---------------------------------------------------------------------------

/// Rating in a nested rubric response.
#[derive(Debug, Clone, Serialize)]
pub struct RatingResponse {
    pub id: i64,
    pub description: String,
    pub points: f64,
    pub position: i32,
}

/// Criterion in a nested rubric response.
#[derive(Debug, Clone, Serialize)]
pub struct CriterionResponse {
    pub id: i64,
    pub description: String,
    pub points: f64,
    pub position: i32,
    pub ratings: Vec<RatingResponse>,
}

/// Full rubric response with nested criteria and ratings.
#[derive(Debug, Clone, Serialize)]
pub struct RubricResponse {
    pub id: i64,
    pub course_id: i64,
    pub title: String,
    pub points_possible: f64,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub criteria: Vec<CriterionResponse>,
}

// ---------------------------------------------------------------------------
// Permission helpers
// ---------------------------------------------------------------------------

fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

fn can_manage_rubrics(is_admin: bool, has_teacher_enrollment: bool) -> bool {
    is_admin || has_teacher_enrollment
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
    Ok(can_manage_rubrics(false, row.is_some()))
}

// ---------------------------------------------------------------------------
// Nested response builders
// ---------------------------------------------------------------------------

/// Fetch full rubric with all criteria and ratings nested.
async fn fetch_rubric_with_nested(
    pool: &PgPool,
    rubric_id: i64,
) -> Result<Option<RubricResponse>, ApiError> {
    let rubric: Option<Rubric> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, points_possible, workflow_state, created_at, updated_at
        FROM rubrics WHERE id = $1
        "#,
    )
    .bind(rubric_id)
    .fetch_optional(pool)
    .await?;

    let Some(rubric) = rubric else {
        return Ok(None);
    };

    let criteria: Vec<RubricCriterion> = sqlx::query_as(
        r#"
        SELECT id, rubric_id, description, points, position, created_at, updated_at
        FROM rubric_criteria WHERE rubric_id = $1 ORDER BY position ASC
        "#,
    )
    .bind(rubric_id)
    .fetch_all(pool)
    .await?;

    let mut criteria_response = Vec::with_capacity(criteria.len());
    for criterion in criteria {
        let ratings: Vec<RubricRating> = sqlx::query_as(
            r#"
            SELECT id, criterion_id, description, points, position, created_at, updated_at
            FROM rubric_ratings WHERE criterion_id = $1 ORDER BY position ASC
            "#,
        )
        .bind(criterion.id)
        .fetch_all(pool)
        .await?;

        let ratings_response: Vec<RatingResponse> = ratings
            .into_iter()
            .map(|r| RatingResponse {
                id: r.id,
                description: r.description,
                points: r.points,
                position: r.position,
            })
            .collect();

        criteria_response.push(CriterionResponse {
            id: criterion.id,
            description: criterion.description,
            points: criterion.points,
            position: criterion.position,
            ratings: ratings_response,
        });
    }

    Ok(Some(RubricResponse {
        id: rubric.id,
        course_id: rubric.course_id,
        title: rubric.title,
        points_possible: rubric.points_possible,
        workflow_state: rubric.workflow_state,
        created_at: rubric.created_at,
        updated_at: rubric.updated_at,
        criteria: criteria_response,
    }))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/rubrics  → 201
async fn create_rubric(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateRubric>,
) -> Result<(StatusCode, Json<RubricResponse>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    if payload.title.trim().is_empty() {
        return Err(ApiError::Validation("title is required".into()));
    }

    let points_possible = payload.points_possible.unwrap_or(0.0);

    let rubric: Rubric = sqlx::query_as(
        r#"
        INSERT INTO rubrics (course_id, title, points_possible)
        VALUES ($1, $2, $3)
        RETURNING id, course_id, title, points_possible, workflow_state, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&payload.title)
    .bind(points_possible)
    .fetch_one(&state.db)
    .await?;

    let mut criteria_response = Vec::new();
    for (position, criterion) in payload.criteria.iter().enumerate() {
        let criterion_row: RubricCriterion = sqlx::query_as(
            r#"
            INSERT INTO rubric_criteria (rubric_id, description, points, position)
            VALUES ($1, $2, $3, $4)
            RETURNING id, rubric_id, description, points, position, created_at, updated_at
            "#,
        )
        .bind(rubric.id)
        .bind(&criterion.description)
        .bind(criterion.points)
        .bind(position as i32)
        .fetch_one(&state.db)
        .await?;

        let mut ratings_response = Vec::new();
        if let Some(ref ratings) = criterion.ratings {
            for (rating_pos, rating) in ratings.iter().enumerate() {
                let rating_row: RubricRating = sqlx::query_as(
                    r#"
                    INSERT INTO rubric_ratings (criterion_id, description, points, position)
                    VALUES ($1, $2, $3, $4)
                    RETURNING id, criterion_id, description, points, position, created_at, updated_at
                    "#,
                )
                .bind(criterion_row.id)
                .bind(&rating.description)
                .bind(rating.points)
                .bind(rating_pos as i32)
                .fetch_one(&state.db)
                .await?;

                ratings_response.push(RatingResponse {
                    id: rating_row.id,
                    description: rating_row.description,
                    points: rating_row.points,
                    position: rating_row.position,
                });
            }
        }

        criteria_response.push(CriterionResponse {
            id: criterion_row.id,
            description: criterion_row.description,
            points: criterion_row.points,
            position: criterion_row.position,
            ratings: ratings_response,
        });
    }

    let response = RubricResponse {
        id: rubric.id,
        course_id: rubric.course_id,
        title: rubric.title,
        points_possible: rubric.points_possible,
        workflow_state: rubric.workflow_state,
        created_at: rubric.created_at,
        updated_at: rubric.updated_at,
        criteria: criteria_response,
    };

    Ok((StatusCode::CREATED, Json(response)))
}

/// GET /courses/{courseId}/rubrics  → 200
/// Canvas requires manage_rubrics permission to list rubrics; students cannot
/// enumerate the course rubric catalog through this endpoint.
async fn list_course_rubrics(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Rubric>>, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<Rubric> = sqlx::query_as(
        r#"
        SELECT id, course_id, title, points_possible, workflow_state, created_at, updated_at
        FROM rubrics
        WHERE course_id = $1 AND workflow_state != 'deleted'
        ORDER BY created_at ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows))
}

/// GET /rubrics/{id}  → 200
/// Returns full rubric with nested criteria + ratings.
async fn get_rubric(
    State(state): State<AppState>,
    Path(rubric_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<RubricResponse>, ApiError> {
    let rubric = fetch_rubric_with_nested(&state.db, rubric_id)
        .await?
        .ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, rubric.course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(rubric))
}

/// PUT /rubrics/{id}  → 200
async fn update_rubric(
    State(state): State<AppState>,
    Path(rubric_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateRubric>,
) -> Result<Json<RubricResponse>, ApiError> {
    let existing: (i64,) = sqlx::query_as(r#"SELECT course_id FROM rubrics WHERE id = $1"#)
        .bind(rubric_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, existing.0, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Fetch current values for partial updates
    let current: Rubric = sqlx::query_as(
        r#"
        SELECT id, course_id, title, points_possible, workflow_state, created_at, updated_at
        FROM rubrics WHERE id = $1
        "#,
    )
    .bind(rubric_id)
    .fetch_one(&state.db)
    .await?;

    let title = payload.title.unwrap_or(current.title);
    let points_possible = payload.points_possible.unwrap_or(current.points_possible);

    let updated: Rubric = sqlx::query_as(
        r#"
        UPDATE rubrics
        SET title = $2, points_possible = $3, updated_at = now()
        WHERE id = $1
        RETURNING id, course_id, title, points_possible, workflow_state, created_at, updated_at
        "#,
    )
    .bind(rubric_id)
    .bind(&title)
    .bind(points_possible)
    .fetch_one(&state.db)
    .await?;

    // Re-fetch with nested criteria/ratings for response
    let response = fetch_rubric_with_nested(&state.db, rubric_id)
        .await?
        .unwrap_or(RubricResponse {
            id: updated.id,
            course_id: updated.course_id,
            title: updated.title,
            points_possible: updated.points_possible,
            workflow_state: updated.workflow_state,
            created_at: updated.created_at,
            updated_at: updated.updated_at,
            criteria: vec![],
        });

    Ok(Json(response))
}

/// DELETE /rubrics/{id}  → 204 (soft-delete)
async fn delete_rubric(
    State(state): State<AppState>,
    Path(rubric_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM rubrics WHERE id = $1"#)
        .bind(rubric_id)
        .fetch_optional(&state.db)
        .await?;

    let (course_id,) = row.ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        r#"UPDATE rubrics SET workflow_state = 'deleted', updated_at = now() WHERE id = $1"#,
    )
    .bind(rubric_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /rubrics/{rubricId}/assessments  → 201
async fn create_assessment(
    State(state): State<AppState>,
    Path(rubric_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateAssessment>,
) -> Result<(StatusCode, Json<RubricAssessment>), ApiError> {
    // Verify rubric exists and get course_id
    let rubric_row: Option<(i64,)> =
        sqlx::query_as(r#"SELECT course_id FROM rubrics WHERE id = $1"#)
            .bind(rubric_id)
            .fetch_optional(&state.db)
            .await?;

    let (course_id,) = rubric_row.ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let data_json = serde_json::to_value(&payload.data).unwrap_or(serde_json::Value::Null);

    let assessment: RubricAssessment = sqlx::query_as(
        r#"
        INSERT INTO rubric_assessments (rubric_id, submission_id, assessor_id, score, data)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id, rubric_id, submission_id, assessor_id, score, data, workflow_state, created_at, updated_at
        "#,
    )
    .bind(rubric_id)
    .bind(payload.submission_id)
    .bind(auth.id)
    .bind(payload.score)
    .bind(data_json)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(assessment)))
}

/// Query params for listing assessments.
#[derive(Debug, Deserialize)]
pub struct ListAssessmentsQuery {
    pub submission_id: Option<i64>,
}

/// GET /rubrics/{rubricId}/assessments?submission_id=
/// Teachers or the assessed student can list.
async fn list_assessments(
    State(state): State<AppState>,
    Path(rubric_id): Path<i64>,
    Query(params): Query<ListAssessmentsQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<RubricAssessment>>, ApiError> {
    // Verify rubric exists and get course_id
    let rubric_row: Option<(i64,)> =
        sqlx::query_as(r#"SELECT course_id FROM rubrics WHERE id = $1"#)
            .bind(rubric_id)
            .fetch_optional(&state.db)
            .await?;

    let (course_id,) = rubric_row.ok_or(ApiError::NotFound)?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // If not a teacher/admin, enforce submission ownership check via subquery
    if is_admin(&auth) {
        let rows: Vec<RubricAssessment> = if let Some(sub_id) = params.submission_id {
            sqlx::query_as(
                r#"
                SELECT id, rubric_id, submission_id, assessor_id, score, data, workflow_state, created_at, updated_at
                FROM rubric_assessments
                WHERE rubric_id = $1 AND submission_id = $2
                ORDER BY created_at DESC
                "#,
            )
            .bind(rubric_id)
            .bind(sub_id)
            .fetch_all(&state.db)
            .await?
        } else {
            sqlx::query_as(
                r#"
                SELECT id, rubric_id, submission_id, assessor_id, score, data, workflow_state, created_at, updated_at
                FROM rubric_assessments
                WHERE rubric_id = $1
                ORDER BY created_at DESC
                "#,
            )
            .bind(rubric_id)
            .fetch_all(&state.db)
            .await?
        };
        return Ok(Json(rows));
    }

    // For non-teachers: only return assessments where they are the assessor
    // or the submission belongs to them (student who was assessed)
    let rows: Vec<RubricAssessment> = if let Some(sub_id) = params.submission_id {
        sqlx::query_as(
            r#"
            SELECT ra.id, ra.rubric_id, ra.submission_id, ra.assessor_id, ra.score, ra.data, ra.workflow_state, ra.created_at, ra.updated_at
            FROM rubric_assessments ra
            JOIN submissions s ON s.id = ra.submission_id
            WHERE ra.rubric_id = $1 AND ra.submission_id = $2
              AND (ra.assessor_id = $3 OR s.user_id = $3)
            ORDER BY ra.created_at DESC
            "#,
        )
        .bind(rubric_id)
        .bind(sub_id)
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?
    } else {
        sqlx::query_as(
            r#"
            SELECT ra.id, ra.rubric_id, ra.submission_id, ra.assessor_id, ra.score, ra.data, ra.workflow_state, ra.created_at, ra.updated_at
            FROM rubric_assessments ra
            JOIN submissions s ON s.id = ra.submission_id
            WHERE ra.rubric_id = $1
              AND (ra.assessor_id = $2 OR s.user_id = $2)
            ORDER BY ra.created_at DESC
            "#,
        )
        .bind(rubric_id)
        .bind(auth.id)
        .fetch_all(&state.db)
        .await?
    };

    Ok(Json(rows))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/rubrics", post(create_rubric))
        .route("/courses/{courseId}/rubrics", get(list_course_rubrics))
        .route("/rubrics/{id}", get(get_rubric))
        .route("/rubrics/{id}", put(update_rubric))
        .route("/rubrics/{id}", delete(delete_rubric))
        .route("/rubrics/{rubricId}/assessments", post(create_assessment))
        .route("/rubrics/{rubricId}/assessments", get(list_assessments))
}

#[cfg(test)]
mod tests {
    use super::can_manage_rubrics;

    #[test]
    fn only_course_teachers_or_admins_can_manage_rubrics() {
        assert!(can_manage_rubrics(true, false));
        assert!(can_manage_rubrics(false, true));
        assert!(!can_manage_rubrics(false, false));
    }
}
