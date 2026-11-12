//! star-enrollments — enrollment management for star-lms.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use axum::{
    extract::{Path, Query, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// --------------------------------------------------------------------------
// Domain types
// --------------------------------------------------------------------------

/// Flat enrollment row from the DB (includes user info from join).
/// All columns from enrollments + joined user columns.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct EnrollmentWithUser {
    pub id: i64,
    pub user_id: i64,
    pub course_id: i64,
    /// Canvas `course_section_id`; NULL means the course default/all sections.
    pub course_section_id: Option<i64>,
    pub role: String,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub name: String,
    pub email: String,
}

/// Request to create an enrollment.
#[derive(Debug, Deserialize)]
pub struct CreateEnrollmentRequest {
    pub user_id: i64,
    pub course_id: i64,
    pub role: Option<String>,
    /// Optional section within `course_id` (Canvas `enrollment[course_section_id]`).
    pub course_section_id: Option<i64>,
}

/// Request to create an enrollment through a section-scoped route.
/// The course is derived from the section, so no `course_id` is accepted.
#[derive(Debug, Deserialize)]
pub struct CreateSectionEnrollmentRequest {
    pub user_id: i64,
    pub role: Option<String>,
}

/// Query params for listing enrollments.
#[derive(Debug, Deserialize)]
pub struct EnrollmentFilter {
    pub user_id: Option<i64>,
    pub course_id: Option<i64>,
}

/// Canvas "Enrollment" JSON for the lifecycle + user-scoped endpoints.
///
/// Canvas renames `workflow_state` to `enrollment_state` in API responses
/// (`enrollment_json` in lib/api/v1/user.rb), so the serialized field carries
/// that name while the DB column keeps its original one.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct EnrollmentJson {
    pub id: i64,
    pub user_id: i64,
    pub course_id: i64,
    pub course_section_id: Option<i64>,
    pub role: String,
    #[serde(rename = "enrollment_state")]
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub last_attended_at: Option<DateTime<Utc>>,
    pub total_activity_time: i32,
}

/// Query params for `GET /users/{userId}/enrollments`.
///
/// Canvas accepts `state[]`, `role` and `course_id`; the repeated `state[]`
/// form is collapsed to a single `state` here because the router does not
/// enable repeated-key deserialization.
#[derive(Debug, Deserialize)]
pub struct UserEnrollmentFilter {
    pub state: Option<String>,
    pub role: Option<String>,
    pub course_id: Option<i64>,
}

/// Minimal enrollment state needed to drive a lifecycle transition.
#[derive(Debug, Clone, sqlx::FromRow)]
struct EnrollmentStateRow {
    user_id: i64,
    workflow_state: String,
}

/// Query params for `PUT /courses/{courseId}/users/{userId}/last_attended`.
#[derive(Debug, Deserialize)]
pub struct LastAttendedParams {
    pub date: Option<String>,
}

/// Query params for `GET /temporary_enrollment_status`.
/// Query params for `GET /temporary_enrollment_status`.
///
/// Accepts comma-separated IDs (`1,2,3`) or repeated bracket form
/// (`user_ids[]=1&user_ids[]=2`). Both spellings are parsed manually via
/// `RawQuery` to support repeated keys (which Axum's `Query` binder drops).
#[derive(Debug, Deserialize)]
pub struct BulkTemporaryEnrollmentParams {
    pub user_ids: Option<String>,
    pub limit: Option<usize>,
}

/// Minimal percent-decoding for raw query/body components.
fn decode_query_component(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        decoded.push((high * 16 + low) as u8);
                        index += 3;
                    }
                    _ => {
                        decoded.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Parse `user_ids` from a raw query string, supporting both comma-separated
/// (`1,2,3`) and repeated-bracket (`user_ids[]=1&user_ids[]=2`) forms.
fn parse_bulk_enrollment_query(
    raw_query: Option<&str>,
) -> Result<(Vec<i64>, Option<i64>), ApiError> {
    let mut user_ids: Vec<i64> = Vec::new();
    let mut limit: Option<i64> = None;

    let Some(raw_query) = raw_query else {
        return Err(ApiError::BadRequest("user_ids is required".into()));
    };

    for pair in raw_query.split('&') {
        let Some((raw_key, raw_value)) = pair.split_once('=') else {
            continue;
        };
        let key = decode_query_component(raw_key).to_ascii_lowercase();
        let value = decode_query_component(raw_value);

        match key.as_str() {
            "user_ids" | "user_ids[]" => {
                for part in value.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    if let Ok(id) = part.parse::<i64>() {
                        if !user_ids.contains(&id) {
                            user_ids.push(id);
                        }
                    }
                }
            }
            "limit" => {
                if let Ok(l) = value.parse::<i64>() {
                    limit = Some(l);
                }
            }
            _ => {}
        }
    }

    if user_ids.is_empty() {
        return Err(ApiError::BadRequest("user_ids is required".into()));
    }

    Ok((user_ids, limit))
}

