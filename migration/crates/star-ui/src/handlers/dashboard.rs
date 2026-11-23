//! Dashboard handler — Canvas `users#user_dashboard` (HTML).

use askama::Template;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use star_common::{ApiError, AppState};
use star_dashboard::{CourseBadge, DashboardCard, TodayItem};

use crate::handlers::error::from_api_error;
use crate::session::SessionUser;
use crate::{assets, env::JsEnv, queries};

/// The dashboard page template context.
#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardPage {
    pub title: String,
    pub body_class: String,
    pub css_url: String,
    pub env_json: String,
    pub notices: Vec<String>,
    pub user_name: String,
    pub active_path: &'static str,
    pub breadcrumb: Vec<(String, String)>,
    pub csrf_token: String,
    /// Unread conversation count for the global-nav Inbox badge (#72).
    pub unread_badge: i64,
    pub cards: Vec<DashboardCard>,
    /// Today panel rows: due-soon and overdue work, soonest first.
    pub today: Vec<TodayRow>,
    /// Per-course card badges keyed by course id.
    pub badges: std::collections::HashMap<i64, BadgeCounts>,
}

/// Template-facing row for the Today panel (dates preformatted server-side).
pub struct TodayRow {
    pub title: String,
    pub course_code: String,
    pub href: String,
    pub due_label: String,
    pub overdue: bool,
}

/// Template-facing per-course counts.
pub struct BadgeCounts {
    pub new_announcements: i64,
    pub recent_feedback: i64,
}

impl IntoResponse for DashboardPage {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response(),
            Err(err) => {
                ApiError::Db(format!("dashboard template render failed: {err}")).into_response()
            }
        }
    }
}

/// GET /dashboard — the card view (phase 1 renders cards only; the activity
/// stream view is phase 2 per spec §4).
pub async fn dashboard(
    State(app): State<AppState>,
    user: SessionUser,
    jar: axum_extra::extract::CookieJar,
) -> Response {
    let origin = crate::request_origin();

    let cards = match star_dashboard::load_dashboard_cards(&app.db, user.id, &user.role).await {
        Ok(cards) => cards,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let (preferences, use_high_contrast) = match queries::load_preferences(&app.db, user.id).await {
        Ok(loaded) => loaded,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let settings = match queries::load_settings(&app.db, user.id).await {
        Ok(s) => s,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let user_name = match queries::load_user_name(&app.db, user.id).await {
        Ok(name) => name.unwrap_or_default(),
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let unread_badge = match star_conversations::ui::load_unread_count(&app.db, user.id).await {
        Ok(count) => count,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let today_items = match star_dashboard::load_today_items(&app.db, user.id).await {
        Ok(items) => items,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let course_badges = match star_dashboard::load_course_badges(&app.db, user.id).await {
        Ok(badges) => badges,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    let today: Vec<TodayRow> = today_items
        .iter()
        .map(
            |TodayItem {
                assignment_id,
                title,
                course_id,
                course_code,
                due_at,
                overdue,
            }| TodayRow {
            title: title.clone(),
            course_code: course_code.clone(),
            href: format!("/courses/{course_id}/assignments/{assignment_id}"),
            due_label: due_at.format("%b %-d, %l:%M %P").to_string(),
            overdue: *overdue,
        })
        .collect();
    let badges: std::collections::HashMap<i64, BadgeCounts> = course_badges
        .into_iter()
        .map(
            |CourseBadge {
                 course_id,
                 new_announcements,
                 recent_feedback,
             }| {
                (
                    course_id,
                    BadgeCounts {
                        new_announcements,
                        recent_feedback,
                    },
                )
            },
        )
        .collect();

    let mut env = JsEnv::for_user(
        user.id,
        &user.role,
        preferences,
        use_high_contrast,
        &origin,
        None,
        "",
        Vec::new(),
    );
    env.settings = settings;

    let (jar, csrf_token) = crate::session::ensure_csrf_cookie(jar);

    let page = DashboardPage {
        title: "Dashboard".to_string(),
        body_class: "primary-nav-expanded with-right-side".to_string(),
        css_url: assets::app_css_url(),
        env_json: env.to_json_string(),
        notices: Vec::new(),
        user_name,
        active_path: "dashboard",
        breadcrumb: vec![("Dashboard".to_string(), "/dashboard".to_string())],
        csrf_token,
        unread_badge,
        cards,
        today,
        badges,
    };
    (jar, page).into_response()
}
