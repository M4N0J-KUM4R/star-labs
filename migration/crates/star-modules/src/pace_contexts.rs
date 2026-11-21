//! course_pacing — Canvas `CoursePacing::PaceContextsApiController` parity.
//!
//! Routes (the crate router mounts this module under `/api/v1`):
//! - `GET /courses/{courseId}/pace_contexts` — the pace contexts of a course
//!
//! Canvas route (config/routes.rb, the course API block):
//!   get "courses/:course_id/pace_contexts", action: :index
//!
//! Canvas controller (`app/controllers/course_pacing/pace_contexts_api_controller.rb`):
//! the `before_action` chain is `require_context` (the course, 404 when it
//! does not exist), `ensure_pacing_enabled` (404 unless the course setting
//! `enable_course_paces` is on), `authorize_action` (the granular
//! manage-course-content set) and `load_type` (400 unless the `type`
//! parameter is one of "course", "section" or "student_enrollment").
//!
//! The response is the Canvas `CoursePacing::PaceContextsPresenter.as_json`
//! shape: `name`, `type`, `item_id`, `associated_section_count`,
//! `associated_student_count`, `applied_pace` and `on_pace`, next to the
//! `total_entries` count of the unfiltered set.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

use crate::course_pacing::{
    course_paces_enabled, load_course, require_content_right, ContentRight, CourseRow,
};

/// Canvas `CoursePacing::PaceContextsApiController::PERMITTED_CONTEXT_TYPES`.
const PERMITTED_CONTEXT_TYPES: [&str; 3] = ["course", "section", "student_enrollment"];

/// The query parameters Canvas permits (`filter_params`).
#[derive(Debug, Default, Deserialize)]
struct ContextsQuery {
    #[serde(rename = "type")]
    context_type: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    search_term: Option<String>,
    contexts: Option<String>,
}

/// One row of the pace-context listing.
struct PaceContext {
    id: i64,
    name: String,
    /// The Canvas class name of the context: "Course", "CourseSection" or
    /// "StudentEnrollment".
    class_name: &'static str,
    section_count: Option<i64>,
    student_count: Option<i64>,
    /// The user of a student-enrollment context.
    user_id: Option<i64>,
    /// The section of a student-enrollment context (Canvas
    /// `student_enrollment.course_section`).
    course_section_id: Option<i64>,
}

/// Domain router for the Canvas pace-contexts routes.
pub fn router() -> Router<AppState> {
    Router::new().route("/courses/{courseId}/pace_contexts", get(index))
}

/// Canvas `CoursePacing::PaceContextsApiController#index`.
async fn index(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    auth: AuthUser,
    Query(params): Query<ContextsQuery>,
) -> Result<Response, ApiError> {
    // `require_context`: the course must exist (404).
    let course = load_course(&state.db, course_id).await?;
    // `ensure_pacing_enabled`: 404 unless the course paces are on.
    if !course_paces_enabled(&state.db, course.id).await? {
        return Err(ApiError::NotFound);
    }
    // `authorize_action`: the granular manage-course-content set.
    require_content_right(&state.db, &auth, course.id, ContentRight::Any).await?;
    // `load_type`: 400 for an unknown context type.
    let context_type = params.context_type.as_deref().unwrap_or_default();
    if !PERMITTED_CONTEXT_TYPES.contains(&context_type) {
        return Err(ApiError::BadRequest(
            "Expected a value of 'course', 'section', or 'student_enrollment'".to_string(),
        ));
    }

    let contexts = contexts_of_type(&state.db, &course, context_type, &params).await?;
    let total_entries = contexts.len();

    let mut rendered = Vec::with_capacity(contexts.len());
    for context in &contexts {
        rendered.push(context_json(&state.db, course.id, context).await?);
    }

    Ok(Json(json!({
        "pace_contexts": rendered,
        "total_entries": total_entries,
    }))
    .into_response())
}

