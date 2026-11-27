//! peer_reviews — Canvas `PeerReviewsApiController` parity.
//!
//! Ground truth (read from `canvas-src`):
//! - `app/controllers/peer_reviews_api_controller.rb` — the controller.
//! - `app/services/peer_review/allocation_service.rb` — the allocation.
//! - `lib/api/v1/assessment_request.rb` — the JSON body of a peer review.
//! - `app/models/assessment_request.rb` — the scopes of the listing.
//! - `spec/apis/v1/peer_reviews_api_spec.rb` — the behaviour contract.
//!
//! Routes served here (the app mounts this router below `/api/v1`):
//!
//! | Verb   | Canvas path                                                                    |
//! |--------|--------------------------------------------------------------------------------|
//! | GET    | `/sections/:section_id/assignments/:assignment_id/peer_reviews`               |
//! | GET    | `/sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews` |
//! | POST   | `/sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews` |
//! | DELETE | `/sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews` |
//! | POST   | `/courses/:course_id/assignments/:assignment_id/allocate`                     |
//!
//! The course scoped peer review routes (`/courses/:course_id/assignments/
//! :assignment_id/peer_reviews` and `.../submissions/:submission_id/peer_reviews`)
//! are already served by `star-assignments` and are deliberately **not**
//! registered again here: `star-http` merges both routers and a duplicate
//! path would break the axum router merge.
//!
//! Canvas `before_action` order — a missing resource is a 404 *before* a
//! missing permission is a 403, an anonymous caller is a 401:
//!
//! 1. `get_course_from_section` — the course of the section (404)
//! 2. `require_context` — the course has to be active (404)
//! 3. `require_assignment` — the assignment has to be active in that course (404)
//! 4. `peer_review_assets` (`create` and `destroy` only) — the submission
//!    (404), the reviewer `user_id` (404) and the reviewed student (404)
//! 5. the action — `authorized_action(@assignment, user, :grade)` (403)
//!
//! ## Required schema
//!
//! The coordinator adds these columns in `star-db/migrations` (the queries
//! below read them):
//!
//! ```sql
//! ALTER TABLE assignments ADD COLUMN IF NOT EXISTS anonymous_peer_reviews BOOLEAN NOT NULL DEFAULT FALSE;
//! ALTER TABLE assignments ADD COLUMN IF NOT EXISTS peer_review_submission_required BOOLEAN NOT NULL DEFAULT TRUE;
//! ALTER TABLE assignments ADD COLUMN IF NOT EXISTS peer_review_across_sections BOOLEAN NOT NULL DEFAULT FALSE;
//! ALTER TABLE assignments ADD COLUMN IF NOT EXISTS intra_group_peer_reviews BOOLEAN NOT NULL DEFAULT FALSE;
//! ALTER TABLE assignments ADD COLUMN IF NOT EXISTS only_visible_to_overrides BOOLEAN NOT NULL DEFAULT FALSE;
//! ALTER TABLE assignments ADD COLUMN IF NOT EXISTS group_category_id BIGINT;
//! ALTER TABLE assignments ADD COLUMN IF NOT EXISTS unlock_at TIMESTAMPTZ;
//! ALTER TABLE assignments ADD COLUMN IF NOT EXISTS lock_at TIMESTAMPTZ;
//! ALTER TABLE submissions ADD COLUMN IF NOT EXISTS group_id BIGINT;
//! ```
//!
//! Canvas stores the peer review dates of an assessor in the overrides of a
//! `peer_review_sub_assignment` (a sub assignment of the peer review
//! assignment). star-labs has no `peer_review_sub_assignments` table, so —
//! like Canvas' own "graceful degradation" when the sub assignment is missing
//! — the peer review start/lock dates are `nil` and those two checks of the
//! allocation service are skipped.
//!
//! ## Known approximations
//!
//! - `AssignmentVisibility::AssignmentVisibilityService` is a large multi
//!   join query (module items, assignment groups, overrides, sections,
//!   groups). The port checks the course enrollment plus the active
//!   assignment overrides of the caller, which covers the documented cases
//!   (ad-hoc, section and group overrides) but not the module item gating.
//! - `students_visible_to` is the set of the active students of the course;
//!   the section restricted enrollment visibility of Canvas is not modelled.
//! - The reviewer and the reviewed student are looked up with the
//!   `user_id` parameter of the request; Canvas reads the same parameter
//!   from the query string or the body.

