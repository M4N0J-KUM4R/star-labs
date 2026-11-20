//! Assignments journey handlers (issue #71) — Canvas `assignments#index` /
//! `assignments#show` / new / edit / submit, server-rendered.
//!
//! Every handler gates through [`crate::handlers::course_context`]
//! (`star_courses::load_course_for_viewer`, 404-before-403), so the pages
//! cannot bypass `/api/v1` course authorization. Teachers additionally pass
//! the `is_teacher` check (star_courses::check_teacher_or_admin) for the
//! write paths (new, edit); students get their own submission state pills
//! and the text-entry submit form.

use askama::Template;
use axum::extract::{Form, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use chrono::{NaiveDate, TimeZone, Utc};
use serde::Deserialize;

use star_assignments::{
    create_assignment_record, load_assignment_submissions, update_assignment_record,
    upsert_own_text_submission, AssignmentRecordUpdate, NewAssignmentRecord, UiAssignment,
};
use star_common::{ApiError, AppState};

use crate::assets;
use crate::handlers::course_context::{course_breadcrumb, course_context, course_env, ensure_csrf};
use crate::handlers::error::from_api_error;
use crate::session::{check_csrf, SessionUser};

/// Presentation-ready row of the assignments index. All display strings are
/// precomputed here so the template only does field access (issue #71).
pub struct AssignmentRowView {
    pub id: i64,
    pub title: String,
    /// "Due Jan 5, 2026" / "No due date".
    pub due_label: String,
    /// "10 pts".
    pub points_label: String,
    /// "graded" | "submitted" | "unsubmitted".
    pub pill_class: String,
    /// "Graded" | "Submitted" | "Not Submitted".
    pub pill_label: String,
    pub published: bool,
}

impl AssignmentRowView {
    fn from_assignment(a: &UiAssignment) -> Self {
        let (pill_class, pill_label) = pill(&a.submission_state);
        let due_label = a
            .due_at
            .map(|d| format!("Due {}", d.format("%b %-d, %Y")))
            .unwrap_or_else(|| "No due date".to_string());
        AssignmentRowView {
            id: a.id,
            title: a.title.clone(),
            due_label,
            points_label: format!("{} pts", trim_f64(a.points_possible)),
            pill_class: pill_class.to_string(),
            pill_label: pill_label.to_string(),
            published: a.workflow_state == "published",
        }
    }
}

/// A group bucket on the assignments index: every non-deleted assignment of
/// the course, keyed by (group position, group id, group name). Groupless
/// assignments land in the trailing "Ungrouped" bucket (Canvas'
/// `require_assignment_group` keeps a default group, but rows predating that
/// or created without one still render).
pub struct AssignmentGroupView {
    pub group_id: Option<i64>,
    pub name: String,
    pub assignments: Vec<AssignmentRowView>,
    /// "3 assignments" / "1 assignment".
    pub count_label: String,
}

/// Bucket assignments by group, ordered by group position then insertion
/// order. Known-but-empty groups are dropped unless they are the only
/// bucket content.
pub(crate) fn group_assignments(
    rows: Vec<UiAssignment>,
    groups: &[(i64, String, i32)],
) -> Vec<AssignmentGroupView> {
    let mut views: Vec<AssignmentGroupView> = groups
        .iter()
        .map(|(id, name, _)| AssignmentGroupView {
            group_id: Some(*id),
            name: name.clone(),
            assignments: Vec::new(),
            count_label: String::new(),
        })
        .collect();

    for row in rows {
        let slot = match row.assignment_group_id {
            Some(gid) => views.iter_mut().find(|v| v.group_id == Some(gid)),
            None => None,
        };
        match slot {
            Some(view) => view
                .assignments
                .push(AssignmentRowView::from_assignment(&row)),
            None => {
                // Groupless (or dangling group id): the trailing bucket.
                if let Some(last) = views.last_mut() {
                    if last.group_id.is_none() {
                        last.assignments
                            .push(AssignmentRowView::from_assignment(&row));
                        continue;
                    }
                }
                views.push(AssignmentGroupView {
                    group_id: None,
                    name: "Ungrouped".to_string(),
                    assignments: vec![AssignmentRowView::from_assignment(&row)],
                    count_label: String::new(),
                });
            }
        }
    }

    // Keep empty buckets only when every bucket is empty (one "no
    // assignments" message beats a stack of empty group cards).
    if views.iter().any(|v| !v.assignments.is_empty()) {
        views.retain(|v| !v.assignments.is_empty() || v.group_id.is_none());
    }
    for view in views.iter_mut() {
        let n = view.assignments.len();
        view.count_label = if n == 1 {
            "1 assignment".to_string()
        } else {
            format!("{n} assignments")
        };
    }
    views
}

/// The submission state pill (class, label) pair.
fn pill(state: &Option<String>) -> (&'static str, &'static str) {
    match state.as_deref() {
        Some("graded") => ("graded", "Graded"),
        Some("submitted") => ("submitted", "Submitted"),
        _ => ("unsubmitted", "Not Submitted"),
    }
}

/// The assignments index template context (Canvas assignments#index).
#[derive(Template)]
#[template(path = "assignments_index.html")]
pub struct AssignmentsIndexPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub is_teacher: bool,
    pub groups: Vec<AssignmentGroupView>,
    /// Total points across the visible (non-deleted) assignments.
    pub total_points: String,
}

