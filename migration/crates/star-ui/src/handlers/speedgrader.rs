//! SpeedGrader-style grading workflow handlers (issue #108) — Canvas
//! `gradebooks#speedgrader`, server-rendered.
//!
//! Distinct from the gradebook LIST view (#71): this is the per-student
//! grading queue. The left nav lists every enrolled student (Canvas order:
//! name then id) for the selected assignment (`?assignment_id=`, default
//! the first assignment by group+position, then insertion order); the main
//! panel shows the selected student's submission (text body, attempt info,
//! current score) and ONE no-JS form that submits score + comment (+ rubric
//! criterion picks when the assignment has a rubric) together. PRG back to
//! the same student+assignment with a `graded=1` success notice.
//!
//! Gates: teacher-or-admin only (`is_teacher` from course_context, which
//! runs star_courses::check_teacher_or_admin after the 404-before-403
//! course gate); students get 403, unenrolled users the course gate's
//! 404/403. Next/prev student are plain links.
//!
//! Data access reuses the domain crates' pub loaders + write paths:
//! - `star_assignments::ui::{load_grading_students, load_submission_for,
//!   grade_submission_for_user}` — same upsert shape as the REST
//!   `course_grade_submission` handler;
//! - `star_submission_comments::ui::{load_submission_comment_feed,
//!   create_comment_record}` — same validation + INSERT as the REST
//!   `create_comment_for`;
//! - `star_rubrics::ui::{load_assignment_rubric_panel,
//!   load_grading_assessment, upsert_grading_assessment}` — same column set
//!   and resolution rules as the REST `insert_assessment`.

use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::json;

use star_common::{ApiError, AppState};

use crate::assets;
use crate::handlers::course_context::{course_breadcrumb, course_context, course_env, ensure_csrf};
use crate::handlers::error::from_api_error;
use crate::session::{check_csrf, SessionUser};

// ---------------------------------------------------------------------------
// View structs
// ---------------------------------------------------------------------------

/// Presentation-ready student row of the SpeedGrader left nav. All display
/// strings are precomputed so the template only does field access.
pub struct GradingStudentView {
    pub user_id: i64,
    pub user_name: String,
    /// "graded" | "submitted" | "unsubmitted".
    pub pill_class: String,
    /// "Graded" | "Submitted" | "Not Submitted".
    pub pill_label: String,
    /// True when this row is the selected student.
    pub selected: bool,
    /// Link target preserving the assignment selection.
    pub url: String,
}

/// One comment of the feed.
pub struct CommentView {
    pub id: i64,
    pub author_label: String,
    pub comment: String,
    pub created_label: String,
}

/// One criterion row of the rubric panel (with the current pick, if any).
#[derive(Clone)]
pub struct RubricCriterionView {
    pub id: i64,
    pub description: String,
    pub points_label: String,
    /// Rating options: (id, label, selected).
    pub ratings: Vec<(i64, String, bool)>,
}

/// The rubric panel view.
#[derive(Clone)]
pub struct RubricPanelView {
    pub title: String,
    pub points_label: String,
    pub criteria: Vec<RubricCriterionView>,
}

/// The SpeedGrader page template context.
#[derive(Template)]
#[template(path = "speedgrader.html")]
pub struct SpeedGraderPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub unread_badge: i64,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    /// The graded assignment selection: (id, title) pairs, group+position
    /// order (the gradebook column order).
    pub assignments: Vec<(i64, String)>,
    pub assignment_id: i64,
    pub assignment_title: String,
    pub points_label: String,
    pub due_label: String,
    /// The selected student (name + id).
    pub student_id: i64,
    pub student_name: String,
    pub students: Vec<GradingStudentView>,
    /// Plain-link next/prev student navigation (no JS).
    pub prev_url: Option<String>,
    pub next_url: Option<String>,
    pub position_label: String,
    /// "Graded" | "Submitted" | "Not Submitted" for the selected student.
    pub pill_class: String,
    pub pill_label: String,
    pub submitted_label: String,
    pub attempt_label: String,
    /// The submission text body (empty when none).
    pub submission_body: String,
    /// Current score prefill for the form ("" when ungraded).
    pub score_value: String,
    /// "8 / 10" or "–".
    pub score_label: String,
    pub comments: Vec<CommentView>,
    pub rubric: Option<RubricPanelView>,
    /// Validation error re-rendered above the form.
    pub error: String,
    pub form_action: String,
}

