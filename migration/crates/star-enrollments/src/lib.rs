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
use star_common::{ApiError, ApiJson, AppState};

mod enrollment_json;
mod observer_alert_thresholds;
mod temporary_enrollment_pairings;

use crate::enrollment_json::{
    enrollment_columns, enrollment_order, port_enrollment_json, EnrollmentApiRow,
    EnrollmentJsonOptions, GradeRights, ENROLLMENT_JOINS,
};
use std::collections::HashSet;

// --------------------------------------------------------------------------
// Domain types
// --------------------------------------------------------------------------

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
#[derive(Debug, Default, Deserialize)]
pub struct CreateSectionEnrollmentRequest {
    #[serde(default)]
    pub user_id: Option<i64>,
    pub role: Option<String>,
}

/// Query params for listing enrollments.
#[derive(Debug, Deserialize)]
pub struct EnrollmentFilter {
    pub user_id: Option<i64>,
    pub course_id: Option<i64>,
}

/// What the caller may see of the enrollments it reads.
///
/// Canvas decides which optional parts of the enrollment payload a caller
/// gets from its rights on the enrollment's course
/// (`Api::V1::User#enrollment_json`, lib/api/v1/user.rb:292):
///
/// - the activity fields need the enrollment to be the caller's own or the
///   caller to be able to read course reports,
/// - the SIS ids need the caller to be able to read SIS data,
/// - the grades need grade rights, and their contents depend on whether the
///   caller may also manage the grades.
///
/// The port has no course-report, SIS or grade-scheme permission model, so
/// the site admin and the teachers of the course stand in for them.
struct CallerRights {
    admin: bool,
    taught_courses: HashSet<i64>,
}

impl CallerRights {
    /// The courses the caller teaches (Canvas: a TeacherEnrollment).
    async fn load(
        pool: &PgPool,
        caller_id: i64,
        admin: bool,
        course_ids: &[i64],
    ) -> Result<Self, ApiError> {
        let rows: Vec<(i64,)> = sqlx::query_as(
            r#"
            SELECT DISTINCT course_id FROM enrollments
            WHERE user_id = $1 AND role = 'teacher' AND course_id = ANY($2)
            "#,
        )
        .bind(caller_id)
        .bind(course_ids.to_vec())
        .fetch_all(pool)
        .await
        .map_err(ApiError::from)?;

        Ok(CallerRights {
            admin,
            taught_courses: rows.into_iter().map(|(course_id,)| course_id).collect(),
        })
    }

    /// Canvas renders the activity fields for the enrolled user and for
    /// callers who may read course reports.
    fn sees_activity(&self, enrollment_user_id: i64, caller_id: i64) -> bool {
        enrollment_user_id == caller_id || self.admin
    }

    /// Canvas renders the SIS ids for callers who may read SIS data.
    fn reads_sis(&self, course_id: i64) -> bool {
        self.admin || self.taught_courses.contains(&course_id)
    }

    /// The grade rights of the caller on the enrollment's course.
    fn grade_rights(&self, enrollment_user_id: i64, caller_id: i64, course_id: i64) -> GradeRights {
        if self.admin || self.taught_courses.contains(&course_id) {
            GradeRights::Manage
        } else if enrollment_user_id == caller_id {
            GradeRights::Read
        } else {
            GradeRights::None
        }
    }

    /// The options of one enrollment row.
    fn options(
        &self,
        row: &EnrollmentApiRow,
        caller_id: i64,
        include_user: bool,
    ) -> EnrollmentJsonOptions {
        EnrollmentJsonOptions {
            include_user,
            include_activity: self.sees_activity(row.user_id, caller_id),
            include_sis: self.reads_sis(row.course_id),
            grade_rights: self.grade_rights(row.user_id, caller_id, row.course_id),
        }
    }
}

