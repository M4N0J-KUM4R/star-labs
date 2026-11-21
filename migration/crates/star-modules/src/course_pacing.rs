//! course_pacing — Canvas `CoursePacesController` (API v1) parity.
//!
//! Canvas routes (config/routes.rb, the course API block):
//! - POST   /courses/{courseId}/course_pacing
//! - GET    /courses/{courseId}/course_pacing/new
//! - GET    /courses/{courseId}/course_pacing/{id}
//! - PUT    /courses/{courseId}/course_pacing/{id}
//! - DELETE /courses/{courseId}/course_pacing/{id}
//! - POST   /courses/{courseId}/course_pacing/{id}/publish
//! - POST   /courses/{courseId}/course_pacing/compress_dates
//! - POST   /courses/{courseId}/course_pacing/bulk_create_enrollment_paces
//!
//! Canvas controller (app/controllers/course_paces_controller.rb):
//! `load_context` (the course, one of its sections or one of its
//! enrollments, taken from the `course_section_id` / `enrollment_id` query
//! parameters), `load_course`, `require_feature_flag` (404 unless the course
//! setting `enable_course_paces` is on) and `load_course_pace` (404) all run
//! before the permission check, so a caller without the right still sees the
//! 404s.
//!
//! Permissions (Canvas):
//! - `api_show`, `new` — the granular manage-course-content set
//!   (`manage_course_content_add|edit|delete`);
//! - `create` — `manage_course_content_add`;
//! - `update`, `publish`, `compress_dates`, `bulk_create_enrollment_paces` —
//!   `manage_course_content_edit`;
//! - `destroy` — `manage_course_content_delete`.
//!
//! This workspace has no role-override table, so (like
//! star-content/src/migration_issues.rs) the granular set collapses into one
//! gate per context: a course context needs a teacher/ta/designer enrollment
//! in the course, global admins pass everywhere.
//!
//! JSON shapes:
//! - `CoursePacePresenter#as_json` for the `course_pace` member: id,
//!   course_id, course_section_id, user_id, workflow_state,
//!   exclude_weekends, selected_days_to_skip, hard_end_dates, created_at,
//!   updated_at, published_at, root_account_id, modules, context_id,
//!   assignments_weighting, time_to_complete_calendar_days, context_type,
//!   start_date, start_date_context, end_date, end_date_context;
//! - `Api::V1::Progress#progress_json` for the `progress` member (and the
//!   whole body of `publish`): id, context_id, context_type, user_id, tag,
//!   completion, workflow_state, created_at, updated_at, message, url;
//! - `compress_dates` answers the flat `{module_item_id => due date}` map.
//!
//! Deviations from Canvas (all forced by the port schema):
//! - The port has no `content_tags` table; a Canvas ContentTag is a
//!   `module_items` row here, so `module_item_id` points at `module_items.id`.
//! - The port has no course time zone, so every date is computed and
//!   rendered in UTC.
//! - The port has no course-pace job runner, so the publish work runs
//!   inline; the `course_pace_publish` progress row still stays `queued`,
//!   exactly like the Canvas response of the endpoint that queues the job.
//! - `enrollments.start_at` is nullable in the port, so a user pace falls
//!   back to the enrollment's `created_at` (Canvas uses the same fallback
//!   when `start_at` is nil).
//! - `calendar_events` has no blackout flag, so only the `blackout_dates`
//!   table feeds the business-day holidays.
//! - The port's `CourseSettings` struct has no `enable_course_paces` key, so
//!   the course settings API cannot turn the paces on; the module reads the
//!   raw settings document (`courses.settings` merged with
//!   `course_settings.settings`).
//! - `create` and `update` render the pace as it was before the publish ran,
//!   exactly like Canvas, where the endpoint only queues the publish job.

use std::collections::HashSet;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, SecondsFormat, Timelike, Utc, Weekday};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use star_auth::AuthUser;
use star_common::{ApiError, AppState};

// ---------------------------------------------------------------------------
// Canvas JSON helpers
// ---------------------------------------------------------------------------

/// Canvas renders timestamps as RFC3339 with seconds and a `Z` suffix.
fn ts(value: &DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn opt_ts(value: &Option<DateTime<Utc>>) -> Option<String> {
    value.as_ref().map(ts)
}

/// The 422 body of a failed validation (Canvas renders
/// `json: { success: false, errors: ... }` with `:unprocessable_content`).
fn unprocessable(errors: Value) -> (StatusCode, Json<Value>) {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "success": false, "errors": errors })),
    )
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// One `course_paces` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct CoursePaceRow {
    id: i64,
    course_id: i64,
    course_section_id: Option<i64>,
    user_id: Option<i64>,
    root_account_id: Option<i64>,
    workflow_state: String,
    exclude_weekends: bool,
    selected_days_to_skip: Vec<String>,
    hard_end_dates: bool,
    end_date: Option<DateTime<Utc>>,
    published_at: Option<DateTime<Utc>>,
    assignments_weighting: String,
    time_to_complete_calendar_days: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// One `course_pace_module_items` row joined with its module item, its
/// module and (for assignment items) the assignment itself.
#[derive(Debug, Clone, sqlx::FromRow)]
struct PaceItemRow {
    id: i64,
    course_pace_id: i64,
    module_item_id: i64,
    root_account_id: Option<i64>,
    duration: i32,
    item_type: String,
    item_id: i64,
    item_position: i32,
    item_title: Option<String>,
    item_workflow_state: String,
    module_id: i64,
    module_name: String,
    module_position: i32,
    assignment_title: Option<String>,
    points_possible: Option<f64>,
}

/// One `progress` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct ProgressRow {
    id: i64,
    tag: String,
    workflow_state: String,
    completion: i32,
    context_type: Option<String>,
    context_id: Option<i64>,
    user_id: Option<i64>,
    message: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// The course row fields the pacing code needs.
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct CourseRow {
    pub(crate) id: i64,
    pub(crate) account_id: Option<i64>,
    pub(crate) start_at: Option<DateTime<Utc>>,
    pub(crate) end_at: Option<DateTime<Utc>>,
    pub(crate) enrollment_term_id: Option<i64>,
}

/// A `course_sections` row.
#[derive(Debug, Clone, sqlx::FromRow)]
struct SectionRow {
    course_id: i64,
    start_at: Option<DateTime<Utc>>,
    end_at: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

/// The optional query parameters Canvas reads to pick the pacing context.
#[derive(Debug, Default, Deserialize)]
pub struct PaceQuery {
    pub course_section_id: Option<i64>,
    pub enrollment_id: Option<i64>,
}

/// One entry of `course_pace_module_items_attributes`.
#[derive(Debug, Clone, Deserialize)]
pub struct PaceModuleItemAttributes {
    pub id: Option<i64>,
    pub module_item_id: Option<i64>,
    pub duration: Option<i32>,
    pub root_account_id: Option<i64>,
}

/// The permitted `course_pace` attributes (Canvas `create_params` /
/// `update_params`).
#[derive(Debug, Default, Clone)]
struct PaceParams {
    context_id: Option<i64>,
    context_type: Option<String>,
    course_section_id: Option<i64>,
    user_id: Option<i64>,
    end_date: Option<DateTime<Utc>>,
    start_date: Option<DateTime<Utc>>,
    exclude_weekends: Option<bool>,
    hard_end_dates: Option<bool>,
    workflow_state: Option<String>,
    time_to_complete_calendar_days: Option<i32>,
    selected_days_to_skip: Option<Vec<String>>,
    assignments_weighting: Option<Value>,
    /// True when the request carried `course_pace_module_items_attributes`.
    /// Rails only touches the collection when the key is present.
    module_items_given: bool,
    module_items: Vec<PaceModuleItemAttributes>,
}

/// Canvas `set_context_ids_in`: `context_type` "Section" fills
/// `course_section_id`, "Enrollment" fills `user_id`; the context pair is
/// dropped afterwards.
fn set_context_ids_in(params: &mut PaceParams) {
    let (Some(context_id), Some(context_type)) = (params.context_id, params.context_type.clone())
    else {
        return;
    };
    match context_type.as_str() {
        "Section" => params.course_section_id = Some(context_id),
        "Enrollment" => params.user_id = Some(context_id),
        _ => {}
    }
    params.context_id = None;
    params.context_type = None;
}

fn json_bool(value: Option<&Value>) -> Option<bool> {
    match value {
        Some(Value::Bool(value)) => Some(*value),
        Some(Value::String(text)) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "t" | "1" | "yes" | "on" => Some(true),
            "false" | "f" | "0" | "no" | "off" => Some(false),
            _ => None,
        },
        Some(Value::Number(number)) => number.as_f64().map(|value| value != 0.0),
        _ => None,
    }
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(number)) => number.as_i64(),
        Some(Value::String(text)) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// The integer of a JSON value that is not optional.
fn json_i64_value(value: &Value) -> Option<i64> {
    json_i64(Some(value))
}

/// Canvas casts a date column from a string; an unparsable value becomes
/// nil (Rails' `Type::Date` cast returns nil for garbage).
fn json_datetime(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let text = value?.as_str()?.trim().to_string();
    if text.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(&text) {
        return Some(parsed.with_timezone(&Utc));
    }
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(parsed) = chrono::NaiveDateTime::parse_from_str(&text, format) {
            return Some(DateTime::from_naive_utc_and_offset(parsed, Utc));
        }
    }
    if let Ok(date) = NaiveDate::parse_from_str(&text, "%Y-%m-%d") {
        if let Some(naive) = date.and_hms_opt(0, 0, 0) {
            return Some(DateTime::from_naive_utc_and_offset(naive, Utc));
        }
    }
    None
}

fn json_string_list(value: Option<&Value>) -> Option<Vec<String>> {
    let array = value?.as_array()?;
    Some(
        array
            .iter()
            .filter_map(|item| item.as_str().map(|text| text.to_string()))
            .collect(),
    )
}

