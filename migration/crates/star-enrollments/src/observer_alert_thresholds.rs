//! observer_alert_thresholds — Canvas `ObserverAlertThresholdsApiController`
//! parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /users/{userId}/observer_alert_thresholds` — the thresholds of the observer
//! - `POST   /users/{userId}/observer_alert_thresholds` — create a threshold
//! - `GET    /users/{userId}/observer_alert_thresholds/{thresholdId}` — show one
//! - `PUT    /users/{userId}/observer_alert_thresholds/{thresholdId}` — update one
//! - `DELETE /users/{userId}/observer_alert_thresholds/{thresholdId}` — delete one
//!
//! Canvas routes (config/routes.rb, the user API block):
//!   get    "users/:user_id/observer_alert_thresholds", action: :index
//!   post   "users/:user_id/observer_alert_thresholds", action: :create
//!   get    "users/:user_id/observer_alert_thresholds/:observer_alert_threshold_id", action: :show
//!   put    "users/:user_id/observer_alert_thresholds/:observer_alert_threshold_id", action: :update
//!   delete "users/:user_id/observer_alert_thresholds/:observer_alert_threshold_id", action: :destroy
//!
//! Canvas controller (`app/controllers/observer_alert_thresholds_api_controller.rb`):
//! a threshold belongs to the observer — the caller — and the observer has
//! to be linked to the student. A threshold of another observer is a 401.
//!
//! The threshold JSON is the Canvas `Api::V1::ObserverAlertThreshold` shape:
//! `id`, `user_id`, `observer_id`, `alert_type`, `threshold` and
//! `workflow_state`.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

/// An `observer_alert_thresholds` row.
#[derive(Debug, sqlx::FromRow)]
struct Threshold {
    id: i64,
    user_id: i64,
    observer_id: i64,
    alert_type: String,
    threshold: Option<String>,
    workflow_state: String,
}

/// The columns of an `observer_alert_thresholds` row.
const THRESHOLD_COLUMNS: &str = "id, user_id, observer_id, alert_type, threshold, workflow_state";

/// The query parameters of the listing.
#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    student_id: Option<i64>,
}

/// The body of a create or update request.
#[derive(Debug, Default, Deserialize)]
struct ThresholdRequest {
    observer_alert_threshold: Option<ThresholdFields>,
}

/// The fields of a threshold in a request.
#[derive(Debug, Default, Deserialize)]
struct ThresholdFields {
    alert_type: Option<String>,
    threshold: Option<String>,
    user_id: Option<i64>,
}

/// Canvas `ObserverAlertThreshold::ALERT_TYPES_WITH_THRESHOLD`.
const ALERT_TYPES_WITH_THRESHOLD: [&str; 4] = [
    "assignment_grade_high",
    "assignment_grade_low",
    "course_grade_high",
    "course_grade_low",
];

/// Canvas `ObserverAlertThreshold::ALERT_TYPES_WITHOUT_THRESHOLD`.
const ALERT_TYPES_WITHOUT_THRESHOLD: [&str; 3] = [
    "assignment_missing",
    "course_announcement",
    "institution_announcement",
];

/// Domain router for the Canvas observer-alert-threshold routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/users/{user_id}/observer_alert_thresholds",
            get(index).post(create),
        )
        .route(
            "/users/{user_id}/observer_alert_thresholds/{observer_alert_threshold_id}",
            get(show).put(update).delete(destroy),
        )
}

/// `GET /users/:user_id/observer_alert_thresholds` — thresholds#index.
async fn index(
    State(state): State<AppState>,
    Path(_user_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<ListQuery>,
) -> Result<Response, ApiError> {
    // Canvas lists the thresholds of the observer — the caller.
    let mut sql = format!(
        "SELECT {THRESHOLD_COLUMNS} FROM observer_alert_thresholds \
         WHERE observer_id = $1 AND workflow_state <> 'deleted'"
    );
    if params.student_id.is_some() {
        sql.push_str(" AND user_id = $2");
    }
    sql.push_str(" ORDER BY id ASC");

    let mut rows: Vec<Threshold> = sqlx::query_as(&sql)
        .bind(auth.id)
        .bind(params.student_id)
        .fetch_all(&state.db)
        .await?;

    // Canvas only keeps the thresholds whose observer and student are still
    // linked.
    let mut items = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        if !users_still_linked(&state.db, row.observer_id, row.user_id).await? {
            continue;
        }
        items.push(threshold_json(&row));
    }
    Ok(Json(items).into_response())
}

