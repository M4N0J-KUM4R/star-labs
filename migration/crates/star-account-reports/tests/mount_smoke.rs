//! Mount-smoke test for star-account-reports (scaffold, issue #138).
//!
//! Verifies the crate's router mounts and answers exactly the way star-http
//! assembles it: the implemented domain routes require auth (401 without a
//! token) and genuinely unknown paths fall through to axum's 404.
//!
//! Conventions (mirrors star-enrollments):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-account-reports

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
        .merge(star_account_reports::router())
        .with_state(state);
    Some(TestServer::new(app).expect("build TestServer"))
}

#[tokio::test]
async fn unknown_path_is_404() {
    let Some(server) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // The account-reports routes have landed (issue lane): the real route
    // now answers 401 without auth instead of the scaffold 404.
    let resp = server.get("/accounts/999999999/reports").await;
    assert_eq!(
        resp.status_code(),
        401,
        "implemented route must require auth; body: {}",
        resp.text()
    );
    // A genuinely unknown path still falls through to axum's 404.
    let resp = server
        .get("/accounts/999999999/reports_extra_unknown")
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown paths must 404; body: {}",
        resp.text()
    );
}
