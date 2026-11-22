//! Observer alerts — Canvas `ObserverAlertsApiController` (parity gap).
//!
//! Routes:
//!   - GET /users/{userId}/observer_alerts/unread_count     (alerts_count)
//!   - GET /users/{userId}/observer_alerts/{studentId}      (alerts_by_student)
//!   - PUT /users/{userId}/observer_alerts/{alertId}/{workflowState} (update)
//!
//! Canvas semantics:
//!   - `require_observer`: the caller must be the observer themselves (or
//!     an admin) — `read_observer_alerts` on the target user; unknown user
//!     → 404 (api_find).
//!   - alerts_by_student: active alerts of the observer for that student,
//!     newest first, FILTERED to still-linked pairs (users_are_still_linked
//!     via active observation links; unlinked → empty list), excluding
//!     deleted/expired AccountNotification contexts.
//!   - alerts_count: unread (`workflow_state = 'unread'`) alerts, optionally
//!     per student, still-linked only → `{unread_count: N}`.
//!   - update: only the observer's own alert, still linked; `read` or
//!     `dismissed` states (other states leave the row untouched but still
//!     save, Canvas `case` falls through); returns the alert JSON.
//!
//! Serializer fields (Api::V1::ObserverAlert): id, title, user_id,
//! observer_id, observer_alert_threshold_id, alert_type, context_type,
//! context_id, workflow_state, action_date + html_url/locked_for_user.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
struct ObserverAlertRow {
    id: i64,
    title: String,
    user_id: i64,
    observer_id: i64,
    observer_alert_threshold_id: i64,
    alert_type: String,
    context_type: Option<String>,
    context_id: Option<i64>,
    workflow_state: String,
    action_date: chrono::DateTime<chrono::Utc>,
}

fn alert_json(a: &ObserverAlertRow) -> Value {
    // html_url by context (Course contexts deep-link to the course; other
    // context types have no workspace URL yet — Canvas leaves nil).
    let html_url = match (a.context_type.as_deref(), a.context_id) {
        (Some("Course"), Some(id)) => json!(format!("/courses/{id}")),
        _ => Value::Null,
    };
    // locked_for_user: false for AccountNotification / missing context
    // (Canvas: !(context.nil? || AccountNotification || ...)).
    let locked_for_user = !matches!(
        a.context_type.as_deref(),
        None | Some("AccountNotification")
    );
    json!({
        "id": a.id,
        "title": a.title,
        "user_id": a.user_id,
        "observer_id": a.observer_id,
        "observer_alert_threshold_id": a.observer_alert_threshold_id,
        "alert_type": a.alert_type,
        "context_type": a.context_type,
        "context_id": a.context_id,
        "workflow_state": a.workflow_state,
        "action_date": a.action_date.to_rfc3339(),
        "html_url": html_url,
        "locked_for_user": locked_for_user,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// api_find(User): 404 for an unknown observer user.
async fn load_user(pool: &PgPool, user_id: i64) -> Result<(), ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)")
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

/// require_observer: self or admin (read_observer_alerts).
fn authorized_as_observer(observer_id: i64, auth: &AuthUser) -> bool {
    auth.id == observer_id || auth.role == "admin"
}

/// users_are_still_linked: an active observer_enrollments row between the
/// observer and the student (0141; Canvas observation links / associated
/// enrollments).
async fn users_are_still_linked(
    pool: &PgPool,
    observer_id: i64,
    student_id: i64,
) -> Result<bool, ApiError> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM observer_enrollments \
         WHERE observer_id = $1 AND user_id = $2)",
    )
    .bind(observer_id)
    .bind(student_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

#[derive(Debug, Deserialize)]
pub struct CountQuery {
    pub student_id: Option<i64>,
    pub per_page: Option<i64>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /users/{userId}/observer_alerts/unread_count
async fn alerts_count(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Query(query): Query<CountQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    load_user(&state.db, user_id).await?;
    if !authorized_as_observer(user_id, &auth) {
        return Err(ApiError::Forbidden);
    }
    let count: i64 = match query.student_id {
        Some(student_id) => {
            // Still-linked filter applies per student.
            if !users_are_still_linked(&state.db, user_id, student_id).await? {
                0
            } else {
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM observer_alerts \
                     WHERE observer_id = $1 AND user_id = $2 AND workflow_state = 'unread'",
                )
                .bind(user_id)
                .bind(student_id)
                .fetch_one(&state.db)
                .await?
            }
        }
        None => {
            // All still-linked students of this observer.
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM observer_alerts oa \
                 WHERE oa.observer_id = $1 AND oa.workflow_state = 'unread' \
                   AND EXISTS (SELECT 1 FROM observer_enrollments oe \
                               WHERE oe.observer_id = oa.observer_id \
                                 AND oe.user_id = oa.user_id)",
            )
            .bind(user_id)
            .fetch_one(&state.db)
            .await?
        }
    };
    Ok(Json(json!({ "unread_count": count })))
}