impl IntoResponse for AssignmentsIndexPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("assignments index template render failed: {err}"))
                .into_response(),
        }
    }
}

/// GET /courses/{id}/assignments — the assignments index.
///
/// Students see only published assignments; teachers (and admins) see
/// unpublished ones too, styled muted, plus the "Add Assignment" button.
pub async fn assignments_index(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let mut rows =
        match star_assignments::load_course_assignments_with_groups(&app.db, course_id, user.id)
            .await
        {
            Ok(rows) => rows,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    // Students never see unpublished assignments (Canvas assignments#index
    // scopes to published for non-managers).
    if !ctx.is_teacher {
        rows.retain(|r| r.workflow_state == "published");
    }
    let total_points: f64 = rows.iter().map(|r| r.points_possible).sum();

    // Known groups of the course (even empty ones) for the bucket headings.
    let groups: Vec<(i64, String, i32)> = match sqlx::query_as(
        r#"SELECT id, name, position FROM assignment_groups
           WHERE course_id = $1 AND workflow_state <> 'deleted'
           ORDER BY position ASC, id ASC"#,
    )
    .bind(course_id)
    .fetch_all(&app.db)
    .await
    {
        Ok(groups) => groups,
        Err(e) => return from_api_error(&ApiError::from(e), &origin).into_response(),
    };

    let groups = group_assignments(rows, &groups);

    let env = course_env(&ctx, &user, &origin, "assignments", course_id);
    let (jar, csrf_token) = ensure_csrf(jar);

    // The breadcrumb borrows `ctx` while the page struct below moves its
    // fields, so it is built first.
    let breadcrumb = course_breadcrumb(&ctx, course_id);

    let page = AssignmentsIndexPage {
        title: format!("{} - Assignments", ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} assignments"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb,
        csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "assignments".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        is_teacher: ctx.is_teacher,
        groups,
        total_points: trim_f64(total_points),
    };
    (jar, page).into_response()
}

/// Presentation-ready row of the teacher's read-only submissions table.
pub struct SubmissionRowView {
    pub user_id: i64,
    pub user_name: String,
    /// submissions.workflow_state (submitted/graded).
    pub state: String,
    pub submitted_label: String,
    /// "8 / 10" or "–".
    pub score_label: String,
    /// First 80 chars of the text body, or "–".
    pub body_preview: String,
}

/// The assignment show template context (Canvas assignments#show).
#[derive(Template)]
#[template(path = "assignment_show.html")]
pub struct AssignmentShowPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub assignment_id: i64,
    pub assignment_title: String,
    pub description_html: String,
    pub is_teacher: bool,
    pub is_student: bool,
    pub published: bool,
    pub group_name: Option<String>,
    pub submission_pill: String,
    pub submission_pill_class: String,
    pub submission_score_label: String,
    pub due_label: String,
    pub points_label: String,
    pub submissions: Vec<SubmissionRowView>,
    pub submissions_count: usize,
}

impl IntoResponse for AssignmentShowPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("assignment show template render failed: {err}"))
                .into_response(),
        }
    }
}

