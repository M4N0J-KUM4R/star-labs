//! star-http — the only binary for star-lms.
//!
//! Owns main() only: config, pool, state, serve. Router assembly lives in
//! the library (`src/lib.rs`) so parity tooling drives the same router.

use sqlx::postgres::PgPoolOptions;
use star_common::AppState;
use std::{env, net::SocketAddr};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

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
        return Err("STAR_JWT_SECRET must be set".into());
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

    let state = AppState {
        db: pool,
        jwt_secret,
    };

    let app = star_http::build_app().with_state(state);

    // -------------------------------------------------------------------------
    // Bind + serve
    // -------------------------------------------------------------------------
    let addr: SocketAddr = format!("0.0.0.0:{port}").parse().expect("invalid socket address");
    info!("listening on {addr}");

    axum::serve(TcpListener::bind(addr).await?, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}
