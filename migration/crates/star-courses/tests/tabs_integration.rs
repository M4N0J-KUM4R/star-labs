//! Integration tests for the account, group and user navigation tabs and the
//! course tab update.
//!
//! Canvas `TabsController` (app/controllers/tabs_controller.rb):
//! - `index` renders the tabs the caller may see for the context; a caller
//!   without read rights gets 404.
//! - `update` reorders or hides one course tab; Home and Settings are pinned
//!   and cannot be managed, and an out-of-range position is rejected.
//!
//! Fleet rule: dedicated accounts, courses and users per test, every seeded
//! row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "tabs-{}-{}",
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
            .merge(star_courses::router())
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_account_user(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id) VALUES ($1, $2)")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account user");
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

async fn seed_teacher(pool: &PgPool, user_id: i64, course_id: i64) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'teacher')")
        .bind(user_id)
        .bind(course_id)
        .execute(pool)
        .await
        .expect("seed teacher");
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

async fn seed_membership(pool: &PgPool, group_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state) VALUES ($1, $2, 'accepted')",
    )
    .bind(group_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed membership");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM groups WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM course_tabs WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)")
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

fn tab_ids(body: &Value) -> Vec<String> {
    body.as_array()
        .expect("tabs array")
        .iter()
        .map(|tab| tab["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn account_tabs_are_visible_to_account_admins_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_user(&pool, account, admin).await;

    let resp = server
        .get(&format!("/accounts/{account}/tabs"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let ids = tab_ids(&body);
    assert!(ids.contains(&"courses".to_string()), "ids: {ids:?}");
    assert!(ids.contains(&"users".to_string()), "ids: {ids:?}");
    assert!(ids.contains(&"permissions".to_string()), "ids: {ids:?}");
    // The position is one-based and dense.
    assert_eq!(body[0]["position"], 1);
    assert_eq!(body[1]["position"], 2);
    // Account tabs are internal and public.
    assert_eq!(body[0]["type"], "internal");
    assert_eq!(body[0]["visibility"], "public");
    assert_eq!(body[0]["html_url"], format!("/accounts/{account}"));

    // A user without account rights may not read them.
    let resp = server
        .get(&format!("/accounts/{account}/tabs"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // A missing account is 404.
    let resp = server
        .get("/accounts/999999999/tabs")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_tabs_are_visible_to_members_and_teachers() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_user(&pool, account, admin).await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_teacher(&pool, teacher, course).await;
    let group = seed_group(&pool, course, "Group").await;
    seed_membership(&pool, group, member).await;

    let resp = server
        .get(&format!("/groups/{group}/tabs"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let ids = tab_ids(&resp.json());
    assert_eq!(
        ids,
        vec![
            "home",
            "announcements",
            "pages",
            "people",
            "discussions",
            "files",
            "conferences",
            "collaborations",
            "collaborations_new",
        ]
    );

    // A course teacher sees them too.
    let resp = server
        .get(&format!("/groups/{group}/tabs"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Somebody unrelated gets 404, as Canvas raises RecordNotFound.
    let resp = server
        .get(&format!("/groups/{group}/tabs"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn user_tabs_are_served_to_the_user_itself() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;

    let resp = server
        .get(&format!("/users/{user}/tabs"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        tab_ids(&body),
        vec!["notifications", "profile", "files", "profile_settings"]
    );
    assert_eq!(body[1]["html_url"], "/profile");

    // Another user may not read them.
    let resp = server
        .get(&format!("/users/{user}/tabs"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_tab_update_hides_reorders_and_rejects_pinned_tabs() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_teacher(&pool, teacher, course).await;

    // Hide a tab.
    let resp = server
        .put(&format!("/courses/{course}/tabs/announcements"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "hidden": "true" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], "announcements");
    assert_eq!(body["hidden"], true);
    assert_eq!(body["visibility"], "admins");

    // Show it again.  Canvas only renders `hidden` when the tab is hidden,
    // so the key is absent again once the tab is visible.
    let resp = server
        .put(&format!("/courses/{course}/tabs/announcements"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "hidden": "false" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body.get("hidden").is_none(), "body: {body}");
    assert_eq!(body["visibility"], "public");

    // Home is pinned and cannot be managed.
    let resp = server
        .put(&format!("/courses/{course}/tabs/home"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "hidden": "true" }))
        .await;
    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["error"], "home is not manageable");

    // An out-of-range position is rejected.
    let resp = server
        .put(&format!("/courses/{course}/tabs/pages"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&serde_json::json!({ "position": 999 }))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(
        resp.json::<Value>()["error"],
        "That tab location is invalid"
    );

    // A student may not manage the tabs.
    let resp = server
        .put(&format!("/courses/{course}/tabs/pages"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&serde_json::json!({ "hidden": "true" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    cleanup(&pool, &prefix).await;
}
