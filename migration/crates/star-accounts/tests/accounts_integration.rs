//! Integration tests for star-accounts.
//!
//! Exercises the real accounts router (GET /accounts, GET /accounts/{id},
//! POST /accounts/{id}/sub_accounts, GET /accounts/{id}/courses,
//! GET /accounts/{id}/users, POST /accounts/{id}/users/{userId}) through
//! axum-test against a real Postgres.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-accounts
//! - Uses a unique account-name / course-code / email prefix per run + full
//!   teardown so it is safe to run repeatedly against a shared/dev database.
//!
//! Characterization notes — behavior these tests pin (bugs are noted, not
//! fixed; see the report accompanying this change):
//! - Permission model: every route requires the global `admin` role except
//!   GET /accounts/{id}, which also accepts account admins via the parent
//!   chain (upward-only walk, capped at MAX_ACCOUNT_TREE_DEPTH = 10 hops).
//!   Permission is checked before existence, so a non-admin gets 403 (not
//!   404) even for a missing account.
//! - KNOWN BUG (star-accounts/src/lib.rs:161, 318, 437, 493, 504): every
//!   "does this row exist" probe is `SELECT 1 ...` decoded into `(i64,)`,
//!   but Postgres types the literal `1` as INT4, so sqlx fails to decode it
//!   whenever a row matches. Result: sub-account creation, account-users
//!   listing, and admin grants return 500 "database error" for any account
//!   that actually exists, and the account-admin read path returns 500
//!   instead of 200. Tests below assert the actual 500 behavior and document
//!   what the handler intended; they must be updated when the bug is fixed.
//! - GET /accounts/{id}/courses only returns courses whose account_id
//!   matches directly; courses in sub-accounts are NOT included (the tree
//!   traversal in the handler is dead code) and a missing account yields
//!   200 + empty page instead of 404. Both deviate from Canvas.
//! - GET /accounts/{id}/users reads account_users JOIN users only — course
//!   enrollments are never consulted.
//! - No handler filters on accounts.workflow_state: 'deleted' accounts are
//!   still listed and still readable.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB never
/// collide on the users.email UNIQUE constraint or leak rows past cleanup.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "acc-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the accounts router
/// exactly the way star-http assembles it. Returns None when DATABASE_URL is
/// unset so callers can skip cleanly.
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
        jwt_secret: "test-jwt-secret".to_string(),
    };

    let server = TestServer::new(star_accounts_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same merge pattern as star-http::build_api_router, restricted to the
/// accounts router.
fn star_accounts_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_accounts::router())
        .with_state(state)
}

/// Create a user directly in the DB (bypassing the register endpoint) with a
/// deterministic password hash and the given role. Returns user id.
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
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Create an account directly in the DB. Returns account id.
async fn seed_account(pool: &PgPool, name: &str, parent: Option<i64>, workflow_state: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO accounts (name, parent_account_id, workflow_state)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(parent)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

/// Create a course directly in the DB, tied to the given account. The name is
/// the plain course name; the unique prefix is carried by course_code.
/// Returns course id.
async fn seed_course(
    pool: &PgPool,
    prefix: &str,
    name: &str,
    account_id: i64,
    description: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code, description, account_id)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .bind(description)
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

/// Insert an account_users row directly, with an explicit created_at so an
/// ORDER BY au.created_at ASC would be deterministic.
async fn seed_account_user(
    pool: &PgPool,
    account_id: i64,
    user_id: i64,
    created_at: chrono::DateTime<chrono::Utc>,
) {
    sqlx::query(
        r#"
        INSERT INTO account_users (account_id, user_id, role, created_at, updated_at)
        VALUES ($1, $2, 'admin', $3, $3)
        "#,
    )
    .bind(account_id)
    .bind(user_id)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("seed account_user");
}

/// Give a user an active student enrollment in a course (used to show the
/// account-users listing reads account_users, never enrollments).
async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, 'student', 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// Mint a real bearer token the same way star-auth does at login time.
/// Note: the AuthUser extractor ignores the token's role claim and reads the
/// user's current role from the DB, so the seeded DB role is what counts.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run. Courses go first because
/// courses.account_id has no ON DELETE action, so an account cannot be
/// removed while a course still references it; account_users rows cascade
/// from both users and accounts.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

/// Page through GET /accounts collecting every row, so assertions work on a
/// shared DB no matter how many foreign accounts exist. Ordering is id ASC
/// and ids are monotonic (bigserial), so pagination never skips our rows.
async fn list_all_accounts(server: &TestServer, token: &str) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut page = 1;
    loop {
        let resp = server
            .get(&format!("/accounts?page={page}&per_page=100"))
            .add_header(axum::http::header::AUTHORIZATION, token)
            .await;
        assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
        let body: Value = resp.json();
        let data = body["data"].as_array().expect("data array").clone();
        let total = body["total"].as_i64().expect("total");
        collected.extend(data);
        if (collected.len() as i64) >= total {
            break;
        }
        assert!(page < 200, "pagination did not converge");
        page += 1;
    }
    collected
}

