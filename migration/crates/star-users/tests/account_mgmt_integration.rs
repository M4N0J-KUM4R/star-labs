//! Integration tests for the star-users account-management slice:
//! custom colors, dashboard positions, session termination, and
//! POST /accounts/{accountId}/users.
//!
//! Skipped cleanly when DATABASE_URL is unset. Unique per-run prefix + full
//! teardown so repeat runs against a shared database are safe.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "acct-mgmt-{}-{}",
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
            .merge(star_users::router())
            .with_state(state),
    )
    .expect("build TestServer");

    Some((server, pool, run_prefix()))
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO users (name, sortable_name, email, password_hash, role) \
         VALUES ($1, $2, $3, 'not-a-real-hash', $4) RETURNING id",
    )
    .bind(name)
    .bind(&sortable_name)
    .bind(format!("{prefix}-{name}@test.local".replace(' ', "-")))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user");
    id
}

async fn seed_course(pool: &PgPool, prefix: &str) -> i64 {
    let (id,): (i64,) =
        sqlx::query_as("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix} course"))
            .bind(format!("{prefix}-COURSE"))
            .fetch_one(pool)
            .await
            .expect("seed course");
    id
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
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
}

#[tokio::test]
async fn custom_colors_roundtrip_and_validation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Color Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Color Other", "student").await;
    let course = seed_course(&pool, &prefix).await;
    let context = format!("course_{course}");
    let url = format!("/users/{owner}/colors");

    // Missing hexcode -> 400.
    let res = server
        .put(&format!("{url}/{context}"))
        .add_header("Authorization", auth(owner, "student"))
        .json(&json!({ "hexcode": "not-a-color" }))
        .await;
    assert_eq!(res.status_code(), 400);

    // Unknown context -> 404 (Canvas Context.find_by_asset_string).
    let res = server
        .put(&format!("{url}/course_999999999"))
        .add_header("Authorization", auth(owner, "student"))
        .json(&json!({ "hexcode": "abc123" }))
        .await;
    assert_eq!(res.status_code(), 404);

    // Valid set normalizes the leading '#'.
    let res = server
        .put(&format!("{url}/{context}"))
        .add_header("Authorization", auth(owner, "student"))
        .json(&json!({ "hexcode": "abc123" }))
        .await;
    assert_eq!(res.status_code(), 200);
    assert_eq!(res.json::<Value>()["hexcode"], "#abc123");

    let res = server
        .get(&url)
        .add_header("Authorization", auth(owner, "student"))
        .await;
    assert_eq!(res.status_code(), 200);
    assert_eq!(
        res.json::<Value>()["custom_colors"][&context],
        Value::String("#abc123".into())
    );

    let res = server
        .get(&format!("{url}/{context}"))
        .add_header("Authorization", auth(owner, "student"))
        .await;
    assert_eq!(res.json::<Value>()["hexcode"], "#abc123");

    // Unset asset string -> 404.
    let res = server
        .get(&format!("{url}/course_{}", course + 1))
        .add_header("Authorization", auth(owner, "student"))
        .await;
    assert_eq!(res.status_code(), 404);

    // Non-self, non-admin -> 403 for both read and write.
    let res = server
        .get(&url)
        .add_header("Authorization", auth(other, "student"))
        .await;
    assert_eq!(res.status_code(), 403);
    let res = server
        .put(&format!("{url}/{context}"))
        .add_header("Authorization", auth(other, "student"))
        .json(&json!({ "hexcode": "ffffff" }))
        .await;
    assert_eq!(res.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn dashboard_positions_roundtrip_and_validation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Dash Owner", "teacher").await;
    let other = seed_user(&pool, &prefix, "Dash Other", "student").await;
    let first = seed_course(&pool, &prefix).await;
    let second = seed_course(&pool, &prefix).await;
    let url = format!("/users/{owner}/dashboard_positions");

    // Empty map is a no-op 200 with the current (empty) map.
    let res = server
        .put(&url)
        .add_header("Authorization", auth(owner, "teacher"))
        .json(&json!({ "dashboard_positions": {} }))
        .await;
    assert_eq!(res.status_code(), 200);
    assert!(res.json::<Value>()["dashboard_positions"]
        .as_object()
        .expect("map")
        .is_empty());

    // Numbers and strings are both accepted; the response is the merged map.
    let res = server
        .put(&url)
        .add_header("Authorization", auth(owner, "teacher"))
        .json(&json!({ "dashboard_positions": {
            format!("course_{first}"): 2,
            format!("course_{second}"): "1"
        }}))
        .await;
    assert_eq!(res.status_code(), 200);
    let body = res.json::<Value>();
    assert_eq!(body["dashboard_positions"][format!("course_{first}")], 2);
    assert_eq!(body["dashboard_positions"][format!("course_{second}")], 1);

    let res = server
        .get(&url)
        .add_header("Authorization", auth(owner, "teacher"))
        .await;
    assert_eq!(
        res.json::<Value>()["dashboard_positions"][format!("course_{second}")],
        1
    );

    // Position above the Canvas limit -> 400.
    let res = server
        .put(&url)
        .add_header("Authorization", auth(owner, "teacher"))
        .json(&json!({ "dashboard_positions": { format!("course_{first}"): 1001 } }))
        .await;
    assert_eq!(res.status_code(), 400);

    // Non-numeric position -> 400.
    let res = server
        .put(&url)
        .add_header("Authorization", auth(owner, "teacher"))
        .json(&json!({ "dashboard_positions": { format!("course_{first}"): "top" } }))
        .await;
    assert_eq!(res.status_code(), 400);

    // Unknown asset -> 404.
    let res = server
        .put(&url)
        .add_header("Authorization", auth(owner, "teacher"))
        .json(&json!({ "dashboard_positions": { "course_999999999": 1 } }))
        .await;
    assert_eq!(res.status_code(), 404);

    // Another regular user -> 403.
    let res = server
        .get(&url)
        .add_header("Authorization", auth(other, "student"))
        .await;
    assert_eq!(res.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn terminate_sessions_expires_tokens() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Session Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Session Other", "student").await;
    let (token_id,): (i64,) = sqlx::query_as(
        "INSERT INTO access_tokens (user_id, token, purpose) \
         VALUES ($1, $2, 'session') RETURNING id",
    )
    .bind(owner)
    .bind(format!("{prefix}-token"))
    .fetch_one(&pool)
    .await
    .expect("seed token");

    let res = server
        .delete(&format!("/users/{owner}/sessions"))
        .add_header("Authorization", auth(other, "student"))
        .await;
    assert_eq!(res.status_code(), 403);

    let res = server
        .delete(&format!("/users/{owner}/sessions"))
        .add_header("Authorization", auth(owner, "student"))
        .await;
    assert_eq!(res.status_code(), 200);
    assert_eq!(res.json::<Value>()["message"], "All sessions terminated");

    let (expires,): (Option<chrono::DateTime<chrono::Utc>>,) =
        sqlx::query_as("SELECT permanent_expires_at FROM access_tokens WHERE id = $1")
            .bind(token_id)
            .fetch_one(&pool)
            .await
            .expect("fetch token");
    assert!(expires.is_some());

    let (last_logged_out,): (Option<chrono::DateTime<chrono::Utc>>,) =
        sqlx::query_as("SELECT last_logged_out FROM users WHERE id = $1")
            .bind(owner)
            .fetch_one(&pool)
            .await
            .expect("fetch user");
    assert!(last_logged_out.is_some());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_account_user_permissions_and_validation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Acct Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Acct Student", "student").await;
    let (account_id,): (i64,) =
        sqlx::query_as("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
            .bind(format!("{prefix} account"))
            .fetch_one(&pool)
            .await
            .expect("seed account");
    let url = format!("/accounts/{account_id}/users");
    let email = format!("{prefix}-created@test.local");

    // Regular users cannot create account users.
    let res = server
        .post(&url)
        .add_header("Authorization", auth(student, "student"))
        .json(&json!({
            "user": { "name": "Nope Person" },
            "pseudonym": { "unique_id": format!("{prefix}-nope@test.local") }
        }))
        .await;
    assert_eq!(res.status_code(), 403);

    // Missing name -> 400.
    let res = server
        .post(&url)
        .add_header("Authorization", auth(admin, "admin"))
        .json(&json!({ "pseudonym": { "unique_id": email } }))
        .await;
    assert_eq!(res.status_code(), 400);

    // Admin creates the user; Canvas-shaped User JSON comes back.
    let res = server
        .post(&url)
        .add_header("Authorization", auth(admin, "admin"))
        .json(&json!({
            "user": { "name": "Created Person" },
            "pseudonym": { "unique_id": email }
        }))
        .await;
    assert_eq!(res.status_code(), 200);
    let created = res.json::<Value>();
    assert_eq!(created["name"], "Created Person");
    assert_eq!(created["email"], email);
    assert_eq!(created["role"], "student");

    // Duplicate login -> 400.
    let res = server
        .post(&url)
        .add_header("Authorization", auth(admin, "admin"))
        .json(&json!({
            "user": { "name": "Created Person" },
            "pseudonym": { "unique_id": email }
        }))
        .await;
    assert_eq!(res.status_code(), 400);

    // Missing account -> 404.
    let res = server
        .post("/accounts/999999999/users")
        .add_header("Authorization", auth(admin, "admin"))
        .json(&json!({
            "user": { "name": "Created Person" },
            "pseudonym": { "unique_id": email }
        }))
        .await;
    assert_eq!(res.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_admin_is_scoped_to_their_account() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let account_admin = seed_user(&pool, &prefix, "Scoped Admin", "student").await;
    let other_admin = seed_user(&pool, &prefix, "Other Scoped Admin", "student").await;
    let (account_a,): (i64,) =
        sqlx::query_as("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
            .bind(format!("{prefix} scoped A"))
            .fetch_one(&pool)
            .await
            .expect("seed account A");
    let (account_b,): (i64,) =
        sqlx::query_as("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
            .bind(format!("{prefix} scoped B"))
            .fetch_one(&pool)
            .await
            .expect("seed account B");
    for (account_id, user_id) in [(account_a, account_admin), (account_b, other_admin)] {
        sqlx::query(
            "INSERT INTO account_users (account_id, user_id, role) \
             VALUES ($1, $2, 'admin')",
        )
        .bind(account_id)
        .bind(user_id)
        .execute(&pool)
        .await
        .expect("seed account admin");
    }

    let body_a = json!({
        "user": { "name": "Scoped Created A" },
        "pseudonym": { "unique_id": format!("{prefix}-scoped-a@test.local") }
    });
    let res = server
        .post(&format!("/accounts/{account_a}/users"))
        .add_header("Authorization", auth(account_admin, "student"))
        .json(&body_a)
        .await;
    assert_eq!(res.status_code(), 200, "{}", res.text());

    let body_b = json!({
        "user": { "name": "Scoped Created B" },
        "pseudonym": { "unique_id": format!("{prefix}-scoped-b@test.local") }
    });
    let res = server
        .post(&format!("/accounts/{account_a}/users"))
        .add_header("Authorization", auth(other_admin, "student"))
        .json(&body_b)
        .await;
    assert_eq!(res.status_code(), 403, "{}", res.text());

    let res = server
        .post(&format!("/accounts/{account_b}/users"))
        .add_header("Authorization", auth(other_admin, "student"))
        .json(&body_b)
        .await;
    assert_eq!(res.status_code(), 200, "{}", res.text());

    cleanup(&pool, &prefix).await;
}
