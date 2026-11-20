//! Integration tests for the Canvas account routes the parity report counted
//! as missing: the `AccountsController` extras, the deprecated user scoped
//! account notification aliases, the career experience routes and the error
//! report route.
//!
//! Pinned behavior:
//! - `/settings/environment` answers the whitelisted root account settings;
//! - `/course_accounts` and `/horizon_accounts` answer the accounts of the
//!   caller and stay empty for an anonymous caller;
//! - the bulk user routes answer a Canvas progress object and the restore
//!   route only works on the root account;
//! - the quiz IP filters come from `account.settings[:ip_filters]`;
//! - the accessibility summary answers 401 — not 403 — for a caller that may
//!   not see the accessibility tab;
//! - the user scoped notification aliases are 404 for another user;
//! - the career routes need a career affiliated root account;
//! - the error report route is public and the honeypot field answers 400 with
//!   an empty body.
//!
//! Fleet rule: dedicated users, accounts and courses per test, every seeded
//! row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "acc-p5-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
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

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(star_accounts_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn star_accounts_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_accounts::router())
        .with_state(state)
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role, sortable_name)
        VALUES ($1, $2, $3, $4, $1)
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

async fn seed_account(pool: &PgPool, name: &str, parent: Option<i64>) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO accounts (name, parent_account_id, workflow_state)
        VALUES ($1, $2, 'active')
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code, account_id)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
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

async fn seed_account_admin(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role, workflow_state) \
         VALUES ($1, $2, 'admin', 'active')",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account admin");
}