/// GET /courses/{id}/assignments/{aid} — assignment show.
///
/// The assignment must belong to the URL's course (404 otherwise, Canvas
/// nested-route behavior). Students never see unpublished assignments; the
/// description is sanitized server-side (all tags stripped) before the
/// template renders it as user content.
pub async fn assignment_show(
    State(app): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let rows =
        match star_assignments::load_course_assignments_with_groups(&app.db, course_id, user.id)
            .await
        {
            Ok(rows) => rows,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    let Some(assignment) = rows.into_iter().find(|r| r.id == assignment_id) else {
        return from_api_error(&ApiError::NotFound, &origin).into_response();
    };

    // Students cannot open unpublished assignments (Canvas flashes and
    // redirects; the shell 404s — the resource is invisible to them).
    if !ctx.is_teacher && assignment.workflow_state == "unpublished" {
        return from_api_error(&ApiError::NotFound, &origin).into_response();
    }

    // Teachers get the read-only submissions table (grading UI is #108).
    let submissions: Vec<SubmissionRowView> = if ctx.is_teacher {
        let rows = match load_assignment_submissions(&app.db, assignment_id).await {
            Ok(rows) => rows,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };
        rows.iter()
            .map(|s| SubmissionRowView {
                user_id: s.user_id,
                user_name: s.user_name.clone(),
                state: s.workflow_state.clone(),
                submitted_label: s.submitted_at.format("%b %-d, %Y %H:%M").to_string(),
                score_label: s
                    .score
                    .map(|score| {
                        format!(
                            "{} / {}",
                            trim_f64(score),
                            trim_f64(assignment.points_possible)
                        )
                    })
                    .unwrap_or_else(|| "–".to_string()),
                body_preview: s
                    .body
                    .as_deref()
                    .map(|b| {
                        let truncated: String = b.chars().take(80).collect();
                        if b.chars().count() > 80 {
                            format!("{truncated}…")
                        } else {
                            truncated
                        }
                    })
                    .unwrap_or_else(|| "–".to_string()),
            })
            .collect()
    } else {
        Vec::new()
    };
    let submissions_count = submissions.len();

    let (pill_class, pill_label) = pill(&assignment.submission_state);
    let due_label = assignment
        .due_at
        .map(|d| d.format("%b %-d, %Y at %H:%M").to_string())
        .unwrap_or_else(|| "No due date".to_string());

    // The description is user content: strip every tag server-side (Canvas
    // sanitizes with Sanitize; the phase-1 shell renders the text only).
    let description_html = sanitize_user_content(assignment.description.as_deref());

    let submission_score_label = assignment
        .submission_score
        .map(|score| {
            format!(
                "{} / {}",
                trim_f64(score),
                trim_f64(assignment.points_possible)
            )
        })
        .unwrap_or_default();

    let group_name = assignment.assignment_group_name.clone();
    let published = assignment.workflow_state == "published";
    let assignment_title = assignment.title.clone();
    let points_label = trim_f64(assignment.points_possible);

    let env = course_env(&ctx, &user, &origin, "assignments", course_id);
    let (jar, csrf_token) = ensure_csrf(jar);

    let page = AssignmentShowPage {
        title: format!("{} - {}", assignment_title, ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} assignments"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course.name.clone(), format!("/courses/{course_id}")),
            (
                "Assignments".to_string(),
                format!("/courses/{course_id}/assignments"),
            ),
        ],
        csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "assignments".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        assignment_id,
        assignment_title,
        description_html,
        is_teacher: ctx.is_teacher,
        is_student: ctx.is_student,
        published,
        group_name,
        submission_pill: pill_label.to_string(),
        submission_pill_class: pill_class.to_string(),
        submission_score_label,
        due_label,
        points_label,
        submissions,
        submissions_count,
    };
    (jar, page).into_response()
}

/// Strip all HTML tags from user content (phase-1 sanitizer). The result is
/// plain text; the template still HTML-escapes it on output.
fn sanitize_user_content(input: Option<&str>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            ch if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// The new/edit assignment form template context.
#[derive(Template)]
#[template(path = "assignment_form.html")]
pub struct AssignmentFormPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    /// `false` = create; `true` = edit.
    pub is_edit: bool,
    pub heading: String,
    pub form_action: String,
    pub name_value: String,
    pub description_value: String,
    pub points_value: String,
    /// `YYYY-MM-DD` for the due date input; empty when unset.
    pub due_date_value: String,
    /// `HH:MM` for the due time input; empty when unset.
    pub due_time_value: String,
    pub group_options: Vec<(i64, String)>,
    /// 0 = no group; otherwise the selected assignment_group id.
    pub selected_group_id: i64,
    pub published: bool,
    /// Validation error re-rendered above the form.
    pub error: String,
}

