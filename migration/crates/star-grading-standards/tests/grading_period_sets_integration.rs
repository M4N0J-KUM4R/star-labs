//! Integration tests for the Canvas grading-period-sets routes.
//!
//!   GET/POST  /accounts/{accountId}/grading_period_sets
//!   PATCH/DELETE /accounts/{accountId}/grading_period_sets/{id}
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-grading-standards \
//!   --test grading_period_sets_integration
//!
//! Cases:
//!   1. CRUD happy path: POST 201 with the Canvas serializer shape
//!      (grading_period_set wrapper + permissions + created_at), GET lists,
//!      PATCH 204 + merged fields, DELETE 204 + repeat 404.
//!   2. Authz: non-root account manage → 403; teacher (read-only path) can
//!      index but not create; anonymous → 401; unknown account → 404.
//!   3. Validation: blank title → 422.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gps-tests-{}-{}",
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
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_grading_standards::router())
            .with_state(state),
    )
    .expect("build TestServer");
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

async fn seed_root_account(pool: &PgPool, prefix: &str) -> i64 {
    // A dedicated root account (parent_account_id NULL) for this test, per
    // the issue #75 isolation convention: never reuse the global root.
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, NULL) RETURNING id",
    )
    .bind(format!("{prefix}-root"))
    .fetch_one(pool)
    .await
    .expect("seed root account")
}

async fn seed_sub_account(pool: &PgPool, prefix: &str, parent: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-sub"))
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed sub account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account admin");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // Dedicated accounts cascade nothing shared; delete by name prefix.
    let _ = sqlx::query(
        "DELETE FROM grading_period_groups WHERE account_id IN \
         (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE account_id IN \
         (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn crud_happy_path() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let root = seed_root_account(&pool, &prefix).await;
    seed_account_admin(&pool, root, admin).await;
    let hdr = auth(admin, "admin");

    // Create → 201 with the Canvas serializer shape.
    let resp = server
        .post(&format!("/accounts/{root}/grading_period_sets"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"grading_period_set": {
            "title": "Fall Semesters",
            "weighted": true,
            "display_totals_for_all_grading_periods": true
        }}))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let body: Value = resp.json();
    let set = &body["grading_period_set"];
    let set_id = set["id"]
        .as_str()
        .expect("string id")
        .parse::<i64>()
        .unwrap();
    assert_eq!(set["title"], "Fall Semesters");
    assert_eq!(set["weighted"], true);
    assert_eq!(set["display_totals_for_all_grading_periods"], true);
    assert_eq!(set["account_id"], root);
    assert_eq!(set["grading_periods"], json!([]));
    assert_eq!(set["permissions"], json!({"read": true, "manage": true}));
    assert!(set["created_at"].as_str().is_some());

    // Index lists it (read path is the same admin here).
    let resp = server
        .get(&format!("/accounts/{root}/grading_period_sets"))
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let list = body["grading_period_sets"].as_array().expect("array");
    assert_eq!(list.len(), 1, "{body}");
    assert_eq!(list[0]["title"], "Fall Semesters");

    // PATCH → 204; merged on the next read.
    let resp = server
        .patch(&format!("/accounts/{root}/grading_period_sets/{set_id}"))
        .add_header("Authorization", hdr.clone())
        .content_type("application/json")
        .json(&json!({"grading_period_set": {"title": "Fall 2026", "weighted": false}}))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let resp = server
        .get(&format!("/accounts/{root}/grading_period_sets"))
        .add_header("Authorization", hdr.clone())
        .await;
    let body: Value = resp.json();
    let set = &body["grading_period_sets"][0];
    assert_eq!(set["title"], "Fall 2026");
    assert_eq!(set["weighted"], false, "merged field updated");
    assert_eq!(
        set["display_totals_for_all_grading_periods"], true,
        "untouched field preserved"
    );

    // DELETE → 204, then the set is gone (repeat DELETE 404).
    let resp = server
        .delete(&format!("/accounts/{root}/grading_period_sets/{set_id}"))
        .add_header("Authorization", hdr.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let resp = server
        .delete(&format!("/accounts/{root}/grading_period_sets/{set_id}"))
        .add_header("Authorization", hdr)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn authorization_chain() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let root = seed_root_account(&pool, &prefix).await;
    let sub = seed_sub_account(&pool, &prefix, root).await;
    seed_account_admin(&pool, root, admin).await;

    // Sub-account admin: create is root-account-only → 403 even as admin.
    let sub_admin = seed_user(&pool, &prefix, "SubAdmin", "admin").await;
    seed_account_admin(&pool, sub, sub_admin).await;
    let resp = server
        .post(&format!("/accounts/{sub}/grading_period_sets"))
        .add_header("Authorization", auth(sub_admin, "admin"))
        .content_type("application/json")
        .json(&json!({"grading_period_set": {"title": "Nope"}}))
        .await;
    assert_eq!(resp.status_code(), 403, "non-root manage: {}", resp.text());

    // Root-account create by a teacher (no manage right) → 403.
    let resp = server
        .post(&format!("/accounts/{root}/grading_period_sets"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .content_type("application/json")
        .json(&json!({"grading_period_set": {"title": "Nope"}}))
        .await;
    assert_eq!(resp.status_code(), 403, "no manage: {}", resp.text());

    // Anonymous create → 401.
    let resp = server
        .post(&format!("/accounts/{root}/grading_period_sets"))
        .content_type("application/json")
        .json(&json!({"grading_period_set": {"title": "Nope"}}))
        .await;
    assert_eq!(resp.status_code(), 401, "anonymous: {}", resp.text());

    // Unknown account → 404 (before authz).
    let resp = server
        .get("/accounts/999999999/grading_period_sets")
        .add_header("Authorization", auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown account: {}", resp.text());

    // Teacher without any account link cannot even index.
    let resp = server
        .get(&format!("/accounts/{root}/grading_period_sets"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "no read link: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn validation_errors() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let root = seed_root_account(&pool, &prefix).await;
    seed_account_admin(&pool, root, admin).await;

    // Blank title on create → 422.
    let resp = server
        .post(&format!("/accounts/{root}/grading_period_sets"))
        .add_header("Authorization", auth(admin, "admin"))
        .content_type("application/json")
        .json(&json!({"grading_period_set": {"title": "  "}}))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    // Create a set, then blank title on update → 422.
    let resp = server
        .post(&format!("/accounts/{root}/grading_period_sets"))
        .add_header("Authorization", auth(admin, "admin"))
        .content_type("application/json")
        .json(&json!({"grading_period_set": {"title": "Valid"}}))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let set_id = resp.json::<Value>()["grading_period_set"]["id"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let resp = server
        .patch(&format!("/accounts/{root}/grading_period_sets/{set_id}"))
        .add_header("Authorization", auth(admin, "admin"))
        .content_type("application/json")
        .json(&json!({"grading_period_set": {"title": ""}}))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