/// Read the permitted `course_pace` attributes out of a request body.
///
/// Canvas requires the `course_pace` key (`params.require(:course_pace)`);
/// this port also accepts a flat body, like the other module routes in this
/// crate do.
fn parse_pace_params(body: &Value) -> PaceParams {
    let source = match body.get("course_pace") {
        Some(value) if value.is_object() => value.clone(),
        _ => body.clone(),
    };

    let mut params = PaceParams {
        context_id: json_i64(source.get("context_id")),
        context_type: source
            .get("context_type")
            .and_then(|value| value.as_str())
            .map(|text| text.to_string()),
        course_section_id: json_i64(source.get("course_section_id")),
        user_id: json_i64(source.get("user_id")),
        end_date: json_datetime(source.get("end_date")),
        start_date: json_datetime(source.get("start_date")),
        exclude_weekends: json_bool(source.get("exclude_weekends")),
        hard_end_dates: json_bool(source.get("hard_end_dates")),
        workflow_state: source
            .get("workflow_state")
            .and_then(|value| value.as_str())
            .map(|text| text.to_string()),
        time_to_complete_calendar_days: json_i64(source.get("time_to_complete_calendar_days"))
            .map(|value| value as i32),
        selected_days_to_skip: json_string_list(source.get("selected_days_to_skip")),
        assignments_weighting: source.get("assignments_weighting").cloned(),
        module_items_given: false,
        module_items: Vec::new(),
    };

    if let Some(items) = source
        .get("course_pace_module_items_attributes")
        .and_then(|value| value.as_array())
    {
        params.module_items_given = true;
        for item in items {
            params.module_items.push(PaceModuleItemAttributes {
                id: json_i64(item.get("id")),
                module_item_id: json_i64(item.get("module_item_id")),
                duration: json_i64(item.get("duration")).map(|value| value as i32),
                root_account_id: json_i64(item.get("root_account_id")),
            });
        }
    }

    set_context_ids_in(&mut params);
    params
}

/// Canvas `params[:enrollment_ids]` (top level of the bulk request body).
fn parse_enrollment_ids(body: &Value) -> Vec<i64> {
    body.get("enrollment_ids")
        .and_then(|value| value.as_array())
        .map(|items| items.iter().filter_map(json_i64_value).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Context, settings and permissions
// ---------------------------------------------------------------------------

/// The pacing context: the course itself, one of its sections, or one of
/// its enrollments (Canvas `load_context`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum PaceContext {
    Course,
    Section(i64),
    Enrollment(i64),
}

impl PaceContext {
    /// The scope of a context: the section id or the user id the paces of
    /// the context belong to. An enrollment context is scoped by the user of
    /// the enrollment.
    async fn scope(&self, db: &PgPool) -> Result<(Option<i64>, Option<i64>), ApiError> {
        match self {
            PaceContext::Course => Ok((None, None)),
            PaceContext::Section(section_id) => Ok((Some(*section_id), None)),
            PaceContext::Enrollment(enrollment_id) => {
                let row: Option<(i64,)> =
                    sqlx::query_as("SELECT user_id FROM enrollments WHERE id = $1")
                        .bind(enrollment_id)
                        .fetch_optional(db)
                        .await?;
                Ok((None, row.map(|row| row.0)))
            }
        }
    }
}

/// The workspace's stand-in for the Canvas granular manage-course-content
/// permission set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ContentRight {
    /// Canvas `manage_course_content_add` (create).
    Add,
    /// Canvas `manage_course_content_edit` (update, publish, compress_dates,
    /// bulk_create_enrollment_paces).
    Edit,
    /// Canvas `manage_course_content_delete` (destroy).
    Delete,
    /// The granular set (api_show, new): any of add / edit / delete.
    Any,
}

/// The workspace's simplified role model: a course context needs a
/// teacher/ta/designer enrollment, global admins pass everywhere.
pub(crate) async fn require_content_right(
    db: &PgPool,
    auth: &AuthUser,
    course_id: i64,
    _right: ContentRight,
) -> Result<(), ApiError> {
    if auth.role == "admin" {
        return Ok(());
    }

    let row: Option<(String,)> = sqlx::query_as(
        "SELECT role FROM enrollments \
         WHERE user_id = $1 AND course_id = $2 AND workflow_state = 'active' \
           AND role IN ('teacher', 'ta', 'designer') \
         LIMIT 1",
    )
    .bind(auth.id)
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    match row {
        Some(_) => Ok(()),
        None => Err(ApiError::Forbidden),
    }
}

/// Load the course (404 when it does not exist).
pub(crate) async fn load_course(db: &PgPool, course_id: i64) -> Result<CourseRow, ApiError> {
    let course: Option<CourseRow> = sqlx::query_as(
        "SELECT id, account_id, start_at, end_at, enrollment_term_id \
         FROM courses WHERE id = $1 AND workflow_state <> 'deleted'",
    )
    .bind(course_id)
    .fetch_optional(db)
    .await?;

    course.ok_or(ApiError::NotFound)
}

