//! star-enrollments — enrollment management for star-lms.
//!
//! Exposes `pub fn router() -> axum::Router<AppState>`.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
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
