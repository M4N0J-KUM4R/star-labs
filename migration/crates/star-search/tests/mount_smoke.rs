//! Mount-smoke test for star-search (scaffold, issue #138).
//!
//! Verifies the crate's router mounts and answers unknown paths with 404
//! (not 500, not open) exactly the way star-http assembles it. No domain
//! routes exist yet — the smoke request targets a path owned by this crate's
//! future route set.
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
async fn unknown_path_is_404() {
    let Some(server) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // A path this crate will own once routes land. With the scaffold's empty
    // router it must 404 (axum default fallback), never 500 or open.
    let resp = server.get("/search/recipients").await;
    assert_eq!(
        resp.status_code(),
        404,
        "scaffold must 404 on unimplemented paths; body: {}",
        resp.text()
    );
}