/// Error type for the Canvas lifecycle/list endpoints.
///
/// Canvas answers invalid transitions with a 400 `{ "error": ... }` body,
/// which the plain-text `ApiError::BadRequest` cannot express, so wrap the
/// shared `ApiError` and add that one JSON-shaped variant.
enum EnrollmentActionError {
    Api(ApiError),
    Invalid(String),
    /// Canvas `#last_attended` answers 400 `{ "message": "Invalid date time input" }`.
    InvalidDate,
}

impl From<ApiError> for EnrollmentActionError {
    fn from(e: ApiError) -> Self {
        Self::Api(e)
    }
}

impl IntoResponse for EnrollmentActionError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(e) => e.into_response(),
            Self::Invalid(message) => {
                (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
            }
            Self::InvalidDate => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "Invalid date time input" })),
            )
                .into_response(),
        }
    }
}

// --------------------------------------------------------------------------
// Permission helpers
// --------------------------------------------------------------------------

/// Look up a user's role from the DB.
async fn get_user_role(pool: &PgPool, user_id: i64) -> Result<Option<String>, ApiError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT role FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(row.map(|(r,)| r))
}

fn is_admin(role: &str) -> bool {
    role == "admin"
}

fn is_teacher(role: &str) -> bool {
    role == "teacher" || role == "admin"
}

/// Check if a user is enrolled as teacher (or admin) for a given course.
async fn is_teacher_in_course(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    role: &str,
) -> Result<bool, ApiError> {
    if is_admin(role) {
        return Ok(true);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(row.is_some())
}

/// Minimal section row needed by the section-scoped enrollment routes.
#[derive(Debug, Clone, sqlx::FromRow)]
struct CourseSectionRef {
    id: i64,
    course_id: i64,
}

/// Load a section by id. 404 when it does not exist, matching Canvas
/// `api_find(CourseSection, params[:section_id])`.
async fn load_section(pool: &PgPool, section_id: i64) -> Result<CourseSectionRef, ApiError> {
    sqlx::query_as("SELECT id, course_id FROM course_sections WHERE id = $1")
        .bind(section_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)
}

/// Canvas scopes `enrollment[course_section_id]` to the course the enrollment
/// targets; a section belonging to another course is a 404 rather than a
/// cross-course enrollment.
async fn ensure_section_in_course(
    pool: &PgPool,
    section_id: i64,
    course_id: i64,
) -> Result<(), ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM course_sections WHERE id = $1 AND course_id = $2")
            .bind(section_id)
            .bind(course_id)
            .fetch_optional(pool)
            .await
            .map_err(ApiError::from)?;
    row.map(|_| ()).ok_or(ApiError::NotFound)
}

fn validate_enrollment_role(role: &str) -> Result<(), ApiError> {
    if role != "student" && role != "teacher" && role != "ta" {
        return Err(ApiError::Validation(
            "role must be 'student', 'teacher', or 'ta'".into(),
        ));
    }
    Ok(())
}

async fn user_exists(pool: &PgPool, user_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?;
    Ok(row.is_some())
}

/// Check whether a feature flag is enabled for the account a course belongs to.
async fn feature_enabled_for_course(
    pool: &PgPool,
    course_id: i64,
    feature: &str,
) -> Result<bool, ApiError> {
    let row: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT state FROM feature_flags
        WHERE context_type = 'Account'
          AND context_id = (
              SELECT account_id FROM courses WHERE id = $1
          )
          AND feature = $2
        "#,
    )
    .bind(course_id)
    .bind(feature)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(row.map(|(s,)| s == "enabled").unwrap_or(false))
}

