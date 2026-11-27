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
//!   - sis_assignments: `require_view_all_grades` (course teacher/ta or the
//!     context's account admins; 403 without it) + `require_grade_export`
//!     (the course's account settings carry a `sis_integration_id` or a
//!     truthy `bulk_sis_grade_export`, else 400 `{"code": "not_enabled",
//!     ...}`) + `require_published_course` (400
//!     `{"code": "unpublished_course", ...}` on the course variant —
//!     `available`/`completed`, star courses have no `published` state).
//!     Lists the course's published, `post_to_sis` assignments (the account
//!     variant walks the account plus its sub-accounts and honours
//!     `starts_before`/`ends_after`) as a bare Canvas-shaped array with the
//!     SIS ids (`sis_assignment_id` = `assignments.sis_source_id`,
//!     `integration_id`, `integration_data`, `include_in_final_grade`), the
//!     assignment group and the course sections; `include[]=student_overrides`
//!     adds `user_overrides`.
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

/// Canvas `SisApiController::GRADE_EXPORT_NOT_ENABLED_ERROR` — 400.
const GRADE_EXPORT_NOT_ENABLED_ERROR: &str =
    "A SIS integration is not configured and the bulk SIS Grade Export feature is not enabled";

/// Canvas `SisApiController::COURSE_NOT_PUBLISHED_ERROR` — 400.
const COURSE_NOT_PUBLISHED_ERROR: &str = "Grade data is not available for non-published courses";

/// One assignment of the SIS grade-export feed.
#[derive(Debug, sqlx::FromRow)]
struct SisAssignmentRow {
    id: i64,
    course_id: i64,
    title: String,
    assignment_group_id: Option<i64>,
    created_at: chrono::DateTime<chrono::Utc>,
    due_at: Option<chrono::DateTime<chrono::Utc>>,
    unlock_at: Option<chrono::DateTime<chrono::Utc>>,
    lock_at: Option<chrono::DateTime<chrono::Utc>>,
    points_possible: Option<f64>,
    submission_types: Option<String>,
    sis_source_id: Option<String>,
    integration_id: Option<String>,
    integration_data: Option<String>,
    omit_from_final_grade: bool,
    grading_type: Option<String>,
}

/// One section of the grade-export feed.
#[derive(Debug, sqlx::FromRow)]
struct SisSectionRow {
    id: i64,
    name: String,
    sis_source_id: Option<String>,
    integration_id: Option<String>,
    nonxlist_course_id: Option<i64>,
}

/// One assignment group of the grade-export feed.
#[derive(Debug, sqlx::FromRow)]
struct SisAssignmentGroupRow {
    id: i64,
    name: String,
    sis_source_id: Option<String>,
    integration_data: Option<String>,
    group_weight: f64,
}

/// The course row the gates run against.
#[derive(Debug, sqlx::FromRow)]
struct SisCourseRow {
    id: i64,
    account_id: i64,
}

/// One ad-hoc student override of the grade-export feed.
type SisOverrideRow = (
    i64,
    Option<String>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
    Vec<i64>,
);

/// Canvas `Course#published?` — `available` and `completed` are both
/// published lifecycle states (star courses use `available`, not `published`).
fn course_is_published(workflow_state: &str) -> bool {
    workflow_state == "available" || workflow_state == "completed"
}

