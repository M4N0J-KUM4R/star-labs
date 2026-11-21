//! Mount-smoke test for star-collaborations.
//!
//! Verifies the crate's router mounts exactly the way star-http assembles
//! it: unknown paths answer 404 (not 500, not open) and the collaborations
//! routes are mounted — an anonymous call is rejected with 401 by the
//! `AuthUser` extractor, never 404, 500 or open.
//!
//! Conventions (mirrors star-enrollments):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-collaborations

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
        .merge(star_collaborations::router())
        .with_state(state);
    Some(TestServer::new(app).expect("build TestServer"))
}

#[tokio::test]
async fn unknown_path_is_404() {
    let Some(server) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // A path this crate does not own. It must 404 (axum default fallback),
    // never 500 or open.
    let resp = server.get("/courses/1/unknown_route").await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown paths must 404; body: {}",
        resp.text()
    );
}

#[tokio::test]
async fn collaborations_routes_are_mounted() {
    let Some(server) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // The collaborations routes exist: the `AuthUser` extractor rejects the
    // anonymous call with 401 before the handler runs. A 404 would mean the
    // route is not mounted.
    let resp = server.get("/courses/1/collaborations").await;
    assert_eq!(
        resp.status_code(),
        401,
        "the collaborations route must be mounted; body: {}",
        resp.text()
    );
}
