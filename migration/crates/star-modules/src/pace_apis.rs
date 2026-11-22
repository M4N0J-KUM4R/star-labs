//! course_pacing — Canvas `CoursePacing::SectionPacesApiController` and
//! `CoursePacing::StudentEnrollmentPacesApiController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET    /courses/{courseId}/sections/{courseSectionId}/pace`
//! - `POST   /courses/{courseId}/sections/{courseSectionId}/paces`
//! - `PATCH  /courses/{courseId}/sections/{courseSectionId}/pace`
//! - `DELETE /courses/{courseId}/sections/{courseSectionId}/pace`
//! - `GET    /courses/{courseId}/student_enrollments/{studentEnrollmentId}/pace`
//! - `POST   /courses/{courseId}/student_enrollments/{studentEnrollmentId}/paces`
//! - `PATCH  /courses/{courseId}/student_enrollments/{studentEnrollmentId}/pace`
//! - `DELETE /courses/{courseId}/student_enrollments/{studentEnrollmentId}/pace`
//!
//! Canvas routes (config/routes.rb, the course API block):
//!   get    "courses/:course_id/sections/:course_section_id/pace"
//!   post   "courses/:course_id/sections/:course_section_id/paces"
//!   patch  "courses/:course_id/sections/:course_section_id/pace"
//!   delete "courses/:course_id/sections/:course_section_id/pace"
//!   get    "courses/:course_id/student_enrollments/:student_enrollment_id/pace"
//!   post   "courses/:course_id/student_enrollments/:student_enrollment_id/paces"
//!   patch  "courses/:course_id/student_enrollments/:student_enrollment_id/pace"
//!   delete "courses/:course_id/student_enrollments/:student_enrollment_id/pace"
//!
//! Both controllers inherit `CoursePacing::PacesApiController`
//! (`app/controllers/course_pacing/paces_api_controller.rb`), whose
//! `before_action` chain is `load_contexts` (the course, then the section or
//! the student enrollment — 404 when it does not belong to the course) and
//! `authorize_action` (the `manage_course_content_edit` right). The
//! `load_contexts` of the two controllers also reads the root account
//! feature flag `course_pace_draft_state`.
//!
//! The shared actions:
//! - `show` reads the pace of the context (`pace_for(context,
//!   should_duplicate: true)`), 404 when there is none, and answers
//!   `{ "pace": …, "progress": … }`;
//! - `create` creates the pace of the context, 404 when the context is
//!   invalid, and answers 201 with `{ "pace": …, "progress": … }`;
//! - `update` updates the pace of the context, 404 when there is none, and
//!   answers `{ "pace": …, "progress": … }` or 422 with the errors;
//! - `delete` removes the pace of the context, 404 when there is none, and
//!   answers 204.
//!
//! The presenters are `CoursePacing::SectionPacePresenter` (the pace of a
//! section, with a `section` member) and
//! `CoursePacing::StudentEnrollmentPacePresenter` (the pace of a student
//! enrollment, with a `student` member).

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::course_pacing::{
    course_paces_enabled, load_course, require_content_right, require_feature_flag,
    root_account_id, ContentRight, CourseRow,
};
use crate::pace_contexts::overdue_user_ids;

/// The context a pace belongs to.
#[derive(Debug, Clone, Copy)]
enum PaceScope {
    /// A section of the course.
    Section(i64),
    /// A student enrollment of the course.
    Enrollment(i64),
}

/// The row of a `course_paces` row.
#[derive(Debug, sqlx::FromRow)]
struct PaceRow {
    id: i64,
    course_id: i64,
    course_section_id: Option<i64>,
    user_id: Option<i64>,
    root_account_id: Option<i64>,
    workflow_state: String,
    exclude_weekends: bool,
    selected_days_to_skip: Vec<String>,
    hard_end_dates: bool,
    end_date: Option<chrono::DateTime<chrono::Utc>>,
    published_at: Option<chrono::DateTime<chrono::Utc>>,
    assignments_weighting: String,
    time_to_complete_calendar_days: i32,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

/// The columns of a `course_paces` row.
const PACE_COLUMNS: &str = "id, course_id, course_section_id, user_id, root_account_id, \
     workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates, end_date, \
     published_at, assignments_weighting, time_to_complete_calendar_days, created_at, updated_at";

/// Domain router for the Canvas section and student-enrollment pace routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/sections/{courseSectionId}/pace",
            get(show_section_pace)
                .patch(update_section_pace)
                .delete(delete_section_pace),
        )
        .route(
            "/courses/{courseId}/sections/{courseSectionId}/paces",
            post(create_section_pace),
        )
        .route(
            "/courses/{courseId}/student_enrollments/{studentEnrollmentId}/pace",
            get(show_enrollment_pace)
                .patch(update_enrollment_pace)
                .delete(delete_enrollment_pace),
        )
        .route(
            "/courses/{courseId}/student_enrollments/{studentEnrollmentId}/paces",
            post(create_enrollment_pace),
        )
        .route(
            "/courses/{courseId}/bulk_student_enrollments/student_bulk_pace_edit_view",
            get(student_bulk_pace_edit_view),
        )
}

