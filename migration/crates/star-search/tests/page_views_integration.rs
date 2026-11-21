//! Integration tests for the Canvas page views routes.
//!
//! Pinned behavior:
//! - `GET /users/{user_id}/page_views` returns a bare JSON array of page view
//!   hashes, newest first, with the Canvas field names (`id` is the
//!   `request_id`, the id columns live in `links`, `contributed` is `false`);
//! - the user themself, a site admin and an admin of an account the user
//!   belongs to may read the page views, everybody else gets a 403;
//! - `start_time` in the future and `end_time` before `start_time` are 400s;
//! - `POST /users/{user_id}/page_views/query` stores the query and answers
//!   `201` with a `poll_url`; the poll route reports `finished` and the
//!   results route returns the stored CSV;
//! - the batch routes store a query for several users and refuse too many
//!   (`11`) and duplicate user ids.
//!
//! Fleet rule: dedicated users, accounts and courses per test, every seeded
//! row is cleaned up. Skipped cleanly when DATABASE_URL is unset.
//!
//! The `page_views` and `page_view_queries` tables are created by this test
//! (the same DDL the migration needs) so the suite runs before the migration
//! lands; `CREATE TABLE IF NOT EXISTS` keeps it a no-op afterwards.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// The DDL of the two tables the page views routes need.
const PAGE_VIEWS_DDL: [&str; 7] = [
    r#"
    CREATE TABLE IF NOT EXISTS page_views (
        request_id           TEXT PRIMARY KEY,
        session_id           TEXT,
        user_id              BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        url                  TEXT,
        context_id           BIGINT,
        context_type         TEXT,
        asset_id             BIGINT,
        asset_type           TEXT,
        controller           TEXT,
        action               TEXT,
        interaction_seconds  DOUBLE PRECISION,
        created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
        updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
        developer_key_id     BIGINT REFERENCES developer_keys(id) ON DELETE SET NULL,
        user_request         BOOLEAN,
        render_time          DOUBLE PRECISION,
        user_agent           TEXT,
        asset_user_access_id BIGINT,
        participated         BOOLEAN,
        summarized           BOOLEAN,
        account_id           BIGINT REFERENCES accounts(id) ON DELETE SET NULL,
        real_user_id         BIGINT REFERENCES users(id) ON DELETE SET NULL,
        http_method          TEXT,
        remote_ip            TEXT
    )
    "#,
    "CREATE INDEX IF NOT EXISTS index_page_views_on_user_id_and_created_at ON page_views (user_id, created_at)",
    "CREATE INDEX IF NOT EXISTS index_page_views_on_account_id_and_created_at ON page_views (account_id, created_at)",
    "CREATE INDEX IF NOT EXISTS index_page_views_on_context_type_and_context_id ON page_views (context_type, context_id)",
    r#"
    CREATE TABLE IF NOT EXISTS page_view_queries (
        id             UUID PRIMARY KEY,
        requestor_id   BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
        user_id        BIGINT REFERENCES users(id) ON DELETE CASCADE,
        user_ids       BIGINT[] NOT NULL DEFAULT '{}',
        start_date     DATE NOT NULL,
        end_date       DATE NOT NULL,
        results_format TEXT NOT NULL,
        status         TEXT NOT NULL DEFAULT 'finished',
        error_code     TEXT,
        warnings       JSONB,
        content        BYTEA,
        filename       TEXT,
        content_type   TEXT,
        created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
        completed_at   TIMESTAMPTZ
    )
    "#,
    "CREATE INDEX IF NOT EXISTS index_page_view_queries_on_user_id ON page_view_queries (user_id)",
    "CREATE INDEX IF NOT EXISTS index_page_view_queries_on_requestor_id ON page_view_queries (requestor_id)",
];

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "pv-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
        n
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

    for statement in PAGE_VIEWS_DDL {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("create the page view tables");
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server =
        TestServer::new(star_search::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

async fn seed_account_user(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role, workflow_state) \
         VALUES ($1, $2, 'admin', 'active')",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account user");
}