/// Insert an active enrollment and return it in Canvas-ish shape (with user
/// name/email). 409 when (user, course, role) is already enrolled; Canvas
/// allows one enrollment per user/course/role triple in this schema.
async fn insert_enrollment(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    course_section_id: Option<i64>,
    role: &str,
) -> Result<EnrollmentWithUser, ApiError> {
    let result = sqlx::query(
        r#"
        INSERT INTO enrollments (user_id, course_id, course_section_id, role, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(course_section_id)
    .bind(role)
    .execute(pool)
    .await;

    match result {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23505") => {
            return Err(ApiError::Conflict(format!(
                "user {user_id} is already enrolled in course {course_id} as {role}"
            )));
        }
        Err(e) => return Err(ApiError::Db(e.to_string())),
    }

    sqlx::query_as(
        r#"
        SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
               e.workflow_state, e.created_at, u.name, u.email
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        WHERE e.user_id = $1 AND e.course_id = $2 AND e.role = $3
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)
}

// --------------------------------------------------------------------------
// Router
// --------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/enrollments", post(create_enrollment))
        .route("/enrollments", get(list_enrollments))
        .route("/enrollments/{id}", delete(delete_enrollment))
        // Canvas: GET/POST /api/v1/sections/:section_id/enrollments
        .route(
            "/sections/{section_id}/enrollments",
            get(list_section_enrollments).post(create_section_enrollment),
        )
        // Canvas EnrollmentsApiController lifecycle actions.
        .route(
            "/courses/{courseId}/enrollments/{id}/accept",
            post(accept_enrollment),
        )
        .route(
            "/courses/{courseId}/enrollments/{id}/reject",
            post(reject_enrollment),
        )
        .route(
            "/courses/{courseId}/enrollments/{id}/reactivate",
            put(reactivate_enrollment),
        )
        // Canvas: GET /api/v1/users/:user_id/enrollments ("self" allowed)
        .route("/users/{userId}/enrollments", get(list_user_enrollments))
        // Canvas EnrollmentsApiController temporary-enrollment status.
        .route(
            "/users/{userId}/temporary_enrollment_status",
            get(show_temporary_enrollment_status),
        )
        .route(
            "/temporary_enrollment_status",
            get(bulk_temporary_enrollment_status),
        )
        // Canvas EnrollmentsApiController#last_attended.
        .route(
            "/courses/{courseId}/users/{userId}/last_attended",
            put(update_last_attended),
        )
}

// --------------------------------------------------------------------------
// Handlers
// --------------------------------------------------------------------------