/// The merged course settings document (`courses.settings` plus the
/// `course_settings` row).
async fn course_settings_json(db: &PgPool, course_id: i64) -> Value {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as::<_, (Option<String>, Option<String>)>(
            "SELECT c.settings::text, cs.settings \
         FROM courses c LEFT JOIN course_settings cs ON cs.course_id = c.id \
         WHERE c.id = $1",
        )
        .bind(course_id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();

    let mut merged = Map::new();
    if let Some((course_settings, section_settings)) = row {
        for raw in [course_settings, section_settings].into_iter().flatten() {
            if let Ok(Value::Object(object)) = serde_json::from_str::<Value>(&raw) {
                for (key, value) in object {
                    merged.insert(key, value);
                }
            }
        }
    }
    Value::Object(merged)
}

/// Canvas `Course#enable_course_paces` (a course setting, default false).
/// The route answers 404 while it is off.
pub(crate) async fn course_paces_enabled(db: &PgPool, course_id: i64) -> Result<bool, ApiError> {
    let settings = course_settings_json(db, course_id).await;
    Ok(json_bool(settings.get("enable_course_paces")).unwrap_or(false))
}

/// Canvas `Course#restrict_enrollments_to_course_dates` (default true).
fn restrict_enrollments_to_course_dates(settings: &Value) -> bool {
    json_bool(settings.get("restrict_enrollments_to_course_dates")).unwrap_or(true)
}

/// The root account of the course (Canvas `course.root_account`).
async fn root_account_id(db: &PgPool, course: &CourseRow) -> Option<i64> {
    let mut account_id = course.account_id;
    for _ in 0..16 {
        let Some(id) = account_id else { break };
        let parent: Option<(Option<i64>,)> =
            sqlx::query_as("SELECT parent_account_id FROM accounts WHERE id = $1")
                .bind(id)
                .fetch_optional(db)
                .await
                .ok()
                .flatten();
        match parent {
            Some((None,)) => return Some(id),
            Some((Some(parent_id),)) => account_id = Some(parent_id),
            None => return None,
        }
    }
    None
}

/// Canvas `account.feature_enabled?(feature)`.
async fn account_feature_enabled(db: &PgPool, account_id: Option<i64>, feature: &str) -> bool {
    let Some(account_id) = account_id else {
        return false;
    };
    let state: Option<(String,)> = sqlx::query_as(
        "SELECT state FROM feature_flags \
         WHERE context_type = 'Account' AND context_id = $1 AND feature = $2 LIMIT 1",
    )
    .bind(account_id)
    .bind(feature)
    .fetch_optional(db)
    .await
    .ok()
    .flatten();

    match state {
        Some((state,)) => matches!(state.as_str(), "on" | "allowed_on" | "enabled"),
        None => false,
    }
}

/// Resolve the pacing context (Canvas `load_context` + `load_course`).
async fn resolve_context(
    db: &PgPool,
    course_id: i64,
    query: &PaceQuery,
) -> Result<(PaceContext, CourseRow), ApiError> {
    if let Some(enrollment_id) = query.enrollment_id {
        let row: Option<(i64,)> = sqlx::query_as("SELECT course_id FROM enrollments WHERE id = $1")
            .bind(enrollment_id)
            .fetch_optional(db)
            .await?;
        let (enrollment_course_id,) = row.ok_or(ApiError::NotFound)?;
        if enrollment_course_id != course_id {
            return Err(ApiError::NotFound);
        }
        let course = load_course(db, course_id).await?;
        return Ok((PaceContext::Enrollment(enrollment_id), course));
    }

    if let Some(section_id) = query.course_section_id {
        let section: Option<SectionRow> = sqlx::query_as(
            "SELECT course_id, start_at, end_at FROM course_sections \
             WHERE id = $1 AND workflow_state <> 'deleted'",
        )
        .bind(section_id)
        .fetch_optional(db)
        .await?;
        let section = section.ok_or(ApiError::NotFound)?;
        if section.course_id != course_id {
            return Err(ApiError::NotFound);
        }
        let course = load_course(db, course_id).await?;
        return Ok((PaceContext::Section(section_id), course));
    }

    let course = load_course(db, course_id).await?;
    Ok((PaceContext::Course, course))
}

/// Canvas `require_feature_flag`: 404 unless the course setting
/// `enable_course_paces` is on.
pub(crate) async fn require_feature_flag(db: &PgPool, course_id: i64) -> Result<(), ApiError> {
    if course_paces_enabled(db, course_id).await? {
        Ok(())
    } else {
        Err(ApiError::NotFound)
    }
}

// ---------------------------------------------------------------------------
// Pace loading
// ---------------------------------------------------------------------------

const PACE_COLUMNS: &str = "id, course_id, course_section_id, user_id, root_account_id, \
     workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates, end_date, \
     published_at, assignments_weighting, time_to_complete_calendar_days, created_at, updated_at";

/// Canvas `load_course_pace`: the pace must belong to the course.
async fn load_pace(db: &PgPool, course_id: i64, id: i64) -> Result<CoursePaceRow, ApiError> {
    let sql = format!("SELECT {PACE_COLUMNS} FROM course_paces WHERE id = $1 AND course_id = $2");
    let pace: Option<CoursePaceRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(course_id)
        .fetch_optional(db)
        .await?;
    pace.ok_or(ApiError::NotFound)
}

/// The pace module items with their module and assignment.
async fn load_pace_items(db: &PgPool, pace_id: i64) -> Result<Vec<PaceItemRow>, ApiError> {
    let items: Vec<PaceItemRow> = sqlx::query_as(
        r#"
        SELECT cp.id, cp.course_pace_id, cp.module_item_id, cp.root_account_id, cp.duration,
               mi.item_type, mi.item_id, mi.position AS item_position, mi.title AS item_title,
               mi.workflow_state AS item_workflow_state,
               m.id AS module_id, m.name AS module_name, m.position AS module_position,
               a.title AS assignment_title, a.points_possible::float8 AS points_possible
        FROM course_pace_module_items cp
        JOIN module_items mi ON mi.id = cp.module_item_id
        JOIN modules m ON m.id = mi.module_id
        LEFT JOIN assignments a ON mi.item_type = 'assignment' AND a.id = mi.item_id
        WHERE cp.course_pace_id = $1
        ORDER BY m.position ASC, m.id ASC, mi.position ASC, mi.id ASC
        "#,
    )
    .bind(pace_id)
    .fetch_all(db)
    .await?;
    Ok(items)
}

/// The course's assignment module items (the scaffold of `new`).
async fn load_course_assignment_items(
    db: &PgPool,
    course_id: i64,
) -> Result<Vec<PaceItemRow>, ApiError> {
    let items: Vec<PaceItemRow> = sqlx::query_as(
        r#"
        SELECT 0::bigint AS id, 0::bigint AS course_pace_id, mi.id AS module_item_id,
               NULL::bigint AS root_account_id, 0 AS duration,
               mi.item_type, mi.item_id, mi.position AS item_position, mi.title AS item_title,
               mi.workflow_state AS item_workflow_state,
               m.id AS module_id, m.name AS module_name, m.position AS module_position,
               a.title AS assignment_title, a.points_possible::float8 AS points_possible
        FROM module_items mi
        JOIN modules m ON m.id = mi.module_id
        LEFT JOIN assignments a ON mi.item_type = 'assignment' AND a.id = mi.item_id
        WHERE m.course_id = $1 AND mi.item_type = 'assignment'
          AND mi.workflow_state <> 'deleted' AND m.workflow_state <> 'deleted'
        ORDER BY m.position ASC, m.id ASC, mi.position ASC, mi.id ASC
        "#,
    )
    .bind(course_id)
    .fetch_all(db)
    .await?;
    Ok(items)
}

/// One pace of the course, filtered by the scope of a context.
async fn scoped_pace(
    db: &PgPool,
    course_id: i64,
    section_id: Option<i64>,
    user_id: Option<i64>,
    published_only: bool,
) -> Result<Option<CoursePaceRow>, ApiError> {
    let sql = if published_only {
        format!(
            "SELECT {PACE_COLUMNS} FROM course_paces \
             WHERE course_id = $1 AND workflow_state = 'active' AND published_at IS NOT NULL \
               AND course_section_id IS NOT DISTINCT FROM $2 \
               AND user_id IS NOT DISTINCT FROM $3 \
             ORDER BY id ASC LIMIT 1"
        )
    } else {
        format!(
            "SELECT {PACE_COLUMNS} FROM course_paces \
             WHERE course_id = $1 AND workflow_state <> 'deleted' \
               AND course_section_id IS NOT DISTINCT FROM $2 \
               AND user_id IS NOT DISTINCT FROM $3 \
             ORDER BY id ASC LIMIT 1"
        )
    };
    let row: Option<CoursePaceRow> = sqlx::query_as(&sql)
        .bind(course_id)
        .bind(section_id)
        .bind(user_id)
        .fetch_optional(db)
        .await?;
    Ok(row)
}

/// Canvas `CoursePace.pace_for_context(course, context, exact: true)`.
async fn pace_for_context(
    db: &PgPool,
    course_id: i64,
    context: PaceContext,
    exact: bool,
) -> Result<Option<CoursePaceRow>, ApiError> {
    let (section_id, user_id) = context.scope(db).await?;
    if let Some(pace) = scoped_pace(db, course_id, section_id, user_id, true).await? {
        return Ok(Some(pace));
    }
    if let Some(pace) = scoped_pace(db, course_id, section_id, user_id, false).await? {
        return Ok(Some(pace));
    }
    if exact {
        return Ok(None);
    }
    // Without `exact` the primary pace of the course is the fallback.
    if let Some(pace) = scoped_pace(db, course_id, None, None, true).await? {
        return Ok(Some(pace));
    }
    scoped_pace(db, course_id, None, None, false).await
}

// ---------------------------------------------------------------------------
// Dates: business-day math (Canvas `CoursePacesDateHelpers`)
// ---------------------------------------------------------------------------

/// The days of the week that count as work days. Monday is index 0.
#[derive(Debug, Clone, Copy)]
struct WorkWeek {
    enabled: [bool; 7],
}

impl WorkWeek {
    fn all_days() -> Self {
        WorkWeek { enabled: [true; 7] }
    }

    fn weekdays() -> Self {
        WorkWeek {
            enabled: [true, true, true, true, true, false, false],
        }
    }

    /// Canvas: `%i[sun mon tue wed thu fri sat] - selected_days_to_skip`.
    fn from_skipped(skipped: &[String]) -> Self {
        let mut week = WorkWeek::all_days();
        for day in skipped {
            let index = match day.trim().to_ascii_lowercase().as_str() {
                "mon" => Some(0),
                "tue" => Some(1),
                "wed" => Some(2),
                "thu" => Some(3),
                "fri" => Some(4),
                "sat" => Some(5),
                "sun" => Some(6),
                _ => None,
            };
            if let Some(index) = index {
                week.enabled[index] = false;
            }
        }
        week
    }

    fn is_work_day(&self, date: NaiveDate) -> bool {
        let index = match date.weekday() {
            Weekday::Mon => 0,
            Weekday::Tue => 1,
            Weekday::Wed => 2,
            Weekday::Thu => 3,
            Weekday::Fri => 4,
            Weekday::Sat => 5,
            Weekday::Sun => 6,
        };
        self.enabled[index]
    }
}

/// A holiday is a blackout date day.
type Holidays = HashSet<NaiveDate>;

fn is_business_day(date: NaiveDate, week: &WorkWeek, holidays: &Holidays) -> bool {
    week.is_work_day(date) && !holidays.contains(&date)
}

/// Canvas `CoursePacesDateHelpers.add_days` (BusinessTime
/// `duration.business_days.after(start_date)`).
fn add_business_days(
    start: NaiveDate,
    days: i64,
    week: &WorkWeek,
    holidays: &Holidays,
) -> NaiveDate {
    if days <= 0 {
        return start;
    }
    let mut date = start;
    let mut remaining = days;
    while remaining > 0 {
        date += Duration::days(1);
        if is_business_day(date, week, holidays) {
            remaining -= 1;
        }
    }
    date
}

/// Canvas `CoursePacesDateHelpers.first_enabled_day`.
fn first_enabled_day(start: NaiveDate, week: &WorkWeek, holidays: &Holidays) -> NaiveDate {
    let mut date = start;
    while !is_business_day(date, week, holidays) {
        date += Duration::days(1);
    }
    date
}

/// Canvas `CoursePacesDateHelpers.previous_enabled_day`.
fn previous_enabled_day(date: NaiveDate, week: &WorkWeek, holidays: &Holidays) -> NaiveDate {
    let mut cursor = date;
    while !is_business_day(cursor, week, holidays) {
        cursor -= Duration::days(1);
    }
    cursor
}

/// Canvas `CoursePacesDateHelpers.days_between`
/// (`start.to_date.business_days_until(end, inclusive)`): the business days
/// after `start` up to and (when inclusive) including `end`.
fn days_between(
    start: NaiveDate,
    end: NaiveDate,
    inclusive_end: bool,
    week: &WorkWeek,
    holidays: &Holidays,
) -> i64 {
    if end <= start {
        return 0;
    }
    let mut count = 0i64;
    let mut date = start;
    while date < end {
        date += Duration::days(1);
        if date < end && is_business_day(date, week, holidays) {
            count += 1;
        }
    }
    if inclusive_end && is_business_day(end, week, holidays) {
        count += 1;
    }
    count
}

/// The blackout dates of the course (`blackout_dates` rows of the course).
async fn course_blackout_dates(db: &PgPool, course_id: i64) -> Holidays {
    let rows: Vec<(NaiveDate, NaiveDate)> = sqlx::query_as(
        "SELECT start_date::date, end_date::date FROM blackout_dates \
         WHERE context_type = 'Course' AND context_id = $1",
    )
    .bind(course_id)
    .fetch_all(db)
    .await
    .unwrap_or_default();

    let mut holidays = Holidays::new();
    for (start, end) in rows {
        let mut date = start;
        while date <= end {
            holidays.insert(date);
            date += Duration::days(1);
        }
    }
    holidays
}

/// The blackout dates sent with a `compress_dates` request.
fn request_blackout_dates(body: &Value) -> Holidays {
    let mut holidays = Holidays::new();
    let Some(items) = body
        .get("blackout_dates")
        .and_then(|value| value.as_array())
    else {
        return holidays;
    };
    for item in items {
        let start = json_datetime(item.get("start_date")).map(|value| value.date_naive());
        let end = json_datetime(item.get("end_date")).map(|value| value.date_naive());
        let (Some(start), Some(end)) = (start, end) else {
            continue;
        };
        let mut date = start;
        while date <= end {
            holidays.insert(date);
            date += Duration::days(1);
        }
    }
    holidays
}

/// The work week of a pace (Canvas `CoursePacesDateHelpers.business_time_config`).
async fn pace_work_week(
    db: &PgPool,
    root_account: Option<i64>,
    exclude_weekends: bool,
    selected_days_to_skip: &[String],
) -> WorkWeek {
    if account_feature_enabled(db, root_account, "course_paces_skip_selected_days").await {
        WorkWeek::from_skipped(selected_days_to_skip)
    } else if exclude_weekends {
        WorkWeek::weekdays()
    } else {
        WorkWeek::all_days()
    }
}

/// The due dates of the pace items (Canvas `CoursePaceDueDatesCalculator`).
fn compute_due_dates(
    start: NaiveDate,
    durations: &[i32],
    week: &WorkWeek,
    holidays: &Holidays,
) -> Vec<NaiveDate> {
    let mut dates = Vec::with_capacity(durations.len());
    let mut current = start;
    for duration in durations {
        let due = add_business_days(current, *duration as i64, week, holidays);
        dates.push(due);
        current = due;
    }
    dates
}

/// The rounding breakpoint of the hard end date compressor.
const ROUNDING_BREAKPOINT: f64 = 0.75;

/// Canvas `CoursePaceHardEndDateCompressor.round_durations`.
///
/// The tuple is (duration, direction, remainder) with direction 1 = UP,
/// 2 = DOWN and 0 = NONE.
fn round_durations(durations: &[f64], plan_length: i64) -> Vec<i32> {
    let mut rounded: Vec<(i64, u8, f64)> = durations
        .iter()
        .map(|duration| {
            if *duration == 0.0 {
                return (0, 0, 0.0);
            }
            let remainder = duration % 1.0;
            if remainder >= ROUNDING_BREAKPOINT {
                (duration.ceil() as i64, 1, remainder)
            } else {
                (duration.floor() as i64, 2, remainder)
            }
        })
        .collect();

    // A group of assignments rounded down to zero shares one day.
    let mut zero_start: Option<usize> = None;
    let mut zero_remainder = 0.0f64;
    for index in 0..rounded.len() {
        if rounded[index].0 == 0 {
            if zero_start.is_none() {
                zero_start = Some(index);
            }
            zero_remainder += rounded[index].2;
            if zero_remainder >= ROUNDING_BREAKPOINT {
                if let Some(start) = zero_start {
                    rounded[start].0 = 1;
                }
                zero_start = None;
                zero_remainder = 0.0;
            }
        } else {
            zero_start = None;
            zero_remainder = 0.0;
        }
    }

    // Fit the plan length: round up what was rounded down (and the other way
    // round) from the end of the list.
    let mut plan = rounded.iter().map(|entry| entry.0).sum::<i64>();
    if plan != plan_length {
        for index in (0..rounded.len()).rev() {
            if rounded[index].1 == 2 && plan < plan_length {
                rounded[index].0 += 1;
                rounded[index].1 = 1;
                plan += 1;
            } else if rounded[index].1 == 1 && plan > plan_length {
                rounded[index].0 -= 1;
                rounded[index].1 = 2;
                plan -= 1;
            }
        }
    }

    rounded.iter().map(|entry| entry.0 as i32).collect()
}

/// Canvas `CoursePaceHardEndDateCompressor.shift_durations_down`.
fn shift_durations_down(durations: &[i32], days_over: i64) -> Vec<i32> {
    let mut result = durations.to_vec();
    let mut over = days_over;
    for index in (0..result.len()).rev() {
        while result[index] > 0 && over > 0 {
            result[index] -= 1;
            over -= 1;
        }
    }
    result
}

/// Canvas `CoursePacesDateHelpers.first_enabled_day`: the start of a plan
/// moves to the next enabled day when the start itself is not enabled.
fn adjusted_start(start: NaiveDate, week: &WorkWeek, holidays: &Holidays) -> NaiveDate {
    if is_business_day(start, week, holidays) {
        start
    } else {
        first_enabled_day(start, week, holidays)
    }
}

/// Canvas `CoursePaceHardEndDateCompressor.compress`: scale the durations so
/// that the plan ends on the hard end date.
fn compress_durations(
    durations: &[i32],
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
    week: &WorkWeek,
    holidays: &Holidays,
) -> Vec<i32> {
    if durations.is_empty() {
        return durations.to_vec();
    }

    let start = adjusted_start(start_date, week, holidays);
    let end_date = match end_date {
        Some(end) if !is_business_day(end, week, holidays) => {
            Some(previous_enabled_day(end, week, holidays))
        }
        other => other,
    };

    let due_dates = compute_due_dates(start, durations, week, holidays);
    let final_due_date = *due_dates.last().unwrap_or(&start);

    let Some(end_date) = end_date else {
        return durations.to_vec();
    };
    if final_due_date < end_date {
        return durations.to_vec();
    }

    let actual_plan_length = days_between(start, end_date, true, week, holidays);
    let plan_length_with_items = days_between(
        start,
        if final_due_date > start {
            final_due_date
        } else {
            start
        },
        true,
        week,
        holidays,
    );

    let percentage = if plan_length_with_items == 0 {
        0.0
    } else {
        actual_plan_length as f64 / plan_length_with_items as f64
    };
    let unrounded: Vec<f64> = durations
        .iter()
        .map(|duration| *duration as f64 * percentage)
        .collect();
    let mut rounded = round_durations(&unrounded, actual_plan_length);

    let new_due_dates = compute_due_dates(start, &rounded, week, holidays);
    let new_final = *new_due_dates.last().unwrap_or(&start);
    if new_final > end_date {
        let days_over = days_between(end_date, new_final, false, week, holidays);
        rounded = shift_durations_down(&rounded, days_over);
    }

    rounded
}

// ---------------------------------------------------------------------------
// Pace view and presenter
// ---------------------------------------------------------------------------

/// One pace module item as the presenter sees it.
#[derive(Debug, Clone)]
struct PaceItemView {
    id: Option<i64>,
    course_pace_id: Option<i64>,
    root_account_id: Option<i64>,
    duration: i32,
    module_item_id: i64,
    item_type: String,
    position: i32,
    title: Option<String>,
    published: bool,
    assignment_id: Option<i64>,
    points_possible: Option<f64>,
    module_id: i64,
    module_name: String,
    module_position: i32,
}

/// Everything the presenter needs about a pace (saved or not).
#[derive(Debug, Clone)]
struct PaceView {
    id: Option<i64>,
    course_id: i64,
    course_section_id: Option<i64>,
    user_id: Option<i64>,
    workflow_state: String,
    exclude_weekends: bool,
    selected_days_to_skip: Vec<String>,
    hard_end_dates: bool,
    created_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    published_at: Option<DateTime<Utc>>,
    root_account_id: Option<i64>,
    assignments_weighting: Value,
    time_to_complete_calendar_days: i32,
    start_date: Option<DateTime<Utc>>,
    start_date_context: String,
    end_date: Option<DateTime<Utc>>,
    end_date_context: String,
    items: Vec<PaceItemView>,
}

/// Canvas `ContentTag#content_type` of a module item.
fn canvas_content_type(item_type: &str) -> &str {
    match item_type {
        "page" | "wiki_page" => "WikiPage",
        "assignment" => "Assignment",
        "quiz" => "Quizzes::Quiz",
        "discussion" | "topic" => "DiscussionTopic",
        other => other,
    }
}

/// Canvas `TextHelper.round_if_whole`.
fn round_if_whole(value: Option<f64>) -> Value {
    match value {
        Some(value) if (value - value.round()).abs() < f64::EPSILON => json!(value as i64),
        Some(value) => json!(value),
        None => Value::Null,
    }
}

/// Canvas `CoursePacePresenter#assignments_weighting`.
fn assignments_weighting_json(raw: &str) -> Value {
    let value: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    match value {
        Value::Array(items) => {
            let mut map = Map::new();
            for item in items {
                let resource_type = item
                    .get("resource_type")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let duration = item.get("duration").cloned().unwrap_or(Value::Null);
                map.insert(resource_type, duration);
            }
            Value::Object(map)
        }
        Value::Object(map) => Value::Object(map),
        _ => Value::Object(Map::new()),
    }
}

/// Canvas `CoursePacePresenter#as_json`.
fn pace_json(view: &PaceView) -> Value {
    let mut modules: Vec<(i64, String, i32, Vec<Value>)> = Vec::new();
    for item in &view.items {
        let item_json = json!({
            "id": item.id,
            "duration": item.duration,
            "course_pace_id": item.course_pace_id,
            "root_account_id": item.root_account_id,
            "module_item_id": item.module_item_id,
            "assignment_title": item.title,
            "assignment_id": item.assignment_id,
            "points_possible": round_if_whole(item.points_possible),
            "assignment_link": format!(
                "/courses/{}/modules/items/{}",
                view.course_id, item.module_item_id
            ),
            "position": item.position,
            "module_item_type": canvas_content_type(&item.item_type),
            "published": item.published,
            "submission_status": "",
            "unreleased": Value::Null,
        });
        match modules.iter_mut().find(|entry| entry.0 == item.module_id) {
            Some(entry) => entry.3.push(item_json),
            None => modules.push((
                item.module_id,
                item.module_name.clone(),
                item.module_position,
                vec![item_json],
            )),
        }
    }

    let modules_json: Vec<Value> = modules
        .into_iter()
        .map(|(id, name, position, items)| {
            json!({
                "id": id,
                "name": name,
                "position": position,
                "items": items,
            })
        })
        .collect();

    let context_id = view
        .user_id
        .or(view.course_section_id)
        .unwrap_or(view.course_id);
    let context_type = if view.user_id.is_some() {
        "Enrollment"
    } else if view.course_section_id.is_some() {
        "Section"
    } else {
        "Course"
    };

    json!({
        "id": view.id,
        "course_id": view.course_id,
        "course_section_id": view.course_section_id,
        "user_id": view.user_id,
        "workflow_state": view.workflow_state,
        "exclude_weekends": view.exclude_weekends,
        "selected_days_to_skip": view.selected_days_to_skip,
        "hard_end_dates": view.hard_end_dates,
        "created_at": opt_ts(&view.created_at),
        "updated_at": opt_ts(&view.updated_at),
        "published_at": opt_ts(&view.published_at),
        "root_account_id": view.root_account_id,
        "modules": modules_json,
        "context_id": context_id,
        "assignments_weighting": view.assignments_weighting,
        "time_to_complete_calendar_days": view.time_to_complete_calendar_days,
        "context_type": context_type,
        "start_date": opt_ts(&view.start_date),
        "start_date_context": view.start_date_context,
        "end_date": opt_ts(&view.end_date),
        "end_date_context": view.end_date_context,
    })
}

/// Canvas `Api::V1::Progress#progress_json`.
fn progress_json(row: &ProgressRow) -> Value {
    json!({
        "id": row.id,
        "context_id": row.context_id,
        "context_type": row.context_type,
        "user_id": row.user_id,
        "tag": row.tag,
        "completion": row.completion,
        "workflow_state": row.workflow_state,
        "created_at": ts(&row.created_at),
        "updated_at": ts(&row.updated_at),
        "message": row.message,
        "url": format!("/api/v1/progress/{}", row.id),
    })
}

// ---------------------------------------------------------------------------
// Pace dates (Canvas `CoursePace#start_date` / `#effective_end_date`)
// ---------------------------------------------------------------------------

/// The start and end dates of a pace with their contexts.
struct PaceDates {
    start_date: Option<DateTime<Utc>>,
    start_date_context: String,
    end_date: Option<DateTime<Utc>>,
    end_date_context: String,
}

/// Canvas `CourseDateRange#valid_date_range`.
async fn valid_date_range(
    db: &PgPool,
    course: &CourseRow,
    settings: &Value,
) -> (
    Option<DateTime<Utc>>,
    Option<String>,
    Option<DateTime<Utc>>,
    Option<String>,
) {
    let term_dates: Option<(Option<DateTime<Utc>>, Option<DateTime<Utc>>)> =
        match course.enrollment_term_id {
            Some(term_id) => sqlx::query_as("SELECT start_at, end_at FROM terms WHERE id = $1")
                .bind(term_id)
                .fetch_optional(db)
                .await
                .ok()
                .flatten(),
            None => None,
        };

    let mut start = None;
    let mut start_context = None;
    let mut end = None;
    let mut end_context = None;

    if restrict_enrollments_to_course_dates(settings) {
        if let Some(date) = course.start_at {
            start = Some(date);
            start_context = Some("course".to_string());
        }
        if let Some(date) = course.end_at {
            end = Some(date);
            end_context = Some("course".to_string());
        }
    }

    if start.is_none() {
        if let Some((Some(term_start), _)) = term_dates {
            start = Some(term_start);
            start_context = Some("term".to_string());
        }
    }
    if end.is_none() {
        if let Some((_, Some(term_end))) = term_dates {
            end = Some(term_end);
            end_context = Some("term".to_string());
        }
    }

    (start, start_context, end, end_context)
}

/// Canvas `CoursePace#start_date(with_context: true)` and
/// `#effective_end_date(with_context: true)`.
async fn pace_dates(
    db: &PgPool,
    course: &CourseRow,
    section_id: Option<i64>,
    user_id: Option<i64>,
    hard_end_dates: bool,
    pace_end_date: Option<DateTime<Utc>>,
) -> PaceDates {
    let settings = course_settings_json(db, course.id).await;
    let (range_start, range_start_context, range_end, range_end_context) =
        valid_date_range(db, course, &settings).await;

    let section: Option<SectionRow> = match section_id {
        Some(section_id) => {
            sqlx::query_as("SELECT course_id, start_at, end_at FROM course_sections WHERE id = $1")
                .bind(section_id)
                .fetch_optional(db)
                .await
                .ok()
                .flatten()
        }
        None => None,
    };

    // The latest student enrollment of the pace's user. Canvas falls back to
    // the enrollment's `created_at` when `start_at` is nil.
    let enrollment: Option<(Option<DateTime<Utc>>, DateTime<Utc>, Option<i64>)> = match user_id {
        Some(user_id) => sqlx::query_as(
            "SELECT start_at, created_at, course_section_id FROM enrollments \
             WHERE user_id = $1 AND course_id = $2 AND workflow_state = 'active' \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(user_id)
        .bind(course.id)
        .fetch_optional(db)
        .await
        .ok()
        .flatten(),
        None => None,
    };
    let enrollment_start = enrollment
        .as_ref()
        .and_then(|(start_at, created_at, _)| start_at.or(Some(*created_at)));
    let enrollment_section_id = enrollment.as_ref().and_then(|(_, _, section)| *section);

    // Start date.
    let start_source = enrollment_start
        .or_else(|| section.as_ref().and_then(|section| section.start_at))
        .or(range_start);
    let (start_date, start_date_context) = match start_source {
        Some(date) => {
            let context = if enrollment.is_some() {
                "user".to_string()
            } else if section
                .as_ref()
                .and_then(|section| section.start_at)
                .is_some()
            {
                "section".to_string()
            } else {
                range_start_context
                    .clone()
                    .unwrap_or_else(|| "course".to_string())
            };
            (Some(date), context)
        }
        None => (
            Some(
                Utc::now()
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc))
                    .unwrap_or_else(Utc::now),
            ),
            "hypothetical".to_string(),
        ),
    };

    // End date.
    let range_end = range_end.map(|date| {
        if date.hour() == 0 && date.minute() == 0 {
            date - Duration::minutes(1)
        } else {
            date
        }
    });

    let is_student_plan = user_id.is_some() && enrollment.is_some();
    let section_end = section.as_ref().and_then(|section| section.end_at);
    // Canvas `individual_pace_end_date`: the end date of the section the
    // student is in.
    let individual_end = if is_student_plan {
        match enrollment_section_id {
            Some(section_id) => sqlx::query_as::<_, (Option<DateTime<Utc>>,)>(
                "SELECT end_at FROM course_sections WHERE id = $1",
            )
            .bind(section_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten()
            .and_then(|row| row.0),
            None => None,
        }
    } else {
        None
    };

    let end_source = if hard_end_dates {
        pace_end_date
    } else if is_student_plan {
        individual_end
    } else {
        section_end
    }
    .or(range_end);

    let end_date = end_source.map(|date| {
        let naive = date
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .expect("midnight of a valid date");
        DateTime::from_naive_utc_and_offset(naive, Utc)
    });

    let end_date_context = if is_student_plan && end_date.is_some() {
        "user".to_string()
    } else if section_end.is_some() {
        "section".to_string()
    } else if end_date.is_some() {
        if hard_end_dates {
            "hard".to_string()
        } else {
            range_end_context
                .clone()
                .unwrap_or_else(|| "course".to_string())
        }
    } else {
        "hypothetical".to_string()
    };

    PaceDates {
        start_date,
        start_date_context,
        end_date,
        end_date_context,
    }
}

/// The presenter view of a saved pace.
async fn pace_view(db: &PgPool, pace: &CoursePaceRow) -> Result<PaceView, ApiError> {
    let course = load_course(db, pace.course_id).await?;
    let items = load_pace_items(db, pace.id).await?;
    let dates = pace_dates(
        db,
        &course,
        pace.course_section_id,
        pace.user_id,
        pace.hard_end_dates,
        pace.end_date,
    )
    .await;

    let item_views = items
        .into_iter()
        .map(|item| PaceItemView {
            id: Some(item.id),
            course_pace_id: Some(item.course_pace_id),
            root_account_id: item.root_account_id,
            duration: item.duration,
            module_item_id: item.module_item_id,
            item_type: item.item_type.clone(),
            position: item.item_position,
            title: item.item_title.clone().or(item.assignment_title.clone()),
            published: item.item_workflow_state == "active",
            assignment_id: if item.item_type == "assignment" {
                Some(item.item_id)
            } else {
                None
            },
            points_possible: item.points_possible,
            module_id: item.module_id,
            module_name: item.module_name,
            module_position: item.module_position,
        })
        .collect();

    Ok(PaceView {
        id: Some(pace.id),
        course_id: pace.course_id,
        course_section_id: pace.course_section_id,
        user_id: pace.user_id,
        workflow_state: pace.workflow_state.clone(),
        exclude_weekends: pace.exclude_weekends,
        selected_days_to_skip: pace.selected_days_to_skip.clone(),
        hard_end_dates: pace.hard_end_dates,
        created_at: Some(pace.created_at),
        updated_at: Some(pace.updated_at),
        published_at: pace.published_at,
        root_account_id: pace.root_account_id,
        assignments_weighting: assignments_weighting_json(&pace.assignments_weighting),
        time_to_complete_calendar_days: pace.time_to_complete_calendar_days,
        start_date: dates.start_date,
        start_date_context: dates.start_date_context,
        end_date: dates.end_date,
        end_date_context: dates.end_date_context,
        items: item_views,
    })
}

/// The presenter view of an unsaved pace (the scaffold of `new`).
async fn unsaved_pace_view(
    db: &PgPool,
    course: &CourseRow,
    course_section_id: Option<i64>,
    user_id: Option<i64>,
    items: Vec<PaceItemRow>,
) -> Result<PaceView, ApiError> {
    let dates = pace_dates(db, course, course_section_id, user_id, false, None).await;

    let item_views = items
        .into_iter()
        .map(|item| PaceItemView {
            id: None,
            course_pace_id: None,
            root_account_id: None,
            duration: item.duration,
            module_item_id: item.module_item_id,
            item_type: item.item_type.clone(),
            position: item.item_position,
            title: item.item_title.clone().or(item.assignment_title.clone()),
            published: item.item_workflow_state == "active",
            assignment_id: if item.item_type == "assignment" {
                Some(item.item_id)
            } else {
                None
            },
            points_possible: item.points_possible,
            module_id: item.module_id,
            module_name: item.module_name,
            module_position: item.module_position,
        })
        .collect();

    Ok(PaceView {
        id: None,
        course_id: course.id,
        course_section_id,
        user_id,
        workflow_state: "unpublished".to_string(),
        exclude_weekends: false,
        selected_days_to_skip: Vec::new(),
        hard_end_dates: false,
        created_at: None,
        updated_at: None,
        published_at: None,
        root_account_id: None,
        assignments_weighting: Value::Object(Map::new()),
        time_to_complete_calendar_days: 0,
        start_date: dates.start_date,
        start_date_context: dates.start_date_context,
        end_date: dates.end_date,
        end_date_context: dates.end_date_context,
        items: item_views,
    })
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// Create a `progress` row (Canvas `Progress.create!`).
async fn create_progress(
    db: &PgPool,
    tag: &str,
    context_type: &str,
    context_id: i64,
    user_id: i64,
) -> Result<ProgressRow, ApiError> {
    let row: ProgressRow = sqlx::query_as(
        "INSERT INTO progress (tag, workflow_state, completion, context_type, context_id, user_id) \
         VALUES ($1, 'queued', 0, $2, $3, $4) \
         RETURNING id, tag, workflow_state, completion, context_type, context_id, user_id, \
                   message, created_at, updated_at",
    )
    .bind(tag)
    .bind(context_type)
    .bind(context_id)
    .bind(user_id)
    .fetch_one(db)
    .await?;
    Ok(row)
}

/// The latest publish progress of a pace (Canvas `latest_progress`): a
/// completed progress is not reported anymore.
async fn latest_progress(db: &PgPool, pace_id: i64) -> Result<Option<ProgressRow>, ApiError> {
    let row: Option<ProgressRow> = sqlx::query_as(
        "SELECT id, tag, workflow_state, completion, context_type, context_id, user_id, \
                message, created_at, updated_at \
         FROM progress \
         WHERE context_type = 'CoursePace' AND context_id = $1 AND tag = 'course_pace_publish' \
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(pace_id)
    .fetch_optional(db)
    .await?;

    Ok(match row {
        Some(row) if row.workflow_state == "completed" => None,
        other => other,
    })
}

// ---------------------------------------------------------------------------
// Publishing
// ---------------------------------------------------------------------------

/// The student enrollments a pace applies to (Canvas
/// `CoursePace#student_enrollments`).
async fn pace_student_enrollments(
    db: &PgPool,
    pace: &CoursePaceRow,
) -> Result<Vec<(i64, i64)>, ApiError> {
    let rows: Vec<(i64, i64)> = if let Some(user_id) = pace.user_id {
        sqlx::query_as(
            "SELECT id, user_id FROM enrollments \
             WHERE course_id = $1 AND user_id = $2 AND role = 'student' \
               AND workflow_state = 'active'",
        )
        .bind(pace.course_id)
        .bind(user_id)
        .fetch_all(db)
        .await?
    } else if let Some(section_id) = pace.course_section_id {
        sqlx::query_as(
            "SELECT e.id, e.user_id FROM enrollments e \
             WHERE e.course_id = $1 AND e.course_section_id = $2 AND e.role = 'student' \
               AND e.workflow_state = 'active' \
               AND NOT EXISTS (SELECT 1 FROM course_paces cp \
                               WHERE cp.course_id = e.course_id AND cp.user_id = e.user_id \
                                 AND cp.workflow_state <> 'deleted')",
        )
        .bind(pace.course_id)
        .bind(section_id)
        .fetch_all(db)
        .await?
    } else {
        sqlx::query_as(
            "SELECT e.id, e.user_id FROM enrollments e \
             WHERE e.course_id = $1 AND e.role = 'student' AND e.workflow_state = 'active' \
               AND NOT EXISTS (SELECT 1 FROM course_paces cp \
                               WHERE cp.course_id = e.course_id AND cp.user_id = e.user_id \
                                 AND cp.workflow_state <> 'deleted') \
               AND NOT EXISTS (SELECT 1 FROM course_paces cp \
                               WHERE cp.course_id = e.course_id \
                                 AND cp.course_section_id = e.course_section_id \
                                 AND cp.workflow_state <> 'deleted')",
        )
        .bind(pace.course_id)
        .fetch_all(db)
        .await?
    };

    Ok(rows)
}

/// Write the paced due date of one student for one assignment (Canvas
/// `CoursePace#publish` creates an ad-hoc assignment override).
async fn write_paced_override(
    db: &PgPool,
    assignment_id: i64,
    user_id: i64,
    due_at: DateTime<Utc>,
) -> Result<(), ApiError> {
    let existing: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM assignment_overrides \
         WHERE assignment_id = $1 AND workflow_state <> 'deleted' \
           AND due_at = $2 AND $3 = ANY(student_ids) LIMIT 1",
    )
    .bind(assignment_id)
    .bind(due_at)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    if existing.is_some() {
        return Ok(());
    }

    let same_date: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM assignment_overrides \
         WHERE assignment_id = $1 AND workflow_state <> 'deleted' AND due_at = $2 LIMIT 1",
    )
    .bind(assignment_id)
    .bind(due_at)
    .fetch_optional(db)
    .await?;
    if let Some((override_id,)) = same_date {
        sqlx::query(
            "UPDATE assignment_overrides \
             SET student_ids = array_append(student_ids, $1), updated_at = now() \
             WHERE id = $2",
        )
        .bind(user_id)
        .bind(override_id)
        .execute(db)
        .await?;
        return Ok(());
    }

    let for_user: Option<(i64,)> = sqlx::query_as(
        "SELECT id FROM assignment_overrides \
         WHERE assignment_id = $1 AND workflow_state <> 'deleted' \
           AND $2 = ANY(student_ids) LIMIT 1",
    )
    .bind(assignment_id)
    .bind(user_id)
    .fetch_optional(db)
    .await?;
    if let Some((override_id,)) = for_user {
        sqlx::query(
            "UPDATE assignment_overrides SET due_at = $1, updated_at = now() WHERE id = $2",
        )
        .bind(due_at)
        .bind(override_id)
        .execute(db)
        .await?;
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO assignment_overrides (assignment_id, title, due_at, student_ids, workflow_state) \
         VALUES ($1, 'Course Pacing', $2, ARRAY[$3]::bigint[], 'active')",
    )
    .bind(assignment_id)
    .bind(due_at)
    .bind(user_id)
    .execute(db)
    .await?;

    Ok(())
}

/// Canvas `CoursePaceHardEndDateCompressor`: the hard end date of a pace is
/// its own `end_date`, else the end of the course, else the end of the
/// enrollment term.
async fn course_or_term_end_date(db: &PgPool, course: &CourseRow) -> Option<NaiveDate> {
    if let Some(end_at) = course.end_at {
        return Some(end_at.date_naive());
    }
    let term_id = course.enrollment_term_id?;
    let row: Option<(Option<DateTime<Utc>>,)> =
        sqlx::query_as::<_, (Option<DateTime<Utc>>,)>("SELECT end_at FROM terms WHERE id = $1")
            .bind(term_id)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    row.and_then(|(end_at,)| end_at.map(|date| date.date_naive()))
}

/// The publish work of the delayed job (Canvas `CoursePace#publish`): pace
/// the students of the pace and mark the pace as published.
async fn run_publish_work(db: &PgPool, pace: &CoursePaceRow) -> Result<(), ApiError> {
    let items = load_pace_items(db, pace.id).await?;
    let assignment_items: Vec<(i64, i32)> = items
        .iter()
        .filter(|item| item.item_type == "assignment")
        .map(|item| (item.item_id, item.duration))
        .collect();
    if assignment_items.is_empty() {
        mark_pace_published(db, pace.id).await?;
        return Ok(());
    }

    let course = load_course(db, pace.course_id).await?;
    let root_account = root_account_id(db, &course).await;
    let week = pace_work_week(
        db,
        root_account,
        pace.exclude_weekends,
        &pace.selected_days_to_skip,
    )
    .await;
    let holidays = course_blackout_dates(db, pace.course_id).await;

    let dates = pace_dates(
        db,
        &course,
        pace.course_section_id,
        pace.user_id,
        pace.hard_end_dates,
        pace.end_date,
    )
    .await;
    let start = dates
        .start_date
        .map(|date| date.date_naive())
        .unwrap_or_else(|| Utc::now().date_naive());
    // The plan starts counting on an enabled day.
    let start = adjusted_start(start, &week, &holidays);

    let durations: Vec<i32> = assignment_items
        .iter()
        .map(|(_, duration)| *duration)
        .collect();

    for (_enrollment_id, user_id) in pace_student_enrollments(db, pace).await? {
        let due_dates = compute_due_dates(start, &durations, &week, &holidays);
        for (index, (assignment_id, _)) in assignment_items.iter().enumerate() {
            let due_at = due_dates[index]
                .and_hms_opt(0, 0, 0)
                .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc));
            if let Some(due_at) = due_at {
                write_paced_override(db, *assignment_id, user_id, due_at).await?;
            }
        }
    }

    mark_pace_published(db, pace.id).await?;
    Ok(())
}

