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
        .nest("/api/v1", api_router)
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
}