/// The select of the enrollment rows, with the where clause and the order
/// Canvas uses (`enrollments.order(:type, users.sortable_name, :id)`).
fn enrollment_query(where_clause: &str) -> String {
    format!(
        "SELECT {} {} WHERE {} {}",
        enrollment_columns(),
        ENROLLMENT_JOINS,
        where_clause,
        enrollment_order()
    )
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

/// Query params for `GET /users/{userId}/temporary_enrollment_status`.
///
/// Canvas accepts an optional `account_id` to scope the check to; it defaults
/// to the domain root account when absent.
#[derive(Debug, Deserialize)]
pub struct TemporaryEnrollmentStatusParams {
    pub account_id: Option<i64>,
}

/// The default account context for the temporary-enrollment status routes:
/// Canvas falls back to `@domain_root_account` (the seeded root account, id 1,
/// from migration 0024) when the request does not name one.
const DEFAULT_ROOT_ACCOUNT_ID: i64 = 1;

/// Resolve the account context for a temporary-enrollment status request and
/// check the `temporary_enrollments` flag on it.
///
/// Canvas ships `temporary_enrollments` as a standard root-account option
/// that is on unless an account explicitly opts out, so an absent override
/// means enabled and only an explicit `disabled` state turns the status
/// endpoints off. `None` means "feature disabled for this account".
async fn temporary_enrollment_account_or_none(
    pool: &PgPool,
    requested: Option<i64>,
) -> Result<Option<i64>, ApiError> {
    let account_id = requested.unwrap_or(DEFAULT_ROOT_ACCOUNT_ID);

    let (disabled,): (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM feature_flags
            WHERE context_type = 'Account'
              AND context_id = $1
              AND feature = 'temporary_enrollments'
              AND state = 'disabled'
        )
        "#,
    )
    .bind(account_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)?;

    if disabled {
        Ok(None)
    } else {
        Ok(Some(account_id))
    }
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

/// Parse the bulk temporary-enrollment status query: `user_ids` in both
/// comma-separated (`1,2,3`) and repeated-bracket (`user_ids[]=1&user_ids[]=2`)
/// forms, plus the optional `limit` and `account_id` parameters. Infallible —
/// an empty `user_ids` is reported by the caller so the feature check can run
/// first, mirroring the Canvas guard order.
fn parse_bulk_enrollment_query(raw_query: Option<&str>) -> (Vec<i64>, Option<i64>, Option<i64>) {
    let mut user_ids: Vec<i64> = Vec::new();
    let mut limit: Option<i64> = None;
    let mut account_id: Option<i64> = None;

    let Some(raw_query) = raw_query else {
        return (user_ids, limit, account_id);
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
            "account_id" => {
                if let Ok(a) = value.parse::<i64>() {
                    account_id = Some(a);
                }
            }
            _ => {}
        }
    }

    (user_ids, limit, account_id)
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
/// Ask the shared permission layer whether the acting user holds a
/// permission in this course.
///
/// Canvas answers with the permission registry, not with "is this user a
/// teacher", so the port asks the same question the same way
/// ([`star_permissions::check::require`]).
async fn require_course_permission(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    permission: &str,
) -> Result<(), ApiError> {
    star_permissions::check::require(
        pool,
        user_id,
        &star_permissions::PermissionContext::Course { course_id },
        permission,
    )
    .await
}

/// The Canvas permission that lets a user add an enrollment of `role`.
///
/// Canvas `User#can_create_enrollment_for?`
/// (`app/models/user.rb:3474-3485`).
fn add_enrollment_permission(role: &str) -> &'static str {
    match role {
        "teacher" => "add_teacher_to_course",
        "ta" => "add_ta_to_course",
        "designer" => "add_designer_to_course",
        "observer" => "add_observer_to_course",
        _ => "add_student_to_course",
    }
}

/// The Canvas role of an enrollment row, as the permission registry spells it.
fn enrollment_role_kind(role: &str) -> star_permissions::RoleKind {
    star_permissions::RoleKind::from_port_name(role)
        .unwrap_or(star_permissions::RoleKind::StudentEnrollment)
}

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

