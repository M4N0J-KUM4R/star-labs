//! Course home + tab landing handlers — Canvas `courses#show` HTML branch.

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use star_common::{ApiError, AppState};
use star_courses::{Course, CourseTab};
use star_modules::Module;
use star_pages::FrontPageRow;

use crate::handlers::error::from_api_error;
use crate::session::SessionUser;
use crate::{assets, env::JsEnv, queries};

/// View model for the front-page branch of course home.
pub struct FrontPageView {
    pub title: String,
    pub body: String,
}

/// The course home template context.
#[derive(Template)]
#[template(path = "course_home.html")]
pub struct CourseHomePage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub front_page: Option<FrontPageView>,
    pub modules: Vec<Module>,
}

impl IntoResponse for CourseHomePage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("course home template render failed: {err}")).into_response()
            }
        }
    }
}

/// The generic tab landing template context.
#[derive(Template)]
#[template(path = "course_tab.html")]
pub struct CourseTabPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    pub tabs: Vec<CourseTab>,
    pub active_context_tab: String,
    pub course_name: String,
    pub course_id: i64,
    pub tab_label: String,
}

impl IntoResponse for CourseTabPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("course tab template render failed: {err}")).into_response()
            }
        }
    }
}

/// GET /courses/{id} — course home.
///
/// Authorization: the course load goes through `load_course_for_viewer`,
/// which runs the SAME 404-before-403 enrolled-or-admin gate as
/// `GET /api/v1/courses/{id}` — the shell cannot bypass API authorization
/// (acceptance criterion 7).
///
/// Content branch (phase-1 deviation, documented in the PR): there is no
/// `courses.default_view` column in this schema (the dashboard card API
/// hardcodes "modules"), so course home renders the front page when one is
/// set and the module list otherwise.
pub async fn course_home(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: axum_extra::extract::CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let course: Course =
        match star_courses::load_course_for_viewer(&app.db, course_id, &auth_of(&user)).await {
            Ok(course) => course,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    let tabs = match star_courses::load_course_tabs(&app.db, course_id, &auth_of(&user)).await {
        Ok(tabs) => tabs,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let front_page = match star_pages::load_front_page(&app.db, course_id).await {
        Ok(page) => page,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    // Modules are only needed for the module-list branch.
    let modules = if front_page.is_none() {
        match star_modules::load_modules(&app.db, course_id, 100, 0).await {
            Ok(modules) => modules,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        }
    } else {
        Vec::new()
    };

    let (preferences, use_high_contrast) = match queries::load_preferences(&app.db, user.id).await {
        Ok(loaded) => loaded,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let user_name = match queries::load_user_name(&app.db, user.id).await {
        Ok(name) => name.unwrap_or_default(),
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        preferences,
        use_high_contrast,
        &origin,
        Some("home"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = crate::session::ensure_csrf_cookie(jar);

    let page = CourseHomePage {
        title: course.name.clone(),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} home"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (course.name.clone(), format!("/courses/{course_id}")),
        ],
        csrf_token,
        tabs,
        active_context_tab: "home".to_string(),
        course_name: course.name,
        front_page: front_page.map(|page: FrontPageRow| FrontPageView {
            title: page.title,
            body: page.body.unwrap_or_default(),
        }),
        modules,
    };
    (jar, page).into_response()
}

/// GET /courses/{id}/{tab} — tab landing pages.
///
/// Same gate as course home; `active_context_tab` follows the URL segment so
/// the left-side tab highlight matches the page.
pub async fn course_tab(
    State(app): State<AppState>,
    Path((course_id, tab)): Path<(i64, String)>,
    user: SessionUser,
    jar: axum_extra::extract::CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let course: Course =
        match star_courses::load_course_for_viewer(&app.db, course_id, &auth_of(&user)).await {
            Ok(course) => course,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    let tabs = match star_courses::load_course_tabs(&app.db, course_id, &auth_of(&user)).await {
        Ok(tabs) => tabs,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    // Unknown tab ids render the generic page with the segment as label;
    // the section-tab list is what tests compare against the tabs API.
    let tab_label = tabs
        .iter()
        .find(|t| t.id == tab)
        .map(|t| t.label.clone())
        .unwrap_or_else(|| tab.clone());

    let (preferences, use_high_contrast) = match queries::load_preferences(&app.db, user.id).await {
        Ok(loaded) => loaded,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let user_name = match queries::load_user_name(&app.db, user.id).await {
        Ok(name) => name.unwrap_or_default(),
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        preferences,
        use_high_contrast,
        &origin,
        Some(&tab),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = crate::session::ensure_csrf_cookie(jar);

    let page = CourseTabPage {
        title: format!("{} - {}", course.name, tab_label),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} {tab}"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (course.name.clone(), format!("/courses/{course_id}")),
        ],
        csrf_token,
        tabs,
        active_context_tab: tab,
        course_name: course.name,
        course_id,
        tab_label,
    };
    (jar, page).into_response()
}

/// Build the AuthUser view of a session user for the domain-crate loaders
/// (they take `&AuthUser` for their enrolled-or-admin gates).
fn auth_of(user: &SessionUser) -> star_auth::AuthUser {
    star_auth::AuthUser {
        id: user.id,
        role: user.role.clone(),
    }
}