/// Canvas `CoursePace#publish` ends with `update(workflow_state: "active",
/// published_at: Time.zone.now)`.
async fn mark_pace_published(db: &PgPool, pace_id: i64) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE course_paces \
         SET workflow_state = 'active', published_at = now(), updated_at = now() \
         WHERE id = $1",
    )
    .bind(pace_id)
    .execute(db)
    .await?;
    Ok(())
}

/// Canvas `publish_course_pace`: queue the publish progress and run the
/// publish work.
async fn publish_course_pace(
    db: &PgPool,
    pace: &CoursePaceRow,
    auth: &AuthUser,
) -> Result<ProgressRow, ApiError> {
    let progress =
        create_progress(db, "course_pace_publish", "CoursePace", pace.id, auth.id).await?;
    run_publish_work(db, pace).await?;
    Ok(progress)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /courses/{courseId}/course_pacing — Canvas `create`.
async fn create_course_pace(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<PaceQuery>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (_context, course) = resolve_context(&state.db, course_id, &query).await?;
    require_feature_flag(&state.db, course.id).await?;
    require_content_right(&state.db, &auth, course.id, ContentRight::Add).await?;

    let body = body.map(|Json(value)| value).unwrap_or(Value::Null);
    if !body.is_object() {
        return Err(ApiError::BadRequest(
            "param is missing or the value is empty: course_pace".to_string(),
        ));
    }
    let params = parse_pace_params(&body);

    // Canvas validations: `valid_secondary_context` and the assignable
    // module item of every nested record.
    let mut errors: Vec<String> = Vec::new();
    if params.course_section_id.is_some() && params.user_id.is_some() {
        errors.push("Only one of course_section_id and user_id can be given".to_string());
    }
    if let Some(message) = first_unassignable_item(&state.db, &params).await? {
        errors.push(message);
    }
    if !errors.is_empty() {
        return Ok(unprocessable(json!(errors)));
    }

    let root_account = root_account_id(&state.db, &course).await;
    let workflow_state = params
        .workflow_state
        .clone()
        .unwrap_or_else(|| "unpublished".to_string());
    let selected_days_to_skip = params.selected_days_to_skip.clone().unwrap_or_default();
    let assignments_weighting = params
        .assignments_weighting
        .clone()
        .map(|value| value.to_string())
        .unwrap_or_else(|| "[]".to_string());

    let pace_id: i64 = sqlx::query_scalar(
        "INSERT INTO course_paces (course_id, course_section_id, user_id, root_account_id, \
             workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates, end_date, \
             assignments_weighting, time_to_complete_calendar_days) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
    )
    .bind(course.id)
    .bind(params.course_section_id)
    .bind(params.user_id)
    .bind(root_account)
    .bind(&workflow_state)
    .bind(params.exclude_weekends.unwrap_or(false))
    .bind(&selected_days_to_skip)
    .bind(params.hard_end_dates.unwrap_or(false))
    .bind(params.end_date)
    .bind(&assignments_weighting)
    .bind(params.time_to_complete_calendar_days.unwrap_or(0))
    .fetch_one(&state.db)
    .await?;

    replace_pace_items(&state.db, pace_id, &params, root_account).await?;

    let pace = load_pace(&state.db, course.id, pace_id).await?;
    let course_pace_json = pace_json(&pace_view(&state.db, &pace).await?);

    // Canvas publishes the pace unless the root account allows draft paces
    // and the request did not ask for an active pace.
    let draft_state =
        account_feature_enabled(&state.db, root_account, "course_pace_draft_state").await;
    let should_publish = if draft_state {
        params.workflow_state.as_deref() == Some("active")
    } else {
        true
    };

    let progress = if should_publish {
        Some(publish_course_pace(&state.db, &pace, &auth).await?)
    } else {
        None
    };

    Ok((
        StatusCode::OK,
        Json(json!({
            "course_pace": course_pace_json,
            "progress": progress.as_ref().map(progress_json),
        })),
    ))
}

/// GET /courses/{courseId}/course_pacing/new — Canvas `new`.
async fn new_course_pace(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<PaceQuery>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (context, course) = resolve_context(&state.db, course_id, &query).await?;
    require_feature_flag(&state.db, course.id).await?;
    require_content_right(&state.db, &auth, course.id, ContentRight::Any).await?;

    let progress = match pace_for_context(&state.db, course.id, context, false).await? {
        Some(pace) => latest_progress(&state.db, pace.id).await?,
        None => None,
    };

    if let Some(pace) = pace_for_context(&state.db, course.id, context, true).await? {
        let view = pace_view(&state.db, &pace).await?;
        return Ok((
            StatusCode::OK,
            Json(json!({
                "course_pace": pace_json(&view),
                "progress": progress.as_ref().map(progress_json),
            })),
        ));
    }

    // Duplicate a published pace of the context (or of the course).
    let (section_id, user_id) = context.scope(&state.db).await?;
    let mut source = scoped_pace(&state.db, course.id, section_id, user_id, true).await?;
    if source.is_none() {
        if let PaceContext::Enrollment(enrollment_id) = context {
            let enrollment_section: Option<(Option<i64>,)> =
                sqlx::query_as("SELECT course_section_id FROM enrollments WHERE id = $1")
                    .bind(enrollment_id)
                    .fetch_optional(&state.db)
                    .await?;
            if let Some((Some(enrollment_section_id),)) = enrollment_section {
                source = scoped_pace(
                    &state.db,
                    course.id,
                    Some(enrollment_section_id),
                    None,
                    true,
                )
                .await?;
            }
        }
    }
    if source.is_none() {
        source = scoped_pace(&state.db, course.id, None, None, true).await?;
    }

    let items = match &source {
        Some(source) => load_pace_items(&state.db, source.id).await?,
        None => load_course_assignment_items(&state.db, course.id).await?,
    };

    let (view_section, view_user) = match context {
        PaceContext::Course => (None, None),
        PaceContext::Section(section_id) => (Some(section_id), None),
        PaceContext::Enrollment(_) => (None, user_id),
    };

    let view = unsaved_pace_view(&state.db, &course, view_section, view_user, items).await?;
    Ok((
        StatusCode::OK,
        Json(json!({
            "course_pace": pace_json(&view),
            "progress": progress.as_ref().map(progress_json),
        })),
    ))
}

/// GET /courses/{courseId}/course_pacing/{id} — Canvas `api_show`.
async fn show_course_pace(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    Query(query): Query<PaceQuery>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (_context, course) = resolve_context(&state.db, course_id, &query).await?;
    require_feature_flag(&state.db, course.id).await?;
    let pace = load_pace(&state.db, course.id, id).await?;
    require_content_right(&state.db, &auth, course.id, ContentRight::Any).await?;

    let view = pace_view(&state.db, &pace).await?;
    let progress = latest_progress(&state.db, pace.id).await?;

    Ok((
        StatusCode::OK,
        Json(json!({
            "course_pace": pace_json(&view),
            "progress": progress.as_ref().map(progress_json),
        })),
    ))
}

/// PUT /courses/{courseId}/course_pacing/{id} — Canvas `update`.
async fn update_course_pace(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    Query(query): Query<PaceQuery>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (_context, course) = resolve_context(&state.db, course_id, &query).await?;
    require_feature_flag(&state.db, course.id).await?;
    let pace = load_pace(&state.db, course.id, id).await?;
    require_content_right(&state.db, &auth, course.id, ContentRight::Edit).await?;

    let body = body.map(|Json(value)| value).unwrap_or(Value::Null);
    if !body.is_object() {
        return Err(ApiError::BadRequest(
            "param is missing or the value is empty: course_pace".to_string(),
        ));
    }
    let mut params = parse_pace_params(&body);

    let root_account = root_account_id(&state.db, &course).await;
    let draft_state =
        account_feature_enabled(&state.db, root_account, "course_pace_draft_state").await;
    let mut should_publish = false;
    if draft_state
        && (pace.workflow_state == "active" || params.workflow_state.as_deref() == Some("active"))
    {
        params.workflow_state = Some("active".to_string());
        should_publish = true;
    }

    let mut errors: Vec<String> = Vec::new();
    let next_section = params.course_section_id.or(pace.course_section_id);
    let next_user = params.user_id.or(pace.user_id);
    if next_section.is_some() && next_user.is_some() {
        errors.push("Only one of course_section_id and user_id can be given".to_string());
    }
    if let Some(message) = first_unassignable_item(&state.db, &params).await? {
        errors.push(message);
    }
    if !errors.is_empty() {
        return Ok(unprocessable(json!(errors)));
    }

    let workflow_state = params
        .workflow_state
        .clone()
        .unwrap_or_else(|| pace.workflow_state.clone());
    let selected_days_to_skip = params
        .selected_days_to_skip
        .clone()
        .unwrap_or_else(|| pace.selected_days_to_skip.clone());
    let assignments_weighting = match &params.assignments_weighting {
        Some(value) => value.to_string(),
        None => pace.assignments_weighting.clone(),
    };

    sqlx::query(
        "UPDATE course_paces SET course_section_id = $1, user_id = $2, workflow_state = $3, \
             exclude_weekends = $4, selected_days_to_skip = $5, hard_end_dates = $6, \
             end_date = $7, assignments_weighting = $8, time_to_complete_calendar_days = $9, \
             updated_at = now() \
         WHERE id = $10",
    )
    .bind(next_section)
    .bind(next_user)
    .bind(&workflow_state)
    .bind(params.exclude_weekends.unwrap_or(pace.exclude_weekends))
    .bind(&selected_days_to_skip)
    .bind(params.hard_end_dates.unwrap_or(pace.hard_end_dates))
    .bind(params.end_date.or(pace.end_date))
    .bind(&assignments_weighting)
    .bind(
        params
            .time_to_complete_calendar_days
            .unwrap_or(pace.time_to_complete_calendar_days),
    )
    .bind(pace.id)
    .execute(&state.db)
    .await?;

    replace_pace_items(&state.db, pace.id, &params, root_account).await?;

    let updated = load_pace(&state.db, course.id, pace.id).await?;
    let course_pace_json = pace_json(&pace_view(&state.db, &updated).await?);

    let progress = if should_publish || updated.workflow_state == "active" {
        Some(publish_course_pace(&state.db, &updated, &auth).await?)
    } else {
        None
    };

    Ok((
        StatusCode::OK,
        Json(json!({
            "course_pace": course_pace_json,
            "progress": progress.as_ref().map(progress_json),
        })),
    ))
}

/// DELETE /courses/{courseId}/course_pacing/{id} — Canvas `destroy`.
async fn destroy_course_pace(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    Query(query): Query<PaceQuery>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (_context, course) = resolve_context(&state.db, course_id, &query).await?;
    require_feature_flag(&state.db, course.id).await?;
    let pace = load_pace(&state.db, course.id, id).await?;
    require_content_right(&state.db, &auth, course.id, ContentRight::Delete).await?;

    let published = pace.workflow_state == "active";
    let primary = pace.course_section_id.is_none() && pace.user_id.is_none();
    if primary && published {
        return Ok(unprocessable(json!(
            "You cannot delete the default course pace."
        )));
    }

    sqlx::query(
        "UPDATE course_paces SET workflow_state = 'deleted', updated_at = now() WHERE id = $1",
    )
    .bind(pace.id)
    .execute(&state.db)
    .await?;

    // Canvas republishes the paces of the affected enrollments.
    if published {
        republish_affected_enrollments(&state.db, &pace, &auth).await?;
    }

    let mut view = pace_view(&state.db, &pace).await?;
    view.workflow_state = "deleted".to_string();
    Ok((
        StatusCode::OK,
        Json(json!({ "course_pace": pace_json(&view) })),
    ))
}

/// POST /courses/{courseId}/course_pacing/{id}/publish — Canvas `publish`.
async fn publish_course_pace_route(
    State(state): State<AppState>,
    Path((course_id, id)): Path<(i64, i64)>,
    Query(query): Query<PaceQuery>,
    auth: AuthUser,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (_context, course) = resolve_context(&state.db, course_id, &query).await?;
    require_feature_flag(&state.db, course.id).await?;
    let pace = load_pace(&state.db, course.id, id).await?;
    require_content_right(&state.db, &auth, course.id, ContentRight::Edit).await?;

    let progress = publish_course_pace(&state.db, &pace, &auth).await?;
    Ok((StatusCode::OK, Json(progress_json(&progress))))
}

/// POST /courses/{courseId}/course_pacing/compress_dates — Canvas
/// `compress_dates`.
async fn compress_dates(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<PaceQuery>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let (_context, course) = resolve_context(&state.db, course_id, &query).await?;
    require_feature_flag(&state.db, course.id).await?;
    require_content_right(&state.db, &auth, course.id, ContentRight::Edit).await?;

    let body = body.map(|Json(value)| value).unwrap_or(Value::Null);
    if !body.is_object() {
        return Err(ApiError::BadRequest(
            "param is missing or the value is empty: course_pace".to_string(),
        ));
    }
    let params = parse_pace_params(&body);

    let mut errors: Vec<String> = Vec::new();
    if params.course_section_id.is_some() && params.user_id.is_some() {
        errors.push("Only one of course_section_id and user_id can be given".to_string());
    }
    if let Some(message) = first_unassignable_item(&state.db, &params).await? {
        errors.push(message);
    }
    if !errors.is_empty() {
        return Ok(unprocessable(json!(errors)));
    }

    let root_account = root_account_id(&state.db, &course).await;
    let week = pace_work_week(
        &state.db,
        root_account,
        params.exclude_weekends.unwrap_or(false),
        &params.selected_days_to_skip.clone().unwrap_or_default(),
    )
    .await;

    let mut holidays = course_blackout_dates(&state.db, course.id).await;
    holidays.extend(request_blackout_dates(&body));

    // The start date of the compression: the request wins, otherwise the
    // start date of the pace.
    let dates = pace_dates(
        &state.db,
        &course,
        params.course_section_id,
        params.user_id,
        params.hard_end_dates.unwrap_or(false),
        params.end_date,
    )
    .await;
    let start = params
        .start_date
        .map(|date| date.date_naive())
        .or(dates.start_date.map(|date| date.date_naive()))
        .unwrap_or_else(|| Utc::now().date_naive());

    if let Some(end_date) = params.end_date {
        if end_date.date_naive() < start {
            return Ok(unprocessable(json!("End date cannot be before start date")));
        }
    }

    let durations: Vec<i32> = params
        .module_items
        .iter()
        .map(|item| item.duration.unwrap_or(0))
        .collect();
    // The end date of the plan: the one of the request, otherwise the one of
    // the course or its enrollment term.
    let end_date = match params.end_date.map(|date| date.date_naive()) {
        Some(date) => Some(date),
        None => course_or_term_end_date(&state.db, &course).await,
    };
    let compressed = compress_durations(&durations, start, end_date, &week, &holidays);

    // The due dates of the compressed plan start on the first enabled day.
    let due_dates = compute_due_dates(
        adjusted_start(start, &week, &holidays),
        &compressed,
        &week,
        &holidays,
    );
    let mut result = Map::new();
    for (index, item) in params.module_items.iter().enumerate() {
        let Some(module_item_id) = item.module_item_id else {
            continue;
        };
        let due = due_dates[index]
            .and_hms_opt(0, 0, 0)
            .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc));
        if let Some(due) = due {
            result.insert(module_item_id.to_string(), json!(ts(&due)));
        }
    }

    Ok((StatusCode::OK, Json(Value::Object(result))))
}

