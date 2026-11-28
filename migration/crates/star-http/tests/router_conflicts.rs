//! Guard: the assembled application router must not contain conflicting
//! routes.
//!
//! Why this exists
//! ---------------
//! Every crate's own `router()` builds fine on its own, and `cargo test`
//! compiles the crates — but nothing ever assembled the *whole* application
//! router, so a path registered twice (by two workers porting the same
//! Canvas route, or by a crate and its own sibling module) went unnoticed.
//! axum only detects the conflict when the router is built, and the binary
//! panicked at startup:
//!
//! ```text
//! thread 'main' panicked at crates/star-http/src/main.rs:177:10:
//! Overlapping method route. Handler for `GET /courses/{courseId}/pages/{url}/revisions/latest` already exists
//! ```
//!
//! So `star-http` could not start at all: the route count in the parity
//! report counted *route strings*, not a server that boots.
//!
//! This test assembles the same routers `main.rs` does and fails the build
//! if they conflict. Adding a route that duplicates an existing one now
//! breaks CI instead of the server.

use axum::Router;
use star_common::AppState;

/// The API router, assembled exactly as `star-http/src/main.rs` does.
fn build_api_router() -> Router<AppState> {
    Router::new()
        .merge(star_auth::router())
        .merge(star_users::router())
        .merge(star_courses::router())
        .merge(star_assignments::router())
        .merge(star_content::router())
        .merge(star_files::router())
        .merge(star_enrollments::router())
        .merge(star_dashboard::router())
        .merge(star_quizzes::router())
        .merge(star_assignment_groups::router())
        .merge(star_accounts::router())
        .merge(star_calendar::router())
        .merge(star_conversations::router())
        .merge(star_discussions::router())
        .merge(star_grading_standards::router())
        .merge(star_groups::router())
        .merge(star_notifications::router())
        .merge(star_outcomes::router())
        .merge(star_rubrics::router())
        .merge(star_submission_comments::router())
        .merge(star_jobs::router())
        .merge(star_lti::router())
        .merge(star_permissions::router())
        .merge(star_features::router())
        .merge(star_learning_dates::router())
        .merge(star_audit::router())
        .merge(star_media::router())
        .merge(star_account_reports::router())
        .merge(star_collaborations::router())
        .merge(star_conferences::router())
        .merge(star_eportfolios::router())
        .merge(star_polling::router())
        .merge(star_search::router())
        .merge(star_sis::router())
    // star_pages / star_modules / star_announcements come in through
    // star_content::router(); merging them again double-registers every
    // route in those crates.
}

/// The whole application router, assembled exactly as `main.rs` does.
fn build_app_router() -> Router<AppState> {
    Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .merge(star_ui::router())
        .merge(star_lti::lti_advantage_router())
        .nest("/api/v1", build_api_router())
        .nest("/api/graphql", star_graphql::router())
}

#[test]
fn the_api_router_has_no_conflicting_routes() {
    // axum panics with the offending path when two routes collide, so
    // reaching the assertion means the router is conflict-free.
    let _ = build_api_router();
}

#[test]
fn the_application_router_has_no_conflicting_routes() {
    let _ = build_app_router();
}
