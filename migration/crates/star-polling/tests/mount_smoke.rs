//! Mount-smoke test for star-polling.
//!
//! With the domain routes landed (issue #89) the smoke contract is:
//! - real routes are never open: without a bearer token every polling route
//!   answers 401 through the `AuthUser` extractor;
//! - unknown sibling paths still 404 (axum default fallback);
//! - an authenticated caller gets 404 (not 500) for an unknown record id on
//!   a real route, exercising the DB-backed scoped lookup.
//!
//! Conventions (mirrors star-media / star-enrollments):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-polling

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
        .merge(star_polling::router())
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
        "/polls",
        "/polls/1",
        "/polls/1/poll_choices",
        "/polls/1/poll_sessions",
        "/polls/1/poll_sessions/2/open",
        "/poll_sessions/opened",
    ] {
        let resp = server.get(path).await;
        assert_eq!(
            resp.status_code(),
            401,
            "GET {path} without auth must be 401; body: {}",
            resp.text()
        );
    }

    // The submissions collection is POST-only: a GET there is a 405 from the
    // method router (the extractor never runs), while the POST is a 401
    // without a token.
    let resp = server
        .get("/polls/1/poll_sessions/2/poll_submissions")
        .await;
    assert_eq!(
        resp.status_code(),
        405,
        "GET on the POST-only submissions route must be 405; body: {}",
        resp.text()
    );
    let resp = server
        .post("/polls/1/poll_sessions/2/poll_submissions")
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "POST /poll_submissions without auth must be 401; body: {}",
        resp.text()
    );

    // A genuinely unmounted sibling path stays 404.
    let resp = server.get("/polls/1/poll_fractions").await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown path must 404; body: {}",
        resp.text()
    );

    // Authenticated caller, unknown record: the scoped lookup 404s.
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ('polling-smoke', 'polling-smoke-@test.local', 'x', 'teacher') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("seed smoke user");
    let token = star_auth::create_token(user_id, "teacher", JWT_SECRET).expect("mint token");

    let resp = server
        .get("/polls/999999999")
        .add_header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown poll id must be 404; body: {}",
        resp.text()
    );

    let _ = sqlx::query("DELETE FROM polls WHERE user_id = $1")
        .bind(user_id)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(user_id)
        .execute(&pool)
        .await;
}