/// POST /courses/{courseId}/course_pacing/bulk_create_enrollment_paces —
/// Canvas `bulk_create_enrollment_paces`.
async fn bulk_create_enrollment_paces(
    State(state): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<PaceQuery>,
    auth: AuthUser,
    body: Option<Json<Value>>,
) -> Result<StatusCode, ApiError> {
    let (_context, course) = resolve_context(&state.db, course_id, &query).await?;
    require_feature_flag(&state.db, course.id).await?;
    require_content_right(&state.db, &auth, course.id, ContentRight::Edit).await?;

    let body = body.map(|Json(value)| value).unwrap_or(Value::Null);
    if !body.is_object() {
        return Err(ApiError::BadRequest(
            "param is missing or the value is empty: course_pace".to_string(),
        ));
    }
    let params = parse_pace_params(&body);
    let enrollment_ids = parse_enrollment_ids(&body);

    // Canvas `run_bulk_assign_enrollment_paces_delayed_job` creates the
    // progress of the bulk job.
    create_progress(&state.db, "bulk_assign_paces", "Course", course.id, auth.id).await?;

    let root_account = root_account_id(&state.db, &course).await;
    for enrollment_id in enrollment_ids {
        let enrollment: Option<(i64, i64)> =
            sqlx::query_as("SELECT user_id, course_id FROM enrollments WHERE id = $1")
                .bind(enrollment_id)
                .fetch_optional(&state.db)
                .await?;
        let Some((user_id, enrollment_course_id)) = enrollment else {
            continue;
        };
        if enrollment_course_id != course.id {
            continue;
        }
        // Canvas builds the pace with the user of the enrollment; a section
        // in the bulk params would make the pace invalid (`valid_secondary_context`).
        if params.course_section_id.is_some() {
            continue;
        }

        let pace_id: i64 = sqlx::query_scalar(
            "INSERT INTO course_paces (course_id, course_section_id, user_id, root_account_id, \
                 workflow_state, exclude_weekends, selected_days_to_skip, hard_end_dates, \
                 end_date, assignments_weighting, time_to_complete_calendar_days) \
             VALUES ($1, NULL, $2, $3, 'active', $4, $5, $6, $7, $8, $9) RETURNING id",
        )
        .bind(course.id)
        .bind(user_id)
        .bind(root_account)
        .bind(params.exclude_weekends.unwrap_or(false))
        .bind(&params.selected_days_to_skip.clone().unwrap_or_default())
        .bind(params.hard_end_dates.unwrap_or(false))
        .bind(params.end_date)
        .bind(
            params
                .assignments_weighting
                .clone()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "[]".to_string()),
        )
        .bind(params.time_to_complete_calendar_days.unwrap_or(0))
        .fetch_one(&state.db)
        .await?;

        replace_pace_items(&state.db, pace_id, &params, root_account).await?;

        let pace = load_pace(&state.db, course.id, pace_id).await?;
        publish_course_pace(&state.db, &pace, &auth).await?;
    }

    // Canvas renders nothing here.
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Helpers used by the handlers
// ---------------------------------------------------------------------------