/// POST /enrollments — create an enrollment.
/// 409 if (user_id, course_id, role) combo already exists.
/// 404 if user or course missing.
async fn create_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateEnrollmentRequest>,
) -> Result<(StatusCode, Json<EnrollmentWithUser>), ApiError> {
    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    // Only teachers and admins can create enrollments.
    if !is_teacher(&auth_role) {
        return Err(ApiError::Forbidden);
    }

    // Verify course exists.
    let course_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(payload.course_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;

    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Verify user exists.
    if !user_exists(&state.db, payload.user_id).await? {
        return Err(ApiError::NotFound);
    }

    let role = payload.role.as_deref().unwrap_or("student");
    validate_enrollment_role(role)?;

    // Canvas: an explicit course_section_id must belong to the course being
    // enrolled in, otherwise the section lookup is a 404.
    if let Some(section_id) = payload.course_section_id {
        ensure_section_in_course(&state.db, section_id, payload.course_id).await?;
    }

    let enrollment = insert_enrollment(
        &state.db,
        payload.user_id,
        payload.course_id,
        payload.course_section_id,
        role,
    )
    .await?;

    Ok((StatusCode::CREATED, Json(enrollment)))
}

/// GET /enrollments — list enrollments.
/// Admin sees all; teachers see enrollments for courses they're teaching.
async fn list_enrollments(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(filter): Query<EnrollmentFilter>,
) -> Result<Json<Vec<EnrollmentWithUser>>, ApiError> {
    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    let rows: Vec<EnrollmentWithUser> = if is_admin(&auth_role) {
        // Admin: return all, optionally filtered.
        if let Some(user_id) = filter.user_id {
            sqlx::query_as(
                r#"
                SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
                       e.workflow_state, e.created_at, u.name, u.email
                FROM enrollments e
                JOIN users u ON u.id = e.user_id
                WHERE e.user_id = $1 AND e.workflow_state != 'deleted'
                ORDER BY e.created_at DESC
                "#,
            )
            .bind(user_id)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::from)?
        } else if let Some(course_id) = filter.course_id {
            sqlx::query_as(
                r#"
                SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
                       e.workflow_state, e.created_at, u.name, u.email
                FROM enrollments e
                JOIN users u ON u.id = e.user_id
                WHERE e.course_id = $1 AND e.workflow_state != 'deleted'
                ORDER BY e.created_at DESC
                "#,
            )
            .bind(course_id)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::from)?
        } else {
            sqlx::query_as(
                r#"
                SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
                       e.workflow_state, e.created_at, u.name, u.email
                FROM enrollments e
                JOIN users u ON u.id = e.user_id
                WHERE e.workflow_state != 'deleted'
                ORDER BY e.created_at DESC
                "#,
            )
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::from)?
        }
    } else {
        // Non-admin: only enrollments for courses this user teaches.
        let enrolled_course_ids: Vec<(i64,)> = sqlx::query_as(
            r#"
            SELECT course_id FROM enrollments
            WHERE user_id = $1 AND role = 'teacher'
            "#,
        )
        .bind(auth.id)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::from)?;

        let course_ids: Vec<i64> = enrolled_course_ids.into_iter().map(|(id,)| id).collect();

        if course_ids.is_empty() {
            return Ok(Json(vec![]));
        }

        if let Some(user_id) = filter.user_id {
            sqlx::query_as(
                r#"
                SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
                       e.workflow_state, e.created_at, u.name, u.email
                FROM enrollments e
                JOIN users u ON u.id = e.user_id
                WHERE e.course_id = ANY($1) AND e.user_id = $2 AND e.workflow_state != 'deleted'
                ORDER BY e.created_at DESC
                "#,
            )
            .bind(&course_ids)
            .bind(user_id)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::from)?
        } else if let Some(course_id) = filter.course_id {
            if !course_ids.contains(&course_id) {
                return Err(ApiError::Forbidden);
            }
            sqlx::query_as(
                r#"
                SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
                       e.workflow_state, e.created_at, u.name, u.email
                FROM enrollments e
                JOIN users u ON u.id = e.user_id
                WHERE e.course_id = $1 AND e.workflow_state != 'deleted'
                ORDER BY e.created_at DESC
                "#,
            )
            .bind(course_id)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::from)?
        } else {
            sqlx::query_as(
                r#"
                SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
                       e.workflow_state, e.created_at, u.name, u.email
                FROM enrollments e
                JOIN users u ON u.id = e.user_id
                WHERE e.course_id = ANY($1) AND e.workflow_state != 'deleted'
                ORDER BY e.created_at DESC
                "#,
            )
            .bind(&course_ids)
            .fetch_all(&state.db)
            .await
            .map_err(ApiError::from)?
        }
    };

    Ok(Json(rows))
}

/// DELETE /enrollments/{id} — delete an enrollment.
/// Course teacher or admin only.
async fn delete_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(enrollment_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    // Fetch the enrollment to know which course it's in.
    let enrollment: Option<(i64, i64)> =
        sqlx::query_as("SELECT course_id, user_id FROM enrollments WHERE id = $1")
            .bind(enrollment_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::from)?;

    let (course_id, _enrolled_user_id) = enrollment.ok_or(ApiError::NotFound)?;

    // Check permission: must be admin, or must be the teacher of this course.
    if !is_admin(&auth_role)
        && !is_teacher_in_course(&state.db, auth.id, course_id, &auth_role).await?
    {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        "UPDATE enrollments SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(enrollment_id)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

// --------------------------------------------------------------------------
// Section-scoped enrollment routes (Canvas EnrollmentsApiController)
// --------------------------------------------------------------------------

/// GET /sections/{section_id}/enrollments — Canvas "List enrollments" scoped to
/// a section.
///
/// Admins and teachers of the section's course see every enrollment in the
/// section. Any other user may list their own enrollment in that section, as
/// documented for the Canvas enrollments API ("A user can, however, return
/// his/her own enrollments").
async fn list_section_enrollments(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(section_id): Path<i64>,
) -> Result<Json<Vec<EnrollmentWithUser>>, ApiError> {
    let section = load_section(&state.db, section_id).await?;

    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    let can_read_roster = is_admin(&auth_role)
        || is_teacher_in_course(&state.db, auth.id, section.course_id, &auth_role).await?;

    let rows: Vec<EnrollmentWithUser> = if can_read_roster {
        sqlx::query_as(
            r#"
            SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
                   e.workflow_state, e.created_at, u.name, u.email
            FROM enrollments e
            JOIN users u ON u.id = e.user_id
            WHERE e.course_section_id = $1 AND e.workflow_state != 'deleted'
            ORDER BY e.created_at DESC
            "#,
        )
        .bind(section_id)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::from)?
    } else {
        sqlx::query_as(
            r#"
            SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
                   e.workflow_state, e.created_at, u.name, u.email
            FROM enrollments e
            JOIN users u ON u.id = e.user_id
            WHERE e.course_section_id = $1 AND e.user_id = $2
              AND e.workflow_state != 'deleted'
            ORDER BY e.created_at DESC
            "#,
        )
        .bind(section_id)
        .bind(auth.id)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::from)?
    };

    Ok(Json(rows))
}

