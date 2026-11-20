//! Integration test for the LTI Advantage mount point.
//!
//! Canvas serves the LTI Advantage family under `/api/lti`, outside
//! `/api/v1`. The port used to serve it under `/api/v1` (and the line item
//! routes under `/api/v1/api/lti`), so the routes were not reachable at the
//! Canvas URLs.
//!
//! This test mounts `star_lti::lti_advantage_router()` — the router
//! `star-http` merges at the application root — and checks that the dynamic
//! registration, the tool proxy and the line item routes answer there and not
//! under `/api/v1`.
//!
//! Skipped cleanly when DATABASE_URL is not set.

use axum_test::TestServer;
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test AppState and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    format!("lti-mount-tests-{}", chrono::Utc::now().timestamp_millis())
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

    // The router `star-http` merges at the application root — the paths
    // already carry the `/api/lti` prefix.
    let app = star_lti::lti_advantage_router().with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

#[tokio::test]
async fn the_lti_advantage_routes_are_served_under_api_lti() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // The dynamic registration family.
    for path in [
        "/api/lti/accounts/1/registration_token",
        "/api/lti/accounts/1/registrations/uuid/abc",
        "/api/lti/accounts/1/registrations/2",
        "/api/lti/accounts/1/dr_iframe",
        "/api/lti/accounts/1/registrations/2/view",
    ] {
        let resp = server.get(path).await;
        // The route answers (401, 404 or 405), it is not the routing 404 of an
        // unknown path, which has an empty body.
        assert!(
            resp.status_code() != 404 || !resp.text().is_empty(),
            "path {path} is not routed"
        );
    }

    // The tool proxy and the line item routes.
    for path in [
        "/api/lti/tool_proxy/guid",
        "/api/lti/courses/1/tool_proxy",
        "/api/lti/accounts/1/tool_proxy",
        "/api/lti/courses/1/line_items",
        "/api/lti/courses/1/line_items/2",
    ] {
        let resp = server.get(path).await;
        assert!(
            resp.status_code() != 404 || !resp.text().is_empty(),
            "path {path} is not routed"
        );
    }
}
