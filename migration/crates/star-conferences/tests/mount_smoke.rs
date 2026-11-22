//! Mount-smoke test for star-conferences (scaffold, issue #138; routes
//! landed with issue #91).
//!
//! Verifies the crate's router mounts and answers unknown paths with 404
//! (not 500, not open) exactly the way star-http assembles it.
//!
//! Conventions (mirrors star-polling):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-conferences

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
        .merge(star_conferences::router())
        .with_state(state);
    Some(TestServer::new(app).expect("build TestServer"))
}

#[tokio::test]
async fn unknown_path_is_404() {
    let Some(server) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // A path outside this crate's route set must 404 (axum default
    // fallback), never 500 or open.
    let resp = server.get("/courses/1/conferences/999").await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown paths must 404; body: {}",
        resp.text()
    );
}

#[tokio::test]
async fn routes_mount_and_auth_gate() {
    let Some(server) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // The seven mounted routes answer (401 without a token, except the
    // recording_ready webhooks which take no user at all — those answer
    // 404 for an unknown conference id instead).
    let authed = [
        "/courses/1/conferences",
        "/groups/1/conferences",
        "/conferences",
    ];
    for path in authed {
        let resp = server.get(path).await;
        assert_eq!(
            resp.status_code(),
            401,
            "GET {path} must be auth-gated; body: {}",
            resp.text()
        );
    }
    for path in ["/courses/1/conferences", "/groups/1/conferences"] {
        let resp = server.post(path).json(&serde_json::json!({})).await;
        assert_eq!(resp.status_code(), 401, "POST {path} must be auth-gated");
    }
    for path in [
        "/courses/999999999/conferences/2/recording_ready",
        "/groups/999999999/conferences/2/recording_ready",
    ] {
        let resp = server
            .post(path)
            .json(&serde_json::json!({ "signed_parameters": "bad" }))
            .await;
        assert_eq!(
            resp.status_code(),
            404,
            "POST {path} with unknown ids is 404; body: {}",
            resp.text()
        );
    }
}
