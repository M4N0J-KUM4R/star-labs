//! Integration tests for the slice `lti-3` support-helper routes
//! (`support_helpers/turnitin/*`).
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set.
//! - Unique per-run prefix + full teardown.
//!
//! Scope note: the other half of slice lti-3 (the 11 dynamic_registration
//! routes) is covered by `dynamic_registration_integration.rs` — it was
//! implemented by slice lti-4 (PR #163) and is not re-tested here.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "lti3_tii_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
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

    let server = TestServer::new(star_lti::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) \
         RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Shared assertions
// ---------------------------------------------------------------------------

/// Every support-helper route, in one list, so the auth matrix and the 503
/// contract are exercised uniformly across all 7 endpoints.
const ROUTES: [&str; 7] = [
    "/support_helpers/turnitin/md5",
    "/support_helpers/turnitin/error2305",
    "/support_helpers/turnitin/shard",
    "/support_helpers/turnitin/assignment",
    "/support_helpers/turnitin/pending",
    "/support_helpers/turnitin/expired",
    "/support_helpers/turnitin/refresh_lti_attachment",
];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turnitin_routes_require_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    for route in ROUTES {
        // Anonymous gets 401.
        let resp = server.get(route).await;
        assert_eq!(resp.status_code(), 401, "anonymous {route}");
    }

    let student = seed_user(&pool, &prefix, "student", "student").await;
    for route in ROUTES {
        // Student gets 403.
        let resp = server
            .get(route)
            .add_header(AUTHORIZATION, auth(student, "student"))
            .await;
        assert_eq!(resp.status_code(), 403, "student {route}");
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn md5_computes_rfc1321_digest_of_object_id() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let token = auth(admin, "admin");

    // A known RFC 1321 vector through the HTTP surface.
    let resp = server
        .get("/support_helpers/turnitin/md5?object_id=abc")
        .add_header(AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["object_id"], "abc");
    assert_eq!(body["md5"], "900150983cd24fb0d6963f7d28e17f72");

    // Empty / absent object_id digests the empty string.
    let resp = server
        .get("/support_helpers/turnitin/md5")
        .add_header(AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["md5"], "d41d8cd98f00b204e9800998ecf8427e");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn data_dependent_turnitin_routes_stub_503_for_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    for route in ROUTES {
        let resp = server
            .get(route)
            .add_header(AUTHORIZATION, auth(admin, "admin"))
            .await;
        assert_eq!(resp.status_code(), 503, "admin {route}: {}", resp.text());
        let body: Value = resp.json();
        assert_eq!(
            body["error"], "turnitin_integration_not_configured",
            "route {route}"
        );
    }

    cleanup(&pool, &prefix).await;
}
