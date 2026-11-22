//! Small parity-gap controllers, batched:
//!
//! - `SecurityController` (LTI security):
//!   - GET /api/lti/security/jwks (jwks — reuse the OAuth2 signing key)
//!   - GET /api/lti/security/openid-configuration (openid_configuration)
//! - `SisApiController`:
//!   - GET /api/sis/accounts/{accountId}/assignments (sis_assignments)
//!   - GET /api/sis/courses/{courseId}/assignments   (sis_assignments)
//! - `CourseAuditApiController`:
//!   - GET /api/audit/course/courses/{courseId}   (for_course)
//!   - GET /api/audit/course/accounts/{accountId} (for_account)
//!
//! Canvas semantics:
//!   - security#jwks: the platform JWKS (same key set as the OAuth2
//!     endpoints; unauthenticated).
//!   - security#openid_configuration: the platform's OpenID Connect
//!     discovery document (issuer, jwks_uri, token endpoint...).
//!   - sis_assignments: view_all_grades + grade export (teacher/ta/admin;
//!     the course must be published — 400 unpublished_course / 401
//!     not_enabled per Canvas' fixed error shapes).
//!   - course audit: the course/account event stream (Auditors::Course);
//!     view course/audit rights; query_options window like the other
//!     auditors.

use crate::AuthUser;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Security (LTI)
// ---------------------------------------------------------------------------

/// GET /api/lti/security/jwks — the OAuth2 signing key as a JWKS.
async fn lti_jwks() -> Response {
    // Reuse the star-auth OAuth2 signing key (same platform key).
    crate::oauth2_jwks_json().await.into_response()
}

/// GET /api/lti/security/openid_configuration — OIDC discovery document.
async fn lti_openid_configuration() -> Json<Value> {
    Json(json!({
        "issuer": "canvas-lms",
        "jwks_uri": "/api/lti/security/jwks",
        "token_endpoint": "/login/oauth2/token",
        "authorization_endpoint": "/login/oauth2/auth",
        "registration_endpoint": "/api/lti/developers/registrations",
        "scopes_supported": [
            "https://purl.imsglobal.org/spec/lti-ags/scope/lineitem",
            "https://purl.imsglobal.org/spec/lti-nrps/scope/contextmembership.readonly",
        ],
        "response_types_supported": ["id_token"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
        "token_endpoint_auth_methods_supported": ["private_key_jwt"],
        "claims_supported": ["sub", "iss", "aud", "exp", "iat", "nonce"],
    }))
}

// ---------------------------------------------------------------------------
// SIS assignments
// ---------------------------------------------------------------------------

/// require_view_all_grades + require_grade_export + published course.
async fn sis_assignment_gate(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<(), ApiError> {
    let course: Option<(String, Option<Value>)> = sqlx::query_as(
        "SELECT workflow_state, (SELECT settings FROM accounts a \
          WHERE a.id = c.account_id) FROM courses c WHERE c.id = $1",
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    let (workflow_state, account_settings) = course.ok_or(ApiError::NotFound)?;

    // SIS integration configured / bulk grade export enabled: the account's
    // sis_integration_id or the bulk_sis_grade_export flag in settings.
    let settings = account_settings.unwrap_or(Value::Null);
    let grade_export = settings
        .as_object()
        .map(|s| {
            s.get("sis_integration_id").is_some_and(Value::is_string)
                || s.get("bulk_sis_grade_export")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    if !grade_export {
        // Canvas 401 {code: not_enabled, error: ...}.
        return Err(ApiError::Unauthorized);
    }
    if workflow_state != "published" {
        // Canvas 400 {code: unpublished_course, error: ...}.
        return Err(ApiError::BadRequest("unpublished_course".into()));
    }

    // view_all_grades: teacher/ta or admin.
    if auth.role != "admin" {
        let teacher: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM enrollments \
             WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
               AND workflow_state <> 'deleted')",
        )
        .bind(auth.id)
        .bind(course_id)
        .fetch_one(pool)
        .await?;
        if !teacher {
            return Err(ApiError::Forbidden);
        }
    }
    Ok(())
}

/// GET /api/sis/{accounts|courses}/{id}/assignments — the SIS grade-export
/// assignment list.
async fn sis_assignments(
    State(state): State<AppState>,
    Path((context, context_id)): Path<(String, i64)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    // Resolve the course from either context shape.
    let course_id: i64 = match context.as_str() {
        "courses" => context_id,
        "accounts" => {
            // The account's first course (SIS exports run per course; the
            // account variant enumerates one course's assignments).
            sqlx::query_scalar(
                "SELECT id FROM courses WHERE account_id = $1 AND workflow_state <> 'deleted' \
                 ORDER BY id LIMIT 1",
            )
            .bind(context_id)
            .fetch_optional(&state.db)
            .await?
            .ok_or(ApiError::NotFound)?
        }
        _ => return Err(ApiError::NotFound),
    };
    sis_assignment_gate(&state.db, &auth, course_id).await?;

    // sis_assignments_json: the course's assignments with their
    // integration_data (integration_id) and points.
    type AssignmentRow = (i64, String, Option<String>, Option<f64>, Option<String>);
    let rows: Vec<AssignmentRow> = sqlx::query_as(
        "SELECT id, name, integration_id, points_possible, submission_types \
         FROM assignments WHERE course_id = $1 AND workflow_state <> 'deleted' ORDER BY id",
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(json!({
        "assignments": rows
            .into_iter()
            .map(|(id, name, integration_id, points, types)| json!({
                "id": id,
                "name": name,
                "integration_id": integration_id,
                "points_possible": points,
                "submission_types": types,
            }))
            .collect::<Vec<_>>(),
    })))
}

// ---------------------------------------------------------------------------
// Course audit
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub per_page: Option<i64>,
}

fn parse_time(
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

/// Auditors::Course events live in course_events (migration 0236).
async fn course_events(
    pool: &PgPool,
    course_id: Option<i64>,
    account_id: Option<i64>,
    query: &AuditQuery,
) -> Result<Vec<Value>, ApiError> {
    let oldest = parse_time(query.start_time.as_deref(), "start_time")?;
    let newest = parse_time(query.end_time.as_deref(), "end_time")?;
    let per_page = query.per_page.unwrap_or(50).clamp(1, 100);
    type CourseEventRow = (
        i64,
        i64,
        Option<i64>,
        Option<String>,
        Value,
        chrono::DateTime<chrono::Utc>,
    );
    let rows: Vec<CourseEventRow> = sqlx::query_as(
        "SELECT id, course_id, account_id, event_type, payload, created_at \
             FROM course_events \
             WHERE ($1::bigint IS NULL OR course_id = $1) \
               AND ($2::bigint IS NULL OR account_id = $2) \
               AND ($3::timestamptz IS NULL OR created_at >= $3) \
               AND ($4::timestamptz IS NULL OR created_at <= $4) \
             ORDER BY created_at DESC LIMIT $5",
    )
    .bind(course_id)
    .bind(account_id)
    .bind(oldest)
    .bind(newest)
    .bind(per_page)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, course, account, event_type, payload, created_at)| {
            json!({
                "id": id,
                "course_id": course,
                "account_id": account,
                "event_type": event_type,
                "payload": payload,
                "created_at": created_at.to_rfc3339(),
            })
        })
        .collect())
}

