//! Integration tests for the star-users user logins (pseudonyms), course
//! nicknames and bookmarks routes.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Unique per-run prefix + full teardown, so repeat runs against a shared
//!   database are safe.
//!
//! The routes under test live in `star_users::users_api`; the coordinator
//! wires `star_users::users_api::router()` into the crate router.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use serde_json::{json, Value};
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
        "users-api-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, migrate, and build a TestServer over the users API router.
async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = std::env::var("DATABASE_URL").ok()?;

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&db_url)
        .await
        .expect("connect to Postgres");

    star_db::MIGRATOR.run(&pool).await.expect("run migrations");

    // The course nicknames table is new; the coordinator turns it into a
    // migration, the test creates it on its own so it runs either way.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS course_nicknames (
            id         BIGSERIAL PRIMARY KEY,
            user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            course_id  BIGINT NOT NULL,
            nickname   TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            UNIQUE (user_id, course_id)
        )
        "#,
    )
    .execute(&pool)
    .await
    .expect("create course_nicknames");

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };

    let server = TestServer::new(star_users::users_api::router().with_state(state))
        .expect("build TestServer");
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
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin') \
         ON CONFLICT (account_id, user_id) DO NOTHING",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account admin");
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

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student') \
         ON CONFLICT (user_id, course_id, role) DO NOTHING",
    )
    .bind(user_id)
    .bind(course_id)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// A Canvas login (a `pseudonyms` row).
async fn seed_pseudonym(
    pool: &PgPool,
    user_id: i64,
    account_id: i64,
    unique_id: &str,
    position: i32,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO pseudonyms (
            user_id, account_id, workflow_state, unique_id, unique_id_normalized,
            crypted_password, password_salt, persistence_token, single_access_token,
            perishable_token, created_at, updated_at, unique_ids, position
        )
        VALUES ($1, $2, 'active', $3, $4, 'not-a-real-hash', '', 'token', 'token', 'token',
                now(), now(), '{}'::jsonb, $5)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(account_id)
    .bind(unique_id)
    .bind(unique_id.to_lowercase())
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed pseudonym")
}