async fn seed_notification(pool: &PgPool, account_id: i64, subject: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO account_notifications
            (account_id, subject, message, start_at, end_at, workflow_state)
        VALUES ($1, $2, 'body', now(), now() + interval '1 day', 'active')
        RETURNING id
        "#,
    )
    .bind(account_id)
    .bind(subject)
    .fetch_one(pool)
    .await
    .expect("seed account notification")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM account_notifications WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM error_reports WHERE user_id IN \
           (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM user_preferences WHERE user_id IN \
           (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM canvas_career_user_experiences WHERE user_id IN \
           (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM accessibility_course_statistics WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
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
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
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
async fn the_environment_route_answers_the_root_account_settings() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;

    // The root account carries the public settings.
    let original: Value = sqlx::query_scalar(
        "SELECT settings FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("read the root account settings");
    sqlx::query(
        "UPDATE accounts SET settings = settings || jsonb_build_object( \
            'open_registration', true, 'calendar_contexts_limit', 3) \
         WHERE parent_account_id IS NULL",
    )
    .execute(&pool)
    .await
    .expect("seed the root account settings");

    let resp = server
        .get("/settings/environment")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["open_registration"], true);
    assert_eq!(body["calendar_contexts_limit"], 3);

    // An anonymous caller is a 401.
    let resp = server.get("/settings/environment").await;
    assert_eq!(resp.status_code(), 401);

    sqlx::query("UPDATE accounts SET settings = $1 WHERE parent_account_id IS NULL")
        .bind(original)
        .execute(&pool)
        .await
        .expect("restore the root account settings");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_course_and_horizon_account_lists_answer_the_accounts_of_the_caller() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_account_admin(&pool, account, teacher).await;

    let resp = server
        .get("/course_accounts")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let list = list.as_array().expect("array");
    assert!(list.iter().any(|item| item["id"] == account), "{list:?}");
    // The read only shape carries the default time zone.
    assert!(list[0].get("default_time_zone").is_some(), "{list:?}");

    // An anonymous caller gets an empty list, not a 401.
    let resp = server.get("/course_accounts").await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert!(resp.json::<Value>().as_array().expect("array").is_empty());

    let resp = server
        .get("/horizon_accounts")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let list = list.as_array().expect("array");
    assert!(list.iter().any(|item| item["id"] == account), "{list:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_bulk_user_routes_answer_a_progress_object() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    seed_account_admin(&pool, account, admin).await;
    seed_account_admin(&pool, account, member).await;

    // The bulk update suspends the membership.
    let resp = server
        .put(&format!("/accounts/{account}/users/bulk_update"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "user": { "event": "suspend" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let progress: Value = resp.json();
    assert_eq!(progress["tag"], "update_multiple_users");
    assert_eq!(progress["context_type"], "Account");

    // The remove route answers a progress object too.
    let resp = server
        .delete(&format!("/accounts/{account}/users?user_ids[]={member}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let progress: Value = resp.json();
    assert_eq!(progress["tag"], "remove_users_from_account");

    // The membership is gone.
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_users WHERE account_id = $1 AND user_id = $2",
    )
    .bind(account)
    .bind(member)
    .fetch_one(&pool)
    .await
    .expect("count the memberships");
    assert_eq!(left, 0);

    // A caller without the right is a 403.
    let resp = server
        .put(&format!("/accounts/{account}/users/bulk_update"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn only_the_root_account_restores_a_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    seed_account_admin(&pool, account, admin).await;

    // A sub-account may not restore a user.
    let resp = server
        .put(&format!("/accounts/{account}/users/{user}/restore"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The root account may.
    let root: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("read the root account");
    sqlx::query("UPDATE users SET workflow_state = 'deleted' WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .expect("mark the user deleted");

    let resp = server
        .put(&format!("/accounts/{root}/users/{user}/restore"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let restored: Value = resp.json();
    assert_eq!(restored["id"], user);
    assert!(restored.get("name").is_some(), "{restored}");

    let state: String = sqlx::query_scalar("SELECT workflow_state FROM users WHERE id = $1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .expect("read the user state");
    assert_eq!(state, "registered");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_quiz_ip_filters_come_from_the_account_settings() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    seed_account_admin(&pool, account, admin).await;
    sqlx::query(
        "UPDATE accounts SET settings = settings || jsonb_build_object( \
            'ip_filters', jsonb_build_object('Campus', '10.0.0.0/8')) \
         WHERE id = $1",
    )
    .bind(account)
    .execute(&pool)
    .await
    .expect("seed the ip filters");

    let resp = server
        .get(&format!("/accounts/{account}/quiz_ip_filters"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let filters = body["quiz_ip_filters"].as_array().expect("array");
    assert_eq!(filters.len(), 1, "{body}");
    assert_eq!(filters[0]["name"], "Campus");
    assert_eq!(filters[0]["filter"], "10.0.0.0/8");
    assert_eq!(filters[0]["account"], format!("{prefix}-Account"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_accessibility_summary_counts_the_course_statistics() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    seed_account_admin(&pool, account, admin).await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, student, course, "student").await;
    sqlx::query(
        "INSERT INTO accessibility_course_statistics \
         (course_id, active_issue_count, resolved_issue_count) VALUES ($1, 4, 2)",
    )
    .bind(course)
    .execute(&pool)
    .await
    .expect("seed the course statistics");

    let resp = server
        .get(&format!("/accounts/{account}/accessibility_issue_summary"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["active"], 4);
    assert_eq!(body["resolved"], 2);

    // Canvas answers 401 — not 403 — for a caller that may not see the tab.
    let resp = server
        .get(&format!("/accounts/{account}/accessibility_issue_summary"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_user_scoped_notification_aliases_are_scoped_to_the_caller() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let account = seed_account(&pool, &format!("{prefix}-Account"), None).await;
    seed_account_admin(&pool, account, user).await;
    let notification = seed_notification(&pool, account, "Maintenance").await;

    let resp = server
        .get(&format!(
            "/accounts/{account}/users/{user}/account_notifications"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let list = list.as_array().expect("array");
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["subject"], "Maintenance");

    // The show route serves the notification.
    let resp = server
        .get(&format!(
            "/accounts/{account}/users/{user}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["id"], notification);

    // Another user is a 404.
    let resp = server
        .get(&format!(
            "/accounts/{account}/users/{other}/account_notifications"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Closing the notification marks it closed for the caller.
    let resp = server
        .delete(&format!(
            "/accounts/{account}/users/{user}/account_notifications/{notification}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/accounts/{account}/users/{user}/account_notifications?show_is_closed=true"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let list = list.as_array().expect("array");
    assert_eq!(list[0]["closed"], true, "{list:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_career_routes_follow_the_root_account_flag() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;

    let original: Value = sqlx::query_scalar(
        "SELECT settings FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("read the root account settings");

    // Without a career affiliated root account the feature is off.
    let resp = server
        .get("/career/enabled")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["enabled"], false);

    let resp = server
        .post("/career/user_experiences")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // With the flag the experience routes work.
    sqlx::query(
        "UPDATE accounts SET settings = settings || jsonb_build_object( \
            'horizon_account_ids', '[1]'::jsonb, \
            'canvas_career_config', jsonb_build_object('learner_launch_url', 'http://career.test')) \
         WHERE parent_account_id IS NULL",
    )
    .execute(&pool)
    .await
    .expect("seed the career flag");

    // A career course with a learner enrollment is what makes the user a
    // career learner (Canvas `ExperienceResolver`).
    let root_account: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("read the root account");
    let course = seed_course(&pool, &prefix, "Career", root_account).await;
    sqlx::query("UPDATE courses SET horizon_course = true WHERE id = $1")
        .bind(course)
        .execute(&pool)
        .await
        .expect("mark the course as a career course");
    seed_enrollment(&pool, user, course, "student").await;

    let resp = server
        .get("/career/enabled")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["enabled"], true);

    let resp = server
        .get("/career/experience_summary")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let summary: Value = resp.json();
    assert_eq!(summary["current_app"], "career_learner");
    assert_eq!(summary["available_apps"], json!(["career_learner"]));

    // An invalid experience is the Canvas 400.
    let resp = server
        .post("/career/switch_experience")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "experience": "nonsense" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["error"], "invalid_experience");

    let resp = server
        .post("/career/switch_experience")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "experience": "career" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["experience"], "career");

    // An invalid role is the Canvas 400.
    let resp = server
        .post("/career/switch_role")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "role": "nonsense" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["error"], "invalid_role");

    let resp = server
        .post("/career/switch_role")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "role": "learner" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["role"], "learner");

    // The user experience is created and deleted again.
    let resp = server
        .post("/career/user_experiences")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "active");

    let resp = server
        .delete("/career/user_experiences")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "deleted");

    // Deleting it twice is a 404.
    let resp = server
        .delete("/career/user_experiences")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    sqlx::query("UPDATE accounts SET settings = $1 WHERE parent_account_id IS NULL")
        .bind(original)
        .execute(&pool)
        .await
        .expect("restore the root account settings");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_error_report_route_is_public_and_guards_the_honeypot() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;

    // The route is public: no token is needed.
    let resp = server
        .post("/error_reports")
        .json(&json!({ "error": { "subject": "things are broken" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["logged"], true);
    assert!(body["id"].as_i64().is_some(), "{body}");

    // The honeypot field answers 400 with an empty body.
    let resp = server
        .post("/error_reports")
        .json(&json!({ "error": { "subject": "x", "username": "bot" } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.text(), "");

    // An authenticated report is stored with its user.
    let resp = server
        .post("/error_reports")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "error": { "subject": "from a user", "url": "http://example.test" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM error_reports WHERE user_id = $1")
        .bind(user)
        .fetch_one(&pool)
        .await
        .expect("count the error reports");
    assert_eq!(stored, 1);

    cleanup(&pool, &prefix).await;
}
