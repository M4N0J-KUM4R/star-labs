//! star-worker — the background job worker process.
//!
//! Canvas runs its background work in a separate process, started by
//! `script/delayed_job` (`canvas-src/script/delayed_job`) and configured by
//! `config/delayed_jobs.yml`. This binary is that process for star-lms: it
//! claims the due job rows, runs the handler each row names and records the
//! result.
//!
//! Configuration (every value has the Canvas default):
//!
//! | Variable | Default | Canvas source |
//! |---|---|---|
//! | `DATABASE_URL` | — (required) | the database the queue lives in |
//! | `STAR_WORKER_NAME` | `star-worker-<pid>` | `config/initializers/delayed_job.rb:83` |
//! | `STAR_WORKER_POLL_INTERVAL_MS` | `2000` | `config/initializers/delayed_job.rb:81` |
//! | `STAR_WORKER_BATCH_SIZE` | `20` | `config/initializers/delayed_job.rb:74` |
//! | `STAR_WORKER_LEASE_SECONDS` | `300` | the port's claim lease |
//! | `STAR_WORKER_QUEUE` | `canvas_queue` | `config/initializers/delayed_job.rb:80` |
//! | `STAR_WORKER_COUNT` | `1` | `config/delayed_jobs.yml:2-7` runs several pools |

use std::env;

use sqlx::postgres::PgPoolOptions;
use star_worker::{run_loop, WorkerConfig};
use tokio::sync::watch;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // -----------------------------------------------------------------------
    // Tracing — RUST_LOG defaults to "info"
    // -----------------------------------------------------------------------
    let rust_log = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(rust_log))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // -----------------------------------------------------------------------
    // Configuration — fail fast, never echo a secret
    // -----------------------------------------------------------------------
    let database_url = env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL must be set (PostgreSQL connection string)")?;
    if database_url.trim().is_empty() {
        return Err("DATABASE_URL is set but empty".into());
    }

    let config = WorkerConfig::from_env()?;
    let worker_count: usize = env::var("STAR_WORKER_COUNT")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(1)
        .clamp(1, 32);

    // -----------------------------------------------------------------------
    // Database pool + migrations
    // -----------------------------------------------------------------------
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&database_url)
        .await?;

    star_db::MIGRATOR.run(&pool).await?;

    // -----------------------------------------------------------------------
    // Worker loops
    // -----------------------------------------------------------------------
    let (stop_tx, stop_rx) = watch::channel(false);

    let mut handles = Vec::with_capacity(worker_count);
    for index in 0..worker_count {
        let mut config = config.clone();
        if worker_count > 1 {
            config.worker_name = format!("{}-{}", config.worker_name, index);
        }
        let pool = pool.clone();
        let stop_rx = stop_rx.clone();

        handles.push(tokio::spawn(async move {
            if let Err(error) = run_loop(pool, config, stop_rx).await {
                error!(%error, "worker loop stopped with an error");
            }
        }));
    }

    // -----------------------------------------------------------------------
    // Shutdown — SIGTERM and SIGINT stop the workers cleanly
    // -----------------------------------------------------------------------
    shutdown_signal().await;

    info!("shutdown requested, finishing the running job");
    let _ = stop_tx.send(true);

    for handle in handles {
        let _ = handle.await;
    }

    info!("worker stopped");
    Ok(())
}

/// Wait for SIGTERM or SIGINT.
async fn shutdown_signal() {
    use tokio::signal;

    let mut terminate = match signal::unix::signal(signal::unix::SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            error!(%error, "cannot listen for SIGTERM");
            let _ = signal::ctrl_c().await;
            return;
        }
    };

    tokio::select! {
        _ = signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
