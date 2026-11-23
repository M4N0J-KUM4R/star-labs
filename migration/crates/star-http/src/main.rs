//! star-http — the only binary for star-lms.
//!
//! Owns main(), router assembly, auth middleware, and graceful shutdown.

use axum::{routing::get, Json, Router};
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

    let port: u16 = env::var("STAR_PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse()
        .expect("STAR_PORT must be a valid u16");

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
    // Build API router under /api/v1
    // -------------------------------------------------------------------------
    let api_router = build_api_router();

    // -------------------------------------------------------------------------
    // Full app router — /health at top level, everything else under /api/v1
    // -------------------------------------------------------------------------
    let app = Router::new()
        .route("/health", get(health))
        // Server-rendered UI shell (star-ui, spec p5): top level, NOT under
        // /api/v1 — /, /login, /logout, /dashboard, /courses/:id, /static.
        // It is the app's fallback rather than a merge: `star_ui::app` carries
        // Rails' `_method` override, which has to run ahead of routing, and
        // none of the root-mounted routers below own a path the shell
        // registers, so the two shapes answer the same requests.
        .fallback_service(star_ui::app(state.clone()))
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
        .nest("/api/v1", api_router)
        // Canvas serves its GraphQL API at /api/graphql (outside /api/v1).
        .nest("/api/graphql", star_graphql::router())
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
