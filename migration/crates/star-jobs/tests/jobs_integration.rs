//! Integration tests for star-jobs against a real Postgres.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
//!   cargo test -p star-jobs
//! - Unique per-run prefix + full teardown, so repeat runs against a shared
//!   database are safe.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test AppState and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so repeated runs against a shared DB never collide.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "jobs-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, migrate, and build a TestServer over the jobs router.
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

    let server = TestServer::new(star_jobs::router().with_state(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
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
    let _ = sqlx::query("DELETE FROM jobs WHERE tag LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_lists_and_shows_enqueued_job() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let tag = format!("{prefix}-export");

    // Enqueue an internal job row (no HTTP create route, no worker).
    let job = star_jobs::enqueue_job(&pool, Some(owner), &tag, Some("Course"), Some(4242))
        .await
        .expect("enqueue job");
    assert_eq!(job.workflow_state, "queued");
    assert_eq!(job.progress, 0);

    // GET /jobs — newest first, Canvas job shape.
    let resp = server
        .get("/jobs")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "GET /jobs as admin; body: {}",
        resp.text()
    );
    let list: Value = resp.json();
    let jobs = list.as_array().expect("job list is a JSON array");
    let found = jobs
        .iter()
        .find(|j| j["id"].as_i64() == Some(job.id))
        .expect("enqueued job appears in the list");

    assert_eq!(found["tag"], tag.as_str());
    assert_eq!(found["workflow_state"], "queued");
    assert_eq!(found["progress"].as_i64(), Some(0));
    assert_eq!(found["user_id"].as_i64(), Some(owner));
    assert_eq!(found["context_type"], "Course");
    assert_eq!(found["context_id"].as_i64(), Some(4242));
    assert!(found["created_at"].is_string(), "created_at is serialized");
    assert!(found["updated_at"].is_string(), "updated_at is serialized");

    // GET /jobs/{id} — same object.
    let resp = server
        .get(&format!("/jobs/{}", job.id))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "GET /jobs/{{id}} as admin; body: {}",
        resp.text()
    );
    let detail: Value = resp.json();
    assert_eq!(detail["id"].as_i64(), Some(job.id));
    assert_eq!(detail["tag"], tag.as_str());

    // GET /jobs/{id} for a missing job is 404.
    let resp = server
        .get("/jobs/999999999")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "missing job must be 404");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn cancel_is_idempotent_and_rejects_completed_jobs() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let tag = format!("{prefix}-cancel");

    let job = star_jobs::enqueue_job(&pool, None, &tag, None, None)
        .await
        .expect("enqueue job");

    // First cancel: queued -> cancelled.
    let resp = server
        .post(&format!("/jobs/{}/cancel", job.id))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "POST cancel queued job; body: {}",
        resp.text()
    );
    let cancelled: Value = resp.json();
    assert_eq!(cancelled["id"].as_i64(), Some(job.id));
    assert_eq!(cancelled["workflow_state"], "cancelled");

    // Retry: idempotent, updated_at is not bumped.
    let resp = server
        .post(&format!("/jobs/{}/cancel", job.id))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "cancel is idempotent; body: {}",
        resp.text()
    );
    let again: Value = resp.json();
    assert_eq!(again["workflow_state"], "cancelled");
    assert_eq!(
        again["updated_at"], cancelled["updated_at"],
        "re-cancelling must not touch updated_at"
    );

    // A completed job cannot be cancelled.
    let done = star_jobs::enqueue_job(&pool, None, &format!("{prefix}-done"), None, None)
        .await
        .expect("enqueue job");
    sqlx::query("UPDATE jobs SET workflow_state = 'completed', progress = 100 WHERE id = $1")
        .bind(done.id)
        .execute(&pool)
        .await
        .expect("mark job completed");

    let resp = server
        .post(&format!("/jobs/{}/cancel", done.id))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        409,
        "cancel of a completed job must conflict; body: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_filters_by_workflow_state_and_paginates() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let queued = star_jobs::enqueue_job(&pool, None, &format!("{prefix}-queued"), None, None)
        .await
        .expect("enqueue queued job");
    let done = star_jobs::enqueue_job(&pool, None, &format!("{prefix}-done"), None, None)
        .await
        .expect("enqueue done job");
    sqlx::query("UPDATE jobs SET workflow_state = 'completed', progress = 100 WHERE id = $1")
        .bind(done.id)
        .execute(&pool)
        .await
        .expect("mark job completed");

    // workflow_state filter narrows the listing to matching states only.
    let resp = server
        .get("/jobs?workflow_state=queued&limit=100")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "filtered list; body: {}",
        resp.text()
    );
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("job list is a JSON array");
    assert!(
        rows.iter()
            .all(|j| j["workflow_state"].as_str() == Some("queued")),
        "only queued jobs may be returned: {rows:?}"
    );
    assert!(
        rows.iter().any(|j| j["id"].as_i64() == Some(queued.id)),
        "queued job must be in the filtered list"
    );
    assert!(
        !rows.iter().any(|j| j["id"].as_i64() == Some(done.id)),
        "completed job must not be in the queued list"
    );

    // limit is honoured (and clamped server-side, never zero).
    let resp = server
        .get("/jobs?limit=1")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(1),
        "limit=1 returns exactly one row"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn non_admin_is_forbidden_and_anonymous_is_unauthorized() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let job = star_jobs::enqueue_job(&pool, None, &format!("{prefix}-auth"), None, None)
        .await
        .expect("enqueue job");
    let bearer = auth(teacher, "teacher");

    // Authenticated non-admin: Canvas returns 403 for non-site-admins.
    for resp in [
        server
            .get("/jobs")
            .add_header(AUTHORIZATION, bearer.clone())
            .await,
        server
            .get(&format!("/jobs/{}", job.id))
            .add_header(AUTHORIZATION, bearer.clone())
            .await,
        server
            .post(&format!("/jobs/{}/cancel", job.id))
            .add_header(AUTHORIZATION, bearer.clone())
            .await,
    ] {
        assert_eq!(
            resp.status_code(),
            403,
            "non-admin must be forbidden; body: {}",
            resp.text()
        );
    }

    // Anonymous callers are rejected by the auth extractor.
    let resp = server.get("/jobs").await;
    assert_eq!(resp.status_code(), 401);
    let resp = server.post(&format!("/jobs/{}/cancel", job.id)).await;
    assert_eq!(resp.status_code(), 401);

    cleanup(&pool, &prefix).await;
}