/// POST /sections/{section_id}/enrollments — Canvas "Enroll a user" scoped to a
/// section. The course is derived from the section, so a section that does not
/// exist (or is not in a course) is a 404 and no cross-course enrollment is
/// possible.
async fn create_section_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(section_id): Path<i64>,
    Json(payload): Json<CreateSectionEnrollmentRequest>,
) -> Result<(StatusCode, Json<EnrollmentWithUser>), ApiError> {
    let section = load_section(&state.db, section_id).await?;

    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    // Course teachers (or admins) may enroll users in the section's course.
    if !is_admin(&auth_role)
        && !is_teacher_in_course(&state.db, auth.id, section.course_id, &auth_role).await?
    {
        return Err(ApiError::Forbidden);
    }

    if !user_exists(&state.db, payload.user_id).await? {
        return Err(ApiError::NotFound);
    }

    let role = payload.role.as_deref().unwrap_or("student");
    validate_enrollment_role(role)?;

    let enrollment = insert_enrollment(
        &state.db,
        payload.user_id,
        section.course_id,
        Some(section.id),
        role,
    )
    .await?;

    Ok((StatusCode::CREATED, Json(enrollment)))
}

// --------------------------------------------------------------------------
// Enrollment lifecycle (Canvas EnrollmentsApiController: accept/reject/
// reactivate) and the user-scoped enrollment listing.
// --------------------------------------------------------------------------

/// Load one enrollment within a course. Canvas scopes every lifecycle action
/// through `@context.enrollments.find(params[:id])`, so an enrollment that
/// belongs to another course (or was soft-deleted) is a 404.
async fn load_course_enrollment(
    pool: &PgPool,
    course_id: i64,
    enrollment_id: i64,
) -> Result<EnrollmentStateRow, ApiError> {
    sqlx::query_as(
        r#"
        SELECT user_id, workflow_state
        FROM enrollments
        WHERE id = $1 AND course_id = $2 AND workflow_state != 'deleted'
        "#,
    )
    .bind(enrollment_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::from)?
    .ok_or(ApiError::NotFound)
}

/// POST /courses/{courseId}/enrollments/{id}/accept — accept a pending course
/// invitation.
///
/// Canvas returns 200 `{ success: true }` for an already-active membership,
/// 400 for an inactive one, and lets a previously rejected invitation be
/// accepted (it is flipped back to invited before activating).
async fn accept_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, enrollment_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, EnrollmentActionError> {
    let enrollment = load_course_enrollment(&state.db, course_id, enrollment_id).await?;

    // Canvas: only the invited user may accept their own invitation.
    if enrollment.user_id != auth.id {
        return Err(ApiError::Forbidden.into());
    }

    match enrollment.workflow_state.as_str() {
        "active" => return Ok(Json(json!({ "success": true }))),
        "invited" | "rejected" => {}
        "inactive" => {
            return Err(EnrollmentActionError::Invalid(
                "membership not activated".to_string(),
            ))
        }
        _ => {
            return Err(EnrollmentActionError::Invalid(
                "no current invitation".to_string(),
            ))
        }
    }

    sqlx::query(
        "UPDATE enrollments SET workflow_state = 'active', updated_at = now() WHERE id = $1",
    )
    .bind(enrollment_id)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(json!({ "success": true })))
}