/// Canvas `Assignment.sis_grade_export_enabled?(context)` reads the
/// `post_grades` feature flag; star-lms keeps the account-settings
/// approximation the endpoint was first built on: the course's account
/// settings hash carries a `sis_integration_id` string or a truthy
/// `bulk_sis_grade_export`.
async fn sis_grade_export_enabled(pool: &PgPool, account_id: i64) -> Result<bool, ApiError> {
    let settings: Option<Option<Value>> = sqlx::query_scalar(
        "SELECT settings FROM accounts WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await?;
    let Some(settings) = settings else {
        return Ok(false);
    };
    let settings = settings.unwrap_or(Value::Null);
    Ok(settings
        .as_object()
        .map(|s| {
            s.get("sis_integration_id").is_some_and(Value::is_string)
                || s.get("bulk_sis_grade_export")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
        })
        .unwrap_or(false))
}

/// The Canvas-shaped 400 bodies (`GRADE_EXPORT_NOT_ENABLED_ERROR`,
/// `COURSE_NOT_PUBLISHED_ERROR`).
fn sis_bad_request(code: &str, error: &str) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(json!({ "code": code, "error": error })),
    )
        .into_response()
}

/// GET /api/sis/accounts/{accountId}/assignments — Canvas:
/// SisApiController#sis_assignments (accounts context, concrete form).
async fn sis_assignments_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    Query(query): Query<SisAssignmentsQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    sis_assignments_feed(&state, SisContext::Account(account_id), &query, &auth).await
}

/// GET /api/sis/courses/{courseId}/assignments — Canvas:
/// SisApiController#sis_assignments (courses context, concrete form).
async fn sis_assignments_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<SisAssignmentsQuery>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    sis_assignments_feed(&state, SisContext::Course(course_id), &query, &auth).await
}

/// The query parameters of the grade-export feed (`starts_before`/`ends_after`
/// only apply to the account variant, like Canvas) and
/// `include[]=student_overrides`.
#[derive(Debug, Default, Deserialize)]
pub struct SisAssignmentsQuery {
    pub starts_before: Option<String>,
    pub ends_after: Option<String>,
    /// Canvas `include[]=student_overrides` — the `[]`-suffixed key is the
    /// documented Rails spelling.
    #[serde(alias = "include[]")]
    pub include: Option<String>,
}

enum SisContext {
    Account(i64),
    Course(i64),
}