/// The query parameters of the bulk pace edit view.
#[derive(Debug, Default, Deserialize)]
struct BulkViewQuery {
    page: Option<i64>,
    per_page: Option<i64>,
    filter_section: Option<String>,
    filter_pace_status: Option<String>,
    search_term: Option<String>,
    sort: Option<String>,
    order: Option<String>,
}

/// Canvas `CoursePacing::BulkStudentEnrollmentPacesApiController#student_bulk_pace_edit_view`.
async fn student_bulk_pace_edit_view(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<BulkViewQuery>,
) -> Result<Response, ApiError> {
    let course = load_course(&state.db, course_id).await?;
    require_feature_flag(&state.db, course.id).await?;
    require_content_right(&state.db, &auth, course.id, ContentRight::Edit).await?;

    let per_page = params.per_page.filter(|value| *value > 0).unwrap_or(10);
    let page = params.page.filter(|value| *value > 0).unwrap_or(1);
    let offset = (page - 1) * per_page;

    // The active students of the course without an individual pace.
    let mut sql = String::from(
        "SELECT e.id, e.user_id, e.course_section_id, u.name, e.created_at \
         FROM enrollments e JOIN users u ON u.id = e.user_id \
         WHERE e.course_id = $1 AND e.workflow_state = 'active'",
    );
    if params
        .filter_section
        .as_deref()
        .is_some_and(|value| !value.is_empty())
    {
        sql.push_str(" AND e.course_section_id = $2");
    }
    if let Some(search_term) = params
        .search_term
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        sql.push_str(" AND u.name ILIKE '");
        sql.push_str(&search_term.replace('\'', "''"));
        sql.push_str("%'");
    }
    // Canvas excludes the students that already have an individual pace.
    sql.push_str(
        " AND NOT EXISTS (SELECT 1 FROM course_paces cp \
         WHERE cp.course_id = e.course_id AND cp.user_id = e.user_id \
           AND cp.workflow_state = 'active')",
    );
    sql.push_str(" ORDER BY e.user_id, e.created_at DESC");

    let mut students: Vec<PaceStudent> = sqlx::query_as(&sql)
        .bind(course_id)
        .fetch_all(&state.db)
        .await?;

    // The students that are off pace.
    let overdue = overdue_user_ids(&state.db, course.id).await?;
    if let Some(status) = params
        .filter_pace_status
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        if status == "on-pace" {
            students.retain(|row| !overdue.contains(&row.user_id));
        } else {
            students.retain(|row| overdue.contains(&row.user_id));
        }
    }

    // Canvas sorts by the name of the user when `sort` is present.
    if params.sort.is_some() {
        students.sort_by(|left, right| left.name.cmp(&right.name));
        if params.order.as_deref() == Some("desc") {
            students.reverse();
        }
    }

    let total_students = students.len() as i64;
    let page_count = if per_page > 0 {
        (total_students as f64 / per_page as f64).ceil() as i64
    } else {
        0
    };

    // The sections of the course.
    let sections: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT id, course_id, name FROM course_sections \
         WHERE course_id = $1 AND workflow_state = 'active' ORDER BY name ASC",
    )
    .bind(course_id)
    .fetch_all(&state.db)
    .await?;

    // The students of the page.
    let paged = students
        .iter()
        .skip(offset as usize)
        .take(per_page as usize)
        .map(|row| {
            let enrollment_id = row.enrollment_id;
            let user_id = row.user_id;
            let name = row.name.clone();
            let created_at = row.created_at;
            let student_sections: Vec<Value> = sections
                .iter()
                .filter(|section| Some(section.0) == row.course_section_id)
                .map(|section| {
                    json!({
                        "id": section.0.to_string(),
                        "course_id": section.1.to_string(),
                        "name": section.2,
                    })
                })
                .collect();
            json!({
                "id": user_id.to_string(),
                "name": name,
                "paceStatus": if overdue.contains(&user_id) { "off-pace" } else { "on-pace" },
                "enrollmentId": enrollment_id.to_string(),
                "enrollmentDate": created_at.to_rfc3339(),
                "sections": student_sections,
            })
        })
        .collect::<Vec<Value>>();

    Ok(Json(json!({
        "students": paged,
        "pages": page_count,
        "sections": sections
            .iter()
            .map(|section| json!({
                "id": section.0.to_string(),
                "course_id": section.1.to_string(),
                "name": section.2,
            }))
            .collect::<Vec<Value>>(),
    }))
    .into_response())
}

