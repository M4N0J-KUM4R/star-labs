//! star-courses — courses and enrollments for star-lms.
//!
//! Implements the Canvas-style courses API with role-based access control.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// Course record matching Canvas API v1 shape.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Course {
    pub id: i64,
    pub name: String,
    pub course_code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub workflow_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub syllabus_body: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Valid course workflow states (Canvas API v1).
const COURSE_WORKFLOW_STATES: &[&str] = &["available", "unpublished", "completed", "deleted"];

/// Request to create a course.
#[derive(Debug, Deserialize)]
pub struct CreateCourse {
    pub name: String,
    pub course_code: String,
    pub description: Option<String>,
    #[serde(default = "default_workflow_state")]
    pub workflow_state: String,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub syllabus_body: Option<String>,
}

fn default_workflow_state() -> String {
    "available".to_string()
}

/// Request to update a course.
#[derive(Debug, Deserialize)]
pub struct UpdateCourse {
    pub name: Option<String>,
    pub course_code: Option<String>,
    pub description: Option<String>,
    pub workflow_state: Option<String>,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
    pub syllabus_body: Option<String>,
}

/// Flat row from the enrollment+user join query.
/// Used internally with sqlx::FromRow, then mapped to EnrollmentWithUser.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct EnrollmentRow {
    pub id: i64,
    pub role: String,
    pub user_id: i64,
    pub user_name: String,
    pub user_email: String,
}