/// `GET /users/:user_id/observer_alert_thresholds/:observer_alert_threshold_id` —
/// thresholds#show.
async fn show(
    State(state): State<AppState>,
    Path((_user_id, threshold_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let threshold = load_threshold(&state.db, threshold_id).await?;
    require_observer(&state.db, &auth, &threshold).await?;
    Ok(Json(threshold_json(&threshold)).into_response())
}

/// `POST /users/:user_id/observer_alert_thresholds` — thresholds#create.
async fn create(
    State(state): State<AppState>,
    Path(_user_id): Path<i64>,
    auth: AuthUser,
    body: Option<Json<ThresholdRequest>>,
) -> Result<Response, ApiError> {
    let body = body.map(|Json(body)| body).unwrap_or_default();
    let fields = body.observer_alert_threshold.unwrap_or_default();

    let Some(alert_type) = fields.alert_type else {
        return Err(ApiError::BadRequest("Alert type is invalid".to_string()));
    };
    if !ALERT_TYPES_WITH_THRESHOLD.contains(&alert_type.as_str())
        && !ALERT_TYPES_WITHOUT_THRESHOLD.contains(&alert_type.as_str())
    {
        return Err(ApiError::BadRequest("Alert type is invalid".to_string()));
    }

    // The student has to exist.
    let Some(student_id) = fields.user_id else {
        return Err(ApiError::BadRequest("user_id is invalid".to_string()));
    };
    let student: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(student_id)
        .fetch_optional(&state.db)
        .await?;
    if student.is_none() {
        return Err(ApiError::BadRequest("user_id is invalid".to_string()));
    }

    // Canvas updates the threshold of the same student and alert type.
    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM observer_alert_thresholds \
         WHERE observer_id = $1 AND user_id = $2 AND alert_type = $3 LIMIT 1",
    )
    .bind(auth.id)
    .bind(student_id)
    .bind(&alert_type)
    .fetch_optional(&state.db)
    .await?;

    let id = match existing {
        Some((id,)) => {
            sqlx::query(
                "UPDATE observer_alert_thresholds \
                 SET threshold = $1, workflow_state = 'active', updated_at = now() WHERE id = $2",
            )
            .bind(&fields.threshold)
            .bind(id)
            .execute(&state.db)
            .await?;
            id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO observer_alert_thresholds \
                     (alert_type, threshold, workflow_state, user_id, observer_id) \
                 VALUES ($1, $2, 'active', $3, $4) RETURNING id",
            )
            .bind(&alert_type)
            .bind(&fields.threshold)
            .bind(student_id)
            .bind(auth.id)
            .fetch_one(&state.db)
            .await?
        }
    };

    let threshold = load_threshold(&state.db, id).await?;
    Ok(Json(threshold_json(&threshold)).into_response())
}

/// `PUT /users/:user_id/observer_alert_thresholds/:observer_alert_threshold_id` —
/// thresholds#update.
async fn update(
    State(state): State<AppState>,
    Path((_user_id, threshold_id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Option<Json<ThresholdRequest>>,
) -> Result<Response, ApiError> {
    let threshold = load_threshold(&state.db, threshold_id).await?;
    require_observer(&state.db, &auth, &threshold).await?;

    let body = body.map(|Json(body)| body).unwrap_or_default();
    let fields = body.observer_alert_threshold.unwrap_or_default();

    sqlx::query(
        "UPDATE observer_alert_thresholds SET threshold = $1, updated_at = now() WHERE id = $2",
    )
    .bind(fields.threshold)
    .bind(threshold_id)
    .execute(&state.db)
    .await?;

    let threshold = load_threshold(&state.db, threshold_id).await?;
    Ok(Json(threshold_json(&threshold)).into_response())
}

/// `DELETE /users/:user_id/observer_alert_thresholds/:observer_alert_threshold_id` —
/// thresholds#destroy.
async fn destroy(
    State(state): State<AppState>,
    Path((_user_id, threshold_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let threshold = load_threshold(&state.db, threshold_id).await?;
    require_observer(&state.db, &auth, &threshold).await?;

    sqlx::query("DELETE FROM observer_alert_thresholds WHERE id = $1")
        .bind(threshold_id)
        .execute(&state.db)
        .await?;

    Ok(Json(threshold_json(&threshold)).into_response())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The threshold of an id — 404.
async fn load_threshold(db: &PgPool, threshold_id: i64) -> Result<Threshold, ApiError> {
    let sql = format!(
        "SELECT {THRESHOLD_COLUMNS} FROM observer_alert_thresholds \
         WHERE id = $1 AND workflow_state <> 'deleted'"
    );
    sqlx::query_as(&sql)
        .bind(threshold_id)
        .fetch_optional(db)
        .await?
        .ok_or(ApiError::NotFound)
}

/// The threshold has to belong to the caller and the observer and the
/// student have to be linked.
async fn require_observer(
    db: &PgPool,
    auth: &AuthUser,
    threshold: &Threshold,
) -> Result<(), ApiError> {
    if threshold.observer_id != auth.id {
        return Err(ApiError::Unauthorized);
    }
    if !users_still_linked(db, threshold.observer_id, threshold.user_id).await? {
        return Err(ApiError::Unauthorized);
    }
    Ok(())
}

/// Canvas `ObserverAlertThreshold#users_are_still_linked?` — the observer
/// watches the student.
async fn users_still_linked(
    db: &PgPool,
    observer_id: i64,
    student_id: i64,
) -> Result<bool, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as(
        "SELECT 1 FROM observer_enrollments \
         WHERE observer_id = $1 AND user_id = $2 LIMIT 1",
    )
    .bind(observer_id)
    .bind(student_id)
    .fetch_optional(db)
    .await?;
    Ok(row.is_some())
}

/// The Canvas `Api::V1::ObserverAlertThreshold` JSON.
fn threshold_json(threshold: &Threshold) -> Value {
    json!({
        "id": threshold.id,
        "user_id": threshold.user_id,
        "observer_id": threshold.observer_id,
        "alert_type": threshold.alert_type,
        "threshold": threshold.threshold,
        "workflow_state": threshold.workflow_state,
    })
}