/// POST /courses/{courseId}/enrollments/{id}/reject — reject a pending course
/// invitation. Rejecting an already-rejected invitation is still a success.
async fn reject_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, enrollment_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, EnrollmentActionError> {
    let enrollment = load_course_enrollment(&state.db, course_id, enrollment_id).await?;

    if enrollment.user_id != auth.id {
        return Err(ApiError::Forbidden.into());
    }

    match enrollment.workflow_state.as_str() {
        "rejected" => return Ok(Json(json!({ "success": true }))),
        "invited" => {}
        "inactive" => {
            return Err(EnrollmentActionError::Invalid(
                "membership not activated".to_string(),
            ))
        }
        _ => {
            return Err(EnrollmentActionError::Invalid(
                "no current invitation".to_string(),
            ))
        }
    }

    sqlx::query(
        "UPDATE enrollments SET workflow_state = 'rejected', updated_at = now() WHERE id = $1",
    )
    .bind(enrollment_id)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(json!({ "success": true })))
}

/// PUT /courses/{courseId}/enrollments/{id}/reactivate — re-activate an
/// inactive enrollment, returning the Canvas enrollment shape.
///
/// Mirrors `Enrollment#can_be_deleted_by`: course teachers (who can remove an
/// enrollment) and admins may reactivate it. Any other state is a 400.
async fn reactivate_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, enrollment_id)): Path<(i64, i64)>,
) -> Result<Json<EnrollmentJson>, EnrollmentActionError> {
    let enrollment = load_course_enrollment(&state.db, course_id, enrollment_id).await?;

    let auth_role = get_user_role(&state.db, auth.id)
        .await?
        .unwrap_or_else(|| "student".to_string());

    if !is_admin(&auth_role)
        && !is_teacher_in_course(&state.db, auth.id, course_id, &auth_role).await?
    {
        return Err(ApiError::Forbidden.into());
    }

    if enrollment.workflow_state != "inactive" {
        return Err(EnrollmentActionError::Invalid(
            "enrollment not inactive".to_string(),
        ));
    }

    let updated: EnrollmentJson = sqlx::query_as(
        r#"
        UPDATE enrollments
        SET workflow_state = 'active', updated_at = now()
        WHERE id = $1 AND course_id = $2
        RETURNING id, user_id, course_id, course_section_id, role, workflow_state,
                  created_at, updated_at, start_at, end_at,
                  last_activity_at, last_attended_at, total_activity_time
        "#,
    )
    .bind(enrollment_id)
    .bind(course_id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(updated))
}

/// Valid `state` values for `GET /users/{userId}/enrollments`, mirroring the
/// list Canvas accepts in `user_index_enrollments`. The composite values are
/// expanded into the concrete workflow states this schema stores.
fn state_filter(state: &str) -> Option<Vec<String>> {
    let states: Vec<String> = match state {
        "current_and_invited" | "current_future_and_restricted" => {
            vec!["active".into(), "invited".into()]
        }
        "current_and_future" | "current_and_concluded" => {
            vec!["active".into(), "invited".into(), "completed".into()]
        }
        "active" | "invited" | "inactive" | "rejected" | "creation_pending" | "pending_active"
        | "pending_invited" | "completed" => vec![state.to_string()],
        _ => return None,
    };
    Some(states)
}

