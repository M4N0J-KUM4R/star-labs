//! Integration tests for the Canvas content share and block editor template
//! routes.
//!
//! Pinned behavior:
//! - a content share exports the shared content as a common cartridge, stores
//!   one row for the sender (`read`) and one row per receiver (`unread`);
//! - the sender's share lists the receivers, the received share does not;
//! - `unread_count` counts the unread received shares;
//! - another user's share is a 404 and the modify routes are a 403;
//! - the block editor templates need the `block_editor` and
//!   `block_template_editor` features of the account of the caller, and the
//!   templates of a course are only visible to a template editor.
//!
//! Fleet rule: dedicated users, accounts, courses and assignments per test,
//! every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is
//! unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "cs-tpl-{}-{}-{}",
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
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_content::router())
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

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, $2, 10) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
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

async fn enable_feature(pool: &PgPool, account_id: i64, feature: &str) {
    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state) \
         VALUES ('Account', $1, $2, $2, 'enabled') \
         ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'enabled'",
    )
    .bind(account_id)
    .bind(feature)
    .execute(pool)
    .await
    .expect("enable the feature");
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM content_shares WHERE name IN \
           (SELECT title FROM assignments WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM content_exports WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM block_editor_templates WHERE context_type = 'Course' AND context_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM assignments WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Account' AND context_id IN \
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
async fn a_content_share_is_created_for_the_sender_and_the_receivers() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let sender = seed_user(&pool, &prefix, "Sender", "student").await;
    let receiver = seed_user(&pool, &prefix, "Receiver", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, sender, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Shared assignment").await;

    // The share is created for the sender and the receiver.
    let resp = server
        .post(&format!("/users/{sender}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(sender, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": assignment.to_string(),
            "receiver_ids": [receiver.to_string()],
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let share: Value = resp.json();
    assert_eq!(share["read_state"], "read");
    assert_eq!(share["name"], "Shared assignment");
    let receivers = share["receivers"].as_array().expect("receivers");
    assert_eq!(receivers.len(), 1);
    assert_eq!(receivers[0]["id"], receiver);
    let share_id = share["id"].as_i64().expect("share id");

    // The sender lists the share.
    let resp = server
        .get(&format!("/users/{sender}/content_shares/sent"))
        .add_header(axum::http::header::AUTHORIZATION, auth(sender, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(|items| items.len()), Some(1));

    // The receiver has an unread share.
    let resp = server
        .get(&format!("/users/{receiver}/content_shares/received"))
        .add_header(axum::http::header::AUTHORIZATION, auth(receiver, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let list = list.as_array().expect("array");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["read_state"], "unread");
    // A received share has no receivers.
    assert!(list[0]["receivers"]
        .as_array()
        .expect("receivers")
        .is_empty());

    // The unread count of the receiver.
    let resp = server
        .get(&format!("/users/{receiver}/content_shares/unread_count"))
        .add_header(axum::http::header::AUTHORIZATION, auth(receiver, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["unread_count"], 1);

    // Marking the share read lowers the count.
    let resp = server
        .put(&format!("/users/{receiver}/content_shares/{share_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(receiver, "student"))
        .json(&json!({ "read_state": "read" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["read_state"], "read");

    let resp = server
        .get(&format!("/users/{receiver}/content_shares/unread_count"))
        .add_header(axum::http::header::AUTHORIZATION, auth(receiver, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["unread_count"], 0);

    // Another user may not touch the share.
    let resp = server
        .put(&format!("/users/{sender}/content_shares/{share_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "read_state": "read" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The sender deletes the share.
    let resp = server
        .delete(&format!("/users/{sender}/content_shares/{share_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(sender, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["message"], "content share deleted");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_content_share_validates_its_content_type() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "User", "student").await;
    let receiver = seed_user(&pool, &prefix, "Receiver", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_course(&pool, &prefix, "Course", account).await;

    // An unknown content type is the Canvas 400.
    let resp = server
        .post(&format!("/users/{user}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "content_type": "nonsense",
            "content_id": "1",
            "receiver_ids": [receiver.to_string()],
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert!(resp.text().contains("Content type not allowed"));

    // A missing content id is the Canvas 400.
    let resp = server
        .post(&format!("/users/{user}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "content_type": "assignment",
            "receiver_ids": [receiver.to_string()],
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "Content type and id required"
    );

    // Content that does not exist is the Canvas 400.
    let resp = server
        .post(&format!("/users/{user}/content_shares"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "content_type": "assignment",
            "content_id": "999999999",
            "receiver_ids": [receiver.to_string()],
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "Requested share content not found"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_block_editor_templates_need_the_account_features() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_account_user(&pool, account, teacher).await;
    seed_account_user(&pool, account, student).await;

    // Without the feature the index is a 403.
    let resp = server
        .get(&format!("/courses/{course}/block_editor_templates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The features of the account of the caller open the templates.
    enable_feature(&pool, account, "block_editor").await;
    enable_feature(&pool, account, "block_template_editor").await;

    let resp = server
        .get(&format!("/courses/{course}/block_editor_templates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert!(resp.json::<Value>().as_array().expect("array").is_empty());

    // The create route needs a template editor.
    let resp = server
        .post(&format!("/courses/{course}/block_editor_templates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "name": "Student template" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "Cannot create block templates."
    );

    // The teacher creates a template.
    let resp = server
        .post(&format!("/courses/{course}/block_editor_templates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Navigation", "node_tree": { "a": 1 } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let template: Value = resp.json();
    assert_eq!(template["name"], "Navigation");
    assert_eq!(template["workflow_state"], "unpublished");
    assert_eq!(template["template_type"], "page");
    assert_eq!(template["editor_version"], "0.2");
    let template_id = template["id"].as_i64().expect("template id");

    // The index lists it.
    let resp = server
        .get(&format!("/courses/{course}/block_editor_templates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(|items| items.len()), Some(1));

    // The update route changes the name.
    let resp = server
        .put(&format!(
            "/courses/{course}/block_editor_templates/{template_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Navigation v2" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["name"], "Navigation v2");

    // The publish route activates the template.
    let resp = server
        .post(&format!(
            "/courses/{course}/block_editor_templates/{template_id}/publish"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "active");

    // The editor flags.
    let resp = server
        .get(&format!(
            "/courses/{course}/block_editor_templates/can_edit"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let flags: Value = resp.json();
    assert_eq!(flags["can_edit"], true);
    assert_eq!(flags["can_edit_global"], true);

    // The destroy route removes the template.
    let resp = server
        .delete(&format!(
            "/courses/{course}/block_editor_templates/{template_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["id"], template_id);

    cleanup(&pool, &prefix).await;
}
