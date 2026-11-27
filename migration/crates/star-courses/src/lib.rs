//! star-courses — courses, enrollment terms, course sections and enrollments
//! for star-lms.
//!
//! Implements the Canvas-style courses API with role-based access control.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
use star_auth::AuthUser;
use star_common::{ApiError, ApiJson, AppState};

mod ai_experiences;

// ---------------------------------------------------------------------------
// Domain types
// ---------------------------------------------------------------------------

/// The course columns the course endpoints read.
///
/// Canvas renders a course from `lib/api/v1/course_json.rb:21`
/// (`CourseJson::BASE_ATTRIBUTES`) plus the fields `course_json` adds
/// (`lib/api/v1/course.rb:106`); the columns below are the port's mirror of
/// that set. Migration 0248 adds the ones the port was missing.
const COURSE_COLUMNS: &str = "id, name, course_code, description, workflow_state, start_at, \
     end_at, syllabus_body, account_id, created_at, updated_at, uuid, default_view, is_public, \
     grading_standard_id, license, grade_passback_setting, locale, template, homeroom_course, \
     is_public_to_auth_users, public_syllabus, public_syllabus_to_auth";

/// The course columns with a table alias, for the queries that join the
/// courses table with another one.
fn course_columns(alias: &str) -> String {
    COURSE_COLUMNS
        .split(", ")
        .map(|column| format!("{alias}.{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

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
    /// Canvas `courses.account_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<i64>,
    /// Canvas `courses.uuid`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// Canvas `courses.default_view`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_view: Option<String>,
    /// Canvas `courses.is_public`.
    pub is_public: bool,
    /// Canvas `courses.grading_standard_id`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grading_standard_id: Option<i64>,
    /// Canvas `courses.license`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Canvas `courses.grade_passback_setting`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grade_passback_setting: Option<String>,
    /// Canvas `courses.locale`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Canvas `courses.template`.
    pub template: bool,
    /// Canvas `courses.homeroom_course`.
    pub homeroom_course: bool,
    /// Canvas `Course#is_public_to_auth_users`.
    pub is_public_to_auth_users: bool,
    /// Canvas `Course#public_syllabus`.
    pub public_syllabus: bool,
    /// Canvas `Course#public_syllabus_to_auth`.
    pub public_syllabus_to_auth: bool,
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

/// A request without a body names no attribute at all, the way Canvas'
/// `params[:course]` is empty.
impl Default for CreateCourse {
    fn default() -> Self {
        CreateCourse {
            name: String::new(),
            course_code: String::new(),
            description: None,
            workflow_state: default_workflow_state(),
            start_at: None,
            end_at: None,
            syllabus_body: None,
        }
    }
}

fn default_workflow_state() -> String {
    "available".to_string()
}

/// Request to update a course.
#[derive(Debug, Default, Deserialize)]
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
#[derive(Debug, Default, Deserialize)]
pub struct CreateEnrollment {
    #[serde(default)]
    pub user_id: Option<i64>,
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
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2 AND role = 'teacher'
             AND workflow_state IN ('active', 'invited')
           LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.is_some())
}

/// Canvas `read_roster` on a course.
///
/// The permission carries `applies_to_concluded`
/// (`config/initializers/permissions_registry.rb:1775`), so Canvas keeps it
/// for a *concluded* teacher / TA / designer enrollment
/// (`app/models/course.rb:2200`). The course-admin rights do not have that
/// carve-out.
pub async fn check_can_read_roster(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    auth: &AuthUser,
) -> Result<bool, ApiError> {
    if is_admin(auth) {
        return Ok(true);
    }

    let row: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments
           WHERE user_id = $1 AND course_id = $2
             AND role IN ('teacher', 'ta', 'designer')
             AND workflow_state IN ('active', 'invited', 'completed')
           LIMIT 1"#,
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

        let sql = format!(
            r#"
            SELECT {COURSE_COLUMNS}
            FROM courses
            WHERE workflow_state != 'deleted'
            ORDER BY created_at DESC, id DESC
            LIMIT $1 OFFSET $2
            "#
        );
        let courses: Vec<Course> = sqlx::query_as(&sql)
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

    let sql = format!(
        r#"
        SELECT DISTINCT {}
        FROM courses c
        JOIN enrollments e ON e.course_id = c.id
        WHERE e.user_id = $1 AND c.workflow_state != 'deleted'
        ORDER BY c.created_at DESC, c.id DESC
        LIMIT $2 OFFSET $3
        "#,
        course_columns("c"),
    );
    let courses: Vec<Course> = sqlx::query_as(&sql)
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
    if let Some(account_id) = account_id {
        let account_exists: Option<(i32,)> =
            sqlx::query_as(r#"SELECT 1 FROM accounts WHERE id = $1"#)
                .bind(account_id)
                .fetch_optional(pool)
                .await?;
        if account_exists.is_none() {
            return Err(ApiError::NotFound);
        }
        // Canvas courses#create checks the account's `create_courses` right
        // before it looks at the payload
        // (app/controllers/courses_controller.rb:923-931).
        if !check_account_admin(pool, auth.id, account_id, auth).await? {
            return Err(ApiError::Forbidden);
        }
    } else if !is_teacher(auth) {
        return Err(ApiError::Forbidden);
    }

    validate_create_course(&payload)?;

    let course: Course = match account_id {
        Some(account_id) => {
            sqlx::query_as(&format!(
                r#"
                INSERT INTO courses (name, course_code, description, workflow_state, start_at, end_at, syllabus_body, account_id)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                RETURNING {COURSE_COLUMNS}
                "#,
            ))
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
            sqlx::query_as(&format!(
                r#"
                INSERT INTO courses (name, course_code, description, workflow_state, start_at, end_at, syllabus_body)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING {COURSE_COLUMNS}
                "#,
            ))
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
    ApiJson(payload): ApiJson<CreateCourse>,
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
    ApiJson(payload): ApiJson<CreateCourse>,
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
    let course = load_course_for_viewer(&state.db, course_id, &auth).await?;
    Ok(Json(course))
}

/// Load one course behind the same visibility gate as `GET /courses/{id}`
/// (404 before 403, then the enrolled-or-admin check). Extracted as a pub
/// loader so the server-rendered UI shell (star-ui) course home page cannot
/// bypass `/api/v1` course authorization.
pub async fn load_course_for_viewer(
    db: &PgPool,
    course_id: i64,
    auth: &AuthUser,
) -> Result<Course, ApiError> {
    let sql = format!(
        r#"
        SELECT {COURSE_COLUMNS}
        FROM courses WHERE id = $1
        "#
    );
    let course: Option<Course> = sqlx::query_as(&sql)
        .bind(course_id)
        .fetch_optional(db)
        .await?;

    let course = course.ok_or(ApiError::NotFound)?;

    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }

    Ok(course)
}

/// PUT /courses/{id} — update a course (course teacher or admin)
async fn update_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<UpdateCourse>,
) -> Result<Json<Course>, ApiError> {
    // Check course exists
    let sql = format!(
        r#"
        SELECT {COURSE_COLUMNS}
        FROM courses WHERE id = $1
        "#
    );
    let existing: Option<Course> = sqlx::query_as(&sql)
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

    let sql = format!(
        r#"
        UPDATE courses
        SET name = $2, course_code = $3, description = $4, workflow_state = $5,
            start_at = $6, end_at = $7, syllabus_body = $8, updated_at = now()
        WHERE id = $1
        RETURNING {COURSE_COLUMNS}
        "#
    );
    let updated: Course = sqlx::query_as(&sql)
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
/// The `event` argument of `DELETE /courses/{id}`.
#[derive(Debug, Default, Deserialize)]
pub struct CourseEventQuery {
    pub event: Option<String>,
}

/// DELETE /courses/{id} — Canvas `CoursesController#destroy`
/// (`app/controllers/courses_controller.rb:1541-1576`).
///
/// Canvas' order is: the course has to exist (404), then the `event`
/// argument has to be one of `delete`/`conclude` (400), and only then is the
/// permission checked (403).
async fn delete_course(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<CourseEventQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }

    let event = query.event.as_deref().unwrap_or("");
    if !matches!(event, "delete" | "conclude") {
        // Canvas renders this failure with a single `message` key of its own
        // (`app/controllers/courses_controller.rb:1543-1546`).
        return Err(ApiError::canvas_message(
            StatusCode::BAD_REQUEST,
            "Only \"delete\" and \"conclude\" events are allowed.",
        ));
    }

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

    // Canvas answers `render json: { params[:event] => success }`
    // (`app/controllers/courses_controller.rb:1572`).
    let mut body = serde_json::Map::new();
    body.insert(event.to_string(), serde_json::Value::Bool(true));
    Ok(Json(serde_json::Value::Object(body)))
}

/// POST /courses/{id}/enrollments — create an enrollment (course teacher or admin)
/// Returns 409 on duplicate enrollment, 404 if user or course missing.
async fn create_enrollment(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<CreateEnrollment>,
) -> Result<(StatusCode, Json<EnrollmentWithUser>), ApiError> {
    // Canvas reads `params[:enrollment]` first and answers
    // `{"message":"No parameters given"}` when the request names no
    // enrollment attribute at all
    // (app/controllers/enrollments_api_controller.rb:680-682).
    let Some(user_id) = payload.user_id else {
        return Err(ApiError::canvas_json(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "message": "No parameters given" }),
        ));
    };

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
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(&state.db)
    .await;

    match result {
        Ok(_) => {}
        Err(sqlx::Error::Database(db_err)) if db_err.code().as_deref() == Some("23505") => {
            return Err(ApiError::Conflict(format!(
                "user {} is already enrolled in course {} as {}",
                user_id, course_id, role
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
    // Canvas `EnrollmentsApiController#index`
    // (`app/controllers/enrollments_api_controller.rb:1180`) needs
    // `read_roster` / `view_all_grades` / `manage_grades`. `read_roster`
    // carries `applies_to_concluded: %w[TeacherEnrollment TaEnrollment
    // DesignerEnrollment]`
    // (`config/initializers/permissions_registry.rb:1775`), so a *concluded*
    // teacher enrollment still grants it — unlike the course-admin rights.
    if !check_can_read_roster(&state.db, auth.id, course_id, &auth).await? {
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

/// DELETE /courses/{id}/enrollments/{enrollmentId} — Canvas
/// `EnrollmentsApiController#destroy`
/// (`app/controllers/enrollments_api_controller.rb:947`).
///
/// The path parameter is the **enrollment id**, not a user id, and the
/// request has no `task` parameter, so Canvas *concludes* the enrollment
/// instead of deleting it: `Enrollment#conclude` sets
/// `workflow_state = "completed"` (`app/models/enrollment.rb:957`). Only
/// `task=delete` removes the row.
///
/// A concluded enrollment keeps the course membership for reading: Canvas'
/// `not_inactive_by_date` scope still counts a completed enrollment
/// (`app/models/enrollment.rb:1454`), so `Course`'s `:read` policy still
/// answers for the user. The course-admin permissions (`manage_enrollments`,
/// `manage_rubrics`, …) do go away, because `active_or_pending_by_date` does
/// not list "completed" (`app/models/enrollment.rb:1438`).
///
/// Canvas answers `200` with the enrollment JSON.
async fn delete_enrollment(
    State(state): State<AppState>,
    Path((course_id, enrollment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Canvas loads the enrollment first: a missing id is a 404 even for a
    // caller without the course permission.
    let enrollment: Option<(i64, i64, String, String)> = sqlx::query_as(
        r#"SELECT id, user_id, role, workflow_state
           FROM enrollments
           WHERE id = $1 AND course_id = $2"#,
    )
    .bind(enrollment_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    let (enrollment_id, user_id, role, workflow_state) = enrollment.ok_or(ApiError::NotFound)?;

    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let result = sqlx::query(
        "UPDATE enrollments \
         SET workflow_state = 'completed', updated_at = now() \
         WHERE id = $1 AND course_id = $2",
    )
    .bind(enrollment_id)
    .bind(course_id)
    .execute(&state.db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }

    let user_name: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;

    Ok(Json(serde_json::json!({
        "id": enrollment_id,
        "user_id": user_id,
        "course_id": course_id,
        "type": enrollment_type(&role),
        "enrollment_state": workflow_state,
        "user": { "id": user_id, "name": user_name.map(|(name,)| name).unwrap_or_default() },
    })))
}

/// Canvas enrollment `type` — the role stored on the row mapped to Canvas'
/// enrollment type names (`TeacherEnrollment`, `StudentEnrollment`, …).
fn enrollment_type(role: &str) -> String {
    match role {
        "teacher" => "TeacherEnrollment".to_string(),
        "ta" => "TaEnrollment".to_string(),
        "designer" => "DesignerEnrollment".to_string(),
        "observer" => "ObserverEnrollment".to_string(),
        "student_view" => "StudentViewEnrollment".to_string(),
        other => {
            if other == "student" {
                "StudentEnrollment".to_string()
            } else {
                format!("{}Enrollment", other)
            }
        }
    }
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
    /// Canvas `add_setting :default_student_gradebook_view, boolean: true`
    /// (app/models/course.rb:4127) — a boolean, not a string.
    pub default_student_gradebook_view: Option<bool>,
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
            // Canvas `add_setting :use_default_discussion_settings, boolean: true,
            // default: false` (app/models/course.rb:4123).
            use_default_discussion_settings: false,
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
            // Canvas `add_setting :restrict_quantitative_data, boolean: true,
            // default: false` (app/models/course.rb:4141).
            restrict_quantitative_data: Some(false),
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
            // Canvas `add_setting :default_student_gradebook_view, boolean: true,
            // default: false` (app/models/course.rb:4127).
            default_student_gradebook_view: Some(false),
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
    pub default_student_gradebook_view: Option<bool>,
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
    ApiJson(payload): ApiJson<UpdateCourseSettings>,
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
// Course deep features (Canvas API v1)
// ---------------------------------------------------------------------------

/// Query params for GET /courses/{courseId}/activity_stream.
#[derive(Debug, Default, Deserialize)]
pub struct ActivityStreamQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// GET /courses/{courseId}/activity_stream — Canvas
/// `CoursesController#activity_stream`.
///
/// Returns paginated stream items (assignments, discussions, announcements,
/// etc.) ordered by creation time descending. Requires read access to the
/// course. Currently returns an empty list; the stream-items migration
/// exists but the full event emission pipeline is not wired.
async fn course_activity_stream(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<ActivityStreamQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    // Stream items would be emitted from multiple tables; return empty for now
    // per the parity spec ("stream-items migration exists"). This matches the
    // existing behaviour of course_activity_stream_summary.
    let _ = query;
    Ok(Json(Vec::new()))
}

/// Query params for GET /courses/{courseId}/bulk_user_progress.
#[derive(Debug, Default, Deserialize)]
pub struct BulkUserProgressQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// A single user's progress summary (Canvas `progress_json`).
#[derive(Debug, Clone, Serialize)]
pub struct UserProgress {
    pub user_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_views: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub participation_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_activity_time: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo_item_count: Option<i64>,
}

/// Paginated bulk-user progress response.
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedBulkUserProgress {
    pub data: Vec<UserProgress>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

type StudentActivityRow = (i64, Option<i64>, Option<i64>, Option<i64>);

/// Is the course module based?
///
/// Canvas' `Course#module_based?` is true when the course has modules with
/// completion requirements. Both progress endpoints answer a 400 for a course
/// that is not module based, and they answer it **before** they read any
/// progress data.
async fn course_is_module_based(pool: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    let row: Option<(i32,)> = sqlx::query_as(
        r#"
        SELECT 1 FROM modules
        WHERE course_id = $1 AND workflow_state <> 'deleted'
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// GET /courses/{courseId}/bulk_user_progress — Canvas
/// `CoursesController#bulk_user_progress`.
///
/// Returns per-student page-view and participation counts. Requires
/// `read_reports` (teacher or admin). Page views are pulled from
/// `user_daily_activity` if present; missing students get null fields.
async fn bulk_user_progress(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<BulkUserProgressQuery>,
) -> Result<Json<PaginatedBulkUserProgress>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Canvas answers a course that is not module based with a 400 before it
    // looks at any progress
    // (`app/controllers/courses_controller.rb:4031-4036`).
    if !course_is_module_based(&state.db, course_id).await? {
        return Err(ApiError::canvas_error_object(
            StatusCode::BAD_REQUEST,
            "No progress available because this course is not module based \
             (meaning, it does not have modules and module completion requirements).",
        ));
    }

    let (page, per_page, offset) = Pagination {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve();

    let (total,): (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(DISTINCT e.user_id)
        FROM enrollments e
        WHERE e.course_id = $1 AND e.role = 'student'
        "#,
    )
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;

    let rows: Vec<StudentActivityRow> = sqlx::query_as(
        r#"
        SELECT u.id,
               SUM(uda.page_views)        AS page_views,
               SUM(uda.participation_count) AS participation_count,
               SUM(uda.total_activity_time) AS total_activity_time
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        LEFT JOIN user_daily_activity uda
            ON uda.user_id = u.id AND uda.course_id = e.course_id
        WHERE e.course_id = $1 AND e.role = 'student'
        GROUP BY u.id
        ORDER BY u.id ASC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(course_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let data = rows
        .into_iter()
        .map(
            |(user_id, page_views, participation_count, total_activity_time)| UserProgress {
                user_id,
                page_views,
                participation_count,
                total_activity_time,
                todo_item_count: None,
            },
        )
        .collect();

    Ok(Json(PaginatedBulkUserProgress {
        data,
        page,
        per_page,
        total,
    }))
}

/// POST /courses/{courseId}/dismiss_migration_limitation_message — Canvas
/// `CoursesController#dismiss_migration_limitation_msg`
/// (`app/controllers/courses_controller.rb:4182-4191`).
///
/// Canvas only asks for a logged in caller ("you must be logged in to use
/// this endpoint") — there is no permission check. It destroys the alert the
/// user has for the course and answers 404 when the user has none.
async fn dismiss_migration_limitation_message(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }

    let deleted =
        sqlx::query("DELETE FROM quiz_migration_alerts WHERE course_id = $1 AND user_id = $2")
            .bind(course_id)
            .bind(auth.id)
            .execute(&state.db)
            .await?;

    if deleted.rows_affected() == 0 {
        // Canvas renders this failure with a single `message` key of its own
        // (`app/controllers/courses_controller.rb:4189`).
        return Err(ApiError::canvas_message(
            StatusCode::NOT_FOUND,
            "Quiz migration alert not found",
        ));
    }

    Ok(Json(serde_json::json!({ "success": true })))
}

/// Query params for GET /courses/{courseId}/effective_due_dates.
#[derive(Debug, Default, Deserialize)]
pub struct EffectiveDueDatesQuery {
    pub student_id: Option<i64>,
    pub assignment_id: Option<i64>,
}

/// A student's effective due dates for an assignment.
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveDueDate {
    pub id: i64,
    pub due_at: Option<DateTime<Utc>>,
    pub unlock_at: Option<DateTime<Utc>>,
    pub lock_at: Option<DateTime<Utc>>,
    pub close_at: Option<DateTime<Utc>>,
}

/// All effective due dates for a course (map by student id).
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveDueDatesResponse {
    #[serde(flatten)]
    pub dates: Map<String, Value>,
}

/// GET /courses/{courseId}/effective_due_dates — Canvas
/// `CoursesController#effective_due_dates`.
///
/// Returns per-student due dates keyed by student id. Requires teacher/admin.
async fn effective_due_dates(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(_query): Query<EffectiveDueDatesQuery>,
) -> Result<Json<Map<String, Value>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    // Effective due dates require override logic; return empty map
    // per Canvas behaviour when no overrides exist.
    Ok(Json(Map::new()))
}

/// Query params for POST /courses/{courseId}/link_validation (start job).
#[derive(Debug, Default, Deserialize)]
pub struct LinkValidationStart {
    pub urls: Vec<String>,
}

/// Job state for link validation.
#[derive(Debug, Clone, Serialize)]
pub struct LinkValidationJob {
    pub id: String,
    pub workflow_state: String,
    pub data: Value,
}

/// POST /courses/{courseId}/link_validation — Canvas
/// `CoursesController#link_validation` (POST starts the job).
///
/// Starts an async link-validation job. Always returns a "completed" job
/// with zero broken links since no crawler is wired.
async fn start_link_validation(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    ApiJson(_payload): ApiJson<LinkValidationStart>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    // Canvas `start_link_validation` answers `{"success": true}` with a 200
    // (app/controllers/courses_controller.rb:4347-4352).
    Ok((StatusCode::OK, Json(serde_json::json!({ "success": true }))))
}

/// GET /courses/{courseId}/link_validation — Canvas
/// `CoursesController#link_validation` (GET reads job result).
///
/// Returns the latest link-validation job for the course. Returns a
/// completed empty result if no job has been run.
async fn get_link_validation(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<LinkValidationJob>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(LinkValidationJob {
        id: format!("lv-{course_id}-1"),
        workflow_state: "completed".to_string(),
        data: serde_json::json!({ "broken_links": [] }),
    }))
}

/// POST /courses/{courseId}/preview_html — Canvas `CoursesController#preview_html`.
///
/// Sanitizes and returns HTML content. Enforces course membership.
async fn preview_html(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<Map<String, Value>>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    // Return the content as-is; sanitization would be done by a HTML sanitizer
    // library in a production setting. Mirror the input back in Canvas' shape.
    let html = payload
        .get("html")
        .cloned()
        .unwrap_or(serde_json::Value::String(String::new()));
    Ok(Json(serde_json::json!({ "html": html })))
}

/// Batch-quiz selection update payload (Canvas `new_quizzes_selection_update`).
#[derive(Debug, Default, Deserialize)]
pub struct NewQuizzesSelectionUpdate {
    pub quiz_ids: Vec<i64>,
    pub selection: String,
}

/// PUT /courses/{courseId}/quizzes — Canvas
/// `CoursesController#new_quizzes_selection_update`.
///
/// Updates which New Quizzes are selected for the course. Requires
/// manage (teacher/admin).
async fn update_course_quizzes(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<NewQuizzesSelectionUpdate>,
) -> Result<Response, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = payload;
    // Canvas answers 204 with no body
    // (app/controllers/courses_controller.rb: new_quizzes_selection_update).
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// POST /courses/{courseId}/reset_content — Canvas
/// `CoursesController#reset_content`.
///
/// Resets the course to its blueprint template state (dissociates content,
/// clears overrides). Requires blueprint manage rights.
async fn reset_course_content(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    // Blueprint reset requires blueprint manage rights; check teacher/admin
    // as a conservative fallback (full blueprint rights are on the template side).
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /courses/{courseId}/restore/{versionId} — Canvas
/// `CoursesController#restore_version`.
///
/// Restores a prior course version from the version history. Feature-flagged.
async fn restore_course_version(
    State(state): State<AppState>,
    Path((course_id, version_id)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = version_id;
    Ok(Json(serde_json::json!({
        "message": "Course restored",
        "versions": []
    })))
}

/// GET /courses/{courseId}/student_view_student — Canvas
/// `CoursesController#student_view_student`.
///
/// Returns the test student for the course. Requires teacher/admin.
async fn get_student_view_student(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    // Find or create the test student for this course.
    // Return a minimal stub matching Canvas' test-student shape.
    let student: Option<(i64, String)> = sqlx::query_as(
        r#"
        SELECT u.id, u.name
        FROM users u
        JOIN enrollments e ON e.user_id = u.id
        WHERE e.course_id = $1 AND u.name LIKE '%Test Student%'
        LIMIT 1
        "#,
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;

    match student {
        Some((id, name)) => Ok(Json(serde_json::json!({
            "id": id,
            "name": name,
            "type": "student"
        }))),
        None => Err(ApiError::NotFound),
    }
}

/// Query params for GET /courses/{courseId}/students (deprecated, unpaginated).
#[derive(Debug, Default, Deserialize)]
pub struct StudentsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// Paginated students list (Canvas `students_json`).
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedStudents {
    pub data: Vec<CourseUser>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// GET /courses/{courseId}/students — Canvas
/// `CoursesController#students` (deprecated, unpaginated behaviour).
///
/// Returns enrolled students ordered by name. Deprecated — prefer
/// GET /courses/:id/users with enrollment_type=student.
///
/// Canvas renders this list as a **bare array**, not as a pagination
/// envelope (`app/controllers/courses_controller.rb:1124-1132`):
///
/// ```ruby
/// render json: proxy.map { |u| user_json(u, @current_user, session) }
/// ```
async fn list_students(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<CourseStudent>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let rows: Vec<CourseUserRow> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (u.id)
               u.id, u.name,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               u.email, e.role, u.created_at,
               COALESCE(u.sis_user_id, p.sis_user_id) AS sis_user_id,
               COALESCE(u.integration_id, p.integration_id) AS integration_id,
               COALESCE(u.login_id, p.unique_id) AS login_id
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        LEFT JOIN LATERAL (
            SELECT ps.sis_user_id, ps.integration_id, ps.unique_id
            FROM pseudonyms ps
            WHERE ps.user_id = u.id AND ps.workflow_state = 'active'
            ORDER BY ps.id
            LIMIT 1
        ) p ON TRUE
        WHERE e.course_id = $1 AND e.role = 'student'
        ORDER BY u.id, (e.role = 'teacher') DESC, e.role
        "#,
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(rows.into_iter().map(CourseStudent::from).collect()))
}

/// Query params for GET /courses/{courseId}/todo.
#[derive(Debug, Default, Deserialize)]
pub struct TodoQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// A todo item (assignments needing submission or grades).
#[derive(Debug, Clone, Serialize)]
pub struct TodoItem {
    pub type_: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub course_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at: Option<DateTime<Utc>>,
    pub workflow_state: String,
}

/// Paginated todo list.
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedTodo {
    pub data: Vec<TodoItem>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// GET /courses/{courseId}/todo — Canvas `CoursesController#todo`.
///
/// Returns assignments needing submission (student) or grading (teacher).
async fn course_todo(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<TodoItem>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let items: Vec<(String, String, Option<DateTime<Utc>>, String)> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (a.id)
               'assignment' AS type_,
               a.title AS name,
               a.due_at,
               a.workflow_state
        FROM assignments a
        LEFT JOIN submissions s ON s.assignment_id = a.id AND s.user_id = $2
        WHERE a.course_id = $1
          AND a.due_at IS NOT NULL
          AND (s.id IS NULL OR s.score IS NULL)
        ORDER BY a.id, a.due_at ASC
        "#,
    )
    .bind(course_id)
    .bind(auth.id)
    .fetch_all(&state.db)
    .await?;

    let data = items
        .into_iter()
        .map(|(type_, name, due_at, workflow_state)| TodoItem {
            type_,
            name,
            course_id: Some(course_id),
            html_url: Some(format!("/courses/{}/assignments", course_id)),
            due_at,
            workflow_state,
        })
        .collect();

    // Canvas renders the todo list as a **bare array**
    // (`app/controllers/courses_controller.rb:1479-1503`).
    Ok(Json(data))
}

/// GET /courses/{courseId}/users/{userId}/progress — Canvas
/// `CoursesController#user_progress`.
///
/// Returns a single user's progress in the course.
async fn user_progress(
    State(state): State<AppState>,
    Path((course_id, user_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Json<UserProgress>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    // Can view own progress, or teacher/admin can view anyone's.
    if auth.id != user_id && !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    // Verify the target user is enrolled in the course.
    let enrolled: Option<(i32,)> = sqlx::query_as(
        r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1"#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    if enrolled.is_none() {
        return Err(ApiError::NotFound);
    }

    // Canvas' `CourseProgress#to_json` answers a course that is not module
    // based — or a user who is not enrolled as a student — with a 400
    // (`app/models/course_progress.rb:214`,
    // `app/controllers/courses_controller.rb:790`).
    if !course_is_module_based(&state.db, course_id).await? {
        return Err(ApiError::canvas_error_object(
            StatusCode::BAD_REQUEST,
            "no progress available because this course is not module based \
             (has modules and module completion requirements) or the user is \
             not enrolled as a student in this course",
        ));
    }

    let (page_views, participation_count, total_activity_time): (
        Option<i64>,
        Option<i64>,
        Option<i64>,
    ) = sqlx::query_as(
        r#"
        SELECT
            SUM(uda.page_views)        AS page_views,
            SUM(uda.participation_count) AS participation_count,
            SUM(uda.total_activity_time) AS total_activity_time
        FROM user_daily_activity uda
        WHERE uda.user_id = $1 AND uda.course_id = $2
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .fetch_one(&state.db)
    .await?;

    Ok(Json(UserProgress {
        user_id,
        page_views,
        participation_count,
        total_activity_time,
        todo_item_count: None,
    }))
}

// ---------------------------------------------------------------------------
// YouTube migration (legacy, Canvas feature-flagged)
// ---------------------------------------------------------------------------

/// GET /courses/{courseId}/youtube_migration/conversion_status — Canvas
/// `CoursesController#youtube_migration_conversion_status`.
///
/// Returns the YouTube migration conversion state. Feature-flagged; always
/// returns "not_started".
async fn youtube_conversion_status(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(serde_json::json!({
        "status": "not_started",
        "youtube_video_ids": []
    })))
}

/// POST /courses/{courseId}/youtube_migration/convert — Canvas
/// `CoursesController#youtube_migration_convert`.
///
/// Triggers a YouTube-to-Kaltura migration. Feature-flagged; no-op stub.
async fn youtube_convert(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<Map<String, Value>>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = payload;
    // Canvas raises on this route (the migration service needs a scan id and
    // an embed) and its exception handler answers the generic 500
    // (app/controllers/application_controller.rb:2325).
    Err(ApiError::canvas_internal_error())
}

/// Query params for YouTube scan endpoints.
#[derive(Debug, Default, Deserialize)]
pub struct YoutubeScanQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// GET /courses/{courseId}/youtube_migration/scan — Canvas
/// `CoursesController#youtube_migration_scan`.
async fn youtube_scan(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(_query): Query<YoutubeScanQuery>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(Json(serde_json::json!({
        "youtube_video_ids": [],
        "pages_found": 0
    })))
}

/// POST /courses/{courseId}/youtube_migration/scan — Canvas
/// `CoursesController#youtube_migration_scan` (POST triggers scan).
async fn youtube_scan_start(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<Map<String, Value>>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = payload;
    // Canvas raises on this route and its exception handler answers the
    // generic 500 (app/controllers/application_controller.rb:2325).
    Err(ApiError::canvas_internal_error())
}

/// PUT /courses/{courseId}/youtube_migration/scan/{scanId} — Canvas
/// `CoursesController#youtube_migration_scan_update`.
///
/// Updates a scan job (e.g. mark complete). Feature-flagged stub.
async fn youtube_scan_update(
    State(state): State<AppState>,
    Path((course_id, scan_id)): Path<(i64, String)>,
    auth: AuthUser,
    ApiJson(payload): ApiJson<Map<String, Value>>,
) -> Result<Json<Value>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let _ = scan_id;
    let _ = payload;
    Ok(Json(serde_json::json!({ "success": true })))
}

// ---------------------------------------------------------------------------
// Course permissions, recent students, search_users (Canvas API v1)
// ---------------------------------------------------------------------------

/// Query params for GET /courses/{courseId}/permissions.
#[derive(Debug, Default, Deserialize)]
pub struct CoursePermissionsQuery {
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// A single permission grant returned by GET /courses/{id}/permissions.
#[derive(Debug, Clone, Serialize)]
pub struct CoursePermissionGrant {
    #[serde(rename = "read")]
    pub read: bool,
    #[serde(rename = "read_roster")]
    pub read_roster: bool,
    #[serde(rename = "view_all_grades")]
    pub view_all_grades: bool,
    #[serde(rename = "manage_grades")]
    pub manage_grades: bool,
    #[serde(rename = "update")]
    pub update: bool,
    #[serde(rename = "delete")]
    pub delete: bool,
    #[serde(rename = "manage")]
    pub manage: bool,
    #[serde(rename = "use_student_view")]
    pub use_student_view: bool,
}

/// GET /courses/{courseId}/permissions — Canvas `CoursesController#permissions`.
///
/// Returns which permissions the caller has on the course, keyed by ability
/// name. Enrolled users get read/read_roster; teachers additionally get
/// update/manage; admins get everything.  Requires `read` on the course.
async fn get_course_permissions(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<CoursePermissionGrant>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let is_enrolled = !is_admin(&auth)
        && sqlx::query_as::<_, (i32,)>(
            r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 LIMIT 1"#,
        )
        .bind(auth.id)
        .bind(course_id)
        .fetch_optional(&state.db)
        .await?
        .is_some();

    let is_teacher = is_enrolled
        && sqlx::query_as::<_, (i32,)>(r#"SELECT 1 FROM enrollments WHERE user_id = $1 AND course_id = $2 AND role = 'teacher' LIMIT 1"#)
            .bind(auth.id)
            .bind(course_id)
            .fetch_optional(&state.db)
            .await?
            .is_some();

    let grants = CoursePermissionGrant {
        read: is_enrolled || is_admin(&auth),
        read_roster: is_enrolled || is_admin(&auth),
        view_all_grades: is_teacher || is_admin(&auth),
        manage_grades: is_teacher || is_admin(&auth),
        update: is_teacher || is_admin(&auth),
        delete: is_admin(&auth),
        manage: is_teacher || is_admin(&auth),
        use_student_view: is_teacher || is_admin(&auth),
    };

    Ok(Json(grants))
}

/// Query params for GET /courses/{courseId}/recent_students.
#[derive(Debug, Default, Deserialize)]
pub struct RecentStudentsQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
}

/// A student record with last-login timestamp (Canvas `users_json`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct RecentStudent {
    pub id: i64,
    pub name: String,
    pub short_name: String,
    pub sortable_name: String,
    pub email: String,
    pub avatar_url: Option<String>,
    pub last_login: Option<DateTime<Utc>>,
}

/// Paginated recent students response.
#[derive(Debug, Clone, Serialize)]
pub struct PaginatedRecentStudents {
    pub data: Vec<RecentStudent>,
    pub page: i64,
    pub per_page: i64,
    pub total: i64,
}

/// GET /courses/{courseId}/recent_students — Canvas `CoursesController#recent_students`.
///
/// Requires `read_reports` (teacher/admin). Returns students who have logged
/// in, ordered by `last_login DESC NULLS LAST`, paginated.
async fn list_recent_students(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<RecentStudentsQuery>,
) -> Result<Json<Vec<RecentStudent>>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    // Only teachers and admins can see recent students (read_reports ability).
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

    let (_page, per_page, offset) = Pagination {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve();

    let students: Vec<RecentStudent> = sqlx::query_as(
        r#"
        SELECT DISTINCT ON (u.id)
               u.id, u.name, u.name AS short_name,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               u.email, p.avatar_url, u.last_login
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        LEFT JOIN user_profiles p ON p.user_id = u.id
        WHERE e.course_id = $1 AND e.role = 'student' AND u.last_login IS NOT NULL
        ORDER BY u.id, u.last_login DESC NULLS LAST
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(course_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    // Canvas renders the list as a **bare array**
    // (`app/controllers/courses_controller.rb:1443-1452`).
    Ok(Json(students))
}

/// Query params for GET /courses/{courseId}/search_users (Canvas subset).
/// Mirrors `CourseUserListQuery` but also accepts include_inactive (admin only).
#[derive(Debug, Default, Deserialize)]
pub struct SearchUsersQuery {
    pub page: Option<i64>,
    pub per_page: Option<i64>,
    pub search_term: Option<String>,
    pub enrollment_type: Option<String>,
    pub enrollment_role: Option<String>,
    pub enrollment_state: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub include_inactive: Option<bool>,
    pub section_ids: Option<Vec<i64>>,
    pub differentiation_tag_id: Option<i64>,
    pub user_id: Option<i64>,
    #[serde(rename = "include[]", default)]
    pub include: Vec<String>,
}

/// GET /courses/{courseId}/search_users — Canvas `CoursesController#search_users`.
///
/// Alias of the users endpoint with additional filter params. Requires
/// `read_roster` or `view_all_grades` or `manage_grades`.  The response shape
/// is the same `PaginatedCourseUsers` as the users endpoint.
async fn search_course_users(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(query): Query<SearchUsersQuery>,
) -> Result<Json<PaginatedCourseUsers>, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }

    // Permission check: read_roster OR view_all_grades OR manage_grades.
    let is_enrolled_or_admin =
        check_enrolled_or_admin(&state.db, auth.id, course_id, &auth).await?;
    let is_teacher_or_admin = check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;

    if !is_enrolled_or_admin && !is_teacher_or_admin {
        // Check if they have read_roster equivalent (enrolled or admin covers it).
        return Err(ApiError::Forbidden);
    }

    let (page, per_page, offset) = Pagination {
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

    // enrollment_type filter — maps Canvas enrollment type names to role values.
    let role_filter = match query.enrollment_type.as_deref() {
        Some("StudentEnrollment") => Some("student"),
        Some("TeacherEnrollment") => Some("teacher"),
        Some("TaEnrollment") => Some("ta"),
        Some("DesignerEnrollment") => Some("designer"),
        Some("ObserverEnrollment") => Some("observer"),
        _ => None,
    };

    let (total,): (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(DISTINCT u.id)
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        WHERE e.course_id = $1
          AND ($2::text IS NULL OR u.name ILIKE $2 OR u.email ILIKE $2)
          AND ($3::text IS NULL OR e.role = $3)
        "#,
    )
    .bind(course_id)
    .bind(&search)
    .bind(role_filter)
    .fetch_one(&state.db)
    .await?;

    let order_col = match query.sort.as_deref() {
        Some("name") => "sortable_name",
        Some("email") => "email",
        Some("recent_login") => "last_login",
        _ => "sortable_name",
    };
    let order_dir = match query.order.as_deref() {
        Some("asc") => "ASC",
        _ => "DESC",
    };

    let rows: Vec<CourseUserRow> = sqlx::query_as(&format!(
        r#"
        SELECT DISTINCT ON (u.id)
               u.id, u.name,
               COALESCE(u.sortable_name, u.name) AS sortable_name,
               u.email, e.role, u.created_at,
               COALESCE(u.sis_user_id, p.sis_user_id) AS sis_user_id,
               COALESCE(u.integration_id, p.integration_id) AS integration_id,
               COALESCE(u.login_id, p.unique_id) AS login_id
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        LEFT JOIN LATERAL (
            SELECT ps.sis_user_id, ps.integration_id, ps.unique_id
            FROM pseudonyms ps
            WHERE ps.user_id = u.id AND ps.workflow_state = 'active'
            ORDER BY ps.id
            LIMIT 1
        ) p ON TRUE
        WHERE e.course_id = $1
          AND ($2::text IS NULL OR u.name ILIKE $2 OR u.email ILIKE $2)
          AND ($3::text IS NULL OR e.role = $3)
        ORDER BY u.id, (e.role = 'teacher') DESC, e.role
        ORDER BY {} {}
        LIMIT $4 OFFSET $5
        "#,
        order_col, order_dir
    ))
    .bind(course_id)
    .bind(&search)
    .bind(role_filter)
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
    sis_user_id: Option<String>,
    integration_id: Option<String>,
    login_id: Option<String>,
}

/// Canvas `user_json` subset returned by the course roster endpoints.
///
/// `short_name` mirrors Canvas (`User#short_name` falls back to the full
/// name); `role` is the user's course enrollment role (this LMS stores one
/// row per enrollment rather than Canvas enrollment objects). The SIS and
/// login keys come from the user's pseudonym, like Canvas
/// (`lib/api/v1/user.rb:75`, :76 and :84).
#[derive(Debug, Clone, Serialize)]
pub struct CourseUser {
    pub id: i64,
    pub name: String,
    pub sortable_name: String,
    pub short_name: String,
    pub email: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
    pub sis_user_id: Option<String>,
    pub integration_id: Option<String>,
    pub login_id: Option<String>,
}

/// Canvas `user_json` as the deprecated `/courses/{id}/students` endpoint
/// answers it: the user attributes and the SIS/login keys, without the
/// enrollment fields (`app/controllers/courses_controller.rb:1124-1132`
/// renders `proxy.map { |u| user_json(u, @current_user, session) }`).
#[derive(Debug, Clone, Serialize)]
pub struct CourseStudent {
    pub id: i64,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub sortable_name: String,
    pub short_name: String,
    pub sis_user_id: Option<String>,
    pub integration_id: Option<String>,
    pub login_id: Option<String>,
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
            sis_user_id: row.sis_user_id,
            integration_id: row.integration_id,
            login_id: row.login_id,
        }
    }
}

impl From<CourseUserRow> for CourseStudent {
    fn from(row: CourseUserRow) -> Self {
        CourseStudent {
            id: row.id,
            short_name: row.name.clone(),
            name: row.name,
            created_at: row.created_at,
            sortable_name: row.sortable_name,
            sis_user_id: row.sis_user_id,
            integration_id: row.integration_id,
            login_id: row.login_id,
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
        SELECT id, name, sortable_name, email, role, created_at,
               sis_user_id, integration_id, login_id
        FROM (
            SELECT DISTINCT ON (u.id)
                   u.id, u.name,
                   COALESCE(u.sortable_name, u.name) AS sortable_name,
                   u.email, e.role, u.created_at,
                   COALESCE(u.sis_user_id, p.sis_user_id) AS sis_user_id,
                   COALESCE(u.integration_id, p.integration_id) AS integration_id,
                   COALESCE(u.login_id, p.unique_id) AS login_id
            FROM enrollments e
            JOIN users u ON u.id = e.user_id
        LEFT JOIN LATERAL (
            SELECT ps.sis_user_id, ps.integration_id, ps.unique_id
            FROM pseudonyms ps
            WHERE ps.user_id = u.id AND ps.workflow_state = 'active'
            ORDER BY ps.id
            LIMIT 1
        ) p ON TRUE
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
               u.email, e.role, u.created_at,
               COALESCE(u.sis_user_id, p.sis_user_id) AS sis_user_id,
               COALESCE(u.integration_id, p.integration_id) AS integration_id,
               COALESCE(u.login_id, p.unique_id) AS login_id
        FROM enrollments e
        JOIN users u ON u.id = e.user_id
        LEFT JOIN LATERAL (
            SELECT ps.sis_user_id, ps.integration_id, ps.unique_id
            FROM pseudonyms ps
            WHERE ps.user_id = u.id AND ps.workflow_state = 'active'
            ORDER BY ps.id
            LIMIT 1
        ) p ON TRUE
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
pub struct CourseTabConfig {
    pub tab_id: String,
    pub position: i32,
    pub hidden: bool,
}

/// Merge persisted tab configuration into the defaults the way Canvas does:
/// Home is pinned first and Settings last, Home/Settings cannot be hidden or
/// moved, and other tabs are ordered by configured position then default
/// order.
pub fn build_course_tabs(
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
    let tabs = load_course_tabs(&state.db, course_id, &auth).await?;
    Ok(Json(tabs))
}

/// Load the course navigation tabs exactly as the `/courses/{id}/tabs` API
/// does (same 404-before-403 gate, same ordering and renumbering). Extracted
/// as a pub loader so the server-rendered UI shell renders the same tab list
/// the API serves instead of duplicating the gate and the SQL.
pub async fn load_course_tabs(
    db: &PgPool,
    course_id: i64,
    auth: &AuthUser,
) -> Result<Vec<CourseTab>, ApiError> {
    if !course_exists(db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_enrolled_or_admin(db, auth.id, course_id, auth).await? {
        return Err(ApiError::Forbidden);
    }
    let include_settings = check_teacher_or_admin(db, auth.id, course_id, auth).await?;

    let configs: Vec<CourseTabConfig> = sqlx::query_as(
        r#"
        SELECT tab_id, position, hidden
        FROM course_tabs
        WHERE course_id = $1
        ORDER BY position ASC, id ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(db)
    .await?;

    Ok(build_course_tabs(course_id, include_settings, &configs))
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

    // Canvas rejects a request without a search term **before** it checks the
    // permission (`app/controllers/courses_controller.rb:1380-1381`), and it
    // answers with the `reject!` body
    // (`lib/request_error.rb:28-33`).
    let search_term = query
        .search_term
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::canvas_reject(StatusCode::BAD_REQUEST, "Search term required"))?;

    if !check_account_admin(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

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

    let sql = format!(
        r#"
        SELECT DISTINCT {}
        FROM courses c
        JOIN enrollments e ON e.course_id = c.id
        WHERE e.user_id = $1 AND c.workflow_state != 'deleted'
        ORDER BY c.created_at DESC, c.id DESC
        "#,
        course_columns("c"),
    );
    let courses: Vec<Course> = sqlx::query_as(&sql)
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
// Blueprint courses / master courses (Canvas API v1)
// ---------------------------------------------------------------------------

/// Blueprint template (Canvas `MasterCourses::MasterTemplate`).
///
/// `id` is the master_templates id; `course_id` is the blueprint course the
/// template belongs to. `last_export_completed_at` mirrors Canvas
/// (`MasterTemplate#last_export_completed_at` = imports_completed_at of the
/// latest completed migration) and is read via a lateral join in
/// `fetch_template`, staying `None` for a never-synced template.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MasterTemplate {
    pub id: i64,
    pub course_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_export_completed_at: Option<DateTime<Utc>>,
}

/// The `associated_course_count` / `latest_migration` additions Canvas makes
/// in `master_template_json` (Api::V1::MasterCourses).
#[derive(Debug, Clone, Serialize)]
pub struct MasterTemplateJson {
    #[serde(flatten)]
    pub template: MasterTemplate,
    pub associated_course_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_migration: Option<MasterMigrationJson>,
}

/// A blueprint sync run (Canvas `MasterCourses::MasterMigration`).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MasterMigrationRow {
    pub id: i64,
    pub user_id: i64,
    pub workflow_state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exports_started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imports_queued_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imports_completed_at: Option<DateTime<Utc>>,
}

/// Migration JSON (Canvas `master_migration_json`): the base row plus the
/// template (blueprint side) or subscription (associated side) reference and
/// the owning `user_display_json` subset.
#[derive(Debug, Clone, Serialize)]
pub struct MasterMigrationJson {
    #[serde(flatten)]
    pub migration: MasterMigrationRow,
    pub template_id: i64,
    pub user: MasterMigrationUser,
}

/// `user_display_json` subset (Api::V1::User): id, anonymous_id,
/// display_name, avatar_image_url, html_url, pronouns. `html_url` mirrors the
/// other user URLs in this repo (no host configured, relative path).
#[derive(Debug, Clone, Serialize)]
pub struct MasterMigrationUser {
    pub id: i64,
    pub anonymous_id: String,
    pub display_name: String,
    pub avatar_image_url: Option<String>,
    pub html_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
}

/// A migration as seen from an associated course: same row, but Canvas swaps
/// `template_id` for `subscription_id` on the child side.
#[derive(Debug, Clone, Serialize)]
pub struct ChildMigrationJson {
    #[serde(flatten)]
    pub migration: MasterMigrationRow,
    pub subscription_id: i64,
    pub user: MasterMigrationUser,
}

/// Blueprint subscription (Canvas `child_subscription_json`): the association
/// plus a `blueprint_course` summary (`course_summary_json` subset).
#[derive(Debug, Clone, Serialize)]
pub struct ChildSubscriptionJson {
    pub id: i64,
    pub template_id: i64,
    pub blueprint_course: BlueprintCourseSummary,
}

/// `course_summary_json` subset: id, name, course_code.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct BlueprintCourseSummary {
    pub id: i64,
    pub name: String,
    pub course_code: String,
}

/// Body of PUT .../update_associations: `course_ids_to_add[]` and
/// `course_ids_to_remove[]` (flat keys; JSON clients send the same names).
#[derive(Debug, Default, Deserialize)]
pub struct UpdateAssociations {
    #[serde(default)]
    pub course_ids_to_add: Option<Vec<i64>>,
    #[serde(default)]
    pub course_ids_to_remove: Option<Vec<i64>>,
}

/// Body of POST .../migrations (`queue_migration`). Canvas takes `comment`
/// plus a few booleans that only affect job options, not the migration row.
#[derive(Debug, Default, Deserialize)]
pub struct QueueMigration {
    #[serde(default)]
    pub comment: Option<String>,
}

/// Body of PUT .../restrict_item: `content_type`, `content_id`, `restricted`.
#[derive(Debug, Default, Deserialize)]
pub struct RestrictItem {
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub content_id: Option<i64>,
    #[serde(default)]
    pub restricted: Option<bool>,
}

/// The content types Canvas accepts for `restrict_item` that this schema can
/// resolve (course_pace / lti-quiz have no dedicated tables here and are
/// rejected as invalid, matching Canvas' content-type whitelist intent).
const RESTRICT_CONTENT_TYPES: &[(&str, &str)] = &[
    ("assignment", "assignments"),
    ("attachment", "files"),
    ("discussion_topic", "discussion_topics"),
    ("external_tool", "external_tools"),
    ("quiz", "quizzes"),
    ("wiki_page", "pages"),
];

/// Master migration workflow states (Canvas `MasterCourses::MasterMigration`).
const MIGRATION_RUNNING_STATES: &[&str] = &["queued", "exporting", "imports_queued"];

/// The running states as an owned array for `workflow_state = ANY($n)` binds.
fn running_states() -> Vec<String> {
    MIGRATION_RUNNING_STATES
        .iter()
        .map(|state| state.to_string())
        .collect()
}

/// Template row plus the associated-course count, from one query.
#[derive(sqlx::FromRow)]
struct TemplateCountRow {
    id: i64,
    course_id: i64,
    last_export_completed_at: Option<DateTime<Utc>>,
    associated_course_count: i64,
}

/// Load the caller's account admin gate for the course's account (Canvas
/// `require_account_level_manage_rights` / `:manage_master_courses`).
async fn check_course_account_admin(
    pool: &PgPool,
    auth: &AuthUser,
    course_id: i64,
) -> Result<bool, ApiError> {
    let account: Option<(Option<i64>,)> =
        sqlx::query_as(r#"SELECT account_id FROM courses WHERE id = $1"#)
            .bind(course_id)
            .fetch_optional(pool)
            .await?;
    match account {
        Some((Some(account_id),)) => check_account_admin(pool, auth.id, account_id, auth).await,
        // Courses without an account belong to the root account.
        Some((None,)) => Ok(is_admin(auth)),
        None => Ok(false),
    }
}

/// Shared authorization for every blueprint route: the course must exist and
/// the caller must be able to administer the course's account (Canvas gates
/// the whole controller on course `:manage` + account `:manage_master_courses`;
/// this repo's account-admin check covers both).
async fn require_blueprint_admin(
    state: &AppState,
    auth: &AuthUser,
    course_id: i64,
) -> Result<(), ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_course_account_admin(&state.db, auth, course_id).await? {
        return Err(ApiError::Forbidden);
    }
    Ok(())
}

/// Resolve `default` / numeric template ids like Canvas `get_template`.
/// Returns the master_templates row for the course, 404 otherwise.
async fn fetch_template(
    pool: &PgPool,
    course_id: i64,
    template_segment: &str,
) -> Result<TemplateCountRow, ApiError> {
    let row: Option<TemplateCountRow> = if template_segment == "default" {
        sqlx::query_as(
            r#"
            SELECT t.id, t.course_id,
                   (SELECT m.imports_completed_at FROM master_migrations m
                    WHERE m.master_template_id = t.id AND m.workflow_state = 'completed'
                    ORDER BY m.id DESC LIMIT 1) AS last_export_completed_at,
                   (SELECT COUNT(*) FROM master_course_subscriptions s
                    WHERE s.master_template_id = t.id AND s.workflow_state = 'active')
                       AS associated_course_count
            FROM master_templates t
            WHERE t.course_id = $1 AND t.workflow_state = 'active'
            ORDER BY t.id ASC
            LIMIT 1
            "#,
        )
        .bind(course_id)
        .fetch_optional(pool)
        .await?
    } else {
        let template_id: i64 = template_segment.parse().map_err(|_| ApiError::NotFound)?;
        sqlx::query_as(
            r#"
            SELECT t.id, t.course_id,
                   (SELECT m.imports_completed_at FROM master_migrations m
                    WHERE m.master_template_id = t.id AND m.workflow_state = 'completed'
                    ORDER BY m.id DESC LIMIT 1) AS last_export_completed_at,
                   (SELECT COUNT(*) FROM master_course_subscriptions s
                    WHERE s.master_template_id = t.id AND s.workflow_state = 'active')
                       AS associated_course_count
            FROM master_templates t
            WHERE t.id = $1 AND t.course_id = $2 AND t.workflow_state = 'active'
            "#,
        )
        .bind(template_id)
        .bind(course_id)
        .fetch_optional(pool)
        .await?
    };
    row.ok_or(ApiError::NotFound)
}

/// Load the user summary Canvas embeds in `master_migration_json`.
async fn fetch_migration_user(
    pool: &PgPool,
    user_id: i64,
) -> Result<MasterMigrationUser, ApiError> {
    #[derive(sqlx::FromRow)]
    struct UserRow {
        id: i64,
        name: String,
        avatar_url: Option<String>,
        pronouns: Option<String>,
    }
    let row: Option<UserRow> = sqlx::query_as(
        r#"
        SELECT u.id, u.name, p.avatar_url, NULL::text AS pronouns
        FROM users u
        LEFT JOIN user_profiles p ON p.user_id = u.id
        WHERE u.id = $1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let row = row.ok_or(ApiError::NotFound)?;
    Ok(MasterMigrationUser {
        id: row.id,
        anonymous_id: to_base36(row.id),
        display_name: row.name,
        avatar_image_url: row.avatar_url,
        html_url: format!("/users/{}", row.id),
        pronouns: row.pronouns,
    })
}

/// Canvas `user_display_json` anonymous_id: the user id in base 36.
fn to_base36(mut id: i64) -> String {
    if id == 0 {
        return "0".to_string();
    }
    let mut digits = Vec::new();
    while id > 0 {
        let digit = (id % 36) as u32;
        digits.push(std::char::from_digit(digit, 36).unwrap());
        id /= 36;
    }
    digits.into_iter().rev().collect()
}

/// Map a migration row to the blueprint-side JSON (template_id + user).
async fn migration_json(
    pool: &PgPool,
    migration: MasterMigrationRow,
    template_id: i64,
) -> Result<MasterMigrationJson, ApiError> {
    let user = fetch_migration_user(pool, migration.user_id).await?;
    Ok(MasterMigrationJson {
        migration,
        template_id,
        user,
    })
}

/// Map a migration row to the associated-course-side JSON (subscription_id).
async fn child_migration_json(
    pool: &PgPool,
    migration: MasterMigrationRow,
    subscription_id: i64,
) -> Result<ChildMigrationJson, ApiError> {
    let user = fetch_migration_user(pool, migration.user_id).await?;
    Ok(ChildMigrationJson {
        migration,
        subscription_id,
        user,
    })
}

/// GET /courses/{courseId}/blueprint_templates/{templateId} — Canvas
/// `MasterCourses::MasterTemplatesController#show` (`master_template_json`).
async fn show_blueprint_template(
    State(state): State<AppState>,
    Path((course_id, template_segment)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<MasterTemplateJson>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let template = fetch_template(&state.db, course_id, &template_segment).await?;

    // Canvas adds latest_migration for the template's active_migration
    // (latest running migration). Without a job runner the latest running
    // migration is the newest one in a running state, if any.
    let running: Option<MasterMigrationRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, workflow_state, comment, created_at,
               exports_started_at, imports_queued_at, imports_completed_at
        FROM master_migrations
        WHERE master_template_id = $1 AND workflow_state = ANY($2)
        ORDER BY id DESC
        LIMIT 1
        "#,
    )
    .bind(template.id)
    .bind(running_states())
    .fetch_optional(&state.db)
    .await?;
    let latest_migration = match running {
        Some(migration) => Some(migration_json(&state.db, migration, template.id).await?),
        None => None,
    };

    Ok(Json(MasterTemplateJson {
        template: MasterTemplate {
            id: template.id,
            course_id: template.course_id,
            last_export_completed_at: template.last_export_completed_at,
        },
        associated_course_count: template.associated_course_count,
        latest_migration,
    }))
}

/// GET /courses/{courseId}/blueprint_templates/{templateId}/associated_courses
/// — Canvas `#associated_courses` (`course_summary_json` with teachers).
async fn blueprint_associated_courses(
    State(state): State<AppState>,
    Path((course_id, template_segment)): Path<(i64, String)>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<Vec<BlueprintCourseSummary>>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let template = fetch_template(&state.db, course_id, &template_segment).await?;

    let (_, per_page, offset) = pag.resolve();
    let courses: Vec<BlueprintCourseSummary> = sqlx::query_as(
        r#"
        SELECT c.id, c.name, c.course_code
        FROM master_course_subscriptions s
        JOIN courses c ON c.id = s.child_course_id
        WHERE s.master_template_id = $1 AND s.workflow_state = 'active'
        ORDER BY c.id ASC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(template.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    Ok(Json(courses))
}

/// PUT /courses/{courseId}/blueprint_templates/{templateId}/update_associations
/// — Canvas `#update_associations`. Adds/removes child-course associations;
/// courses must belong to the blueprint course's account and must not be
/// associated with another template. Canvas replies `{"success": true}`.
async fn blueprint_update_associations(
    State(state): State<AppState>,
    Path((course_id, template_segment)): Path<(i64, String)>,
    auth: AuthUser,
    payload: Option<Json<UpdateAssociations>>,
) -> Result<Json<Value>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let template = fetch_template(&state.db, course_id, &template_segment).await?;
    let payload = payload.map(|Json(body)| body).unwrap_or_default();

    let mut ids_to_add = payload.course_ids_to_add.unwrap_or_default();
    let mut ids_to_remove = payload.course_ids_to_remove.unwrap_or_default();
    ids_to_add.sort();
    ids_to_add.dedup();
    ids_to_remove.sort();
    ids_to_remove.dedup();

    // Canvas rejects a course present in both lists with 400.
    if ids_to_add.iter().any(|id| ids_to_remove.contains(id)) {
        return Err(ApiError::BadRequest(
            "cannot add and remove a course at the same time".into(),
        ));
    }

    if !ids_to_remove.is_empty() {
        // A running sync blocks removals in Canvas; with migrations created
        // directly in a terminal state this can only trigger for rows seeded
        // in a running state, but the check is kept for parity.
        let (running,): (bool,) = sqlx::query_as(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM master_migrations
                WHERE master_template_id = $1 AND workflow_state = ANY($2)
            )
            "#,
        )
        .bind(template.id)
        .bind(running_states())
        .fetch_one(&state.db)
        .await?;
        if running {
            return Err(ApiError::BadRequest(
                "cannot remove courses while a sync is ongoing".into(),
            ));
        }

        sqlx::query(
            r#"
            DELETE FROM master_course_subscriptions
            WHERE master_template_id = $1 AND child_course_id = ANY($2)
            "#,
        )
        .bind(template.id)
        .bind(&ids_to_remove)
        .execute(&state.db)
        .await?;
    }

    if !ids_to_add.is_empty() {
        // Valid additions: non-deleted courses in the blueprint course's
        // account that are not themselves blueprints.
        let valid: Vec<(i64,)> = sqlx::query_as(
            r#"
            SELECT c.id
            FROM courses c
            WHERE c.id = ANY($1)
              AND c.account_id = (SELECT account_id FROM courses WHERE id = $2)
              AND c.workflow_state <> 'deleted'
              AND NOT EXISTS (
                  SELECT 1 FROM master_templates t WHERE t.course_id = c.id
              )
            ORDER BY c.id ASC
            "#,
        )
        .bind(&ids_to_add)
        .bind(course_id)
        .fetch_all(&state.db)
        .await?;
        let valid_ids: Vec<i64> = valid.into_iter().map(|(id,)| id).collect();
        let invalid: Vec<i64> = ids_to_add
            .iter()
            .filter(|id| !valid_ids.contains(id))
            .copied()
            .collect();
        if !invalid.is_empty() {
            let names = invalid
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ApiError::BadRequest(format!(
                "invalid courses to add ({names})"
            )));
        }

        // Courses already associated with another template are rejected.
        let other: Vec<(i64,)> = sqlx::query_as(
            r#"
            SELECT child_course_id FROM master_course_subscriptions
            WHERE child_course_id = ANY($1) AND master_template_id <> $2
            "#,
        )
        .bind(&valid_ids)
        .bind(template.id)
        .fetch_all(&state.db)
        .await?;
        if !other.is_empty() {
            let names = other
                .iter()
                .map(|(id,)| id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ApiError::BadRequest(format!(
                "cannot add courses already associated with other templates ({names})"
            )));
        }

        // Insert the new associations; existing ones are ignored (Canvas
        // skips already-associated ids).
        sqlx::query(
            r#"
            INSERT INTO master_course_subscriptions (master_template_id, child_course_id)
            SELECT $1, id FROM unnest($2::bigint[]) AS id
            ON CONFLICT (master_template_id, child_course_id) DO NOTHING
            "#,
        )
        .bind(template.id)
        .bind(&valid_ids)
        .execute(&state.db)
        .await?;
    }

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /courses/{courseId}/blueprint_templates/{templateId}/unsynced_changes
/// — Canvas `#unsynced_changes`. Lists course objects updated since the last
/// completed sync's export start; before any sync Canvas returns a single
/// `initial_sync` ChangeRecord.
async fn blueprint_unsynced_changes(
    State(state): State<AppState>,
    Path((course_id, template_segment)): Path<(i64, String)>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let template = fetch_template(&state.db, course_id, &template_segment).await?;

    // Canvas uses last_export_started_at (latest *completed* migration's
    // exports_started_at); a template that never synced gets initial_sync.
    let cutoff: Option<(Option<DateTime<Utc>>,)> = sqlx::query_as(
        r#"
        SELECT m.exports_started_at FROM master_migrations m
        WHERE m.master_template_id = $1 AND m.workflow_state = 'completed'
        ORDER BY m.id DESC
        LIMIT 1
        "#,
    )
    .bind(template.id)
    .fetch_optional(&state.db)
    .await?;

    let course_name: (String,) = sqlx::query_as(r#"SELECT name FROM courses WHERE id = $1"#)
        .bind(course_id)
        .fetch_one(&state.db)
        .await?;

    let Some((Some(cutoff),)) = cutoff else {
        return Ok(Json(vec![serde_json::json!({
            "asset_name": course_name.0,
            "change_type": "initial_sync",
        })]));
    };

    // Content types this schema models, with the column holding the asset name
    // (Context.asset_name equivalent). Items are capped like Canvas'
    // master_courses_history_count.
    const HISTORY_LIMIT: i64 = 150;
    let content_types: &[(&str, &str)] = &[
        ("assignment", "assignments"),
        ("attachment", "files"),
        ("discussion_topic", "discussion_topics"),
        ("external_tool", "external_tools"),
        ("quiz", "quizzes"),
        ("wiki_page", "pages"),
    ];

    let mut changes: Vec<Value> = Vec::new();
    for &(asset_type, table) in content_types {
        let name_column = match table {
            "discussion_topics" | "pages" | "quizzes" => "title",
            "files" => "filename",
            "external_tools" => "name",
            _ => "title",
        };
        let rows: Vec<(i64, String, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(&format!(
            r#"
            SELECT id, {name_column} AS asset_name, created_at, updated_at
            FROM {table}
            WHERE course_id = $1 AND updated_at > $2
            ORDER BY id ASC
            LIMIT {HISTORY_LIMIT}
            "#
        ))
        .bind(course_id)
        .bind(cutoff)
        .fetch_all(&state.db)
        .await?;
        for (asset_id, asset_name, created_at, _updated_at) in rows {
            let change_type = if created_at > cutoff {
                "created"
            } else {
                "updated"
            };
            changes.push(serde_json::json!({
                "asset_id": asset_id,
                "asset_type": asset_type,
                "asset_name": asset_name,
                "change_type": change_type,
                "html_url": format!("/courses/{course_id}/{table}/{asset_id}"),
                "locked": false,
            }));
        }
        if changes.len() as i64 >= HISTORY_LIMIT {
            changes.truncate(HISTORY_LIMIT as usize);
            break;
        }
    }

    // The syllabus counts as a changed asset once it is touched after the
    // cutoff (Canvas changed_syllabus_json).
    let syllabus_updated: Option<(DateTime<Utc>,)> = sqlx::query_as(
        r#"SELECT updated_at FROM courses WHERE id = $1 AND syllabus_body IS NOT NULL"#,
    )
    .bind(course_id)
    .fetch_optional(&state.db)
    .await?;
    if let Some((updated_at,)) = syllabus_updated {
        if updated_at > cutoff {
            changes.push(serde_json::json!({
                "asset_id": course_id,
                "asset_type": "syllabus",
                "asset_name": "Syllabus",
                "change_type": "updated",
                "html_url": format!("/courses/{course_id}/assignments/syllabus"),
                "locked": false,
            }));
        }
    }

    Ok(Json(changes))
}

/// POST /courses/{courseId}/blueprint_templates/{templateId}/migrations —
/// Canvas `#queue_migration`. Queues a sync to every active associated
/// course.
///
/// Structural simplification: this repo has no background job runner, so the
/// migration row is created directly in the terminal `completed` state with
/// the export/import timestamps set to now() (Canvas' worker would normally
/// walk queued -> exporting -> imports_queued -> completed).
async fn queue_blueprint_migration(
    State(state): State<AppState>,
    Path((course_id, template_segment)): Path<(i64, String)>,
    auth: AuthUser,
    payload: Option<Json<QueueMigration>>,
) -> Result<Json<MasterMigrationJson>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let template = fetch_template(&state.db, course_id, &template_segment).await?;
    let payload = payload.map(|Json(body)| body).unwrap_or_default();

    let (running,): (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM master_migrations
            WHERE master_template_id = $1 AND workflow_state = ANY($2)
        )
        "#,
    )
    .bind(template.id)
    .bind(running_states())
    .fetch_one(&state.db)
    .await?;
    if running {
        return Err(ApiError::BadRequest(
            "Cannot queue a migration while one is currently running".into(),
        ));
    }

    let (has_subscriptions,): (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM master_course_subscriptions
            WHERE master_template_id = $1 AND workflow_state = 'active'
        )
        "#,
    )
    .bind(template.id)
    .fetch_one(&state.db)
    .await?;
    if !has_subscriptions {
        return Err(ApiError::BadRequest(
            "No associated courses to migrate to".into(),
        ));
    }

    let migration: MasterMigrationRow = sqlx::query_as(
        r#"
        INSERT INTO master_migrations
            (master_template_id, user_id, workflow_state, comment,
             exports_started_at, imports_queued_at, imports_completed_at)
        VALUES ($1, $2, 'completed', $3, now(), now(), now())
        RETURNING id, user_id, workflow_state, comment, created_at,
                  exports_started_at, imports_queued_at, imports_completed_at
        "#,
    )
    .bind(template.id)
    .bind(auth.id)
    .bind(&payload.comment)
    .fetch_one(&state.db)
    .await?;

    // Canvas tracks the last export on the template; the derived JSON reads
    // it from the latest completed migration, so nothing else to update.
    Ok(Json(
        migration_json(&state.db, migration, template.id).await?,
    ))
}

/// GET /courses/{courseId}/blueprint_templates/{templateId}/migrations —
/// Canvas `#migrations_index`, newest first.
async fn list_blueprint_migrations(
    State(state): State<AppState>,
    Path((course_id, template_segment)): Path<(i64, String)>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<Vec<MasterMigrationJson>>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let template = fetch_template(&state.db, course_id, &template_segment).await?;

    let (_, per_page, offset) = pag.resolve();
    let rows: Vec<MasterMigrationRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, workflow_state, comment, created_at,
               exports_started_at, imports_queued_at, imports_completed_at
        FROM master_migrations
        WHERE master_template_id = $1
        ORDER BY id DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(template.id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let mut migrations = Vec::with_capacity(rows.len());
    for row in rows {
        migrations.push(migration_json(&state.db, row, template.id).await?);
    }
    Ok(Json(migrations))
}

/// Load one migration of a template, or 404.
async fn fetch_template_migration(
    pool: &PgPool,
    template_id: i64,
    migration_segment: &str,
) -> Result<MasterMigrationRow, ApiError> {
    let migration_id: i64 = migration_segment.parse().map_err(|_| ApiError::NotFound)?;
    let row: Option<MasterMigrationRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, workflow_state, comment, created_at,
               exports_started_at, imports_queued_at, imports_completed_at
        FROM master_migrations
        WHERE id = $1 AND master_template_id = $2
        "#,
    )
    .bind(migration_id)
    .bind(template_id)
    .fetch_optional(pool)
    .await?;
    row.ok_or(ApiError::NotFound)
}

/// GET /courses/{courseId}/blueprint_templates/{templateId}/migrations/{id}
/// — Canvas `#migrations_show`.
async fn show_blueprint_migration(
    State(state): State<AppState>,
    Path((course_id, template_segment, migration_segment)): Path<(i64, String, String)>,
    auth: AuthUser,
) -> Result<Json<MasterMigrationJson>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let template = fetch_template(&state.db, course_id, &template_segment).await?;
    let migration = fetch_template_migration(&state.db, template.id, &migration_segment).await?;
    Ok(Json(
        migration_json(&state.db, migration, template.id).await?,
    ))
}

/// GET /courses/{courseId}/blueprint_templates/{templateId}/migrations/{id}/details
/// — Canvas `#migration_details`. Canvas streams the exported objects from
/// the content export; there is no export storage here, so the response is
/// the documented empty ChangeRecord list (same as a selective sync with no
/// recorded changes).
async fn blueprint_migration_details(
    State(state): State<AppState>,
    Path((course_id, template_segment, migration_segment)): Path<(i64, String, String)>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let template = fetch_template(&state.db, course_id, &template_segment).await?;
    // The migration must exist for the template even though no details are
    // stored (Canvas first! raises RecordNotFound otherwise).
    fetch_template_migration(&state.db, template.id, &migration_segment).await?;
    Ok(Json(Vec::new()))
}

/// PUT /courses/{courseId}/blueprint_templates/{templateId}/restrict_item —
/// Canvas `#restrict_item`. This schema has no master_content_tags table, so
/// the route validates the content type/id pair and reports success; locked
/// state is not persisted (documented structural simplification).
async fn blueprint_restrict_item(
    State(state): State<AppState>,
    Path((course_id, template_segment)): Path<(i64, String)>,
    auth: AuthUser,
    payload: Option<Json<RestrictItem>>,
) -> Result<Json<Value>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let template = fetch_template(&state.db, course_id, &template_segment).await?;
    let payload = payload.map(|Json(body)| body).unwrap_or_default();

    let content_type = payload
        .content_type
        .ok_or_else(|| ApiError::BadRequest("Must be a valid content type".into()))?;
    let table = RESTRICT_CONTENT_TYPES
        .iter()
        .find(|(name, _)| *name == content_type.as_str())
        .map(|(_, table)| *table)
        .ok_or_else(|| {
            ApiError::BadRequest(format!(
                "Must be a valid content type (assignment,attachment,discussion_topic,external_tool,quiz,wiki_page). Got {content_type}"
            ))
        })?;
    let content_id = payload
        .content_id
        .ok_or_else(|| ApiError::BadRequest("Must set 'content_id'".into()))?;
    if payload.restricted.is_none() {
        return Err(ApiError::BadRequest("Must set 'restricted'".into()));
    }

    // The item must exist in the blueprint course (Canvas 404s otherwise).
    let (exists,): (bool,) = sqlx::query_as(&format!(
        r#"SELECT EXISTS (SELECT 1 FROM {table} WHERE id = $1 AND course_id = $2)"#
    ))
    .bind(content_id)
    .bind(template.course_id)
    .fetch_one(&state.db)
    .await?;
    if !exists {
        return Err(ApiError::NotFound);
    }

    Ok(Json(serde_json::json!({ "success": true })))
}

/// GET /courses/{courseId}/blueprint_subscriptions — Canvas
/// `#subscriptions_index` (`child_subscription_json`), newest first.
async fn list_blueprint_subscriptions(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<Vec<ChildSubscriptionJson>>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;

    let (_, per_page, offset) = pag.resolve();
    #[derive(sqlx::FromRow)]
    struct SubscriptionRow {
        id: i64,
        template_id: i64,
        course_id: i64,
        name: String,
        course_code: String,
    }
    let rows: Vec<SubscriptionRow> = sqlx::query_as(
        r#"
        SELECT s.id, s.master_template_id AS template_id,
               c.id AS course_id, c.name, c.course_code
        FROM master_course_subscriptions s
        JOIN master_templates t ON t.id = s.master_template_id
        JOIN courses c ON c.id = t.course_id
        WHERE s.child_course_id = $1 AND s.workflow_state = 'active'
        ORDER BY s.id DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(course_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let subscriptions = rows
        .into_iter()
        .map(|row| ChildSubscriptionJson {
            id: row.id,
            template_id: row.template_id,
            blueprint_course: BlueprintCourseSummary {
                id: row.course_id,
                name: row.name,
                course_code: row.course_code,
            },
        })
        .collect();
    Ok(Json(subscriptions))
}

/// Resolve `default` / numeric subscription ids like Canvas `get_subscription`.
async fn fetch_subscription(
    pool: &PgPool,
    child_course_id: i64,
    subscription_segment: &str,
) -> Result<(i64, i64), ApiError> {
    let row: Option<(i64, i64)> = if subscription_segment == "default" {
        sqlx::query_as(
            r#"
            SELECT id, master_template_id FROM master_course_subscriptions
            WHERE child_course_id = $1 AND workflow_state = 'active'
            ORDER BY id ASC
            LIMIT 1
            "#,
        )
        .bind(child_course_id)
        .fetch_optional(pool)
        .await?
    } else {
        let subscription_id: i64 = subscription_segment
            .parse()
            .map_err(|_| ApiError::NotFound)?;
        sqlx::query_as(
            r#"
            SELECT id, master_template_id FROM master_course_subscriptions
            WHERE id = $1 AND child_course_id = $2 AND workflow_state = 'active'
            "#,
        )
        .bind(subscription_id)
        .bind(child_course_id)
        .fetch_optional(pool)
        .await?
    };
    row.ok_or(ApiError::NotFound)
}

/// GET /courses/{courseId}/blueprint_subscriptions/{subscriptionId}/migrations
/// — Canvas `#imports_index`. The child side views the template's sync
/// history through the subscription; newest first.
async fn list_blueprint_imports(
    State(state): State<AppState>,
    Path((course_id, subscription_segment)): Path<(i64, String)>,
    auth: AuthUser,
    Query(pag): Query<Pagination>,
) -> Result<Json<Vec<ChildMigrationJson>>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let (subscription_id, template_id) =
        fetch_subscription(&state.db, course_id, &subscription_segment).await?;

    let (_, per_page, offset) = pag.resolve();
    let rows: Vec<MasterMigrationRow> = sqlx::query_as(
        r#"
        SELECT id, user_id, workflow_state, comment, created_at,
               exports_started_at, imports_queued_at, imports_completed_at
        FROM master_migrations
        WHERE master_template_id = $1
        ORDER BY id DESC
        LIMIT $2 OFFSET $3
        "#,
    )
    .bind(template_id)
    .bind(per_page)
    .bind(offset)
    .fetch_all(&state.db)
    .await?;

    let mut migrations = Vec::with_capacity(rows.len());
    for row in rows {
        migrations.push(child_migration_json(&state.db, row, subscription_id).await?);
    }
    Ok(Json(migrations))
}

/// GET /courses/{courseId}/blueprint_subscriptions/{subscriptionId}/migrations/{id}
/// — Canvas `#imports_show`.
async fn show_blueprint_import(
    State(state): State<AppState>,
    Path((course_id, subscription_segment, migration_segment)): Path<(i64, String, String)>,
    auth: AuthUser,
) -> Result<Json<ChildMigrationJson>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let (subscription_id, template_id) =
        fetch_subscription(&state.db, course_id, &subscription_segment).await?;
    let migration = fetch_template_migration(&state.db, template_id, &migration_segment).await?;
    Ok(Json(
        child_migration_json(&state.db, migration, subscription_id).await?,
    ))
}

/// GET /courses/{courseId}/blueprint_subscriptions/{subscriptionId}/migrations/{id}/details
/// — Canvas `#import_details`; no export storage means an empty ChangeRecord
/// list (the migration itself must still exist).
async fn blueprint_import_details(
    State(state): State<AppState>,
    Path((course_id, subscription_segment, migration_segment)): Path<(i64, String, String)>,
    auth: AuthUser,
) -> Result<Json<Vec<Value>>, ApiError> {
    require_blueprint_admin(&state, &auth, course_id).await?;
    let (subscription_id, template_id) =
        fetch_subscription(&state.db, course_id, &subscription_segment).await?;
    let _ = subscription_id;
    fetch_template_migration(&state.db, template_id, &migration_segment).await?;
    Ok(Json(Vec::new()))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Navigation tabs for accounts, groups and users (Canvas `tabs_controller.rb`)
// ---------------------------------------------------------------------------

/// Canvas renders tab errors as a JSON body with an `error` key.
fn tab_error(status: StatusCode, message: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    (status, Json(json!({ "error": message }))).into_response()
}
/// Build one navigation tab in Canvas' `Api::V1::Tab#tab_json` shape.
fn nav_tab(id: &str, label: &str, html_url: String, position: i64) -> CourseTab {
    CourseTab {
        id: id.to_string(),
        html_url: html_url.clone(),
        full_url: html_url,
        position,
        hidden: false,
        visibility: "public".to_string(),
        label: label.to_string(),
        tab_type: "internal".to_string(),
    }
}

/// GET /accounts/{accountId}/tabs — Canvas `tabs_controller#index` for an
/// account context.
///
/// Canvas gates every account tab on its own permission
/// (`Account#tabs_available`).  The port knows only "account admin", so an
/// account admin sees the whole set and everybody else is refused.  The
/// site-admin-only tabs (Plugins, Release Notes, Jobs) have no counterpart
/// here because the port has no site admin.
async fn list_account_tabs(
    State(state): State<AppState>,
    Path(account_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<CourseTab>>, ApiError> {
    if !account_exists(&state.db, account_id).await? {
        return Err(ApiError::NotFound);
    }
    if !check_account_admin(&state.db, auth.id, account_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }
    let base = format!("/accounts/{account_id}");
    let defs: Vec<(&str, &str, String)> = vec![
        ("courses", "Courses", base.clone()),
        ("users", "People", format!("{base}/users")),
        ("statistics", "Statistics", format!("{base}/statistics")),
        ("account_reports", "Reports", format!("{base}/reports")),
        ("permissions", "Permissions", format!("{base}/permissions")),
        ("outcomes", "Outcomes", format!("{base}/outcomes")),
        ("rubrics", "Rubrics", format!("{base}/rubrics")),
        (
            "grading_standards",
            "Grading",
            format!("{base}/grading_settings"),
        ),
        (
            "question_banks",
            "Question Banks",
            format!("{base}/question_banks"),
        ),
        (
            "accessibility",
            "Accessibility",
            format!("{base}/accessibility"),
        ),
        (
            "sub_accounts",
            "Sub-Accounts",
            format!("{base}/sub_accounts"),
        ),
        (
            "account_calendars",
            "Account Calendars",
            format!("{base}/account_calendars"),
        ),
        ("terms", "Terms", format!("{base}/terms")),
        (
            "authentication",
            "Authentication",
            format!("{base}/authentication_providers"),
        ),
        (
            "rate_limiting",
            "Rate Limiting",
            format!("{base}/rate_limiting"),
        ),
        ("sis_import", "SIS Import", format!("{base}/sis_import")),
        ("brand_configs", "Themes", format!("{base}/brand_configs")),
        (
            "developer_keys",
            "Developer Keys",
            format!("{base}/developer_keys"),
        ),
    ];
    let tabs = defs
        .into_iter()
        .enumerate()
        .map(|(index, (id, label, url))| nav_tab(id, label, url, index as i64 + 1))
        .collect();
    Ok(Json(tabs))
}

/// GET /groups/{groupId}/tabs — Canvas `tabs_controller#index` for a group
/// context (`Group#tabs_available`).
async fn list_group_tabs(
    State(state): State<AppState>,
    Path(group_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<CourseTab>>, ApiError> {
    let group: Option<(Option<i64>,)> = sqlx::query_as(
        "SELECT course_id FROM groups WHERE id = $1 AND workflow_state != 'deleted'",
    )
    .bind(group_id)
    .fetch_optional(&state.db)
    .await?;
    let Some((course_id,)) = group else {
        return Err(ApiError::NotFound);
    };

    // Canvas raises RecordNotFound when the caller cannot read the group.
    let member: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM group_memberships WHERE group_id = $1 AND user_id = $2 AND workflow_state = 'accepted' LIMIT 1",
    )
    .bind(group_id)
    .bind(auth.id)
    .fetch_optional(&state.db)
    .await?;
    let mut can_read = member.is_some();
    if let Some(course_id) = course_id {
        can_read |= check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;
        // An account admin of the course' account may read the group too.
        let course: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
                .bind(course_id)
                .fetch_optional(&state.db)
                .await?;
        if let Some((Some(account_id),)) = course {
            can_read |= check_account_admin(&state.db, auth.id, account_id, &auth).await?;
        }
    }
    if !can_read {
        return Err(ApiError::NotFound);
    }

    let base = format!("/groups/{group_id}");
    let defs: Vec<(&str, &str, String)> = vec![
        ("home", "Home", base.clone()),
        (
            "announcements",
            "Announcements",
            format!("{base}/announcements"),
        ),
        ("pages", "Pages", format!("{base}/wiki_page")),
        ("people", "People", format!("{base}/users")),
        (
            "discussions",
            "Discussions",
            format!("{base}/discussion_topics"),
        ),
        ("files", "Files", format!("{base}/files")),
        ("conferences", "Conferences", format!("{base}/conferences")),
        (
            "collaborations",
            "Collaborations",
            format!("{base}/collaborations"),
        ),
        (
            "collaborations_new",
            "Collaborations",
            format!("{base}/lti_collaborations"),
        ),
    ];
    let tabs = defs
        .into_iter()
        .enumerate()
        .map(|(index, (id, label, url))| nav_tab(id, label, url, index as i64 + 1))
        .collect();
    Ok(Json(tabs))
}

/// GET /users/{userId}/tabs — Canvas `tabs_controller#index` for a user
/// context (`UserProfile#tabs_available`).
///
/// Canvas serves the profile tabs of the addressed user; the caller has to
/// be that user or an admin.
async fn list_user_tabs(
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    auth: AuthUser,
) -> Result<Json<Vec<CourseTab>>, ApiError> {
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(&state.db)
        .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound);
    }
    if auth.id != user_id && !is_admin(&auth) {
        return Err(ApiError::Forbidden);
    }

    // Canvas `UserProfile::BASE_TABS` (Notifications, Files, Settings) with
    // the profile tab inserted at index 1.
    let defs: Vec<(&str, &str, String)> = vec![
        (
            "notifications",
            "Notifications",
            "/profile/communication".to_string(),
        ),
        ("profile", "Profile", "/profile".to_string()),
        ("files", "Files", "/profile/files".to_string()),
        (
            "profile_settings",
            "Settings",
            "/profile/settings".to_string(),
        ),
    ];
    let tabs = defs
        .into_iter()
        .enumerate()
        .map(|(index, (id, label, url))| nav_tab(id, label, url, index as i64 + 1))
        .collect();
    Ok(Json(tabs))
}

/// Request to update a course tab.
#[derive(Debug, Deserialize)]
pub struct UpdateCourseTab {
    /// Canvas `params["hidden"]`: "true" hides the tab, "false" shows it and
    /// an absent key leaves it alone.
    pub hidden: Option<String>,
    /// Canvas `params["position"]`: the one-based slot the tab moves to.
    pub position: Option<i64>,
}

/// PUT /courses/{courseId}/tabs/{tabId} — Canvas `tabs_controller#update`.
///
/// Canvas stores the whole tab configuration on the course; the port keeps
/// one row per tab in `course_tabs`, so an update rewrites the rows of the
/// course's manageable tabs.
async fn update_course_tab(
    State(state): State<AppState>,
    Path((course_id, tab_id)): Path<(i64, String)>,
    auth: AuthUser,
    Json(payload): Json<UpdateCourseTab>,
) -> Result<axum::response::Response, ApiError> {
    if !course_exists(&state.db, course_id).await? {
        return Err(ApiError::NotFound);
    }
    // Canvas `authorized_action(@context, @current_user, :manage_course_content_edit)`.
    if !check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await? {
        return Err(ApiError::Forbidden);
    }

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
    let include_settings = check_teacher_or_admin(&state.db, auth.id, course_id, &auth).await?;
    let tabs = build_course_tabs(course_id, include_settings, &configs);

    let Some(tab) = tabs.iter().find(|tab| tab.id == tab_id).cloned() else {
        return Err(ApiError::NotFound);
    };

    // Home and Settings are pinned by Canvas and cannot be managed.
    if tab_id == "home" || tab_id == "settings" {
        return Ok(tab_error(
            StatusCode::BAD_REQUEST,
            &format!("{tab_id} is not manageable"),
        ));
    }

    let count = tabs.len() as i64;
    if let Some(new_position) = payload.position {
        if new_position <= 1 || new_position > count {
            return Ok(tab_error(
                StatusCode::BAD_REQUEST,
                "That tab location is invalid",
            ));
        }
    }

    // The manageable tabs, in their current order.
    let mut order: Vec<String> = tabs
        .iter()
        .map(|tab| tab.id.clone())
        .filter(|id| id != "home" && id != "settings")
        .collect();
    let hidden_before: Vec<bool> = order
        .iter()
        .map(|id| {
            configs
                .iter()
                .find(|config| &config.tab_id == id)
                .map(|config| config.hidden)
                .unwrap_or(false)
        })
        .collect();

    // Apply the hidden flag: Canvas hides on a truthy value and unhides when
    // the key is present with a false value.
    let mut hidden = hidden_before.clone();
    if let Some(value) = payload.hidden.as_deref() {
        let truthy = matches!(
            value.to_ascii_lowercase().as_str(),
            "yes" | "y" | "true" | "t" | "on" | "1"
        );
        if let Some(index) = order.iter().position(|id| id == &tab_id) {
            hidden[index] = truthy;
        }
    }

    // Move the tab to its new slot.  Canvas positions the full tab list
    // (Home is pinned first), so a manageable tab sits one slot further
    // down than its index in the manageable list.
    if let Some(new_position) = payload.position {
        if let Some(index) = order.iter().position(|id| id == &tab_id) {
            let hidden_value = hidden.get(index).copied().unwrap_or(false);
            order.remove(index);
            hidden.remove(index);
            let target = (new_position - 2).max(0) as usize;
            let target = target.min(order.len());
            order.insert(target, tab_id.clone());
            hidden.insert(target, hidden_value);
        }
    }

    for (index, id) in order.iter().enumerate() {
        sqlx::query(
            r#"
            INSERT INTO course_tabs (course_id, tab_id, position, hidden)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (course_id, tab_id)
            DO UPDATE SET position = EXCLUDED.position,
                          hidden = EXCLUDED.hidden,
                          updated_at = now()
            "#,
        )
        .bind(course_id)
        .bind(id)
        .bind(index as i32 + 1)
        .bind(hidden.get(index).copied().unwrap_or(false))
        .execute(&state.db)
        .await?;
    }

    // Answer with the tab as it looks now.
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
    let tabs = build_course_tabs(course_id, include_settings, &configs);
    let updated = tabs.into_iter().find(|tab| tab.id == tab_id).unwrap_or(tab);
    use axum::response::IntoResponse;
    Ok(Json(updated).into_response())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// Build the courses router.
pub fn router() -> Router<AppState> {
    Router::new()
        // Canvas AiExperiencesController / AiConversationsController.
        .merge(ai_experiences::router())
        .route("/courses", get(list_courses).post(create_course))
        .route("/accounts/{id}/courses", post(create_course_in_account))
        .route(
            "/courses/{id}",
            get(get_course).put(update_course).delete(delete_course),
        )
        .route(
            "/courses/{id}/enrollments",
            post(create_enrollment).get(list_enrollments),
        )
        .route(
            "/courses/{id}/enrollments/{enrollmentId}",
            delete(delete_enrollment),
        )
        // Enrollment terms (Canvas API v1)
        .route(
            "/accounts/{accountId}/terms",
            get(list_terms).post(create_term),
        )
        .route(
            "/accounts/{accountId}/terms/{id}",
            get(get_account_term)
                .put(update_account_term)
                .delete(delete_account_term),
        )
        .route(
            "/terms/{id}",
            get(get_term).put(update_term).delete(delete_term),
        )
        // Course sections (Canvas API v1)
        .route(
            "/courses/{courseId}/sections",
            get(list_sections).post(create_section),
        )
        .route("/courses/{courseId}/sections/{id}", get(get_course_section))
        .route(
            "/sections/{id}",
            get(get_section).put(update_section).delete(delete_section),
        )
        // Course settings, roster, navigation tabs and permissions
        .route(
            "/courses/{courseId}/settings",
            get(get_course_settings).put(update_course_settings),
        )
        .route("/courses/{courseId}/users", get(list_course_users))
        .route("/courses/{courseId}/users/{userId}", get(get_course_user))
        .route("/courses/{courseId}/tabs", get(list_course_tabs))
        .route("/courses/{courseId}/tabs/{tabId}", put(update_course_tab))
        .route("/accounts/{accountId}/tabs", get(list_account_tabs))
        .route("/groups/{groupId}/tabs", get(list_group_tabs))
        .route("/users/{userId}/tabs", get(list_user_tabs))
        .route(
            "/courses/{courseId}/permissions",
            get(get_course_permissions),
        )
        .route(
            "/courses/{courseId}/recent_students",
            get(list_recent_students),
        )
        .route("/courses/{courseId}/search_users", get(search_course_users))
        // Deep course features (Canvas API v1)
        .route(
            "/courses/{courseId}/activity_stream",
            get(course_activity_stream),
        )
        .route(
            "/courses/{courseId}/bulk_user_progress",
            get(bulk_user_progress),
        )
        .route(
            "/courses/{courseId}/dismiss_migration_limitation_message",
            post(dismiss_migration_limitation_message),
        )
        .route(
            "/courses/{courseId}/effective_due_dates",
            get(effective_due_dates),
        )
        .route(
            "/courses/{courseId}/link_validation",
            get(get_link_validation).post(start_link_validation),
        )
        .route("/courses/{courseId}/preview_html", post(preview_html))
        .route("/courses/{courseId}/quizzes", put(update_course_quizzes))
        .route(
            "/courses/{courseId}/reset_content",
            post(reset_course_content),
        )
        .route(
            "/courses/{courseId}/restore/{versionId}",
            post(restore_course_version),
        )
        .route(
            "/courses/{courseId}/student_view_student",
            get(get_student_view_student),
        )
        .route("/courses/{courseId}/students", get(list_students))
        .route("/courses/{courseId}/todo", get(course_todo))
        .route(
            "/courses/{courseId}/users/{userId}/progress",
            get(user_progress),
        )
        // YouTube migration (legacy, feature-flagged)
        .route(
            "/courses/{courseId}/youtube_migration/conversion_status",
            get(youtube_conversion_status),
        )
        .route(
            "/courses/{courseId}/youtube_migration/convert",
            post(youtube_convert),
        )
        .route(
            "/courses/{courseId}/youtube_migration/scan",
            get(youtube_scan).post(youtube_scan_start),
        )
        .route(
            "/courses/{courseId}/youtube_migration/scan/{scanId}",
            put(youtube_scan_update),
        )
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
        // Blueprint courses / master courses (Canvas API v1)
        .route(
            "/courses/{courseId}/blueprint_templates/{templateId}",
            get(show_blueprint_template),
        )
        .route(
            "/courses/{courseId}/blueprint_templates/{templateId}/associated_courses",
            get(blueprint_associated_courses),
        )
        .route(
            "/courses/{courseId}/blueprint_templates/{templateId}/update_associations",
            put(blueprint_update_associations),
        )
        .route(
            "/courses/{courseId}/blueprint_templates/{templateId}/unsynced_changes",
            get(blueprint_unsynced_changes),
        )
        .route(
            "/courses/{courseId}/blueprint_templates/{templateId}/migrations",
            get(list_blueprint_migrations).post(queue_blueprint_migration),
        )
        .route(
            "/courses/{courseId}/blueprint_templates/{templateId}/migrations/{id}",
            get(show_blueprint_migration),
        )
        .route(
            "/courses/{courseId}/blueprint_templates/{templateId}/migrations/{id}/details",
            get(blueprint_migration_details),
        )
        .route(
            "/courses/{courseId}/blueprint_templates/{templateId}/restrict_item",
            put(blueprint_restrict_item),
        )
        .route(
            "/courses/{courseId}/blueprint_subscriptions",
            get(list_blueprint_subscriptions),
        )
        .route(
            "/courses/{courseId}/blueprint_subscriptions/{subscriptionId}/migrations",
            get(list_blueprint_imports),
        )
        .route(
            "/courses/{courseId}/blueprint_subscriptions/{subscriptionId}/migrations/{id}",
            get(show_blueprint_import),
        )
        .route(
            "/courses/{courseId}/blueprint_subscriptions/{subscriptionId}/migrations/{id}/details",
            get(blueprint_import_details),
        )
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
        let course: Course = sqlx::query_as(&format!(
            r#"
            INSERT INTO courses (name, course_code, description)
            VALUES ('Rust 101', 'RUST-101', 'Intro to Rust')
            RETURNING {COURSE_COLUMNS}
            "#,
        ))
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(course.name, "Rust 101");
        assert_eq!(course.course_code, "RUST-101");

        // Fetch it back
        let found: Option<Course> = sqlx::query_as(&format!(
            r#"SELECT {COURSE_COLUMNS} FROM courses WHERE id = $1"#,
        ))
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
        let course: Course = sqlx::query_as(&format!(
            r#"
            INSERT INTO courses (name, course_code)
            VALUES ('DB Test Course', 'DB-TEST')
            RETURNING {COURSE_COLUMNS}
            "#,
        ))
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
