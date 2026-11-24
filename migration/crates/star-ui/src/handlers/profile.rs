//! `GET /profile` — the first page served as the Svelte shell.
//!
//! Canvas' route is `resource :profile, controller: :profile, only: [:show,
//! :update]` (config/routes.rb:1045), which draws `GET /profile` →
//! `ProfileController#show` (app/controllers/profile_controller.rb:177) rendering
//! app/views/profile/show.html.erb. The tabs API already points at this path —
//! `("profile", "Profile", "/profile")` in star-courses/src/lib.rs:4753, asserted by
//! `tabs_integration.rs:283` — and until now nothing served it, so the profile link in
//! the global nav was a 404.
//!
//! The handler's job is only what Canvas' controller job that the browser cannot do:
//! authorize the session, read the preferences that decide which stylesheet set and
//! which `window.ENV` the page gets, and hand the rest to the bundle. The page's data
//! comes from `/api/v1/users/:id/courses` like Canvas' own React page fetches it, which
//! is why nothing here queries courses.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::CookieJar;

use star_common::AppState;

use crate::env::JsEnv;
use crate::handlers::error::from_api_error;
use crate::queries;
use crate::session::SessionUser;
use crate::shell;

pub async fn profile(State(app): State<AppState>, user: SessionUser, jar: CookieJar) -> Response {
    let origin = crate::request_origin();

    // Preferences decide presentation, not data: `use_high_contrast` selects the
    // variant directory (see `shell::variant`) and the same value goes into ENV so a
    // client-side navigation reloads from that directory too.
    let (preferences, use_high_contrast) = match queries::load_preferences(&app.db, user.id).await {
        Ok(loaded) => loaded,
        Err(e) => return from_api_error(&e, &origin).into_response(),
    };

    // `active_context_tab` is "profile" because that is the tab the profile page marks
    // active in Canvas' own nav (`_new_nav_header.html.erb`), and the SPA reads it from
    // ENV rather than recomputing it.
    let env = JsEnv::for_user(
        user.id,
        &user.role,
        preferences,
        use_high_contrast,
        &origin,
        Some("profile"),
        "",
        Vec::new(),
    );

    // The bundle writes through `/api/v1`, and `session_bridge` requires the
    // double-submit pair on an unsafe method. `star_csrf` is the half the browser
    // can read, so the shell has to have set it before the first POST — every
    // Askama page already does this (login.rs:89) and so does the dashboard shell
    // (dashboard.rs:57).
    let (jar, _csrf_token) = crate::session::ensure_csrf_cookie(jar);

    // The dyslexic half of Canvas' variant matrix is not reachable yet: it needs
    // `ENV.PREFERENCES.user_system_default_font_family`, which `EnvPreferences` does not
    // carry. CANVAS_CSS.md "Known gap" tracks it alongside the RTL variants.
    let html = shell::render(
        "/profile",
        "Profile",
        &env.to_json_string(),
        use_high_contrast,
        false,
    );
    (jar, shell::response(html)).into_response()
}