/// One page view of a user, created at `created_at`.
async fn seed_page_view(
    pool: &PgPool,
    prefix: &str,
    index: &str,
    user_id: i64,
    account_id: i64,
    course_id: i64,
    created_at: &str,
) -> String {
    let request_id = format!("{prefix}-pv-{index}");
    sqlx::query(
        r#"
        INSERT INTO page_views (
            request_id, session_id, user_id, url, context_type, context_id,
            asset_type, asset_id, controller, action, interaction_seconds,
            created_at, updated_at, user_request, render_time, user_agent,
            participated, account_id, http_method, remote_ip
        )
        VALUES (
            $1, 'session-1', $2, '/somewhere/in/app', 'Course', $3,
            'Assignment', 42, 'assignments', 'show', 7.21,
            $4::timestamptz, $4::timestamptz,
            true, 0.369, 'Firefox/12.0', false, $5, 'get', '10.10.10.10'
        )
        "#,
    )
    .bind(&request_id)
    .bind(user_id)
    .bind(course_id)
    .bind(created_at)
    .bind(account_id)
    .execute(pool)
    .await
    .expect("seed page view");
    request_id
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM page_views WHERE request_id LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM page_view_queries WHERE requestor_id IN \
           (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn a_user_reads_their_own_page_views() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, user, course, "student").await;
    let older = seed_page_view(
        &pool,
        &prefix,
        "old",
        user,
        account,
        course,
        "2020-01-13 10:00:00+00",
    )
    .await;
    let newer = seed_page_view(
        &pool,
        &prefix,
        "new",
        user,
        account,
        course,
        "2020-01-15 10:00:00+00",
    )
    .await;

    // The index answers with a bare array, newest first.
    let resp = server
        .get(&format!("/users/{user}/page_views"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 2, "body: {list}");
    assert_eq!(items[0]["id"], newer);
    assert_eq!(items[1]["id"], older);

    // The Canvas field names: the id columns live in `links`.
    assert_eq!(items[0]["links"]["user"], user);
    assert_eq!(items[0]["links"]["context"], course);
    assert_eq!(items[0]["links"]["account"], account);
    assert_eq!(items[0]["links"]["asset"], 42);
    assert_eq!(items[0]["links"]["real_user"], Value::Null);
    assert_eq!(items[0]["contributed"], false);
    assert_eq!(items[0]["app_name"], Value::Null);
    assert_eq!(items[0]["controller"], "assignments");
    assert_eq!(items[0]["action"], "show");
    assert_eq!(items[0]["http_method"], "get");
    assert_eq!(items[0]["remote_ip"], "10.10.10.10");
    assert_eq!(items[0]["interaction_seconds"], 7.21);
    assert_eq!(items[0]["participated"], false);
    assert_eq!(items[0]["user_request"], true);
    assert!(items[0].get("user_id").is_none(), "body: {list}");
    assert!(items[0].get("context_id").is_none(), "body: {list}");
    assert!(items[0].get("account_id").is_none(), "body: {list}");

    // Another user may not read them.
    let resp = server
        .get(&format!("/users/{user}/page_views"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An unknown user is a 404.
    let resp = server
        .get("/users/999999999/page_views")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_account_admin_reads_the_page_views_of_the_users_of_the_account() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, user, course, "student").await;
    seed_account_user(&pool, account, admin).await;
    seed_page_view(
        &pool,
        &prefix,
        "pv",
        user,
        account,
        course,
        "2020-01-15 10:00:00+00",
    )
    .await;

    // The account admin of the account of the user reads the page views.
    let resp = server
        .get(&format!("/users/{user}/page_views"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().expect("array").len(), 1);

    // A site admin reads them as well.
    let site_admin = seed_user(&pool, &prefix, "SiteAdmin", "admin").await;
    let resp = server
        .get(&format!("/users/{user}/page_views"))
        .add_header(axum::http::header::AUTHORIZATION, auth(site_admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_index_validates_the_time_range_and_paginates() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, user, course, "student").await;
    seed_page_view(
        &pool,
        &prefix,
        "a",
        user,
        account,
        course,
        "2020-01-13 10:00:00+00",
    )
    .await;
    seed_page_view(
        &pool,
        &prefix,
        "b",
        user,
        account,
        course,
        "2020-01-14 10:00:00+00",
    )
    .await;
    seed_page_view(
        &pool,
        &prefix,
        "c",
        user,
        account,
        course,
        "2020-01-15 10:00:00+00",
    )
    .await;

    // A start time in the future is a 400.
    let resp = server
        .get(&format!(
            "/users/{user}/page_views?start_time=2999-01-01T00:00:00Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["error"],
        "start_time cannot be in the future"
    );

    // An end time before the start time is a 400.
    let resp = server
        .get(&format!(
            "/users/{user}/page_views?start_time=2021-07-04&end_time=2021-07-03"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["error"],
        "end_time must be after start_time"
    );

    // The page views are paginated, newest first.
    let resp = server
        .get(&format!("/users/{user}/page_views?per_page=2"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 2);
    assert!(items[0]["id"].as_str().expect("id").ends_with("-c"));

    let resp = server
        .get(&format!("/users/{user}/page_views?per_page=2&page=2"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert!(items[0]["id"].as_str().expect("id").ends_with("-a"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_query_is_stored_and_its_results_are_read_back() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, user, course, "student").await;
    let request_id = seed_page_view(
        &pool,
        &prefix,
        "pv",
        user,
        account,
        course,
        "2020-01-15 10:00:00+00",
    )
    .await;

    // The query is stored and answered with a poll url.
    let resp = server
        .post(&format!("/users/{user}/page_views/query"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "start_date": "2020-01-01",
            "end_date": "2020-02-01",
            "results_format": "csv",
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let poll_url = resp.json::<Value>()["poll_url"]
        .as_str()
        .expect("poll url")
        .to_string();
    assert!(
        poll_url.starts_with(&format!("/api/v1/users/{user}/page_views/query/")),
        "poll url: {poll_url}"
    );
    let query_id = poll_url.rsplit('/').next().expect("query id").to_string();

    // The poll reports the finished query and its results url.
    let resp = server
        .get(&format!("/users/{user}/page_views/query/{query_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let poll: Value = resp.json();
    assert_eq!(poll["query_id"], query_id);
    assert_eq!(poll["status"], "finished");
    assert_eq!(poll["format"], "csv");
    assert_eq!(poll["error_code"], Value::Null);
    assert_eq!(
        poll["results_url"],
        format!("/api/v1/users/{user}/page_views/query/{query_id}/results")
    );

    // The results are the stored CSV.
    let resp = server
        .get(&format!(
            "/users/{user}/page_views/query/{query_id}/results"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.headers()["content-type"], "text/csv");
    let disposition = resp.headers()["content-disposition"]
        .to_str()
        .expect("disposition");
    assert!(
        disposition.contains(&format!("filename=\"{query_id}.csv\"")),
        "disposition: {disposition}"
    );
    let body = resp.text();
    assert!(body.starts_with("request_id,user_id,url"), "body: {body}");
    assert!(body.contains(&request_id), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_query_routes_validate_their_parameters() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, user, course, "student").await;

    // A missing parameter is a 400.
    let resp = server
        .post(&format!("/users/{user}/page_views/query"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "start_time": "2024-12-01",
            "end_time": "2025-01-10",
            "results_format": "csv",
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["error"],
        "Parameter start_date is missing."
    );

    // An unknown format is a 400 (the months are valid here).
    let resp = server
        .post(&format!("/users/{user}/page_views/query"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "start_date": "2024-12-01",
            "end_date": "2025-01-01",
            "results_format": "xml",
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // A month interval that is not a full month is a 400 as well.
    let resp = server
        .post(&format!("/users/{user}/page_views/query"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "start_date": "2024-12-01",
            "end_date": "2025-01-10",
            "results_format": "csv",
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // An invalid query id is a 400, an unknown query id a 404.
    let resp = server
        .get(&format!("/users/{user}/page_views/query/not-a-uuid"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["error"], "Invalid query ID");

    let resp = server
        .get(&format!(
            "/users/{user}/page_views/query/550e8400-e29b-41d4-a716-446655440000"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["error"], "The query was not found.");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_batch_query_stores_a_query_for_several_users() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let first = seed_user(&pool, &prefix, "First", "student").await;
    let second = seed_user(&pool, &prefix, "Second", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, first, course, "student").await;
    seed_enrollment(&pool, second, course, "student").await;
    let first_view = seed_page_view(
        &pool,
        &prefix,
        "first",
        first,
        account,
        course,
        "2020-01-15 10:00:00+00",
    )
    .await;
    let second_view = seed_page_view(
        &pool,
        &prefix,
        "second",
        second,
        account,
        course,
        "2020-01-15 10:00:00+00",
    )
    .await;

    // The batch query is stored and answered with a poll url.
    let resp = server
        .post("/users/page_views/query")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": [first, second],
            "start_date": "2020-01-01",
            "end_date": "2020-02-01",
            "results_format": "csv",
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let poll_url = resp.json::<Value>()["poll_url"]
        .as_str()
        .expect("poll url")
        .to_string();
    assert!(
        poll_url.starts_with("/api/v1/users/page_views/query/"),
        "poll url: {poll_url}"
    );
    let query_id = poll_url.rsplit('/').next().expect("query id").to_string();

    // The poll reports the finished query, without a warnings key.
    let resp = server
        .get(&format!("/users/page_views/query/{query_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let poll: Value = resp.json();
    assert_eq!(poll["query_id"], query_id);
    assert_eq!(poll["status"], "finished");
    assert_eq!(poll["format"], "csv");
    assert_eq!(
        poll["results_url"],
        format!("/api/v1/users/page_views/query/{query_id}/results")
    );
    assert!(poll.get("warnings").is_none(), "body: {poll}");

    // The results hold the page views of both users.
    let resp = server
        .get(&format!("/users/page_views/query/{query_id}/results"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.headers()["content-type"], "text/csv");
    let body = resp.text();
    assert!(body.contains(&first_view), "body: {body}");
    assert!(body.contains(&second_view), "body: {body}");

    // Too many user ids are a 400.
    let resp = server
        .post("/users/page_views/query")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": (1..=11).collect::<Vec<i64>>(),
            "start_date": "2020-01-01",
            "end_date": "2020-02-01",
            "results_format": "csv",
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["error"],
        "Too many user IDs. Maximum: 10, provided: 11"
    );

    // A duplicate user id is a 400.
    let resp = server
        .post("/users/page_views/query")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user_ids": [first, second, first],
            "start_date": "2020-01-01",
            "end_date": "2020-02-01",
            "results_format": "csv",
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["error"],
        format!("Duplicate user ID found: {first}")
    );

    cleanup(&pool, &prefix).await;
}