/// The first nested module item that is not an assignment (Canvas
/// `CoursePaceModuleItem#assignable_module_item`).
async fn first_unassignable_item(
    db: &PgPool,
    params: &PaceParams,
) -> Result<Option<String>, ApiError> {
    for item in &params.module_items {
        let Some(module_item_id) = item.module_item_id else {
            continue;
        };
        let item_type: Option<(String,)> =
            sqlx::query_as("SELECT item_type FROM module_items WHERE id = $1")
                .bind(module_item_id)
                .fetch_optional(db)
                .await?;
        match item_type {
            Some((item_type,)) if item_type == "assignment" => {}
            _ => return Ok(Some("Module item is not assignable".to_string())),
        }
    }
    Ok(None)
}

/// Replace the module items of a pace (Rails' nested attributes replace the
/// collection of a has_many).
async fn replace_pace_items(
    db: &PgPool,
    pace_id: i64,
    params: &PaceParams,
    root_account: Option<i64>,
) -> Result<(), ApiError> {
    if !params.module_items_given {
        return Ok(());
    }

    let existing: Vec<(i64,)> =
        sqlx::query_as("SELECT id FROM course_pace_module_items WHERE course_pace_id = $1")
            .bind(pace_id)
            .fetch_all(db)
            .await?;

    let mut kept: Vec<i64> = Vec::new();
    for item in &params.module_items {
        let duration = item.duration.unwrap_or(0);
        match item.id {
            Some(id) if existing.iter().any(|(existing_id,)| *existing_id == id) => {
                sqlx::query(
                    "UPDATE course_pace_module_items \
                     SET module_item_id = COALESCE($1, module_item_id), duration = $2, \
                         root_account_id = COALESCE($3, root_account_id), updated_at = now() \
                     WHERE id = $4 AND course_pace_id = $5",
                )
                .bind(item.module_item_id)
                .bind(duration)
                .bind(item.root_account_id.or(root_account))
                .bind(id)
                .bind(pace_id)
                .execute(db)
                .await?;
                kept.push(id);
            }
            _ => {
                let Some(module_item_id) = item.module_item_id else {
                    continue;
                };
                let new_id: i64 = sqlx::query_scalar(
                    "INSERT INTO course_pace_module_items \
                         (course_pace_id, module_item_id, root_account_id, duration) \
                     VALUES ($1, $2, $3, $4) RETURNING id",
                )
                .bind(pace_id)
                .bind(module_item_id)
                .bind(item.root_account_id.or(root_account))
                .bind(duration)
                .fetch_one(db)
                .await?;
                kept.push(new_id);
            }
        }
    }

    for (id,) in existing {
        if !kept.contains(&id) {
            sqlx::query("DELETE FROM course_pace_module_items WHERE id = $1")
                .bind(id)
                .execute(db)
                .await?;
        }
    }

    Ok(())
}

