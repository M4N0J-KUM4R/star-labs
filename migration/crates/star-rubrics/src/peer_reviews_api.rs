//! peer_reviews — Canvas `PeerReviewsApiController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /sections/{sectionId}/assignments/{assignmentId}/peer_reviews`
//! - `GET    /sections/{sectionId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews`
//! - `POST   /sections/{sectionId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews`
//! - `DELETE /sections/{sectionId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews`
//! - `POST   /courses/{courseId}/assignments/{assignmentId}/allocate`
//!
//! Canvas routes (config/routes.rb, the course and section API blocks):
//!   get    "courses/:course_id/assignments/:assignment_id/peer_reviews", action: :index
//!   get    "sections/:section_id/assignments/:assignment_id/peer_reviews", action: :index
//!   get    "courses/:course_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews", action: :index
//!   get    "sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews", action: :index
//!   post   "courses/:course_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews", action: :create
//!   post   "sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews", action: :create
//!   delete "courses/:course_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews", action: :destroy
//!   delete "sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews", action: :destroy
//!   post   "courses/:course_id/assignments/:assignment_id/allocate", action: :allocate
//!
//! Canvas controller (`app/controllers/peer_reviews_api_controller.rb`): the
//! `before_action` chain is `get_course_from_section` (the course of the
//! section — 404), `require_context` (404), `require_assignment` (404) and
//! `peer_review_assets` (404 when the reviewer or the student is missing).
//!
//! The request JSON is the Canvas `Api::V1::AssessmentRequest` shape: `id`,
//! `user_id`, `assessor_id`, `asset_id`, `asset_type` and `workflow_state`,
//! plus the `user` and `assessor` members when `include[]=user` is asked
//! for.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// An `assessment_requests` row.
#[derive(Debug, sqlx::FromRow)]
struct AssessmentRequest {
    id: i64,
    user_id: i64,
    assessor_id: i64,
    asset_id: i64,
    asset_type: String,
    workflow_state: String,
}

/// The query parameters of the listing.
#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    include: Option<String>,
}

/// The body of a create request.
#[derive(Debug, Default, Deserialize)]
struct CreateRequest {
    reviewer_id: Option<i64>,
    user_id: Option<i64>,
}

/// Domain router for the Canvas peer-review routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/sections/{section_id}/assignments/{assignment_id}/peer_reviews",
            get(index_section),
        )
        .route(
            "/sections/{section_id}/assignments/{assignment_id}/submissions/{submission_id}/peer_reviews",
            get(index_section_submission).post(create_section).delete(destroy_section),
        )
        .route(
            "/courses/{course_id}/assignments/{assignment_id}/allocate",
            axum::routing::post(allocate),
        )
}