async fn seed_channel(pool: &PgPool, user_id: i64, path: &str) {
    sqlx::query(
        "INSERT INTO communication_channels (user_id, path, path_type, workflow_state) \
         VALUES ($1, $2, 'email', 'active')",
    )
    .bind(user_id)
    .bind(path)
    .execute(pool)
    .await
    .expect("seed communication channel");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM course_nicknames WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM pseudonyms WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM bookmarks_bookmarks WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM communication_channels WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
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

// ---------------------------------------------------------------------------
// Pseudonyms
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_logins_lists_the_logins_of_the_account() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let login = seed_pseudonym(&pool, student, account, "student@example.com", 1).await;

    let response = server
        .get(&format!("/accounts/{account}/logins"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let list = body.as_array().expect("array");
    assert_eq!(list.len(), 1, "one login of the account");
    assert_eq!(list[0]["id"], json!(login));
    assert_eq!(list[0]["user_id"], json!(student));
    assert_eq!(list[0]["account_id"], json!(account));
    assert_eq!(list[0]["unique_id"], json!("student@example.com"));
    assert_eq!(list[0]["workflow_state"], json!("active"));
    assert!(list[0]["created_at"].is_string());
    // The caller administrates the account, so the SIS fields are visible.
    assert!(list[0].get("sis_user_id").is_some());

    // The user themself sees the same list.
    let response = server
        .get(&format!("/accounts/{account}/logins"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body.as_array().map(|list| list.len()), Some(1));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_logins_answers_404_and_400() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let sub = seed_account(&pool, &prefix, "sub", Some(account)).await;

    // A missing account is a 404.
    let response = server
        .get("/accounts/99999999/logins")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    // A sub account is a 400.
    let response = server
        .get(&format!("/accounts/{sub}/logins"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(
        body["message"],
        json!("Action must be called on a root account.")
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn user_logins_answers_404_and_403() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "student", "student").await;
    let stranger = seed_user(&pool, &prefix, "stranger", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    seed_pseudonym(&pool, student, account, "student@example.com", 1).await;

    // The user themself.
    let response = server
        .get("/users/self/logins")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body.as_array().map(|list| list.len()), Some(1));

    // A missing user is a 404.
    let response = server
        .get("/users/99999999/logins")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    // Another user without the right is a 403.
    let response = server
        .get(&format!("/users/{student}/logins"))
        .add_header(AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(response.status_code(), 403, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_login_checks_the_permissions_and_creates_the_login() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let sub = seed_account(&pool, &prefix, "sub", Some(account)).await;

    // An account admin creates a login for the user.
    let response = server
        .post(&format!("/accounts/{account}/logins"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user": { "id": student },
            "login": { "unique_id": "new-login", "declared_user_type": "teacher" },
        }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["unique_id"], json!("new-login"));
    assert_eq!(body["user_id"], json!(student));
    assert_eq!(body["account_id"], json!(account));
    assert_eq!(body["workflow_state"], json!("active"));
    assert_eq!(body["declared_user_type"], json!("teacher"));

    // Without a recognized parameter the action is a 400.
    let response = server
        .post(&format!("/accounts/{account}/logins"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "user": { "id": student } }))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["message"], json!("missing required parameter"));

    // A sub account is a 400.
    let response = server
        .post(&format!("/accounts/{sub}/logins"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "user": { "id": student },
            "login": { "unique_id": "another-login" },
        }))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());

    // A user without `manage_user_logins` is a 403.
    let response = server
        .post(&format!("/accounts/{account}/logins"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "user": { "id": student },
            "login": { "unique_id": "student-login" },
        }))
        .await;
    assert_eq!(response.status_code(), 403, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_login_changes_the_login() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let login = seed_pseudonym(&pool, student, account, "student@example.com", 1).await;

    let response = server
        .put(&format!("/accounts/{account}/logins/{login}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "login": { "unique_id": "renamed@example.com" } }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["unique_id"], json!("renamed@example.com"));
    assert_eq!(body["id"], json!(login));

    // The change is persisted.
    let stored: (String,) = sqlx::query_as("SELECT unique_id FROM pseudonyms WHERE id = $1")
        .bind(login)
        .fetch_one(&pool)
        .await
        .expect("read the login");
    assert_eq!(stored.0, "renamed@example.com");

    // An invalid workflow state is a 400.
    let response = server
        .put(&format!("/accounts/{account}/logins/{login}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "login": { "workflow_state": "bogus" } }))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["workflow_state"][0], json!("invalid workflow_state"));

    // A missing login is a 404.
    let response = server
        .put(&format!("/accounts/{account}/logins/99999999"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "login": { "unique_id": "x" } }))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn destroy_login_soft_deletes_the_login() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let first = seed_pseudonym(&pool, student, account, "one@example.com", 1).await;
    let second = seed_pseudonym(&pool, student, account, "two@example.com", 2).await;

    // The login of another user is a 404.
    let response = server
        .delete(&format!("/users/{admin}/logins/{first}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    let response = server
        .delete(&format!("/users/{student}/logins/{first}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"], json!(first));
    assert_eq!(body["workflow_state"], json!("deleted"));

    // The row survives, soft deleted.
    let stored: (String,) = sqlx::query_as("SELECT workflow_state FROM pseudonyms WHERE id = $1")
        .bind(first)
        .fetch_one(&pool)
        .await
        .expect("read the login");
    assert_eq!(stored.0, "deleted");

    // The last login of the user may not be removed.
    let response = server
        .delete(&format!("/users/{student}/logins/{second}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["base"][0], json!("Users must have at least one login"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn reset_password_answers_the_documented_json() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    seed_account_admin(&pool, account, admin).await;
    // The caller needs a login of their own.
    seed_pseudonym(&pool, admin, account, "admin@example.com", 1).await;
    // Canvas looks the address up in the domain root account, so the login of
    // the address has to live there.
    let domain_root: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("domain root account");
    seed_pseudonym(&pool, student, domain_root, "student@example.com", 1).await;
    seed_channel(&pool, student, "student@example.com").await;

    // A user without `manage_user_logins` is a 403.
    let response = server
        .post("/users/reset_password")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "email": "student@example.com" }))
        .await;
    assert_eq!(response.status_code(), 403, "body: {}", response.text());

    // The account admin of the address gets the recovery started.
    let response = server
        .post("/users/reset_password")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "email": "student@example.com" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["requested"], json!(true));

    // An unknown address is a 404.
    let response = server
        .post("/users/reset_password")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "email": "nobody@example.com" }))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["requested"], json!(false));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Course nicknames
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_nicknames_are_set_read_and_removed() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let course = seed_course(&pool, &prefix, "physics", account).await;

    // A missing nickname is a 400.
    let response = server
        .put(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["message"], json!("missing nickname"));

    // A nickname of 60 characters is too long.
    let long = "n".repeat(60);
    let response = server
        .put(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "nickname": long }))
        .await;
    assert_eq!(response.status_code(), 400, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["message"], json!("nickname too long"));

    // The nickname is set.
    let response = server
        .put(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "nickname": "Physics" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["course_id"], json!(course));
    assert_eq!(body["nickname"], json!("Physics"));
    assert_eq!(body["name"], json!(format!("{prefix}-physics")));

    // It is read back.
    let response = server
        .get(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["nickname"], json!("Physics"));

    // And it is listed.
    let response = server
        .get("/users/self/course_nicknames")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let list = body.as_array().expect("array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["course_id"], json!(course));

    // Removing it answers the nickname without a value.
    let response = server
        .delete(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert!(body["nickname"].is_null());

    // Removing it twice is a 404.
    let response = server
        .delete(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["message"], json!("no nickname exists for course"));

    // A missing course is a 404.
    let response = server
        .get("/users/self/course_nicknames/99999999")
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_nicknames_are_cleared_and_are_private() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let account = seed_account(&pool, &prefix, "root", None).await;
    let course = seed_course(&pool, &prefix, "physics", account).await;

    // A user who may not read the course is a 403.
    let response = server
        .get(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 403, "body: {}", response.text());

    // An enrolled user may read the course.
    seed_enrollment(&pool, student, course).await;
    let response = server
        .get(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());

    // The nickname of the student is not the nickname of the admin.
    let response = server
        .put(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "nickname": "Student nickname" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());

    let response = server
        .get(&format!("/users/self/course_nicknames/{course}"))
        .add_header(AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert!(body["nickname"].is_null());

    // Clearing removes every nickname of the caller.
    let response = server
        .delete("/users/self/course_nicknames")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["message"], json!("OK"));

    let response = server
        .get("/users/self/course_nicknames")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body.as_array().map(|list| list.len()), Some(0));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Bookmarks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bookmarks_are_created_read_updated_and_removed() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "student", "student").await;

    // A bookmark is created.
    let response = server
        .post("/users/self/bookmarks")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "name": "Biology 101", "url": "/courses/1" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let id = body["id"].as_i64().expect("bookmark id");
    assert_eq!(body["name"], json!("Biology 101"));
    assert_eq!(body["url"], json!("/courses/1"));
    assert_eq!(body["position"], json!(1));
    assert!(body["data"].is_null());

    // A bookmark with data keeps it.
    let response = server
        .post("/users/self/bookmarks")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "name": "Chemistry",
            "url": "/courses/2",
            "data": { "active_tab": 1 },
        }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["data"], json!({ "active_tab": 1 }));
    assert_eq!(body["position"], json!(2));

    // The list is ordered by position.
    let response = server
        .get("/users/self/bookmarks")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    let list = body.as_array().expect("array");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["name"], json!("Biology 101"));
    assert_eq!(list[1]["name"], json!("Chemistry"));

    // The single bookmark is read.
    let response = server
        .get(&format!("/users/self/bookmarks/{id}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"], json!(id));

    // It is updated.
    let response = server
        .put(&format!("/users/self/bookmarks/{id}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "name": "Renamed" }))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["name"], json!("Renamed"));
    assert_eq!(body["url"], json!("/courses/1"));

    // A missing bookmark is a 404.
    let response = server
        .get("/users/self/bookmarks/99999999")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    // The bookmark of another user is not found.
    let other = seed_user(&pool, &prefix, "other", "student").await;
    let response = server
        .get(&format!("/users/self/bookmarks/{id}"))
        .add_header(AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(response.status_code(), 404, "body: {}", response.text());

    // The bookmark is removed; Canvas answers with the removed bookmark.
    let response = server
        .delete(&format!("/users/self/bookmarks/{id}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body["id"], json!(id));
    assert_eq!(body["name"], json!("Renamed"));

    let response = server
        .get("/users/self/bookmarks")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200, "body: {}", response.text());
    let body: Value = response.json();
    assert_eq!(body.as_array().map(|list| list.len()), Some(1));

    cleanup(&pool, &prefix).await;
}
