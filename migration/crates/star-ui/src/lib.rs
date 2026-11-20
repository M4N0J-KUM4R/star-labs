//! star-ui — the server-rendered UI shell for star-lms (spec §2).
//!
//! Merged at the TOP LEVEL of star-http's router (sibling of `/health`, NOT
//! under `/api/v1`): `/`, `/login`, `/logout`, `/dashboard`,
//! `/courses/{id}`, `/courses/{id}/{tab}` and `/static/{hash}/{file}`.
//!
//! Auth is the `star_session` cookie (HttpOnly, SameSite=Lax) verified by
//! the exact same path as the API's Bearer `AuthUser`; shell POSTs are CSRF
//! double-submit protected; `/api/v1` is untouched.

pub mod assets;
pub mod env;
pub mod handlers;
pub mod queries;
pub mod session;

use axum::routing::{get, post};
use axum::Router;

use star_common::AppState;

/// The shell router. Mounted via `.merge(star_ui::router())` at the top level
/// of the app router in star-http.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(handlers::login::root))
        .route(
            "/login",
            get(handlers::login::login_form).post(handlers::login::login_submit),
        )
        .route("/logout", post(handlers::login::logout))
        .route("/dashboard", get(handlers::dashboard::dashboard))
        .route("/courses/{id}", get(handlers::course::course_home))
        .route("/courses/{id}/{tab}", get(handlers::course::course_tab))
        .route("/static/{hash}/{file}", get(assets::serve_static))
}

/// Request origin for `ENV.DEEP_LINKING_POST_MESSAGE_ORIGIN`.
///
/// No host header is available without threading the request through every
/// handler, so this mirrors what the tests exercise: an http origin on the
/// local host. When a real deployment needs LTI deep-linking validation, this
/// should read the Host/X-Forwarded-Proto headers.
pub fn request_origin() -> String {
    "http://localhost".to_string()
}