/// `GET /sections/:section_id/assignments/:assignment_id/peer_reviews` —
/// peer_reviews#index.
async fn index_section(
    State(state): State<AppState>,
    Path((section_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let course_id = course_of_section(&state.db, section_id).await?;
    let assignment = load_assignment(&state.db, course_id, assignment_id).await?;
    let requests = requests_for_assignment(&state.db, assignment.id, None).await?;
    let items = requests_json(&state.db, &requests, &auth, &params).await?;
    Ok(Json(items).into_response())
}

/// `GET /sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews`.
async fn index_section_submission(
    State(state): State<AppState>,
    Path((section_id, assignment_id, submission_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Response, ApiError> {
    let course_id = course_of_section(&state.db, section_id).await?;
    let assignment = load_assignment(&state.db, course_id, assignment_id).await?;
    require_submission(&state.db, assignment.id, submission_id).await?;
    let requests = requests_for_assignment(&state.db, assignment.id, Some(submission_id)).await?;
    let items = requests_json(&state.db, &requests, &auth, &params).await?;
    Ok(Json(items).into_response())
}

/// `POST /sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews`.
async fn create_section(
    State(state): State<AppState>,
    Path((section_id, assignment_id, submission_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
    body: Option<Json<CreateRequest>>,
) -> Result<Response, ApiError> {
    let course_id = course_of_section(&state.db, section_id).await?;
    let assignment = load_assignment(&state.db, course_id, assignment_id).await?;
    require_submission(&state.db, assignment.id, submission_id).await?;
    create_request(&state, &assignment, submission_id, &auth, body).await
}

/// `DELETE /sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews`.
async fn destroy_section(
    State(state): State<AppState>,
    Path((section_id, assignment_id, submission_id)): Path<(i64, i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course_id = course_of_section(&state.db, section_id).await?;
    let assignment = load_assignment(&state.db, course_id, assignment_id).await?;
    require_submission(&state.db, assignment.id, submission_id).await?;

    // Canvas deletes the review of the caller for the submission.
    let request: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM assessment_requests \
         WHERE asset_id = $1 AND assessor_id = $2 AND workflow_state <> 'deleted' LIMIT 1",
    )
    .bind(submission_id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    let Some((id,)) = request else {
        return Err(ApiError::NotFound);
    };
    sqlx::query("DELETE FROM assessment_requests WHERE id = $1")
        .bind(id)
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /courses/:course_id/assignments/:assignment_id/allocate` —
/// peer_reviews#allocate.
async fn allocate(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let assignment = load_assignment(&state.db, course_id, assignment_id).await?;

    // Canvas only allocates for an assignment that is visible to the caller.
    let visible: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM assignments a \
         JOIN enrollments e ON e.course_id = a.course_id AND e.user_id = $2 \
         WHERE a.id = $1 AND e.workflow_state <> 'deleted' LIMIT 1",
    )
    .bind(assignment.id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    if visible.is_none() {
        return Err(ApiError::Forbidden);
    }

    // The allocation pairs the students that still owe a review.
    let submissions: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT s.id, s.user_id FROM submissions s \
         WHERE s.assignment_id = $1 AND s.user_id <> $2 \
           AND NOT EXISTS (SELECT 1 FROM assessment_requests ar \
                           WHERE ar.asset_id = s.id AND ar.assessor_id = $2) \
         ORDER BY s.id ASC",
    )
    .bind(assignment.id)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    let mut created = Vec::with_capacity(submissions.len());
    for (submission_id, student_id) in submissions {
        let request: Option<(i64,)> = sqlx::query_as(
            "INSERT INTO assessment_requests \
                 (user_id, assessor_id, asset_id, asset_type, workflow_state) \
             VALUES ($1, $2, $3, 'Submission', 'assigned') \
             ON CONFLICT (asset_id, assessor_id) DO UPDATE SET assessor_id = EXCLUDED.assessor_id \
             RETURNING id",
        )
        .bind(student_id)
        .bind(auth.id)
        .bind(submission_id)
        .fetch_optional(&state.db)
        .await?;
        if let Some((id,)) = request {
            created.push(id);
        }
    }

    let requests = requests_by_ids(&state.db, &created).await?;
    let items = requests_json(&state.db, &requests, &auth, &ListQuery::default()).await?;
    Ok(Json(items).into_response())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The course of a section — 404.
async fn course_of_section(db: &PgPool, section_id: i64) -> Result<i64, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT course_id FROM course_sections WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(section_id)
    .fetch_optional(db)
    .await?;
    row.map(|row| row.0).ok_or(ApiError::NotFound)
}

/// The assignment of a course — 404.
async fn load_assignment(db: &PgPool, course_id: i64, assignment_id: i64) -> Result<i64, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM assignments WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(assignment_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    row.map(|row| row.0).ok_or(ApiError::NotFound)
}

/// The submission of an assignment — 404.
async fn require_submission(
    db: &PgPool,
    assignment_id: i64,
    submission_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM submissions WHERE id = $1 AND assignment_id = $2")
            .bind(submission_id)
            .bind(assignment_id)
            .fetch_optional(db)
            .await?;
    if row.is_some() {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// The review requests of an assignment, optionally of one submission.
async fn requests_for_assignment(
    db: &PgPool,
    assignment_id: i64,
    submission_id: Option<i64>,
) -> Result<Vec<AssessmentRequest>, ApiError> {
    let mut sql = String::from(
        "SELECT ar.id, ar.user_id, ar.assessor_id, ar.asset_id, ar.asset_type, ar.workflow_state \
         FROM assessment_requests ar \
         JOIN submissions s ON s.id = ar.asset_id \
         WHERE s.assignment_id = $1 AND ar.workflow_state <> 'deleted'",
    );
    if submission_id.is_some() {
        sql.push_str(" AND ar.asset_id = $2");
    }
    sql.push_str(" ORDER BY ar.id ASC");

    let rows: Vec<AssessmentRequest> = if let Some(submission_id) = submission_id {
        sqlx::query_as(&sql)
            .bind(assignment_id)
            .bind(submission_id)
            .fetch_all(db)
            .await?
    } else {
        sqlx::query_as(&sql)
            .bind(assignment_id)
            .fetch_all(db)
            .await?
    };
    Ok(rows)
}

/// The review requests of a list of ids.
async fn requests_by_ids(db: &PgPool, ids: &[i64]) -> Result<Vec<AssessmentRequest>, ApiError> {
    let rows: Vec<AssessmentRequest> = sqlx::query_as(
        "SELECT id, user_id, assessor_id, asset_id, asset_type, workflow_state \
         FROM assessment_requests WHERE id = ANY($1) ORDER BY id ASC",
    )
    .bind(ids)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// The creation of a review request.
async fn create_request(
    state: &AppState,
    assignment_id: &i64,
    submission_id: i64,
    auth: &AuthUser,
    body: Option<Json<CreateRequest>>,
) -> Result<Response, ApiError> {
    let body = body.map(|Json(body)| body).unwrap_or_default();

    // The student whose submission is reviewed.
    let student_id = match body.user_id {
        Some(user_id) => user_id,
        None => {
            let row: Option<(i64,)> =
                sqlx::query_as("SELECT user_id FROM submissions WHERE id = $1")
                    .bind(submission_id)
                    .fetch_optional(&state.db)
                    .await?;
            row.map(|row| row.0).ok_or(ApiError::NotFound)?
        }
    };

    // The reviewer; Canvas defaults to the caller.
    let reviewer_id = body.reviewer_id.unwrap_or(auth.id);
    if reviewer_id == student_id {
        return Err(ApiError::BadRequest("Create failed".to_string()));
    }

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO assessment_requests \
             (user_id, assessor_id, asset_id, asset_type, workflow_state) \
         VALUES ($1, $2, $3, 'Submission', 'assigned') \
         ON CONFLICT (asset_id, assessor_id) DO UPDATE SET workflow_state = 'assigned' \
         RETURNING id",
    )
    .bind(student_id)
    .bind(reviewer_id)
    .bind(submission_id)
    .fetch_one(&state.db)
    .await?;

    let requests = requests_by_ids(&state.db, &[id]).await?;
    let items = requests_json(&state.db, &requests, auth, &ListQuery::default()).await?;
    Ok(Json(items.first().cloned().unwrap_or(Value::Null)).into_response())
}

/// The JSON of the review requests — Canvas
/// `Api::V1::AssessmentRequest#assessment_requests_json`.
async fn requests_json(
    db: &PgPool,
    requests: &[AssessmentRequest],
    auth: &AuthUser,
    params: &ListQuery,
) -> Result<Vec<Value>, ApiError> {
    let includes = params.include.as_deref().unwrap_or_default();
    let with_user = includes.contains("user");

    let mut items = Vec::with_capacity(requests.len());
    for request in requests {
        let mut object = serde_json::Map::new();
        object.insert("id".to_string(), json!(request.id));
        object.insert("user_id".to_string(), json!(request.user_id));
        object.insert("assessor_id".to_string(), json!(request.assessor_id));
        object.insert("asset_id".to_string(), json!(request.asset_id));
        object.insert("asset_type".to_string(), json!(request.asset_type));
        object.insert("workflow_state".to_string(), json!(request.workflow_state));

        if with_user {
            object.insert("user".to_string(), user_json(db, request.user_id).await?);
            object.insert(
                "assessor".to_string(),
                user_json(db, request.assessor_id).await?,
            );
        }
        items.push(Value::Object(object));
    }
    let _ = auth;
    Ok(items)
}

/// The Canvas `user_display_json` of a user.
async fn user_json(db: &PgPool, user_id: i64) -> Result<Value, ApiError> {
    let row: Option<(i64, String)> = sqlx::query_as("SELECT id, name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    let Some((id, name)) = row else {
        return Ok(Value::Null);
    };
    Ok(json!({
        "id": id,
        "name": name,
        "pseudonym": Value::Null,
        "avatar_url": Value::Null,
    }))
}
