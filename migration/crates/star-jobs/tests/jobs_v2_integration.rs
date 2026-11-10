//! Integration tests for the Jobs V2 surface (`/api/v1/jobs2`).
//!
//! Same harness conventions as `tests/jobs_integration.rs`:
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Unique per-run prefix + full teardown, so repeat runs against a shared
//!   database are safe.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use chrono::{DateTime, Duration, Utc};
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
        "jobs2-tests-{}-{}",
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

/// Insert one job row for the bucket/cluster assertions.
async fn seed_job(
    pool: &PgPool,
    tag: &str,
    workflow_state: &str,
    run_at: DateTime<Utc>,
    shard_id: Option<i64>,
    next_in_strand: bool,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO jobs (tag, workflow_state, progress, run_at, shard_id, next_in_strand)
        VALUES ($1, $2, 0, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(tag)
    .bind(workflow_state)
    .bind(run_at)
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
    let _ = sqlx::query("DELETE FROM job_clusters WHERE database_server_id LIKE $1")
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
async fn list_filters_by_bucket_and_emits_canvas_info() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();
    let older = seed_job(
        &pool,
        &format!("{prefix}-queued-older"),
        "queued",
        now - Duration::hours(2),
        None,
        true,
    )
    .await;
    let newer = seed_job(
        &pool,
        &format!("{prefix}-queued-newer"),
        "queued",
        now - Duration::hours(1),
        None,
        true,
    )
    .await;
    let future = seed_job(
        &pool,
        &format!("{prefix}-future"),
        "queued",
        now + Duration::hours(3),
        None,
        true,
    )
    .await;
    let failed = seed_job(
        &pool,
        &format!("{prefix}-failed"),
        "failed",
        now - Duration::minutes(5),
        None,
        true,
    )
    .await;
    sqlx::query("UPDATE jobs SET failed_at = now(), last_error = 'boom' WHERE id = $1")
        .bind(failed)
        .execute(&pool)
        .await
        .expect("mark job failed");

    let bearer = auth(admin, "admin");

    // queued bucket: only due queued jobs, oldest first, numeric info.
    let resp = server
        .get("/jobs2?bucket=queued")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "queued list; body: {}",
        resp.text()
    );
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("job list is a JSON array");
    let ids: Vec<i64> = rows.iter().filter_map(|j| j["id"].as_i64()).collect();
    let older_pos = ids.iter().position(|id| *id == older).expect("older job");
    let newer_pos = ids.iter().position(|id| *id == newer).expect("newer job");
    assert!(
        older_pos < newer_pos,
        "queued jobs are ordered by run_at ASC: {ids:?}"
    );
    assert!(
        !ids.contains(&future) && !ids.contains(&failed),
        "future/failed jobs must not be in the queued bucket: {ids:?}"
    );
    let item = &rows[older_pos];
    assert!(item.get("bucket").is_none(), "bucket listings omit bucket");
    assert!(
        item["info"].as_i64().is_some_and(|seconds| seconds >= 7100),
        "queued info is seconds waited: {}",
        item["info"]
    );

    // future bucket: info is the scheduled timestamp.
    let resp = server
        .get("/jobs2?bucket=future")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    let item = rows
        .as_array()
        .expect("job list is a JSON array")
        .iter()
        .find(|j| j["id"].as_i64() == Some(future))
        .expect("future job listed");
    assert!(
        item["info"].as_str().is_some_and(|s| s.contains('T')),
        "future info is an ISO timestamp: {}",
        item["info"]
    );

    // failed bucket: failed-only columns are present.
    let resp = server
        .get("/jobs2?bucket=failed")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    let item = rows
        .as_array()
        .expect("job list is a JSON array")
        .iter()
        .find(|j| j["id"].as_i64() == Some(failed))
        .expect("failed job listed");
    assert_eq!(item["last_error"], "boom");
    assert!(item["failed_at"].is_string());
    assert!(item["info"].is_string(), "failed info is a timestamp");

    // No bucket: rows carry an inferred bucket instead of info.
    let resp = server
        .get(&format!("/jobs2?tag={prefix}-queued-newer"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    assert_eq!(rows.as_array().map(Vec::len), Some(1));
    assert_eq!(rows[0]["bucket"], "queued");
    assert_eq!(rows[0]["workflow_state"], Value::Null);
    assert!(rows[0].get("info").is_none());

    // Unknown bucket is rejected (Canvas requires a valid bucket).
    let resp = server
        .get("/jobs2?bucket=completed")
        .add_header(AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 400, "invalid bucket must be a 400");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn lookup_matches_id_and_original_job_id() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let failed = seed_job(
        &pool,
        &format!("{prefix}-lookup"),
        "failed",
        Utc::now(),
        None,
        true,
    )
    .await;
    let original_id = 9_100_000_000_i64;
    sqlx::query(
        "UPDATE jobs SET failed_at = now(), original_job_id = $1, handler = 'handler-yml' WHERE id = $2",
    )
    .bind(original_id)
    .bind(failed)
    .execute(&pool)
    .await
    .expect("mark job failed");

    let bearer = auth(admin, "admin");

    // Lookup by job id: Canvas returns an array with an inferred bucket.
    let resp = server
        .get(&format!("/jobs2/{failed}"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "lookup; body: {}", resp.text());
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("lookup returns a JSON array");
    assert_eq!(rows.len(), 1, "lookup by id returns one row: {rows:?}");
    assert_eq!(rows[0]["id"].as_i64(), Some(failed));
    assert_eq!(rows[0]["bucket"], "failed");
    assert_eq!(rows[0]["original_job_id"].as_i64(), Some(original_id));
    assert_eq!(rows[0]["handler"], "handler-yml");

    // Lookup by original_job_id finds the failed row.
    let resp = server
        .get(&format!("/jobs2/{original_id}"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("lookup returns a JSON array");
    assert!(
        rows.iter()
            .any(|j| j["original_job_id"].as_i64() == Some(original_id)),
        "original_job_id lookup matches the failed row: {rows:?}"
    );

    // Unknown ids are an empty array (Canvas does not raise).
    let resp = server
        .get("/jobs2/999999999")
        .add_header(AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    assert_eq!(rows.as_array().map(Vec::len), Some(0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn requeue_failed_job_creates_queued_copy() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let failed = seed_job(
        &pool,
        &format!("{prefix}-requeue"),
        "failed",
        Utc::now() - Duration::minutes(10),
        None,
        true,
    )
    .await;
    sqlx::query(
        r#"
        UPDATE jobs
        SET failed_at = now(), handler = 'handler-yml', attempts = 3,
            max_attempts = 3, last_error = 'kaboom'
        WHERE id = $1
        "#,
    )
    .bind(failed)
    .execute(&pool)
    .await
    .expect("mark job failed");

    let bearer = auth(admin, "admin");

    let resp = server
        .post(&format!("/jobs2/{failed}/requeue"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "requeue; body: {}", resp.text());
    let job: Value = resp.json();
    let new_id = job["id"].as_i64().expect("new job id");
    assert_ne!(new_id, failed, "requeue creates a new job row");
    assert_eq!(job["bucket"], "queued");
    assert_eq!(job["handler"], "handler-yml");
    assert_eq!(job["attempts"].as_i64(), Some(0));
    assert_eq!(job["max_attempts"].as_i64(), Some(3));

    // The failed row is kept and linked to its replacement.
    let (state, requeued): (String, Option<i64>) =
        sqlx::query_as("SELECT workflow_state, requeued_job_id FROM jobs WHERE id = $1")
            .bind(failed)
            .fetch_one(&pool)
            .await
            .expect("load failed job");
    assert_eq!(state, "failed");
    assert_eq!(requeued, Some(new_id));

    // Only failed jobs can be requeued; missing ids are 404 too.
    let queued = seed_job(
        &pool,
        &format!("{prefix}-not-failed"),
        "queued",
        Utc::now(),
        None,
        true,
    )
    .await;
    let resp = server
        .post(&format!("/jobs2/{queued}/requeue"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "non-failed job cannot be requeued");
    let resp = server
        .post("/jobs2/999999999/requeue")
        .add_header(AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 404, "missing failed job is 404");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn clusters_report_counts_and_jobs2_is_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;

    let cluster_a: i64 = sqlx::query_scalar(
        "INSERT INTO job_clusters (database_server_id, domain) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-jobs1"))
    .bind(format!("{prefix}-a.example.com"))
    .fetch_one(&pool)
    .await
    .expect("seed cluster a");
    let cluster_b: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO job_clusters (database_server_id, domain, block_stranded, jobs_held)
        VALUES ($1, $2, TRUE, TRUE)
        RETURNING id
        "#,
    )
    .bind(format!("{prefix}-jobs2"))
    .bind(format!("{prefix}-b.example.com"))
    .fetch_one(&pool)
    .await
    .expect("seed cluster b");

    let now = Utc::now();
    // Cluster A: one runnable, one future, one running, one stranded.
    seed_job(
        &pool,
        &format!("{prefix}-a-q"),
        "queued",
        now,
        Some(cluster_a),
        true,
    )
    .await;
    seed_job(
        &pool,
        &format!("{prefix}-a-f"),
        "queued",
        now + Duration::hours(2),
        Some(cluster_a),
        true,
    )
    .await;
    seed_job(
        &pool,
        &format!("{prefix}-a-r"),
        "running",
        now,
        Some(cluster_a),
        true,
    )
    .await;
    seed_job(
        &pool,
        &format!("{prefix}-a-blocked"),
        "queued",
        now,
        Some(cluster_a),
        false,
    )
    .await;
    // Cluster B: block_stranded makes its queued work count as blocked.
    seed_job(
        &pool,
        &format!("{prefix}-b-q"),
        "queued",
        now,
        Some(cluster_b),
        true,
    )
    .await;

    let bearer = auth(admin, "admin");

    let resp = server
        .get(&format!("/jobs2/clusters?job_shards={cluster_a}"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "clusters; body: {}", resp.text());
    let clusters: Value = resp.json();
    let clusters = clusters.as_array().expect("clusters is a JSON array");
    assert_eq!(clusters.len(), 1, "job_shards filters the cluster list");
    let cluster = &clusters[0];
    assert_eq!(cluster["id"].as_i64(), Some(cluster_a));
    assert_eq!(
        cluster["database_server_id"],
        format!("{prefix}-jobs1").as_str()
    );
    assert_eq!(
        cluster["domain"],
        format!("{prefix}-a.example.com").as_str()
    );
    assert_eq!(cluster["block_stranded_shard_ids"], serde_json::json!([]));
    assert_eq!(cluster["jobs_held_shard_ids"], serde_json::json!([]));
    assert_eq!(cluster["counts"]["queued"].as_i64(), Some(1));
    assert_eq!(cluster["counts"]["running"].as_i64(), Some(1));
    assert_eq!(cluster["counts"]["future"].as_i64(), Some(1));
    assert_eq!(cluster["counts"]["blocked"].as_i64(), Some(1));

    let resp = server
        .get(&format!(
            "/jobs2/clusters?job_shards={cluster_a},{cluster_b}&per_page=10"
        ))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let clusters: Value = resp.json();
    let clusters = clusters.as_array().expect("clusters is a JSON array");
    assert_eq!(clusters.len(), 2);
    let cluster = clusters
        .iter()
        .find(|c| c["id"].as_i64() == Some(cluster_b))
        .expect("cluster b listed");
    assert_eq!(
        cluster["block_stranded_shard_ids"],
        serde_json::json!([cluster_b])
    );
    assert_eq!(
        cluster["jobs_held_shard_ids"],
        serde_json::json!([cluster_b])
    );
    assert_eq!(cluster["counts"]["queued"].as_i64(), Some(1));
    assert_eq!(cluster["counts"]["blocked"].as_i64(), Some(1));

    // Non-admins are forbidden on every Jobs V2 route; anonymous is 401.
    let teacher_bearer = auth(teacher, "teacher");
    let resp = server
        .get("/jobs2")
        .add_header(AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403);
    let resp = server
        .get(&format!("/jobs2/{cluster_a}"))
        .add_header(AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403);
    let resp = server
        .get("/jobs2/clusters")
        .add_header(AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403);
    let resp = server
        .post("/jobs2/1/requeue")
        .add_header(AUTHORIZATION, teacher_bearer)
        .await;
    assert_eq!(resp.status_code(), 403);

    assert_eq!(server.get("/jobs2").await.status_code(), 401);
    assert_eq!(server.get("/jobs2/clusters").await.status_code(), 401);
    assert_eq!(server.post("/jobs2/1/requeue").await.status_code(), 401);

    cleanup(&pool, &prefix).await;
}
