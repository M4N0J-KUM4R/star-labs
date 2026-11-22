//! Syllabus accessibility scans — Canvas `SyllabusApiController` (parity gap).
//!
//! Routes:
//!   - POST /courses/{courseId}/syllabus/accessibility/scan       (sync)
//!   - POST /courses/{courseId}/syllabus/accessibility/queue_scan (async)
//!
//! Canvas semantics:
//!   - require_context + require_course_context (404 unknown course).
//!   - manage-course-content right (teacher/ta or admin) — 401 otherwise
//!     (render_unauthorized_action).
//!   - a11y_checker_enabled? — the course's feature flag; disabled → 401.
//!     The workspace convention reads feature flags from account settings
//!     (feature_flags live there); the a11y checker is ENABLED by default
//!     here (no flag gate stored), matching the announcements scans.
//!   - scan: synchronous heuristic over the syllabus_body HTML
//!     (img without alt, table without header cells); queue_scan: 202.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn load_syllabus(pool: &PgPool, course_id: i64) -> Result<(i64, Option<String>), ApiError> {
    let row: Option<(i64, Option<String>)> = sqlx::query_as(
        "SELECT id, syllabus_body FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

/// manage-course-content: teacher/ta or admin.
async fn can_manage_course(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if auth.role == "admin" {
        return Ok(true);
    }
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
           AND workflow_state <> 'deleted')",
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .map_err(Into::into)
}

/// The Canvas accessibility-checker heuristics the syllabus HTML runs
/// through (same stance as the announcements scans).
fn count_issues(html: &str) -> i64 {
    let img_count = html.matches("<img").count() as i64;
    let alt_count = html.matches("alt=").count() as i64;
    let missing_alt = (img_count - alt_count).max(0);
    let table_no_header =
        i64::from(html.contains("<table") && !html.contains("<th") && !html.contains("scope="));
    missing_alt + table_no_header
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/syllabus/accessibility/scan
async fn syllabus_accessibility_scan(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let (course_id, syllabus) = load_syllabus(&state.db, course_id).await?;
    if !can_manage_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Unauthorized);
    }
    let issues = count_issues(syllabus.as_deref().unwrap_or(""));
    Ok(Json(json!({
        "course_id": course_id,
        "content_type": "syllabus",
        "error_count": issues,
        "warning_count": 0,
    })))
}

/// POST /courses/{courseId}/syllabus/accessibility/queue_scan
async fn syllabus_accessibility_queue_scan(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    let (course_id, _) = load_syllabus(&state.db, course_id).await?;
    if !can_manage_course(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Unauthorized);
    }
    Ok(StatusCode::ACCEPTED)
}

/// Mount the syllabus-scan routes.
pub fn syllabus_api_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/syllabus/accessibility/scan",
            post(syllabus_accessibility_scan),
        )
        .route(
            "/courses/{courseId}/syllabus/accessibility/queue_scan",
            post(syllabus_accessibility_queue_scan),
        )
}