/// Insert an active enrollment and return its row. 409 when (user, course,
/// role) is already enrolled; Canvas allows one enrollment per user/course/role
/// triple in this schema.
async fn insert_enrollment(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    course_section_id: Option<i64>,
    role: &str,
) -> Result<EnrollmentApiRow, ApiError> {
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

    sqlx::query_as(&enrollment_query(
        "e.user_id = $1 AND e.course_id = $2 AND e.role = $3",
    ))
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
        // The alert thresholds of an observer.
        .merge(observer_alert_thresholds::router())
        // The temporary enrollment pairings of an account.
        .merge(temporary_enrollment_pairings::router())
        .route(
            "/enrollments",
            post(create_enrollment).get(list_enrollments),
        )
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
        // Canvas: GET /api/v1/accounts/:account_id/enrollments/:id (#show).
        .route(
            "/accounts/{accountId}/enrollments/{id}",
            get(show_account_enrollment),
        )
        // Canvas: POST /api/v1/accounts/:account_id/bulk_enrollment.
        .route(
            "/accounts/{accountId}/bulk_enrollment",
            post(create_bulk_enrollment),
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
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    let role = payload.role.as_deref().unwrap_or("student");
    validate_enrollment_role(role)?;

    // Canvas `User#can_create_enrollment_for?` -> the granular
    // `add_*_to_course` permission for the enrollment's role on that course
    // (`app/models/user.rb:3474-3485`).
    require_course_permission(
        &state.db,
        auth.id,
        payload.course_id,
        add_enrollment_permission(role),
    )
    .await?;

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

    let rights = CallerRights::load(
        &state.db,
        auth.id,
        is_admin(&auth_role),
        &[enrollment.course_id],
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(port_enrollment_json(
            &enrollment,
            &rights.options(&enrollment, auth.id, false),
        )),
    ))
}

/// GET /enrollments — list enrollments.
/// Admin sees all; teachers see enrollments for courses they're teaching.
async fn list_enrollments(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(filter): Query<EnrollmentFilter>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    // Non-admin: only enrollments for courses this user teaches.
    let taught_course_ids: Option<Vec<i64>> = if is_admin(&auth_role) {
        None
    } else {
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

        if let Some(course_id) = filter.course_id {
            if !course_ids.contains(&course_id) {
                return Err(ApiError::Forbidden);
            }
        }

        Some(course_ids)
    };

    let rows: Vec<EnrollmentApiRow> = sqlx::query_as(&enrollment_query(
        "e.workflow_state != 'deleted' \
          AND ($1::bigint IS NULL OR e.user_id = $1) \
          AND ($2::bigint IS NULL OR e.course_id = $2) \
          AND ($3::bigint[] IS NULL OR e.course_id = ANY($3))",
    ))
    .bind(filter.user_id)
    .bind(filter.course_id)
    .bind(taught_course_ids)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    let course_ids: Vec<i64> = rows.iter().map(|row| row.course_id).collect();
    let rights = CallerRights::load(&state.db, auth.id, is_admin(&auth_role), &course_ids).await?;

    let payload: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| port_enrollment_json(row, &rights.options(row, auth.id, true)))
        .collect();

    Ok(Json(payload))
}

/// DELETE /enrollments/{id} — delete an enrollment.
/// Course teacher or admin only.
async fn delete_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(enrollment_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    // Fetch the enrollment to know which course it's in.
    let enrollment: Option<(i64, i64, String)> =
        sqlx::query_as("SELECT course_id, user_id, role FROM enrollments WHERE id = $1")
            .bind(enrollment_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::from)?;

    let (course_id, _enrolled_user_id, role) = enrollment.ok_or(ApiError::NotFound)?;

    // Canvas `EnrollmentsApiController#destroy` without a `task` parameter
    // checks `Enrollment#can_be_concluded_by`
    // (`app/controllers/enrollments_api_controller.rb:947-968`): a student
    // enrollment needs `manage_students`, every other enrollment needs
    // `allow_course_admin_actions` on the course.
    if !star_permissions::check::can_conclude_enrollment(
        &state.db,
        auth.id,
        course_id,
        enrollment_role_kind(&role),
    )
    .await?
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
///
/// The answer is the Canvas enrollment payload: the index action of
/// `EnrollmentsApiController` renders every enrollment with
/// `enrollment_json`, and it always includes the nested user.
async fn list_section_enrollments(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(section_id): Path<i64>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let section = load_section(&state.db, section_id).await?;

    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    // Canvas `EnrollmentsApiController#course_index_enrollments` ->
    // `grants_any_right?(:read_roster, :view_all_grades, :manage_grades)`
    // (`app/controllers/enrollments_api_controller.rb:1187`).
    let can_read_roster = star_permissions::check::require_any(
        &state.db,
        auth.id,
        &star_permissions::PermissionContext::Course {
            course_id: section.course_id,
        },
        &["read_roster", "view_all_grades", "manage_grades"],
    )
    .await
    .is_ok();

    let rows: Vec<EnrollmentApiRow> = if can_read_roster {
        sqlx::query_as(&enrollment_query(
            "e.course_section_id = $1 AND e.workflow_state != 'deleted'",
        ))
        .bind(section_id)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::from)?
    } else {
        sqlx::query_as(&enrollment_query(
            "e.course_section_id = $1 AND e.user_id = $2 AND e.workflow_state != 'deleted'",
        ))
        .bind(section_id)
        .bind(auth.id)
        .fetch_all(&state.db)
        .await
        .map_err(ApiError::from)?
    };

    let course_ids: Vec<i64> = rows.iter().map(|row| row.course_id).collect();
    let rights = CallerRights::load(&state.db, auth.id, is_admin(&auth_role), &course_ids).await?;

    let payload: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| row.to_json(&rights.options(row, auth.id, true)))
        .collect();

    Ok(Json(payload))
}