/// Canvas `CoursePacing::PaceContextsService#contexts_of_type`.
async fn contexts_of_type(
    db: &PgPool,
    course: &CourseRow,
    context_type: &str,
    params: &ContextsQuery,
) -> Result<Vec<PaceContext>, ApiError> {
    match context_type {
        "course" => Ok(vec![PaceContext {
            id: course.id,
            name: course_name(db, course.id).await?,
            class_name: "Course",
            section_count: Some(section_count(db, course.id).await?),
            student_count: Some(student_count(db, course.id).await?),
            user_id: None,
            course_section_id: None,
        }]),
        "section" => section_contexts(db, course.id, params).await,
        "student_enrollment" => student_contexts(db, course.id, params).await,
        other => Err(ApiError::BadRequest(format!(
            "Expected a value of 'course', 'section', or 'student_enrollment', got '{other}'"
        ))),
    }
}

/// The active sections of the course, filtered and ordered like Canvas.
async fn section_contexts(
    db: &PgPool,
    course_id: i64,
    params: &ContextsQuery,
) -> Result<Vec<PaceContext>, ApiError> {
    // Canvas: `course.active_course_sections`, filtered by the search term,
    // then by the `contexts` filter.
    let mut sql = String::from(
        "SELECT id, name FROM course_sections \
         WHERE course_id = $1 AND workflow_state <> 'deleted'",
    );
    if let Some(search_term) = params
        .search_term
        .as_deref()
        .filter(|term| !term.is_empty())
    {
        sql.push_str(" AND name ILIKE $2");
    }
    sql.push_str(" ORDER BY name ASC");

    let mut rows: Vec<(i64, String)> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(params.search_term.as_deref().unwrap_or_default())
        .fetch_all(db)
        .await?;

    if let Some(ids) = context_ids(params) {
        rows.retain(|(id, _)| ids.contains(id));
    }
    // Canvas sorts the sections by name only when `sort` is "name"; the
    // default order is the natural one.
    if params.sort.as_deref() == Some("name") {
        rows.sort_by(|left, right| left.1.cmp(&right.1));
    }
    if params.order.as_deref() == Some("desc") {
        rows.reverse();
    }

    Ok(rows
        .into_iter()
        .map(|(id, name)| PaceContext {
            id,
            name,
            class_name: "CourseSection",
            section_count: None,
            student_count: None,
            user_id: None,
            course_section_id: None,
        })
        .collect())
}

/// The student enrollments of the course, one per student.
async fn student_contexts(
    db: &PgPool,
    course_id: i64,
    params: &ContextsQuery,
) -> Result<Vec<PaceContext>, ApiError> {
    // Canvas: `DISTINCT ON (enrollments.user_id) enrollments.*`, ordered by
    // `user_id, created_at DESC`, keeping only the latest enrollment of a
    // student, and only the enrollments that are active, creation_pending or
    // invited.
    let mut sql = String::from(
        "SELECT DISTINCT ON (e.user_id) e.id, e.user_id, e.course_section_id, u.name \
         FROM enrollments e JOIN users u ON u.id = e.user_id \
         WHERE e.course_id = $1 \
           AND e.workflow_state IN ('active', 'creation_pending', 'invited')",
    );
    if let Some(search_term) = params
        .search_term
        .as_deref()
        .filter(|term| !term.is_empty())
    {
        sql.push_str(" AND u.name ILIKE $2");
    }
    sql.push_str(" ORDER BY e.user_id, e.created_at DESC");

    let mut rows: Vec<(i64, i64, Option<i64>, String)> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(params.search_term.as_deref().unwrap_or_default())
        .fetch_all(db)
        .await?;

    if let Some(ids) = context_ids(params) {
        rows.retain(|(id, _, _, _)| ids.contains(id));
    }
    // Canvas sorts the enrollments by the sortable name of the user when
    // `sort` is "name".
    if params.sort.as_deref() == Some("name") {
        rows.sort_by(|left, right| left.3.cmp(&right.3));
    }
    if params.order.as_deref() == Some("desc") {
        rows.reverse();
    }

    Ok(rows
        .into_iter()
        .map(|(id, user_id, course_section_id, name)| PaceContext {
            id,
            name,
            class_name: "StudentEnrollment",
            section_count: None,
            student_count: None,
            user_id: Some(user_id),
            course_section_id,
        })
        .collect())
}

