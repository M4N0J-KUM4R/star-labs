//! star-courses — courses, enrollment terms, course sections and enrollments
//! for star-lms.
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
use serde_json::{Map, Value};
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

/// Enrollment term (Canvas API v1 EnrollmentTerm shape).
///
/// Mirrors `enrollment_term_json`: id, name, start_at, end_at,
/// workflow_state, created_at. `account_id` is internal (auth scoping).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Term {
    pub id: i64,
    #[serde(skip)]
    pub account_id: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_at: Option<DateTime<Utc>>,
    pub workflow_state: String,
    pub created_at: DateTime<Utc>,
}

/// Canvas list response for GET /accounts/{accountId}/terms.
#[derive(Debug, Clone, Serialize)]
pub struct EnrollmentTermsList {
    pub enrollment_terms: Vec<Term>,
}

/// Course section (Canvas API v1 Section shape requested by the brief).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct CourseSection {
    pub id: i64,
    pub name: String,
    pub course_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_at: Option<DateTime<Utc>>,
    pub workflow_state: String,
}

/// Fields accepted for creating/updating an enrollment term.
#[derive(Debug, Default, Deserialize)]
pub struct TermFields {
    pub name: Option<String>,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
}

/// Canvas wraps term params in `enrollment_term`; flat bodies are accepted too.
#[derive(Debug, Deserialize)]
pub struct TermPayload {
    pub enrollment_term: Option<TermFields>,
    #[serde(flatten)]
    pub flat: TermFields,
}

impl TermPayload {
    fn into_fields(self) -> TermFields {
        self.enrollment_term.unwrap_or(self.flat)
    }
}

/// Fields accepted for creating/updating a course section.
#[derive(Debug, Default, Deserialize)]
pub struct CourseSectionFields {
    pub name: Option<String>,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
}

/// Canvas wraps section params in `course_section`; flat bodies are accepted too.
#[derive(Debug, Deserialize)]
pub struct CourseSectionPayload {
    pub course_section: Option<CourseSectionFields>,
    #[serde(flatten)]
    pub flat: CourseSectionFields,
}

impl CourseSectionPayload {
    fn into_fields(self) -> CourseSectionFields {
        self.course_section.unwrap_or(self.flat)
    }
}

/// Query params for GET /accounts/{accountId}/terms (Canvas subset).
#[derive(Debug, Default, Deserialize)]
pub struct TermListQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub term_name: Option<String>,
    #[serde(rename = "workflow_state[]", default)]
    pub workflow_states: Vec<String>,
    pub workflow_state: Option<String>,
}

impl TermListQuery {
    /// Canvas resolves `workflow_state[]` (plus the bare form) to a filter.
    fn resolved_states(&self) -> Option<Vec<String>> {
        let mut requested = self.workflow_states.clone();
        if let Some(single) = &self.workflow_state {
            requested.push(single.clone());
        }
        resolve_workflow_states(&requested)
    }

    fn pagination(&self) -> (i64, i64, i64) {
        Pagination {
            page: self.page,
            per_page: self.per_page,
        }
        .resolve()
    }
}

/// Query params for GET /courses/{courseId}/sections (Canvas subset).
#[derive(Debug, Default, Deserialize)]
pub struct SectionListQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub search_term: Option<String>,
}

/// Resolve Canvas term `workflow_state[]` values.
///
/// Unknown values are ignored, an empty selection defaults to `active`, and
/// `all` means "no filter" (`None`). Pure helper so it is unit-testable.
fn resolve_workflow_states(requested: &[String]) -> Option<Vec<String>> {
    let mut states: Vec<String> = requested
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| matches!(s.as_str(), "active" | "deleted" | "all"))
        .collect();

    if states.iter().any(|s| s == "all") {
        return None;
    }
    if states.is_empty() {
        states.push("active".to_string());
    }
    states.sort();
    states.dedup();
    Some(states)
}

