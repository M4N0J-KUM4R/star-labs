//! Course settings + people/admin journey handlers (issue #73) — Canvas
//! `courses#settings` (`/settings`) and `context#roster` (`/users`),
//! server-rendered.
//!
//! Every handler gates through [`crate::handlers::course_context`]
//! (`star_courses::load_course_for_viewer`, 404-before-403), so the pages
//! cannot bypass `/api/v1` course authorization. Teachers/admins
//! additionally pass the `is_teacher` check (star_courses::
//! check_teacher_or_admin) for every settings page and every people
//! mutation — a non-teacher non-admin gets 403.
//!
//! Settings save maps the form onto the exact REST `PUT /courses/{id}`
//! update (`star_courses::update_course_record`, the extracted body of the
//! API handler), so the validation set is identical: name, course_code,
//! description, workflow_state (of available/unpublished/completed/deleted),
//! syllabus_body. `default_view` and self-enrollment are NOT rendered: this
//! schema has no `courses.default_view` / `courses.self_enrollment` columns
//! and the REST update cannot persist them (documented deviation — only
//! fields the API can actually persist are on the form).
//!
//! Add-people maps onto `star_enrollments::ui::enroll_user_in_course`, the
//! extracted body of the REST `POST /api/v1/enrollments` create (login
//! resolved to a user id, role validated to student/teacher/ta, duplicate
//! triple 409). Conclude/reactivate flip `enrollments.workflow_state`
//! inactive/active, the same transitions the REST lifecycle endpoints use.

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;
use serde::Deserialize;

use star_common::{ApiError, AppState};
use star_courses::{Course, CourseTab, UpdateCourse};
use star_enrollments::ui::RosterRow;

use crate::assets;
use crate::handlers::course_context::{course_breadcrumb, course_context, course_env, ensure_csrf};
use crate::handlers::error::from_api_error;
use crate::session::{check_csrf, SessionUser};

/// The settings page template context (Canvas courses#settings, Details tab).
#[derive(Template)]
#[template(path = "course_settings.html")]
pub struct CourseSettingsPage {
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
    pub tabs: Vec<CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    /// Current field values (prefill) or the submitted ones on error.
    pub name_value: String,
    pub code_value: String,
    pub description_value: String,
    /// "published" when the checkbox is checked ("available"/"completed"
    /// render as published; "unpublished" is the only unpublish state).
    pub published: bool,
    /// Plain-text syllabus summary (tags stripped, whitespace collapsed).
    pub syllabus_summary: String,
    /// Validation error re-rendered above the form.
    pub error: String,
}

impl IntoResponse for CourseSettingsPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("course settings template render failed: {err}"))
                .into_response(),
        }
    }
}

/// The settings form fields. Only what the REST `PUT /courses/{id}` accepts.
#[derive(Deserialize, Debug, Clone)]
pub struct SettingsForm {
    pub _csrf: String,
    pub name: String,
    #[serde(default)]
    pub course_code: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// "published" when the checkbox is checked.
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub syllabus_body: Option<String>,
}

/// Strip all HTML tags and collapse whitespace for the read-only syllabus
/// summary (same tag-stripping approach as the assignments sanitizer).
fn syllabus_summary(body: Option<&str>) -> String {
    let Some(body) = body else {
        return String::new();
    };
    let mut out = String::with_capacity(body.len());
    let mut in_tag = false;
    for ch in body.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            '\n' | '\r' | '\t' if !in_tag => out.push(' '),
            ch if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let collapsed: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
}

/// Build the (re)renderable settings page. `form` (when present) overrides
/// the stored values so a failed POST keeps the input. Shared by GET and the
/// error branch of POST.
struct SettingsRender<'a> {
    course_id: i64,
    course: &'a Course,
    form: Option<&'a SettingsForm>,
    error: String,
    notices: Vec<String>,
}

