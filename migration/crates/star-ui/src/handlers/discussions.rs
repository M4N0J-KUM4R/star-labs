//! Discussions handlers — Canvas `discussion_topics#index` / `#new` / `#show`
//! server-rendered port (issue #72).
//!
//! Routes (registered in `lib.rs` BEFORE the generic `/courses/{id}/{tab}`
//! route so they win the match):
//! - GET  /courses/{id}/discussions                — index (also served at
//!   /courses/{id}/discussion_topics, the html_url the tabs API emits)
//! - GET  /courses/{id}/discussions/new            — new-topic form (teacher)
//! - POST /courses/{id}/discussions                — create topic (teacher), PRG
//! - GET  /courses/{id}/discussions/{topic_id}     — topic show, marks read
//! - POST /courses/{id}/discussions/{topic_id}/entries — post entry or reply
//!
//! All data access goes through the `star_discussions::ui` pub loaders, which
//! run the same enrolled-or-admin / teacher-or-admin gates as the REST API.
//! Deviation (documented): entries render FLAT because the threaded `/view`
//! endpoint is missing in the Rust API; the new-topic form omits
//! `delayed_post` (no column).

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::{CookieJar, Form};
use serde::Deserialize;

use star_common::{ApiError, AppState};
use star_conversations::ui as conversations_ui;
use star_courses::CourseTab;
use star_discussions::ui as discussions_ui;
use star_discussions::TopicIndexRow;

use crate::handlers::error::from_api_error;
use crate::session::{check_csrf, ensure_csrf_cookie, SessionUser};
use crate::{assets, env::JsEnv, queries};

/// CSRF rejection page shared by every POST below.
fn csrf_rejected() -> Response {
    (
        StatusCode::FORBIDDEN,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        "<h1>403 Forbidden</h1><p>Invalid or missing CSRF token.</p>".to_string(),
    )
        .into_response()
}

/// Build the AuthUser view of a session user for the domain-crate loaders.
fn auth_of(user: &SessionUser) -> star_auth::AuthUser {
    star_auth::AuthUser {
        id: user.id,
        role: user.role.clone(),
    }
}

/// A topic row in the index view model (adds the derived unread flag).
pub struct TopicRow {
    pub id: i64,
    pub title: String,
    pub author_name: String,
    pub entry_count: i64,
    pub unread: bool,
    pub pinned: bool,
    pub locked: bool,
    pub last_reply_at: Option<String>,
}

/// The discussions index template context.
#[derive(Template)]
#[template(path = "discussions_index.html")]
pub struct DiscussionsIndexPage {
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
    pub topics: Vec<TopicRow>,
    pub can_create: bool,
}

impl IntoResponse for DiscussionsIndexPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("discussions index template render failed: {err}"))
                .into_response(),
        }
    }
}

/// The new-topic form template context (teacher only).
#[derive(Template)]
#[template(path = "discussion_new.html")]
pub struct DiscussionNewPage {
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
    pub error: String,
}

impl IntoResponse for DiscussionNewPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("discussion new template render failed: {err}"))
                .into_response(),
        }
    }
}

/// An entry in the topic show view model.
pub struct EntryView {
    pub id: i64,
    pub author_name: String,
    pub message: String,
    pub created_at: String,
    pub parent_id: Option<i64>,
}

/// The topic show template context.
#[derive(Template)]
#[template(path = "discussion_show.html")]
pub struct DiscussionShowPage {
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
    pub topic_id: i64,
    pub topic_title: String,
    pub topic_message: String,
    pub topic_author: String,
    pub created_at: String,
    pub pinned: bool,
    pub locked: bool,
    pub unread: bool,
    pub entries: Vec<EntryView>,
}

impl IntoResponse for DiscussionShowPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => ApiError::Db(format!("discussion show template render failed: {err}"))
                .into_response(),
        }
    }
}

/// Load the course context shared by every discussions page. The course load
/// runs the SAME 404-before-403 enrolled-or-admin gate as the REST API, so an
/// unenrolled non-admin never sees discussion data.
struct CourseContext {
    course_name: String,
    tabs: Vec<CourseTab>,
    preferences: crate::env::EnvPreferences,
    use_high_contrast: bool,
    user_name: String,
    unread_badge: i64,
}

