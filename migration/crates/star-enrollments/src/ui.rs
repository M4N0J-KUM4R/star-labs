//! Pub data loaders for the server-rendered people/admin pages (star-ui,
//! issue #73).
//!
//! Same extraction pattern as `star_courses::load_course_for_viewer`: the
//! queries the REST handlers run privately live here as `pub` fns so the
//! shell stays thin and cannot bypass the API's gates. `load_course_roster`
//! improves on the REST enrollment lists by joining the user name/email AND
//! the enrollment workflow_state (the JSON summary shape only carries ids +
//! role), which is what the roster table renders.

use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::ApiError;

use crate::{get_user_role, insert_enrollment, is_teacher, user_exists, validate_enrollment_role};

/// One roster row for the course people page — an enrollment joined with the
/// user's display data and the lifecycle state the row styling keys off.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RosterRow {
    pub enrollment_id: i64,
    pub user_id: i64,
    pub name: String,
    pub email: String,
    pub role: String,
    /// enrollments.workflow_state (active/invited/inactive/...). Rows
    /// predating the state column COALESCE to 'active' like every writer in
    /// this crate does.
    pub workflow_state: String,
}

/// Load the full roster of a course (every non-deleted enrollment, ordered by
/// user name then enrollment id). Visibility gating stays with the caller
/// (the shell's course_context already ran the enrolled-or-admin course
/// gate; mutations add the teacher check).
pub async fn load_course_roster(db: &PgPool, course_id: i64) -> Result<Vec<RosterRow>, ApiError> {
    let rows: Vec<RosterRow> = sqlx::query_as(
        r#"
        SELECT e.id AS enrollment_id, e.user_id, u.name, u.email, e.role,
               COALESCE(e.workflow_state, 'active') AS workflow_state
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        WHERE e.course_id = $1 AND COALESCE(e.workflow_state, 'active') <> 'deleted'
        ORDER BY u.name ASC, e.id ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(db)
    .await
    .map_err(ApiError::from)?;
    Ok(rows)
}

/// Resolve a login (the users.email the Canvas "Add People" flow asks for)
/// to a user id. Case-insensitive exact match; NULL when no such user.
pub async fn find_user_id_by_login(db: &PgPool, login: &str) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT id FROM users WHERE lower(email) = lower($1) LIMIT 1")
            .bind(login.trim())
            .fetch_optional(db)
            .await
            .map_err(ApiError::from)?;
    Ok(row.map(|(id,)| id))
}

/// Enroll a user in a course behind the gate + validation order of the REST
/// `POST /api/v1/courses/:course_id/enrollments` (canvas-src/config/routes.rb:1402,
/// served by `star_courses::create_enrollment`): the actor must be a
/// teacher/admin (role reloaded from the DB, never trusted from the caller),
/// course and user must exist (404), the role must be valid, and a duplicate
/// (user, course, role) triple is a 409. One deliberate difference: this
/// helper takes Canvas' full role set (student/teacher/ta), while
/// `create_enrollment` narrows to student/teacher.
pub async fn enroll_user_in_course(
    db: &PgPool,
    actor: &AuthUser,
    course_id: i64,
    user_id: i64,
    role: &str,
) -> Result<crate::enrollment_json::EnrollmentApiRow, ApiError> {
    let auth_role = get_user_role(db, actor.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    // Only teachers and admins can create enrollments.
    if !is_teacher(&auth_role) {
        return Err(ApiError::Forbidden);
    }

    // Verify course exists.
    let course_exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await
        .map_err(ApiError::from)?;
    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Verify user exists.
    if !user_exists(db, user_id).await? {
        return Err(ApiError::NotFound);
    }

    validate_enrollment_role(role)?;

    insert_enrollment(db, user_id, course_id, None, role).await
}

/// Conclude a user's membership in a course: every non-deleted, non-inactive
/// enrollment of the user flips to `workflow_state = 'inactive'` — the state
/// the REST `reactivate` endpoint (inactive -> active) is the counterpart
/// of. 404 when the user has no enrollment in the course at all.
pub async fn conclude_course_enrollment(
    db: &PgPool,
    course_id: i64,
    user_id: i64,
) -> Result<(), ApiError> {
    let result = sqlx::query(
        r#"
        UPDATE enrollments
        SET workflow_state = 'inactive', updated_at = now()
        WHERE course_id = $1 AND user_id = $2
          AND COALESCE(workflow_state, 'active') NOT IN ('deleted', 'inactive')
        "#,
    )
    .bind(course_id)
    .bind(user_id)
    .execute(db)
    .await
    .map_err(ApiError::from)?;

    if result.rows_affected() == 0 {
        let any: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2")
                .bind(course_id)
                .bind(user_id)
                .fetch_optional(db)
                .await
                .map_err(ApiError::from)?;
        if any.is_none() {
            return Err(ApiError::NotFound);
        }
    }
    Ok(())
}

/// Reactivate a user's concluded membership: inactive enrollments flip back
/// to `workflow_state = 'active'` (same transition as the REST
/// `PUT /courses/{courseId}/enrollments/{id}/reactivate`). 404 when the user
/// has no enrollment in the course; a user with no inactive enrollment is a
/// no-op success (the roster only offers the action on inactive rows).
pub async fn reactivate_course_enrollment(
    db: &PgPool,
    course_id: i64,
    user_id: i64,
) -> Result<(), ApiError> {
    let result = sqlx::query(
        r#"
        UPDATE enrollments
        SET workflow_state = 'active', updated_at = now()
        WHERE course_id = $1 AND user_id = $2 AND workflow_state = 'inactive'
        "#,
    )
    .bind(course_id)
    .bind(user_id)
    .execute(db)
    .await
    .map_err(ApiError::from)?;

    if result.rows_affected() == 0 {
        let any: Option<(i32,)> =
            sqlx::query_as("SELECT 1 FROM enrollments WHERE course_id = $1 AND user_id = $2")
                .bind(course_id)
                .bind(user_id)
                .fetch_optional(db)
                .await
                .map_err(ApiError::from)?;
        if any.is_none() {
            return Err(ApiError::NotFound);
        }
    }
    Ok(())
}