async fn render_settings(
    app: &AppState,
    user: &SessionUser,
    render: SettingsRender<'_>,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();
    let ctx = match course_context(app, user, render.course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let mut name_value = render.course.name.clone();
    let mut code_value = render.course.course_code.clone();
    let mut description_value = render.course.description.clone().unwrap_or_default();
    let mut published = render.course.workflow_state != "unpublished";
    let mut summary = syllabus_summary(render.course.syllabus_body.as_deref());

    if let Some(form) = render.form {
        name_value = form.name.clone();
        code_value = form.course_code.clone().unwrap_or_default();
        description_value = form.description.clone().unwrap_or_default();
        published = form.published.is_some();
        summary = syllabus_summary(form.syllabus_body.as_deref());
    }

    let env = course_env(&ctx, user, &origin, "settings", render.course_id);
    let (jar, csrf_token) = ensure_csrf(jar);

    // The breadcrumb borrows `ctx` while the page struct below moves its
    // fields, so it is built first.
    let breadcrumb = course_breadcrumb(&ctx, render.course_id);

    let page = CourseSettingsPage {
        title: format!("{} - Settings", ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{} settings",
            render.course_id
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: render.notices,
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb,
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "settings".to_string(),
        course_name: ctx.course.name.clone(),
        course_id: render.course_id,
        name_value,
        code_value,
        description_value,
        published,
        syllabus_summary: summary,
        error: render.error,
    };
    (jar, page).into_response()
}

/// Query params that turn into flash notices after a PRG redirect.
#[derive(Deserialize, Default)]
pub struct NoticeQuery {
    #[serde(default)]
    pub saved: Option<String>,
    #[serde(default)]
    pub added: Option<String>,
    #[serde(default)]
    pub concluded: Option<String>,
    #[serde(default)]
    pub reactivated: Option<String>,
}

/// GET /courses/{id}/settings — teacher/admin only (students 403).
///
/// `?saved=1` (the POST's PRG target) renders the success notice.
pub async fn course_settings(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<NoticeQuery>,
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

    let notices = if query.saved.is_some() {
        vec!["Course settings saved.".to_string()]
    } else {
        Vec::new()
    };

    let course = ctx.course.clone();
    let render = SettingsRender {
        course_id,
        course: &course,
        form: None,
        error: String::new(),
        notices,
    };
    render_settings(&app, &user, render, jar).await
}

/// POST /courses/{id}/settings — CSRF-checked form save.
///
/// Maps the form onto the same validation the REST update applies (via
/// `star_courses::update_course_record`): a blank name is a 422 re-render;
/// an invalid workflow_state cannot happen through the checkbox but is
/// validated by the shared loader anyway. On success a 303 PRG redirect
/// back to the settings page with a success notice.
pub async fn course_settings_save(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<SettingsForm>,
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

    // Same rule the REST create/update validation applies to names.
    if form.name.trim().is_empty() {
        let course = ctx.course.clone();
        let render = SettingsRender {
            course_id,
            course: &course,
            form: Some(&form),
            error: "Name is required.".to_string(),
            notices: Vec::new(),
        };
        let mut resp = render_settings(&app, &user, render, jar).await;
        *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
        return resp;
    }

    let payload = UpdateCourse {
        name: Some(form.name.clone()),
        course_code: Some(form.course_code.clone().unwrap_or_default()),
        description: if form
            .description
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            None
        } else {
            Some(form.description.clone().unwrap_or_default())
        },
        workflow_state: Some(if form.published.is_some() {
            "available".to_string()
        } else {
            "unpublished".to_string()
        }),
        start_at: None,
        end_at: None,
        // An empty textarea keeps the stored syllabus (REST semantics: None
        // keeps the current value).
        syllabus_body: if form
            .syllabus_body
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
        {
            None
        } else {
            Some(form.syllabus_body.clone().unwrap_or_default())
        },
    };

    match star_courses::update_course_record(
        &app.db,
        course_id,
        &crate::handlers::course_context::auth_of(&user),
        payload,
    )
    .await
    {
        Ok(_) => {
            let location = format!("/courses/{course_id}/settings?saved=1");
            (StatusCode::SEE_OTHER, [("location", location.as_str())]).into_response()
        }
        Err(ApiError::Validation(msg)) => {
            let course = ctx.course.clone();
            let render = SettingsRender {
                course_id,
                course: &course,
                form: Some(&form),
                error: msg,
                notices: Vec::new(),
            };
            let mut resp = render_settings(&app, &user, render, jar).await;
            *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            resp
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// Presentation-ready row of the roster table (Canvas context#roster).
pub struct RosterRowView {
    pub enrollment_id: i64,
    pub user_id: i64,
    pub name: String,
    pub email: String,
    /// "Student" | "Teacher" | "Ta" | the raw role.
    pub role_label: String,
    /// "active" | "invited" | "inactive" | other raw state.
    pub state: String,
    /// "Active" | "Invited" | "Inactive" | title-cased state.
    pub state_label: String,
}

/// The people page template context (Canvas context#roster).
#[derive(Template)]
#[template(path = "course_people.html")]
pub struct CoursePeoplePage {
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
    pub tabs: Vec<CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub rows: Vec<RosterRowView>,
    pub is_teacher: bool,
}

impl IntoResponse for CoursePeoplePage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("course people template render failed: {err}")).into_response()
            }
        }
    }
}