/// GET /users/{userId}/enrollments — list one user's enrollments.
///
/// Canvas: the user themselves always sees their own enrollments, while other
/// callers need roster rights on the user's courses. Here that maps to
/// admins (all courses) and course teachers (their own courses only); everyone
/// else is forbidden. `self` is accepted as the user id, matching `api_find`.
async fn list_user_enrollments(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_ref): Path<String>,
    Query(filter): Query<UserEnrollmentFilter>,
) -> Result<Json<Vec<EnrollmentJson>>, EnrollmentActionError> {
    let target_user_id = if user_ref == "self" {
        auth.id
    } else {
        user_ref.parse::<i64>().map_err(|_| ApiError::NotFound)?
    };

    if !user_exists(&state.db, target_user_id).await? {
        return Err(ApiError::NotFound.into());
    }

    let auth_role = get_user_role(&state.db, auth.id)
        .await?
        .unwrap_or_else(|| "student".to_string());

    let is_self = target_user_id == auth.id;

    // Canvas defaults to the user's current + invited enrollments; `state`
    // narrows (or widens) that set.
    let states = match filter.state.as_deref() {
        Some(state) => state_filter(state)
            .ok_or_else(|| EnrollmentActionError::Invalid(format!("Invalid state {state}")))?,
        None => vec!["active".to_string(), "invited".to_string()],
    };

    if let Some(role) = filter.role.as_deref() {
        if role != "student" && role != "teacher" && role != "ta" {
            return Err(EnrollmentActionError::Invalid("role not found".to_string()));
        }
    }

    // Non-self, non-admin callers are scoped to courses they teach.
    let teacher_course_ids: Option<Vec<i64>> = if is_self || is_admin(&auth_role) {
        None
    } else {
        let rows: Vec<(i64,)> = sqlx::query_as(
            r#"
            SELECT course_id FROM enrollments
            WHERE user_id = $1 AND role = 'teacher' AND workflow_state != 'deleted'
            "#,
        )
        .bind(auth.id)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::from)?;

        let course_ids: Vec<i64> = rows.into_iter().map(|(id,)| id).collect();
        if course_ids.is_empty() {
            return Err(ApiError::Forbidden.into());
        }
        Some(course_ids)
    };

    let rows: Vec<EnrollmentJson> = sqlx::query_as(
        r#"
        SELECT e.id, e.user_id, e.course_id, e.course_section_id, e.role,
               e.workflow_state, e.created_at, e.updated_at, e.start_at, e.end_at,
               e.last_activity_at, e.last_attended_at, e.total_activity_time
        FROM enrollments e
        WHERE e.user_id = $1
          AND e.workflow_state = ANY($2::text[])
          AND ($3::text IS NULL OR e.role = $3)
          AND ($4::bigint IS NULL OR e.course_id = $4)
          AND ($5::bigint[] IS NULL OR e.course_id = ANY($5::bigint[]))
        ORDER BY e.created_at DESC
        "#,
    )
    .bind(target_user_id)
    .bind(&states)
    .bind(filter.role.as_deref())
    .bind(filter.course_id)
    .bind(teacher_course_ids)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(rows))
}

// --------------------------------------------------------------------------
// Temporary enrollment status (Canvas EnrollmentsApiController
// #show_temporary_enrollment_status / #bulk_temporary_enrollment_status)
// --------------------------------------------------------------------------

/// Compute one user's temporary enrollment status flags.
///
/// Canvas scopes these to `Enrollment.all_active_or_pending` joined to
/// available courses. This schema has no course workflow state, so the
/// `workflow_state = 'active'` enrollment filter is the whole condition.
async fn temporary_enrollment_status(
    pool: &PgPool,
    user_id: i64,
) -> Result<serde_json::Value, ApiError> {
    let (is_provider,): (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM enrollments
            WHERE temporary_enrollment_source_user_id = $1 AND workflow_state = 'active'
        )
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)?;

    let (is_recipient,): (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM enrollments
            WHERE user_id = $1 AND temporary_enrollment_source_user_id IS NOT NULL
              AND workflow_state = 'active'
        )
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)?;

    let (can_provide,): (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM enrollments WHERE user_id = $1 AND workflow_state = 'active'
        )
        "#,
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)?;

    Ok(json!({
        "is_provider": is_provider,
        "is_recipient": is_recipient,
        "can_provide": can_provide,
    }))
}

/// GET /users/{userId}/temporary_enrollment_status — one user's temporary
/// enrollment flags. A caller may read their own status; other users require
/// admin, mirroring Canvas `grants_right?(:api_show_user)`.
///
/// Canvas (enrollments_api_controller.rb:1096): when the `temporary_enrollments`
/// feature flag is disabled for the account, the response is nil/empty/404.
async fn show_temporary_enrollment_status(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_ref): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let target_user_id = if user_ref == "self" {
        auth.id
    } else {
        user_ref.parse::<i64>().map_err(|_| ApiError::NotFound)?
    };

    if !user_exists(&state.db, target_user_id).await? {
        return Err(ApiError::NotFound);
    }

    let auth_role = get_user_role(&state.db, auth.id)
        .await?
        .unwrap_or_else(|| "student".to_string());

    if target_user_id != auth.id && !is_admin(&auth_role) {
        return Err(ApiError::Forbidden);
    }

    // Canvas checks `@domain_root_account&.feature_enabled?(:temporary_enrollments)`
    // at the account level. If disabled for every account the user has enrollments
    // in, return an empty 200 object so the response is not a valid status object.
    let has_enabled_account: bool = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM feature_flags ff
            JOIN enrollments e ON e.course_id IN (
                SELECT id FROM courses WHERE account_id = ff.context_id
            )
            WHERE e.user_id = $1
              AND ff.context_type = 'Account'
              AND ff.feature = 'temporary_enrollments'
              AND ff.state = 'enabled'
            LIMIT 1
        )
        "#,
    )
    .bind(target_user_id)
    .fetch_one(&state.db)
    .await
    .map_err(ApiError::from)?;

    if !has_enabled_account {
        return Ok(Json(json!({})));
    }

    Ok(Json(
        temporary_enrollment_status(&state.db, target_user_id).await?,
    ))
}

