//! Context scoping for the OutcomeLink routes
//! (`canvas-src/config/routes.rb:2547-2550`, the `outcome_groups_api` scope
//! looped over `%w[global account course]` at `:2536`).
//!
//! Two defects this pins down:
//!
//! 1. Four of these routes were wired to handlers whose `Path<T>` extractor
//!    asked for fewer values than the template has `{param}` segments --
//!    e.g. `Path<i64>` on `/accounts/{accountId}/outcome_groups/{id}/outcomes`.
//!    axum rejects that mismatch before the handler runs, so the route could
//!    never answer; the parity scoreboard counted it as implemented because it
//!    only compares templates.
//! 2. The handlers that survived took the group by id alone, so
//!    `/accounts/2/outcome_groups/1/outcomes` answered for a group owned by
//!    account 1. Canvas scopes the lookup with `context_outcome_groups`
//!    (`outcome_groups_api_controller.rb:770-772`) and 404s the other case.
//!
//! Skipped cleanly when DATABASE_URL is not set.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    format!(
        "outlink-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
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
    let server =
        TestServer::new(star_outcomes::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
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

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
}

async fn seed_course(pool: &PgPool, prefix: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-course"))
    .bind(format!("{prefix}-course"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'teacher')")
        .bind(user_id)
        .bind(course_id)
        .execute(pool)
        .await
        .expect("seed teacher enrollment");
}

/// A group in the given context (`Account` with `context_id`, `Course` with
/// `course_id`, or `Global`).
async fn seed_group(
    pool: &PgPool,
    prefix: &str,
    context: &str,
    context_id: Option<i64>,
    course_id: Option<i64>,
) -> i64 {
    let (sql, bind_account, bind_course) = match context {
        "Global" => (
            "INSERT INTO outcome_groups (context_type, title) VALUES ('Global', $1) RETURNING id",
            None,
            None,
        ),
        "Course" => (
            "INSERT INTO outcome_groups (context_type, course_id, title) VALUES ('Course', $2, $1) RETURNING id",
            None,
            course_id,
        ),
        _ => (
            "INSERT INTO outcome_groups (context_type, context_id, title) VALUES ('Account', $2, $1) RETURNING id",
            context_id,
            None,
        ),
    };
    let mut query = sqlx::query_scalar(sql).bind(format!("{prefix} {context} group"));
    if let Some(account_id) = bind_account {
        query = query.bind(account_id);
    }
    if let Some(cid) = bind_course {
        query = query.bind(cid);
    }
    query.fetch_one(pool).await.expect("seed group")
}

async fn seed_outcome(pool: &PgPool, prefix: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO learning_outcomes (title) VALUES ($1) RETURNING id")
        .bind(format!("{prefix} outcome"))
        .fetch_one(pool)
        .await
        .expect("seed outcome")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM outcome_links WHERE outcome_id IN \
         (SELECT id FROM learning_outcomes WHERE title LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM learning_outcomes WHERE title LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM outcome_groups WHERE title LIKE $1")
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
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE user_id IN \
         (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
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

// ---------------------------------------------------------------------------
// /accounts/{accountId}/outcome_groups/{id}/outcomes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_scoped_link_list_and_create_answer() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Acct").await;
    let other = seed_account(&pool, &prefix, "Other").await;
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    seed_account_admin(&pool, account, admin).await;
    let group = seed_group(&pool, &prefix, "Account", Some(account), None).await;
    let outcome = seed_outcome(&pool, &prefix).await;
    let token = auth(admin, "teacher");

    // GET was a Path-arity rejection before: it must answer, empty at first.
    let resp = server
        .get(&format!(
            "/accounts/{account}/outcome_groups/{group}/outcomes"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rows: Vec<Value> = resp.json();
    assert!(rows.is_empty(), "expected no links, got {rows:?}");

    // POST (outcome named in the body) → 201, Canvas' `action: :link`.
    let resp = server
        .post(&format!(
            "/accounts/{account}/outcome_groups/{group}/outcomes"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .json(&json!({ "outcome_id": outcome }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["context_type"], json!("Account"));
    assert_eq!(body["context_id"], json!(account));

    let resp = server
        .get(&format!(
            "/accounts/{account}/outcome_groups/{group}/outcomes"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let rows: Vec<Value> = resp.json();
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert_eq!(rows[0]["outcome_id"], json!(outcome));

    // The account in the URL must own the group. The caller has rights in
    // *both* accounts, so this reaches Canvas' `context_outcome_groups.find`
    // and 404s there — the account check passed, the context scope did not.
    seed_account_admin(&pool, other, admin).await;
    let resp = server
        .get(&format!(
            "/accounts/{other}/outcome_groups/{group}/outcomes"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Order, copied from Canvas: the account is resolved first
    // (`before_action :get_context`), so an unknown account is 404 even for a
    // user with no rights anywhere.
    let resp = server
        .get("/accounts/99999999/outcome_groups/1/outcomes")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Then the rights check: `can_read_outcomes` runs before the group lookup,
    // so an account the caller does not administer is 403, not 404.
    let outsider = seed_user(&pool, &prefix, "Outsider", "teacher").await;
    let resp = server
        .get(&format!(
            "/accounts/{account}/outcome_groups/{group}/outcomes"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_path_named_link_answers() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Acct").await;
    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    seed_account_admin(&pool, account, admin).await;
    let group = seed_group(&pool, &prefix, "Account", Some(account), None).await;
    let outcome = seed_outcome(&pool, &prefix).await;

    // PUT .../outcomes/{outcomeId} — Canvas' `action: :link` with the outcome
    // in the path; this was a Path-arity rejection before.
    let resp = server
        .put(&format!(
            "/accounts/{account}/outcome_groups/{group}/outcomes/{outcome}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["outcome_id"], json!(outcome));
    assert_eq!(body["context_type"], json!("Account"));
    assert_eq!(body["context_id"], json!(account));

    // A group from a different context cannot be linked through this URL.
    let global_group = seed_group(&pool, &prefix, "Global", None, None).await;
    let resp = server
        .put(&format!(
            "/accounts/{account}/outcome_groups/{global_group}/outcomes/{outcome}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// global + course path-named links
// ---------------------------------------------------------------------------

#[tokio::test]
async fn global_path_named_link_answers_for_a_global_group() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let group = seed_group(&pool, &prefix, "Global", None, None).await;
    let outcome = seed_outcome(&pool, &prefix).await;
    let admin = seed_user(&pool, &prefix, "SuperAdmin", "admin").await;

    let resp = server
        .put(&format!(
            "/global/outcome_groups/{group}/outcomes/{outcome}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["outcome_id"], json!(outcome));
    assert_eq!(body["outcome_group_id"], json!(group));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_path_named_link_scopes_the_group_to_the_course() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Acct").await;
    let course = seed_course(&pool, &prefix, account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher, course).await;
    let group = seed_group(&pool, &prefix, "Course", None, Some(course)).await;
    let outcome = seed_outcome(&pool, &prefix).await;

    // PUT .../outcomes/{outcomeId} used `link_outcome_to_group`, whose Path
    // asked for 2 values on a 3-parameter route.
    let resp = server
        .put(&format!(
            "/courses/{course}/outcome_groups/{group}/outcomes/{outcome}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["outcome_id"], json!(outcome));
    assert_eq!(body["context_type"], json!("Course"));

    // A global group is not in this course's context → 404, as Canvas
    // `context_outcome_groups.find` returns nothing.
    let global_group = seed_group(&pool, &prefix, "Global", None, None).await;
    let resp = server
        .put(&format!(
            "/courses/{course}/outcome_groups/{global_group}/outcomes/{outcome}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // And a non-teacher on the course is 403 before the group lookup, which is
    // Canvas' `can_manage_outcomes` → `context_outcome_groups.find` order.
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let resp = server
        .put(&format!(
            "/courses/{course}/outcome_groups/{group}/outcomes/{outcome}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
