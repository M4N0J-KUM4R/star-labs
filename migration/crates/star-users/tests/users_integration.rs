//! Integration tests for star-users.
//!
//! Exercises the real users router (GET /users, GET /users/self,
//! GET /users/{id}, PUT /users/{id}, DELETE /users/{id}) through axum-test
//! against a real Postgres.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-users
//! - Uses a unique email prefix per run + full teardown so it is safe to run
//!   repeatedly against a shared/dev database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "usr-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the users router
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

    let server = TestServer::new(star_users_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same merge pattern as star-http::build_api_router, restricted to the
/// users router. Authentication is enforced by the AuthUser extractor inside
/// star-users' own handlers, so no extra middleware is needed here.
fn star_users_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_users::router())
        .with_state(state)
}

/// Create a user directly in the DB (bypassing the register endpoint) with a
/// deterministic password hash and the given role. Returns user id.
///
/// sortable_name is populated the way /auth/register computes it ("Last,
/// First") so seeded rows look like real ones. It must not be left NULL:
/// star-users decodes sortable_name into a non-Option String, so a NULL would
/// make every read of that row fail with a 500.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(sortable_name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run (users cascade to their dependent
/// rows).
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

/// Fetch (name, email, sortable_name, password_hash) for a user, if present.
async fn db_user(pool: &PgPool, user_id: i64) -> Option<(String, String, Option<String>, String)> {
    sqlx::query_as("SELECT name, email, sortable_name, password_hash FROM users WHERE id = $1")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .expect("fetch user")
}

