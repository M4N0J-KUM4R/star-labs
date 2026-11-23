//! star-http library — router assembly shared by the binary and by
//! behavioral-parity tooling (examples/behavior_parity.rs), so probes run
//! against the exact router production serves.

use axum::{routing::get, Json, Router};
use star_common::AppState;
use tower_http::trace::TraceLayer;

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

/// Distinctive fallback 404 so parity tooling can tell axum's "no route
/// matched" from a handler's own empty-body 404 (e.g. a missing resource).
async fn not_matched() -> (axum::http::StatusCode, Json<serde_json::Value>) {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": { "code": "route_not_found", "message": "no route matches this verb and path" }
        })),
    )
}

/// Full app router — `/health` at top level, everything else under `/api/v1`.
/// Same assembly as `main` has always used; the binary adds pool/state/serve.
pub fn build_app() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        // Server-rendered UI shell (star-ui, spec p5): top level, NOT under
        // /api/v1 — /, /login, /logout, /dashboard, /courses/:id, /static.
        .merge(star_ui::router())
        // Canvas serves the LTI Advantage family at /api/lti (outside
        // /api/v1), so the router carries the prefix and is merged here.
        .merge(star_lti::lti_advantage_router())
        // Canvas serves the login/oauth2 family (auth, token, revoke, jwks)
        // at the top level, outside /api/v1 (config/routes.rb:3087-3094).
        .merge(star_auth::login_router())
        // The sis/lti-security gap routes Canvas serves outside /api/v1
        // (the "/api/sis" and "/api/lti" route sets) — root-mounted like
        // the LTI Advantage family above.
        .merge(star_auth::small_controllers_root_routes())
        // The port-C gap routes Canvas serves outside /api/v1:
        // /canvadoc_session (canvadoc_sessions_controller show/create) and
        // /api/sis/courses/:course_id/disable_post_to_sis
        // (disable_post_to_sis_api_controller) — root-mounted like the sets
        // above; the route strings carry their full paths.
        .merge(star_courses::root_routes())
        .nest("/api/v1", build_api_router())
        // Canvas serves its GraphQL API at /api/graphql (outside /api/v1).
        .nest("/api/graphql", star_graphql::router())
        .fallback(not_matched)
        .layer(TraceLayer::new_for_http())
}

/// Assemble all domain routers under /api/v1.
pub fn build_api_router() -> Router<AppState> {
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
        .merge(star_gradebook::router())
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
        // Scaffold crates (issue #138) — routers are empty until the feature
        // workers land their routes; merged here once so they never have to
        // touch this file.
        .merge(star_account_reports::router())
        .merge(star_collaborations::router())
        .merge(star_conferences::router())
        .merge(star_eportfolios::router())
        .merge(star_polling::router())
        .merge(star_search::router())
        .merge(star_sis::router())
    // NOTE: star_pages / star_modules / star_announcements are NOT merged
    // here — star-content/src/lib.rs already merges their routers (lines
    // 56-58). Merging them here as well panics axum at startup with
    // "Overlapping method route" (e.g. GET /pages/{id} registered twice).
}