/// The `contexts` filter: a JSON array of context ids.
fn context_ids(params: &ContextsQuery) -> Option<Vec<i64>> {
    let raw = params
        .contexts
        .as_deref()
        .filter(|value| !value.is_empty())?;
    let parsed: Vec<Value> = serde_json::from_str(raw).ok()?;
    Some(
        parsed
            .iter()
            .filter_map(|value| value.as_i64())
            .collect::<Vec<i64>>(),
    )
}

/// The name of the course.
async fn course_name(db: &PgPool, course_id: i64) -> Result<String, ApiError> {
    let row: Option<(String,)> = sqlx::query_as("SELECT name FROM courses WHERE id = $1")
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    Ok(row.map(|row| row.0).unwrap_or_default())
}

/// The number of the active sections of a course.
async fn section_count(db: &PgPool, course_id: i64) -> Result<i64, ApiError> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM course_sections \
         WHERE course_id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(count.0)
}

/// The number of the student enrollments of a course.
async fn student_count(db: &PgPool, course_id: i64) -> Result<i64, ApiError> {
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM enrollments \
         WHERE course_id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_one(db)
    .await?;
    Ok(count.0)
}

/// Canvas `CoursePacing::PaceContextsPresenter.as_json`.
async fn context_json(
    db: &PgPool,
    course_id: i64,
    context: &PaceContext,
) -> Result<Value, ApiError> {
    let applied_pace = applied_pace_for(db, course_id, context).await?;
    let on_pace = match context.user_id {
        // Only a student-enrollment context carries the on-pace flag.
        Some(user_id) => !overdue_user_ids(db, course_id).await?.contains(&user_id),
        None => Value::Null,
    };

    Ok(json!({
        "name": context.name,
        "type": context.class_name,
        "item_id": context.id,
        "associated_section_count": context.section_count,
        "associated_student_count": context.student_count,
        "applied_pace": applied_pace,
        "on_pace": on_pace,
    }))
}

/// Canvas `CoursePacing::PaceContextsPresenter.applied_pace_for`.
///
/// The applied pace of a context is the pace of the context itself; a
/// student enrollment without one of its own falls back to the pace of its
/// section, and a section without one falls back to the primary pace of the
/// course.
async fn applied_pace_for(
    db: &PgPool,
    course_id: i64,
    context: &PaceContext,
) -> Result<Value, ApiError> {
    let pace = match context.class_name {
        "Course" => primary_pace(db, course_id).await?,
        "CourseSection" => section_pace(db, course_id, context.id)
            .await?
            .or(primary_pace(db, course_id).await?),
        _ => {
            // The enrollment's own pace, then the pace of its section, then
            // the primary pace of the course.
            let own = enrollment_pace(db, course_id, context.user_id).await?;
            if own.is_some() {
                own
            } else {
                let section = match context.course_section_id {
                    Some(section_id) => section_pace(db, course_id, section_id).await?,
                    None => None,
                };
                section.or(primary_pace(db, course_id).await?)
            }
        }
    };

    let Some(pace) = pace else {
        return Ok(Value::Null);
    };

    let duration: (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(duration), 0) FROM course_pace_module_items WHERE course_pace_id = $1",
    )
    .bind(pace.id)
    .fetch_one(db)
    .await?;
    let published_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT published_at FROM course_paces WHERE id = $1")
            .bind(pace.id)
            .fetch_one(db)
            .await?;

    Ok(json!({
        "name": pace_name(db, &pace).await?,
        "type": pace_type(&pace),
        // Canvas `CoursePace#duration`: the sum of the durations plus one day
        // for the oddity of the first item's due date.
        "duration": duration.0 + 1,
        "last_modified": published_at.map(|value| value.to_rfc3339()),
    }))
}