/// Canvas `SisApiController#sis_assignments` — the published, `post_to_sis`
/// assignments of the context with their SIS ids and section/group
/// convenience objects (`Api::V1::SisAssignment#sis_assignment_json`).
async fn sis_assignments_feed(
    state: &AppState,
    context: SisContext,
    query: &SisAssignmentsQuery,
    auth: &AuthUser,
) -> Result<Response, ApiError> {
    // The course ids of the context (Canvas `published_course_ids`: the one
    // course, or the account's published courses including every
    // sub-account, optionally windowed by `starts_before`/`ends_after`).
    let (course_ids, context_account_id) = match &context {
        SisContext::Course(course_id) => {
            let course: Option<SisCourseRow> = sqlx::query_as(
                "SELECT id, account_id FROM courses \
                 WHERE id = $1 AND workflow_state <> 'deleted'",
            )
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
            let course = course.ok_or(ApiError::NotFound)?;
            (vec![course.id], course.account_id)
        }
        SisContext::Account(account_id) => {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1)")
                    .bind(account_id)
                    .fetch_one(&state.db)
                    .await?;
            if !exists {
                return Err(ApiError::NotFound);
            }
            // `Account.sub_account_ids_recursive` — the account plus every
            // descendant account.
            let ids: Vec<i64> = sqlx::query_scalar(
                "WITH RECURSIVE subtree AS ( \
                    SELECT id FROM accounts WHERE id = $1 \
                    UNION ALL \
                    SELECT a.id FROM accounts a \
                    JOIN subtree s ON a.parent_account_id = s.id \
                    WHERE a.workflow_state <> 'deleted') \
                 SELECT id FROM subtree",
            )
            .bind(account_id)
            .fetch_all(&state.db)
            .await?;
            let mut sql = String::from(
                "SELECT id FROM courses \
                 WHERE account_id = ANY($1) AND workflow_state = 'available'",
            );
            // Canvas windows the account variant with
            // `courses.start_at`/`conclude_at` (star courses carry
            // `start_at`/`end_at`; the enrollment-term join Canvas adds is
            // not modeled here).
            if query.starts_before.is_some() || query.ends_after.is_some() {
                sql.push_str(" AND ($2::timestamptz IS NULL OR start_at IS NULL OR start_at < $2)");
                sql.push_str(" AND ($3::timestamptz IS NULL OR end_at IS NULL OR end_at > $3)");
            }
            sql.push_str(" ORDER BY id");
            let mut q = sqlx::query_scalar(&sql).bind(ids);
            if query.starts_before.is_some() || query.ends_after.is_some() {
                for raw in [&query.starts_before, &query.ends_after] {
                    let parsed = raw
                        .as_deref()
                        .map(|value| {
                            chrono::DateTime::parse_from_rfc3339(value)
                                .map(|parsed| parsed.with_timezone(&chrono::Utc))
                                .map_err(|_| ApiError::BadRequest("invalid timestamp".into()))
                        })
                        .transpose()?;
                    q = q.bind(parsed);
                }
            }
            (q.fetch_all(&state.db).await?, *account_id)
        }
    };

    // `before_action :require_view_all_grades` — the account variant checks
    // the account's admins (Canvas `view_all_grades` on the Account), the
    // course variant the course's teachers/TAs and admins.
    let allowed = if auth.role == "admin" {
        true
    } else {
        match &context {
            SisContext::Course(course_id) => {
                sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM enrollments \
                     WHERE user_id = $1 AND course_id = $2 AND role IN ('teacher', 'ta') \
                       AND workflow_state <> 'deleted')",
                )
                .bind(auth.id)
                .bind(course_id)
                .fetch_one(&state.db)
                .await?
            }
            SisContext::Account(account_id) => {
                sqlx::query_scalar(
                    "WITH RECURSIVE ancestors AS ( \
                        SELECT id, parent_account_id FROM accounts WHERE id = $1 \
                        UNION ALL \
                        SELECT a.id, a.parent_account_id FROM accounts a \
                        JOIN ancestors s ON a.id = s.parent_account_id) \
                     SELECT EXISTS (SELECT 1 FROM account_users \
                       WHERE user_id = $2 AND role = 'admin' \
                         AND account_id IN (SELECT id FROM ancestors) \
                         AND workflow_state <> 'deleted')",
                )
                .bind(account_id)
                .bind(auth.id)
                .fetch_one(&state.db)
                .await?
            }
        }
    };
    if !allowed {
        return Err(ApiError::Forbidden);
    }

    // `before_action :require_grade_export` — 400
    // GRADE_EXPORT_NOT_ENABLED_ERROR (the course's account governs both
    // variants; Canvas reads the `post_grades` feature flag on the context).
    if !sis_grade_export_enabled(&state.db, context_account_id).await? {
        return Ok(sis_bad_request(
            "not_enabled",
            GRADE_EXPORT_NOT_ENABLED_ERROR,
        ));
    }

    // `before_action :require_published_course` — 400
    // COURSE_NOT_PUBLISHED_ERROR, course context only.
    if let SisContext::Course(course_id) = &context {
        let state_value: String = sqlx::query_scalar(
            "SELECT workflow_state FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(course_id)
        .fetch_one(&state.db)
        .await?;
        if !course_is_published(&state_value) {
            return Ok(sis_bad_request(
                "unpublished_course",
                COURSE_NOT_PUBLISHED_ERROR,
            ));
        }
    }

    // Canvas `published_assignments`: published + post_to_sis, ordered by
    // course then id.
    let rows: Vec<SisAssignmentRow> = sqlx::query_as(
        "SELECT a.id, a.course_id, a.title, a.assignment_group_id, a.created_at, a.due_at, \
                a.unlock_at, a.lock_at, a.points_possible::float8, a.submission_types, \
                a.sis_source_id, a.integration_id, a.integration_data, \
                a.omit_from_final_grade, a.grading_type \
         FROM assignments a \
         WHERE a.course_id = ANY($1) AND a.workflow_state = 'published' \
           AND a.post_to_sis IS TRUE \
         ORDER BY a.course_id, a.id",
    )
    .bind(course_ids)
    .fetch_all(&state.db)
    .await?;

    let student_overrides = query
        .include
        .as_deref()
        .map(|value| {
            value
                .split(',')
                .any(|part| part.trim() == "student_overrides")
        })
        .unwrap_or(false);

    let mut assignments = Vec::with_capacity(rows.len());
    for row in &rows {
        let group: Option<SisAssignmentGroupRow> = match row.assignment_group_id {
            Some(group_id) => {
                sqlx::query_as(
                    "SELECT id, name, sis_source_id, integration_data, group_weight \
                 FROM assignment_groups WHERE id = $1 AND workflow_state <> 'deleted'",
                )
                .bind(group_id)
                .fetch_optional(&state.db)
                .await?
            }
            None => None,
        };
        assignments.push(sis_assignment_json(state, row, group, student_overrides).await?);
    }

    Ok(Json(Value::Array(assignments)).into_response())
}

