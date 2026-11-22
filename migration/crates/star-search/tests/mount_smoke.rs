//! Mount-smoke test for star-search.
//!
//! With the domain routes landed (issue #91) the smoke contract is:
//! - authenticated routes answer 401 without a bearer token (AuthUser
//!   extractor), never an open 200;
//! - unknown sibling paths stay 404 (axum default fallback).
//!
//! Conventions (mirrors star-enrollments):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-search

use axum_test::TestServer;
use star_common::AppState;

async fn setup() -> Option<TestServer> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    let state = AppState {
        db: pool,
        jwt_secret: "test-jwt-secret".to_string(),
    };

    // Same merge pattern as star-http::build_api_router.
    let app = axum::Router::new()
        .merge(star_search::router())
        .with_state(state);
    Some(TestServer::new(app).expect("build TestServer"))
}

#[tokio::test]
async fn mounted_routes_require_auth_and_unknown_paths_stay_404() {
    let Some(server) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Real authed GET routes without a token: 401, never an open 200 (the
    // scaffold-era expectation of a bare 404 no longer holds once the
    // routes exist).
    for path in [
        "/search/recipients",
        "/conversations/find_recipients",
        "/search/rubrics",
        "/courses/1/smartsearch",
        "/courses/1/smartsearch/index_status",
        "/courses/1/smartsearch/log",
    ] {
        let resp = server.get(path).await;
        assert_eq!(
            resp.status_code(),
            401,
            "GET {path} without auth must be 401; body: {}",
            resp.text()
        );
    }

    // The PUBLIC catalog route (search/all_courses) has no auth extractor —
    // its 401-when-catalog-off / 200-when-on behavior is pinned by
    // search_integration.rs (which owns the root-account setting it flips);
    // asserting it here would race that suite's parallel run.

    // A genuinely unmounted sibling path stays 404.
    let resp = server.get("/search/fractions").await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown path must 404; body: {}",
        resp.text()
    );
}