fn role_label(role: &str) -> String {
    match role {
        "student" => "Student".to_string(),
        "teacher" => "Teacher".to_string(),
        "ta" => "Ta".to_string(),
        other => other.to_string(),
    }
}

fn state_label(state: &str) -> String {
    match state {
        "active" => "Active".to_string(),
        "invited" => "Invited".to_string(),
        "inactive" => "Inactive".to_string(),
        other => {
            let mut chars = other.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        }
    }
}

/// GET /courses/{id}/users — the people page (the People tab's html_url).
///
/// Any enrolled user or admin can read the roster (the course gate already
/// ran); only teachers/admins get the "Add People" button and the row
/// conclude/reactivate forms. `?added=1` / `?concluded=1` /
/// `?reactivated=1` (the PRG targets) render the success notices.
pub async fn course_people(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<NoticeQuery>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let roster: Vec<RosterRow> =
        match star_enrollments::ui::load_course_roster(&app.db, course_id).await {
            Ok(rows) => rows,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };
    let rows: Vec<RosterRowView> = roster
        .into_iter()
        .map(|r| RosterRowView {
            enrollment_id: r.enrollment_id,
            user_id: r.user_id,
            name: r.name,
            email: r.email,
            role_label: role_label(&r.role),
            state: r.workflow_state.clone(),
            state_label: state_label(&r.workflow_state),
        })
        .collect();

    let mut notices = Vec::new();
    if query.added.is_some() {
        notices.push("Person added to the course.".to_string());
    }
    if query.concluded.is_some() {
        notices.push("Enrollment concluded.".to_string());
    }
    if query.reactivated.is_some() {
        notices.push("Enrollment reactivated.".to_string());
    }

    let env = course_env(&ctx, &user, &origin, "people", course_id);
    let (jar, csrf_token) = ensure_csrf(jar);

    let breadcrumb = course_breadcrumb(&ctx, course_id);

    let page = CoursePeoplePage {
        title: format!("{} - People", ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} people"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices,
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb,
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "people".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        rows,
        is_teacher: ctx.is_teacher,
    };
    (jar, page).into_response()
}

/// The add-people form template context (Canvas "Add People" wizard,
/// single-step no-JS equivalent).
#[derive(Template)]
#[template(path = "course_people_add.html")]
pub struct AddPeoplePage {
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
    pub tabs: Vec<CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub login_value: String,
    /// "student" | "teacher" | "ta".
    pub selected_role: String,
    /// Validation error re-rendered above the form.
    pub error: String,
}

impl IntoResponse for AddPeoplePage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("add people template render failed: {err}")).into_response()
            }
        }
    }
}

/// The add-people form fields.
#[derive(Deserialize, Debug, Clone)]
pub struct AddPeopleForm {
    pub _csrf: String,
    /// Login (email) of the user to enroll.
    pub login: String,
    /// "student" | "teacher" | "ta".
    #[serde(default)]
    pub role: Option<String>,
}

