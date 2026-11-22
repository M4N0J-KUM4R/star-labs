//! Dashboard handler — Canvas `users#user_dashboard` (HTML).

use askama::Template;
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};

use star_common::{ApiError, AppState};
use star_dashboard::DashboardCard;

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
    };
    (jar, page).into_response()
}
