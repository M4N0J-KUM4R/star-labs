//! `GET /dashboard` — the card view served as the Svelte shell.
//!
//! Canvas' route is `get "dashboard" => "users#user_dashboard"` in
//! `config/routes.rb` and the view (`app/views/users/user_dashboard.html.erb`) renders
//! only mount points; the cards and the To Do list come from the two API calls the
//! bundle makes (`GET /api/v1/dashboard/dashboard_cards` and
//! `GET /api/v1/users/self/todo`). So this handler does what the browser cannot:
//! authorize the session, read the preferences that pick the stylesheet variant, and
//! hand the page to the bundle. Nothing here queries cards, today items or badges —
//! asking for them server-side as well was a second fetch of data the client was going
//! to request anyway.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;

use star_common::AppState;

use crate::env::JsEnv;
use crate::handlers::error::from_api_error;
use crate::queries;
use crate::session::SessionUser;
use crate::shell;

/// GET /dashboard — the card view (the activity stream view is the same page; the
/// bundle switches on `PREFERENCES.dashboard_layout`, as Canvas' React does).
pub async fn dashboard(State(app): State<AppState>, user: SessionUser, jar: CookieJar) -> Response {
    let origin = crate::request_origin();

    let (preferences, use_high_contrast) = match queries::load_preferences(&app.db, user.id).await {
        Ok(loaded) => loaded,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };
    let settings = match queries::load_settings(&app.db, user.id).await {
        Ok(loaded) => loaded,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    // `active_context_tab` is left unset: Canvas' `user_dashboard` controller sets no
    // explicit tab, and the dashboard's own nav highlight comes from the route the
    // bundle boots on.
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

    // The bundle writes through `/api/v1` and `session_bridge` requires the
    // double-submit pair on an unsafe method, so the shell has to have set `star_csrf`
    // before the first POST — every Askama page already does this (login.rs, profile.rs).
    let (jar, _csrf_token) = crate::session::ensure_csrf_cookie(jar);

    let html = shell::render(
        "/dashboard",
        "Dashboard",
        &env.to_json_string(),
        use_high_contrast,
        false,
    );
    (jar, shell::response(html)).into_response()
}
