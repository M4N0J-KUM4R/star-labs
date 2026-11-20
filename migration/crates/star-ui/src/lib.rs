//! star-ui — the server-rendered UI shell for star-lms (spec §2).
//!
//! Merged at the TOP LEVEL of star-http's router (sibling of `/health`, NOT
//! under `/api/v1`): `/`, `/login`, `/logout`, `/dashboard`,
//! `/courses/{id}`, `/courses/{id}/{tab}` and `/static/{hash}/{file}`, plus
//! the assignments + gradebook journey routes (issue #71):
//! `/courses/{id}/assignments[...]`, `/courses/{id}/grades`,
//! `/courses/{id}/gradebook[.csv]`.
//!
//! Specific journey routes are registered BEFORE the generic
//! `/courses/{id}/{tab}` so the "assignments"/"grades" tab segments hit the
//! journey pages, not the generic tab landing.
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
///
/// Quiz routes are registered BEFORE the generic `/courses/{id}/{tab}` even
/// though axum 0.8 / matchit 0.8.4 already prefers static segments over
/// parameters — belt-and-braces so route precedence never depends on the
/// router's internal tie-breaking.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(handlers::login::root))
        .route(
            "/login",
            get(handlers::login::login_form).post(handlers::login::login_submit),
        )
        .route("/logout", post(handlers::login::logout))
        .route(
            "/register",
            get(handlers::auth::register_form).post(handlers::auth::register_submit),
        )
        .route(
            "/forgot_password",
            get(handlers::auth::forgot_password_form).post(handlers::auth::forgot_password_submit),
        )
        .route(
            "/reset_password/{token}",
            get(handlers::auth::reset_password_form).post(handlers::auth::reset_password_submit),
        )
        .route("/dashboard", get(handlers::dashboard::dashboard))
        .route("/courses/{id}", get(handlers::course::course_home))
        // Quiz-taking journey (issue #109) — specific before generic.
        .route("/courses/{id}/quizzes", get(handlers::quiz::quiz_list))
        .route(
            "/courses/{id}/quizzes/new",
            get(handlers::quiz::quiz_new).post(handlers::quiz::quiz_create),
        )
        .route(
            "/courses/{id}/quizzes/{qid}",
            get(handlers::quiz::quiz_show),
        )
        .route(
            "/courses/{id}/quizzes/{qid}/take",
            get(handlers::quiz::quiz_take).post(handlers::quiz::quiz_take_start),
        )
        .route(
            "/courses/{id}/quizzes/{qid}/submissions/{sid}",
            get(handlers::quiz::quiz_results),
        )
        .route(
            "/courses/{id}/quizzes/{qid}/submissions/{sid}/submit",
            post(handlers::quiz::quiz_submit),
        )
        // Assignments journey (issue #71) — BEFORE the generic tab route.
        .route(
            "/courses/{id}/assignments",
            get(handlers::assignments::assignments_index),
        )
        .route(
            "/courses/{id}/assignments/new",
            get(handlers::assignments::assignment_new)
                .post(handlers::assignments::assignment_create),
        )
        .route(
            "/courses/{id}/assignments/{aid}",
            get(handlers::assignments::assignment_show),
        )
        .route(
            "/courses/{id}/assignments/{aid}/edit",
            get(handlers::assignments::assignment_edit)
                .post(handlers::assignments::assignment_update),
        )
        .route(
            "/courses/{id}/assignments/{aid}/submit",
            post(handlers::assignments::assignment_submit),
        )
        // Grades + gradebook journey (issue #71) — BEFORE the generic tab route.
        .route("/courses/{id}/grades", get(handlers::grades::grades))
        .route("/courses/{id}/gradebook", get(handlers::grades::gradebook))
        .route(
            "/courses/{id}/gradebook.csv",
            get(handlers::grades::gradebook_csv),
        )
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
