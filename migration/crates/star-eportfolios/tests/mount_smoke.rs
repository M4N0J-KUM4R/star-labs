//! Mount-smoke test for star-eportfolios (scaffold train, issue #138;
//! routes landed with issue #91).
//!
//! Verifies the crate's router mounts and answers unknown paths with 404
//! (not 500, not open) exactly the way star-http assembles it.
//!
//! Conventions (mirrors star-enrollments):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-eportfolios

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
        .merge(star_eportfolios::router())
        .with_state(state);
    Some(TestServer::new(app).expect("build TestServer"))
}

#[tokio::test]
async fn unknown_path_is_404() {
    let Some(server) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // A path this crate owns, but with a segment no route matches: must be
    // 404 (axum default fallback), never 500 or open.
    let resp = server.get("/eportfolios/1/nope").await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown path must 404; body: {}",
        resp.text()
    );

    // Anonymous hits on mounted routes answer 401 (the AuthUser extractor),
    // proving the domain routes really mounted.
    let resp = server.get("/users/1/eportfolios").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server.get("/eportfolios/1").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
}
