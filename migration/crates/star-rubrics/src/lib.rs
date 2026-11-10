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
///
/// Mirrors the Canvas `RubricAssessment` shape. `course_id`, `artifact_*`,
/// `assessment_type`, `user_id` and `comments` were added in migration 0048;
/// `submission_id` is optional because Canvas can assess a provisional grade
/// artifact directly.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RubricAssessment {
    pub id: i64,
    pub rubric_id: i64,
    pub course_id: Option<i64>,
    pub rubric_association_id: Option<i64>,
    pub submission_id: Option<i64>,
    pub artifact_id: Option<i64>,
    pub artifact_type: Option<String>,
    pub assessment_type: String,
    pub user_id: Option<i64>,
    pub assessor_id: i64,
    pub score: Option<f64>,
    pub data: Option<serde_json::Value>,
    pub comments: Option<String>,
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
///
/// `submission_id` remains the primary association for this service; Canvas
/// artifact/assessment fields are accepted as optional complements.
#[derive(Debug, Deserialize)]
pub struct CreateAssessment {
    pub submission_id: Option<i64>,
    pub user_id: Option<i64>,
    pub artifact_id: Option<i64>,
    pub artifact_type: Option<String>,
    pub rubric_association_id: Option<i64>,
    pub assessment_type: Option<String>,
    pub score: Option<f64>,
    #[serde(default)]
    pub data: Vec<CriterionAssessment>,
    pub comments: Option<String>,
}