/// Enrollment record with joined user info.
#[derive(Debug, Clone, Serialize)]
pub struct EnrollmentWithUser {
    pub id: i64,
    pub role: String,
    pub user: EnrollmentUser,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnrollmentUser {
    pub id: i64,
    pub name: String,
    pub email: String,
}

/// Map a flat DB row to the nested EnrollmentWithUser type.
impl From<EnrollmentRow> for EnrollmentWithUser {
    fn from(row: EnrollmentRow) -> Self {
        EnrollmentWithUser {
            id: row.id,
            role: row.role,
            user: EnrollmentUser {
                id: row.user_id,
                name: row.user_name,
                email: row.user_email,
            },
        }
    }
}

/// Request to create an enrollment.
#[derive(Debug, Deserialize)]
pub struct CreateEnrollment {
    pub user_id: i64,
    pub role: Option<String>,
}

/// Pagination query parameters.
#[derive(Debug, Deserialize)]
pub struct Pagination {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

impl Pagination {
    /// Resolve into `(page, per_page, offset)` with defaults applied.
    ///
    /// Defaults: page=1, per_page=50. page is clamped to >= 1 and per_page
    /// to [1, 100]. Pure function so the math is unit-testable.
    pub fn resolve(&self) -> (i64, i64, i64) {
        let page = self.page.unwrap_or(1).max(1);
        let per_page = self.per_page.unwrap_or(50).clamp(1, 100);
        (page, per_page, (page - 1) * per_page)
    }
}

/// Paginated list response.
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedCourses {
    pub data: Vec<Course>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// Paginated enrollments list response (GET /courses/{id}/enrollments).
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedEnrollments {
    pub data: Vec<EnrollmentWithUser>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

// ---------------------------------------------------------------------------
// Permission helpers (pure functions — unit tested)
// ---------------------------------------------------------------------------

/// Check if user is admin.
pub fn is_admin(auth: &AuthUser) -> bool {
    auth.role == "admin"
}

/// Check if user has teacher role (teacher or admin).
pub fn is_teacher(auth: &AuthUser) -> bool {
    auth.role == "teacher" || auth.role == "admin"
}

/// Check if user is enrolled in a course (as any role) or is admin.
pub async fn check_enrolled_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

/// Check if user is a teacher in a course or admin.
pub async fn check_teacher_or_admin(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses — list courses (admin gets all, others get enrolled only)
/// Paginated: ?page=1&per_page=50, max per_page=100
async fn list_courses(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<PaginatedCourses>, ApiError> {
    let (page, per_page, offset) = pag.resolve();

    if is_admin(&auth) {
        // Admin sees all courses except soft-deleted
        let (total_row,): (i64,) = sqlx::query_as(r#"SELECT COUNT(*) FROM courses WHERE workflow_state != 'deleted'""#)
            .fetch_one(&state.db)
            .await?;

        let courses: Vec<Course> = sqlx::query_as(
            r#"
            SELECT id, name, course_code, description, workflow_state, start_at, end_at, syllabus_body, created_at, updated_at
            FROM courses
            WHERE workflow_state != 'deleted'
            ORDER BY created_at DESC, id DESC
            LIMIT $1 OFFSET $2
            "#,
        )
        .bind(per_page)
        .bind(offset)
        .fetch_all(&state.db)
        .await?;

        return Ok(Json(PaginatedCourses {
            total: total_row,
            page,
            per_page,
            data: courses,
        }));
    }

    // Non-admin: only courses the user is enrolled in
    // COUNT(DISTINCT course_id): a user can hold multiple roles (student +
    // teacher) in the same course, so raw enrollment rows overcount.
    let (total_row,): (i64,) = sqlx::query_as(
        r#"SELECT COUNT(DISTINCT e.course_id) FROM enrollments e JOIN courses c ON c.id = e.course_id WHERE e.user_id = $1 AND c.workflow_state != 'deleted'"#,
    )
    .bind(auth.id)
    .fetch_one(&state.db)
    .await?;

    let courses: Vec<Course> = sqlx::query_as(
        r#"
        SELECT DISTINCT c.id, c.name, c.course_code, c.description, c.workflow_state, c.start_at, c.end_at, c.syllabus_body, c.created_at, c.updated_at
        FROM courses c
        JOIN enrollments e ON e.course_id = c.id
        WHERE e.user_id = $1 AND c.workflow_state != 'deleted'
        ORDER BY c.created_at DESC, c.id DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(auth.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(PaginatedCourses {
        total: total_row,
        page,
        per_page,
        data: courses,
    }))
}

/// POST /courses — create a course (teacher or admin only)
/// Auto-creates a teacher enrollment for the creator.
async fn create_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateCourse>,
) -> Result<(StatusCode, Json<Course>), ApiError> {
    if !is_teacher(&auth) {
        return Err(ApiError::Forbidden);
    }

    if payload.name.trim().is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }
    if payload.course_code.trim().is_empty() {
        return Err(ApiError::Validation("course_code is required".into()));
    }

    // Validate workflow_state
    if !COURSE_WORKFLOW_STATES.contains(&payload.workflow_state.as_str()) {
        return Err(ApiError::Validation(format!(
            "workflow_state must be one of: {}",
            COURSE_WORKFLOW_STATES.join(", ")
        )));
    }

    let course: Course = sqlx::query_as(
        r#"
        INSERT INTO courses (name, course_code, description, workflow_state, start_at, end_at, syllabus_body)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id, name, course_code, description, workflow_state, start_at, end_at, syllabus_body, created_at, updated_at
        "#,
    )
    .bind(&payload.name)
    .bind(&payload.course_code)
    .bind(&payload.description)
    .bind(&payload.workflow_state)
    .bind(&payload.start_at)
    .bind(&payload.end_at)
    .bind(&payload.syllabus_body)
    .fetch_one(&state.db)
    .await?;

    // Auto-create teacher enrollment for the creator
    sqlx::query(
        r#"
        INSERT INTO enrollments (user_id, course_id, role)
        VALUES ($1, $2, 'teacher')
        "#,
    )
    .bind(auth.id)
    .bind(course.id)
    .execute(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(course)))
}

/// GET /courses/{id} — get a single course (enrolled users or admin)
async fn get_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Course>, ApiError> {
    let course: Option<Course> = sqlx::query_as(
        r#"
        SELECT id, name, course_code, description, workflow_state, start_at, end_at, syllabus_body, created_at, updated_at
        FROM courses WHERE id = $1
        "#,
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    let course = course.ok_or(ApiError::NotFound)?;

    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(course))
}

/// PUT /courses/{id} — update a course (course teacher or admin)
async fn update_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateCourse>,
) -> Result<Json<Course>, ApiError> {
    // Check course exists
    let existing: Option<Course> = sqlx::query_as(
        r#"
        SELECT id, name, course_code, description, workflow_state, start_at, end_at, syllabus_body, created_at, updated_at
        FROM courses WHERE id = $1
        "#,
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    let existing = existing.ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Validate workflow_state if provided
    if let Some(ref ws) = payload.workflow_state {
        if !COURSE_WORKFLOW_STATES.contains(&ws.as_str()) {
            return Err(ApiError::Validation(format!(
                "workflow_state must be one of: {}",
                COURSE_WORKFLOW_STATES.join(", ")
            )));
        }
    }

    let name = payload.name.unwrap_or(existing.name);
    let course_code = payload.course_code.unwrap_or(existing.course_code);
    let description = payload.description.or(existing.description);
    let workflow_state = payload.workflow_state.unwrap_or(existing.workflow_state);
    let start_at = payload.start_at.or(existing.start_at);
    let end_at = payload.end_at.or(existing.end_at);
    let syllabus_body = payload.syllabus_body.or(existing.syllabus_body);

    let updated: Course = sqlx::query_as(
        r#"
        UPDATE courses
        SET name = $2, course_code = $3, description = $4, workflow_state = $5,
            start_at = $6, end_at = $7, syllabus_body = $8, updated_at = now()
        WHERE id = $1
        RETURNING id, name, course_code, description, workflow_state, start_at, end_at, syllabus_body, created_at, updated_at
        "#,
    )
    .bind(course_id)
    .bind(&name)
    .bind(&course_code)
    .bind(&description)
    .bind(&workflow_state)
    .bind(&start_at)
    .bind(&end_at)
    .bind(&syllabus_body)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /courses/{id} — soft-delete a course (admin only: sets workflow_state = 'deleted')
async fn delete_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Soft-delete: set workflow_state to 'deleted'
    let result = sqlx::query(r#"UPDATE courses SET workflow_state = 'deleted', updated_at = now() WHERE id = $1 AND workflow_state != 'deleted'"#)
        .bind(course_id)
        .execute(&state.db)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /courses/{id}/enrollments — create an enrollment (course teacher or admin)
/// Returns 409 on duplicate enrollment, 404 if user or course missing.
async fn create_enrollment(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateEnrollment>,
) -> Result<(StatusCode, Json<EnrollmentWithUser>), ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify course exists
    let course_exists: Option<(i64,)> = sqlx::query_as(r#"SELECT 1 FROM courses WHERE id = $1"#)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;

    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Verify user exists
    let user_exists: Option<(i64,)> = sqlx::query_as(r#"SELECT 1 FROM users WHERE id = $1"#)
        .bind(payload.user_id)
        .fetch_optional(&state.db)
        .await?;

    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    let role = payload.role.as_deref().unwrap_or("student");
    if role != "student" && role != "teacher" {
        return Err(ApiError::Validation(
            "role must be 'student' or 'teacher'".into(),
        ));
    }

    // Try to insert; UNIQUE constraint triggers conflict -> 409
    let result = sqlx::query(
        r#"
        INSERT INTO enrollments (user_id, course_id, role)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(payload.user_id)
    .bind(course_id)
    .bind(role)
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23505") => {
            return Err(ApiError::Conflict(format!(
                "user {} is already enrolled in course {} as {}",
                payload.user_id, course_id, role
            )));
        }
        Err(e) => return Err(ApiError::Db(e.to_string())),
    }

    // Fetch the created enrollment with user info
    let row: EnrollmentRow = sqlx::query_as(
        r#"
        SELECT e.id, e.role, u.id as user_id, u.name as user_name, u.email as user_email
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        WHERE e.user_id = $1 AND e.course_id = $2 AND e.role = $3
        "#,
    )
    .bind(payload.user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(&state.db)
    .await?;

    Ok((StatusCode::CREATED, Json(row.into())))
}

/// GET /courses/{id}/enrollments — list enrollments with user info (course teacher or admin)
/// Paginated: ?page=1&per_page=50, max per_page=100
async fn list_enrollments(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<PaginatedEnrollments>, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let (page, per_page, offset) = pag.resolve();

    let (total,): (i64,) =
        sqlx::query_as(r#"SELECT COUNT(*) FROM enrollments WHERE course_id = $1"#)
            .bind(course_id)
            .fetch_one(&state.db)
            .await?;

    let rows: Vec<EnrollmentRow> = sqlx::query_as(
        r#"
        SELECT e.id, e.role, u.id as user_id, u.name as user_name, u.email as user_email
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        WHERE e.course_id = $1
        ORDER BY e.created_at ASC, e.id ASC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(course_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(PaginatedEnrollments {
        data: rows.into_iter().map(Into::into).collect(),
        page,
        per_page,
        total,
    }))
}

/// DELETE /courses/{id}/enrollments/{userId} — remove an enrollment (course teacher or admin)
async fn delete_enrollment(
    State(state): State<AppState>,
    Path((course_id, user_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<StatusCode, ApiError> {
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        r#"DELETE FROM enrollments WHERE course_id = $1 AND user_id = $2"#,
    )
    .bind(course_id)
    .bind(user_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the courses router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses", get(list_courses))
        .route("/courses", post(create_course))
        .route("/courses/{id}", get(get_course))
        .route("/courses/{id}", put(update_course))
        .route("/courses/{id}", delete(delete_course))
        .route("/courses/{id}/enrollments", post(create_enrollment))
        .route("/courses/{id}/enrollments", get(list_enrollments))
        .route("/courses/{id}/enrollments/{userId}", delete(delete_enrollment))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_admin_true() {
        let auth = AuthUser { id: 1, role: "admin".into() };
        assert!(is_admin(&auth));
        assert!(is_teacher(&auth)); // admin can teach
    }

    #[test]
    fn test_is_admin_false() {
        let auth = AuthUser { id: 1, role: "student".into() };
        assert!(!is_admin(&auth));
        assert!(!is_teacher(&auth));
    }

    #[test]
    fn test_is_teacher_true_admin() {
        let auth = AuthUser { id: 1, role: "admin".into() };
        // is_teacher returns true for both teacher and admin (has course mgmt rights)
        assert!(is_teacher(&auth));
    }

    #[test]
    fn test_is_teacher_true_teacher() {
        let auth = AuthUser { id: 1, role: "teacher".into() };
        assert!(is_teacher(&auth));
        assert!(!is_admin(&auth));
    }

    #[test]
    fn test_is_teacher_false_student() {
        let auth = AuthUser { id: 1, role: "student".into() };
        assert!(!is_teacher(&auth));
        assert!(!is_admin(&auth));
    }

    #[test]
    fn test_pagination_defaults() {
        let pag = Pagination { page: None, per_page: None };
        assert_eq!(pag.resolve(), (1, 50, 0));
    }

    #[test]
    fn test_pagination_page2_offset() {
        let pag = Pagination { page: Some(2), per_page: Some(10) };
        assert_eq!(pag.resolve(), (2, 10, 10));
    }

    #[test]
    fn test_pagination_page_zero_clamped_to_one() {
        let pag = Pagination { page: Some(0), per_page: Some(10) };
        assert_eq!(pag.resolve(), (1, 10, 0));
    }

    #[test]
    fn test_pagination_page_negative_clamped_to_one() {
        let pag = Pagination { page: Some(-3), per_page: Some(10) };
        assert_eq!(pag.resolve(), (1, 10, 0));
    }

    #[test]
    fn test_pagination_per_page_clamped_to_100() {
        let pag = Pagination { page: Some(1), per_page: Some(500) };
        assert_eq!(pag.resolve(), (1, 100, 0));
    }

    #[test]
    fn test_pagination_per_page_zero_clamped_to_one() {
        let pag = Pagination { page: Some(1), per_page: Some(0) };
        assert_eq!(pag.resolve(), (1, 1, 0));
    }

    #[test]
    fn test_pagination_per_page_negative_clamped_to_one() {
        let pag = Pagination { page: Some(4), per_page: Some(-2) };
        assert_eq!(pag.resolve(), (4, 1, 3));
    }

    #[test]
    fn test_pagination_offset_arithmetic() {
        // page=7, per_page=25 -> offset 150
        let pag = Pagination { page: Some(7), per_page: Some(25) };
        assert_eq!(pag.resolve(), (7, 25, 150));
    }
}

#[cfg(all(test, feature = "db-tests"))]
mod db_tests {
    //! DB integration tests — skipped if DATABASE_URL is not set.
    //!
    //! Run with: DATABASE_URL=postgres://... cargo test db_tests

    use super::*;
    use sqlx::PgPool;

    fn get_db_url() -> Option<String> {
        std::env::var("DATABASE_URL").ok()
    }

    #[sqlx::test]
    async fn test_create_and_get_course(pool: PgPool) {
        // Create a course
        let course: Course = sqlx::query_as(
            r#"
            INSERT INTO courses (name, course_code, description)
            VALUES ('Rust 101', 'RUST-101', 'Intro to Rust')
            RETURNING id, name, course_code, description, created_at, updated_at
            "#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(course.name, "Rust 101");
        assert_eq!(course.course_code, "RUST-101");

        // Fetch it back
        let found: Option<Course> = sqlx::query_as(
            r#"SELECT id, name, course_code, description, created_at, updated_at FROM courses WHERE id = $1"#,
        )
        .bind(course.id)
        .fetch_optional(&pool)
        .await
        .unwrap();

        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "Rust 101");
    }

    #[sqlx::test]
    async fn test_enrollment_cascade(pool: PgPool) {
        // Get existing user (from fixtures) or create one
        let user: (i64,) = sqlx::query_as(r#"SELECT id FROM users LIMIT 1"#)
            .fetch_one(&pool)
            .await
            .expect("test requires at least one user in the database");

        // Create course
        let course: Course = sqlx::query_as(
            r#"
            INSERT INTO courses (name, course_code)
            VALUES ('DB Test Course', 'DB-TEST')
            RETURNING id, name, course_code, description, created_at, updated_at
            "#,
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        // Enroll user
        sqlx::query(
            r#"INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student')"#,
        )
        .bind(user.0)
        .bind(course.id)
        .execute(&pool)
        .await
        .unwrap();

        // Verify enrollment
        let count: (i64,) = sqlx::query_as(
            r#"SELECT COUNT(*) FROM enrollments WHERE user_id = $1 AND course_id = $2"#,
        )
        .bind(user.0)
        .bind(course.id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(count.0, 1);
    }
}
