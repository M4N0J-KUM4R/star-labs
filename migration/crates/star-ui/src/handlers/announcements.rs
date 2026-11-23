//! Announcements + syllabus handlers — Canvas `announcements#index/#show/#new`
//! and `courses#syllabus` server-rendered port (issue #111).
//!
//! Routes (registered in `lib.rs` BEFORE the generic `/courses/{id}/{tab}`
//! route so they win the match):
//! - GET  /courses/{id}/announcements                     — index (?page=N)
//! - GET  /courses/{id}/announcements/new                 — new form (teacher)
//! - POST /courses/{id}/announcements                     — create (teacher), PRG
//! - GET  /courses/{id}/announcements/{aid}               — show, marks read
//! - GET  /courses/{id}/assignments/syllabus              — syllabus page
//!
//! Canvas' web routes for these are `concerns :announcements`
//! (`config/routes.rb:266/:702/:796`) expanding `resources :announcements`
//! (`:154-156`, with no `only:`/`except:`), which gives GET/POST on the
//! collection, GET on the member and the `new` page, plus the syllabus page as
//! the `collection { get :syllabus }` inside `resources :assignments`
//! (`:344-346`). The verbs this port does not register — member
//! DELETE/PATCH/PUT, and any write to the syllabus body — go through Rack's
//! `_method` override, which these server-rendered forms do not speak, so they
//! are deliberately absent.
//!
//! All data access goes through the `star_announcements::ui` pub loaders,
//! which run the same enrolled-or-admin / teacher-or-admin gates as the REST
//! API. Deviations (documented, all three verified against the Ruby source):
//! - The index has no author column (the schema has no announcements.author)
//!   and the RSS/atom link is a static placeholder; Canvas renders that page
//!   from a JS bundle (`announcements_controller.rb:78`) and publishes
//!   `/announcements/:feed_code.:format` (`config/routes.rb:657-658`).
//! - Canvas' `AnnouncementsController` implements only `index`, `show` and
//!   `public_feed` (`:64/:105/:109`) — there is no `create`/`new` action, so
//!   the routed collection POST would raise ActionNotFound there. This port
//!   answers it, because announcements here are not discussion topics: Canvas
//!   creates them as `discussion_topics` with `is_announcement`
//!   (`discussion_topics_controller.rb:579`), a flag this workspace's
//!   discussions port does not yet carry.
//! - `announcement#show` renders instead of redirecting: Canvas 302s to the
//!   topic URL (`announcements_controller.rb:105-107`, because an announcement
//!   *is* a topic there), but our `announcements` table is standalone
//!   (`star-db/migrations/0001_core_schema.sql`) with no topic id to point at.
//! - The syllabus page renders the stored body only, with no inline editor.

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;

use star_announcements::ui as announcements_ui;
use star_common::{ApiError, AppState};

use crate::handlers::course_context::{auth_of, course_context};
use crate::handlers::error::from_api_error;
use crate::session::{check_csrf, ensure_csrf_cookie, SessionUser};
use crate::{assets, env::JsEnv};

/// CSRF rejection page shared by every POST below.
fn csrf_rejected() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
    )
        .into_response()
}

/// An announcement row in the index view model.
pub struct AnnouncementRow {
    pub id: i64,
    pub title: String,
    pub posted_at: Option<String>,
    pub unread: bool,
}

/// The announcements index template context.
#[derive(Template)]
#[template(path = "announcements_index.html")]
pub struct AnnouncementsIndexPage {
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
    pub announcements: Vec<AnnouncementRow>,
    pub can_create: bool,
    pub page: i64,
    pub has_prev: bool,
    pub has_next: bool,
}

impl IntoResponse for AnnouncementsIndexPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("announcements index template render failed: {err}"))
                .into_response(),
        }
    }
}

/// The announcement show template context.
#[derive(Template)]
#[template(path = "announcement_show.html")]
pub struct AnnouncementShowPage {
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
    pub announcement_title: String,
    pub message: String,
    pub posted_at: Option<String>,
}

impl IntoResponse for AnnouncementShowPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("announcement show template render failed: {err}"))
                .into_response(),
        }
    }
}

/// The new-announcement form template context (teacher only).
#[derive(Template)]
#[template(path = "announcement_new.html")]
pub struct AnnouncementNewPage {
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
    pub error: String,
}

impl IntoResponse for AnnouncementNewPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("announcement new template render failed: {err}"))
                .into_response(),
        }
    }
}

/// One row of the syllabus course-summary table.
pub struct SummaryRow {
    pub id: i64,
    pub title: String,
    pub group_name: String,
    pub due_at: Option<String>,
    pub points_possible: Option<f64>,
}

/// The syllabus template context (Canvas courses#syllabus: body + summary).
#[derive(Template)]
#[template(path = "syllabus.html")]
pub struct SyllabusPage {
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
    pub syllabus_body: String,
    pub summary: Vec<SummaryRow>,
}