/// GET /users/{userId}/observer_alerts/{studentId}
async fn alerts_by_student(
    State(state): State<AppState>,
    Path((user_id, student_id)): Path<(i64, i64)>,
    Query(query): Query<CountQuery>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    load_user(&state.db, user_id).await?;
    if !authorized_as_observer(user_id, &auth) {
        return Err(ApiError::Forbidden);
    }
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);

    // Unlinked pair → empty list (Canvas clears all_alerts).
    if !users_are_still_linked(&state.db, user_id, student_id).await? {
        return Ok(Json(Vec::new()));
    }

    // Active alerts, newest first, excluding deleted/expired
    // AccountNotification contexts.
    let rows: Vec<ObserverAlertRow> = sqlx::query_as(
        "SELECT oa.id, oa.title, oa.user_id, oa.observer_id, \
                oa.observer_alert_threshold_id, oa.alert_type, oa.context_type, \
                oa.context_id, oa.workflow_state, oa.action_date \
         FROM observer_alerts oa \
         WHERE oa.observer_id = $1 AND oa.user_id = $2 \
           AND oa.workflow_state = 'unread' \
           AND NOT ( \
                oa.context_type = 'AccountNotification' \
                AND (NOT EXISTS (SELECT 1 FROM account_notifications an \
                                 WHERE an.id = oa.context_id \
                                   AND an.workflow_state <> 'deleted' \
                                   AND an.end_at > now()) \
                     OR oa.context_id IS NULL) \
           ) \
         ORDER BY oa.id DESC LIMIT $3",
    )
    .bind(user_id)
    .bind(student_id)
    .bind(per_page)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.iter().map(alert_json).collect()))
}

/// PUT /users/{userId}/observer_alerts/{alertId}/{workflowState}
async fn update_alert(
    State(state): State<AppState>,
    Path((user_id, alert_id, workflow_state)): Path<(i64, i64, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    load_user(&state.db, user_id).await?;
    if !authorized_as_observer(user_id, &auth) {
        return Err(ApiError::Forbidden);
    }
    let alert: Option<ObserverAlertRow> = sqlx::query_as(
        "SELECT id, title, user_id, observer_id, observer_alert_threshold_id, \
                alert_type, context_type, context_id, workflow_state, action_date \
         FROM observer_alerts WHERE id = $1",
    )
    .bind(alert_id)
    .fetch_optional(&state.db)
    .await?;
    let alert = alert.ok_or(ApiError::NotFound)?;
    if alert.observer_id != user_id {
        return Err(ApiError::Forbidden);
    }
    if !users_are_still_linked(&state.db, alert.observer_id, alert.user_id).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas `case`: read/dismissed set the state; any other value saves
    // the row unchanged (idempotent no-op).
    let new_state = match workflow_state.as_str() {
        "read" | "dismissed" => workflow_state.clone(),
        _ => alert.workflow_state.clone(),
    };
    sqlx::query("UPDATE observer_alerts SET workflow_state = $2, updated_at = now() WHERE id = $1")
        .bind(alert_id)
        .bind(&new_state)
        .execute(&state.db)
        .await?;
    let mut json = alert_json(&alert);
    json["workflow_state"] = json!(new_state);
    Ok(Json(json))
}

/// Mount the observer-alert routes.
pub fn observer_alerts_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/users/{userId}/observer_alerts/unread_count",
            get(alerts_count),
        )
        .route(
            "/users/{userId}/observer_alerts/{studentId}",
            get(alerts_by_student),
        )
        .route(
            "/users/{userId}/observer_alerts/{alertId}/{workflowState}",
            axum::routing::put(update_alert),
        )
}
