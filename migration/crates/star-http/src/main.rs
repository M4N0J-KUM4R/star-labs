//! star-http — the only binary for star-lms.
//!
//! Owns main(), router assembly, auth middleware, and graceful shutdown.

use axum::{middleware, routing::get, Extension, Json, Router};
use sqlx::postgres::PgPoolOptions;
use star_common::AppState;
use std::{env, net::SocketAddr};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

// ---------------------------------------------------------------------------
// Health endpoint
// ---------------------------------------------------------------------------

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

// ---------------------------------------------------------------------------
// Shutdown signal
// ---------------------------------------------------------------------------

async fn shutdown_signal() {
    use tokio::signal;

    tokio::select! {
        _ = signal::ctrl_c() => {}
        _ = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .unwrap()
                .recv()
                .await;
        } => {}
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // -------------------------------------------------------------------------
    // Tracing init — RUST_LOG defaults to "info"
    // -------------------------------------------------------------------------
    let rust_log = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(rust_log))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // -------------------------------------------------------------------------
    // Required configuration - fail fast at startup, never echo secret values
    // -------------------------------------------------------------------------
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set (PostgreSQL connection string)")?;
    if database_url.trim().is_empty() {
        return Err("DATABASE_URL is set but empty".into());
    }

    let jwt_secret = env::var("STAR_JWT_SECRET")
        .map_err(|_| "STAR_JWT_SECRET must be set (JWT session signing secret)")?;
    if jwt_secret.trim().is_empty() {
        return Err("STAR_JWT_SECRET is set but empty".into());
    }

    // The port: STAR_PORT is the deployment's own setting, PORT is the
    // conventional 12-factor name and is what the parity replay harness sets
    // when it starts the binary on the port it picked for the run.
    let port: u16 = env::var("STAR_PORT")
        .or_else(|_| env::var("PORT"))
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .expect("STAR_PORT/PORT must be a valid u16");

    // -------------------------------------------------------------------------
    // Database pool + migrations
    // -------------------------------------------------------------------------
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&database_url)
        .await?;

    star_db::MIGRATOR.run(&pool).await?;

    // -------------------------------------------------------------------------
    // App state
    // -------------------------------------------------------------------------
    let state = AppState {
        db: pool,
        jwt_secret,
    };

    // -------------------------------------------------------------------------
    // The cutover front door
    // -------------------------------------------------------------------------
    // The port answers the routes it owns; the routes it does not own yet go
    // to the Canvas instance the deployment still runs. Without
    // STAR_CANVAS_ORIGIN there is nothing to delegate to and the front door
    // is the empty 404 axum would answer anyway, so the parity replay sees
    // only the port's own answers.
    let origin = match star_proxy::CanvasOrigin::from_env() {
        Ok(origin) => origin.map(std::sync::Arc::new),
        Err(error) => return Err(Box::new(error) as Box<dyn std::error::Error + Send + Sync>),
    };
    if let Some(origin) = &origin {
        info!(origin = %origin.base(), "the front door delegates to Canvas");
    }

    // -------------------------------------------------------------------------
    // Build API router under /api/v1
    // -------------------------------------------------------------------------
    // The API router's own fallback is the front door: a path under
    // /api/v1 that the port does not answer is Canvas' to answer, and the
    // nested router would otherwise answer 404 on its own.
    let api_router = build_api_router(origin.clone()).fallback(star_proxy::fallback);

    // -------------------------------------------------------------------------
    // Full app router — /health at top level, everything else under /api/v1
    // -------------------------------------------------------------------------
    // The Canvas response middleware wraps the whole application: Canvas
    // sets its security, context and pagination headers on every answer, so
    // they have to be set on every answer here too. It sits outermost so it
    // sees the final status, headers and body.
    let app = Router::new()
        .route("/health", get(health))
        // Server-rendered UI shell (star-ui, spec p5): top level, NOT under
        // /api/v1 — /, /login, /logout, /dashboard, /courses/:id, /static.
        // It is the app's fallback rather than a merge: `star_ui::app` carries
        // Rails' `_method` override, which has to run ahead of routing, and
        // none of the root-mounted routers below own a path the shell
        // registers, so the two shapes answer the same requests.
        // The front door sits behind the shell: the shell is the app's
        // fallback (it carries Rails' `_method` override, which has to run
        // ahead of routing), so the delegation has to be the shell's own
        // fallback. A request the shell does not answer goes to Canvas.
        .fallback_service(star_ui::app_with_fallback(
            state.clone(),
            star_proxy::fallback,
        ))
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
        // Canvas' browser code authenticates the web session, not an API token:
        // `AnnouncementsApiController < ApplicationController`
        // (app/controllers/announcements_api_controller.rb:26) and `window.ENV`
        // carries no access token (app/controllers/application_controller.rb:264-309).
        // This layer gives the Svelte shell the same shape — it rewrites the
        // `star_session` cookie into the `Authorization` header `AuthUser` already
        // verifies, behind an origin check plus the CSRF double-submit on unsafe
        // methods. Bearer clients are untouched: an existing header short-circuits it.
        .nest(
            "/api/v1",
            api_router.layer(middleware::from_fn(
                star_ui::session_bridge::bridge_session_to_bearer,
            )),
        )
        // Canvas serves its GraphQL API at /api/graphql (outside /api/v1).
        .nest("/api/graphql", star_graphql::router())
        // The front door: a request the port has no route for is Canvas' to
        // answer. The shell is the app's fallback service (it carries the
        // `_method` override), so the delegation is the shell's own fallback
        // — a fallback set here would replace the shell.
        .method_not_allowed_fallback(star_proxy::fallback)
        .layer(Extension(origin.clone()))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            star_canvas_response::canvas_response,
        ))
        .with_state(state)
        .layer(TraceLayer::new_for_http());

    // -------------------------------------------------------------------------
    // Bind + serve
    // -------------------------------------------------------------------------
    let addr: SocketAddr = format!("0.0.0.0:{port}")
        .parse()
        .expect("invalid socket address");

    info!("listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Assemble all domain routers under /api/v1
// ---------------------------------------------------------------------------

fn build_api_router(origin: Option<std::sync::Arc<star_proxy::CanvasOrigin>>) -> Router<AppState> {
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
        // A path the port knows but the method it does not serve is Canvas' to
        // answer: Canvas answers an undefined method with 404, never 405.
        .method_not_allowed_fallback(star_proxy::fallback)
        .layer(Extension(origin))
    // star_pages, star_modules and star_announcements are NOT merged
    // here: star_content::router() already merges all three. Adding them
    // again registered every route in those crates twice, and axum 0.8
    // panics on a duplicate method ("Overlapping method route") — the
    // application could not start at all.
    // crates/star-http/tests/router_conflicts.rs is the gate for this.
    // NOTE: star_pages / star_modules / star_announcements are NOT merged
    // here — star-content/src/lib.rs already merges their routers (lines
    // 56-58). Merging them here as well panics axum at startup with
    // "Overlapping method route" (e.g. GET /pages/{id} registered twice).
}