/// `Api::V1::SisAssignment#sis_assignment_json`.
async fn sis_assignment_json(
    state: &AppState,
    row: &SisAssignmentRow,
    group: Option<SisAssignmentGroupRow>,
    student_overrides: bool,
) -> Result<Value, ApiError> {
    let mut json = json!({
        "id": row.id,
        "course_id": row.course_id,
        "name": row.title,
        "created_at": row.created_at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "due_at": row.due_at.map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        "unlock_at": row.unlock_at.map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        "lock_at": row.lock_at.map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        "points_possible": row.points_possible,
        // Canvas `alias_attribute :sis_assignment_id, :sis_source_id`.
        "sis_assignment_id": row.sis_source_id,
        "integration_id": row.integration_id,
        "integration_data": parse_integration_data(row.integration_data.as_deref()),
        "include_in_final_grade": !(row.omit_from_final_grade
            || row.grading_type.as_deref() == Some("not_graded")),
        "submission_types": submission_types_array(row.submission_types.as_deref()),
    });

    if let Some(group) = group {
        json["assignment_group"] = json!({
            "id": group.id,
            "name": group.name,
            "sis_source_id": group.sis_source_id,
            "integration_data": parse_integration_data(group.integration_data.as_deref()),
            "group_weight": group.group_weight,
        });
    }

    // Canvas `add_sis_course_sections_json`: the course's active sections
    // with the origin (non-crosslisted-away) course inline.
    let sections: Vec<SisSectionRow> = sqlx::query_as(
        "SELECT id, name, sis_source_id, integration_id, nonxlist_course_id \
         FROM course_sections WHERE course_id = $1 AND workflow_state = 'active' ORDER BY id",
    )
    .bind(row.course_id)
    .fetch_all(&state.db)
    .await?;
    let mut section_json = Vec::with_capacity(sections.len());
    for section in sections {
        let origin_course_id = section.nonxlist_course_id.unwrap_or(row.course_id);
        let origin: Option<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT id, name, sis_source_id, integration_id FROM courses WHERE id = $1",
        )
        .bind(origin_course_id)
        .fetch_optional(&state.db)
        .await?;
        let mut section_value = json!({
            "id": section.id,
            "name": section.name,
            "sis_id": section.sis_source_id,
            "integration_id": section.integration_id,
        });
        if let Some((id, name, sis, integration)) = origin {
            section_value["origin_course"] = json!({
                "id": id,
                "name": name,
                "sis_id": sis,
                "integration_id": integration,
            });
            // Canvas adds `xlist_course` only while the section is
            // cross-listed away from its birth course.
            if section.nonxlist_course_id.is_some()
                && section.nonxlist_course_id != Some(row.course_id)
            {
                let xlist: Option<(i64, String, Option<String>, Option<String>)> = sqlx::query_as(
                    "SELECT id, name, sis_source_id, integration_id FROM courses WHERE id = $1",
                )
                .bind(row.course_id)
                .fetch_optional(&state.db)
                .await?;
                if let Some((id, name, sis, integration)) = xlist {
                    section_value["xlist_course"] = json!({
                        "id": id,
                        "name": name,
                        "sis_id": sis,
                        "integration_id": integration,
                    });
                }
            }
        }
        section_json.push(section_value);
    }
    json["sections"] = Value::Array(section_json);

    if student_overrides {
        // Canvas `assignment_user_overrides_json`: the ad-hoc student
        // overrides with their `sis_user_id` (users.sis_source_id).
        let overrides: Vec<SisOverrideRow> = sqlx::query_as(
            "SELECT id, title, due_at, unlock_at, lock_at, student_ids \
                 FROM assignment_overrides \
                 WHERE assignment_id = $1 AND workflow_state = 'active' \
                   AND cardinality(student_ids) > 0 ORDER BY id",
        )
        .bind(row.id)
        .fetch_all(&state.db)
        .await?;
        let mut override_json = Vec::with_capacity(overrides.len());
        for (id, title, due_at, unlock_at, lock_at, student_ids) in overrides {
            let mut students = Vec::with_capacity(student_ids.len());
            for student_id in student_ids {
                let sis: Option<Option<String>> =
                    sqlx::query_scalar("SELECT sis_source_id FROM users WHERE id = $1")
                        .bind(student_id)
                        .fetch_optional(&state.db)
                        .await?;
                students.push(json!({
                    "user_id": student_id,
                    "sis_user_id": sis.flatten(),
                }));
            }
            override_json.push(json!({
                "id": id,
                "title": title,
                "due_at": due_at.map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                "unlock_at": unlock_at.map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                "lock_at": lock_at.map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                "students": students,
            }));
        }
        json["user_overrides"] = Value::Array(override_json);
    }

    Ok(json)
}