/// Reject an end date that precedes its start date (Canvas `validate_dates`).
fn validate_date_range(
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
) -> Result<(), ApiError> {
    if let (Some(start_at), Some(end_at)) = (start_at, end_at) {
        if end_at < start_at {
            return Err(ApiError::Validation(
                "end_at cannot be before start_at".into(),
            ));
        }
    }
    Ok(())
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

/// Maximum account hierarchy depth, guarding against circular references.
const MAX_ACCOUNT_TREE_DEPTH: i64 = 10;

/// Check if a user is an account admin for the given account or any parent
/// account in the chain (same semantics as star-accounts).
async fn check_account_admin(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let mut current_id = Some(account_id);
    let mut depth = 0;

    while let Some(id) = current_id {
        if depth >= MAX_ACCOUNT_TREE_DEPTH {
            return Ok(false);
        }
        depth += 1;

        let row: Option<(i32,)> = sqlx::query_as(
            r#"SELECT 1 FROM account_users WHERE account_id = $1 AND user_id = $2 LIMIT 1"#,
        )
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
        if row.is_some() {
            return Ok(true);
        }

        let parent: Option<(Option<i64>,)> =
            sqlx::query_as(r#"SELECT parent_account_id FROM accounts WHERE id = $1"#)
                .bind(id)
                .fetch_optional(pool)
                .await?;
        current_id = parent.and_then(|(parent_id,)| parent_id);
    }

    Ok(false)
}

/// Lightweight existence probe used to return 404 before permission checks.
async fn account_exists(pool: &PgPool, account_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM accounts WHERE id = $1"#)
        .bind(account_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Lightweight existence probe for courses.
async fn course_exists(pool: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM courses WHERE id = $1"#)
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
const ADMIN_COURSE_COUNT_SQL: &str =
    "SELECT COUNT(*) FROM courses WHERE workflow_state != 'deleted'";

async fn list_courses(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<PaginatedCourses>, ApiError> {
    let (page, per_page, offset) = pag.resolve();

    if is_admin(&auth) {
        // Admin sees all courses except soft-deleted
        let (total_row,): (i64,) = sqlx::query_as(ADMIN_COURSE_COUNT_SQL)
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

/// Validate a create-course payload. Shared by both create routes.
fn validate_create_course(payload: &CreateCourse) -> Result<(), ApiError> {
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

    Ok(())
}

/// Create a course and auto-enroll its creator as teacher.
///
/// Shared by POST /courses and the account-scoped alias. `account_id` is the
/// owning account for the alias route; `None` keeps the column default (root
/// account) exactly as POST /courses always has.
async fn create_course_record(
    pool: &PgPool,
    auth: &AuthUser,
    payload: CreateCourse,
    account_id: Option<i64>,
) -> Result<Course, ApiError> {
    if !is_teacher(auth) {
        return Err(ApiError::Forbidden);
    }

    if let Some(account_id) = account_id {
        let account_exists: Option<(i32,)> =
            sqlx::query_as(r#"SELECT 1 FROM accounts WHERE id = $1"#)
                .bind(account_id)
                .fetch_optional(pool)
                .await?;
        if account_exists.is_none() {
            return Err(ApiError::NotFound);
        }
    }

    validate_create_course(&payload)?;

    let course: Course = match account_id {
        Some(account_id) => {
            sqlx::query_as(
                r#"
                INSERT INTO courses (name, course_code, description, workflow_state, start_at, end_at, syllabus_body, account_id)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING id, name, course_code, description, workflow_state, start_at, end_at, syllabus_body, created_at, updated_at
                "#,
            )
            .bind(&payload.name)
            .bind(&payload.course_code)
            .bind(&payload.description)
            .bind(&payload.workflow_state)
            .bind(payload.start_at)
            .bind(payload.end_at)
            .bind(&payload.syllabus_body)
            .bind(account_id)
            .fetch_one(pool)
            .await?
        }
        None => {
            sqlx::query_as(
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
            .bind(payload.start_at)
            .bind(payload.end_at)
            .bind(&payload.syllabus_body)
            .fetch_one(pool)
            .await?
        }
    };

    // Auto-create teacher enrollment for the creator
    sqlx::query(
        r#"
        INSERT INTO enrollments (user_id, course_id, role)
        VALUES ($1, $2, 'teacher')
        "#,
    )
    .bind(auth.id)
    .bind(course.id)
    .execute(pool)
    .await?;

    Ok(course)
}

/// POST /courses — create a course (teacher or admin only)
/// Auto-creates a teacher enrollment for the creator.
async fn create_course(
    State(state): State<AppState>,
    auth: AuthUser,
    Json(payload): Json<CreateCourse>,
) -> Result<(StatusCode, Json<Course>), ApiError> {
    let course = create_course_record(&state.db, &auth, payload, None).await?;
    Ok((StatusCode::CREATED, Json(course)))
}

/// POST /accounts/{id}/courses — Canvas-compatible account-scoped alias for
/// course creation. The account must exist (404 otherwise); teacher/admin
/// authorization and payload validation match POST /courses.
///
/// The path parameter is named `id` to match star-accounts'
/// GET /accounts/{id}/courses: axum's merged router rejects the same route
/// shape with a different parameter name.
async fn create_course_in_account(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CreateCourse>,
) -> Result<(StatusCode, Json<Course>), ApiError> {
    let course = create_course_record(&state.db, &auth, payload, Some(account_id)).await?;
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
    .bind(start_at)
    .bind(end_at)
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
    let course_exists: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM courses WHERE id = $1"#)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?;

    if course_exists.is_none() {
        return Err(ApiError::NotFound);
    }

    // Verify user exists
    let user_exists: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM users WHERE id = $1"#)
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

    let result = sqlx::query(r#"DELETE FROM enrollments WHERE course_id = $1 AND user_id = $2"#)
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
// Enrollment terms (Canvas API v1)
// ---------------------------------------------------------------------------

/// Load a term by id (including soft-deleted ones, matching Canvas show).
async fn fetch_term(pool: &PgPool, term_id: i64) -> Result<Term, ApiError> {
    let term: Option<Term> = sqlx::query_as(
        r#"
        SELECT id, account_id, name, start_at, end_at, workflow_state, created_at
        FROM terms WHERE id = $1
        "#,
    )
    .bind(term_id)
    .fetch_optional(pool)
    .await?;

    term.ok_or(ApiError::NotFound)
}

/// GET /accounts/{accountId}/terms — Canvas list shape
/// `{ "enrollment_terms": [...] }`, defaulting to active terms.
async fn list_terms(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<TermListQuery>,
) -> Result<Json<EnrollmentTermsList>, ApiError> {
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_account_admin(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let (_, per_page, offset) = query.pagination();
    // Canvas workflow_state[] filter -> (include active, include deleted).
    // `all` (None) includes both; the default is active only.
    let (include_active, include_deleted) = match query.resolved_states() {
        None => (true, true),
        Some(states) => (
            states.iter().any(|s| s == "active"),
            states.iter().any(|s| s == "deleted"),
        ),
    };
    let name_filter = query
        .term_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));

    let terms: Vec<Term> = sqlx::query_as(
        r#"
        SELECT id, account_id, name, start_at, end_at, workflow_state, created_at
        FROM terms
        WHERE account_id = $1
          AND (($2 AND workflow_state = 'active') OR ($3 AND workflow_state = 'deleted'))
          AND ($4::text IS NULL OR name ILIKE $4)
        ORDER BY start_at DESC, end_at DESC, id ASC
        LIMIT $5 OFFSET $6
        "#,
    )
    .bind(account_id)
    .bind(include_active)
    .bind(include_deleted)
    .bind(name_filter)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(EnrollmentTermsList {
        enrollment_terms: terms,
    }))
}

/// POST /accounts/{accountId}/terms — create a term (account admin only).
/// Canvas returns 200 with the serialized term.
async fn create_term(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<TermPayload>,
) -> Result<Json<Term>, ApiError> {
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_account_admin(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let fields = payload.into_fields();
    let name = fields.name.as_deref().map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }
    validate_date_range(fields.start_at, fields.end_at)?;

    let term: Term = sqlx::query_as(
        r#"
        INSERT INTO terms (account_id, name, start_at, end_at)
        VALUES ($1, $2, $3, $4)
        RETURNING id, account_id, name, start_at, end_at, workflow_state, created_at
        "#,
    )
    .bind(account_id)
    .bind(name)
    .bind(fields.start_at)
    .bind(fields.end_at)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(term))
}

/// GET /terms/{id} — retrieve a single term (account admin only).
async fn get_term(
    State(state): State<AppState>,
    Path(term_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Term>, ApiError> {
    let term = fetch_term(&state.db, term_id).await?;
    if !check_account_admin(&state.db, auth.id, term.account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(term))
}

/// Shared update logic for flat and account-scoped term routes.
async fn update_term_record(
    pool: &PgPool,
    auth: &AuthUser,
    term: Term,
    payload: TermPayload,
) -> Result<Term, ApiError> {
    // Canvas updates only active terms (deleted ones are 404).
    if term.workflow_state != "active" {
        return Err(ApiError::NotFound);
    }
    if !check_account_admin(pool, auth.id, term.account_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let fields = payload.into_fields();
    if let Some(name) = fields.name.as_deref() {
        if name.trim().is_empty() {
            return Err(ApiError::Validation("name is required".into()));
        }
    }

    let name = fields.name.unwrap_or(term.name);
    let start_at = fields.start_at.or(term.start_at);
    let end_at = fields.end_at.or(term.end_at);
    validate_date_range(start_at, end_at)?;

    let updated: Term = sqlx::query_as(
        r#"
        UPDATE terms
        SET name = $2, start_at = $3, end_at = $4, updated_at = now()
        WHERE id = $1
        RETURNING id, account_id, name, start_at, end_at, workflow_state, created_at
        "#,
    )
    .bind(term.id)
    .bind(&name)
    .bind(start_at)
    .bind(end_at)
    .fetch_one(pool)
    .await?;

    Ok(updated)
}

/// PUT /terms/{id} — partial update (account admin only).
async fn update_term(
    State(state): State<AppState>,
    Path(term_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<TermPayload>,
) -> Result<Json<Term>, ApiError> {
    let term = fetch_term(&state.db, term_id).await?;
    let updated = update_term_record(&state.db, &auth, term, payload).await?;
    Ok(Json(updated))
}

/// Shared soft-delete logic for flat and account-scoped term routes.
async fn delete_term_record(pool: &PgPool, auth: &AuthUser, term: Term) -> Result<Term, ApiError> {
    if !check_account_admin(pool, auth.id, term.account_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    let deleted: Term = sqlx::query_as(
        r#"
        UPDATE terms SET workflow_state = 'deleted', updated_at = now()
        WHERE id = $1
        RETURNING id, account_id, name, start_at, end_at, workflow_state, created_at
        "#,
    )
    .bind(term.id)
    .fetch_one(pool)
    .await?;

    Ok(deleted)
}

/// DELETE /terms/{id} — soft-delete, returning the former term (Canvas shape).
async fn delete_term(
    State(state): State<AppState>,
    Path(term_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Term>, ApiError> {
    let term = fetch_term(&state.db, term_id).await?;
    let deleted = delete_term_record(&state.db, &auth, term).await?;
    Ok(Json(deleted))
}

/// GET /accounts/{accountId}/terms/{id} — Canvas-compatible scoped alias.
async fn get_account_term(
    State(state): State<AppState>,
    Path((account_id, term_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Term>, ApiError> {
    let term = fetch_term(&state.db, term_id).await?;
    if term.account_id != account_id {
        return Err(ApiError::NotFound);
    }
    if !check_account_admin(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(term))
}

/// PUT /accounts/{accountId}/terms/{id} — Canvas-compatible scoped alias.
async fn update_account_term(
    State(state): State<AppState>,
    Path((account_id, term_id)): Path<(i64, i64)>,
    auth: AuthUser,
    Json(payload): Json<TermPayload>,
) -> Result<Json<Term>, ApiError> {
    let term = fetch_term(&state.db, term_id).await?;
    if term.account_id != account_id {
        return Err(ApiError::NotFound);
    }
    let updated = update_term_record(&state.db, &auth, term, payload).await?;
    Ok(Json(updated))
}

/// DELETE /accounts/{accountId}/terms/{id} — Canvas-compatible scoped alias.
async fn delete_account_term(
    State(state): State<AppState>,
    Path((account_id, term_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<Term>, ApiError> {
    let term = fetch_term(&state.db, term_id).await?;
    if term.account_id != account_id {
        return Err(ApiError::NotFound);
    }
    let deleted = delete_term_record(&state.db, &auth, term).await?;
    Ok(Json(deleted))
}

// ---------------------------------------------------------------------------
// Course sections (Canvas API v1)
// ---------------------------------------------------------------------------

/// Load a section by id, or 404.
async fn fetch_section(pool: &PgPool, section_id: i64) -> Result<CourseSection, ApiError> {
    let section: Option<CourseSection> = sqlx::query_as(
        r#"
        SELECT id, name, course_id, start_at, end_at, workflow_state
        FROM course_sections WHERE id = $1
        "#,
    )
    .bind(section_id)
    .fetch_optional(pool)
    .await?;

    section.ok_or(ApiError::NotFound)
}

/// GET /courses/{courseId}/sections — Canvas returns a bare array of sections.
async fn list_sections(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<SectionListQuery>,
) -> Result<Json<Vec<CourseSection>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let (_, per_page, offset) = Pagination {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve();
    let search = query
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"));

    let sections: Vec<CourseSection> = sqlx::query_as(
        r#"
        SELECT id, name, course_id, start_at, end_at, workflow_state
        FROM course_sections
        WHERE course_id = $1
          AND workflow_state = 'active'
          AND ($2::text IS NULL OR name ILIKE $2)
        ORDER BY name ASC, id ASC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(course_id)
    .bind(search)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(sections))
}

/// POST /courses/{courseId}/sections — create a section (course teacher/admin).
/// Canvas returns 200 with the serialized section.
async fn create_section(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CourseSectionPayload>,
) -> Result<Json<CourseSection>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let fields = payload.into_fields();
    let name = fields.name.as_deref().map(str::trim).unwrap_or("");
    if name.is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }
    validate_date_range(fields.start_at, fields.end_at)?;

    let section: CourseSection = sqlx::query_as(
        r#"
        INSERT INTO course_sections (course_id, name, start_at, end_at)
        VALUES ($1, $2, $3, $4)
        RETURNING id, name, course_id, start_at, end_at, workflow_state
        "#,
    )
    .bind(course_id)
    .bind(name)
    .bind(fields.start_at)
    .bind(fields.end_at)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(section))
}

/// GET /sections/{id} — get a section (anyone enrolled or admin).
async fn get_section(
    State(state): State<AppState>,
    Path(section_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<CourseSection>, ApiError> {
    let section = fetch_section(&state.db, section_id).await?;
    if !check_enrolled_or_admin(&state.db, auth.id, section.course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(section))
}

/// GET /courses/{courseId}/sections/{id} — Canvas-compatible course alias.
async fn get_course_section(
    State(state): State<AppState>,
    Path((course_id, section_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<CourseSection>, ApiError> {
    let section = fetch_section(&state.db, section_id).await?;
    if section.course_id != course_id {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(section))
}

/// PUT /sections/{id} — partial update (course teacher/admin).
async fn update_section(
    State(state): State<AppState>,
    Path(section_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<CourseSectionPayload>,
) -> Result<Json<CourseSection>, ApiError> {
    let section = fetch_section(&state.db, section_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, section.course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let fields = payload.into_fields();
    if let Some(name) = fields.name.as_deref() {
        if name.trim().is_empty() {
            return Err(ApiError::Validation("name is required".into()));
        }
    }

    let name = fields.name.unwrap_or(section.name);
    let start_at = fields.start_at.or(section.start_at);
    let end_at = fields.end_at.or(section.end_at);
    validate_date_range(start_at, end_at)?;

    let updated: CourseSection = sqlx::query_as(
        r#"
        UPDATE course_sections
        SET name = $2, start_at = $3, end_at = $4, updated_at = now()
        WHERE id = $1
        RETURNING id, name, course_id, start_at, end_at, workflow_state
        "#,
    )
    .bind(section.id)
    .bind(&name)
    .bind(start_at)
    .bind(end_at)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(updated))
}

/// DELETE /sections/{id} — soft-delete, returning the former section.
///
/// Canvas refuses to delete sections that have enrollments; section
/// enrollments are owned by a separate workstream, so this route soft-deletes.
async fn delete_section(
    State(state): State<AppState>,
    Path(section_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<CourseSection>, ApiError> {
    let section = fetch_section(&state.db, section_id).await?;
    if !check_teacher_or_admin(&state.db, auth.id, section.course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let deleted: CourseSection = sqlx::query_as(
        r#"
        UPDATE course_sections SET workflow_state = 'deleted', updated_at = now()
        WHERE id = $1
        RETURNING id, name, course_id, start_at, end_at, workflow_state
        "#,
    )
    .bind(section.id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(deleted))
}

// ---------------------------------------------------------------------------
// Course settings (Canvas API v1)
// ---------------------------------------------------------------------------

/// Default due time Canvas reports when a course has none configured.
const DEFAULT_DUE_TIME: &str = "23:59:59";

/// Canvas `course_settings_json` (lib/api/v1/course.rb#course_settings_json).
///
/// Every key is always emitted, including nulls, matching Canvas. The
/// `course_settings.settings` document stores this same shape; the struct
/// level `serde(default)` keeps older/partial documents readable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CourseSettings {
    pub allow_final_grade_override: bool,
    pub allow_student_discussion_topics: bool,
    pub allow_student_forum_attachments: bool,
    pub allow_student_discussion_editing: bool,
    pub allow_student_discussion_reporting: bool,
    pub allow_student_anonymous_discussion_topics: bool,
    pub use_default_discussion_settings: bool,
    pub default_discussion_settings: Value,
    pub filter_speed_grader_by_student_group: bool,
    pub grading_standard_enabled: bool,
    pub grading_standard_id: Option<i64>,
    pub grade_passback_setting: Option<String>,
    pub allow_student_organized_groups: bool,
    pub hide_final_grades: bool,
    pub hide_distribution_graphs: bool,
    pub hide_sections_on_course_users_page: bool,
    pub lock_all_announcements: bool,
    pub usage_rights_required: bool,
    pub restrict_student_past_view: bool,
    pub restrict_student_future_view: bool,
    pub restrict_quantitative_data: Option<bool>,
    pub show_announcements_on_home_page: bool,
    pub home_page_announcement_limit: Option<i64>,
    pub syllabus_course_summary: bool,
    pub homeroom_course: bool,
    pub image_url: Option<String>,
    pub image_id: Option<i64>,
    pub image: Option<String>,
    pub banner_image_url: Option<String>,
    pub banner_image_id: Option<i64>,
    pub banner_image: Option<String>,
    pub course_color: Option<String>,
    pub friendly_name: Option<String>,
    pub default_due_time: String,
    pub conditional_release: bool,
    pub default_student_gradebook_view: Option<String>,
}

impl Default for CourseSettings {
    /// Canvas `Course` column defaults for every setting this LMS models.
    fn default() -> Self {
        CourseSettings {
            allow_final_grade_override: false,
            allow_student_discussion_topics: true,
            allow_student_forum_attachments: true,
            allow_student_discussion_editing: true,
            allow_student_discussion_reporting: true,
            allow_student_anonymous_discussion_topics: false,
            use_default_discussion_settings: true,
            default_discussion_settings: Value::Object(Map::new()),
            filter_speed_grader_by_student_group: false,
            grading_standard_enabled: false,
            grading_standard_id: None,
            grade_passback_setting: None,
            allow_student_organized_groups: true,
            hide_final_grades: false,
            hide_distribution_graphs: false,
            hide_sections_on_course_users_page: false,
            lock_all_announcements: false,
            usage_rights_required: false,
            restrict_student_past_view: false,
            restrict_student_future_view: false,
            restrict_quantitative_data: None,
            show_announcements_on_home_page: false,
            home_page_announcement_limit: None,
            syllabus_course_summary: true,
            homeroom_course: false,
            image_url: None,
            image_id: None,
            image: None,
            banner_image_url: None,
            banner_image_id: None,
            banner_image: None,
            course_color: None,
            friendly_name: None,
            default_due_time: DEFAULT_DUE_TIME.to_string(),
            conditional_release: false,
            default_student_gradebook_view: None,
        }
    }
}

/// PUT /courses/{courseId}/settings body: the Canvas-permitted subset that
/// maps onto `CourseSettings` (`CoursesController#update_settings`).
#[derive(Debug, Default, Deserialize)]
pub struct UpdateCourseSettings {
    pub allow_final_grade_override: Option<bool>,
    pub allow_student_discussion_topics: Option<bool>,
    pub allow_student_forum_attachments: Option<bool>,
    pub allow_student_discussion_editing: Option<bool>,
    pub allow_student_discussion_reporting: Option<bool>,
    pub allow_student_anonymous_discussion_topics: Option<bool>,
    pub use_default_discussion_settings: Option<bool>,
    pub default_discussion_settings: Option<Value>,
    pub filter_speed_grader_by_student_group: Option<bool>,
    pub allow_student_organized_groups: Option<bool>,
    pub hide_final_grades: Option<bool>,
    pub hide_distribution_graphs: Option<bool>,
    pub hide_sections_on_course_users_page: Option<bool>,
    pub lock_all_announcements: Option<bool>,
    pub usage_rights_required: Option<bool>,
    pub restrict_student_past_view: Option<bool>,
    pub restrict_student_future_view: Option<bool>,
    pub restrict_quantitative_data: Option<bool>,
    pub show_announcements_on_home_page: Option<bool>,
    pub home_page_announcement_limit: Option<i64>,
    pub syllabus_course_summary: Option<bool>,
    pub homeroom_course: Option<bool>,
    pub course_color: Option<String>,
    pub friendly_name: Option<String>,
    pub conditional_release: Option<bool>,
    pub default_due_time: Option<String>,
    pub default_student_gradebook_view: Option<String>,
}

impl CourseSettings {
    /// Apply the provided (non-absent) Canvas update params.
    ///
    /// Like the other course PUT routes in this crate, absent/null fields
    /// keep their current value (nullable settings cannot be cleared).
    fn apply(&mut self, patch: UpdateCourseSettings) {
        macro_rules! apply_fields {
            ($($field:ident),+ $(,)?) => {
                $(if let Some(value) = patch.$field { self.$field = value; })+
            };
        }
        apply_fields!(
            allow_final_grade_override,
            allow_student_discussion_topics,
            allow_student_forum_attachments,
            allow_student_discussion_editing,
            allow_student_discussion_reporting,
            allow_student_anonymous_discussion_topics,
            use_default_discussion_settings,
            filter_speed_grader_by_student_group,
            allow_student_organized_groups,
            hide_final_grades,
            hide_distribution_graphs,
            hide_sections_on_course_users_page,
            lock_all_announcements,
            usage_rights_required,
            restrict_student_past_view,
            restrict_student_future_view,
            show_announcements_on_home_page,
            syllabus_course_summary,
            homeroom_course,
            conditional_release,
        );

        // Nullable settings keep their update semantics: Some sets, None keeps.
        if let Some(value) = patch.restrict_quantitative_data {
            self.restrict_quantitative_data = Some(value);
        }
        if let Some(value) = patch.home_page_announcement_limit {
            self.home_page_announcement_limit = Some(value);
        }
        if let Some(value) = patch.course_color {
            self.course_color = Some(value);
        }
        if let Some(value) = patch.friendly_name {
            self.friendly_name = Some(value);
        }
        if let Some(value) = patch.default_student_gradebook_view {
            self.default_student_gradebook_view = Some(value);
        }
        if let Some(value) = patch.default_discussion_settings {
            if let Some(settings) = sanitize_discussion_settings(value) {
                self.default_discussion_settings = settings;
            }
        }
        if let Some(value) = patch.default_due_time {
            self.default_due_time =
                normalize_due_time(&value).unwrap_or_else(|| DEFAULT_DUE_TIME.to_string());
        }
    }
}

/// Canvas `normalize_due_time` (`DefaultDueTimeHelper`): accept "HH:MM" or
/// "HH:MM:SS", reject anything out of range/format. Blank and "inherit"
/// mean unset, which Canvas serializes back as the 23:59:59 default.
fn normalize_due_time(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("inherit") {
        return None;
    }

    let parts: Vec<&str> = raw.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let mut nums = [0u32; 3];
    for (idx, part) in parts.iter().enumerate() {
        nums[idx] = part.parse().ok()?;
    }
    let [hour, minute, second] = nums;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(format!("{hour:02}:{minute:02}:{second:02}"))
}

/// Canvas permits a fixed key set for `default_discussion_settings` and only
/// assigns object params; unknown keys are dropped and non-objects ignored.
fn sanitize_discussion_settings(value: Value) -> Option<Value> {
    const ALLOWED: &[&str] = &[
        "anonymous_state",
        "disallow_threaded_replies",
        "require_initial_post",
        "podcast_enabled",
        "podcast_has_student_posts",
        "allow_rating",
        "only_graders_can_rate",
        "expanded",
        "expanded_locked",
        "sort_order",
        "sort_order_locked",
    ];

    match value {
        Value::Object(map) => {
            let filtered: Map<String, Value> = map
                .into_iter()
                .filter(|(key, _)| ALLOWED.contains(&key.as_str()))
                .collect();
            Some(Value::Object(filtered))
        }
        _ => None,
    }
}

/// Load the effective settings document, falling back to Canvas defaults
/// when the course has never saved settings (or the document is unreadable).
async fn load_course_settings(pool: &PgPool, course_id: i64) -> Result<CourseSettings, ApiError> {
    let row: Option<(String,)> =
        sqlx::query_as(r#"SELECT settings FROM course_settings WHERE course_id = $1"#)
            .bind(course_id)
            .fetch_optional(pool)
            .await?;

    Ok(row
        .and_then(|(raw,)| serde_json::from_str::<CourseSettings>(&raw).ok())
        .unwrap_or_default())
}

/// Upsert the effective settings document for a course.
async fn save_course_settings(
    pool: &PgPool,
    course_id: i64,
    settings: &CourseSettings,
) -> Result<(), ApiError> {
    let raw = serde_json::to_string(settings).map_err(|e| ApiError::Db(e.to_string()))?;
    sqlx::query(
        r#"
        INSERT INTO course_settings (course_id, settings)
        VALUES ($1, $2)
        ON CONFLICT (course_id)
        DO UPDATE SET settings = EXCLUDED.settings, updated_at = now()
        "#,
    )
    .bind(course_id)
    .bind(raw)
    .execute(pool)
    .await?;
    Ok(())
}

/// GET /courses/{courseId}/settings — Canvas `course_settings_json`
/// (requires course read; enrolled users or admin).
async fn get_course_settings(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<CourseSettings>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    Ok(Json(load_course_settings(&state.db, course_id).await?))
}

/// PUT /courses/{courseId}/settings — partial update of the Canvas-permitted
/// settings (course teacher or admin; Canvas `update_course_details`).
async fn update_course_settings(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Json(payload): Json<UpdateCourseSettings>,
) -> Result<Json<CourseSettings>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let mut settings = load_course_settings(&state.db, course_id).await?;
    settings.apply(payload);
    save_course_settings(&state.db, course_id, &settings).await?;

    Ok(Json(settings))
}

// ---------------------------------------------------------------------------
// Course roster (Canvas API v1)
// ---------------------------------------------------------------------------

/// Flat row for the course roster query.
#[derive(Debug, Clone, sqlx::FromRow)]
struct CourseUserRow {
    id: i64,
    name: String,
    sortable_name: String,
    email: String,
    role: String,
    created_at: DateTime<Utc>,
}

/// Canvas `user_json` subset returned by the course roster endpoints.
///
/// `short_name` mirrors Canvas (`User#short_name` falls back to the full
/// name); `role` is the user's course enrollment role (this LMS stores one
/// row per enrollment rather than Canvas enrollment objects).
#[derive(Debug, Clone, Serialize)]
pub struct CourseUser {
    pub id: i64,
    pub name: String,
    pub sortable_name: String,
    pub short_name: String,
    pub email: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

impl From<CourseUserRow> for CourseUser {
    fn from(row: CourseUserRow) -> Self {
        CourseUser {
            short_name: row.name.clone(),
            id: row.id,
            name: row.name,
            sortable_name: row.sortable_name,
            email: row.email,
            role: row.role,
            created_at: row.created_at,
        }
    }
}

/// Paginated roster response (repo-wide pagination envelope).
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedCourseUsers {
    pub data: Vec<CourseUser>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// Query params for GET /courses/{courseId}/users (Canvas subset).
#[derive(Debug, Default, Deserialize)]
pub struct CourseUserListQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub search_term: Option<String>,
}

/// LIKE pattern for the roster `search_term` (Canvas searches name/email).
fn roster_search_pattern(term: Option<&str>) -> Option<String> {
    term.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!("%{s}%"))
}

/// GET /courses/{courseId}/users — course roster (Canvas `users` action).
async fn list_course_users(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<CourseUserListQuery>,
) -> Result<Json<PaginatedCourseUsers>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let (page, per_page, offset) = Pagination {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve();
    let search = roster_search_pattern(query.search_term.as_deref());

    let (total,): (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(DISTINCT u.id)
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        WHERE e.course_id = $1
          AND ($2::text IS NULL OR u.name ILIKE $2 OR u.email ILIKE $2)
        "#,
    )
    .bind(course_id)
    .bind(&search)
    .fetch_one(&state.db)
    .await?;

    // One row per user; teacher is preferred when several roles exist.
    let rows: Vec<CourseUserRow> = sqlx::query_as(
        r#"
        SELECT id, name, sortable_name, email, role, created_at
        FROM (
            SELECT DISTINCT ON (u.id)
                   u.id, u.name,
                   COALESCE(u.sortable_name, u.name) AS sortable_name,
                   u.email, e.role, u.created_at
            FROM enrollments e
            JOIN users u ON u.id = e.user_id
            WHERE e.course_id = $1
              AND ($2::text IS NULL OR u.name ILIKE $2 OR u.email ILIKE $2)
            ORDER BY u.id, (e.role = 'teacher') DESC, e.role
        ) AS roster
        ORDER BY sortable_name ASC, id ASC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(course_id)
    .bind(&search)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(PaginatedCourseUsers {
        data: rows.into_iter().map(Into::into).collect(),
        page,
        per_page,
        total,
    }))
}

/// GET /courses/{courseId}/users/{userId} — single roster user.
/// 404 when the user is not enrolled in the course (Canvas visible-users
/// lookup), 403 when the caller cannot read the course.
async fn get_course_user(
    State(state): State<AppState>,
    Path((course_id, user_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<CourseUser>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let row: Option<CourseUserRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (u.id)
               u.id, u.name,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               u.email, e.role, u.created_at
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        WHERE e.course_id = $1 AND u.id = $2
        ORDER BY u.id, (e.role = 'teacher') DESC, e.role
        "#,
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(&state.db)
    .await?;

    row.map(|row| Json(row.into())).ok_or(ApiError::NotFound)
}

// ---------------------------------------------------------------------------
// Course navigation tabs (Canvas API v1)
// ---------------------------------------------------------------------------

/// A Canvas navigation tab (`Api::V1::Tab#tab_json`).
#[derive(Debug, Clone, Serialize)]
pub struct CourseTab {
    pub id: String,
    pub html_url: String,
    pub full_url: String,
    pub position: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub hidden: bool,
    pub visibility: String,
    pub label: String,
    #[serde(rename = "type")]
    pub tab_type: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Canvas `Course.default_tabs` subset implemented by this LMS, in order.
/// Entries are (tab id / css_class, label, path suffix under /courses/:id).
const DEFAULT_COURSE_TABS: &[(&str, &str, &str)] = &[
    ("home", "Home", ""),
    ("announcements", "Announcements", "/announcements"),
    ("assignments", "Assignments", "/assignments"),
    ("discussions", "Discussions", "/discussion_topics"),
    ("grades", "Grades", "/grades"),
    ("people", "People", "/users"),
    ("pages", "Pages", "/pages"),
    ("files", "Files", "/files"),
    ("syllabus", "Syllabus", "/assignments/syllabus"),
    ("modules", "Modules", "/modules"),
    ("outcomes", "Outcomes", "/outcomes"),
    ("quizzes", "Quizzes", "/quizzes"),
];

/// Settings tab: pinned last, only for course teachers/admins, never hidden.
const SETTINGS_COURSE_TAB: (&str, &str, &str) = ("settings", "Settings", "/settings");

/// One persisted row from `course_tabs`.
#[derive(Debug, Clone, sqlx::FromRow)]
struct CourseTabConfig {
    tab_id: String,
    position: i32,
    hidden: bool,
}

/// Merge persisted tab configuration into the defaults the way Canvas does:
/// Home is pinned first and Settings last, Home/Settings cannot be hidden or
/// moved, and other tabs are ordered by configured position then default
/// order.
fn build_course_tabs(
    course_id: i64,
    include_settings: bool,
    configs: &[CourseTabConfig],
) -> Vec<CourseTab> {
    fn tab(course_id: i64, def: (&str, &str, &str), hidden: bool) -> CourseTab {
        let (id, label, suffix) = def;
        let html_url = format!("/courses/{course_id}{suffix}");
        CourseTab {
            id: id.to_string(),
            html_url: html_url.clone(),
            // No absolute host is configured for API responses, so full_url
            // mirrors html_url (relative), matching other star crates.
            full_url: html_url,
            position: 0,
            hidden,
            visibility: if hidden || id == "settings" {
                "admins"
            } else {
                "public"
            }
            .to_string(),
            label: label.to_string(),
            tab_type: "internal".to_string(),
        }
    }

    let mut ordered: Vec<(i32, usize, CourseTab)> = Vec::new();
    for (idx, def) in DEFAULT_COURSE_TABS.iter().enumerate() {
        let config = configs.iter().find(|config| config.tab_id == def.0);
        let manageable = def.0 != "home";
        let position = if manageable {
            config.map(|c| c.position).unwrap_or(idx as i32 + 1)
        } else {
            0
        };
        let hidden = manageable && config.map(|c| c.hidden).unwrap_or(false);
        ordered.push((position, idx, tab(course_id, *def, hidden)));
    }
    ordered.sort_by_key(|(position, idx, _)| (*position, *idx));

    let mut tabs: Vec<CourseTab> = ordered.into_iter().map(|(_, _, t)| t).collect();
    if include_settings {
        tabs.push(tab(course_id, SETTINGS_COURSE_TAB, false));
    }
    for (idx, tab) in tabs.iter_mut().enumerate() {
        tab.position = idx as i64 + 1;
    }
    tabs
}

/// GET /courses/{courseId}/tabs — Canvas `TabsController#index` (bare array,
/// positions renumbered 1..n after configuration ordering).
async fn list_course_tabs(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<CourseTab>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let include_settings = check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    let configs: Vec<CourseTabConfig> = sqlx::query_as(
        r#"
        SELECT tab_id, position, hidden
        FROM course_tabs
        WHERE course_id = $1
        ORDER BY position ASC, id ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(build_course_tabs(
        course_id,
        include_settings,
        &configs,
    )))
}

// ---------------------------------------------------------------------------
// Content shares, activity stream, user courses and ping (Canvas API v1)
// ---------------------------------------------------------------------------

/// Canvas `users_json` subset returned by `GET /courses/:id/content_share_users`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct ContentShareUser {
    pub id: i64,
    pub name: String,
    pub short_name: String,
    pub sortable_name: String,
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

/// Query params for `GET /courses/:id/content_share_users`.
#[derive(Debug, Default, Deserialize)]
pub struct ContentShareUserQuery {
    pub search_term: Option<String>,
}

/// GET /courses/{courseId}/content_share_users — Canvas
/// `CoursesController#content_share_users`.
///
/// Requires `read_as_admin` (403 otherwise) and a `search_term` (Canvas
/// `reject!` -> 400). Returns account teachers/admins whose name or email
/// matches the term, ordered by name (Canvas orders by `users.name`).
async fn list_content_share_users(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<ContentShareUserQuery>,
) -> Result<Json<Vec<ContentShareUser>>, ApiError> {
    let account_id: Option<(i64,)> =
        sqlx::query_as(r#"SELECT COALESCE(account_id, 1) FROM courses WHERE id = $1"#)
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?;
    let (account_id,) = account_id.ok_or(ApiError::NotFound)?;

    if !check_account_admin(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let search_term = query
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::BadRequest("Search term required".into()))?;
    let pattern = format!("%{search_term}%");

    let users: Vec<ContentShareUser> = sqlx::query_as(
        r#"
        SELECT DISTINCT u.id, u.name, u.name AS short_name,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               u.email, p.avatar_url
        FROM users u
        LEFT JOIN user_profiles p ON p.user_id = u.id
        WHERE (u.name ILIKE $2 OR u.email ILIKE $2)
          AND (
            EXISTS (
                SELECT 1 FROM account_users au
                WHERE au.user_id = u.id AND au.account_id = $1
            )
            OR EXISTS (
                SELECT 1 FROM enrollments e
                JOIN courses c ON c.id = e.course_id
                WHERE e.user_id = u.id AND e.role = 'teacher'
                  AND c.account_id = $1
            )
          )
        ORDER BY u.name ASC
        LIMIT 50
        "#,
    )
    .bind(account_id)
    .bind(&pattern)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(users))
}

/// GET /courses/{courseId}/activity_stream/summary — Canvas
/// `CoursesController#activity_stream_summary` (`api_render_stream_summary`).
///
/// Canvas returns a bare array of `{type, notification_category, count,
/// unread_count}`. This LMS has no stream-item storage, so the summary is
/// empty (the Canvas shape for a course with no activity).
async fn course_activity_stream_summary(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(Vec::new()))
}

/// GET /users/{userId}/courses — Canvas `CoursesController#user_index`.
/// Callers may read their own courses; admins may read anyone's.
async fn list_user_courses(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<Course>>, ApiError> {
    let user_exists: Option<(i32,)> = sqlx::query_as(r#"SELECT 1 FROM users WHERE id = $1"#)
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    if user_exists.is_none() {
        return Err(ApiError::NotFound);
    }
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    let courses: Vec<Course> = sqlx::query_as(
        r#"
        SELECT DISTINCT c.id, c.name, c.course_code, c.description, c.workflow_state,
               c.start_at, c.end_at, c.syllabus_body, c.created_at, c.updated_at
        FROM courses c
        JOIN enrollments e ON e.course_id = c.id
        WHERE e.user_id = $1 AND c.workflow_state != 'deleted'
        ORDER BY c.created_at DESC, c.id DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(courses))
}

/// POST /courses/{courseId}/ping — Canvas returns `{"success": true}`.
async fn ping_course(_auth: AuthUser) -> Result<Json<Value>, ApiError> {
    Ok(Json(serde_json::json!({ "success": true })))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the courses router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/courses", get(list_courses))
        .route("/courses", post(create_course))
        .route("/accounts/{id}/courses", post(create_course_in_account))
        .route("/courses/{id}", get(get_course))
        .route("/courses/{id}", put(update_course))
        .route("/courses/{id}", delete(delete_course))
        .route("/courses/{id}/enrollments", post(create_enrollment))
        .route("/courses/{id}/enrollments", get(list_enrollments))
        .route(
            "/courses/{id}/enrollments/{userId}",
            delete(delete_enrollment),
        )
        // Enrollment terms (Canvas API v1)
        .route("/accounts/{accountId}/terms", get(list_terms))
        .route("/accounts/{accountId}/terms", post(create_term))
        .route("/accounts/{accountId}/terms/{id}", get(get_account_term))
        .route("/accounts/{accountId}/terms/{id}", put(update_account_term))
        .route(
            "/accounts/{accountId}/terms/{id}",
            delete(delete_account_term),
        )
        .route("/terms/{id}", get(get_term))
        .route("/terms/{id}", put(update_term))
        .route("/terms/{id}", delete(delete_term))
        // Course sections (Canvas API v1)
        .route("/courses/{courseId}/sections", get(list_sections))
        .route("/courses/{courseId}/sections", post(create_section))
        .route("/courses/{courseId}/sections/{id}", get(get_course_section))
        .route("/sections/{id}", get(get_section))
        .route("/sections/{id}", put(update_section))
        .route("/sections/{id}", delete(delete_section))
        // Course settings, roster and navigation tabs (Canvas API v1)
        .route(
            "/courses/{courseId}/settings",
            get(get_course_settings).put(update_course_settings),
        )
        .route("/courses/{courseId}/users", get(list_course_users))
        .route("/courses/{courseId}/users/{userId}", get(get_course_user))
        .route("/courses/{courseId}/tabs", get(list_course_tabs))
        // Content shares, activity stream, user courses and ping
        .route(
            "/courses/{courseId}/content_share_users",
            get(list_content_share_users),
        )
        .route(
            "/courses/{courseId}/activity_stream/summary",
            get(course_activity_stream_summary),
        )
        .route("/courses/{courseId}/ping", post(ping_course))
        .route("/users/{userId}/courses", get(list_user_courses))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_course_count_query_has_valid_sql_string() {
        assert_eq!(
            ADMIN_COURSE_COUNT_SQL,
            "SELECT COUNT(*) FROM courses WHERE workflow_state != 'deleted'"
        );
    }

    #[test]
    fn test_is_admin_true() {
        let auth = AuthUser {
            id: 1,
            role: "admin".into(),
        };
        assert!(is_admin(&auth));
        assert!(is_teacher(&auth)); // admin can teach
    }

    #[test]
    fn test_is_admin_false() {
        let auth = AuthUser {
            id: 1,
            role: "student".into(),
        };
        assert!(!is_admin(&auth));
        assert!(!is_teacher(&auth));
    }

    #[test]
    fn test_is_teacher_true_admin() {
        let auth = AuthUser {
            id: 1,
            role: "admin".into(),
        };
        // is_teacher returns true for both teacher and admin (has course mgmt rights)
        assert!(is_teacher(&auth));
    }

    #[test]
    fn test_is_teacher_true_teacher() {
        let auth = AuthUser {
            id: 1,
            role: "teacher".into(),
        };
        assert!(is_teacher(&auth));
        assert!(!is_admin(&auth));
    }

    #[test]
    fn test_is_teacher_false_student() {
        let auth = AuthUser {
            id: 1,
            role: "student".into(),
        };
        assert!(!is_teacher(&auth));
        assert!(!is_admin(&auth));
    }

    #[test]
    fn test_pagination_defaults() {
        let pag = Pagination {
            page: None,
            per_page: None,
        };
        assert_eq!(pag.resolve(), (1, 50, 0));
    }

    #[test]
    fn test_pagination_page2_offset() {
        let pag = Pagination {
            page: Some(2),
            per_page: Some(10),
        };
        assert_eq!(pag.resolve(), (2, 10, 10));
    }

    #[test]
    fn test_pagination_page_zero_clamped_to_one() {
        let pag = Pagination {
            page: Some(0),
            per_page: Some(10),
        };
        assert_eq!(pag.resolve(), (1, 10, 0));
    }

    #[test]
    fn test_pagination_page_negative_clamped_to_one() {
        let pag = Pagination {
            page: Some(-3),
            per_page: Some(10),
        };
        assert_eq!(pag.resolve(), (1, 10, 0));
    }

    #[test]
    fn test_pagination_per_page_clamped_to_100() {
        let pag = Pagination {
            page: Some(1),
            per_page: Some(500),
        };
        assert_eq!(pag.resolve(), (1, 100, 0));
    }

    #[test]
    fn test_pagination_per_page_zero_clamped_to_one() {
        let pag = Pagination {
            page: Some(1),
            per_page: Some(0),
        };
        assert_eq!(pag.resolve(), (1, 1, 0));
    }

    #[test]
    fn test_pagination_per_page_negative_clamped_to_one() {
        let pag = Pagination {
            page: Some(4),
            per_page: Some(-2),
        };
        assert_eq!(pag.resolve(), (4, 1, 3));
    }

    #[test]
    fn test_pagination_offset_arithmetic() {
        // page=7, per_page=25 -> offset 150
        let pag = Pagination {
            page: Some(7),
            per_page: Some(25),
        };
        assert_eq!(pag.resolve(), (7, 25, 150));
    }

    #[test]
    fn test_resolve_workflow_states_defaults_to_active() {
        assert_eq!(
            resolve_workflow_states(&[]),
            Some(vec!["active".to_string()])
        );
    }

    #[test]
    fn test_resolve_workflow_states_all_disables_filter() {
        assert_eq!(resolve_workflow_states(&["all".to_string()]), None);
    }

    #[test]
    fn test_validate_date_range_accepts_valid_and_missing_dates() {
        let start = chrono::DateTime::parse_from_rfc3339("2024-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let end = chrono::DateTime::parse_from_rfc3339("2024-05-10T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert!(validate_date_range(Some(start), Some(end)).is_ok());
        assert!(validate_date_range(Some(start), None).is_ok());
        assert!(validate_date_range(None, None).is_ok());
    }

    #[test]
    fn test_validate_date_range_rejects_end_before_start() {
        let start = chrono::DateTime::parse_from_rfc3339("2024-05-10T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let end = chrono::DateTime::parse_from_rfc3339("2024-01-10T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert!(validate_date_range(Some(start), Some(end)).is_err());
    }

    #[test]
    fn test_course_settings_defaults_and_partial_patch() {
        let mut settings = CourseSettings::default();
        assert!(settings.allow_student_discussion_topics);
        assert!(settings.allow_student_forum_attachments);
        assert!(settings.allow_student_organized_groups);
        assert!(!settings.hide_final_grades);
        assert_eq!(settings.default_due_time, "23:59:59");
        assert_eq!(settings.default_discussion_settings, serde_json::json!({}));

        settings.apply(UpdateCourseSettings {
            hide_final_grades: Some(true),
            allow_student_discussion_topics: Some(false),
            default_due_time: Some("9:05:00".to_string()),
            default_discussion_settings: Some(serde_json::json!({
                "sort_order": "desc",
                "bogus": true,
            })),
            ..Default::default()
        });

        assert!(settings.hide_final_grades);
        assert!(!settings.allow_student_discussion_topics);
        assert!(
            !settings.hide_distribution_graphs,
            "fields absent from the patch keep their value"
        );
        assert_eq!(settings.default_due_time, "09:05:00");
        assert_eq!(
            settings.default_discussion_settings,
            serde_json::json!({ "sort_order": "desc" }),
            "unknown discussion settings keys are dropped"
        );
    }

    #[test]
    fn test_normalize_due_time_canvas_rules() {
        assert_eq!(normalize_due_time("23:59:59"), Some("23:59:59".to_string()));
        assert_eq!(normalize_due_time("9:05"), Some("09:05:00".to_string()));
        assert_eq!(normalize_due_time("inherit"), None);
        assert_eq!(normalize_due_time("  "), None);
        assert_eq!(normalize_due_time("25:00:00"), None);
        assert_eq!(normalize_due_time("11:60:00"), None);
        assert_eq!(normalize_due_time("lunchtime"), None);
    }

    #[test]
    fn test_build_course_tabs_pins_home_settings_and_applies_config() {
        let configs = vec![
            CourseTabConfig {
                tab_id: "modules".into(),
                position: 1,
                hidden: true,
            },
            CourseTabConfig {
                tab_id: "home".into(),
                position: 99,
                hidden: true,
            },
            CourseTabConfig {
                tab_id: "settings".into(),
                position: 1,
                hidden: true,
            },
        ];

        let student_tabs = build_course_tabs(42, false, &configs);
        assert_eq!(student_tabs.first().unwrap().id, "home");
        assert_eq!(student_tabs.first().unwrap().position, 1);
        assert!(
            !student_tabs.first().unwrap().hidden,
            "home cannot be hidden"
        );
        assert!(
            student_tabs.iter().all(|tab| tab.id != "settings"),
            "students do not get the settings tab"
        );

        let teacher_tabs = build_course_tabs(42, true, &configs);
        assert_eq!(teacher_tabs.first().unwrap().id, "home");
        assert_eq!(teacher_tabs[1].id, "modules", "configured first");
        assert_eq!(teacher_tabs.last().unwrap().id, "settings");
        assert_eq!(teacher_tabs.last().unwrap().visibility, "admins");
        assert_eq!(
            teacher_tabs.last().unwrap().html_url,
            "/courses/42/settings"
        );

        let modules = &teacher_tabs[1];
        assert!(modules.hidden);
        assert_eq!(modules.visibility, "admins");
        assert_eq!(modules.html_url, "/courses/42/modules");

        let positions: Vec<i64> = teacher_tabs.iter().map(|tab| tab.position).collect();
        assert_eq!(
            positions,
            (1..=teacher_tabs.len() as i64).collect::<Vec<_>>()
        );
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