/// POST /sections/{section_id}/enrollments — Canvas "Enroll a user" scoped to a
/// section. The course is derived from the section, so a section that does not
/// exist (or is not in a course) is a 404 and no cross-course enrollment is
/// possible.
async fn create_section_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(section_id): Path<i64>,
    ApiJson(payload): ApiJson<CreateSectionEnrollmentRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let section = load_section(&state.db, section_id).await?;

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

    let auth_role = get_user_role(&state.db, auth.id).await?;
    let auth_role = auth_role.unwrap_or_else(|| "student".to_string());

    let role = payload.role.as_deref().unwrap_or("student");
    validate_enrollment_role(role)?;

    // Canvas `User#can_create_enrollment_for?` -> the granular
    // `add_*_to_course` permission for the enrollment's role on the course
    // the section belongs to.
    require_course_permission(
        &state.db,
        auth.id,
        section.course_id,
        add_enrollment_permission(role),
    )
    .await?;

    if !user_exists(&state.db, user_id).await? {
        return Err(ApiError::NotFound);
    }

    let enrollment = insert_enrollment(
        &state.db,
        user_id,
        section.course_id,
        Some(section.id),
        role,
    )
    .await?;

    let rights = CallerRights::load(
        &state.db,
        auth.id,
        is_admin(&auth_role),
        &[enrollment.course_id],
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(port_enrollment_json(
            &enrollment,
            &rights.options(&enrollment, auth.id, false),
        )),
    ))
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
) -> Result<Json<serde_json::Value>, EnrollmentActionError> {
    let enrollment = load_course_enrollment(&state.db, course_id, enrollment_id).await?;

    let auth_role = get_user_role(&state.db, auth.id)
        .await?
        .unwrap_or_else(|| "student".to_string());

    // Canvas `EnrollmentsApiController#reactivate` checks
    // `Enrollment#can_be_deleted_by`
    // (`app/controllers/enrollments_api_controller.rb:1039-1045`): the granular
    // `remove_*` permission for the enrollment's own role, and an account
    // admin for anybody's enrollment.
    let enrollment_role: Option<(String,)> =
        sqlx::query_as("SELECT role FROM enrollments WHERE id = $1 AND course_id = $2")
            .bind(enrollment_id)
            .bind(course_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::from)?;
    let enrollment_role = enrollment_role.map(|(role,)| role).unwrap_or_default();

    if !star_permissions::check::can_delete_enrollment(
        &state.db,
        auth.id,
        course_id,
        enrollment.user_id,
        enrollment_role_kind(&enrollment_role),
    )
    .await?
    {
        return Err(ApiError::Forbidden.into());
    }

    if enrollment.workflow_state != "inactive" {
        return Err(EnrollmentActionError::Invalid(
            "enrollment not inactive".to_string(),
        ));
    }

    sqlx::query(
        "UPDATE enrollments SET workflow_state = 'active', updated_at = now() WHERE id = $1 AND course_id = $2",
    )
    .bind(enrollment_id)
    .bind(course_id)
    .execute(&state.db)
    .await
    .map_err(ApiError::from)?;

    // Canvas answers with the enrollment payload
    // (`render json: enrollment_json(@enrollment, ...)`), which carries the
    // nested user only on the index actions.
    let updated: EnrollmentApiRow = sqlx::query_as(&enrollment_query("e.id = $1"))
        .bind(enrollment_id)
        .fetch_one(&state.db)
        .await
        .map_err(ApiError::from)?;

    let rights = CallerRights::load(
        &state.db,
        auth.id,
        is_admin(&auth_role),
        &[updated.course_id],
    )
    .await?;

    Ok(Json(
        updated.to_json(&rights.options(&updated, auth.id, false)),
    ))
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
///
/// The answer is the Canvas enrollment payload of the index action.
async fn list_user_enrollments(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_ref): Path<String>,
    Query(filter): Query<UserEnrollmentFilter>,
) -> Result<Json<Vec<serde_json::Value>>, EnrollmentActionError> {
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

    let rows: Vec<EnrollmentApiRow> = sqlx::query_as(&enrollment_query(
        "e.user_id = $1 \
          AND e.workflow_state = ANY($2::text[]) \
          AND ($3::text IS NULL OR e.role = $3) \
          AND ($4::bigint IS NULL OR e.course_id = $4) \
          AND ($5::bigint[] IS NULL OR e.course_id = ANY($5::bigint[]))",
    ))
    .bind(target_user_id)
    .bind(&states)
    .bind(filter.role.as_deref())
    .bind(filter.course_id)
    .bind(teacher_course_ids)
    .fetch_all(&state.db)
    .await
    .map_err(ApiError::from)?;

    let course_ids: Vec<i64> = rows.iter().map(|row| row.course_id).collect();
    let rights = CallerRights::load(&state.db, auth.id, is_admin(&auth_role), &course_ids).await?;

    // The index action of `EnrollmentsApiController` renders every
    // enrollment with the nested user.
    let payload: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| row.to_json(&rights.options(row, auth.id, true)))
        .collect();

    Ok(Json(payload))
}