impl IntoResponse for SpeedGraderPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("speedgrader template render failed: {err}")).into_response()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SpeedGraderQuery {
    pub assignment_id: Option<i64>,
    pub student_id: Option<i64>,
    /// Set by the POST's 303: renders the success notice.
    #[serde(default)]
    pub graded: Option<String>,
}

/// Render `f64` without a trailing `.0` (12 -> "12", 12.5 -> "12.5").
fn trim_f64(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// The submission state pill (class, label) pair.
fn pill(state: Option<&str>) -> (&'static str, &'static str) {
    match state {
        Some("graded") => ("graded", "Graded"),
        Some("submitted") => ("submitted", "Submitted"),
        _ => ("unsubmitted", "Not Submitted"),
    }
}

// ---------------------------------------------------------------------------
// Page assembly (shared by GET and the POST's error re-render)
// ---------------------------------------------------------------------------

/// The render inputs shared by the GET handler and the POST's error
/// re-render (a param struct keeps `render_page` under clippy's
/// too_many_arguments threshold).
struct PageInput {
    /// The selected assignment (default the first when absent/unknown).
    assignment_sel: Option<i64>,
    /// The selected student (default the first when absent/unknown).
    student_sel: Option<i64>,
    /// Validation message re-rendered above the form.
    error: String,
    /// Flash notice ("Grade saved." after a successful POST).
    notice: Option<String>,
}

/// Assemble the SpeedGrader page.
async fn render_page(
    app: &AppState,
    user: &SessionUser,
    course_id: i64,
    input: PageInput,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(app, user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !ctx.is_teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    // ---- Assignment selection: group+position order (gradebook columns).
    let mut assignments =
        match star_assignments::load_course_assignments_with_groups(&app.db, course_id, -1).await {
            Ok(rows) => rows,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };
    assignments.sort_by(|a, b| {
        let ka = (a.assignment_group_position.unwrap_or(i32::MAX), a.id);
        let kb = (b.assignment_group_position.unwrap_or(i32::MAX), b.id);
        ka.cmp(&kb)
    });

    // Default: the first assignment; an unknown/foreign id falls back to it
    // too (Canvas would 404; the shell keeps the queue usable).
    let assignment = assignments
        .iter()
        .find(|a| Some(a.id) == input.assignment_sel)
        .or_else(|| assignments.first());
    let Some(assignment) = assignment else {
        // No assignments at all: an empty-state page instead of a 404 — the
        // course exists and the teacher may reach it from the tabs.
        return render_empty(app, user, course_id, jar).await;
    };
    let assignment_id = assignment.id;

    // ---- Student selection: Canvas order (name, id); default the first.
    let students = match star_assignments::ui::load_grading_students(
        &app.db,
        course_id,
        assignment_id,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let student = students
        .iter()
        .find(|s| Some(s.user_id) == input.student_sel)
        .or_else(|| students.first());
    let Some(student) = student else {
        return render_empty(app, user, course_id, jar).await;
    };
    let student_id = student.user_id;

    // ---- The selected student's submission.
    let submission =
        match star_assignments::ui::load_submission_for(&app.db, assignment_id, student_id).await {
            Ok(sub) => sub,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    // ---- Comment feed.
    let feed = submission
        .as_ref()
        .map(|sub| star_submission_comments::ui::load_submission_comment_feed(&app.db, sub.id));
    let comments: Vec<CommentView> = match feed {
        Some(feed) => match feed.await {
            Ok(rows) => rows
                .into_iter()
                .map(|c| CommentView {
                    id: c.id,
                    author_label: c.author_name.unwrap_or_else(|| "Unknown".to_string()),
                    comment: c.comment,
                    created_label: c.created_at.format("%b %-d, %Y %H:%M").to_string(),
                })
                .collect(),
            Err(e) => return from_api_error(&e, &origin).into_response(),
        },
        None => Vec::new(),
    };

    // ---- Rubric panel (when the assignment has one).
    let submission_id = submission.as_ref().map(|s| s.id);
    let rubric = match build_rubric_view(app, assignment_id, submission_id).await {
        Ok(view) => view,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    // ---- Left nav rows + next/prev links.
    let student_views: Vec<GradingStudentView> = students
        .iter()
        .map(|s| {
            let (pill_class, pill_label) = pill(s.submission_state.as_deref());
            GradingStudentView {
                user_id: s.user_id,
                user_name: s.user_name.clone(),
                pill_class: pill_class.to_string(),
                pill_label: pill_label.to_string(),
                selected: s.user_id == student_id,
                url: format!(
                    "/courses/{course_id}/gradebook/speedgrader\
                     ?assignment_id={assignment_id}&student_id={}",
                    s.user_id
                ),
            }
        })
        .collect();
    let selected_index = student_views
        .iter()
        .position(|s| s.user_id == student_id)
        .unwrap_or(0);
    let (prev_url, next_url) = if students.len() > 1 {
        let prev = student_views[selected_index.saturating_sub(1)].url.clone();
        let next = student_views[(selected_index + 1) % student_views.len()]
            .url
            .clone();
        (Some(prev), Some(next))
    } else {
        (None, None)
    };
    let position_label = format!("{} of {}", selected_index + 1, student_views.len());

    let (pill_class, pill_label) = pill(submission.as_ref().map(|s| s.workflow_state.as_str()));
    let submitted_label = submission
        .as_ref()
        .map(|s| s.submitted_at.format("%b %-d, %Y %H:%M").to_string())
        .unwrap_or_else(|| "–".to_string());
    let attempt_label = submission
        .as_ref()
        .map(|s| format!("{}", s.attempt))
        .unwrap_or_else(|| "–".to_string());
    let submission_body = submission
        .as_ref()
        .and_then(|s| s.body.clone())
        .unwrap_or_default();
    let score_label = submission
        .as_ref()
        .and_then(|s| s.score)
        .map(|score| {
            format!(
                "{} / {}",
                trim_f64(score),
                trim_f64(assignment.points_possible)
            )
        })
        .unwrap_or_else(|| "–".to_string());
    let score_value = submission
        .as_ref()
        .and_then(|s| s.score)
        .map(trim_f64)
        .unwrap_or_default();
    let student_name = student.user_name.clone();
    let assignment_title = assignment.title.clone();
    let points_label = trim_f64(assignment.points_possible);
    let due_label = assignment
        .due_at
        .map(|d| d.format("%b %-d, %Y").to_string())
        .unwrap_or_else(|| "No due date".to_string());

    let env = course_env(&ctx, user, &origin, "grades", course_id);
    let (jar, csrf_token) = ensure_csrf(jar);

    let breadcrumb = course_breadcrumb(&ctx, course_id);

    let page = SpeedGraderPage {
        title: format!("{} - SpeedGrader", ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} speedgrader full-width"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: input.notice.into_iter().collect(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb,
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "grades".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        assignments: assignments
            .iter()
            .map(|a| (a.id, a.title.clone()))
            .collect(),
        assignment_id,
        assignment_title,
        points_label,
        due_label,
        student_id,
        student_name,
        students: student_views,
        prev_url,
        next_url,
        position_label,
        pill_class: pill_class.to_string(),
        pill_label: pill_label.to_string(),
        submitted_label,
        attempt_label,
        submission_body,
        score_value,
        score_label,
        comments,
        rubric,
        error: input.error,
        form_action: format!("/courses/{course_id}/gradebook/speedgrader/grade"),
    };
    (jar, page).into_response()
}

/// The no-assignments / no-enrollments empty state.
async fn render_empty(
    app: &AppState,
    user: &SessionUser,
    course_id: i64,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();
    let ctx = match course_context(app, user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !ctx.is_teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }
    let env = course_env(&ctx, user, &origin, "grades", course_id);
    let (jar, csrf_token) = ensure_csrf(jar);
    let breadcrumb = course_breadcrumb(&ctx, course_id);
    let page = SpeedGraderEmptyPage {
        title: format!("{} - SpeedGrader", ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} speedgrader full-width"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb,
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "grades".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        message: "Nothing to grade yet: this course needs at least one assignment and one \
                  enrollment."
            .to_string(),
    };
    (jar, page).into_response()
}

/// The empty-state template context.
#[derive(Template)]
#[template(path = "speedgrader_empty.html")]
pub struct SpeedGraderEmptyPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub unread_badge: i64,
    pub tabs: Vec<star_courses::CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub message: String,
}

impl IntoResponse for SpeedGraderEmptyPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("speedgrader empty template render failed: {err}"))
                .into_response(),
        }
    }
}