/// Canvas `CoursePace#republish_paces_for_affected_enrollments`: the pace
/// that takes over for the deleted one is published again.
async fn republish_affected_enrollments(
    db: &PgPool,
    pace: &CoursePaceRow,
    auth: &AuthUser,
) -> Result<(), ApiError> {
    let fallback = scoped_pace(db, pace.course_id, None, None, true).await?;
    let Some(fallback) = fallback else {
        return Ok(());
    };
    if fallback.id == pace.id {
        return Ok(());
    }
    publish_course_pace(db, &fallback, auth).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

/// The Canvas course pacing routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/courses/{courseId}/course_pacing",
            post(create_course_pace),
        )
        .route(
            "/courses/{courseId}/course_pacing/new",
            get(new_course_pace),
        )
        .route(
            "/courses/{courseId}/course_pacing/compress_dates",
            post(compress_dates),
        )
        .route(
            "/courses/{courseId}/course_pacing/bulk_create_enrollment_paces",
            post(bulk_create_enrollment_paces),
        )
        .route(
            "/courses/{courseId}/course_pacing/{id}",
            get(show_course_pace)
                .put(update_course_pace)
                .delete(destroy_course_pace),
        )
        .route(
            "/courses/{courseId}/course_pacing/{id}/publish",
            post(publish_course_pace_route),
        )
}