// --------------------------------------------------------------------------
// Temporary enrollment status (Canvas EnrollmentsApiController
// #show_temporary_enrollment_status / #bulk_temporary_enrollment_status)
// --------------------------------------------------------------------------

/// Compute one user's temporary enrollment status flags.
///
/// Canvas scopes these to `temporary_enrollment_scope`, which is restricted to
/// enrollments in courses of the account the flag check ran against (the
/// `temporary_enroll_scope` joins through the account's associated courses).
/// This schema has no course workflow state, so the `workflow_state = 'active'`
/// enrollment filter is the whole condition.
async fn temporary_enrollment_status(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
) -> Result<serde_json::Value, ApiError> {
    let (is_provider,): (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM enrollments e
            JOIN courses c ON c.id = e.course_id AND c.account_id = $2
            WHERE e.temporary_enrollment_source_user_id = $1
              AND e.workflow_state = 'active'
        )
        "#,
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)?;

    let (is_recipient,): (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM enrollments e
            JOIN courses c ON c.id = e.course_id AND c.account_id = $2
            WHERE e.user_id = $1
              AND e.temporary_enrollment_source_user_id IS NOT NULL
              AND e.workflow_state = 'active'
        )
        "#,
    )
    .bind(user_id)
    .bind(account_id)
    .fetch_one(pool)
    .await
    .map_err(ApiError::from)?;

    let (can_provide,): (bool,) = sqlx::query_as(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM enrollments e
            JOIN courses c ON c.id = e.course_id AND c.account_id = $2
            WHERE e.user_id = $1 AND e.workflow_state = 'active'
        )
        "#,
    )
    .bind(user_id)
    .bind(account_id)
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
/// feature flag is disabled for the domain root account (or the `account_id`
/// the request names), nothing is rendered — an empty 200 object here.
async fn show_temporary_enrollment_status(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(user_ref): Path<String>,
    Query(params): Query<TemporaryEnrollmentStatusParams>,
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
    // before anything else; when the flag is off nothing is rendered, which is
    // an empty 200 object here so the response can never be mistaken for a
    // valid status object.
    let account_id =
        match temporary_enrollment_account_or_none(&state.db, params.account_id).await? {
            Some(id) => id,
            None => return Ok(Json(json!({}))),
        };

    Ok(Json(
        temporary_enrollment_status(&state.db, target_user_id, account_id).await?,
    ))
}