async fn load_course_context(
    app: &AppState,
    course_id: i64,
    user: &SessionUser,
    origin: &str,
) -> Result<CourseContext, Response> {
    let course = star_courses::load_course_for_viewer(&app.db, course_id, &auth_of(user))
        .await
        .map_err(|e| from_api_error(&e, origin).into_response())?;
    let tabs = star_courses::load_course_tabs(&app.db, course_id, &auth_of(user))
        .await
        .map_err(|e| from_api_error(&e, origin).into_response())?;
    let (preferences, use_high_contrast) = queries::load_preferences(&app.db, user.id)
        .await
        .map_err(|e| from_api_error(&e, origin).into_response())?;
    let user_name = queries::load_user_name(&app.db, user.id)
        .await
        .map_err(|e| from_api_error(&e, origin).into_response())?
        .unwrap_or_default();
    let unread_badge = conversations_ui::load_unread_count(&app.db, user.id)
        .await
        .map_err(|e| from_api_error(&e, origin).into_response())?;
    Ok(CourseContext {
        course_name: course.name,
        tabs,
        preferences,
        use_high_contrast,
        user_name,
        unread_badge,
    })
}

/// GET /courses/{id}/discussions (alias: /courses/{id}/discussion_topics) —
/// the topic index. Teachers see the "+ Discussion" button; the unread
/// marker is per-viewer from `discussion_topic_reads`.
pub async fn discussions_index(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match load_course_context(&app, course_id, &user, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    let rows: Vec<TopicIndexRow> =
        match discussions_ui::load_topic_index(&app.db, course_id, &auth_of(&user)).await {
            Ok(rows) => rows,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    let can_create =
        match star_courses::check_teacher_or_admin(&app.db, user.id, course_id, &auth_of(&user))
            .await
        {
            Ok(flag) => flag,
            Err(_) => false,
        };

    let topics: Vec<TopicRow> = rows
        .into_iter()
        .map(|row| TopicRow {
            id: row.id,
            title: row.title,
            author_name: row.author_name.unwrap_or_else(|| "Unknown".to_string()),
            entry_count: row.entry_count,
            // Unread when no read row exists or the row says unread.
            unread: row.read_state.as_deref() != Some("read"),
            pinned: row.pinned,
            locked: row.locked,
            last_reply_at: row
                .last_reply_at
                .map(|ts| ts.format("%b %-d, %Y").to_string()),
        })
        .collect();

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        Some("discussions"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = DiscussionsIndexPage {
        title: format!("Discussions - {}", ctx.course_name),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} discussions"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course_name.clone(), format!("/courses/{course_id}")),
            (
                "Discussions".to_string(),
                format!("/courses/{course_id}/discussions"),
            ),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "discussions".to_string(),
        course_name: ctx.course_name,
        course_id,
        topics,
        can_create,
    };
    (jar, page).into_response()
}

/// GET /courses/{id}/discussions/new — the new-topic form, teacher-gated
/// (Canvas `user_can_create` via moderate_forum). Students get the 403 page.
pub async fn discussion_new(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match load_course_context(&app, course_id, &user, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    let teacher =
        match star_courses::check_teacher_or_admin(&app.db, user.id, course_id, &auth_of(&user))
            .await
        {
            Ok(flag) => flag,
            Err(_) => false,
        };
    if !teacher {
        return from_api_error(&ApiError::Forbidden, &origin).into_response();
    }

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        Some("discussions"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = DiscussionNewPage {
        title: format!("New Discussion - {}", ctx.course_name),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} discussions"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course_name.clone(), format!("/courses/{course_id}")),
            (
                "Discussions".to_string(),
                format!("/courses/{course_id}/discussions"),
            ),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "discussions".to_string(),
        course_name: ctx.course_name,
        course_id,
        error: String::new(),
    };
    (jar, page).into_response()
}

/// The new-topic form payload.
#[derive(Deserialize)]
pub struct TopicForm {
    pub _csrf: String,
    pub title: String,
    pub message: String,
    /// "on" from the checkbox, absent when unchecked.
    pub pinned: Option<String>,
    /// "on" from the checkbox, absent when unchecked.
    pub locked: Option<String>,
}

/// POST /courses/{id}/discussions — create a topic (teacher), then redirect
/// to the new topic's page (POST/Redirect/GET so a refresh never re-posts).
pub async fn discussion_create(
    State(app): State<AppState>,
    Path(course_id): Path<i64>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<TopicForm>,
) -> Response {
    let origin = crate::request_origin();

    if !check_csrf(&jar, &form._csrf) {
        return csrf_rejected();
    }

    let new_topic = discussions_ui::NewTopic {
        title: form.title,
        message: if form.message.trim().is_empty() {
            None
        } else {
            Some(form.message)
        },
        discussion_type: None,
        pinned: form.pinned.as_deref() == Some("on"),
        locked: form.locked.as_deref() == Some("on"),
    };

    match discussions_ui::create_topic_for_viewer(&app.db, course_id, &auth_of(&user), &new_topic)
        .await
    {
        // PRG: land on the freshly created topic page.
        Ok(topic_id) => {
            Redirect::to(&format!("/courses/{course_id}/discussions/{topic_id}")).into_response()
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}

/// GET /courses/{id}/discussions/{topic_id} — topic show. Marks the topic
/// read on view (Canvas `discussion_topics#show` marks read unless locked),
/// renders the topic body plus the FLAT entry list.
pub async fn discussion_show(
    State(app): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let ctx = match load_course_context(&app, course_id, &user, &origin).await {
        Ok(ctx) => ctx,
        Err(resp) => return resp,
    };

    let detail =
        match discussions_ui::load_topic_for_viewer(&app.db, course_id, topic_id, &auth_of(&user))
            .await
        {
            Ok(detail) => detail,
            Err(e) => return from_api_error(&e, &origin).into_response(),
        };

    // Mark read on view (same upsert the API's PUT .../read performs). The
    // pre-mark unread state still renders in the page's marker; a failure to
    // write the read row is non-fatal — the page renders, the state lags.
    let was_unread =
        match discussions_ui::load_topic_read_state(&app.db, course_id, topic_id, &auth_of(&user))
            .await
        {
            Ok(unread) => unread,
            Err(_) => false,
        };
    let _ = discussions_ui::mark_topic_read_for_viewer(
        &app.db,
        course_id,
        topic_id,
        &auth_of(&user),
        true,
    )
    .await;

    let entries: Vec<EntryView> = detail
        .entries
        .into_iter()
        .map(|entry| EntryView {
            id: entry.id,
            author_name: entry
                .author_name
                .or(entry.user_id.map(|id| format!("User {id}")))
                .unwrap_or_else(|| "Unknown".to_string()),
            message: entry.message.unwrap_or_default(),
            created_at: entry.created_at.format("%b %-d, %Y %H:%M").to_string(),
            parent_id: entry.parent_id,
        })
        .collect();

    let context_asset_string = format!("course_{course_id}");
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        ctx.preferences,
        ctx.use_high_contrast,
        &origin,
        Some("discussions"),
        &context_asset_string,
        Vec::new(),
    );

    let (jar, csrf_token) = ensure_csrf_cookie(jar);

    let page = DiscussionShowPage {
        title: format!("{} - {}", detail.title, ctx.course_name),
        body_class: format!(
            "with-left-side course-menu-expanded context-{context_asset_string} discussions"
        ),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name: ctx.user_name,
        active_path: "courses",
        breadcrumb: vec![
            ("Dashboard".to_string(), "/dashboard".to_string()),
            (ctx.course_name.clone(), format!("/courses/{course_id}")),
            (
                "Discussions".to_string(),
                format!("/courses/{course_id}/discussions"),
            ),
            (detail.title.clone(), String::new()),
        ],
        csrf_token,
        unread_badge: ctx.unread_badge,
        tabs: ctx.tabs,
        active_context_tab: "discussions".to_string(),
        course_name: ctx.course_name,
        course_id,
        topic_id,
        topic_title: detail.title,
        topic_message: detail.message.unwrap_or_default(),
        topic_author: detail.author_name.unwrap_or_else(|| "Unknown".to_string()),
        created_at: detail.created_at.format("%b %-d, %Y").to_string(),
        pinned: detail.pinned,
        locked: detail.locked,
        unread: was_unread,
        entries,
    };
    (jar, page).into_response()
}

/// The reply form payload. `parent_id` distinguishes a root entry (None) from
/// an inline reply to a specific entry (Some) — one form target for both of
/// Canvas' POST .../entries and POST .../entries/{eid}/replies.
#[derive(Deserialize)]
pub struct EntryForm {
    pub _csrf: String,
    pub message: String,
    pub parent_id: Option<i64>,
}

/// POST /courses/{id}/discussions/{topic_id}/entries — post a root entry
/// (parent_id absent) or an inline reply (parent_id set), then redirect back
/// to the topic (PRG).
pub async fn discussion_reply(
    State(app): State<AppState>,
    Path((course_id, topic_id)): Path<(i64, i64)>,
    user: SessionUser,
    jar: CookieJar,
    Form(form): Form<EntryForm>,
) -> Response {
    let origin = crate::request_origin();

    if !check_csrf(&jar, &form._csrf) {
        return csrf_rejected();
    }

    let result = match form.parent_id {
        // Inline reply to a specific entry.
        Some(parent_id) => {
            discussions_ui::create_reply_for_viewer(
                &app.db,
                course_id,
                topic_id,
                parent_id,
                &auth_of(&user),
                &form.message,
            )
            .await
        }
        // Root entry on the topic.
        None => {
            discussions_ui::create_entry_for_viewer(
                &app.db,
                course_id,
                topic_id,
                &auth_of(&user),
                &form.message,
                None,
            )
            .await
        }
    };

    match result {
        Ok(_) => {
            Redirect::to(&format!("/courses/{course_id}/discussions/{topic_id}")).into_response()
        }
        Err(e) => from_api_error(&e, &origin).into_response(),
    }
}