/// Request to update a rubric assessment (partial update).
#[derive(Debug, Deserialize)]
pub struct UpdateAssessment {
    pub score: Option<f64>,
    pub data: Option<Vec<CriterionAssessment>>,
    pub comments: Option<String>,
    pub assessment_type: Option<String>,
    pub workflow_state: Option<String>,
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

/// Column list for single-table rubric assessment queries.
const ASSESSMENT_COLUMNS: &str = "id, rubric_id, course_id, rubric_association_id, submission_id, \
     artifact_id, artifact_type, assessment_type, user_id, assessor_id, score, data, comments, \
     workflow_state, created_at, updated_at";

/// Column list for rubric assessment queries joined with submissions.
const ASSESSMENT_COLUMNS_QUALIFIED: &str = "ra.id, ra.rubric_id, ra.course_id, ra.rubric_association_id, \
     ra.submission_id, ra.artifact_id, ra.artifact_type, ra.assessment_type, ra.user_id, \
     ra.assessor_id, ra.score, ra.data, ra.comments, ra.workflow_state, ra.created_at, ra.updated_at";

/// Canvas only accepts these three rubric assessment types.
const VALID_ASSESSMENT_TYPES: [&str; 3] = ["grading", "peer_review", "provisional_grade"];

fn is_valid_assessment_type(value: &str) -> bool {
    VALID_ASSESSMENT_TYPES.contains(&value)
}

/// Canvas rubric assessment read policy: course managers, the assessor, and
/// the user being assessed may read; other enrolled users may not.
fn can_read_assessment(
    is_admin: bool,
    is_teacher: bool,
    is_assessor: bool,
    is_assessed: bool,
) -> bool {
    is_admin || is_teacher || is_assessor || is_assessed
}

/// Canvas rubric assessment update/delete policy: course managers and the
/// assessor who recorded the assessment.
fn can_modify_assessment(is_admin: bool, is_teacher: bool, is_assessor: bool) -> bool {
    is_admin || is_teacher || is_assessor
}

/// Fetch one assessment by id, or `None` when it does not exist.
async fn fetch_assessment(
    pool: &PgPool,
    assessment_id: i64,
) -> Result<Option<RubricAssessment>, ApiError> {
    let sql = format!("SELECT {ASSESSMENT_COLUMNS} FROM rubric_assessments WHERE id = $1");
    Ok(sqlx::query_as::<_, RubricAssessment>(&sql)
        .bind(assessment_id)
        .fetch_optional(pool)
        .await?)
}

/// Returns `(course_id, owner user_id)` for a submission, following the
/// submission → assignment → course chain.
async fn submission_context(
    pool: &PgPool,
    submission_id: i64,
) -> Result<Option<(i64, i64)>, ApiError> {
    Ok(sqlx::query_as(
        r#"
        SELECT a.course_id, s.user_id
        FROM submissions s
        JOIN assignments a ON a.id = s.assignment_id
        WHERE s.id = $1
        "#,
    )
    .bind(submission_id)
    .fetch_optional(pool)
    .await?)
}

/// Resolve the course and assessed user for a stored assessment, backfilling
/// from the submission when migrations 0023-era rows lack them.
async fn assessment_context(
    pool: &PgPool,
    assessment: &RubricAssessment,
) -> Result<(Option<i64>, Option<i64>), ApiError> {
    let mut course_id = assessment.course_id;
    let mut user_id = assessment.user_id;
    let referenced_submission = assessment
        .submission_id
        .filter(|_| course_id.is_none() || user_id.is_none());
    if let Some(submission_id) = referenced_submission {
        if let Some((sub_course_id, owner_id)) = submission_context(pool, submission_id).await? {
            course_id = course_id.or(Some(sub_course_id));
            user_id = user_id.or(Some(owner_id));
        }
    }
    Ok((course_id, user_id))
}

/// Whether `auth` may manage rubrics in `course_id`. A course-less legacy
/// assessment is only manageable by admins.
async fn caller_manages_course(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: Option<i64>,
) -> Result<bool, ApiError> {
    match course_id {
        Some(cid) => check_teacher_or_admin(pool, auth.id, cid, auth).await,
        None => Ok(is_admin(auth)),
    }
}

/// Shared assessment insert used by the flat and course-scoped create routes.
async fn insert_assessment(
    pool: &PgPool,
    course_id: i64,
    rubric_id: i64,
    assessor_id: i64,
    payload: CreateAssessment,
) -> Result<RubricAssessment, ApiError> {
    let CreateAssessment {
        submission_id,
        user_id,
        artifact_id,
        artifact_type,
        rubric_association_id,
        assessment_type,
        score,
        data,
        comments,
    } = payload;

    let assessment_type = assessment_type.unwrap_or_else(|| "grading".to_string());
    if !is_valid_assessment_type(&assessment_type) {
        return Err(ApiError::Validation(
            "assessment_type must be grading, peer_review, or provisional_grade".into(),
        ));
    }

    if submission_id.is_none() && artifact_id.is_none() {
        return Err(ApiError::Validation(
            "submission_id or artifact_id is required".into(),
        ));
    }

    let mut sub_id = submission_id;
    let mut assessed_id = user_id;
    let mut artifact = artifact_id;
    let mut artifact_kind = artifact_type;

    // Resolve a Canvas "Submission" artifact (and its assessed user) through
    // the submissions table, validating the rubric's course.
    let referenced_submission = sub_id.or_else(|| {
        if artifact_kind.as_deref() == Some("Submission") {
            artifact
        } else {
            None
        }
    });
    if let Some(candidate_id) = referenced_submission {
        let (sub_course_id, owner_id) = submission_context(pool, candidate_id)
            .await?
            .ok_or(ApiError::NotFound)?;
        if sub_course_id != course_id {
            return Err(ApiError::BadRequest(
                "submission does not belong to this rubric's course".into(),
            ));
        }
        sub_id = Some(candidate_id);
        assessed_id = assessed_id.or(Some(owner_id));
        artifact = artifact.or(Some(candidate_id));
        artifact_kind = artifact_kind.or_else(|| Some("Submission".to_string()));
    }

    let data_json = serde_json::to_value(&data).unwrap_or(serde_json::Value::Null);
    let sql = format!(
        "INSERT INTO rubric_assessments (course_id, rubric_id, rubric_association_id, \
         submission_id, artifact_id, artifact_type, assessment_type, user_id, assessor_id, \
         score, data, comments) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
         RETURNING {ASSESSMENT_COLUMNS}"
    );

    Ok(sqlx::query_as::<_, RubricAssessment>(&sql)
        .bind(course_id)
        .bind(rubric_id)
        .bind(rubric_association_id)
        .bind(sub_id)
        .bind(artifact)
        .bind(artifact_kind)
        .bind(assessment_type)
        .bind(assessed_id)
        .bind(assessor_id)
        .bind(score)
        .bind(data_json)
        .bind(comments)
        .fetch_one(pool)
        .await?)
}

/// List assessments for a rubric, scoped to `course_id`.
///
/// Preserves the pre-existing visibility rules: admins see everything, while
/// other callers only see assessments they recorded or that assess them.
async fn list_assessments_for_course(
    pool: &PgPool,
    rubric_id: i64,
    course_id: i64,
    submission_id: Option<i64>,
    caller_id: i64,
    is_admin: bool,
) -> Result<Vec<RubricAssessment>, ApiError> {
    let mut sql = format!(
        "SELECT {ASSESSMENT_COLUMNS_QUALIFIED} FROM rubric_assessments ra \
         LEFT JOIN submissions s ON s.id = ra.submission_id \
         LEFT JOIN assignments a ON a.id = s.assignment_id \
         WHERE ra.rubric_id = $1 AND COALESCE(ra.course_id, a.course_id) = $2"
    );

    if submission_id.is_some() {
        sql.push_str(" AND ra.submission_id = $3");
    }
    let visibility_param = if submission_id.is_some() { 4 } else { 3 };
    if !is_admin {
        sql.push_str(&format!(
            " AND (ra.assessor_id = ${visibility_param} OR COALESCE(ra.user_id, s.user_id) = ${visibility_param})"
        ));
    }
    sql.push_str(" ORDER BY ra.created_at DESC");

    let mut query = sqlx::query_as::<_, RubricAssessment>(&sql)
        .bind(rubric_id)
        .bind(course_id);
    if let Some(sub_id) = submission_id {
        query = query.bind(sub_id);
    }
    if !is_admin {
        query = query.bind(caller_id);
    }

    Ok(query.fetch_all(pool).await?)
}

/// Look up the rubric's course, returning 404 when it does not exist.
async fn rubric_course_id(pool: &PgPool, rubric_id: i64) -> Result<i64, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM rubrics WHERE id = $1"#)
        .bind(rubric_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.ok_or(ApiError::NotFound)?.0)
}