/// Canvas serializes `integration_data` as a hash; star-lms keeps the text
/// column and reads JSON out of it when it holds JSON.
fn parse_integration_data(raw: Option<&str>) -> Value {
    raw.and_then(|value| serde_json::from_str::<Value>(value).ok())
        .filter(|value| value.is_object())
        .unwrap_or(Value::Null)
}

/// Canvas `submission_types_array` — star keeps the text column; a CSV value
/// becomes the Canvas array.
fn submission_types_array(raw: Option<&str>) -> Value {
    match raw {
        None => Value::Null,
        Some(value) => {
            if let Ok(parsed) = serde_json::from_str::<Vec<Value>>(value) {
                return Value::Array(parsed);
            }
            Value::Array(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(|part| Value::String(part.to_string()))
                    .collect(),
            )
        }
    }
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
/// The /api/v1-served routes — merged into the crate router that star-http
/// nests under /api/v1. The audit paths are v1-relative (the manifest
/// normalizes them as `audit/course/...` after stripping /api/v1).
pub fn small_controllers_routes() -> Router<AppState> {
    Router::new()
        .route("/audit/course/courses/{courseId}", get(audit_for_course))
        .route("/audit/course/accounts/{accountId}", get(audit_for_account))
}

/// The routes Canvas serves OUTSIDE /api/v1 (the "/api/lti" and "/api/sis"
/// route sets) — root-mounted by star-http, next to `login_router()`; the
/// route strings carry their full paths.
pub fn small_controllers_root_routes() -> Router<AppState> {
    Router::new()
        .route("/api/lti/security/jwks", get(lti_jwks))
        .route(
            "/api/lti/security/openid-configuration",
            get(lti_openid_configuration),
        )
        .route(
            "/api/sis/accounts/{accountId}/assignments",
            get(sis_assignments_account),
        )
        .route(
            "/api/sis/courses/{courseId}/assignments",
            get(sis_assignments_course),
        )
}