impl IntoResponse for AssignmentFormPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("assignment form template render failed: {err}"))
                .into_response(),
        }
    }
}

/// The form fields shared by new + edit (Canvas assignment form).
#[derive(Deserialize, Debug, Clone)]
pub struct AssignmentForm {
    pub _csrf: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub points_possible: Option<String>,
    /// `YYYY-MM-DD` from the date input.
    #[serde(default)]
    pub due_date: Option<String>,
    /// `HH:MM` from the time input.
    #[serde(default)]
    pub due_time: Option<String>,
    #[serde(default)]
    pub assignment_group_id: Option<String>,
    /// "published" when the checkbox is checked.
    #[serde(default)]
    pub published: Option<String>,
}

/// Parse the form's points field (`""`/garbage -> None).
fn parse_points(raw: Option<&str>) -> Option<f64> {
    raw?.trim().parse::<f64>().ok()
}

/// Parse the form's date + time inputs into an instant (UTC; Canvas stores
/// course-local times but the shell has no timezone plumbing yet, so the
/// naive wall time is interpreted as UTC — documented deviation).
fn parse_due(date: Option<&str>, time: Option<&str>) -> Option<chrono::DateTime<chrono::Utc>> {
    let date = date?.trim();
    if date.is_empty() {
        return None;
    }
    let naive_date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let time = time.unwrap_or("").trim();
    let (h, m) = if time.is_empty() {
        (23, 59)
    } else {
        let mut parts = time.split(':');
        let h: u32 = parts.next()?.parse().ok()?;
        let m: u32 = parts.next().unwrap_or("59").parse().ok()?;
        (h, m)
    };
    let naive = naive_date
        .and_hms_opt(h, m, 0)
        .or_else(|| naive_date.and_hms_opt(0, 0, 0))?;
    Some(Utc.from_utc_datetime(&naive))
}

/// Validation: same rules as the REST create handler.
fn validate_form(form: &AssignmentForm) -> Result<(), String> {
    if form.name.trim().is_empty() {
        return Err("Name is required.".to_string());
    }
    if let Some(points) = parse_points(form.points_possible.as_deref()) {
        if points < 0.0 {
            return Err("Points possible must be >= 0.".to_string());
        }
    }
    Ok(())
}

/// Load the course's assignment groups as form options.
async fn load_group_options(
    app: &AppState,
    course_id: i64,
) -> Result<Vec<(i64, String)>, ApiError> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        r#"SELECT id, name FROM assignment_groups
           WHERE course_id = $1 AND workflow_state <> 'deleted'
           ORDER BY position ASC, id ASC"#,
    )
    .bind(course_id)
    .fetch_all(&app.db)
    .await?;
    Ok(rows)
}