/// Canvas `CoursePacing::SectionPacesApiController#show`.
async fn show_section_pace(
    State(state): State<AppState>,
    Path((course_id, section_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course = load_scope(&state.db, course_id, PaceScope::Section(section_id)).await?;
    authorize(&state.db, &auth, course.id).await?;
    let pace = pace_for(&state.db, course.id, PaceScope::Section(section_id)).await?;
    let Some(pace) = pace else {
        return Err(ApiError::NotFound);
    };
    Ok(Json(json!({
        "pace": section_pace_json(&state.db, &pace).await?,
        "progress": Value::Null,
    }))
    .into_response())
}

/// Canvas `CoursePacing::SectionPacesApiController#create`.
async fn create_section_pace(
    State(state): State<AppState>,
    Path((course_id, section_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course = load_scope(&state.db, course_id, PaceScope::Section(section_id)).await?;
    authorize(&state.db, &auth, course.id).await?;
    let pace = create_in_context(&state.db, &course, PaceScope::Section(section_id)).await?;
    let Some(pace) = pace else {
        return Err(ApiError::NotFound);
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "pace": section_pace_json(&state.db, &pace).await?,
            "progress": Value::Null,
        })),
    )
        .into_response())
}

/// Canvas `CoursePacing::SectionPacesApiController#update`.
async fn update_section_pace(
    State(state): State<AppState>,
    Path((course_id, section_id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let course = load_scope(&state.db, course_id, PaceScope::Section(section_id)).await?;
    authorize(&state.db, &auth, course.id).await?;
    let pace = pace_in_context(&state.db, course.id, PaceScope::Section(section_id)).await?;
    let Some(pace) = pace else {
        return Err(ApiError::NotFound);
    };
    Ok(Json(json!({
        "pace": update_pace(&state.db, &pace, body.as_ref().map(|Json(value)| value)).await?,
        "progress": Value::Null,
    }))
    .into_response())
}

/// Canvas `CoursePacing::SectionPacesApiController#delete`.
async fn delete_section_pace(
    State(state): State<AppState>,
    Path((course_id, section_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course = load_scope(&state.db, course_id, PaceScope::Section(section_id)).await?;
    authorize(&state.db, &auth, course.id).await?;
    let pace = pace_in_context(&state.db, course.id, PaceScope::Section(section_id)).await?;
    let Some(pace) = pace else {
        return Err(ApiError::NotFound);
    };
    delete_pace(&state.db, pace.id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Canvas `CoursePacing::StudentEnrollmentPacesApiController#show`.
async fn show_enrollment_pace(
    State(state): State<AppState>,
    Path((course_id, enrollment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course = load_scope(&state.db, course_id, PaceScope::Enrollment(enrollment_id)).await?;
    authorize(&state.db, &auth, course.id).await?;
    let pace = pace_for(&state.db, course.id, PaceScope::Enrollment(enrollment_id)).await?;
    let Some(pace) = pace else {
        return Err(ApiError::NotFound);
    };
    Ok(Json(json!({
        "pace": enrollment_pace_json(&state.db, &pace, enrollment_id).await?,
        "progress": Value::Null,
    }))
    .into_response())
}

/// Canvas `CoursePacing::StudentEnrollmentPacesApiController#create`.
async fn create_enrollment_pace(
    State(state): State<AppState>,
    Path((course_id, enrollment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course = load_scope(&state.db, course_id, PaceScope::Enrollment(enrollment_id)).await?;
    authorize(&state.db, &auth, course.id).await?;
    let pace = create_in_context(&state.db, &course, PaceScope::Enrollment(enrollment_id)).await?;
    let Some(pace) = pace else {
        return Err(ApiError::NotFound);
    };
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "pace": enrollment_pace_json(&state.db, &pace, enrollment_id).await?,
            "progress": Value::Null,
        })),
    )
        .into_response())
}

/// Canvas `CoursePacing::StudentEnrollmentPacesApiController#update`.
async fn update_enrollment_pace(
    State(state): State<AppState>,
    Path((course_id, enrollment_id)): Path<(i64, i64)>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<Response, ApiError> {
    let course = load_scope(&state.db, course_id, PaceScope::Enrollment(enrollment_id)).await?;
    authorize(&state.db, &auth, course.id).await?;
    let pace = pace_in_context(&state.db, course.id, PaceScope::Enrollment(enrollment_id)).await?;
    let Some(pace) = pace else {
        return Err(ApiError::NotFound);
    };
    Ok(Json(json!({
        "pace": update_pace(&state.db, &pace, body.as_ref().map(|Json(value)| value)).await?,
        "progress": Value::Null,
    }))
    .into_response())
}

/// Canvas `CoursePacing::StudentEnrollmentPacesApiController#delete`.
async fn delete_enrollment_pace(
    State(state): State<AppState>,
    Path((course_id, enrollment_id)): Path<(i64, i64)>,
    auth: AuthUser,
) -> Result<Response, ApiError> {
    let course = load_scope(&state.db, course_id, PaceScope::Enrollment(enrollment_id)).await?;
    authorize(&state.db, &auth, course.id).await?;
    let pace = pace_in_context(&state.db, course.id, PaceScope::Enrollment(enrollment_id)).await?;
    let Some(pace) = pace else {
        return Err(ApiError::NotFound);
    };
    delete_pace(&state.db, pace.id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Canvas `load_contexts` of the two controllers: the course, then the
/// context — 404 when it does not belong to the course.
async fn load_scope(db: &PgPool, course_id: i64, scope: PaceScope) -> Result<CourseRow, ApiError> {
    let course = load_course(db, course_id).await?;
    match scope {
        PaceScope::Section(section_id) => {
            let row: Option<(i64,)> = sqlx::query_as(
                "SELECT course_id FROM course_sections \
                 WHERE id = $1 AND workflow_state <> 'deleted'",
            )
            .bind(section_id)
            .fetch_optional(db)
            .await?;
            match row {
                Some((section_course_id,)) if section_course_id == course_id => Ok(course),
                _ => Err(ApiError::NotFound),
            }
        }
        PaceScope::Enrollment(enrollment_id) => {
            let row: Option<(i64,)> =
                sqlx::query_as("SELECT course_id FROM enrollments WHERE id = $1")
                    .bind(enrollment_id)
                    .fetch_optional(db)
                    .await?;
            match row {
                Some((enrollment_course_id,)) if enrollment_course_id == course_id => Ok(course),
                _ => Err(ApiError::NotFound),
            }
        }
    }
}

/// Canvas `authorize_action`: the `manage_course_content_edit` right, and
/// the course pacing feature flag.
async fn authorize(db: &PgPool, auth: &AuthUser, course_id: i64) -> Result<(), ApiError> {
    require_feature_flag(db, course_id).await?;
    require_content_right(db, auth, course_id, ContentRight::Edit).await
}

/// Canvas `CoursePacing::PaceService#pace_for(context,
/// should_duplicate: true)`.
///
/// The pace of the context itself, otherwise a copy of the template pace of
/// the context (the primary pace of the course for a section, the pace of
/// the section for a student enrollment).
async fn pace_for(
    db: &PgPool,
    course_id: i64,
    scope: PaceScope,
) -> Result<Option<PaceRow>, ApiError> {
    if let Some(pace) = pace_in_context(db, course_id, scope).await? {
        return Ok(Some(pace));
    }
    let template = template_pace_for(db, course_id, scope).await?;
    let Some(template) = template else {
        return Ok(None);
    };
    // Canvas duplicates the template (the copy is not saved).
    Ok(Some(PaceRow {
        id: 0,
        course_id: template.course_id,
        course_section_id: match scope {
            PaceScope::Section(section_id) => Some(section_id),
            PaceScope::Enrollment(_) => None,
        },
        user_id: match scope {
            PaceScope::Section(_) => None,
            PaceScope::Enrollment(enrollment_id) => enrollment_user_id(db, enrollment_id).await?,
        },
        root_account_id: template.root_account_id,
        workflow_state: "unpublished".to_string(),
        exclude_weekends: template.exclude_weekends,
        selected_days_to_skip: template.selected_days_to_skip.clone(),
        hard_end_dates: template.hard_end_dates,
        end_date: template.end_date,
        published_at: None,
        assignments_weighting: template.assignments_weighting.clone(),
        time_to_complete_calendar_days: template.time_to_complete_calendar_days,
        created_at: template.created_at,
        updated_at: template.updated_at,
    }))
}

/// Canvas `CoursePacing::PaceService#pace_in_context`.
async fn pace_in_context(
    db: &PgPool,
    course_id: i64,
    scope: PaceScope,
) -> Result<Option<PaceRow>, ApiError> {
    let sql = match scope {
        PaceScope::Section(_) => format!(
            "SELECT {PACE_COLUMNS} FROM course_paces \
             WHERE course_id = $1 AND course_section_id = $2 AND workflow_state <> 'deleted' \
             ORDER BY id ASC LIMIT 1"
        ),
        PaceScope::Enrollment(_) => format!(
            "SELECT {PACE_COLUMNS} FROM course_paces \
             WHERE course_id = $1 AND user_id = $2 AND workflow_state <> 'deleted' \
             ORDER BY id ASC LIMIT 1"
        ),
    };
    let key = match scope {
        PaceScope::Section(section_id) => section_id,
        // The pace of a student enrollment is stored with the id of the user.
        PaceScope::Enrollment(enrollment_id) => enrollment_user_id(db, enrollment_id)
            .await?
            .unwrap_or_default(),
    };
    let row: Option<PaceRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(key)
        .fetch_optional(db)
        .await?;
    Ok(row)
}

/// Canvas `CoursePacing::PaceService#template_pace_for`.
async fn template_pace_for(
    db: &PgPool,
    course_id: i64,
    scope: PaceScope,
) -> Result<Option<PaceRow>, ApiError> {
    match scope {
        PaceScope::Section(_) => primary_pace(db, course_id).await,
        PaceScope::Enrollment(enrollment_id) => {
            let section_id = enrollment_section_id(db, enrollment_id).await?;
            match section_id {
                Some(section_id) => {
                    // The pace of the section, otherwise the primary pace of
                    // the course.
                    let section_pace: Option<PaceRow> = sqlx::query_as(&format!(
                        "SELECT {PACE_COLUMNS} FROM course_paces \
                         WHERE course_id = $1 AND course_section_id = $2 \
                           AND workflow_state <> 'deleted' ORDER BY id ASC LIMIT 1"
                    ))
                    .bind(course_id)
                    .bind(section_id)
                    .fetch_optional(db)
                    .await?;
                    match section_pace {
                        Some(pace) => Ok(Some(pace)),
                        None => primary_pace(db, course_id).await,
                    }
                }
                None => primary_pace(db, course_id).await,
            }
        }
    }
}

/// The primary pace of the course (`course_paces.primary.take`).
async fn primary_pace(db: &PgPool, course_id: i64) -> Result<Option<PaceRow>, ApiError> {
    let row: Option<PaceRow> = sqlx::query_as(&format!(
        "SELECT {PACE_COLUMNS} FROM course_paces \
         WHERE course_id = $1 AND course_section_id IS NULL AND user_id IS NULL \
           AND workflow_state <> 'deleted' ORDER BY id ASC LIMIT 1"
    ))
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// Canvas `CoursePacing::PaceService#create_in_context`.
async fn create_in_context(
    db: &PgPool,
    course: &CourseRow,
    scope: PaceScope,
) -> Result<Option<PaceRow>, ApiError> {
    if let Some(pace) = pace_in_context(db, course.id, scope).await? {
        return Ok(Some(pace));
    }
    let template = template_pace_for(db, course.id, scope).await?;
    let (course_section_id, user_id) = match scope {
        PaceScope::Section(section_id) => (Some(section_id), None),
        PaceScope::Enrollment(enrollment_id) => {
            (None, enrollment_user_id(db, enrollment_id).await?)
        }
    };
    // Canvas falls back to the course's root account when no template
    // pace exists yet (an enrollment scope can run before any pace of the
    // course exists).
    let root_account_id = match template.as_ref().and_then(|pace| pace.root_account_id) {
        Some(root_account_id) => Some(root_account_id),
        None => root_account_id(db, course).await,
    };
    let workflow_state = "unpublished";
    let exclude_weekends = template
        .as_ref()
        .map(|pace| pace.exclude_weekends)
        .unwrap_or(true);
    let selected_days_to_skip = template
        .as_ref()
        .map(|pace| pace.selected_days_to_skip.clone())
        .unwrap_or_else(|| vec!["sat".to_string(), "sun".to_string()]);
    let hard_end_dates = template
        .as_ref()
        .map(|pace| pace.hard_end_dates)
        .unwrap_or(false);
    let assignments_weighting = template
        .as_ref()
        .map(|pace| pace.assignments_weighting.clone())
        .unwrap_or_else(|| "[]".to_string());

    let pace_id: i64 = sqlx::query_scalar(
        "INSERT INTO course_paces (course_id, course_section_id, user_id, root_account_id, \
             workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates, end_date, \
             assignments_weighting, time_to_complete_calendar_days) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9, $10) RETURNING id",
    )
    .bind(course.id)
    .bind(course_section_id)
    .bind(user_id)
    .bind(root_account_id)
    .bind(workflow_state)
    .bind(exclude_weekends)
    .bind(&selected_days_to_skip)
    .bind(hard_end_dates)
    .bind(&assignments_weighting)
    .bind(0)
    .fetch_one(db)
    .await?;

    // The copy carries the module items of the template.
    if let Some(template) = template {
        sqlx::query(
            "INSERT INTO course_pace_module_items \
                 (course_pace_id, module_item_id, root_account_id, duration) \
             SELECT $1, module_item_id, root_account_id, duration \
             FROM course_pace_module_items WHERE course_pace_id = $2",
        )
        .bind(pace_id)
        .bind(template.id)
        .execute(db)
        .await?;
    }

    let sql = format!("SELECT {PACE_COLUMNS} FROM course_paces WHERE id = $1");
    let row: Option<PaceRow> = sqlx::query_as(&sql)
        .bind(pace_id)
        .fetch_optional(db)
        .await?;
    Ok(row)
}

/// Canvas `CoursePacing::PaceService#update_pace`.
async fn update_pace(db: &PgPool, pace: &PaceRow, body: Option<&Value>) -> Result<Value, ApiError> {
    let Some(body) = body else {
        return Err(ApiError::BadRequest(
            "param is missing or the value is empty: pace".to_string(),
        ));
    };
    let params = body.get("pace").cloned().unwrap_or(Value::Null);
    if !params.is_object() {
        return Err(ApiError::BadRequest(
            "param is missing or the value is empty: pace".to_string(),
        ));
    }

    let end_date = params
        .get("end_date")
        .and_then(|value| value.as_str())
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&chrono::Utc));
    let hard_end_dates = params
        .get("hard_end_dates")
        .and_then(|value| value.as_bool())
        .unwrap_or(pace.hard_end_dates);
    let exclude_weekends = params
        .get("exclude_weekends")
        .and_then(|value| value.as_bool())
        .unwrap_or(pace.exclude_weekends);
    let workflow_state = params
        .get("workflow_state")
        .and_then(|value| value.as_str())
        .unwrap_or(&pace.workflow_state)
        .to_string();
    let selected_days_to_skip = params
        .get("selected_days_to_skip")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(|value| value.to_string()))
                .collect::<Vec<String>>()
        })
        .unwrap_or_else(|| pace.selected_days_to_skip.clone());

    sqlx::query(
        "UPDATE course_paces SET end_date = $1, hard_end_dates = $2, exclude_weekends = $3, \
             workflow_state = $4, selected_days_to_skip = $5, updated_at = now() \
         WHERE id = $6",
    )
    .bind(end_date)
    .bind(hard_end_dates)
    .bind(exclude_weekends)
    .bind(&workflow_state)
    .bind(&selected_days_to_skip)
    .bind(pace.id)
    .execute(db)
    .await?;

    let sql = format!("SELECT {PACE_COLUMNS} FROM course_paces WHERE id = $1");
    let row: Option<PaceRow> = sqlx::query_as(&sql)
        .bind(pace.id)
        .fetch_optional(db)
        .await?;
    let Some(row) = row else {
        return Err(ApiError::NotFound);
    };
    pace_json(db, &row, scope_of(&row)).await
}

/// Canvas `CoursePacing::PaceService#delete_in_context`.
async fn delete_pace(db: &PgPool, pace_id: i64) -> Result<(), ApiError> {
    sqlx::query("DELETE FROM course_paces WHERE id = $1")
        .bind(pace_id)
        .execute(db)
        .await?;
    Ok(())
}

/// The scope a pace belongs to.
fn scope_of(pace: &PaceRow) -> PaceScope {
    match pace.user_id {
        Some(user_id) => PaceScope::Enrollment(user_id),
        None => PaceScope::Section(pace.course_section_id.unwrap_or_default()),
    }
}

/// The user of a student enrollment.
async fn enrollment_user_id(db: &PgPool, enrollment_id: i64) -> Result<Option<i64>, ApiError> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM enrollments WHERE id = $1")
        .bind(enrollment_id)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|row| row.0))
}

/// The section of a student enrollment.
async fn enrollment_section_id(db: &PgPool, enrollment_id: i64) -> Result<Option<i64>, ApiError> {
    let row: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT course_section_id FROM enrollments WHERE id = $1")
            .bind(enrollment_id)
            .fetch_optional(db)
            .await?;
    Ok(row.and_then(|row| row.0))
}

/// Canvas `CoursePacing::SectionPacePresenter#as_json`.
async fn section_pace_json(db: &PgPool, pace: &PaceRow) -> Result<Value, ApiError> {
    let mut json = pace_json(
        db,
        pace,
        PaceScope::Section(pace.course_section_id.unwrap_or_default()),
    )
    .await?;
    let section_id = pace.course_section_id;
    let (name, size) = match section_id {
        Some(section_id) => {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT name FROM course_sections WHERE id = $1")
                    .bind(section_id)
                    .fetch_optional(db)
                    .await?;
            let name = row.map(|row| row.0).unwrap_or_default();
            let size: (i64,) = sqlx::query_as(
                "SELECT COUNT(*) FROM enrollments \
                 WHERE course_section_id = $1 AND role = 'student' AND workflow_state = 'active'",
            )
            .bind(section_id)
            .fetch_one(db)
            .await?;
            (name, size.0)
        }
        None => (String::new(), 0),
    };
    if let Some(object) = json.as_object_mut() {
        object.insert("section".to_string(), json!({ "name": name, "size": size }));
    }
    Ok(json)
}

/// Canvas `CoursePacing::StudentEnrollmentPacePresenter#as_json`.
async fn enrollment_pace_json(
    db: &PgPool,
    pace: &PaceRow,
    enrollment_id: i64,
) -> Result<Value, ApiError> {
    let mut json = pace_json(db, pace, PaceScope::Enrollment(enrollment_id)).await?;
    let name = match pace.user_id {
        Some(user_id) => {
            let row: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
                .bind(user_id)
                .fetch_optional(db)
                .await?;
            row.map(|row| row.0).unwrap_or_default()
        }
        None => String::new(),
    };
    if let Some(object) = json.as_object_mut() {
        object.insert("student".to_string(), json!({ "name": name }));
    }
    Ok(json)
}

/// Canvas `CoursePacing::PacePresenter#default_json` — the shared shape of
/// the section and the student-enrollment presenters.
async fn pace_json(db: &PgPool, pace: &PaceRow, scope: PaceScope) -> Result<Value, ApiError> {
    let modules = modules_json(db, pace.id, pace.course_id).await?;
    let (context_id, context_type) = match scope {
        PaceScope::Section(section_id) => (section_id, "Section"),
        PaceScope::Enrollment(enrollment_id) => (enrollment_id, "StudentEnrollment"),
    };
    let (start_date, start_date_context) = start_dates(db, pace, scope).await?;
    let (end_date, end_date_context) = end_dates(db, pace, scope).await?;

    Ok(json!({
        "id": pace.id,
        "workflow_state": pace.workflow_state,
        "exclude_weekends": pace.exclude_weekends,
        "hard_end_dates": pace.hard_end_dates,
        "created_at": pace.created_at.to_rfc3339(),
        "updated_at": pace.updated_at.to_rfc3339(),
        "published_at": pace.published_at.map(|value| value.to_rfc3339()),
        "root_account_id": pace.root_account_id,
        "modules": modules,
        "context_id": context_id,
        "context_type": context_type,
        "start_date": start_date,
        "start_date_context": start_date_context,
        "end_date": end_date,
        "end_date_context": end_date_context,
    }))
}

/// One student of the bulk pace view.
#[derive(Debug, sqlx::FromRow)]
struct PaceStudent {
    enrollment_id: i64,
    user_id: i64,
    course_section_id: Option<i64>,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

/// One row of the module items of a pace.
#[derive(Debug, sqlx::FromRow)]
struct PaceItemRow {
    module_id: i64,
    module_position: i32,
    module_name: String,
    duration: i32,
    item_id: i64,
    position: i32,
    item_type: String,
    assignment_id: i64,
    title: Option<String>,
    points_possible: Option<f64>,
}

/// Canvas `CoursePacing::PacePresenter#modules_json`.
async fn modules_json(db: &PgPool, pace_id: i64, course_id: i64) -> Result<Vec<Value>, ApiError> {
    if pace_id == 0 {
        return Ok(Vec::new());
    }
    // The module item of a pace points at a module item; the assignment it
    // carries is the `item_id` of an assignment module item.
    let rows: Vec<PaceItemRow> = sqlx::query_as(
        "SELECT m.id, m.position, m.name, cp.duration, mi.id, mi.position, mi.item_type, \
                    mi.item_id, a.title, a.points_possible::float8 AS points_possible \
             FROM course_pace_module_items cp \
             JOIN module_items mi ON mi.id = cp.module_item_id \
             JOIN modules m ON m.id = mi.module_id \
             LEFT JOIN assignments a ON mi.item_type = 'assignment' AND a.id = mi.item_id \
             WHERE cp.course_pace_id = $1 \
             ORDER BY m.position ASC, mi.position ASC",
    )
    .bind(pace_id)
    .fetch_all(db)
    .await?;

    let mut modules: Vec<(i64, String, i32, Vec<Value>)> = Vec::new();
    for row in rows {
        let item = json!({
            "id": row.item_id,
            "duration": row.duration,
            "course_pace_id": pace_id,
            "root_account_id": Value::Null,
            "module_item_id": row.item_id,
            "assignment_title": row.title,
            "assignment_id": row.assignment_id,
            "points_possible": round_if_whole(row.points_possible),
            "assignment_link": format!("/courses/{course_id}/modules/items/{}", row.item_id),
            "position": row.position,
            "module_item_type": canvas_content_type(&row.item_type),
            "published": true,
        });
        match modules.iter_mut().find(|entry| entry.0 == row.module_id) {
            Some(entry) => entry.3.push(item),
            None => modules.push((
                row.module_id,
                row.module_name,
                row.module_position,
                vec![item],
            )),
        }
    }

    Ok(modules
        .into_iter()
        .map(|(id, name, position, items)| {
            json!({ "id": id, "name": name, "position": position, "items": items })
        })
        .collect())
}

/// Canvas `TextHelper.round_if_whole`.
fn round_if_whole(value: Option<f64>) -> Value {
    match value {
        None => Value::Null,
        Some(value) if value.fract() == 0.0 => json!(value.round() as i64),
        Some(value) => json!(value),
    }
}

/// Canvas `ContentTag#content_type` of a module item.
fn canvas_content_type(item_type: &str) -> &'static str {
    match item_type {
        "page" | "wiki_page" => "WikiPage",
        "assignment" => "Assignment",
        "quiz" => "Quiz",
        "discussion" => "Discussion",
        "external_url" => "ExternalUrl",
        "file" => "File",
        _ => "Assignment",
    }
}

/// Canvas `CoursePace#start_date(with_context: true)`.
async fn start_dates(
    db: &PgPool,
    pace: &PaceRow,
    scope: PaceScope,
) -> Result<(Option<String>, String), ApiError> {
    if let PaceScope::Enrollment(enrollment_id) = scope {
        // The start date of the student enrollment.
        let row: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
            sqlx::query_as("SELECT start_at FROM enrollments WHERE id = $1")
                .bind(enrollment_id)
                .fetch_optional(db)
                .await?;
        if let Some((Some(start_at),)) = row {
            return Ok((Some(start_at.to_rfc3339()), "user".to_string()));
        }
        let created: Option<(chrono::DateTime<chrono::Utc>,)> =
            sqlx::query_as("SELECT created_at FROM enrollments WHERE id = $1")
                .bind(enrollment_id)
                .fetch_optional(db)
                .await?;
        if let Some((created,)) = created {
            return Ok((Some(created.to_rfc3339()), "user".to_string()));
        }
    }

    if let PaceScope::Section(section_id) = scope {
        let row: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
            sqlx::query_as("SELECT start_at FROM course_sections WHERE id = $1")
                .bind(section_id)
                .fetch_optional(db)
                .await?;
        if let Some((Some(start_at),)) = row {
            return Ok((Some(start_at.to_rfc3339()), "section".to_string()));
        }
    }

    let row: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT start_at FROM courses WHERE id = $1")
            .bind(pace.course_id)
            .fetch_optional(db)
            .await?;
    match row.and_then(|row| row.0) {
        Some(start_at) => Ok((Some(start_at.to_rfc3339()), "course".to_string())),
        None => Ok((None, "hypothetical".to_string())),
    }
}

/// Canvas `CoursePace#effective_end_date(with_context: true)`.
async fn end_dates(
    db: &PgPool,
    pace: &PaceRow,
    scope: PaceScope,
) -> Result<(Option<String>, String), ApiError> {
    if pace.hard_end_dates {
        if let Some(end_date) = pace.end_date {
            return Ok((Some(end_date.to_rfc3339()), "user".to_string()));
        }
    }

    if let PaceScope::Section(section_id) = scope {
        let row: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
            sqlx::query_as("SELECT end_at FROM course_sections WHERE id = $1")
                .bind(section_id)
                .fetch_optional(db)
                .await?;
        if let Some((Some(end_at),)) = row {
            return Ok((Some(end_at.to_rfc3339()), "section".to_string()));
        }
    }

    let row: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT end_at FROM courses WHERE id = $1")
            .bind(pace.course_id)
            .fetch_optional(db)
            .await?;
    match row.and_then(|row| row.0) {
        Some(end_at) => Ok((Some(end_at.to_rfc3339()), "course".to_string())),
        None => Ok((None, "hypothetical".to_string())),
    }
}

/// The course pacing feature flag of a course (the `load_contexts` of the
/// two controllers reads the root account flag `course_pace_draft_state`).
#[allow(dead_code)]
async fn draft_state_enabled(db: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    course_paces_enabled(db, course_id).await
}