/// Count account_users rows for an account/user pair.
async fn account_user_count(pool: &PgPool, account_id: i64, user_id: i64) -> i64 {
    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM account_users WHERE account_id = $1 AND user_id = $2")
            .bind(account_id)
            .bind(user_id)
            .fetch_one(pool)
            .await
            .unwrap();
    count
}

/// Count accounts rows created by this run.
async fn seeded_account_count(pool: &PgPool, prefix: &str) -> i64 {
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .fetch_one(pool)
        .await
        .unwrap();
    count
}

// ---------------------------------------------------------------------------
// GET /accounts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_accounts_as_admin_includes_seeded_accounts() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let parent = seed_account(&pool, &format!("{prefix}-Parent"), None, "active").await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(parent), "active").await;
    let standalone = seed_account(&pool, &format!("{prefix}-Standalone"), None, "active").await;

    let all = list_all_accounts(&server, &auth(admin, "admin")).await;
    let ours: Vec<&Value> = all
        .iter()
        .filter(|a| {
            a["name"]
                .as_str()
                .map(|n| n.starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(ours.len(), 3, "seeded accounts missing: {ours:?}");

    let by_id = |id: i64| {
        ours.iter()
            .find(|a| a["id"].as_i64() == Some(id))
            .unwrap_or_else(|| panic!("account {id} not listed"))
    };

    // Full Canvas API v1 shape is echoed for each row.
    let p = by_id(parent);
    assert_eq!(p["name"], format!("{prefix}-Parent"));
    assert_eq!(p["workflow_state"], "active");
    assert!(p["created_at"].is_string());
    assert!(p["updated_at"].is_string());
    assert!(
        p.get("parent_account_id").is_none(),
        "root account must omit parent_account_id: {p}"
    );

    let c = by_id(child);
    assert_eq!(c["parent_account_id"], parent, "child echoes parent id");

    assert_eq!(by_id(standalone)["name"], format!("{prefix}-Standalone"));

    // Data is ordered by id ASC.
    let ids: Vec<i64> = ours.iter().filter_map(|a| a["id"].as_i64()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "accounts must be listed in id order");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_accounts_pagination_clamps_out_of_range_params() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    // per_page is clamped to 100, page floors at 1; both echoed back.
    let resp = server
        .get("/accounts?per_page=500&page=0")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["per_page"], 100, "per_page must clamp to 100");
    assert_eq!(body["page"], 1, "page must floor at 1");
    assert!(
        body["data"].as_array().map(|d| d.len()).unwrap_or(0) <= 100,
        "never more than 100 rows per page"
    );
    assert!(body["total"].as_i64().is_some(), "total is reported");

    // A tiny page really returns a single row.
    let resp = server
        .get("/accounts?per_page=1&page=1")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["data"].as_array().map(|d| d.len()), Some(1));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_accounts_as_non_admin_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;

    for (user, role) in [(teacher, "teacher"), (student, "student")] {
        let resp = server
            .get("/accounts")
            .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    }

    // The extractor loads the role from the DB, so a token whose role claim
    // says 'admin' must still be rejected when the user is a teacher.
    let forged_claim = server
        .get("/accounts")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "admin"))
        .await;
    assert_eq!(
        forged_claim.status_code(),
        403,
        "body: {}",
        forged_claim.text()
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_accounts_includes_non_active_workflow_state() {
    // Documents actual behavior: no handler filters on workflow_state, so a
    // 'deleted' account is still listed and still readable. Canvas hides
    // deleted accounts from listings by default.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let deleted = seed_account(&pool, &format!("{prefix}-Deleted"), None, "deleted").await;

    let all = list_all_accounts(&server, &auth(admin, "admin")).await;
    let listed = all.iter().find(|a| a["id"].as_i64() == Some(deleted));
    assert!(listed.is_some(), "deleted account must appear in the list");
    assert_eq!(listed.unwrap()["workflow_state"], "deleted");

    let resp = server
        .get(&format!("/accounts/{deleted}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["workflow_state"], "deleted");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_account_as_admin_returns_full_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let parent = seed_account(&pool, &format!("{prefix}-Parent"), None, "active").await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(parent), "active").await;

    let resp = server
        .get(&format!("/accounts/{parent}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], parent);
    assert_eq!(body["name"], format!("{prefix}-Parent"));
    assert_eq!(body["workflow_state"], "active");
    assert!(
        body.get("parent_account_id").is_none(),
        "root account omits parent_account_id: {body}"
    );
    assert!(body["created_at"].is_string());
    assert!(body["updated_at"].is_string());

    let resp = server
        .get(&format!("/accounts/{child}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], child);
    assert_eq!(body["parent_account_id"], parent);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_account_missing_is_404_for_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .get("/accounts/999999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_account_as_plain_teacher_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None, "active").await;

    for (user, role) in [(teacher, "teacher"), (student, "student")] {
        let resp = server
            .get(&format!("/accounts/{account}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_account_as_account_admin_hits_sqlx_type_bug() {
    // KNOWN BUG (star-accounts/src/lib.rs:161-169): is_account_admin runs
    // `SELECT 1 FROM account_users ...` decoded into `(i64,)`, but Postgres
    // types the literal 1 as INT4, so sqlx fails to decode it exactly when
    // the grant row EXISTS. The intended behavior (200 for the granted
    // account and every descendant via the parent-chain walk) therefore
    // surfaces as 500 "database error" today. The negative cases below do
    // work because no row ever matches: the walk just returns false.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let viewer = seed_user(&pool, &prefix, "Viewer", "teacher").await;
    let parent = seed_account(&pool, &format!("{prefix}-Parent"), None, "active").await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(parent), "active").await;
    let grandchild = seed_account(
        &pool,
        &format!("{prefix}-Grandchild"),
        Some(child),
        "active",
    )
    .await;
    let sibling = seed_account(&pool, &format!("{prefix}-Sibling"), None, "active").await;

    seed_account_user(
        &pool,
        parent,
        viewer,
        chrono::Utc::now() - chrono::Duration::seconds(60),
    )
    .await;

    // Would-be 200s (account itself and descendants) come back as 500s.
    for account_id in [parent, child, grandchild] {
        let resp = server
            .get(&format!("/accounts/{account_id}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(viewer, "teacher"))
            .await;
        assert_eq!(
            resp.status_code(),
            500,
            "account admin on parent must (buggy) 500 for {account_id}: {}",
            resp.text()
        );
        assert!(
            resp.text().contains("database error"),
            "expected the sqlx decode failure body: {}",
            resp.text()
        );
    }

    // Denials still work: sibling (unrelated tree) and missing accounts are
    // 403, and the account admin is not a global admin.
    for path in [
        format!("/accounts/{sibling}"),
        "/accounts/999999999".to_string(),
    ] {
        let resp = server
            .get(&path)
            .add_header(axum::http::header::AUTHORIZATION, auth(viewer, "teacher"))
            .await;
        assert_eq!(resp.status_code(), 403, "path {path}: {}", resp.text());
    }

    let resp = server
        .get("/accounts")
        .add_header(axum::http::header::AUTHORIZATION, auth(viewer, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Permission is checked before existence, so the missing account above
    // yielded 403 (not 404); the global admin does get 404 for it.
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let resp = server
        .get("/accounts/999999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_account_as_account_admin_of_child_cannot_see_parent() {
    // The walk only climbs upwards, so an admin of a sub-account cannot read
    // its own parent — matching Canvas semantics. Reading the child itself
    // hits the SELECT 1 decode bug and 500s (see the test above).
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let viewer = seed_user(&pool, &prefix, "Viewer", "teacher").await;
    let parent = seed_account(&pool, &format!("{prefix}-Parent"), None, "active").await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(parent), "active").await;

    seed_account_user(
        &pool,
        child,
        viewer,
        chrono::Utc::now() - chrono::Duration::seconds(60),
    )
    .await;

    let resp = server
        .get(&format!("/accounts/{child}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(viewer, "teacher"))
        .await;
    assert_eq!(
        resp.status_code(),
        500,
        "buggy but granted: {}",
        resp.text()
    );

    let resp = server
        .get(&format!("/accounts/{parent}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(viewer, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_account_parent_chain_walk_stops_after_ten_hops() {
    // Documents MAX_ACCOUNT_TREE_DEPTH: an account admin can reach accounts
    // at most 10 hops up from the requested one. With an 11-account chain
    // c0 -> c1 -> ... -> c10 and the grant on c0, a read of c10 gives up at
    // the depth cap (403) before ever reaching the granted row, while a read
    // of c9 does reach it — and therefore hits the SELECT 1 decode bug (500)
    // instead of the intended 200.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let viewer = seed_user(&pool, &prefix, "Viewer", "teacher").await;

    let mut chain: Vec<i64> = Vec::new();
    let mut parent: Option<i64> = None;
    for i in 0..11 {
        let id = seed_account(&pool, &format!("{prefix}-Chain{i}"), parent, "active").await;
        chain.push(id);
        parent = Some(id);
    }

    seed_account_user(
        &pool,
        chain[0],
        viewer,
        chrono::Utc::now() - chrono::Duration::seconds(60),
    )
    .await;

    // Inside the depth budget: the walk reaches the granted row -> 500 (bug).
    let resp = server
        .get(&format!("/accounts/{}", chain[9]))
        .add_header(axum::http::header::AUTHORIZATION, auth(viewer, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 500, "body: {}", resp.text());

    // One hop beyond the budget: the walk gives up -> 403.
    let resp = server
        .get(&format!("/accounts/{}", chain[10]))
        .add_header(axum::http::header::AUTHORIZATION, auth(viewer, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The global admin is never affected by the depth cap.
    let resp = server
        .get(&format!("/accounts/{}", chain[10]))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /accounts/{id}/sub_accounts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_sub_account_with_existing_parent_hits_sqlx_type_bug() {
    // KNOWN BUG (star-accounts/src/lib.rs:318-326): the parent-existence
    // probe is `SELECT 1 FROM accounts WHERE id = $1` decoded into `(i64,)`,
    // but Postgres types 1 as INT4, so the decode fails exactly when the
    // parent EXISTS. Sub-account creation therefore 500s on the happy path
    // and never reaches the INSERT. Intended behavior: 201 with the response
    // echoing parent_account_id, a persisted row with workflow_state
    // 'active', and the new account visible in GET /accounts.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let parent = seed_account(&pool, &format!("{prefix}-Parent"), None, "active").await;
    let name = format!("{prefix}-NewSub");

    let resp = server
        .post(&format!("/accounts/{parent}/sub_accounts"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "name": name }))
        .await;
    assert_eq!(resp.status_code(), 500, "body: {}", resp.text());
    assert!(
        resp.text().contains("database error"),
        "expected the sqlx decode failure body: {}",
        resp.text()
    );

    // The INSERT is unreachable, so nothing may have been created.
    assert_eq!(
        seeded_account_count(&pool, &prefix).await,
        1,
        "only the seeded parent may exist"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_sub_account_missing_parent_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .post("/accounts/999999999/sub_accounts")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "name": format!("{prefix}-Orphan") }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    assert_eq!(
        seeded_account_count(&pool, &prefix).await,
        0,
        "nothing may be created for a missing parent"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_sub_account_blank_name_check_is_shadowed_by_type_bug() {
    // The blank-name validation (lib.rs:328-330, intended 422) is
    // unreachable today: the parent-existence probe at lib.rs:318 fails
    // first for any existing parent. Pinned as a 500 with no rows created.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let parent = seed_account(&pool, &format!("{prefix}-Parent"), None, "active").await;

    for blank in ["", "   "] {
        let resp = server
            .post(&format!("/accounts/{parent}/sub_accounts"))
            .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
            .json(&json!({ "name": blank }))
            .await;
        assert_eq!(resp.status_code(), 500, "name {blank:?}: {}", resp.text());
    }

    assert_eq!(
        seeded_account_count(&pool, &prefix).await,
        1,
        "only the seeded parent may exist"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_sub_account_as_non_admin_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let parent = seed_account(&pool, &format!("{prefix}-Parent"), None, "active").await;

    for (user, role) in [(teacher, "teacher"), (student, "student")] {
        let resp = server
            .post(&format!("/accounts/{parent}/sub_accounts"))
            .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
            .json(&json!({ "name": format!("{prefix}-Forbidden") }))
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    }

    assert_eq!(
        seeded_account_count(&pool, &prefix).await,
        1,
        "non-admins must not create accounts"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{id}/courses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_account_courses_includes_subaccount_courses() {
    // The handler walks the sub-account tree with a depth-guarded BFS and
    // lists courses of the whole subtree (matching Canvas). Courses of
    // SIBLING accounts are not included. Ordering is created_at DESC,
    // id DESC.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None, "active").await;
    let other = seed_account(&pool, &format!("{prefix}-Other"), None, "active").await;
    let sub = seed_account(&pool, &format!("{prefix}-Sub"), Some(account), "active").await;

    let older = seed_course(&pool, &prefix, "OlderCourse", account, None).await;
    let newer = seed_course(
        &pool,
        &prefix,
        "NewerCourse",
        account,
        Some("a test course"),
    )
    .await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse", other, None).await;
    let sub_course = seed_course(&pool, &prefix, "SubCourse", sub, None).await;

    let resp = server
        .get(&format!("/accounts/{account}/courses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["page"], 1);
    assert_eq!(body["per_page"], 50);
    assert_eq!(body["total"], 3, "direct + sub-account courses: {body}");

    let data = body["data"].as_array().expect("data array");
    assert_eq!(data.len(), 3);

    // Ordering is created_at DESC, id DESC: the later insert comes first.
    assert_eq!(data[0]["id"], sub_course);
    assert_eq!(data[0]["name"], "SubCourse");
    assert_eq!(data[0]["course_code"], format!("{prefix}-SubCourse"));
    assert_eq!(data[1]["id"], newer);
    assert_eq!(data[1]["name"], "NewerCourse");
    assert_eq!(data[1]["course_code"], format!("{prefix}-NewerCourse"));
    assert_eq!(data[1]["description"], "a test course");
    assert!(data[1]["created_at"].is_string());
    assert!(data[1]["updated_at"].is_string());

    assert_eq!(data[2]["id"], older);
    assert_eq!(data[2]["name"], "OlderCourse");
    assert_eq!(data[2]["course_code"], format!("{prefix}-OlderCourse"));
    assert!(
        data[2].get("description").is_none(),
        "NULL description is omitted: {}",
        data[2]
    );

    let listed_ids: Vec<i64> = data.iter().filter_map(|c| c["id"].as_i64()).collect();
    assert!(
        !listed_ids.contains(&other_course),
        "another account's course must not leak"
    );
    assert!(
        listed_ids.contains(&sub_course),
        "sub-account courses must be listed (BFS over the account tree)"
    );

    // Pagination over the account's own courses.
    let resp = server
        .get(&format!("/accounts/{account}/courses?per_page=1&page=2"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["data"].as_array().map(|d| d.len()), Some(1));
    assert_eq!(body["data"][0]["id"], newer);
    assert_eq!(body["total"], 3);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_account_courses_missing_account_returns_empty_200() {
    // Documents actual behavior (deviation): unlike list_account_users,
    // this handler never verifies the account exists, so a missing id
    // returns 200 with an empty page instead of 404.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .get("/accounts/999999999/courses")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["total"], 0);
    assert_eq!(body["data"].as_array().map(|d| d.len()), Some(0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_account_courses_as_non_admin_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None, "active").await;

    // Even an account admin of the account itself is rejected here: the
    // courses listing requires the global admin role.
    seed_account_user(
        &pool,
        account,
        teacher,
        chrono::Utc::now() - chrono::Duration::seconds(60),
    )
    .await;

    let resp = server
        .get(&format!("/accounts/{account}/courses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{id}/users
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_account_users_with_existing_account_hits_sqlx_type_bug() {
    // KNOWN BUG (star-accounts/src/lib.rs:437-445): the account-existence
    // probe is `SELECT 1 FROM accounts WHERE id = $1` decoded into `(i64,)`,
    // but Postgres types 1 as INT4, so the decode fails exactly when the
    // account EXISTS. The listing 500s on the happy path. Intended behavior
    // (untestable until fixed): account_users JOIN users on the account,
    // ordered by au.created_at ASC with page/per_page/total pagination,
    // echoing user_name/user_email; enrollment-only users never appear —
    // the seeded enrollment below would prove that once the bug is fixed.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let first = seed_user(&pool, &prefix, "First", "teacher").await;
    let second = seed_user(&pool, &prefix, "Second", "student").await;
    let enrolled_only = seed_user(&pool, &prefix, "EnrolledOnly", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None, "active").await;
    let other = seed_account(&pool, &format!("{prefix}-Other"), None, "active").await;
    let course = seed_course(&pool, &prefix, "AccountCourse", account, None).await;

    seed_account_user(
        &pool,
        account,
        first,
        chrono::Utc::now() - chrono::Duration::seconds(120),
    )
    .await;
    seed_account_user(
        &pool,
        account,
        second,
        chrono::Utc::now() - chrono::Duration::seconds(60),
    )
    .await;
    seed_account_user(
        &pool,
        other,
        admin,
        chrono::Utc::now() - chrono::Duration::seconds(30),
    )
    .await;
    seed_enrollment(&pool, enrolled_only, course).await;

    let resp = server
        .get(&format!("/accounts/{account}/users"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 500, "body: {}", resp.text());
    assert!(
        resp.text().contains("database error"),
        "expected the sqlx decode failure body: {}",
        resp.text()
    );

    // The listing is read-only: all seeded rows must be untouched.
    assert_eq!(account_user_count(&pool, account, first).await, 1);
    assert_eq!(account_user_count(&pool, account, second).await, 1);
    assert_eq!(account_user_count(&pool, other, admin).await, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_account_users_missing_account_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .get("/accounts/999999999/users")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_account_users_as_non_admin_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None, "active").await;

    let resp = server
        .get(&format!("/accounts/{account}/users"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /accounts/{id}/users/{userId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn grant_account_admin_with_existing_rows_hits_sqlx_type_bug() {
    // KNOWN BUG (star-accounts/src/lib.rs:493-512): both existence probes
    // (`SELECT 1 FROM accounts ...` and `SELECT 1 FROM users ...`) are
    // decoded into `(i64,)`, but Postgres types 1 as INT4, so the decode
    // fails exactly when the rows exist. The grant 500s on the happy path
    // and never reaches the upsert at lib.rs:522. Intended behavior
    // (untestable until fixed): 201 with the AccountUser body, role
    // defaulting to 'admin' (for {} and {"role": null}), duplicates
    // upserting the same (account_id, user_id) row, and only 'admin'
    // accepted (422 otherwise). The duplicate, default-role and
    // invalid-role requests are all pinned below as 500s with zero rows
    // created.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let target = seed_user(&pool, &prefix, "Target", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None, "active").await;
    let child = seed_account(&pool, &format!("{prefix}-Child"), Some(account), "active").await;
    let sibling = seed_account(&pool, &format!("{prefix}-Sibling"), None, "active").await;

    for payload in [
        json!({}),                    // default role
        json!({ "role": null }),      // explicit null role
        json!({ "role": "admin" }),   // explicit admin
        json!({ "role": "admin" }),   // duplicate grant
        json!({ "role": "teacher" }), // would-be 422 validation
    ] {
        let resp = server
            .post(&format!("/accounts/{account}/users/{target}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
            .json(&payload)
            .await;
        assert_eq!(
            resp.status_code(),
            500,
            "payload {payload}: {}",
            resp.text()
        );
        assert!(
            resp.text().contains("database error"),
            "expected the sqlx decode failure body: {}",
            resp.text()
        );
    }

    // The upsert is unreachable, so no grant may exist...
    assert_eq!(
        account_user_count(&pool, account, target).await,
        0,
        "the 500 must be side-effect free"
    );

    // ...and the target still has no scoped read access: the walk finds no
    // grant row anywhere, so the account, its child and the sibling are all
    // denied (403, not the buggy 500 a granted user would hit).
    for account_id in [account, child, sibling] {
        let resp = server
            .get(&format!("/accounts/{account_id}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(target, "student"))
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    }

    // The global admin is unaffected and can still read the account.
    let resp = server
        .get(&format!("/accounts/{account}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grant_account_admin_missing_account_or_user_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let target = seed_user(&pool, &prefix, "Target", "student").await;

    // Missing account: the probe returns no row, so the decode bug does not
    // trigger and the intended 404 is produced.
    let resp = server
        .post(&format!("/accounts/999999999/users/{target}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 404, "missing account: {}", resp.text());

    // Missing user with an existing account: the account probe now matches a
    // row and trips the decode bug (lib.rs:493) before the user probe, so
    // the intended 404 surfaces as 500 today.
    let account = seed_account(&pool, &format!("{prefix}-Account"), None, "active").await;
    let resp = server
        .post(&format!("/accounts/{account}/users/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 500, "missing user: {}", resp.text());

    let (count,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM account_users WHERE account_id = $1")
            .bind(account)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grant_account_admin_as_non_admin_is_403_even_for_account_admins() {
    // Documents actual behavior: only the global admin role may grant;
    // being an account admin of the target account is not enough. The role
    // gate runs before any DB probe, so this path is unaffected by the
    // decode bug.
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let account_admin = seed_user(&pool, &prefix, "AccountAdmin", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None, "active").await;

    seed_account_user(
        &pool,
        account,
        account_admin,
        chrono::Utc::now() - chrono::Duration::seconds(60),
    )
    .await;

    for (user, role) in [(account_admin, "teacher"), (student, "student")] {
        let resp = server
            .post(&format!("/accounts/{account}/users/{student}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
            .json(&json!({}))
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    }

    assert_eq!(
        account_user_count(&pool, account, student).await,
        0,
        "non-admins must not grant anything"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Authentication
// ---------------------------------------------------------------------------

#[tokio::test]
async fn all_routes_without_token_are_401() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // POST routes defined by the router. (/accounts itself is GET-only; a
    // POST there is a 405 method mismatch, out of scope here.)
    let post_cases: Vec<(&str, Option<Value>)> = vec![
        (
            "/accounts/1/sub_accounts",
            Some(json!({ "name": "unauthenticated" })),
        ),
        ("/accounts/1/users/2", Some(json!({}))),
    ];

    for (path, body) in post_cases {
        let mut req = server.post(path);
        if let Some(payload) = body {
            req = req.json(&payload);
        }
        let resp = req.await;
        assert_eq!(
            resp.status_code(),
            401,
            "POST {path} without token must be 401: {}",
            resp.text()
        );
    }

    for path in [
        "/accounts",
        "/accounts/1",
        "/accounts/1/courses",
        "/accounts/1/users",
    ] {
        let resp = server.get(path).await;
        assert_eq!(
            resp.status_code(),
            401,
            "GET {path} without token must be 401: {}",
            resp.text()
        );
    }
}

#[tokio::test]
async fn garbage_bearer_token_is_401() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server
        .get("/accounts")
        .add_header(axum::http::header::AUTHORIZATION, "Bearer not-a-jwt")
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    let resp = server
        .get("/accounts/1")
        .add_header(axum::http::header::AUTHORIZATION, "Bearer not-a-jwt")
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
}
