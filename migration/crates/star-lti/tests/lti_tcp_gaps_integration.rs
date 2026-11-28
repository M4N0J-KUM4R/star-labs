//! Integration test for the LTI tool consumer profile optional profile-id
//! path variants (P5 gap): Canvas routes
//! `/api/lti/courses/:course_id/tool_consumer_profile(/:tool_consumer_profile_id)`
//! and the accounts sibling — both the id-less and the profile-id spellings
//! resolve to the default platform TCP document.
//!
//! The routes live in the `/api/lti` route set (outside /api/v1), so this
//! mounts `star_lti::lti_advantage_router()` exactly like `star-http` does.
//!
//! Skipped cleanly when DATABASE_URL is not set.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    // The counter and the process id make the prefix unique: two tests
    // that start in the same millisecond would otherwise seed the same
    // email and collide on the users' unique key.
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "lti-tcp-tests-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
        n
    )
}

async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };

    // The router star-http merges at the application root — the paths
    // already carry the /api/lti prefix.
    let app = star_lti::lti_advantage_router().with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

#[tokio::test]
async fn tool_consumer_profile_optional_id_variants() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Any context id renders a TCP document (the handler is unauthenticated
    // and context-agnostic), so seeded rows are unnecessary — but keep the
    // seeded ids unique per run for cleanliness.
    let _course: i64 =
        sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix}-course"))
            .bind(format!("{prefix}-course"))
            .fetch_one(&pool)
            .await
            .expect("seed course");

    for url in [
        "/api/lti/courses/1/tool_consumer_profile",
        "/api/lti/courses/1/tool_consumer_profile/default",
        "/api/lti/accounts/1/tool_consumer_profile",
        "/api/lti/accounts/1/tool_consumer_profile/uuid-1",
    ] {
        let resp = server.get(url).await;
        assert_eq!(resp.status_code(), 200, "{url}: {}", resp.text());
        let tcp = resp.json::<Value>();
        assert_eq!(tcp["@type"], "ToolConsumer", "{url}: {tcp}");
        assert!(tcp["id"]
            .as_str()
            .is_some_and(|id| id.ends_with("/default")));
        assert!(tcp["service_offered"]
            .as_array()
            .is_some_and(|services| !services.is_empty()));
    }

    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await;
}