/// GET /temporary_enrollment_status?user_ids=1,2 — statuses for many users.
///
/// Canvas omits users the caller cannot see (it filters on
/// `grants_right?(:api_show_user)`); here that means the caller themselves plus
/// every user when the caller is an admin. A missing `user_ids` is Canvas's
/// 400 with an empty JSON body.
async fn bulk_temporary_enrollment_status(
    State(state): State<AppState>,
    auth: AuthUser,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (user_ids, raw_limit) = match parse_bulk_enrollment_query(raw_query) {
        Ok(x) => x,
        Err(ApiError::BadRequest(_)) => {
            return (StatusCode::BAD_REQUEST, Json(json!({}))).into_response()
        }
        Err(e) => return e.into_response(),
    };

    // Canvas defaults to 10 and caps at 100 (Api::PER_PAGE / MAX_PER_PAGE).
    let limit = raw_limit.unwrap_or(10).clamp(1, 100);

    match bulk_temporary_enrollment_status_inner(&state, &auth, user_ids, limit).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn bulk_temporary_enrollment_status_inner(
    state: &AppState,
    auth: &AuthUser,
    mut user_ids: Vec<i64>,
    limit: i64,
) -> Result<serde_json::Value, ApiError> {
    user_ids.truncate(limit as usize);

    let auth_role = get_user_role(&state.db, auth.id)
        .await?
        .unwrap_or_else(|| "student".to_string());
    let caller_is_admin = is_admin(&auth_role);

    let mut result = serde_json::Map::new();
    for user_id in user_ids {
        if user_id != auth.id && !caller_is_admin {
            continue;
        }
        if !user_exists(&state.db, user_id).await? {
            continue;
        }
        result.insert(
            user_id.to_string(),
            temporary_enrollment_status(&state.db, user_id).await?,
        );
    }

    Ok(serde_json::Value::Object(result))
}

// --------------------------------------------------------------------------
// Last attended date (Canvas EnrollmentsApiController#last_attended)
// --------------------------------------------------------------------------

/// Parse the Canvas `date` argument. Canvas uses `Time.zone.parse`, which
/// accepts an RFC 1123 date; RFC 3339 and plain `YYYY-MM-DD` are accepted here.
fn parse_last_attended_date(raw: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = DateTime::parse_from_rfc2822(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|dt| dt.and_utc())
}

/// PUT /courses/{courseId}/users/{userId}/last_attended — stamp the last
/// attended date on every enrollment of one user in one course.
///
/// Canvas authorizes `[:view_all_grades, :manage_grades]` (course teachers and
/// admins), answers `{ date: ... }`, and returns 400
/// `{ message: "Invalid date time input" }` when `date` does not parse.
async fn update_last_attended(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((course_id, user_id)): Path<(i64, i64)>,
    Query(params): Query<LastAttendedParams>,
) -> Result<Json<serde_json::Value>, EnrollmentActionError> {
    let course_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?;
    if course_exists.is_none() {
        return Err(ApiError::NotFound.into());
    }

    // Canvas requires the target user to be enrolled in the course.
    let enrolled: Option<(i32,)> =
        sqlx::query_as("SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1")
            .bind(user_id)
            .bind(course_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::from)?;
    if enrolled.is_none() {
        return Err(ApiError::NotFound.into());
    }

    let auth_role = get_user_role(&state.db, auth.id)
        .await?
        .unwrap_or_else(|| "student".to_string());

    if !is_admin(&auth_role)
        && !is_teacher_in_course(&state.db, auth.id, course_id, &auth_role).await?
    {
        return Err(ApiError::Forbidden.into());
    }

    let date = params
        .date
        .as_deref()
        .and_then(parse_last_attended_date)
        .ok_or(EnrollmentActionError::InvalidDate)?;

    // Canvas updates every enrollment of the user in the course, including
    // ones in other sections.
    sqlx::query(
        r#"
        UPDATE enrollments
        SET last_attended_at = $1, updated_at = now()
        WHERE course_id = $2 AND user_id = $3
        "#,
    )
    .bind(date)
    .bind(course_id)
    .bind(user_id)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    Ok(Json(json!({ "date": date })))
}