/// GET /temporary_enrollment_status?user_ids=1,2 — statuses for many users.
///
/// Canvas omits users the caller cannot see (it filters on
/// `grants_right?(:api_show_user)`); here that means the caller themselves plus
/// every user when the caller is an admin. A missing `user_ids` is Canvas's
/// 400 with an empty JSON body. When the `temporary_enrollments` feature is
/// disabled on the account context (optional `account_id`, defaulting to the
/// domain root account), Canvas renders `render json: {}` — an empty 200.
async fn bulk_temporary_enrollment_status(
    State(state): State<AppState>,
    auth: AuthUser,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let (user_ids, raw_limit, account_id) = parse_bulk_enrollment_query(raw_query.as_deref());

    // Canvas: `return render json: {} unless feature_enabled?(:temporary_enrollments)`
    // comes before the user_ids guard, so the disabled case is a 200 `{}`.
    let account_id = match temporary_enrollment_account_or_none(&state.db, account_id).await {
        Ok(Some(id)) => id,
        Ok(None) => return Json(json!({})).into_response(),
        Err(e) => return e.into_response(),
    };

    // Canvas: `return render json: {}, status: :bad_request unless params[:user_ids].present?`.
    if user_ids.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(json!({}))).into_response();
    }

    // Canvas defaults to 10 and caps at 100 (Api::PER_PAGE / MAX_PER_PAGE).
    let limit = raw_limit.unwrap_or(10).clamp(1, 100);

    match bulk_temporary_enrollment_status_inner(&state, &auth, user_ids, limit, account_id).await {
        Ok(value) => Json(value).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn bulk_temporary_enrollment_status_inner(
    state: &AppState,
    auth: &AuthUser,
    mut user_ids: Vec<i64>,
    limit: i64,
    account_id: i64,
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
            temporary_enrollment_status(&state.db, user_id, account_id).await?,
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

    // Canvas runs `authorized_action(@context, ..., [:view_all_grades,
    // :manage_grades])` before anything else (enrollments_api_controller.rb
    // #last_attended), so course teachers/admins pass here first.
    star_permissions::check::require_any(
        &state.db,
        auth.id,
        &star_permissions::PermissionContext::Course { course_id },
        &["view_all_grades", "manage_grades"],
    )
    .await?;

    // Canvas parses `Time.zone.parse(params[:date])` next and renders
    // 400 { "message": "Invalid date time input" } before touching any
    // enrollment rows, so a bad date wins over a missing enrollment (404).
    let date = params
        .date
        .as_deref()
        .and_then(parse_last_attended_date)
        .ok_or(EnrollmentActionError::InvalidDate)?;

    // `Enrollment.where(course_id:, user_id:)` — a user with no enrollment in
    // this course is a 404 before the bulk update proceeds.
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

// --------------------------------------------------------------------------
// Canvas: GET /api/v1/accounts/:account_id/enrollments/:id (#show) and
// POST /api/v1/accounts/:account_id/bulk_enrollment (#bulk_enrollment)
// --------------------------------------------------------------------------

/// Minimal account row: id plus parent, enough to walk the account chain.
#[derive(Debug, Clone, sqlx::FromRow)]
struct AccountRef {
    id: i64,
    parent_account_id: Option<i64>,
}

/// Load an account. 404 when it does not exist, matching Canvas
/// `require_context` raising `ActiveRecord::RecordNotFound`.
async fn load_account(pool: &PgPool, account_id: i64) -> Result<AccountRef, ApiError> {
    sqlx::query_as("SELECT id, parent_account_id FROM accounts WHERE id = $1")
        .bind(account_id)
        .fetch_optional(pool)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)
}

/// Ids of `account_id` and every ancestor (Canvas `Account#account_chain_ids`).
///
/// Cycle-safe through `UNION` de-duplication, mirroring the recursive walks in
/// star-accounts.
async fn account_chain_ids(pool: &PgPool, account_id: i64) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        r#"
        WITH RECURSIVE chain AS (
            SELECT id, parent_account_id FROM accounts WHERE id = $1
          UNION
            SELECT a.id, a.parent_account_id
            FROM accounts a JOIN chain c ON a.id = c.parent_account_id
        )
        SELECT id FROM chain
        "#,
    )
    .bind(account_id)
    .fetch_all(pool)
    .await
    .map_err(ApiError::from)?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// GET /accounts/{accountId}/enrollments/{id} — Canvas