async fn render_add_people(
    app: &AppState,
    user: &SessionUser,
    course_id: i64,
    form: Option<&AddPeopleForm>,
    error: String,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();
    let ctx = match course_context(app, user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let (login_value, selected_role) = match form {
        Some(form) => (
            form.login.clone(),
            form.role.clone().unwrap_or_else(|| "student".to_string()),
        ),
        None => (String::new(), "student".to_string()),
    };

    let env = course_env(&ctx, user, &origin, "people", course_id);
    let (jar, csrf_token) = ensure_csrf(jar);

    let breadcrumb = course_breadcrumb(&ctx, course_id);

    let page = AddPeoplePage {
        title: format!("{} - Add People", ctx.course.name),
        body_class: format!(
            "with-left-side course-menu-expanded context-course_{course_id} people form"
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
        active_context_tab: "people".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        login_value,
        selected_role,
        error,
    };
    (jar, page).into_response()
}

/// GET /courses/{id}/users/add — teacher-only enrollment creation form.
pub async fn course_people_add(
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

    render_add_people(&app, &user, course_id, None, String::new(), jar).await
}

/// POST /courses/{id}/users/add — create the enrollment.
///
/// Reuses the enrollments REST create validation exactly (via
/// `star_enrollments::ui::enroll_user_in_course`): unknown login -> 422
/// re-render, invalid role -> 422 re-render, duplicate enrollment -> 422
/// re-render with the conflict message. Success is a 303 PRG to the people
/// page where the new roster row appears.
pub async fn course_people_add_submit(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<AddPeopleForm>,
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

    let role = form.role.clone().unwrap_or_else(|| "student".to_string());

    let login = form.login.trim().to_string();
    if login.is_empty() {
        let mut resp = render_add_people(
            &app,
            &user,
            course_id,
            Some(&form),
            "Login is required.".to_string(),
            jar,
        )
        .await;
        *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
        return resp;
    }

    let user_id = match star_enrollments::ui::find_user_id_by_login(&app.db, &login).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            let mut resp = render_add_people(
                &app,
                &user,
                course_id,
                Some(&form),
                format!("No user with login {login} exists."),
                jar,
            )
            .await;
            *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            return resp;
        }
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    match star_enrollments::ui::enroll_user_in_course(
        &app.db,
        &crate::handlers::course_context::auth_of(&user),
        course_id,
        user_id,
        &role,
    )
    .await
    {
        Ok(_) => {
            let location = format!("/courses/{course_id}/users?added=1");
            (StatusCode::SEE_OTHER, [("location", location.as_str())]).into_response()
        }
        Err(ApiError::Validation(msg)) => {
            let mut resp = render_add_people(&app, &user, course_id, Some(&form), msg, jar).await;
            *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            resp
        }
        Err(ApiError::Conflict(msg)) => {
            let mut resp = render_add_people(&app, &user, course_id, Some(&form), msg, jar).await;
            *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            resp
        }
        Err(ApiError::NotFound) => {
            let mut resp = render_add_people(
                &app,
                &user,
                course_id,
                Some(&form),
                "No user with that login exists.".to_string(),
                jar,
            )
            .await;
            *resp.status_mut() = StatusCode::UNPROCESSABLE_ENTITY;
            resp
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// POST /courses/{id}/users/{user_id}/conclude — teacher-only, CSRF, PRG.
///
/// Flips the user's non-deleted enrollments in this course to `inactive`
/// (the state the REST reactivate endpoint is the counterpart of).
pub async fn course_people_conclude(
    State(app): State<AppState>,
    Path((course_id, target_user_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<ConcludeForm>,
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

    match star_enrollments::ui::conclude_course_enrollment(&app.db, course_id, target_user_id).await
    {
        Ok(()) => {
            let location = format!("/courses/{course_id}/users?concluded=1");
            (StatusCode::SEE_OTHER, [("location", location.as_str())]).into_response()
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// POST /courses/{id}/users/{user_id}/reactivate — teacher-only, CSRF, PRG.
///
/// Flips inactive enrollments back to `active` (same transition as the REST
/// `PUT /courses/{courseId}/enrollments/{id}/reactivate`).
pub async fn course_people_reactivate(
    State(app): State<AppState>,
    Path((course_id, target_user_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<ConcludeForm>,
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

    match star_enrollments::ui::reactivate_course_enrollment(&app.db, course_id, target_user_id)
        .await
    {
        Ok(()) => {
            let location = format!("/courses/{course_id}/users?reactivated=1");
            (StatusCode::SEE_OTHER, [("location", location.as_str())]).into_response()
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// The CSRF-only form body of the conclude/reactivate row actions.
#[derive(Deserialize, Debug)]
pub struct ConcludeForm {
    pub _csrf: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syllabus_summary_strips_and_collapses() {
        assert_eq!(
            syllabus_summary(Some("<p>Hello   <b>world</b></p>\n<p>Second</p>")),
            "Hello world Second"
        );
        assert_eq!(syllabus_summary(None), "");
        assert_eq!(syllabus_summary(Some("plain text")), "plain text");
    }

    #[test]
    fn role_and_state_labels() {
        assert_eq!(role_label("student"), "Student");
        assert_eq!(role_label("teacher"), "Teacher");
        assert_eq!(role_label("ta"), "Ta");
        assert_eq!(state_label("active"), "Active");
        assert_eq!(state_label("invited"), "Invited");
        assert_eq!(state_label("inactive"), "Inactive");
        assert_eq!(state_label("creation_pending"), "Creation_pending");
    }
}