/// POST /rubrics/{rubricId}/assessments  → 201 (existing flat route)
async fn create_assessment(
    State(state): State<AppState>,
    Path(rubric_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateAssessment>,
) -> Result<(StatusCode, Json<RubricAssessment>), ApiError> {
    let course_id = rubric_course_id(&state.db, rubric_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let assessment = insert_assessment(&state.db, course_id, rubric_id, auth.id, payload).await?;
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
    let course_id = rubric_course_id(&state.db, rubric_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows = list_assessments_for_course(
        &state.db,
        rubric_id,
        course_id,
        params.submission_id,
        auth.id,
        is_admin(&auth),
    )
    .await?;
    Ok(Json(rows))
}

/// POST /courses/{courseId}/rubrics/{rubricId}/assessments  → 201
async fn create_course_assessment(
    State(state): State<AppState>,
    Path((course_id, rubric_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<CreateAssessment>,
) -> Result<(StatusCode, Json<RubricAssessment>), ApiError> {
    verify_rubric_course(&state.db, course_id, rubric_id).await?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let assessment = insert_assessment(&state.db, course_id, rubric_id, auth.id, payload).await?;
    Ok((StatusCode::CREATED, Json(assessment)))
}

/// GET /courses/{courseId}/rubrics/{rubricId}/assessments?submission_id=
async fn list_course_assessments(
    State(state): State<AppState>,
    Path((course_id, rubric_id)): Path<(i64, i64)>,
    Query(params): Query<ListAssessmentsQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<RubricAssessment>>, ApiError> {
    verify_rubric_course(&state.db, course_id, rubric_id).await?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows = list_assessments_for_course(
        &state.db,
        rubric_id,
        course_id,
        params.submission_id,
        auth.id,
        is_admin(&auth),
    )
    .await?;
    Ok(Json(rows))
}

/// GET /rubric_assessments/{id}  → 200
async fn get_rubric_assessment(
    State(state): State<AppState>,
    Path(assessment_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<RubricAssessment>, ApiError> {
    let assessment = fetch_assessment(&state.db, assessment_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let (course_id, assessed_user_id) = assessment_context(&state.db, &assessment).await?;
    let is_teacher = caller_manages_course(&state.db, &auth, course_id).await?;

    if !can_read_assessment(
        is_admin(&auth),
        is_teacher,
        assessment.assessor_id == auth.id,
        assessed_user_id == Some(auth.id),
    ) {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(assessment))
}

/// PUT /rubric_assessments/{id}  → 200
async fn update_rubric_assessment(
    State(state): State<AppState>,
    Path(assessment_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateAssessment>,
) -> Result<Json<RubricAssessment>, ApiError> {
    let assessment = fetch_assessment(&state.db, assessment_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let (course_id, _) = assessment_context(&state.db, &assessment).await?;
    let is_teacher = caller_manages_course(&state.db, &auth, course_id).await?;

    if !can_modify_assessment(
        is_admin(&auth),
        is_teacher,
        assessment.assessor_id == auth.id,
    ) {
        return Err(ApiError::Forbidden);
    }

    if payload
        .assessment_type
        .as_deref()
        .is_some_and(|assessment_type| !is_valid_assessment_type(assessment_type))
    {
        return Err(ApiError::Validation(
            "assessment_type must be grading, peer_review, or provisional_grade".into(),
        ));
    }
    if payload
        .workflow_state
        .as_deref()
        .is_some_and(|workflow_state| !matches!(workflow_state, "active" | "deleted"))
    {
        return Err(ApiError::Validation(
            "workflow_state must be active or deleted".into(),
        ));
    }

    let data_json = payload
        .data
        .as_ref()
        .map(|data| serde_json::to_value(data).unwrap_or(serde_json::Value::Null));
    let sql = format!(
        "UPDATE rubric_assessments SET score = COALESCE($2, score), data = COALESCE($3, data), \
         comments = COALESCE($4, comments), assessment_type = COALESCE($5, assessment_type), \
         workflow_state = COALESCE($6, workflow_state), updated_at = now() \
         WHERE id = $1 RETURNING {ASSESSMENT_COLUMNS}"
    );

    let updated = sqlx::query_as::<_, RubricAssessment>(&sql)
        .bind(assessment_id)
        .bind(payload.score)
        .bind(data_json)
        .bind(payload.comments)
        .bind(payload.assessment_type)
        .bind(payload.workflow_state)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(updated))
}

/// DELETE /rubric_assessments/{id}  → 200 with the deleted assessment.
///
/// Mirrors Canvas `RubricAssessmentsController#destroy`, which removes the row
/// and renders the assessment JSON rather than returning 204.
async fn delete_rubric_assessment(
    State(state): State<AppState>,
    Path(assessment_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<RubricAssessment>, ApiError> {
    let assessment = fetch_assessment(&state.db, assessment_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let (course_id, _) = assessment_context(&state.db, &assessment).await?;
    let is_teacher = caller_manages_course(&state.db, &auth, course_id).await?;

    if !can_modify_assessment(
        is_admin(&auth),
        is_teacher,
        assessment.assessor_id == auth.id,
    ) {
        return Err(ApiError::Forbidden);
    }

    let sql =
        format!("DELETE FROM rubric_assessments WHERE id = $1 RETURNING {ASSESSMENT_COLUMNS}");
    let deleted = sqlx::query_as::<_, RubricAssessment>(&sql)
        .bind(assessment_id)
        .fetch_optional(&state.db)
        .await?
        .ok_or(ApiError::NotFound)?;

    Ok(Json(deleted))
}

// ---------------------------------------------------------------------------
// Canvas nested route aliases
// ---------------------------------------------------------------------------

/// Ensure `rubric_id` exists and belongs to `course_id`.
/// Missing and mismatched rubrics both map to 404 so a course-scoped alias
/// never reveals rubrics from another course.
async fn verify_rubric_course(
    pool: &PgPool,
    course_id: i64,
    rubric_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(r#"SELECT course_id FROM rubrics WHERE id = $1"#)
        .bind(rubric_id)
        .fetch_optional(pool)
        .await?;

    match row {
        Some((rubric_course_id,)) if rubric_course_id == course_id => Ok(()),
        _ => Err(ApiError::NotFound),
    }
}

/// GET /courses/{courseId}/rubrics/{id}  → 200 (Canvas nested alias).
async fn get_course_rubric(
    State(state): State<AppState>,
    Path((course_id, rubric_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<RubricResponse>, ApiError> {
    verify_rubric_course(&state.db, course_id, rubric_id).await?;
    get_rubric(State(state), Path(rubric_id), auth).await
}

/// PUT /courses/{courseId}/rubrics/{id}  → 200 (Canvas nested alias).
async fn update_course_rubric(
    State(state): State<AppState>,
    Path((course_id, rubric_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<UpdateRubric>,
) -> Result<Json<RubricResponse>, ApiError> {
    verify_rubric_course(&state.db, course_id, rubric_id).await?;
    update_rubric(State(state), Path(rubric_id), auth, Json(payload)).await
}

/// DELETE /courses/{courseId}/rubrics/{id}  → 204 (Canvas nested alias).
async fn delete_course_rubric(
    State(state): State<AppState>,
    Path((course_id, rubric_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    verify_rubric_course(&state.db, course_id, rubric_id).await?;
    delete_rubric(State(state), Path(rubric_id), auth).await
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses/{courseId}/rubrics", post(create_rubric))
        .route("/courses/{courseId}/rubrics", get(list_course_rubrics))
        .route("/courses/{courseId}/rubrics/{id}", get(get_course_rubric))
        .route(
            "/courses/{courseId}/rubrics/{id}",
            put(update_course_rubric),
        )
        .route(
            "/courses/{courseId}/rubrics/{id}",
            delete(delete_course_rubric),
        )
        .route("/rubrics/{id}", get(get_rubric))
        .route("/rubrics/{id}", put(update_rubric))
        .route("/rubrics/{id}", delete(delete_rubric))
        .route("/rubrics/{rubricId}/assessments", post(create_assessment))
        .route("/rubrics/{rubricId}/assessments", get(list_assessments))
        .route(
            "/courses/{courseId}/rubrics/{rubricId}/assessments",
            post(create_course_assessment),
        )
        .route(
            "/courses/{courseId}/rubrics/{rubricId}/assessments",
            get(list_course_assessments),
        )
        .route("/rubric_assessments/{id}", get(get_rubric_assessment))
        .route("/rubric_assessments/{id}", put(update_rubric_assessment))
        .route("/rubric_assessments/{id}", delete(delete_rubric_assessment))
}

#[cfg(test)]
mod tests {
    use super::{
        can_manage_rubrics, can_modify_assessment, can_read_assessment, is_valid_assessment_type,
    };

    #[test]
    fn only_course_teachers_or_admins_can_manage_rubrics() {
        assert!(can_manage_rubrics(true, false));
        assert!(can_manage_rubrics(false, true));
        assert!(!can_manage_rubrics(false, false));
    }

    #[test]
    fn canvas_assessment_types_are_enforced() {
        assert!(is_valid_assessment_type("grading"));
        assert!(is_valid_assessment_type("peer_review"));
        assert!(is_valid_assessment_type("provisional_grade"));
        assert!(!is_valid_assessment_type("curve"));
        assert!(!is_valid_assessment_type(""));
    }

    #[test]
    fn assessment_read_policy_matches_canvas() {
        // Course managers, assessor, and assessed user can read.
        assert!(can_read_assessment(true, false, false, false));
        assert!(can_read_assessment(false, true, false, false));
        assert!(can_read_assessment(false, false, true, false));
        assert!(can_read_assessment(false, false, false, true));
        // An unrelated enrolled user cannot.
        assert!(!can_read_assessment(false, false, false, false));
    }

    #[test]
    fn assessment_write_policy_matches_canvas() {
        assert!(can_modify_assessment(true, false, false));
        assert!(can_modify_assessment(false, true, false));
        assert!(can_modify_assessment(false, false, true));
        assert!(!can_modify_assessment(false, false, false));
    }
}
