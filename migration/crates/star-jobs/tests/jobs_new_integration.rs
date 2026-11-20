//! Integration tests for star-jobs — V1 JSON-mode and V2 grouped/management routes.
//!
//! Conventions (mirrors `jobs_integration.rs`):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Unique per-run prefix + full teardown, so repeat runs against a shared
//!   database are safe.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test AppState and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "jobs-new-{}-{}",
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

/// Insert one V2 job row.
async fn seed_job(
    pool: &PgPool,
    tag: &str,
    workflow_state: &str,
    run_at: DateTime<Utc>,
    strand: Option<&str>,
    singleton: Option<&str>,
    shard_id: Option<i64>,
    next_in_strand: bool,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO jobs (tag, workflow_state, progress, run_at, strand, singleton,
                          shard_id, next_in_strand, max_concurrent, priority,
                          locked_by, locked_at, failed_at)
        VALUES ($1, $2, 0, $3, $4, $5, $6, $7, 1, 0, NULL, NULL, NULL)
        RETURNING id
        "#,
    )
    .bind(tag)
    .bind(workflow_state)
    .bind(run_at)
    .bind(strand)
    .bind(singleton)
    .bind(shard_id)
    .bind(next_in_strand)
    .fetch_one(pool)
    .await
    .expect("seed job")
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
    let _ = sqlx::query("DELETE FROM progress WHERE tag LIKE $1")
        .bind(format!("%{}%", prefix))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Route 1: GET /jobs (V1 JSON mode) — only=jobs, only=running, only=tags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v1_list_jobs_json_returns_jobs_and_total() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let _ = seed_user(&pool, &prefix, "teacher", "teacher").await;

    let now = Utc::now();
    let j1 = seed_job(
        &pool,
        &format!("{prefix}-export"),
        "queued",
        now,
        None,
        None,
        None,
        true,
    )
    .await;
    let j2 = seed_job(
        &pool,
        &format!("{prefix}-import"),
        "queued",
        now,
        Some("my_strand"),
        None,
        None,
        true,
    )
    .await;
    let _ = seed_job(
        &pool,
        &format!("{prefix}-running"),
        "running",
        now,
        None,
        None,
        None,
        true,
    )
    .await;

    let bearer = auth(admin, "admin");

    // GET /jobs?only=jobs → {jobs: [...], total: N}
    let resp = server
        .get("/jobs?only=jobs&limit=10")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "GET /jobs?only=jobs; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert!(body.get("jobs").is_some(), "must have 'jobs' key: {}", body);
    assert!(
        body.get("total").is_some(),
        "must have 'total' key: {}",
        body
    );
    let jobs = body["jobs"].as_array().expect("jobs is an array");
    assert!(
        jobs.iter().any(|j| j["id"].as_i64() == Some(j1)),
        "seeded job j1 must appear"
    );

    // only=running → {running: [...]}
    let resp = server
        .get("/jobs?only=running")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert!(
        body.get("running").is_some(),
        "only=running must wrap in 'running': {}",
        body
    );

    // only=tags → {tags: [{tag, count}, ...]}
    let resp = server
        .get("/jobs?only=tags")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert!(
        body.get("tags").is_some(),
        "only=tags must wrap in 'tags': {}",
        body
    );

    // Auth negative: teacher → 403.
    let teacher_bearer = auth(admin, "teacher");
    let resp = server
        .get("/jobs?only=jobs")
        .add_header(AUTHORIZATION, teacher_bearer)
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin must be 403");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Route 2: GET /jobs/:id (V1 JSON mode) — flavor param
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v1_show_job_returns_job_with_all_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    // A queued job.
    let queued = seed_job(
        &pool,
        &format!("{prefix}-show-queued"),
        "queued",
        now,
        None,
        None,
        None,
        true,
    )
    .await;

    // A failed job.
    let failed = seed_job(
        &pool,
        &format!("{prefix}-show-failed"),
        "failed",
        now,
        None,
        None,
        None,
        true,
    )
    .await;
    sqlx::query("UPDATE jobs SET failed_at = now(), last_error = 'boom', original_job_id = 999 WHERE id = $1")
        .bind(failed)
        .execute(&pool)
        .await
        .expect("mark failed");

    let bearer = auth(admin, "admin");

    // GET /jobs/:id → single job object.
    let resp = server
        .get(&format!("/jobs/{queued}"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "GET /jobs/:id; body: {}",
        resp.text()
    );
    let job: Value = resp.json();
    assert_eq!(
        job["id"].as_i64(),
        Some(queued),
        "must return the job object directly"
    );

    // 404 for missing.
    let resp = server
        .get("/jobs/999999999")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "missing job must be 404");

    // flavor=failed: look up in failed table.
    let resp = server
        .get(&format!("/jobs/{failed}?flavor=failed"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let job: Value = resp.json();
    assert_eq!(job["id"].as_i64(), Some(failed));

    // Auth negative: anonymous → 401.
    let resp = server.get(&format!("/jobs/{queued}")).await;
    assert_eq!(resp.status_code(), 401, "anonymous must be 401");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Route 3: POST /jobs/batch_update
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_update_force_delete_and_unblock() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    let j1 = seed_job(
        &pool,
        &format!("{prefix}-del1"),
        "queued",
        now,
        None,
        None,
        None,
        true,
    )
    .await;
    let j2 = seed_job(
        &pool,
        &format!("{prefix}-del2"),
        "queued",
        now,
        None,
        None,
        None,
        true,
    )
    .await;
    let _ = seed_job(
        &pool,
        &format!("{prefix}-strand-job"),
        "queued",
        now,
        Some("my_strand"),
        None,
        None,
        true,
    )
    .await;

    let bearer = auth(admin, "admin");

    // force_delete with job_ids.
    let resp = server
        .post("/jobs/batch_update")
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({
            "update_action": "force_delete",
            "job_ids": [j1, j2]
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "batch_update; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["status"], "OK");
    assert_eq!(body["count"], 2);

    // unblock with strand name.
    let resp = server
        .post("/jobs/batch_update")
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({
            "update_action": "unblock",
            "q": "my_strand"
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "unblock; body: {}", resp.text());

    // Auth negative: teacher → 403.
    let teacher_bearer = auth(admin, "teacher");
    let resp = server
        .post("/jobs/batch_update")
        .add_header(AUTHORIZATION, teacher_bearer)
        .json(&json!({
            "update_action": "force_delete",
            "job_ids": [j1]
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin must be 403");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Route 4: GET /jobs2/:bucket/by_:group/search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grouped_search_returns_tag_counts() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    for _ in 0..3 {
        let _ = seed_job(
            &pool,
            &format!("{prefix}-ExportJob"),
            "queued",
            now,
            None,
            None,
            None,
            true,
        )
        .await;
    }
    for _ in 0..2 {
        let _ = seed_job(
            &pool,
            &format!("{prefix}-ImportJob"),
            "queued",
            now,
            None,
            None,
            None,
            true,
        )
        .await;
    }

    let bearer = auth(admin, "admin");

    // Happy path: search by tag.
    let resp = server
        .get(&format!("/jobs2/queued/by_tag/search?term=Export"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "grouped search; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    // Returns a map like {"ExportJob": 3}.
    assert!(body.is_object(), "search must return an object: {}", body);

    // Bad bucket → 400.
    let resp = server
        .get("/jobs2/invalid/by_tag/search?term=foo")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "invalid bucket must be 400");

    // Auth negative: no auth → 401.
    let resp = server.get("/jobs2/queued/by_tag/search?term=foo").await;
    assert_eq!(resp.status_code(), 401, "anonymous must be 401");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Route 5: GET /jobs2/:bucket/by_:group
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grouped_info_returns_count_and_info() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    for _ in 0..3 {
        let _ = seed_job(
            &pool,
            &format!("{prefix}-strand-A-job"),
            "queued",
            now,
            Some("strand_A"),
            None,
            None,
            true,
        )
        .await;
    }
    let _ = seed_job(
        &pool,
        &format!("{prefix}-strand-B-orphan"),
        "queued",
        now,
        Some("strand_B"),
        None,
        None,
        false,
    )
    .await;

    let bearer = auth(admin, "admin");

    // by_strand: grouped by strand, includes count and info.
    let resp = server
        .get("/jobs2/queued/by_strand")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "grouped_info; body: {}",
        resp.text()
    );
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("must be array");
    assert!(
        rows.iter().any(|r| r["count"].as_i64() == Some(3)),
        "strand_A must have count 3"
    );
    // Orphaned check: strand_B has no next_in_strand job → orphaned = true.
    let strand_b = rows
        .iter()
        .find(|r| r["strand"].as_str() == Some("strand_B"));
    if let Some(r) = strand_b {
        assert_eq!(
            r["orphaned"].as_bool(),
            Some(true),
            "strand_B must be orphaned"
        );
    }

    // Auth negative: teacher → 403.
    let teacher_bearer = auth(admin, "teacher");
    let resp = server
        .get("/jobs2/queued/by_strand")
        .add_header(AUTHORIZATION, teacher_bearer)
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin must be 403");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Route 6: GET /jobs2/:bucket
// ---------------------------------------------------------------------------

#[tokio::test]
async fn v2_list_bucket_jobs_includes_info_field() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    // Queued job.
    let queued = seed_job(
        &pool,
        &format!("{prefix}-q"),
        "queued",
        now - Duration::hours(1),
        None,
        None,
        None,
        true,
    )
    .await;
    // Future job.
    let future = seed_job(
        &pool,
        &format!("{prefix}-f"),
        "queued",
        now + Duration::hours(1),
        None,
        None,
        None,
        true,
    )
    .await;
    // Failed job.
    let failed = seed_job(
        &pool,
        &format!("{prefix}-fail"),
        "failed",
        now,
        None,
        None,
        None,
        true,
    )
    .await;
    sqlx::query("UPDATE jobs SET failed_at = now(), last_error = 'boom' WHERE id = $1")
        .bind(failed)
        .execute(&pool)
        .await
        .expect("mark failed");

    let bearer = auth(admin, "admin");

    // Queued bucket: info is seconds (integer).
    let resp = server
        .get("/jobs2/queued")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Vec<Value> = serde_json::from_value(resp.json()).expect("array");
    let queued_row = rows
        .iter()
        .find(|r| r["id"].as_i64() == Some(queued))
        .expect("queued job");
    assert!(
        queued_row["info"].is_number(),
        "queued info must be integer seconds: {}",
        queued_row["info"]
    );

    // Future bucket: info is ISO timestamp.
    let resp = server
        .get("/jobs2/future")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Vec<Value> = serde_json::from_value(resp.json()).expect("array");
    let future_row = rows
        .iter()
        .find(|r| r["id"].as_i64() == Some(future))
        .expect("future job");
    assert!(
        future_row["info"].as_str().is_some_and(|s| s.contains('T')),
        "future info must be ISO string: {}",
        future_row["info"]
    );

    // Failed bucket: includes failed_at, last_error.
    let resp = server
        .get("/jobs2/failed")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Vec<Value> = serde_json::from_value(resp.json()).expect("array");
    let failed_row = rows
        .iter()
        .find(|r| r["id"].as_i64() == Some(failed))
        .expect("failed job");
    assert!(
        failed_row["last_error"].is_string(),
        "failed job must have last_error"
    );
    assert!(
        failed_row["failed_at"].is_string(),
        "failed job must have failed_at"
    );

    // Auth negative: teacher → 403.
    let teacher_bearer = auth(admin, "teacher");
    let resp = server
        .get("/jobs2/queued")
        .add_header(AUTHORIZATION, teacher_bearer)
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin must be 403");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Route 7: PUT /jobs2/manage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn manage_strand_updates_max_concurrent_and_priority() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    let _ = seed_job(
        &pool,
        &format!("{prefix}-manage-job"),
        "queued",
        now,
        Some("manage_strand"),
        None,
        None,
        true,
    )
    .await;

    let bearer = auth(admin, "admin");

    // Happy path.
    let resp = server
        .put("/jobs2/manage")
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({
            "strand": "manage_strand",
            "max_concurrent": 5,
            "priority": 10
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "manage; body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "OK");
    assert!(
        body["count"].as_i64().unwrap_or(0) >= 1,
        "must affect at least 1 job"
    );

    // Missing strand → 400.
    let resp = server
        .put("/jobs2/manage")
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "missing strand must be 400");

    // Auth negative: teacher → 403.
    let teacher_bearer = auth(admin, "teacher");
    let resp = server
        .put("/jobs2/manage")
        .add_header(AUTHORIZATION, teacher_bearer)
        .json(&json!({"strand": "any", "max_concurrent": 1}))
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin must be 403");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Route 8: PUT /jobs2/unstuck
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unstuck_strand_mode_and_async_progress_mode() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    let _ = seed_job(
        &pool,
        &format!("{prefix}-stuck-job"),
        "queued",
        now,
        Some("stuck_strand"),
        None,
        None,
        false,
    )
    .await;

    let bearer = auth(admin, "admin");

    // Strand mode: unsticks the strand.
    let resp = server
        .put("/jobs2/unstuck")
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({"strand": "stuck_strand"}))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "unstuck strand; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["status"], "OK");
    assert!(body["count"].as_i64().unwrap_or(0) >= 1);

    // Auth negative: teacher → 403.
    let teacher_bearer = auth(admin, "teacher");
    let resp = server
        .put("/jobs2/unstuck")
        .add_header(AUTHORIZATION, teacher_bearer)
        .json(&json!({"strand": "any"}))
        .await;
    assert_eq!(resp.status_code(), 403, "non-admin must be 403");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn unstuck_async_returns_progress_record() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let bearer = auth(admin, "admin");

    // Async mode: no strand/singleton → returns progress record with 202.
    let resp = server
        .put("/jobs2/unstuck")
        .add_header(AUTHORIZATION, bearer.clone())
        .json(&json!({}))
        .await;
    assert_eq!(
        resp.status_code(),
        202,
        "async unstuck must be 202; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["status"], "pending");
    assert!(
        body.get("progress").is_some(),
        "must have progress record: {}",
        body
    );
    let progress = &body["progress"];
    assert!(progress["id"].is_number(), "progress.id must be set");
    assert_eq!(progress["tag"], "JobsV2Controller::run_unstucker!");
    assert_eq!(progress["workflow_state"], "queued");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Route 9: GET /jobs2/throttle/check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn throttle_check_returns_matched_counts() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    // Tag prefix "Export" with 3 matching unspecificed jobs.
    for _ in 0..3 {
        let _ = seed_job(
            &pool,
            &format!("{prefix}-ExportJob"),
            "queued",
            now,
            None,
            None,
            None,
            true,
        )
        .await;
    }
    // A different tag.
    let _ = seed_job(
        &pool,
        &format!("{prefix}-ImportJob"),
        "queued",
        now,
        None,
        None,
        None,
        true,
    )
    .await;

    let bearer = auth(admin, "admin");

    // Happy path.
    let resp = server
        .get("/jobs2/throttle/check?term=Export")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "throttle_check; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert!(
        body["matched_jobs"].is_number(),
        "must have matched_jobs: {}",
        body
    );
    assert!(
        body["matched_tags"].is_number(),
        "must have matched_tags: {}",
        body
    );

    // Missing term → 400.
    let resp = server
        .get("/jobs2/throttle/check")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "missing term must be 400");

    // Auth negative: anonymous → 401.
    let resp = server.get("/jobs2/throttle/check?term=Export").await;
    assert_eq!(resp.status_code(), 401, "anonymous must be 401");

    cleanup(&pool, &prefix).await;
}
