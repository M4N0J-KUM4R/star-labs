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

/// The other half of the same axis: routes Canvas declares *inside*
/// `ApiRouteSet::V1.draw` must answer under `/api/v1`, and must not be
/// reachable at the bare path the root mount would give them.
///
/// Canvas: `get`/`post "accounts/:account_id/asset_processors/tii_migrations"`
/// and `.../migrate_all` (`canvas-src/config/routes.rb:2217-2219`), plus the
/// `support_helpers/asset_processor`, `.../asset_processor_discussion`,
/// `.../plagiarism_platform` and `.../submission_lifecycle_manage` scopes
/// (`:2510-2532`). The TII-migration and support-helper registrations used to
/// sit in `lti_gaps_advantage_routes()` — merged at the application root, the
/// mount for the `/api/lti` family — so they answered `/accounts/1/...` and
/// `/support_helpers/...` and nothing at the Canvas URL.
#[tokio::test]
async fn the_api_v1_served_gap_routes_answer_under_api_v1_only() {
    let Some((server, _pool, _prefix)) = setup_v1_and_root().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // (verb, tail) with the tail Canvas uses, `/api/v1` prefix added below.
    let served = [
        ("get", "/accounts/1/asset_processors/tii_migrations"),
        ("post", "/accounts/1/asset_processors/tii_migrations"),
        (
            "post",
            "/accounts/1/asset_processors/tii_migrations/migrate_all",
        ),
        ("get", "/support_helpers/asset_processor/submission_details"),
        ("post", "/support_helpers/asset_processor/bulk_resubmit"),
        (
            "post",
            "/support_helpers/asset_processor_discussion/bulk_resubmit",
        ),
        ("get", "/support_helpers/plagiarism_platform/add_service"),
        (
            "get",
            "/support_helpers/plagiarism_platform/resubmit_for_assignment/1",
        ),
        ("get", "/support_helpers/submission_lifecycle_manage/course"),
    ];

    for (verb, tail) in served {
        let path = format!("/api/v1{tail}");
        let resp = match verb {
            "post" => server.post(&path).await,
            _ => server.get(&path).await,
        };
        // Reaching the handler and being refused (401 without a token) still
        // proves the route exists; only an unrouted path is an empty 404.
        assert!(
            resp.status_code() != 404 || !resp.text().is_empty(),
            "{path} is not routed under /api/v1"
        );

        let bare = server.get(tail).await;
        assert_eq!(
            bare.status_code(),
            404,
            "{tail} is reachable at the root; Canvas serves it under /api/v1"
        );
        assert!(
            bare.text().is_empty(),
            "{tail} answered a body, so a route exists at the root"
        );
    }
}

/// The mount `star-http` actually builds: the crate router nested at `/api/v1`
/// and the Advantage router merged at the application root.
async fn setup_v1_and_root() -> Option<(TestServer, PgPool, String)> {
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
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_lti::lti_advantage_router())
            .nest("/api/v1", star_lti::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}