/// Build the rubric panel view with the current assessment pre-filled.
async fn build_rubric_view(
    app: &AppState,
    assignment_id: i64,
    submission_id: Option<i64>,
) -> Result<Option<RubricPanelView>, ApiError> {
    let Some(panel) =
        star_rubrics::ui::load_assignment_rubric_panel(&app.db, assignment_id).await?
    else {
        return Ok(None);
    };

    // The existing grading assessment's picks, keyed by criterion_id.
    let mut picks: std::collections::HashMap<i64, (Option<i64>, Option<f64>)> =
        std::collections::HashMap::new();
    if let Some(submission_id) = submission_id {
        if let Some(assessment) =
            star_rubrics::ui::load_grading_assessment(&app.db, panel.rubric_id, submission_id)
                .await?
        {
            if let Some(serde_json::Value::Array(entries)) = assessment.data.clone() {
                for entry in entries {
                    let criterion_id = entry.get("criterion_id").and_then(|v| v.as_i64());
                    let rating_id = entry.get("rating_id").and_then(|v| v.as_i64());
                    let points = entry.get("points").and_then(|v| v.as_f64());
                    if let Some(criterion_id) = criterion_id {
                        picks.insert(criterion_id, (rating_id, points));
                    }
                }
            }
        }
    }

    let criteria = panel
        .criteria
        .into_iter()
        .map(|criterion| {
            let (picked_rating, picked_points) =
                picks.get(&criterion.id).cloned().unwrap_or((None, None));
            let ratings = criterion
                .ratings
                .iter()
                .map(|r| {
                    let selected = Some(r.id) == picked_rating;
                    let label = if r.description.is_empty() {
                        trim_f64(r.points)
                    } else {
                        format!("{} ({} pts)", r.description, trim_f64(r.points))
                    };
                    (r.id, label, selected)
                })
                .collect();
            let points_label = picked_points
                .map(trim_f64)
                .unwrap_or_else(|| "–".to_string());
            RubricCriterionView {
                id: criterion.id,
                description: criterion.description,
                points_label,
                ratings,
            }
        })
        .collect();

    Ok(Some(RubricPanelView {
        title: panel.title,
        points_label: trim_f64(panel.points_possible),
        criteria,
    }))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /courses/{id}/gradebook/speedgrader — the grading queue.
///
/// Teacher-or-admin only. `?assignment_id=` selects the assignment (default
/// the first by group+position); `?student_id=` the student (default the
/// first by name); `?graded=1` (set by the POST's 303) renders the success
/// notice.
pub async fn speedgrader(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<SpeedGraderQuery>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let notice = if query.graded.as_deref() == Some("1") {
        Some("Grade saved.".to_string())
    } else {
        None
    };
    render_page(
        &app,
        &user,
        course_id,
        PageInput {
            assignment_sel: query.assignment_id,
            student_sel: query.student_id,
            error: String::new(),
            notice,
        },
        jar,
    )
    .await
}

/// POST /courses/{id}/gradebook/speedgrader/grade — the ONE grading form.
///
/// CSRF double-submit on the raw body (the rubric picks are dynamic
/// `criterion_<id>` fields, so the body is decoded with
/// `url::form_urlencoded` exactly like the quiz submit handler). Persists:
/// the score via `star_assignments::ui::grade_submission_for_user` (the
/// REST upsert shape), the comment via
/// `star_submission_comments::ui::create_comment_record` (the REST
/// validation), and the rubric assessment via
/// `star_rubrics::ui::upsert_grading_assessment` (the REST INSERT shape).
/// On success a 303 back to the same student+assignment with `graded=1`.
pub async fn speedgrader_grade(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
    body: Bytes,
) -> Response {
    let origin = crate::request_origin();

    // CSRF double-submit on the raw body.
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(&body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let field = |name: &str| -> String {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let form_token = field("_csrf");
    if !check_csrf(&jar, &form_token) {
        return (
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
        )
            .into_response();
    }

    let assignment_id: Option<i64> = field("assignment_id").trim().parse().ok();
    let student_id: Option<i64> = field("student_id").trim().parse().ok();
    let score_raw = field("score");
    let comment = field("comment").trim().to_string();

    // Validation: the REST grading handler requires a score; the shell
    // mirrors it (a blank score is a re-render with the message, 422).
    let score = match score_raw.trim().parse::<f64>() {
        Ok(score) => score,
        Err(_) => {
            let mut resp = render_page(
                &app,
                &user,
                course_id,
                PageInput {
                    assignment_sel: assignment_id,
                    student_sel: student_id,
                    error: "Score is required and must be a number.".to_string(),
                    notice: None,
                },
                jar,
            )
            .await;
            *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            return resp;
        }
    };

    // The (assignment, student) pair must belong to this course — the gate
    // reuses the same course_context 404-before-403 + teacher check.
    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };
    if !ctx.is_teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    let target_ok = sqlx::query_scalar::<_, i32>(
        r#"SELECT 1 FROM assignments a
           WHERE a.id = $1 AND a.course_id = $2 AND a.workflow_state <> 'deleted'"#,
    )
    .bind(assignment_id.unwrap_or(-1))
    .bind(course_id)
    .fetch_optional(&app.db)
    .await
    .unwrap_or(None)
    .is_some();
    if !target_ok {
        return from_api_error(&ApiError::NotFound, &origin).into_response();
    }
    let assignment_id = assignment_id.unwrap_or(-1);
    let student_id = match student_id {
        Some(sid) => sid,
        None => return from_api_error(&ApiError::NotFound, &origin).into_response(),
    };
    let enrolled = sqlx::query_scalar::<_, i32>(
        r#"SELECT 1 FROM enrollments
           WHERE course_id = $1 AND user_id = $2 AND workflow_state <> 'deleted'
           LIMIT 1"#,
    )
    .bind(course_id)
    .bind(student_id)
    .fetch_optional(&app.db)
    .await
    .unwrap_or(None)
    .is_some();
    if !enrolled {
        return from_api_error(&ApiError::NotFound, &origin).into_response();
    }

    // Score: the REST upsert shape (Validation errors re-render as 422).
    let submission = match star_assignments::ui::grade_submission_for_user(
        &app.db,
        assignment_id,
        student_id,
        user.id,
        score,
    )
    .await
    {
        Ok(submission) => submission,
        Err(ApiError::Validation(msg)) => {
            let mut resp = render_page(
                &app,
                &user,
                course_id,
                PageInput {
                    assignment_sel: Some(assignment_id),
                    student_sel: Some(student_id),
                    error: msg,
                    notice: None,
                },
                jar,
            )
            .await;
            *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            return resp;
        }
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    // Comment: only when the teacher typed one (the REST create path's
    // empty-comment validation lives inside the loader).
    if !comment.is_empty() {
        if let Err(e) = star_submission_comments::ui::create_comment_record(
            &app.db,
            submission.id,
            user.id,
            &comment,
        )
        .await
        {
            return from_api_error(&e, &origin).into_response();
        }
    }

    // Rubric: persist the criterion picks when the assignment has a rubric
    // and the form carried any `criterion_<id>` field.
    let panel = match star_rubrics::ui::load_assignment_rubric_panel(&app.db, assignment_id).await {
        Ok(panel) => panel,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    if let Some(panel) = panel {
        let mut data = Vec::new();
        let mut total = 0.0;
        let mut any_pick = false;
        for criterion in &panel.criteria {
            let raw = field(&format!("criterion_{}", criterion.id));
            if raw.trim().is_empty() {
                continue;
            }
            if let Ok(rating_id) = raw.trim().parse::<i64>() {
                if let Some(rating) = criterion.ratings.iter().find(|r| r.id == rating_id) {
                    any_pick = true;
                    total += rating.points;
                    data.push(json!({
                        "criterion_id": criterion.id,
                        "rating_id": rating.id,
                        "points": rating.points,
                        "comments": null,
                    }));
                }
            }
        }
        if any_pick {
            if let Err(e) = star_rubrics::ui::upsert_grading_assessment(
                &app.db,
                panel.rubric_id,
                panel.association_id,
                submission.id,
                user.id,
                total,
                serde_json::Value::Array(data),
            )
            .await
            {
                return from_api_error(&e, &origin).into_response();
            }
        }
    }

    let location = format!(
        "/courses/{course_id}/gradebook/speedgrader\
         ?assignment_id={assignment_id}&student_id={student_id}&graded=1"
    );
    (StatusCode::SEE_OTHER, [("location", location.as_str())]).into_response()
}