/// `EnrollmentsApiController#show`.
///
/// Canvas looks the enrollment up through the context account's root
/// (`all_enrollments`, which includes soft-deleted rows), then renders it when
/// the caller is the enrolled user or can `:read_roster` on the context. A
/// sub-account context additionally 404s unless the enrollment's course hangs
/// under it (`course.account_chain_ids.include?(@context.id)`).
///
/// The answer is the Canvas enrollment payload. The show action renders it
/// without the nested user (`render json: enrollment_json(enrollment, ...)`).
async fn show_account_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path((account_id, enrollment_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let account = load_account(&state.db, account_id).await?;

    // Canvas `all_enrollments.find(params[:id])`: by primary key, no
    // workflow_state filter — deleted enrollments are still visible here.
    let enrollment: EnrollmentApiRow = sqlx::query_as(&enrollment_query("e.id = $1"))
        .bind(enrollment_id)
        .fetch_optional(&state.db)
        .await
        .map_err(ApiError::from)?
        .ok_or(ApiError::NotFound)?;

    // Canvas: own enrollment renders without any roster right; others need
    // `:read_roster` on the context account (unauthorized -> 401).
    if enrollment.user_id != auth.id {
        // Canvas `EnrollmentsApiController#show` ->
        // `authorized_action(@context, @current_user, :read_roster)`
        // (`app/controllers/enrollments_api_controller.rb:566`).
        if !star_permissions::check::can(
            &state.db,
            auth.id,
            &star_permissions::PermissionContext::Account {
                account_id: account.id,
            },
            "read_roster",
        )
        .await?
        {
            return Err(ApiError::Unauthorized);
        }

        // Sub-account guard: the enrollment's course must hang under the
        // context account. The root account passes everything under it.
        if account.parent_account_id.is_some() {
            let course_account: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT account_id FROM courses WHERE id = $1")
                    .bind(enrollment.course_id)
                    .fetch_optional(&state.db)
                    .await
                    .map_err(ApiError::from)?;
            let course_chain = match course_account {
                Some((Some(ca),)) => account_chain_ids(&state.db, ca).await?,
                _ => Vec::new(),
            };
            if !course_chain.contains(&account.id) {
                return Err(ApiError::NotFound);
            }
        }
    }

    let auth_role = get_user_role(&state.db, auth.id)
        .await?
        .unwrap_or_else(|| "student".to_string());
    let rights = CallerRights::load(
        &state.db,
        auth.id,
        is_admin(&auth_role),
        &[enrollment.course_id],
    )
    .await?;

    Ok(Json(enrollment.to_json(&rights.options(
        &enrollment,
        auth.id,
        false,
    ))))
}

/// Body for POST /accounts/{accountId}/bulk_enrollment. Canvas takes
/// `user_ids[]`, `course_ids[]`, `enrollment_type`, `start_at` and `end_at`
/// (`enrollment_role_id` custom-role support is omitted with the rest of the
/// custom-role surface).
#[derive(Debug, Default, Deserialize)]
pub struct BulkEnrollmentRequest {
    #[serde(default)]
    pub user_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub course_ids: Option<Vec<i64>>,
    pub enrollment_type: Option<String>,
    pub start_at: Option<DateTime<Utc>>,
    pub end_at: Option<DateTime<Utc>>,
}

/// The enrollment types Canvas validates through `Enrollment.valid_type?`.
const BULK_ENROLLMENT_TYPES: [&str; 5] = [
    "StudentEnrollment",
    "TeacherEnrollment",
    "TaEnrollment",
    "ObserverEnrollment",
    "DesignerEnrollment",
];

/// POST /accounts/{accountId}/bulk_enrollment — Canvas
/// `EnrollmentsApiController#bulk_enrollment`.
///
/// Canvas validates the enrollment type, rejects user×course products above
/// 500 with 400 "Too many users to enroll at once.", checks
/// `can_create_enrollment_for?` on every target course (401 on the first
/// failure), then enqueues `Enrollment::BulkUpdate` and answers the Progress
/// JSON. Star has no progresses table, so the enrollments are inserted
/// synchronously and a completed Progress-shaped body is returned (the same
/// deviation star-assignments documents for `bulk_update_grades`).
async fn create_bulk_enrollment(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(account_id): Path<i64>,
    ApiJson(payload): ApiJson<BulkEnrollmentRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let account = load_account(&state.db, account_id).await?;

    let enrollment_type = payload
        .enrollment_type
        .as_deref()
        .unwrap_or("StudentEnrollment");
    if !BULK_ENROLLMENT_TYPES.contains(&enrollment_type) {
        return Err(ApiError::BadRequest("Invalid enrollment type.".into()));
    }

    // Canvas: `user_ids.size * course_ids.size > 500` -> 400 (empty lists
    // short-circuit the check, matching `!user_ids.empty? && ...`).
    // Canvas calls `user_ids.empty?` on the raw parameter and raises when the
    // request names no user ids at all; its exception handler answers the
    // generic 500 (app/controllers/enrollments_api_controller.rb:907).
    let Some(user_ids) = payload.user_ids.as_ref() else {
        return Err(ApiError::canvas_internal_error());
    };
    let course_ids = payload.course_ids.clone().unwrap_or_default();

    if !user_ids.is_empty() && !course_ids.is_empty() && user_ids.len() * course_ids.len() > 500 {
        return Err(ApiError::BadRequest(
            "Too many users to enroll at once.".into(),
        ));
    }

    // Canvas `api_find(Course, course_id)` per target: unknown course -> 404,
    // and `can_create_enrollment_for?` -> 401 for non teachers/admins.
    let auth_role = get_user_role(&state.db, auth.id)
        .await?
        .unwrap_or_else(|| "student".to_string());
    for course_id in &course_ids {
        let course: Option<(i64,)> = sqlx::query_as("SELECT id FROM courses WHERE id = $1")
            .bind(course_id)
            .fetch_optional(&state.db)
            .await
            .map_err(ApiError::from)?;
        if course.is_none() {
            return Err(ApiError::NotFound);
        }
        if !is_admin(&auth_role)
            && !is_teacher_in_course(&state.db, auth.id, *course_id, &auth_role).await?
        {
            return Err(ApiError::Unauthorized);
        }
    }

    // `Enrollment::BulkUpdate` creates one enrollment per (user, course) pair,
    // keeping existing ones (`allow_multiple_enrollments` + upsert semantics:
    // the same pair is not created twice, matching the UNIQUE constraint).
    for course_id in &course_ids {
        for user_id in user_ids {
            sqlx::query(
                r#"
                INSERT INTO enrollments
                    (user_id, course_id, role, workflow_state, start_at, end_at)
                VALUES ($1, $2, $3, 'active', $4, $5)
                ON CONFLICT (user_id, course_id, role) DO NOTHING
                "#,
            )
            .bind(user_id)
            .bind(course_id)
            .bind(enrollment_role_column(enrollment_type))
            .bind(payload.start_at)
            .bind(payload.end_at)
            .execute(&state.db)
            .await
            .map_err(ApiError::from)?;
        }
    }

    // Progress-shaped response (synchronous deviation, see doc comment).
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": 0,
            "context_id": account.id,
            "context_type": "Account",
            "user_id": auth.id,
            "tag": "bulk_enrollment",
            "completion": 100,
            "workflow_state": "completed",
            "created_at": Utc::now(),
            "updated_at": Utc::now(),
            "url": format!("/api/v1/accounts/{}/enrollments/bulk_enrollment", account.id),
        })),
    ))
}

/// Map a Canvas enrollment type to the star `enrollments.role` value used by
/// the rest of the crate (`create_enrollment`'s default is "student").
fn enrollment_role_column(enrollment_type: &str) -> &'static str {
    match enrollment_type {
        "TeacherEnrollment" => "teacher",
        "TaEnrollment" => "ta",
        "ObserverEnrollment" => "observer",
        "DesignerEnrollment" => "designer",
        _ => "student",
    }
}