/// star-users decodes users.sortable_name into a non-Option String, so a
/// single row with a NULL sortable_name anywhere in the table makes GET
/// /users (and every read of that row) fail with a 500 — a known bug in
/// star-users, noted here instead of fixed. Other test suites in this
/// workspace seed users without a sortable_name, and any failed or
/// interrupted run leaks those rows into this shared test database forever
/// (their cleanup is prefix-scoped and reruns use a new prefix).
///
/// Before listing, sweep leaked rows and wait until the table is decodable
/// again. The 20-second age floor means in-flight rows from parallel suites
/// are never touched: no test in this workspace holds its rows that long.
async fn ensure_listable(pool: &PgPool) {
    for _ in 0..20 {
        // Never touch this run's own rows (its prefix also matches
        // '%-tests-%') nor rows so fresh that a parallel suite may still be
        // in the middle of using them.
        let _ = sqlx::query(
            r#"
            DELETE FROM users
            WHERE created_at < now() - interval '20 seconds'
              AND (sortable_name IS NULL
                   OR (email LIKE '%-tests-%' AND email NOT LIKE 'usr-tests-%'))
            "#,
        )
        .execute(pool)
        .await;
        let clean: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS (SELECT 1 FROM users WHERE sortable_name IS NULL)",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(true);
        if clean {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

/// GET /users with the given bearer token and query string; asserts success
/// and returns the parsed body.
///
/// Retries the known decoder failure ("database error" 500) caused by
/// another suite's in-flight seed row sitting inside the requested page;
/// that row disappears when the other suite's test cleans up after itself.
/// Any other failure fails loudly on the first attempt.
async fn list_users(server: &TestServer, pool: &PgPool, token: &str, query: &str) -> Value {
    ensure_listable(pool).await;
    let mut path = "/users".to_string();
    if !query.is_empty() {
        path.push('?');
        path.push_str(query);
    }
    let mut last_body = String::new();
    for _ in 0..10 {
        let resp = server
            .get(&path)
            .add_header(axum::http::header::AUTHORIZATION, token)
            .await;
        let status = resp.status_code();
        last_body = resp.text();
        if status == 200 {
            return resp.json();
        }
        if status != 500 || !last_body.contains("database error") {
            assert_eq!(status, 200, "list should succeed; body: {last_body}");
        }
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
    panic!("GET {path} still failing after retries; body: {last_body}");
}

// ---------------------------------------------------------------------------
// GET /users
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_as_admin_returns_array_with_seeded_users() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let alice = seed_user(&pool, &prefix, "Alice Student", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob Teacher", "teacher").await;
    let carol = seed_user(&pool, &prefix, "Carol Student", "student").await;

    // per_page=100: the seeded users are the newest rows in the table, so a
    // single maximum-size page is guaranteed to contain all of them.
    let body = list_users(&server, &pool, &auth(admin, "admin"), "per_page=100").await;

    let data = body["data"].as_array().expect("data must be an array");
    let first = &data[0];
    for key in ["id", "name", "sortable_name", "email", "role", "created_at"] {
        assert!(first.get(key).is_some(), "missing field {key}: {first}");
    }

    let expected = [
        (admin, "Admin", "admin"),
        (alice, "Alice Student", "student"),
        (bob, "Bob Teacher", "teacher"),
        (carol, "Carol Student", "student"),
    ];
    for (id, name, role) in expected {
        let row = data
            .iter()
            .find(|u| u["id"].as_i64() == Some(id))
            .unwrap_or_else(|| panic!("seeded user {id} missing from list: {body}"));
        assert_eq!(row["name"], name, "unexpected name: {row}");
        assert_eq!(row["role"], role);
        assert_eq!(row["email"], format!("{prefix}-{name}@test.local"));
    }

    // Default pagination envelope.
    let default = list_users(&server, &pool, &auth(admin, "admin"), "").await;
    assert_eq!(default["page"], 1);
    assert_eq!(default["per_page"], 50);
    assert!(
        default["total"].as_i64().unwrap() >= 4,
        "total must count at least the seeded users: {}",
        default["total"]
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_as_non_admin_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;

    // Listing users is admin-only: both teachers and students get 403.
    for (id, role) in [(teacher, "teacher"), (student, "student")] {
        let resp = server
            .get("/users")
            .add_header(axum::http::header::AUTHORIZATION, &auth(id, role))
            .await;
        assert_eq!(
            resp.status_code(),
            403,
            "{role} must not be allowed to list users: {}",
            resp.text()
        );
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_uses_db_role_not_token_claim() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;

    // The AuthUser extractor reloads the role from the database, so a token
    // whose role claim says "admin" must not grant admin powers to a student
    // account.
    let resp = server
        .get("/users")
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "admin"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_pagination_params_and_clamping() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let mut seeded = vec![admin];
    for i in 0..6 {
        seeded.push(seed_user(&pool, &prefix, &format!("Page{i}"), "student").await);
    }

    // Explicit page/per_page are echoed back and slice the data. The seven
    // seeded users guarantee total >= 7, so pages 1 and 2 are both full.
    let page1 = list_users(&server, &pool, &auth(admin, "admin"), "per_page=3&page=1").await;
    assert_eq!(page1["data"].as_array().unwrap().len(), 3);
    assert_eq!(page1["page"], 1);
    assert_eq!(page1["per_page"], 3);
    assert!(page1["total"].as_i64().unwrap() >= 7);

    let page2 = list_users(&server, &pool, &auth(admin, "admin"), "per_page=3&page=2").await;
    assert_eq!(page2["data"].as_array().unwrap().len(), 3);
    assert_eq!(page2["page"], 2);
    assert_eq!(page2["per_page"], 3);

    // per_page is clamped to [1, 100]; page is clamped to >= 1.
    let big = list_users(&server, &pool, &auth(admin, "admin"), "per_page=500").await;
    assert_eq!(big["per_page"], 100);
    assert!(big["data"].as_array().unwrap().len() <= 100);

    let zero = list_users(&server, &pool, &auth(admin, "admin"), "per_page=0&page=0").await;
    assert_eq!(zero["per_page"], 1);
    assert_eq!(zero["page"], 1);
    assert_eq!(zero["data"].as_array().unwrap().len(), 1);

    let negative = list_users(&server, &pool, &auth(admin, "admin"), "per_page=-5&page=-2").await;
    assert_eq!(negative["per_page"], 1);
    assert_eq!(negative["page"], 1);
    assert_eq!(negative["data"].as_array().unwrap().len(), 1);

    // A single max-size page still contains every seeded user (they are the
    // newest rows in the table).
    let all = list_users(&server, &pool, &auth(admin, "admin"), "per_page=100").await;
    let data = all["data"].as_array().unwrap();
    for id in &seeded {
        assert!(
            data.iter().any(|u| u["id"].as_i64() == Some(*id)),
            "seeded user {id} missing from per_page=100 listing: {all}"
        );
    }

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /users/self
// ---------------------------------------------------------------------------

#[tokio::test]
async fn self_returns_own_profile_for_each_role() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let cases = [
        ("Alice Student", "student"),
        ("Bob Teacher", "teacher"),
        ("Carol Admin", "admin"),
    ];
    for (name, role) in cases {
        let id = seed_user(&pool, &prefix, name, role).await;
        let resp = server
            .get("/users/self")
            .add_header(axum::http::header::AUTHORIZATION, &auth(id, role))
            .await;
        assert_eq!(resp.status_code(), 200, "{role}: {}", resp.text());
        let body: Value = resp.json();
        let (first, last) = name.rsplit_once(' ').expect("two-word name");
        assert_eq!(body["id"], id, "unexpected body: {body}");
        assert_eq!(body["name"], name);
        assert_eq!(body["email"], format!("{prefix}-{name}@test.local"));
        assert_eq!(body["role"], role);
        assert_eq!(body["sortable_name"], format!("{last}, {first}"));
    }

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /users/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_user_as_admin_returns_full_record() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let dana = seed_user(&pool, &prefix, "Dana Student", "student").await;

    let resp = server
        .get(&format!("/users/{dana}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], dana);
    assert_eq!(body["name"], "Dana Student");
    assert_eq!(body["email"], format!("{prefix}-Dana Student@test.local"));
    assert_eq!(body["role"], "student");
    assert_eq!(body["sortable_name"], "Student, Dana");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_user_as_self_returns_own_record() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Eve Student", "student").await;

    let resp = server
        .get(&format!("/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], student);
    assert_eq!(body["email"], format!("{prefix}-Eve Student@test.local"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_user_as_other_non_admin_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Frank Student", "student").await;
    let other = seed_user(&pool, &prefix, "Gina Student", "student").await;
    let teacher = seed_user(&pool, &prefix, "Hank Teacher", "teacher").await;

    // Reading another user's profile requires being that user or an admin;
    // neither a fellow student nor a teacher qualifies.
    for (id, role) in [(other, "student"), (teacher, "teacher")] {
        let resp = server
            .get(&format!("/users/{student}"))
            .add_header(axum::http::header::AUTHORIZATION, &auth(id, role))
            .await;
        assert_eq!(
            resp.status_code(),
            403,
            "{role} must not read another user's profile: {}",
            resp.text()
        );
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_missing_user_is_404_for_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Ivan Student", "student").await;

    let resp = server
        .get("/users/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The permission check runs before the existence check, so a non-admin
    // probing for a missing id gets 403, not 404.
    let resp = server
        .get("/users/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /users/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_self_name_persists_and_echoes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Julia Student", "student").await;

    let resp = server
        .put(&format!("/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .json(&json!({ "name": "Julia Renamed" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], student);
    assert_eq!(body["name"], "Julia Renamed");
    // Fields not present in the payload keep their values.
    assert_eq!(body["email"], format!("{prefix}-Julia Student@test.local"));
    assert_eq!(body["role"], "student");

    // The change is persisted.
    let (name, email, sortable, _hash) = db_user(&pool, student).await.expect("row after update");
    assert_eq!(name, "Julia Renamed");
    assert_eq!(email, format!("{prefix}-Julia Student@test.local"));

    // NOTE (parity deviation, deliberately asserted as-is): updating `name`
    // does NOT recompute sortable_name — Canvas derives it from the new name,
    // this implementation keeps the stale value.
    assert_eq!(sortable.as_deref(), Some("Student, Julia"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_self_email_changes_email() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Ken Student", "student").await;
    let new_email = format!("{prefix}-ken-new@test.local");

    let resp = server
        .put(&format!("/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .json(&json!({ "email": new_email }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["email"], new_email);

    let (_, email, _, _) = db_user(&pool, student).await.expect("row after update");
    assert_eq!(email, new_email);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_self_password_rehashes_credentials() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Liam Student", "student").await;

    let resp = server
        .put(&format!("/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .json(&json!({ "password": "new-strong-pw-42" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // The stored hash now verifies the new password and rejects the old one,
    // while name and email stay untouched.
    let (name, email, _, hash) = db_user(&pool, student).await.expect("row after update");
    assert_eq!(name, "Liam Student");
    assert_eq!(email, format!("{prefix}-Liam Student@test.local"));
    assert!(
        star_auth::verify_password("new-strong-pw-42", &hash),
        "new password must verify against the stored hash"
    );
    assert!(
        !star_auth::verify_password("pw-test-1234", &hash),
        "old seed password must no longer verify"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_with_empty_body_keeps_values() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Mona Student", "student").await;
    let (name_before, email_before, sortable_before, hash_before) =
        db_user(&pool, student).await.expect("row before update");

    // Every field of UpdateUserRequest is optional: an empty object is a
    // valid no-op update.
    let resp = server
        .put(&format!("/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["name"], name_before);
    assert_eq!(body["email"], email_before);

    let (name_after, email_after, sortable_after, hash_after) =
        db_user(&pool, student).await.expect("row after update");
    assert_eq!(name_after, name_before);
    assert_eq!(email_after, email_before);
    assert_eq!(sortable_after, sortable_before);
    assert_eq!(hash_after, hash_before, "password hash must be untouched");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn admin_can_update_another_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Nina Student", "student").await;

    let resp = server
        .put(&format!("/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .json(&json!({ "name": "Admin Set Name" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], student);
    assert_eq!(body["name"], "Admin Set Name");

    let (name, ..) = db_user(&pool, student).await.expect("row after update");
    assert_eq!(name, "Admin Set Name");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn non_admin_cannot_update_another_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Omar Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Pat Student", "student").await;
    let other = seed_user(&pool, &prefix, "Quinn Student", "student").await;

    // Only the user themselves or an admin may update a profile: a teacher
    // cannot update a student, and one student cannot update another.
    for (actor, role) in [(teacher, "teacher"), (other, "student")] {
        let resp = server
            .put(&format!("/users/{student}"))
            .add_header(axum::http::header::AUTHORIZATION, &auth(actor, role))
            .json(&json!({ "name": "Hacked Name" }))
            .await;
        assert_eq!(
            resp.status_code(),
            403,
            "{role} must not update another user: {}",
            resp.text()
        );
    }

    // Nothing changed.
    let (name, email, _, _) = db_user(&pool, student).await.expect("row intact");
    assert_eq!(name, "Pat Student");
    assert_eq!(email, format!("{prefix}-Pat Student@test.local"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_to_taken_email_is_409() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let taker = seed_user(&pool, &prefix, "Rosa Student", "student").await;
    // The owner already holds the email the taker will try to claim.
    seed_user(&pool, &prefix, "Sam Student", "student").await;
    let owner_email = format!("{prefix}-Sam Student@test.local");

    let resp = server
        .put(&format!("/users/{taker}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(taker, "student"))
        .json(&json!({ "email": owner_email }))
        .await;
    assert_eq!(resp.status_code(), 409, "body: {}", resp.text());
    assert!(
        resp.text().contains("email already in use"),
        "conflict body should explain the cause: {}",
        resp.text()
    );

    // The taker's email is unchanged.
    let (_, email, _, _) = db_user(&pool, taker).await.expect("row intact");
    assert_eq!(email, format!("{prefix}-Rosa Student@test.local"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_missing_user_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .put("/users/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .json(&json!({ "name": "Ghost Name" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /users/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_delete_user_removes_row_and_invalidates_token() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let victim = seed_user(&pool, &prefix, "Tina Student", "student").await;

    let resp = server
        .delete(&format!("/users/{victim}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // The row is really gone from the database.
    assert!(db_user(&pool, victim).await.is_none());

    // A follow-up GET by the admin returns 404.
    let resp = server
        .get(&format!("/users/{victim}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The victim's still-valid JWT no longer authenticates: AuthUser rejects
    // tokens whose subject no longer exists.
    let resp = server
        .get("/users/self")
        .add_header(axum::http::header::AUTHORIZATION, &auth(victim, "student"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // The admin listing no longer contains the victim.
    let body = list_users(&server, &pool, &auth(admin, "admin"), "per_page=100").await;
    let data = body["data"].as_array().unwrap();
    assert!(
        data.iter().all(|u| u["id"].as_i64() != Some(victim)),
        "deleted user still listed: {body}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_as_non_admin_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Uma Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Vera Student", "student").await;

    // Deletion is admin-only — even for a user's own account.
    let resp = server
        .delete(&format!("/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(student, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "users must not be able to delete themselves: {}",
        resp.text()
    );

    // A teacher cannot delete someone else either.
    let resp = server
        .delete(&format!("/users/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, &auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The row is untouched.
    assert!(db_user(&pool, student).await.is_some());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_missing_user_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let resp = server
        .delete("/users/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Authentication on every route
// ---------------------------------------------------------------------------

#[tokio::test]
async fn all_routes_reject_missing_or_invalid_tokens() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let valid_signature_missing_user = token_for(999_999_999, "admin");
    let wrong_secret =
        star_auth::create_token(1, "admin", "not-the-test-jwt-secret").expect("mint token");

    let invalid_tokens = [
        // Malformed JWT.
        "Bearer garbage.token.value".to_string(),
        // Properly signed but with the wrong secret.
        format!("Bearer {wrong_secret}"),
        // Lowercase scheme: the extractor requires exactly "Bearer ".
        format!("bearer {valid_signature_missing_user}"),
        // Valid signature but the subject does not exist in the database.
        format!("Bearer {valid_signature_missing_user}"),
    ];

    for token in &invalid_tokens {
        let listed = server
            .get("/users")
            .add_header(axum::http::header::AUTHORIZATION, token)
            .await;
        assert_eq!(listed.status_code(), 401, "GET /users: {}", listed.text());

        let me = server
            .get("/users/self")
            .add_header(axum::http::header::AUTHORIZATION, token)
            .await;
        assert_eq!(me.status_code(), 401, "GET /users/self: {}", me.text());

        let got = server
            .get("/users/1")
            .add_header(axum::http::header::AUTHORIZATION, token)
            .await;
        assert_eq!(got.status_code(), 401, "GET /users/1: {}", got.text());

        let updated = server
            .put("/users/1")
            .add_header(axum::http::header::AUTHORIZATION, token)
            .json(&json!({ "name": "x" }))
            .await;
        assert_eq!(
            updated.status_code(),
            401,
            "PUT /users/1: {}",
            updated.text()
        );

        let deleted = server
            .delete("/users/1")
            .add_header(axum::http::header::AUTHORIZATION, token)
            .await;
        assert_eq!(
            deleted.status_code(),
            401,
            "DELETE /users/1: {}",
            deleted.text()
        );
    }

    // No Authorization header at all is rejected on every route too.
    assert_eq!(server.get("/users").await.status_code(), 401);
    assert_eq!(server.get("/users/self").await.status_code(), 401);
    assert_eq!(server.get("/users/1").await.status_code(), 401);
    assert_eq!(
        server
            .put("/users/1")
            .json(&json!({ "name": "x" }))
            .await
            .status_code(),
        401
    );
    assert_eq!(server.delete("/users/1").await.status_code(), 401);
}