impl IntoResponse for SyllabusPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("syllabus template render failed: {err}")).into_response()
            }
        }
    }
}

/// `?page=N` on the index.
#[derive(Deserialize, Default)]
pub struct IndexQuery {
    pub page: Option<i64>,
}

const PER_PAGE: i64 = 20;

/// GET /courses/{id}/announcements — the index (20 per page like Canvas).
pub async fn announcements_index(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    Query(query): Query<IndexQuery>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let page = query.page.unwrap_or(1).max(1);
    let rows = match announcements_ui::load_index_for_ui(
        &app.db,
        &auth_of(&user),
        course_id,
        page,
        PER_PAGE,
    )
    .await
    {
        Ok(rows) => rows,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let total =
        match announcements_ui::count_index_for_ui(&app.db, &auth_of(&user), course_id).await {
            Ok(n) => n,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    let can_create = ctx.is_teacher;

    let announcements: Vec<AnnouncementRow> = rows
        .into_iter()
        .map(|row| AnnouncementRow {
            id: row.id,
            title: row.title,
            posted_at: row.posted_at.map(|ts| ts.format("%b %-d, %Y").to_string()),
            unread: row.unread,
        })
        .collect();

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        Some("announcements"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page_struct = AnnouncementsIndexPage {
        title: format!("Announcements - {}", ctx.course.name.clone()),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} announcements"
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
                "Announcements".to_string(),
                format!("/courses/{course_id}/announcements"),
            ),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "announcements".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        has_prev: page > 1,
        has_next: page * PER_PAGE < total,
        announcements,
        can_create,
        page,
    };
    (jar, page_struct).into_response()
}

/// GET /courses/{id}/announcements/new — the new-announcement form.
pub async fn announcement_new(
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

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        Some("announcements"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = AnnouncementNewPage {
        title: format!("New Announcement - {}", ctx.course.name.clone()),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} announcements"
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
                "Announcements".to_string(),
                format!("/courses/{course_id}/announcements"),
            ),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "announcements".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        error: String::new(),
    };
    (jar, page).into_response()
}

/// The announcement form payloads.
#[derive(Deserialize)]
pub struct AnnouncementForm {
    pub _csrf: String,
    pub title: String,
    pub message: String,
}

/// POST /courses/{id}/announcements — create (teacher), PRG to the new page.
pub async fn announcement_create(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<AnnouncementForm>,
) -> Response {
    let origin = crate::request_origin();

    if !check_csrf(&jar, &form._csrf) {
        return csrf_rejected();
    }

    match announcements_ui::create_for_ui(
        &app.db,
        &auth_of(&user),
        course_id,
        &form.title,
        &form.message,
    )
    .await
    {
        Ok(announcement_id) => Redirect::to(&format!(
            "/courses/{course_id}/announcements/{announcement_id}"
        ))
        .into_response(),
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// GET /courses/{id}/announcements/{aid} — show; marks the announcement read
/// for the viewer (Canvas marks the topic read on view).
pub async fn announcement_show(
    State(app): State<AppState>,
    Path((course_id, announcement_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match course_context(&app, &user, course_id, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return *resp,
    };

    let detail = match announcements_ui::load_detail_for_ui(
        &app.db,
        &auth_of(&user),
        course_id,
        announcement_id,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        Some("announcements"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = AnnouncementShowPage {
        title: format!("{} - {}", detail.title, ctx.course.name.clone()),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} announcements"
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
                "Announcements".to_string(),
                format!("/courses/{course_id}/announcements"),
            ),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "announcements".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        announcement_title: detail.title,
        message: detail.message,
        posted_at: detail
            .posted_at
            .map(|ts| ts.format("%b %-d, %Y").to_string()),
    };
    (jar, page).into_response()
}

/// GET /courses/{id}/assignments/syllabus — the syllabus page.
pub async fn syllabus_show(
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

    let body =
        match announcements_ui::load_syllabus_for_ui(&app.db, &auth_of(&user), course_id).await {
            Ok(b) => b,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };
    let summary_rows =
        match announcements_ui::load_course_summary_for_ui(&app.db, &auth_of(&user), course_id)
            .await
        {
            Ok(rows) => rows,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    let summary: Vec<SummaryRow> = summary_rows
        .into_iter()
        .map(|row| SummaryRow {
            id: row.id,
            title: row.title,
            group_name: row.group_name,
            due_at: row.due_at.map(|ts| ts.format("%b %-d, %Y").to_string()),
            points_possible: row.points_possible,
        })
        .collect();

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        Some("syllabus"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = SyllabusPage {
        title: format!("Syllabus - {}", ctx.course.name.clone()),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} syllabus"
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
                "Syllabus".to_string(),
                format!("/courses/{course_id}/assignments/syllabus"),
            ),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "assignments".to_string(),
        course_name: ctx.course.name.clone(),
        course_id,
        syllabus_body: body.unwrap_or_default(),
        summary,
    };
    (jar, page).into_response()
}
