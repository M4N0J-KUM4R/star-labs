//! Mount-smoke test for star-collaborations (routes landed, issue #91).
//!
//! Smoke contract:
//! - real routes are never open: without a bearer token every
//!   collaborations route answers 401 through the `AuthUser` extractor
//!   (GET and DELETE alike);
//! - unknown sibling paths still 404 (axum default fallback);
//! - an authenticated caller gets 404 (not 500) for an unknown context id
//!   on a real route, exercising the DB-backed scoped lookup.
//!
//! Conventions (mirrors star-polling):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-collaborations

use axum_test::TestServer;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

async fn setup() -> Option<(TestServer, PgPool)> {
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

    // Same merge pattern as star-http::build_api_router.
    let app = axum::Router::new()
        .merge(star_collaborations::router())
        .with_state(state);
    Some((TestServer::new(app).expect("build TestServer"), pool))
}

#[tokio::test]
async fn mounted_routes_require_auth_and_unknown_paths_stay_404() {
    let Some((server, pool)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Real GET routes without a token: 401 from the auth extractor, never an
    // open 200 (the scaffold-era expectation of a bare 404 no longer holds
    // once the routes exist).
    for path in [
        "/courses/1/collaborations",
        "/groups/1/collaborations",
        "/collaborations/1/members",
        "/courses/1/potential_collaborators",
        "/groups/1/potential_collaborators",
    ] {
        let resp = server.get(path).await;
        assert_eq!(
            resp.status_code(),
            401,
            "GET {path} without auth must be 401; body: {}",
            resp.text()
        );
    }

    // The destroy routes are DELETE-only: without a token the DELETE is a
    // 401 from the extractor.
    for path in ["/courses/1/collaborations/2", "/groups/1/collaborations/2"] {
        let resp = server.delete(path).await;
        assert_eq!(
            resp.status_code(),
            401,
            "DELETE {path} without auth must be 401; body: {}",
            resp.text()
        );
    }

    // Unknown sibling paths stay 404 (axum default fallback).
    for path in [
        "/courses/1/collaborations/2/members",
        "/accounts/1/collaborations",
        "/collaborations",
    ] {
        let resp = server.get(path).await;
        assert_eq!(
            resp.status_code(),
            404,
            "GET {path} must stay 404; body: {}",
            resp.text()
        );
    }

    // Authenticated caller on a real route with an unknown context id: 404
    // from the scoped lookup, never 500. AuthUser rejects ids with no users
    // row, so a real caller is seeded (and torn down) first.
    let suffix = format!(
        "smoke_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    );
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role)          VALUES ($1, $2, $3, 'teacher') RETURNING id",
    )
    .bind(format!("{suffix}-Teacher"))
    .bind(format!("{suffix}@test.local"))
    .bind("not-a-real-hash")
    .fetch_one(&pool)
    .await
    .expect("seed caller");
    let token = star_auth::create_token(user_id, "teacher", JWT_SECRET).expect("mint token");
    let bearer = format!("Bearer {token}");
    let resp = server
        .get("/courses/999999999/collaborations")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown course must 404; body: {}",
        resp.text()
    );
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("teardown caller");
}