/// Render `f64` without a trailing `.0` (12 -> "12", 12.5 -> "12.5").
fn trim_f64(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Build the (re)renderable form page from the current values. Shared by the
/// GET handlers and the error branches of the POST handlers. `form` (when
/// present) overrides the prefilled values so a failed POST keeps the input.
async fn render_form(
    app: &AppState,
    user: &SessionUser,
    course_id: i64,
    assignment_id: Option<i64>,
    form: Option<&AssignmentForm>,
    error: String,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();
    let ctx = match course_context(app, user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let group_options = match load_group_options(app, course_id).await {
        Ok(options) => options,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    // Edit mode: prefill from the existing row.
    let (
        mut name_value,
        mut description_value,
        mut points_value,
        mut due_date_value,
        mut due_time_value,
        mut selected_group_id,
        mut published,
        heading,
        form_action,
    ) = if let Some(aid) = assignment_id {
        let rows =
            star_assignments::load_course_assignments_with_groups(&app.db, course_id, user.id)
                .await;
        let row = match rows {
            Ok(rows) => rows.into_iter().find(|r| r.id == aid),
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };
        let Some(row) = row else {
            return from_api_error(&ApiError::NotFound, &origin).into_response();
        };
        (
            row.title,
            row.description.unwrap_or_default(),
            trim_f64(row.points_possible),
            row.due_at
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            row.due_at
                .map(|d| d.format("%H:%M").to_string())
                .unwrap_or_default(),
            row.assignment_group_id.unwrap_or(0),
            row.workflow_state == "published",
            "Edit Assignment".to_string(),
            format!("/courses/{course_id}/assignments/{aid}/edit"),
        )
    } else {
        (
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            0,
            true,
            "Add Assignment".to_string(),
            format!("/courses/{course_id}/assignments/new"),
        )
    };

    // A submitted form overrides the prefills (error re-render keeps input).
    if let Some(form) = form {
        name_value = form.name.clone();
        description_value = form.description.clone().unwrap_or_default();
        points_value = form.points_possible.clone().unwrap_or_default();
        due_date_value = form.due_date.clone().unwrap_or_default();
        due_time_value = form.due_time.clone().unwrap_or_default();
        selected_group_id = form
            .assignment_group_id
            .as_deref()
            .and_then(|g| g.trim().parse::<i64>().ok())
            .unwrap_or(0);
        published = form.published.is_some();
    }

    let env = course_env(&ctx, user, &origin, "assignments", course_id);
    let (jar, csrf_token) = ensure_csrf(jar);

    let page = AssignmentFormPage {
        title: format!("{} - {}", heading, ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} assignments form"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course.name.clone(), format!("/courses/{course_id}")),
            (
                "Assignments".to_string(),
                format!("/courses/{course_id}/assignments"),
            ),
        ],
        csrf_token,
        tabs: ctx.tabs,
        active_context_tab: "assignments".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        is_edit: assignment_id.is_some(),
        heading,
        form_action,
        name_value,
        description_value,
        points_value,
        due_date_value,
        due_time_value,
        group_options,
        selected_group_id,
        published,
        error,
    };
    (jar, page).into_response()
}

/// GET /courses/{id}/assignments/new — teacher-only creation form.
pub async fn assignment_new(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !ctx.is_teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    render_form(&app, &user, course_id, None, None, String::new(), jar).await
}

/// POST /courses/{id}/assignments/new — create from the form.
///
/// Teacher-or-admin only (403 otherwise, including CSRF failures); on
/// validation error the form re-renders with the message (422), on success a
/// 303 to the assignments index.
pub async fn assignment_create(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<AssignmentForm>,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !ctx.is_teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }
    if !check_csrf(&jar, &form._csrf) {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    if let Err(msg) = validate_form(&form) {
        let mut resp = render_form(&app, &user, course_id, None, Some(&form), msg, jar).await;
        *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
        return resp;
    }

    // Only accept a group that belongs to this course.
    let group_id = form
        .assignment_group_id
        .as_deref()
        .and_then(|g| g.trim().parse::<i64>().ok())
        .filter(|id| *id > 0);
    let group_id = match group_id {
        Some(gid) => {
            let known: Option<i32> = sqlx::query_scalar(
                r#"SELECT 1 FROM assignment_groups
                   WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'"#,
            )
            .bind(gid)
            .bind(course_id)
            .fetch_optional(&app.db)
            .await
            .unwrap_or(None);
            if known.is_none() {
                let mut resp = render_form(
                    &app,
                    &user,
                    course_id,
                    None,
                    Some(&form),
                    "Unknown assignment group.".to_string(),
                    jar,
                )
                .await;
                *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
                return resp;
            }
            Some(gid)
        }
        None => None,
    };

    let created = create_assignment_record(
        &app.db,
        &NewAssignmentRecord {
            course_id,
            title: &form.name,
            description: form.description.as_deref(),
            points_possible: parse_points(form.points_possible.as_deref()),
            due_at: parse_due(form.due_date.as_deref(), form.due_time.as_deref()),
            assignment_group_id: group_id,
            workflow_state: if form.published.is_some() {
                "published"
            } else {
                "unpublished"
            },
        },
    )
    .await;

    match created {
        Ok(_id) => {
            let location = format!("/courses/{course_id}/assignments");
            (StatusCode::SEE_OTHER, [("location", location.as_str())]).into_response()
        }
        Err(ApiError::Validation(msg)) => {
            let mut resp = render_form(&app, &user, course_id, None, Some(&form), msg, jar).await;
            *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            resp
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// GET /courses/{id}/assignments/{aid}/edit — teacher-only edit form.
pub async fn assignment_edit(
    State(app): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !ctx.is_teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    // The assignment must belong to this course (Canvas nested-route 404).
    let in_course: Option<i32> = sqlx::query_scalar(
        r#"SELECT 1 FROM assignments
           WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'"#,
    )
    .bind(assignment_id)
    .bind(course_id)
    .fetch_optional(&app.db)
    .await
    .unwrap_or(None);
    if in_course.is_none() {
        return from_api_error(&ApiError::NotFound, &origin).into_response();
    }

    render_form(
        &app,
        &user,
        course_id,
        Some(assignment_id),
        None,
        String::new(),
        jar,
    )
    .await
}

/// POST /courses/{id}/assignments/{aid}/edit — update from the form.
pub async fn assignment_update(
    State(app): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<AssignmentForm>,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !ctx.is_teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }
    if !check_csrf(&jar, &form._csrf) {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    // The assignment must belong to this course (Canvas nested-route 404).
    let in_course: Option<i32> = sqlx::query_scalar(
        r#"SELECT 1 FROM assignments
           WHERE id = $1 AND course_id = $2 AND workflow_state <> 'deleted'"#,
    )
    .bind(assignment_id)
    .bind(course_id)
    .fetch_optional(&app.db)
    .await
    .unwrap_or(None);
    if in_course.is_none() {
        return from_api_error(&ApiError::NotFound, &origin).into_response();
    }

    if let Err(msg) = validate_form(&form) {
        let mut resp = render_form(
            &app,
            &user,
            course_id,
            Some(assignment_id),
            Some(&form),
            msg,
            jar,
        )
        .await;
        *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
        return resp;
    }

    let group_id = form
        .assignment_group_id
        .as_deref()
        .and_then(|g| g.trim().parse::<i64>().ok())
        .filter(|id| *id > 0);

    let updated = update_assignment_record(
        &app.db,
        assignment_id,
        &AssignmentRecordUpdate {
            title: Some(&form.name),
            description: form.description.as_deref(),
            points_possible: parse_points(form.points_possible.as_deref()),
            due_at: parse_due(form.due_date.as_deref(), form.due_time.as_deref()),
            assignment_group_id: group_id,
            workflow_state: Some(if form.published.is_some() {
                "published"
            } else {
                "unpublished"
            }),
        },
    )
    .await;

    match updated {
        Ok(()) => {
            let location = format!("/courses/{course_id}/assignments/{assignment_id}");
            (StatusCode::SEE_OTHER, [("location", location.as_str())]).into_response()
        }
        Err(ApiError::Validation(msg)) => {
            let mut resp = render_form(
                &app,
                &user,
                course_id,
                Some(assignment_id),
                Some(&form),
                msg,
                jar,
            )
            .await;
            *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            resp
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// The text-entry submission form (POST body).
#[derive(Deserialize, Debug)]
pub struct SubmitForm {
    pub _csrf: String,
    pub body: String,
}

/// POST /courses/{id}/assignments/{aid}/submit — student text-entry submit.
///
/// Same upsert SQL shape as the REST `create_submission`
/// (ON CONFLICT (assignment_id, user_id) DO UPDATE), gated to enrolled
/// students of the course. On success a 303 back to the assignment show
/// page, where the submission state pill now reads Submitted/Graded.
pub async fn assignment_submit(
    State(app): State<AppState>,
    Path((course_id, assignment_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<SubmitForm>,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !check_csrf(&jar, &form._csrf) {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    // The assignment must be a published member of the course.
    let open: Option<i32> = sqlx::query_scalar(
        r#"SELECT 1 FROM assignments
           WHERE id = $1 AND course_id = $2 AND workflow_state = 'published'"#,
    )
    .bind(assignment_id)
    .bind(course_id)
    .fetch_optional(&app.db)
    .await
    .unwrap_or(None);
    if open.is_none() {
        return from_api_error(&ApiError::NotFound, &origin).into_response();
    }

    // Only enrolled students submit; teachers/admins manage, not submit
    // (Canvas' submit form is the student view).
    if ctx.is_teacher || !ctx.is_student {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    if form.body.trim().is_empty() {
        return from_api_error(
            &ApiError::Validation("Submission text is required.".into()),
            &origin,
        )
        .into_response();
    }

    match upsert_own_text_submission(&app.db, assignment_id, user.id, &form.body).await {
        Ok(_sid) => {
            let location = format!("/courses/{course_id}/assignments/{assignment_id}?submitted=1");
            (StatusCode::SEE_OTHER, [("location", location.as_str())]).into_response()
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}
