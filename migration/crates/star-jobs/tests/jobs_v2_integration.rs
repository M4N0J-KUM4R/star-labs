//! Integration tests for the Jobs V2 surface (`/api/v1/jobs2`).
//!
//! Same harness conventions as `tests/jobs_integration.rs`:
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Unique per-run prefix + full teardown, so repeat runs against a shared
//!   database are safe.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
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

/// Insert one job with strand / singleton attributes.
#[allow(clippy::too_many_arguments)]
async fn seed_strand_job(
    pool: &PgPool,
    tag: &str,
    strand: Option<&str>,
    singleton: Option<&str>,
    run_at: DateTime<Utc>,
    next_in_strand: bool,
    max_concurrent: i32,
    locked_by: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO jobs (
            tag, workflow_state, progress, run_at, next_in_strand, max_concurrent,
            strand, singleton, locked_by
        )
        VALUES ($1, 'queued', 0, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
    )
    .bind(tag)
    .bind(run_at)
    .bind(next_in_strand)
    .bind(max_concurrent)
    .bind(strand)
    .bind(singleton)
    .bind(locked_by)
    .fetch_one(pool)
    .await
    .expect("seed strand job")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM jobs WHERE tag LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    // The unstucker job row carries a fixed tag, so it is removed by tag and
    // by the test user that requested it.
    let _ = sqlx::query(
        "DELETE FROM jobs WHERE tag = $1 \
         AND user_id IN (SELECT id FROM users WHERE email LIKE $2)",
    )
    .bind("JobsV2Controller::run_unstucker!")
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM progress WHERE tag = $1 \
         AND user_id IN (SELECT id FROM users WHERE email LIKE $2)",
    )
    .bind("JobsV2Controller::run_unstucker!")
    .bind(format!("{prefix}%@test.local"))
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
        .get("/jobs2/queued")
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
        .get("/jobs2/future")
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
        .get("/jobs2/failed")
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

    // Canvas' bucket listing takes the tag filter and omits `bucket`.
    let resp = server
        .get(&format!("/jobs2/queued?tag={prefix}-queued-newer"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rows: Value = resp.json();
    assert_eq!(rows.as_array().map(Vec::len), Some(1));
    assert_eq!(rows[0]["workflow_state"], Value::Null);
    assert!(
        rows[0].get("bucket").is_none(),
        "bucket listings omit bucket"
    );
    assert!(rows[0].get("info").is_some(), "bucket listings carry info");

    // Canvas routes only the four bucket names, so any other segment is a 404.
    let resp = server
        .get("/jobs2/completed")
        .add_header(AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 404, "unknown bucket must be a 404");

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
        .get("/jobs2/queued")
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

    assert_eq!(server.get("/jobs2/queued").await.status_code(), 401);
    assert_eq!(server.get("/jobs2/clusters").await.status_code(), 401);
    assert_eq!(server.post("/jobs2/1/requeue").await.status_code(), 401);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Jobs V2: grouped info, search, manage, unstuck, throttle, stuck
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grouped_info_groups_queued_jobs_by_tag() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    // The jobs live in their own time window (100 days ago) so the listing
    // is not disturbed by jobs other tests seed in the same database.
    let base = now - Duration::days(100);

    // Two jobs share a tag, one job has its own tag. Canvas groups by the RAW
    // tag column (`JobsV2Controller#grouped_info` → `.group(@group)`), so the
    // two pp jobs must carry the same tag to land in one group.
    seed_strand_job(
        &pool,
        &format!("{prefix}-pp"),
        None,
        None,
        base,
        true,
        1,
        None,
    )
    .await;
    seed_strand_job(
        &pool,
        &format!("{prefix}-pp"),
        None,
        None,
        base + Duration::hours(1),
        true,
        1,
        None,
    )
    .await;
    seed_strand_job(
        &pool,
        &format!("{prefix}-p"),
        None,
        None,
        base + Duration::hours(2),
        true,
        1,
        None,
    )
    .await;
    // A future job is not queued.
    seed_strand_job(
        &pool,
        &format!("{prefix}-future"),
        None,
        None,
        now + Duration::hours(2),
        true,
        1,
        None,
    )
    .await;

    let window = format!(
        "start_date={}&end_date={}",
        (now - Duration::days(101)).to_rfc3339_opts(SecondsFormat::Millis, true),
        (now - Duration::days(99)).to_rfc3339_opts(SecondsFormat::Millis, true)
    );
    let bearer = auth(admin, "admin");

    let resp = server
        .get(&format!("/jobs2/queued/by_tag?{window}"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "grouped info; body: {}",
        resp.text()
    );
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("grouped info is a JSON array");
    assert_eq!(rows.len(), 2, "two tag groups: {rows:?}");

    // Default order is the bucket's info column (oldest run_at first).
    assert_eq!(rows[0]["tag"], format!("{prefix}-pp"));
    assert_eq!(rows[0]["count"], 2);
    assert!(
        rows[0]["info"]
            .as_i64()
            .is_some_and(|seconds| seconds >= 8_000_000),
        "queued info is the seconds waited: {}",
        rows[0]["info"]
    );
    assert_eq!(rows[1]["tag"], format!("{prefix}-p"));
    assert_eq!(rows[1]["count"], 1);
    assert!(
        rows[0].get("orphaned").is_none(),
        "tag groups carry no orphaned flag"
    );

    // order=count sorts by the group size (descending).
    let resp = server
        .get(&format!("/jobs2/queued/by_tag?order=count&{window}"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("grouped info is a JSON array");
    assert_eq!(rows[0]["count"], 2);
    assert_eq!(rows[0]["tag"], format!("{prefix}-pp"));

    // Canvas paginates with per_page / page.
    let resp = server
        .get(&format!("/jobs2/queued/by_tag?per_page=1&{window}"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("grouped info is a JSON array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["tag"], format!("{prefix}-pp"));

    let resp = server
        .get(&format!("/jobs2/queued/by_tag?per_page=1&page=2&{window}"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("grouped info is a JSON array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["tag"], format!("{prefix}-p"));

    // An unknown bucket is rejected.
    let resp = server
        .get("/jobs2/bogus/by_tag")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "invalid bucket must be a 400");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grouped_info_flags_orphaned_strands_and_singletons() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    let stuck_strand = format!("{prefix}-bar");
    let healthy_strand = format!("{prefix}-foo");
    let stuck_singleton = format!("{prefix}-misery");

    // A strand whose jobs are all waiting behind each other.
    seed_strand_job(
        &pool,
        &format!("{prefix}-bar-1"),
        Some(&stuck_strand),
        None,
        now - Duration::hours(2),
        false,
        2,
        None,
    )
    .await;
    seed_strand_job(
        &pool,
        &format!("{prefix}-bar-2"),
        Some(&stuck_strand),
        None,
        now - Duration::hours(1),
        false,
        2,
        None,
    )
    .await;
    // A strand that can progress.
    seed_strand_job(
        &pool,
        &format!("{prefix}-foo"),
        Some(&healthy_strand),
        None,
        now - Duration::hours(1),
        true,
        1,
        None,
    )
    .await;
    // A singleton that cannot progress.
    seed_strand_job(
        &pool,
        &format!("{prefix}-misery"),
        None,
        Some(&stuck_singleton),
        now - Duration::hours(1),
        false,
        1,
        None,
    )
    .await;

    let bearer = auth(admin, "admin");

    let resp = server
        .get("/jobs2/queued/by_strand")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "grouped info; body: {}",
        resp.text()
    );
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("grouped info is a JSON array");

    let bar = rows
        .iter()
        .find(|row| row["strand"] == stuck_strand.as_str())
        .expect("stuck strand group");
    assert_eq!(bar["count"], 2);
    assert_eq!(bar["orphaned"], true);

    let foo = rows
        .iter()
        .find(|row| row["strand"] == healthy_strand.as_str())
        .expect("healthy strand group");
    assert_eq!(foo["count"], 1);
    assert_eq!(foo["orphaned"], false);

    // Singletons are grouped and flagged the same way.
    let resp = server
        .get("/jobs2/queued/by_singleton")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("grouped info is a JSON array");
    let misery = rows
        .iter()
        .find(|row| row["singleton"] == stuck_singleton.as_str())
        .expect("singleton group");
    assert_eq!(misery["count"], 1);
    assert_eq!(misery["orphaned"], true);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn search_counts_matching_groups() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    for _ in 0..2 {
        seed_strand_job(
            &pool,
            &format!("{prefix}-Kernel.pp"),
            None,
            None,
            now - Duration::hours(2),
            true,
            1,
            None,
        )
        .await;
    }
    seed_strand_job(
        &pool,
        &format!("{prefix}-Kernel.p"),
        None,
        None,
        now - Duration::hours(1),
        true,
        1,
        None,
    )
    .await;

    let bearer = auth(admin, "admin");

    let resp = server
        .get(&format!(
            "/jobs2/queued/by_tag/search?term={prefix}-Kernel.p"
        ))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "search; body: {}", resp.text());
    let counts: Value = resp.json();
    assert_eq!(
        counts.get(format!("{prefix}-Kernel.pp")),
        Some(&Value::from(2)),
        "search counts the matching tags: {counts}"
    );
    assert_eq!(
        counts.get(format!("{prefix}-Kernel.p")),
        Some(&Value::from(1)),
        "search counts the matching tags: {counts}"
    );

    // The term is required.
    let resp = server
        .get("/jobs2/queued/by_tag/search")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "search without a term is a 400");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn manage_updates_strand_settings() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();
    let strand = format!("{prefix}-manage");

    seed_strand_job(
        &pool,
        &format!("{prefix}-manage-1"),
        Some(&strand),
        None,
        now,
        true,
        1,
        None,
    )
    .await;
    seed_strand_job(
        &pool,
        &format!("{prefix}-manage-2"),
        Some(&strand),
        None,
        now,
        false,
        1,
        None,
    )
    .await;

    let bearer = auth(admin, "admin");

    let resp = server
        .put("/jobs2/manage")
        .json(&serde_json::json!({
            "strand": strand,
            "max_concurrent": 11,
            "priority": 23,
        }))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "manage; body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "OK");
    assert_eq!(body["count"], 2);

    let rows: Vec<(i32, i32, bool)> = sqlx::query_as(
        "SELECT max_concurrent, priority, next_in_strand FROM jobs WHERE strand = $1 ORDER BY id",
    )
    .bind(&strand)
    .fetch_all(&pool)
    .await
    .expect("load managed strand jobs");
    assert_eq!(rows.len(), 2);
    for (max_concurrent, priority, next_in_strand) in rows {
        assert_eq!(max_concurrent, 11);
        assert_eq!(priority, 23);
        // Raising max_concurrent unblocks the strand.
        assert!(next_in_strand);
    }

    // The strand is required.
    let resp = server
        .put("/jobs2/manage")
        .json(&serde_json::json!({ "max_concurrent": 3 }))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "manage without a strand is a 400");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn unstuck_unblocks_strands_and_singletons() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();
    let strand = format!("{prefix}-stuck");
    let singleton = format!("{prefix}-singleton");

    seed_strand_job(
        &pool,
        &format!("{prefix}-stuck-1"),
        Some(&strand),
        None,
        now,
        false,
        2,
        None,
    )
    .await;
    seed_strand_job(
        &pool,
        &format!("{prefix}-stuck-2"),
        Some(&strand),
        None,
        now,
        false,
        2,
        None,
    )
    .await;
    seed_strand_job(
        &pool,
        &format!("{prefix}-singleton"),
        None,
        Some(&singleton),
        now,
        false,
        1,
        None,
    )
    .await;

    let bearer = auth(admin, "admin");

    // A stuck strand is unblocked synchronously.
    let resp = server
        .put("/jobs2/unstuck")
        .json(&serde_json::json!({ "strand": strand }))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "unstuck; body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "OK");
    assert_eq!(body["count"], 2);

    let next_in_strand: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE strand = $1 AND next_in_strand = TRUE")
            .bind(&strand)
            .fetch_one(&pool)
            .await
            .expect("count unblocked strand jobs");
    assert_eq!(next_in_strand, 2);

    // A strand that is not stuck reports a count of zero.
    let resp = server
        .put("/jobs2/unstuck")
        .json(&serde_json::json!({ "strand": strand }))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["status"], "OK");
    assert_eq!(body["count"], 0);

    // A stuck singleton is unblocked as well.
    let resp = server
        .put("/jobs2/unstuck")
        .json(&serde_json::json!({ "singleton": singleton }))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["status"], "OK");
    assert_eq!(body["count"], 1);

    // An unknown strand is a 404.
    let resp = server
        .put("/jobs2/unstuck")
        .json(&serde_json::json!({ "strand": format!("{prefix}-missing") }))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "unknown strand is a 404");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn unstuck_without_target_queues_the_unstucker() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let bearer = auth(admin, "admin");

    let resp = server
        .put("/jobs2/unstuck")
        .json(&serde_json::json!({}))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 202, "unstuck; body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "pending");

    let progress = &body["progress"];
    assert!(progress["id"].as_i64().is_some(), "progress id: {progress}");
    assert_eq!(progress["tag"], "JobsV2Controller::run_unstucker!");
    assert_eq!(progress["workflow_state"], "queued");
    assert_eq!(progress["completion"], 0);
    assert_eq!(progress["context_type"], "User");
    assert_eq!(progress["context_id"], admin);

    let progress_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM progress WHERE tag = $1")
        .bind("JobsV2Controller::run_unstucker!")
        .fetch_one(&pool)
        .await
        .expect("count progress rows");
    assert!(progress_rows >= 1, "a progress row was created");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn throttle_check_and_throttle_strand_matched_jobs() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    for _ in 0..3 {
        seed_strand_job(
            &pool,
            &format!("{prefix}-print"),
            None,
            None,
            now,
            true,
            1,
            None,
        )
        .await;
    }
    for _ in 0..2 {
        seed_strand_job(
            &pool,
            &format!("{prefix}-printf"),
            None,
            None,
            now,
            true,
            1,
            None,
        )
        .await;
    }

    let bearer = auth(admin, "admin");

    let resp = server
        .get(&format!("/jobs2/throttle/check?term={prefix}-print"))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "throttle check; body: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["matched_jobs"], 5);
    assert_eq!(body["matched_tags"], 2);

    // The term is required.
    let resp = server
        .get("/jobs2/throttle/check")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "throttle check without a term is a 400"
    );

    // Throttling strands the matched jobs under a temporary strand.
    let resp = server
        .put("/jobs2/throttle")
        .json(&serde_json::json!({
            "term": format!("{prefix}-print"),
            "max_concurrent": 2,
        }))
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "throttle; body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["job_count"], 5);
    let new_strand = body["new_strand"].as_str().expect("new strand").to_string();
    assert!(new_strand.starts_with("tmp_strand_"), "{new_strand}");

    let stranded: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE strand = $1")
        .bind(&new_strand)
        .fetch_one(&pool)
        .await
        .expect("count stranded jobs");
    assert_eq!(stranded, 5);

    let next_in_strand: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE strand = $1 AND next_in_strand = TRUE")
            .bind(&new_strand)
            .fetch_one(&pool)
            .await
            .expect("count runnable stranded jobs");
    assert_eq!(next_in_strand, 2, "max_concurrent jobs may run");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn stuck_strands_and_singletons_are_listed() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let now = Utc::now();

    let stuck_strand = format!("{prefix}-sadness");
    let healthy_strand = format!("{prefix}-happiness");
    let stuck_singleton = format!("{prefix}-misery");

    seed_strand_job(
        &pool,
        &format!("{prefix}-sadness-1"),
        Some(&stuck_strand),
        None,
        now,
        false,
        1,
        None,
    )
    .await;
    seed_strand_job(
        &pool,
        &format!("{prefix}-sadness-2"),
        Some(&stuck_strand),
        None,
        now,
        false,
        1,
        None,
    )
    .await;
    seed_strand_job(
        &pool,
        &format!("{prefix}-happiness"),
        Some(&healthy_strand),
        None,
        now,
        true,
        1,
        None,
    )
    .await;
    seed_strand_job(
        &pool,
        &format!("{prefix}-misery"),
        None,
        Some(&stuck_singleton),
        now,
        false,
        1,
        None,
    )
    .await;

    let bearer = auth(admin, "admin");

    let resp = server
        .get("/jobs2/stuck/strands")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "stuck strands; body: {}",
        resp.text()
    );
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("stuck strands is a JSON array");

    let sadness = rows
        .iter()
        .find(|row| row["name"] == stuck_strand.as_str())
        .expect("stuck strand listed");
    assert_eq!(sadness["count"], 2);
    assert!(
        !rows
            .iter()
            .any(|row| row["name"] == healthy_strand.as_str()),
        "a strand that can progress is not stuck"
    );

    let resp = server
        .get("/jobs2/stuck/singletons")
        .add_header(AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let rows: Value = resp.json();
    let rows = rows.as_array().expect("stuck singletons is a JSON array");
    let misery = rows
        .iter()
        .find(|row| row["name"] == stuck_singleton.as_str())
        .expect("stuck singleton listed");
    assert_eq!(misery["count"], 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn jobs_v2_grouped_and_write_routes_require_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let teacher_bearer = auth(teacher, "teacher");

    for path in [
        "/jobs2/queued/by_tag",
        "/jobs2/queued/by_strand",
        "/jobs2/queued/by_singleton",
        "/jobs2/queued/by_tag/search?term=x",
        "/jobs2/throttle/check?term=x",
        "/jobs2/stuck/strands",
        "/jobs2/stuck/singletons",
    ] {
        let resp = server
            .get(path)
            .add_header(AUTHORIZATION, teacher_bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 403, "{path} must be admin only");
        let resp = server.get(path).await;
        assert_eq!(resp.status_code(), 401, "{path} must require a token");
    }

    for path in ["/jobs2/manage", "/jobs2/unstuck", "/jobs2/throttle"] {
        let resp = server
            .put(path)
            .add_header(AUTHORIZATION, teacher_bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 403, "{path} must be admin only");
        let resp = server.put(path).await;
        assert_eq!(resp.status_code(), 401, "{path} must require a token");
    }

    cleanup(&pool, &prefix).await;
}