use axum::{
    body::Bytes,
    extract::{Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::is_admin;

// ---------------------------------------------------------------------------
// Canvas constants
// ---------------------------------------------------------------------------

/// Canvas `FeatureFlags` — the flag that gates the peer review allocation.
const FEATURE_PEER_REVIEW_ALLOCATION: &str = "peer_review_allocation_and_grading";

/// The depth of the account chain that is walked for a feature flag lookup.
const MAX_ACCOUNT_DEPTH: usize = 10;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// An `assessment_requests` row — Canvas `AssessmentRequest`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct AssessmentRequest {
    id: i64,
    user_id: i64,
    assessor_id: i64,
    asset_id: i64,
    asset_type: String,
    workflow_state: String,
}

/// The peer review settings of an assignment.
#[derive(Debug, Clone, sqlx::FromRow)]
struct Assignment {
    id: i64,
    course_id: i64,
    peer_reviews: bool,
    peer_review_count: i32,
    anonymous_peer_reviews: bool,
    peer_review_submission_required: bool,
    peer_review_across_sections: bool,
    intra_group_peer_reviews: bool,
    only_visible_to_overrides: bool,
    group_category_id: Option<i64>,
    unlock_at: Option<DateTime<Utc>>,
    lock_at: Option<DateTime<Utc>>,
}

/// The resolved Canvas context of a peer review request.
#[derive(Debug, Clone)]
struct PeerReviewScope {
    course_id: i64,
    assignment: Assignment,
}

/// The `include[]` parameters of the peer review routes.
#[derive(Debug, Default)]
struct PeerReviewQuery {
    include_user: bool,
    include_submission_comments: bool,
}

/// A submission that can be allocated for review.
#[derive(Debug, Clone)]
struct CandidateSubmission {
    id: i64,
    user_id: i64,
    submitted_at: DateTime<Utc>,
    review_count: i64,
}

/// Canvas `AllocationRule` rows of an assessor, grouped by their category.
#[derive(Debug, Default, Clone)]
struct AllocationRules {
    must_review: Vec<i64>,
    should_review: Vec<i64>,
    must_not_review: Vec<i64>,
    should_not_review: Vec<i64>,
}

impl AllocationRules {
    /// Canvas `submission_priority` — 1 (must review), 2 (should review),
    /// 3 (regular) and 4 (should not review).
    fn priority(&self, user_id: i64) -> i32 {
        if self.must_review.contains(&user_id) {
            1
        } else if self.should_review.contains(&user_id) {
            2
        } else if self.should_not_review.contains(&user_id) {
            4
        } else {
            3
        }
    }
}

/// Canvas `PeerReview::AllocationService#error_result` — the status and the
/// message of the `{"errors": {"base": ...}}` body.
#[derive(Debug)]
struct AllocationFailure {
    status: StatusCode,
    message: String,
}

impl AllocationFailure {
    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

impl From<ApiError> for AllocationFailure {
    fn from(error: ApiError) -> Self {
        Self {
            status: error.status(),
            message: error.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Canvas response bodies
// ---------------------------------------------------------------------------

/// A JSON body with an explicit status code.
fn json_response(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

/// Canvas `ApplicationController#api_error_json` for a missing resource.
fn not_found() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        json!({"errors": [{"message": "The specified resource does not exist."}]}),
    )
}

/// Canvas `ApplicationController#render_json_unauthorized` for an
/// authenticated but unauthorized caller.
fn unauthorized() -> Response {
    json_response(
        StatusCode::FORBIDDEN,
        json!({
            "status": "unauthorized",
            "errors": [{"message": "user not authorized to perform that action"}],
        }),
    )
}

/// Canvas `render json: { errors: { base: message } }`.
fn base_error(status: StatusCode, message: &str) -> Response {
    json_response(status, json!({"errors": {"base": message}}))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The Canvas `PeerReviewsApiController` routes of this slice.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/sections/{section_id}/assignments/{assignment_id}/peer_reviews",
            get(index_section),
        )
        .route(
            "/sections/{section_id}/assignments/{assignment_id}/submissions/{submission_id}/peer_reviews",
            get(index_section_submission)
                .post(create_section)
                .delete(destroy_section),
        )
        .route(
            "/courses/{course_id}/assignments/{assignment_id}/allocate",
            post(allocate),
        )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /sections/:section_id/assignments/:assignment_id/peer_reviews` —
/// Canvas `peer_reviews#index`.
async fn index_section(
    State(state): State<AppState>,
    Path((section_id, assignment_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let scope = section_scope(&state.db, section_id, assignment_id).await?;
    let requests = select_requests(&state.db, &scope, None, &auth).await?;
    let query = parse_query(raw_query.as_deref());
    let items = requests_json(&state.db, &requests, &scope, &auth, &query).await?;
    Ok(Json(items).into_response())
}

/// `GET /sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews`
/// — Canvas `peer_reviews#index`. A submission that does not belong to the
/// assignment is not an error here (the scope simply does not match), Canvas
/// answers with an empty array.
async fn index_section_submission(
    State(state): State<AppState>,
    Path((section_id, assignment_id, submission_id)): Path<(i64, i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let scope = section_scope(&state.db, section_id, assignment_id).await?;
    let requests = select_requests(&state.db, &scope, Some(submission_id), &auth).await?;
    let query = parse_query(raw_query.as_deref());
    let items = requests_json(&state.db, &requests, &scope, &auth, &query).await?;
    Ok(Json(items).into_response())
}

/// `POST /sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews`
/// — Canvas `peer_reviews#create`.
async fn create_section(
    State(state): State<AppState>,
    Path((section_id, assignment_id, submission_id)): Path<(i64, i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let scope = section_scope(&state.db, section_id, assignment_id).await?;

    // Canvas `peer_review_assets`.
    let student_id = require_submission(&state.db, scope.assignment.id, submission_id).await?;
    let Some(reviewer_id) = parse_user_id(&body, raw_query.as_deref()) else {
        return Ok(not_found());
    };
    if !is_visible_student(&state.db, scope.course_id, reviewer_id).await? {
        return Ok(not_found());
    }
    if !is_visible_student(&state.db, scope.course_id, student_id).await? {
        return Ok(not_found());
    }

    // Canvas: a reviewer may not review their own submission.
    if reviewer_id == student_id {
        return Ok(base_error(StatusCode::BAD_REQUEST, "Create failed"));
    }

    // Canvas `authorized_action(@assignment, @current_user, :grade)`.
    if !can_grade(&state.db, scope.course_id, auth.id, &auth).await? {
        return Ok(unauthorized());
    }

    let request =
        assign_peer_review(&state.db, scope.assignment.id, reviewer_id, student_id).await?;
    let query = parse_query(raw_query.as_deref());
    let items = requests_json(&state.db, &[request], &scope, &auth, &query).await?;
    Ok(Json(items.into_iter().next().unwrap_or(Value::Null)).into_response())
}

/// `DELETE /sections/:section_id/assignments/:assignment_id/submissions/:submission_id/peer_reviews`
/// — Canvas `peer_reviews#destroy`.
async fn destroy_section(
    State(state): State<AppState>,
    Path((section_id, assignment_id, submission_id)): Path<(i64, i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
    body: Bytes,
) -> Result<Response, ApiError> {
    let scope = section_scope(&state.db, section_id, assignment_id).await?;

    // Canvas `peer_review_assets`.
    let student_id = require_submission(&state.db, scope.assignment.id, submission_id).await?;
    let Some(reviewer_id) = parse_user_id(&body, raw_query.as_deref()) else {
        return Ok(not_found());
    };
    if !is_visible_student(&state.db, scope.course_id, reviewer_id).await? {
        return Ok(not_found());
    }
    if !is_visible_student(&state.db, scope.course_id, student_id).await? {
        return Ok(not_found());
    }

    // Canvas `authorized_action(@assignment, @current_user, :grade)`.
    if !can_grade(&state.db, scope.course_id, auth.id, &auth).await? {
        return Ok(unauthorized());
    }

    let request: Option<AssessmentRequest> = sqlx::query_as(
        "SELECT id, user_id, assessor_id, asset_id, asset_type, workflow_state \
         FROM assessment_requests \
         WHERE asset_id = $1 AND assessor_id = $2 AND user_id = $3 LIMIT 1",
    )
    .bind(submission_id)
    .bind(reviewer_id)
    .bind(student_id)
    .fetch_optional(&state.db)
    .await?;

    let Some(request) = request else {
        return Ok(base_error(StatusCode::BAD_REQUEST, "Delete failed"));
    };

    sqlx::query("DELETE FROM assessment_requests WHERE id = $1")
        .bind(request.id)
        .execute(&state.db)
        .await?;

    // Canvas renders the destroyed request.
    let items = requests_json(
        &state.db,
        &[request],
        &scope,
        &auth,
        &PeerReviewQuery::default(),
    )
    .await?;
    Ok(Json(items.into_iter().next().unwrap_or(Value::Null)).into_response())
}

/// `POST /courses/:course_id/assignments/:assignment_id/allocate` — Canvas
/// `peer_reviews#allocate`.
async fn allocate(
    State(state): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    RawQuery(raw_query): RawQuery,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    // Canvas `require_context` — the course has to exist and to be active.
    load_course(&state.db, course_id).await?;
    let assignment = load_assignment(&state.db, course_id, assignment_id).await?;

    // Canvas: the assignment has to be visible to the caller.
    if !assignment_visible_to(&state.db, &assignment, auth.id).await? {
        return Ok(base_error(
            StatusCode::FORBIDDEN,
            "Assignment is not assigned to you",
        ));
    }

    let requests = match allocate_reviews(&state.db, &assignment, auth.id).await {
        Ok(requests) => requests,
        Err(failure) => return Ok(base_error(failure.status, &failure.message)),
    };

    let scope = PeerReviewScope {
        course_id,
        assignment,
    };
    let query = parse_query(raw_query.as_deref());
    let items = requests_json(&state.db, &requests, &scope, &auth, &query).await?;
    Ok(Json(items).into_response())
}

// ---------------------------------------------------------------------------
// Canvas context
// ---------------------------------------------------------------------------

/// The course of a section — Canvas `get_course_from_section` (404).
async fn course_of_section(db: &PgPool, section_id: i64) -> Result<i64, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT course_id FROM course_sections WHERE id = $1")
        .bind(section_id)
        .fetch_optional(db)
        .await?;
    row.map(|row| row.0).ok_or(ApiError::NotFound)
}

/// An active course — Canvas `Course.active` (404).
async fn load_course(db: &PgPool, course_id: i64) -> Result<i64, ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    row.map(|row| row.0).ok_or(ApiError::NotFound)
}

/// An active assignment of a course — Canvas `@context.assignments.active`
/// (404).
async fn load_assignment(
    db: &PgPool,
    course_id: i64,
    assignment_id: i64,
) -> Result<Assignment, ApiError> {
    let assignment: Option<Assignment> = sqlx::query_as(
        "SELECT id, course_id, peer_reviews, peer_review_count, anonymous_peer_reviews, \
                peer_review_submission_required, peer_review_across_sections, \
                intra_group_peer_reviews, only_visible_to_overrides, group_category_id, \
                unlock_at, lock_at \
         FROM assignments \
         WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'",
    )
    .bind(assignment_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    assignment.ok_or(ApiError::NotFound)
}

/// The context of a section scoped peer review request.
async fn section_scope(
    db: &PgPool,
    section_id: i64,
    assignment_id: i64,
) -> Result<PeerReviewScope, ApiError> {
    let course_id = course_of_section(db, section_id).await?;
    load_course(db, course_id).await?;
    let assignment = load_assignment(db, course_id, assignment_id).await?;
    Ok(PeerReviewScope {
        course_id,
        assignment,
    })
}

/// The owner of a submission of the assignment — Canvas
/// `@assignment.submissions.find(params[:submission_id])` (404).
async fn require_submission(
    db: &PgPool,
    assignment_id: i64,
    submission_id: i64,
) -> Result<i64, ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT user_id FROM submissions WHERE id = $1 AND assignment_id = $2")
            .bind(submission_id)
            .bind(assignment_id)
            .fetch_optional(db)
            .await?;
    row.map(|row| row.0).ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// Canvas `authorized_action(@assignment, user, :grade)` — a course teacher
/// (with an active enrollment) or an administrator.
async fn can_grade(
    db: &PgPool,
    course_id: i64,
    user_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' \
           AND workflow_state = 'active' LIMIT 1",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Canvas `@context.students_visible_to(@current_user)` — the user has to be
/// an active student of the course.
async fn is_visible_student(db: &PgPool, course_id: i64, user_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role = 'student' \
           AND workflow_state = 'active' LIMIT 1",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// Canvas `AssignmentVisibility::AssignmentVisibilityService
/// .visible_assignment_ids_in_course_by_user` — the caller has to be an
/// active student of the course and, when the assignment is restricted to
/// overrides, one of the overrides has to target the caller.
async fn assignment_visible_to(
    db: &PgPool,
    assignment: &Assignment,
    user_id: i64,
) -> Result<bool, ApiError> {
    if !is_visible_student(db, assignment.course_id, user_id).await? {
        return Ok(false);
    }
    if !assignment.only_visible_to_overrides {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM assignment_overrides o \
         WHERE o.assignment_id = $1 AND o.workflow_state <> 'deleted' \
           AND ( $2 = ANY(o.student_ids) \
              OR o.course_section_id IN ( \
                   SELECT e.course_section_id FROM enrollments e \
                   WHERE e.user_id = $2 AND e.course_id = $3 \
                     AND e.course_section_id IS NOT NULL \
                     AND e.workflow_state = 'active') \
              OR o.group_id IN ( \
                   SELECT gm.group_id FROM group_memberships gm \
                   JOIN groups g ON g.id = gm.group_id \
                   WHERE gm.user_id = $2 AND gm.workflow_state <> 'deleted' \
                     AND g.workflow_state <> 'deleted' AND g.course_id = $3) ) \
         LIMIT 1",
    )
    .bind(assignment.id)
    .bind(user_id)
    .bind(assignment.course_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Peer reviews
// ---------------------------------------------------------------------------

/// Canvas `AssessmentRequest.for_assignment` (and `for_assessee` for a
/// caller that may not grade, `for_asset` for a single submission).
async fn select_requests(
    db: &PgPool,
    scope: &PeerReviewScope,
    submission_id: Option<i64>,
    auth: &AuthUser,
) -> Result<Vec<AssessmentRequest>, ApiError> {
    let can_grade = can_grade(db, scope.course_id, auth.id, auth).await?;
    let assessee: Option<i64> = if can_grade { None } else { Some(auth.id) };
    let rows: Vec<AssessmentRequest> = sqlx::query_as(
        "SELECT ar.id, ar.user_id, ar.assessor_id, ar.asset_id, ar.asset_type, ar.workflow_state \
         FROM assessment_requests ar \
         JOIN submissions s ON s.id = ar.asset_id \
         WHERE s.assignment_id = $1 \
           AND ($2::bigint IS NULL OR ar.asset_id = $2) \
           AND ($3::bigint IS NULL OR ar.user_id = $3) \
         ORDER BY ar.id ASC",
    )
    .bind(scope.assignment.id)
    .bind(submission_id)
    .bind(assessee)
    .fetch_all(db)
    .await?;
    Ok(rows)
}

/// Canvas `Assignment#assign_peer_review` — the assessment request of the
/// reviewer for the submission of the reviewed student.
async fn assign_peer_review(
    db: &PgPool,
    assignment_id: i64,
    assessor_id: i64,
    assessee_id: i64,
) -> Result<AssessmentRequest, ApiError> {
    // Canvas `find_or_create_submission` for the reviewer and the reviewee.
    let asset_id = ensure_submission(db, assignment_id, assessee_id).await?;
    ensure_submission(db, assignment_id, assessor_id).await?;

    // Canvas `assign_assessor` reuses an existing request.
    let request: AssessmentRequest = sqlx::query_as(
        "INSERT INTO assessment_requests \
             (user_id, assessor_id, asset_id, asset_type, workflow_state) \
         VALUES ($1, $2, $3, 'Submission', 'assigned') \
         ON CONFLICT (asset_id, assessor_id) DO UPDATE \
             SET workflow_state = assessment_requests.workflow_state \
         RETURNING id, user_id, assessor_id, asset_id, asset_type, workflow_state",
    )
    .bind(assessee_id)
    .bind(assessor_id)
    .bind(asset_id)
    .fetch_one(db)
    .await?;
    Ok(request)
}

/// Canvas `find_or_create_submission`.
async fn ensure_submission(db: &PgPool, assignment_id: i64, user_id: i64) -> Result<i64, ApiError> {
    sqlx::query(
        "INSERT INTO submissions (assignment_id, user_id) VALUES ($1, $2) \
         ON CONFLICT (assignment_id, user_id) DO NOTHING",
    )
    .bind(assignment_id)
    .bind(user_id)
    .execute(db)
    .await?;

    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM submissions WHERE assignment_id = $1 AND user_id = $2")
            .bind(assignment_id)
            .bind(user_id)
            .fetch_optional(db)
            .await?;
    row.map(|row| row.0).ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Allocation (Canvas `PeerReview::AllocationService`)
// ---------------------------------------------------------------------------

/// Canvas `PeerReview::AllocationService#allocate`.
async fn allocate_reviews(
    db: &PgPool,
    assignment: &Assignment,
    assessor_id: i64,
) -> Result<Vec<AssessmentRequest>, AllocationFailure> {
    // The feature flag has to be enabled on the course.
    if !feature_enabled(db, assignment.course_id, FEATURE_PEER_REVIEW_ALLOCATION).await? {
        return Err(AllocationFailure::forbidden(
            "Peer review allocation and grading feature is not enabled",
        ));
    }

    // The assignment has to have peer reviews enabled.
    if !assignment.peer_reviews {
        return Err(AllocationFailure::forbidden(
            "Assignment does not have peer reviews enabled",
        ));
    }

    // The assessor needs an active student enrollment.
    if !is_visible_student(db, assignment.course_id, assessor_id).await? {
        return Err(AllocationFailure::forbidden(
            "You cannot receive new peer review allocations because your enrollment is not active",
        ));
    }

    // The assessor has to have submitted when the assignment demands it.
    if assignment.peer_review_submission_required {
        let submitted: Option<(i32,)> = sqlx::query_as(
            "SELECT 1 FROM submissions \
             WHERE assignment_id = $1 AND user_id = $2 AND submission_type IS NOT NULL LIMIT 1",
        )
        .bind(assignment.id)
        .bind(assessor_id)
        .fetch_optional(db)
        .await
        .map_err(ApiError::from)?;
        if submitted.is_none() {
            return Err(AllocationFailure::forbidden(
                "You must submit the assignment before requesting peer reviews",
            ));
        }
    }

    // Canvas `low_level_locked_for?`.
    let now = Utc::now();
    if let Some(unlock_at) = assignment.unlock_at {
        if unlock_at > now {
            return Err(AllocationFailure::forbidden(format!(
                "The assignment is locked until {}",
                format_time(&unlock_at)
            )));
        }
    }
    if let Some(lock_at) = assignment.lock_at {
        if lock_at < now {
            return Err(AllocationFailure::forbidden(format!(
                "This assignment is no longer available as of {}",
                format_time(&lock_at)
            )));
        }
    }

    // The peer review start and lock dates come from the overrides of the
    // peer review sub assignment. star-labs has no
    // `peer_review_sub_assignments` table, so (like Canvas when the sub
    // assignment is missing) they are nil and the checks are skipped.

    // The assessor may not have more reviews than the assignment asks for.
    let total_assigned = count_assessment_requests(db, assignment.id, assessor_id).await?;
    if total_assigned >= i64::from(assignment.peer_review_count) {
        return Err(AllocationFailure::forbidden(
            "You have been assigned all required peer reviews",
        ));
    }

    let ongoing = ongoing_requests(db, assignment.id, assessor_id).await?;
    let rules = allocation_rules(db, assignment.id, assessor_id).await?;
    let mut allocated: Vec<AssessmentRequest> = Vec::new();
    let mut remaining = assignment.peer_review_count - total_assigned as i32;

    // The "must review" rules are enforced first.
    if remaining > 0 {
        let assigned_users: Vec<i64> = ongoing.iter().map(|request| request.user_id).collect();
        let mut must_review: Vec<i64> = rules
            .must_review
            .iter()
            .copied()
            .filter(|user_id| !assigned_users.contains(user_id))
            .collect();
        if !assignment.peer_review_across_sections {
            must_review =
                restrict_to_assessor_sections(db, assignment.course_id, assessor_id, must_review)
                    .await?;
        }
        for user_id in must_review {
            let request = assign_peer_review(db, assignment.id, assessor_id, user_id).await?;
            allocated.push(request);
        }
        remaining -= allocated.len() as i32;
    }

    // The remaining slots are filled with the available submissions.
    if remaining > 0 {
        let available = available_submissions(db, assignment, assessor_id, &rules).await?;
        if total_assigned == 0 && allocated.is_empty() && available.is_empty() {
            return Err(AllocationFailure::bad_request(
                "There are no peer reviews available to allocate to you.",
            ));
        }
        for submission in select_submissions(&available, &rules, remaining as usize) {
            let request =
                assign_peer_review(db, assignment.id, assessor_id, submission.user_id).await?;
            allocated.push(request);
        }
    }

    let mut all = ongoing;
    all.extend(allocated);
    Ok(all)
}

/// Canvas `AssessmentRequest.for_assignment(...).for_assessor(...).incomplete`.
async fn ongoing_requests(
    db: &PgPool,
    assignment_id: i64,
    assessor_id: i64,
) -> Result<Vec<AssessmentRequest>, AllocationFailure> {
    let rows: Vec<AssessmentRequest> = sqlx::query_as(
        "SELECT ar.id, ar.user_id, ar.assessor_id, ar.asset_id, ar.asset_type, ar.workflow_state \
         FROM assessment_requests ar \
         JOIN submissions s ON s.id = ar.asset_id \
         WHERE s.assignment_id = $1 AND ar.assessor_id = $2 \
           AND ar.workflow_state = 'assigned' \
         ORDER BY ar.id ASC",
    )
    .bind(assignment_id)
    .bind(assessor_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)?;
    Ok(rows)
}

/// Canvas `AssessmentRequest.for_assignment(...).for_assessor(...).count`.
async fn count_assessment_requests(
    db: &PgPool,
    assignment_id: i64,
    assessor_id: i64,
) -> Result<i64, AllocationFailure> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM assessment_requests ar \
         JOIN submissions s ON s.id = ar.asset_id \
         WHERE s.assignment_id = $1 AND ar.assessor_id = $2",
    )
    .bind(assignment_id)
    .bind(assessor_id)
    .fetch_one(db)
    .await
    .map_err(ApiError::from)?;
    Ok(count)
}

/// Canvas `AllocationRule.active.where(assignment:, assessor_id:)`.
async fn allocation_rules(
    db: &PgPool,
    assignment_id: i64,
    assessor_id: i64,
) -> Result<AllocationRules, AllocationFailure> {
    let rows: Vec<(i64, bool, bool)> = sqlx::query_as(
        "SELECT assessee_id, must_review, review_permitted FROM allocation_rules \
         WHERE assignment_id = $1 AND assessor_id = $2 AND workflow_state = 'active'",
    )
    .bind(assignment_id)
    .bind(assessor_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)?;

    let mut rules = AllocationRules::default();
    for (assessee_id, must_review, review_permitted) in rows {
        match (must_review, review_permitted) {
            (true, true) => rules.must_review.push(assessee_id),
            (false, true) => rules.should_review.push(assessee_id),
            (true, false) => rules.must_not_review.push(assessee_id),
            (false, false) => rules.should_not_review.push(assessee_id),
        }
    }
    Ok(rules)
}

/// The sections of the assessor — Canvas `assessor_section_ids`.
async fn assessor_section_ids(
    db: &PgPool,
    course_id: i64,
    assessor_id: i64,
) -> Result<Vec<i64>, AllocationFailure> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT course_section_id FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role = 'student' \
           AND workflow_state = 'active' AND course_section_id IS NOT NULL",
    )
    .bind(assessor_id)
    .bind(course_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)?;
    Ok(rows.into_iter().map(|row| row.0).collect())
}

/// Canvas: the "must review" users are restricted to the sections of the
/// assessor when `peer_review_across_sections` is off.
async fn restrict_to_assessor_sections(
    db: &PgPool,
    course_id: i64,
    assessor_id: i64,
    user_ids: Vec<i64>,
) -> Result<Vec<i64>, AllocationFailure> {
    let section_ids = assessor_section_ids(db, course_id, assessor_id).await?;
    if section_ids.is_empty() {
        return Ok(user_ids);
    }
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT user_id FROM enrollments \
         WHERE course_id = $1 AND role = 'student' AND workflow_state = 'active' \
           AND course_section_id = ANY($2::bigint[])",
    )
    .bind(course_id)
    .bind(&section_ids)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)?;

    let section_users: Vec<i64> = rows.into_iter().map(|row| row.0).collect();
    Ok(user_ids
        .into_iter()
        .filter(|user_id| section_users.contains(user_id))
        .collect())
}

/// Canvas `preload_available_submissions`.
async fn available_submissions(
    db: &PgPool,
    assignment: &Assignment,
    assessor_id: i64,
    rules: &AllocationRules,
) -> Result<Vec<CandidateSubmission>, AllocationFailure> {
    // The users the assessor already reviews and the users of the "must not
    // review" rules are no candidates.
    let mut excluded: Vec<i64> = rules.must_not_review.clone();
    let assigned: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT ar.user_id FROM assessment_requests ar \
         JOIN submissions s ON s.id = ar.asset_id \
         WHERE s.assignment_id = $1 AND ar.assessor_id = $2",
    )
    .bind(assignment.id)
    .bind(assessor_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)?;
    excluded.extend(assigned.into_iter().map(|row| row.0));

    let rows: Vec<(i64, i64, DateTime<Utc>, i64)> = sqlx::query_as(
        "SELECT s.id, s.user_id, s.submitted_at, \
                (SELECT COUNT(*) FROM assessment_requests ar WHERE ar.asset_id = s.id) \
         FROM submissions s \
         WHERE s.assignment_id = $1 \
           AND s.workflow_state <> 'deleted' \
           AND s.submission_type IS NOT NULL \
           AND s.user_id <> $2 \
           AND NOT EXISTS (SELECT 1 FROM assessment_requests ar \
                           WHERE ar.asset_id = s.id AND ar.assessor_id = $2) \
           AND NOT (s.user_id = ANY($3::bigint[]))",
    )
    .bind(assignment.id)
    .bind(assessor_id)
    .bind(&excluded)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)?;

    let mut candidates: Vec<CandidateSubmission> = rows
        .into_iter()
        .map(
            |(id, user_id, submitted_at, review_count)| CandidateSubmission {
                id,
                user_id,
                submitted_at,
                review_count,
            },
        )
        .collect();

    // The submissions of the sections of the assessor, unless the peer
    // reviews may cross the sections.
    if !assignment.peer_review_across_sections {
        let section_ids = assessor_section_ids(db, assignment.course_id, assessor_id).await?;
        if !section_ids.is_empty() {
            let section_users: Vec<(i64,)> = sqlx::query_as(
                "SELECT DISTINCT user_id FROM enrollments \
                 WHERE course_id = $1 AND role = 'student' AND workflow_state = 'active' \
                   AND course_section_id = ANY($2::bigint[])",
            )
            .bind(assignment.course_id)
            .bind(&section_ids)
            .fetch_all(db)
            .await
            .map_err(ApiError::from)?;
            let section_users: Vec<i64> = section_users.into_iter().map(|row| row.0).collect();
            candidates.retain(|candidate| section_users.contains(&candidate.user_id));
        }
    }

    // The submissions of the group of the assessor are no candidates, unless
    // the assignment allows the intra group peer reviews.
    if assignment.group_category_id.is_some() && !assignment.intra_group_peer_reviews {
        let assessor_group: Option<(Option<i64>,)> = sqlx::query_as(
            "SELECT group_id FROM submissions WHERE assignment_id = $1 AND user_id = $2 LIMIT 1",
        )
        .bind(assignment.id)
        .bind(assessor_id)
        .fetch_optional(db)
        .await
        .map_err(ApiError::from)?;
        if let Some((Some(group_id),)) = assessor_group {
            let candidate_ids: Vec<i64> = candidates.iter().map(|candidate| candidate.id).collect();
            let groups: Vec<(i64, Option<i64>)> =
                sqlx::query_as("SELECT id, group_id FROM submissions WHERE id = ANY($1::bigint[])")
                    .bind(&candidate_ids)
                    .fetch_all(db)
                    .await
                    .map_err(ApiError::from)?;
            // Canvas `where.not(group_id: group_id)` — a `group_id <> group`
            // comparison, which drops the submissions without a group too.
            let excluded: Vec<i64> = groups
                .into_iter()
                .filter(|(_, candidate_group)| {
                    candidate_group.is_none() || *candidate_group == Some(group_id)
                })
                .map(|(id, _)| id)
                .collect();
            candidates.retain(|candidate| !excluded.contains(&candidate.id));
        }
    }

    Ok(candidates)
}

/// Canvas `select_submissions_to_allocate` — the priority tier, then the
/// number of reviews, then the submission date.
fn select_submissions(
    candidates: &[CandidateSubmission],
    rules: &AllocationRules,
    count: usize,
) -> Vec<CandidateSubmission> {
    let mut sorted = candidates.to_vec();
    sorted.sort_by(|left, right| {
        let left_key = (
            rules.priority(left.user_id),
            left.review_count,
            left.submitted_at,
        );
        let right_key = (
            rules.priority(right.user_id),
            right.review_count,
            right.submitted_at,
        );
        left_key.cmp(&right_key)
    });
    sorted.truncate(count);
    sorted
}

// ---------------------------------------------------------------------------
// Feature flags
// ---------------------------------------------------------------------------

/// Canvas `@context.feature_enabled?(:peer_review_allocation_and_grading)`.
async fn feature_enabled(db: &PgPool, course_id: i64, feature: &str) -> Result<bool, ApiError> {
    let course_state: Option<(String,)> = sqlx::query_as(
        "SELECT state FROM feature_flags \
         WHERE context_type = 'Course' AND context_id = $1 AND feature = $2 LIMIT 1",
    )
    .bind(course_id)
    .bind(feature)
    .fetch_optional(db)
    .await?;
    if let Some((state,)) = course_state {
        return Ok(state == "enabled");
    }

    for account_id in course_account_chain(db, course_id).await? {
        let account_state: Option<(String,)> = sqlx::query_as(
            "SELECT state FROM feature_flags \
             WHERE context_type = 'Account' AND context_id = $1 AND feature = $2 LIMIT 1",
        )
        .bind(account_id)
        .bind(feature)
        .fetch_optional(db)
        .await?;
        if let Some((state,)) = account_state {
            return Ok(state == "enabled");
        }
    }
    Ok(false)
}

/// The account chain of a course — the course account and its parents.
async fn course_account_chain(db: &PgPool, course_id: i64) -> Result<Vec<i64>, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(db)
            .await?;
    let Some((account_id,)) = row else {
        return Ok(Vec::new());
    };
    let Some(account_id) = account_id else {
        return Ok(Vec::new());
    };

    let mut chain = Vec::new();
    let mut current = account_id;
    for _ in 0..MAX_ACCOUNT_DEPTH {
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(current)
                .fetch_optional(db)
                .await?;
        chain.push(current);
        match parent {
            Some((Some(parent_id),)) => current = parent_id,
            _ => break,
        }
    }
    Ok(chain)
}

// ---------------------------------------------------------------------------
// JSON bodies
// ---------------------------------------------------------------------------

/// Canvas `Api::V1::AssessmentRequest#assessment_requests_json`.
async fn requests_json(
    db: &PgPool,
    requests: &[AssessmentRequest],
    scope: &PeerReviewScope,
    auth: &AuthUser,
    query: &PeerReviewQuery,
) -> Result<Vec<Value>, ApiError> {
    let can_grade = can_grade(db, scope.course_id, auth.id, auth).await?;
    // Canvas hides the assessor of an anonymous peer review from everybody
    // that may not grade.
    let anonymous = scope.assignment.anonymous_peer_reviews && !can_grade;

    let mut items = Vec::with_capacity(requests.len());
    for request in requests {
        let mut object = Map::new();
        object.insert("id".to_string(), json!(request.id));
        object.insert("user_id".to_string(), json!(request.user_id));
        if !anonymous {
            object.insert("assessor_id".to_string(), json!(request.assessor_id));
        }
        object.insert("asset_id".to_string(), json!(request.asset_id));
        object.insert("asset_type".to_string(), json!(request.asset_type));
        object.insert("workflow_state".to_string(), json!(request.workflow_state));

        if query.include_user {
            object.insert("user".to_string(), user_display(db, request.user_id).await?);
            if !anonymous {
                object.insert(
                    "assessor".to_string(),
                    user_display(db, request.assessor_id).await?,
                );
            }
        }

        if query.include_submission_comments {
            let comments =
                submission_comments_json(db, request.asset_id, anonymous, auth.id, can_grade)
                    .await?;
            object.insert("submission_comments".to_string(), comments);
        }

        items.push(Value::Object(object));
    }
    Ok(items)
}

/// Canvas `Api::V1::SubmissionComment#submission_comments_json`.
async fn submission_comments_json(
    db: &PgPool,
    submission_id: i64,
    anonymous: bool,
    viewer_id: i64,
    can_grade: bool,
) -> Result<Value, ApiError> {
    let rows: Vec<(i64, i64, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT id, author_id, comment, created_at FROM submission_comments \
         WHERE submission_id = $1 AND workflow_state <> 'deleted' ORDER BY id ASC",
    )
    .bind(submission_id)
    .fetch_all(db)
    .await?;

    let mut comments = Vec::with_capacity(rows.len());
    for (id, author_id, comment, created_at) in rows {
        // Canvas `SubmissionComment#can_read_author?`.
        let readable = author_id == viewer_id || !anonymous || can_grade;
        if readable {
            comments.push(json!({
                "id": id,
                "author_id": author_id,
                "author_name": user_name(db, author_id).await?,
                "created_at": created_at,
                "edited_at": Value::Null,
                "comment": comment,
                "attempt": Value::Null,
                "author": user_display(db, author_id).await?,
            }));
        } else {
            comments.push(json!({
                "id": id,
                "author_id": Value::Null,
                "author_name": "Anonymous User",
                "created_at": created_at,
                "edited_at": Value::Null,
                "comment": comment,
                "attempt": Value::Null,
                "author": Value::Object(Map::new()),
            }));
        }
    }
    Ok(json!(comments))
}

/// Canvas `user_display_json` — the house shape is `id` and `display_name`.
async fn user_display(db: &PgPool, user_id: i64) -> Result<Value, ApiError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    Ok(row
        .map(|(name,)| json!({"id": user_id, "display_name": name}))
        .unwrap_or(Value::Null))
}

/// The name of a user — Canvas `SubmissionComment#author_name`.
async fn user_name(db: &PgPool, user_id: i64) -> Result<Value, ApiError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    Ok(row
        .map(|(name,)| Value::String(name))
        .unwrap_or(Value::Null))
}

// ---------------------------------------------------------------------------
// Query parsing
// ---------------------------------------------------------------------------

/// The `include[]` parameters of the request.
fn parse_query(raw_query: Option<&str>) -> PeerReviewQuery {
    let mut query = PeerReviewQuery::default();
    for pair in raw_query.unwrap_or_default().split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let key = decode_component(raw_key).to_ascii_lowercase();
        let key = key.strip_suffix("[]").unwrap_or(key.as_str()).to_string();
        let value = decode_component(raw_value);
        if key.as_str() == "include" {
            if value == "user" {
                query.include_user = true;
            } else if value == "submission_comments" {
                query.include_submission_comments = true;
            }
        }
    }
    query
}

/// Canvas `params[:user_id]` — the reviewer of the peer review. The parameter
/// is read from the body (JSON or form encoded) or from the query string.
fn parse_user_id(body: &[u8], raw_query: Option<&str>) -> Option<i64> {
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        if let Some(id) = value.get("user_id").and_then(json_id) {
            return Some(id);
        }
    }

    let encoded = String::from_utf8_lossy(body);
    for pair in encoded.split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        if decode_component(raw_key).eq_ignore_ascii_case("user_id") {
            if let Some(id) = json_id(&Value::String(decode_component(raw_value))) {
                return Some(id);
            }
        }
    }

    for pair in raw_query.unwrap_or_default().split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        if decode_component(raw_key).eq_ignore_ascii_case("user_id") {
            if let Some(id) = json_id(&Value::String(decode_component(raw_value))) {
                return Some(id);
            }
        }
    }
    None
}

/// An integer of a JSON value.
fn json_id(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Percent-decode a query component.
fn decode_component(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 3 <= bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        index += 3;
                    }
                    Err(_) => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Canvas renders a timestamp with `Time#to_s`.
fn format_time(value: &DateTime<Utc>) -> String {
    value.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}