/// The `course_paces` row of a pace.
#[derive(Debug, sqlx::FromRow)]
struct PaceRow {
    id: i64,
    course_id: i64,
    course_section_id: Option<i64>,
    user_id: Option<i64>,
    published_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The primary pace of the course (`course_paces.primary.take`).
async fn primary_pace(db: &PgPool, course_id: i64) -> Result<Option<PaceRow>, ApiError> {
    let row: Option<PaceRow> = sqlx::query_as(
        "SELECT id, course_id, course_section_id, user_id, published_at FROM course_paces \
         WHERE course_id = $1 AND course_section_id IS NULL AND user_id IS NULL \
           AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .bind(course_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// The pace of a section (`course_paces.find_by(course_section_id:)`).
async fn section_pace(
    db: &PgPool,
    course_id: i64,
    section_id: i64,
) -> Result<Option<PaceRow>, ApiError> {
    let row: Option<PaceRow> = sqlx::query_as(
        "SELECT id, course_id, course_section_id, user_id, published_at FROM course_paces \
         WHERE course_id = $1 AND course_section_id = $2 AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .bind(course_id)
    .bind(section_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// The pace of a user (`course_paces.find_by(user_id:)`).
async fn enrollment_pace(
    db: &PgPool,
    course_id: i64,
    user_id: Option<i64>,
) -> Result<Option<PaceRow>, ApiError> {
    let Some(user_id) = user_id else {
        return Ok(None);
    };
    let row: Option<PaceRow> = sqlx::query_as(
        "SELECT id, course_id, course_section_id, user_id, published_at FROM course_paces \
         WHERE course_id = $1 AND user_id = $2 AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .bind(course_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    Ok(row)
}

/// Canvas `CoursePace#effective_name`: the name of the user, the section or
/// the course the pace belongs to.
async fn pace_name(db: &PgPool, pace: &PaceRow) -> Result<String, ApiError> {
    if let Some(user_id) = pace.user_id {
        let row: Option<(String,)> = sqlx::query_as("SELECT name FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_optional(db)
            .await?;
        return Ok(row.map(|row| row.0).unwrap_or_default());
    }
    if let Some(section_id) = pace.course_section_id {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT name FROM course_sections WHERE id = $1")
                .bind(section_id)
                .fetch_optional(db)
                .await?;
        return Ok(row.map(|row| row.0).unwrap_or_default());
    }
    course_name(db, pace.course_id).await
}

/// Canvas `CoursePace#type`.
fn pace_type(pace: &PaceRow) -> &'static str {
    if pace.user_id.is_some() {
        "StudentEnrollment"
    } else if pace.course_section_id.is_some() {
        "Section"
    } else {
        "Course"
    }
}

/// The users with overdue items — Canvas
/// `CoursePacing::CoursePaceService.off_pace_counts_by_user`.
///
/// A student is off pace when a published assignment of the course has an
/// active individual override with a due date in the past and the student has
/// no submission for it.
async fn overdue_user_ids(db: &PgPool, course_id: i64) -> Result<Vec<i64>, ApiError> {
    let rows: Vec<(Vec<i64>,)> = sqlx::query_as(
        "SELECT ao.student_ids \
         FROM assignment_overrides ao \
         JOIN assignments a ON a.id = ao.assignment_id \
         WHERE a.course_id = $1 \
           AND a.workflow_state = 'published' \
           AND ao.workflow_state = 'active' \
           AND ao.due_at IS NOT NULL \
           AND ao.due_at < now()",
    )
    .bind(course_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().flatten().flatten().collect())
}
