//! Outcomes API — Canvas `OutcomesApiController` direct-outcome routes
//! (parity gap).
//!
//! Routes:
//!   - GET    /outcomes/{id}                    (show)
//!   - PUT    /outcomes/{id}                    (update)
//!   - DELETE /outcomes/{id}                    (destroy)
//!   - GET    /courses/{courseId}/outcome_alignments (outcome_alignments)
//!
//! Canvas semantics:
//!   - `get_outcome` (show/update/destroy): the outcome must be active —
//!     404 otherwise.
//!   - show: `read` on the outcome; update: `update` right; destroy:
//!     soft-delete then JSON of the deleted outcome.
//!   - update (process_params): title, display_name, description,
//!     vendor_guid, calculation_method, calculation_int; mastery_points
//!     updates the criterion. Save failures → 400 (`render ... :bad_request`).
//!   - outcome_alignments: `assignment_id` alone → all outcome alignments of
//!     that assignment (manage/view-all grades); `student_id` → the
//!     student's aligned assignments with submission state; neither → 400
//!     `{message: "student_id or assignment_id is required"}`.

use axum::extract::{Path, Query, State};
use axum::routing::{delete, get, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::is_admin;

// ---------------------------------------------------------------------------
// Rows / serialization
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct OutcomeRow {
    id: i64,
    title: String,
    description: Option<String>,
    display_name: Option<String>,
    vendor_guid: Option<String>,
    mastery_points: Option<f64>,
    calculation_method: Option<String>,
    calculation_int: Option<i32>,
    #[allow(dead_code)]
    workflow_state: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// Canvas `outcome_json` shape (Api::V1::Outcome): criterion is embedded
/// when mastery points are set; calculation_* are always present as
/// objects/strings per the serializer.
fn outcome_json(o: &OutcomeRow) -> Value {
    let mut body = json!({
        "id": o.id,
        "title": o.title,
        "description": o.description,
        "display_name": o.display_name,
        "vendor_guid": o.vendor_guid,
        "url": format!("/api/v1/outcomes/{}", o.id),
        "can_edit": true,
        "can_read": true,
        "created_at": o.created_at.to_rfc3339(),
        "updated_at": o.updated_at.to_rfc3339(),
    });
    if let Some(points) = o.mastery_points {
        body["mastery_points"] = json!(points);
        body["ratings"] = json!([
            {"description": "Exceeds Mastery", "points": points + 1.0, "mastery": true},
            {"description": "Mastery", "points": points, "mastery": true},
            {"description": "Near Mastery", "points": (points / 2.0), "mastery": false},
            {"description": "Well Below Mastery", "points": 0.0, "mastery": false},
        ]);
    }
    if let Some(method) = &o.calculation_method {
        body["calculation_method"] = json!(method);
    }
    if let Some(int) = o.calculation_int {
        body["calculation_int"] = json!(int);
    }
    body
}

async fn fetch_outcome(pool: &PgPool, id: i64) -> Result<OutcomeRow, ApiError> {
    sqlx::query_as(
        "SELECT id, title, description, display_name, vendor_guid, mastery_points, \
                calculation_method, calculation_int, workflow_state, created_at, updated_at \
         FROM learning_outcomes WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct UpdateOutcome {
    pub title: Option<String>,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub vendor_guid: Option<String>,
    pub mastery_points: Option<f64>,
    pub calculation_method: Option<String>,
    pub calculation_int: Option<i32>,
    /// Accepted for Canvas compatibility; the fixed criterion ratings are
    /// derived from mastery_points (update_outcome_criterion).
    #[allow(dead_code)]
    pub ratings: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AlignmentQuery {
    pub assignment_id: Option<i64>,
    pub student_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /outcomes/{id}
async fn show_outcome(
    State(state): State<AppState>,
    Path(outcome_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let outcome = fetch_outcome(&state.db, outcome_id).await?;
    // Canvas `read` on the outcome: any authenticated user may read global
    // outcomes; the crate's enrolled-or-admin check applies to course
    // contexts. Outcomes here are global-scope, so authenticated read.
    let _ = auth;
    Ok(Json(outcome_json(&outcome)))
}

/// PUT /outcomes/{id}
async fn update_outcome(
    State(state): State<AppState>,
    Path(outcome_id): Path<i64>,
    auth: AuthUser,
    _body: Option<Json<UpdateOutcome>>,
) -> Result<Json<Value>, ApiError> {
    // Canvas `update` right on the outcome: global admin or the account
    // admins; keep the crate-wide rule that global admins manage global
    // outcomes.
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let outcome = fetch_outcome(&state.db, outcome_id).await?;
    let Json(payload) = _body.unwrap_or_default();
    if let Some(title) = payload.title.as_deref() {
        if title.trim().is_empty() {
            // Canvas: render errors, :bad_request
            return Err(ApiError::BadRequest("title can't be blank".into()));
        }
    }
    let mastery = payload.mastery_points;
    if let Some(points) = mastery {
        if points < 0.0 {
            return Err(ApiError::BadRequest("mastery_points must be >= 0".into()));
        }
    }

    sqlx::query(
        "UPDATE learning_outcomes SET \
            title = COALESCE($2, title), \
            display_name = COALESCE($3, display_name), \
            description = COALESCE($4, description), \
            vendor_guid = COALESCE($5, vendor_guid), \
            mastery_points = COALESCE($6, mastery_points), \
            calculation_method = COALESCE($7, calculation_method), \
            calculation_int = COALESCE($8, calculation_int), \
            updated_at = now() \
         WHERE id = $1",
    )
    .bind(outcome.id)
    .bind(payload.title.as_deref().map(str::trim))
    .bind(&payload.display_name)
    .bind(&payload.description)
    .bind(&payload.vendor_guid)
    .bind(payload.mastery_points)
    .bind(&payload.calculation_method)
    .bind(payload.calculation_int)
    .execute(&state.db)
    .await?;

    let updated = fetch_outcome(&state.db, outcome_id).await?;
    Ok(Json(outcome_json(&updated)))
}

/// DELETE /outcomes/{id} — soft-delete, JSON of the deleted outcome.
async fn destroy_outcome(
    State(state): State<AppState>,
    Path(outcome_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }
    let outcome = fetch_outcome(&state.db, outcome_id).await?;
    sqlx::query(
        "UPDATE learning_outcomes SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(outcome.id)
    .execute(&state.db)
    .await?;
    Ok(Json(outcome_json(&outcome)))
}

/// GET /courses/{courseId}/outcome_alignments
async fn course_outcome_alignments(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<AlignmentQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    // require_context: unknown course → 404.
    let account_id: Option<i64> = sqlx::query_scalar(
        "SELECT account_id FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    let _account_id = account_id.ok_or(ApiError::NotFound)?;

    // Neither param → 400 {message} (Canvas outcome_alignments branch).
    let assignment_id = query.assignment_id;
    let student_id = query.student_id;
    if assignment_id.is_none() && student_id.is_none() {
        return Err(ApiError::BadRequest(
            "student_id or assignment_id is required".into(),
        ));
    }

    // Strict authorization: manage_grades or view_all_grades (teacher/ta or
    // admin), else the student may only read their own enrollments.
    let can_manage = teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;
    if !can_manage {
        if let Some(sid) = student_id {
            if sid != auth.id {
                return Err(ApiError::Unauthorized);
            }
        } else {
            return Err(ApiError::Unauthorized);
        }
    }

    // Validate the assignment when given (Canvas validate_assignment / the
    // student branch's active lookup) — 404 {message} when it is not in the
    // course.
    if let Some(aid) = assignment_id {
        let assignment_ok: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM assignments \
             WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted')",
        )
        .bind(aid)
        .bind(course_id)
        .fetch_one(&state.db)
        .await?;
        if !assignment_ok {
            return Err(ApiError::NotFound);
        }
    }

    // Assignment branch: every active outcome alignment of that assignment.
    if let Some(aid) = assignment_id.filter(|_| student_id.is_none()) {
        let rows: Vec<(i64, String, i64, Option<String>)> = sqlx::query_as(
            "SELECT ct.content_id AS learning_outcome_id, ct.title, \
                    a.id AS assignment_id, a.submission_types \
             FROM content_tags ct \
             JOIN assignments a ON a.id = ct.content_id AND ct.content_type = 'Assignment' \
             WHERE ct.context_id = $1 AND ct.context_type = 'Course' \
               AND ct.tag_type = 'learning_outcome_alignment' \
               AND ct.workflow_state = 'active' AND a.id = $2",
        )
        .bind(course_id)
        .bind(aid)
        .fetch_all(&state.db)
        .await?;
        return Ok(Json(
            rows.into_iter()
                .map(|(outcome_id, title, aid, sub)| {
                    json!({
                        "learning_outcome_id": outcome_id,
                        "title": title,
                        "assignment_id": aid,
                        "submission_types": sub,
                        "url": format!("/courses/{course_id}/assignments/{aid}"),
                    })
                })
                .collect(),
        ));
    }

    // Student branch: aligned assignments of the course with the student's
    // submission state.
    type StudentAlignmentRow = (i64, String, i64, Option<String>, Option<String>);
    let sid = student_id.expect("checked above");
    let rows: Vec<StudentAlignmentRow> = sqlx::query_as(
        "SELECT ct.content_id AS learning_outcome_id, ct.title, \
                a.id AS assignment_id, a.submission_types, s.workflow_state \
         FROM content_tags ct \
         JOIN assignments a ON a.id = ct.content_id AND ct.content_type = 'Assignment' \
         LEFT JOIN submissions s ON s.assignment_id = a.id AND s.user_id = $2 \
              AND s.workflow_state <> 'deleted' \
         WHERE ct.context_id = $1 AND ct.context_type = 'Course' \
           AND ct.tag_type = 'learning_outcome_alignment' \
           AND ct.workflow_state = 'active' \
           AND a.workflow_state <> 'deleted' \
         ORDER BY a.id",
    )
    .bind(course_id)
    .bind(sid)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(outcome_id, title, aid, sub, state)| {
                json!({
                    "learning_outcome_id": outcome_id,
                    "title": title,
                    "assignment_id": aid,
                    "submission_types": sub,
                    "submission_state": state,
                    "url": format!("/courses/{course_id}/assignments/{aid}"),
                })
            })
            .collect(),
    ))
}

/// Teacher/TA on the course or global admin (manage/view grades proxy).
async fn teacher_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }
    let teacher: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
           AND workflow_state <> 'deleted')",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(pool)
    .await?;
    Ok(teacher)
}

/// Mount the outcomes-api routes.
pub fn outcomes_api_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/outcomes/{id}",
            get(show_outcome)
                .put(put(update_outcome))
                .delete(delete(destroy_outcome)),
        )
        .route(
            "/courses/{courseId}/outcome_alignments",
            get(course_outcome_alignments),
        )
}