/// GET /api/audit/course/courses/{courseId}
async fn audit_for_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<AuditQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM courses WHERE id = $1)")
        .bind(course_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    // Course audit: teacher/ta or admin.
    if auth.role != "admin" {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM enrollments \
             WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
               AND workflow_state <> 'deleted')",
        )
        .bind(auth.id)
        .bind(course_id)
        .fetch_one(&state.db)
        .await?;
        if !allowed {
            return Err(ApiError::Unauthorized);
        }
    }
    let events = course_events(&state.db, Some(course_id), None, &query).await?;
    Ok(Json(json!({
        "events": events,
        "links": { "current": format!("/api/v1/audit/course/courses/{course_id}") },
    })))
}

/// GET /api/audit/course/accounts/{accountId}
async fn audit_for_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    Query(query): Query<AuditQuery>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1)")
        .bind(account_id)
        .fetch_one(&state.db)
        .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }
    // Account audit: the account's admins or global admin.
    if auth.role != "admin" {
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM account_users \
             WHERE account_id = $1 AND user_id = $2 AND role = 'admin')",
        )
        .bind(account_id)
        .bind(auth.id)
        .fetch_one(&state.db)
        .await?;
        if !allowed {
            return Err(ApiError::Unauthorized);
        }
    }
    let events = course_events(&state.db, None, Some(account_id), &query).await?;
    Ok(Json(json!({
        "events": events,
        "links": { "current": format!("/api/v1/audit/course/accounts/{account_id}") },
    })))
}

/// Mount the batched routes.
pub fn small_controllers_routes() -> Router<AppState> {
    Router::new()
        .route("/api/lti/security/jwks", get(lti_jwks))
        .route(
            "/api/lti/security/openid-configuration",
            get(lti_openid_configuration),
        )
        .route(
            "/api/sis/{context}/{contextId}/assignments",
            get(sis_assignments),
        )
        .route(
            "/api/audit/course/courses/{courseId}",
            get(audit_for_course),
        )
        .route(
            "/api/audit/course/accounts/{accountId}",
            get(audit_for_account),
        )
}
