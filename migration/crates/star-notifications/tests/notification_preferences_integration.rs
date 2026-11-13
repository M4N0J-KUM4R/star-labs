//! Integration tests for star-notifications preference routes (scoped forms).
//!
//! Canvas API v1 parity contract (notification_preferences_controller.rb,
//! config/routes.rb:2066–2076):
//! - GET /users/{userId}/communication_channels/{communicationChannelId}/notification_preferences
//! - GET /users/{userId}/communication_channels/{communicationChannelId}/notification_preferences/{notification}
//! - GET /users/{userId}/communication_channels/{type}/{address}/notification_preferences
//! - GET /users/{userId}/communication_channels/{type}/{address}/notification_preferences/{notification}
//! - GET /users/{userId}/notification_preference_categories
//! - PUT /users/{userId}/communication_channels/{type}/{address}/notification_preferences
//! - PUT /users/{userId}/communication_channels/{type}/{address}/notification_preferences/{notification}
//! - PUT /users/{userId}/notification_preference_categories/{category}
//!
//! Permission semantics: GETs self-or-admin; updates SELF only (Canvas
//! render_unauthorized_action). Channel-address forms resolve the channel by
//! (lower(type), lower(address)) among the user's unretired channels; unknown
//! channel/user → 404.
//!
//! These tests pin actual handler behavior without relying on implementation
//! internals. Tests are skipped cleanly when DATABASE_URL is not set.

#![allow(dead_code)]

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "notifpref_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer. Returns None when
/// DATABASE_URL is unset so callers can skip cleanly.
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

    let server = TestServer::new(
        axum::Router::new()
            .nest("/api/v1", star_notifications::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

/// Create a user directly in the DB. Returns user id.
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
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed_user")
}

/// Create a communication channel directly in the DB. Returns channel id.
async fn seed_channel(
    pool: &PgPool,
    user_id: i64,
    address: &str,
    channel_type: &str,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO communication_channels (user_id, path, path_type, workflow_state)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(address)
    .bind(channel_type)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed_channel")
}

/// Insert a stored notification preference directly in the DB.
async fn seed_preference(
    pool: &PgPool,
    user_id: i64,
    channel_id: i64,
    notification: &str,
    category: &str,
    frequency: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO notification_preferences
            (user_id, communication_channel_id, notification, category, frequency)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (communication_channel_id, notification)
        DO UPDATE SET frequency = EXCLUDED.frequency, updated_at = NOW()
        "#,
    )
    .bind(user_id)
    .bind(channel_id)
    .bind(notification)
    .bind(category)
    .bind(frequency)
    .execute(pool)
    .await
    .expect("seed_preference");
}

/// Mint a valid bearer token for the given user.
fn auth_header(_pool: &PgPool, user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Delete every row created for the given users (preferences and channels
/// cascade on user deletion, but stay explicit like the sibling test file).
async fn teardown(pool: &PgPool, user_ids: &[i64]) {
    for user_id in user_ids {
        let _ = sqlx::query("DELETE FROM notification_preferences WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM communication_channels WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
    }
}

// ---------------------------------------------------------------------------
// Tests — GET .../communication_channels/{communicationChannelId}/notification_preferences
// ---------------------------------------------------------------------------

#[tokio::test]
async fn channel_preference_index_scoped_requires_auth() {
    let Some((server, _, _)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server
        .get("/api/v1/users/self/communication_channels/1/notification_preferences")
        .await;
    assert_eq!(resp.status_code(), 401, "unauthenticated request");
}

#[tokio::test]
async fn channel_preference_index_scoped_returns_wrapped_preferences() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "idx-scoped", "student").await;
    let channel_id = seed_channel(
        &pool,
        user_id,
        &format!("{prefix}-idx@example.edu"),
        "email",
        "active",
    )
    .await;
    seed_preference(
        &pool,
        user_id,
        channel_id,
        "new_announcement",
        "announcement",
        "daily",
    )
    .await;
    seed_preference(
        &pool,
        user_id,
        channel_id,
        "new_discussion_entry",
        "discussion",
        "weekly",
    )
    .await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get(&format!(
            "/api/v1/users/self/communication_channels/{channel_id}/notification_preferences"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "self index by channel id: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    let prefs = body["notification_preferences"]
        .as_array()
        .expect("response must wrap preferences in notification_preferences");
    assert_eq!(prefs.len(), 2, "both stored preferences returned");
    // Ordered by notification ASC.
    assert_eq!(prefs[0]["notification"], "new_announcement");
    assert_eq!(prefs[0]["category"], "announcement");
    assert_eq!(prefs[0]["frequency"], "daily");
    assert_eq!(prefs[1]["notification"], "new_discussion_entry");
    assert_eq!(prefs[1]["frequency"], "weekly");
    // Canvas serializes exactly notification/category/frequency.
    assert!(prefs[0].get("id").is_none(), "no id field in Canvas shape");

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn channel_preference_index_scoped_admin_can_read_other_user() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin_id = seed_user(&pool, &prefix, "idx-admin", "admin").await;
    let target_id = seed_user(&pool, &prefix, "idx-target", "student").await;
    let channel_id = seed_channel(
        &pool,
        target_id,
        &format!("{prefix}-other@example.edu"),
        "email",
        "active",
    )
    .await;
    seed_preference(
        &pool,
        target_id,
        channel_id,
        "new_announcement",
        "announcement",
        "daily",
    )
    .await;

    let token = auth_header(&pool, admin_id, "admin");
    let resp = server
        .get(&format!(
            "/api/v1/users/{target_id}/communication_channels/{channel_id}/notification_preferences"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "admin reading other user's preferences: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    assert_eq!(
        body["notification_preferences"][0]["frequency"], "daily",
        "target preference must be visible to admin"
    );

    teardown(&pool, &[admin_id, target_id]).await;
}

#[tokio::test]
async fn channel_preference_index_scoped_student_cannot_read_other_user() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student_a = seed_user(&pool, &prefix, "idx-student-a", "student").await;
    let student_b = seed_user(&pool, &prefix, "idx-student-b", "student").await;
    let channel_id = seed_channel(
        &pool,
        student_b,
        &format!("{prefix}-priv@example.edu"),
        "email",
        "active",
    )
    .await;

    let token = auth_header(&pool, student_a, "student");
    let resp = server
        .get(&format!(
            "/api/v1/users/{student_b}/communication_channels/{channel_id}/notification_preferences"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        403,
        "student reading another user's preferences must be 403: {:#?}",
        resp.text()
    );

    teardown(&pool, &[student_a, student_b]).await;
}

#[tokio::test]
async fn channel_preference_scoped_unknown_user_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "idx-unk-user", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .get("/api/v1/users/99999999/communication_channels/1/notification_preferences")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;
    assert_eq!(resp.status_code(), 404, "unknown user must be 404");

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn channel_preference_scoped_unknown_channel_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "idx-unk-cc", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .get("/api/v1/users/self/communication_channels/999999/notification_preferences")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown channel must be 404: {:#?}",
        resp.text()
    );

    teardown(&pool, &[user_id]).await;
}

// ---------------------------------------------------------------------------
// Tests — GET .../notification_preferences/{notification} (channel id form)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn channel_preference_show_scoped_returns_stored_preference() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "show-scoped", "student").await;
    let channel_id = seed_channel(
        &pool,
        user_id,
        &format!("{prefix}-show@example.edu"),
        "email",
        "active",
    )
    .await;
    seed_preference(
        &pool,
        user_id,
        channel_id,
        "new_announcement",
        "announcement",
        "daily",
    )
    .await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get(&format!(
            "/api/v1/users/self/communication_channels/{channel_id}/notification_preferences/new_announcement"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "show stored preference: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    let prefs = body["notification_preferences"]
        .as_array()
        .expect("show wraps a one-element notification_preferences array");
    assert_eq!(prefs.len(), 1);
    assert_eq!(prefs[0]["notification"], "new_announcement");
    assert_eq!(prefs[0]["category"], "announcement");
    assert_eq!(prefs[0]["frequency"], "daily");

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn channel_preference_show_scoped_defaults_to_never_without_persisting() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "show-default", "student").await;
    let channel_id = seed_channel(
        &pool,
        user_id,
        &format!("{prefix}-never@example.edu"),
        "email",
        "active",
    )
    .await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get(&format!(
            "/api/v1/users/self/communication_channels/{channel_id}/notification_preferences/new_discussion_topic"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "show with no stored row: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    let prefs = &body["notification_preferences"];
    assert_eq!(prefs.as_array().unwrap().len(), 1);
    assert_eq!(prefs[0]["notification"], "new_discussion_topic");
    assert_eq!(prefs[0]["category"], "discussion");
    assert_eq!(prefs[0]["frequency"], "never", "Canvas defaults to never");

    // The "never" default must NOT be persisted by a read.
    let stored: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification_preferences WHERE communication_channel_id = $1",
    )
    .bind(channel_id)
    .fetch_one(&pool)
    .await
    .expect("count stored preferences");
    assert_eq!(stored, 0, "GET show must not write a preference row");

    teardown(&pool, &[user_id]).await;
}

// ---------------------------------------------------------------------------
// Tests — channel-address form (GET .../communication_channels/{type}/{address}/...)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn channel_preference_index_by_address_returns_wrapped_preferences() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let address = format!("{prefix}-addr@example.edu");
    let user_id = seed_user(&pool, &prefix, "idx-addr", "student").await;
    let channel_id = seed_channel(&pool, user_id, &address, "email", "active").await;
    seed_preference(
        &pool,
        user_id,
        channel_id,
        "new_announcement",
        "announcement",
        "immediately",
    )
    .await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get(&format!(
            "/api/v1/users/self/communication_channels/email/{address}/notification_preferences"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "index by address: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    let prefs = body["notification_preferences"]
        .as_array()
        .expect("address form must use the same envelope");
    assert_eq!(prefs.len(), 1);
    assert_eq!(prefs[0]["notification"], "new_announcement");
    assert_eq!(prefs[0]["frequency"], "immediately");

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn channel_preference_by_address_matches_type_and_address_case_insensitively() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let address = format!("{prefix}-case@example.edu");
    let user_id = seed_user(&pool, &prefix, "idx-case", "student").await;
    let channel_id = seed_channel(&pool, user_id, &address, "email", "active").await;
    seed_preference(
        &pool,
        user_id,
        channel_id,
        "new_announcement",
        "announcement",
        "daily",
    )
    .await;

    let token = auth_header(&pool, user_id, "student");
    // Uppercase type and address still resolve the same channel.
    let upper = address.to_uppercase();
    let resp = server
        .get(&format!(
            "/api/v1/users/self/communication_channels/EMAIL/{upper}/notification_preferences"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "case-insensitive (type, address) match: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    assert_eq!(
        body["notification_preferences"][0]["frequency"], "daily",
        "resolved channel must be the seeded one"
    );

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn channel_preference_index_by_address_unknown_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "idx-unk-addr", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .get("/api/v1/users/self/communication_channels/email/nobody@example.edu/notification_preferences")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown address must be 404: {:#?}",
        resp.text()
    );

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn channel_preference_by_address_retired_channel_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let address = format!("{prefix}-retired@example.edu");
    let user_id = seed_user(&pool, &prefix, "idx-retired", "student").await;
    seed_channel(&pool, user_id, &address, "email", "retired").await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get(&format!(
            "/api/v1/users/self/communication_channels/email/{address}/notification_preferences"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "retired channel must not resolve: {:#?}",
        resp.text()
    );

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn channel_preference_show_by_address_returns_stored_preference() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let address = format!("{prefix}-show-addr@example.edu");
    let user_id = seed_user(&pool, &prefix, "show-addr", "student").await;
    let channel_id = seed_channel(&pool, user_id, &address, "email", "active").await;
    seed_preference(
        &pool,
        user_id,
        channel_id,
        "new_announcement",
        "announcement",
        "weekly",
    )
    .await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get(&format!(
            "/api/v1/users/self/communication_channels/email/{address}/notification_preferences/new_announcement"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "show by address: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    assert_eq!(
        body["notification_preferences"][0]["frequency"], "weekly",
        "stored frequency must be returned"
    );

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn channel_preference_show_by_address_defaults_to_never() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let address = format!("{prefix}-never-addr@example.edu");
    let user_id = seed_user(&pool, &prefix, "show-never-addr", "student").await;
    seed_channel(&pool, user_id, &address, "email", "active").await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get(&format!(
            "/api/v1/users/self/communication_channels/email/{address}/notification_preferences/grade_change"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json::<Value>();
    assert_eq!(
        body["notification_preferences"][0]["notification"],
        "grade_change"
    );
    assert_eq!(body["notification_preferences"][0]["category"], "grading");
    assert_eq!(
        body["notification_preferences"][0]["frequency"], "never",
        "no stored row defaults to never"
    );

    teardown(&pool, &[user_id]).await;
}

// ---------------------------------------------------------------------------
// Tests — GET /users/{userId}/notification_preference_categories
// ---------------------------------------------------------------------------

#[tokio::test]
async fn notification_preference_categories_list_returns_canvas_envelope() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "cats", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .get("/api/v1/users/self/notification_preference_categories")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "categories list: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    let categories = body["categories"]
        .as_array()
        .expect("response must wrap slugs in categories");
    let slugs: Vec<&str> = categories
        .iter()
        .map(|slug| slug.as_str().unwrap())
        .collect();
    assert_eq!(
        slugs,
        vec![
            "announcement",
            "discussion",
            "conversation",
            "grading",
            "due_date",
            "calendar",
            "registration",
            "administrative",
            "other",
        ],
        "all derive_category slugs, uncompressed and unique"
    );

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn notification_preference_categories_requires_auth_and_known_user() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server
        .get("/api/v1/users/self/notification_preference_categories")
        .await;
    assert_eq!(resp.status_code(), 401, "unauthenticated request");

    let user_id = seed_user(&pool, &prefix, "cats-unk", "student").await;
    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .get("/api/v1/users/99999999/notification_preference_categories")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .await;
    assert_eq!(resp.status_code(), 404, "unknown user must be 404");

    teardown(&pool, &[user_id]).await;
}

// ---------------------------------------------------------------------------
// Tests — PUT .../communication_channels/{type}/{address}/notification_preferences
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_channel_preferences_by_address_update_all() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let address = format!("{prefix}-update-all@example.edu");
    let user_id = seed_user(&pool, &prefix, "upd-all", "student").await;
    let channel_id = seed_channel(&pool, user_id, &address, "email", "active").await;
    seed_preference(
        &pool,
        user_id,
        channel_id,
        "new_announcement",
        "announcement",
        "daily",
    )
    .await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .put(&format!(
            "/api/v1/users/self/communication_channels/email/{address}/notification_preferences"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&json!({
            "notification_preferences": {
                "new_announcement": { "frequency": "weekly" },
                "new_discussion_entry": { "frequency": "never" }
            }
        }))
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "update_all by address: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    let prefs = body["notification_preferences"]
        .as_array()
        .expect("update_all returns the channel's stored preferences");
    // new_announcement updated in place, new_discussion_entry inserted.
    assert_eq!(prefs.len(), 2);
    let by_name = |name: &str| {
        prefs
            .iter()
            .find(|p| p["notification"] == name)
            .unwrap_or_else(|| panic!("missing {name} in {prefs:?}"))
    };
    assert_eq!(by_name("new_announcement")["frequency"], "weekly");
    assert_eq!(by_name("new_discussion_entry")["frequency"], "never");

    // Persisted for the resolved channel only.
    let stored: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification_preferences WHERE communication_channel_id = $1",
    )
    .bind(channel_id)
    .fetch_one(&pool)
    .await
    .expect("count stored preferences");
    assert_eq!(stored, 2, "both updates must be persisted");

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn update_channel_preference_by_address_single() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let address = format!("{prefix}-update-one@example.edu");
    let user_id = seed_user(&pool, &prefix, "upd-one", "student").await;
    let channel_id = seed_channel(&pool, user_id, &address, "email", "active").await;
    seed_preference(
        &pool,
        user_id,
        channel_id,
        "new_announcement",
        "announcement",
        "daily",
    )
    .await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .put(&format!(
            "/api/v1/users/self/communication_channels/email/{address}/notification_preferences/new_announcement"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&json!({
            "notification_preferences": { "frequency": "immediately" }
        }))
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "single update by address: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    let prefs = body["notification_preferences"]
        .as_array()
        .expect("single update wraps a one-element array");
    assert_eq!(prefs.len(), 1);
    assert_eq!(prefs[0]["notification"], "new_announcement");
    assert_eq!(prefs[0]["category"], "announcement");
    assert_eq!(prefs[0]["frequency"], "immediately");

    let frequency: String = sqlx::query_scalar(
        "SELECT frequency FROM notification_preferences WHERE communication_channel_id = $1 AND notification = 'new_announcement'",
    )
    .bind(channel_id)
    .fetch_one(&pool)
    .await
    .expect("stored frequency");
    assert_eq!(
        frequency, "immediately",
        "upsert must update the stored row"
    );

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn update_channel_preference_by_address_invalid_frequency_is_422() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let address = format!("{prefix}-bad-freq@example.edu");
    let user_id = seed_user(&pool, &prefix, "upd-bad", "student").await;
    seed_channel(&pool, user_id, &address, "email", "active").await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .put(&format!(
            "/api/v1/users/self/communication_channels/email/{address}/notification_preferences/new_announcement"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&json!({
            "notification_preferences": { "frequency": "hourly" }
        }))
        .await;

    assert_eq!(
        resp.status_code(),
        422,
        "invalid frequency must be 422: {:#?}",
        resp.text()
    );

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn update_channel_preferences_by_address_unknown_channel_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "upd-unk-cc", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .put("/api/v1/users/self/communication_channels/email/ghost@example.edu/notification_preferences")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&json!({
            "notification_preferences": { "new_announcement": { "frequency": "daily" } }
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown channel on update must be 404: {:#?}",
        resp.text()
    );

    teardown(&pool, &[user_id]).await;
}

// ---------------------------------------------------------------------------
// Tests — PUT /users/{userId}/notification_preference_categories/{category}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_channel_preferences_by_category_updates_every_notification() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "upd-cat", "student").await;
    let channel_id = seed_channel(
        &pool,
        user_id,
        &format!("{prefix}-cat@example.edu"),
        "email",
        "active",
    )
    .await;
    // A discussion preference that must NOT be touched by an announcement
    // category update.
    seed_preference(
        &pool,
        user_id,
        channel_id,
        "new_discussion_entry",
        "discussion",
        "weekly",
    )
    .await;

    let token = auth_header(&pool, user_id, "student");
    let resp = server
        .put("/api/v1/users/self/notification_preference_categories/announcement")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&json!({
            "notification_preferences": { "frequency": "daily" }
        }))
        .await;

    assert_eq!(
        resp.status_code(),
        200,
        "category update: {:#?}",
        resp.text()
    );
    let body: Value = resp.json::<Value>();
    let prefs = body["notification_preferences"]
        .as_array()
        .expect("category update returns the category's preferences");
    let names: Vec<&str> = prefs
        .iter()
        .map(|p| p["notification"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"new_announcement"), "names: {names:?}");
    assert!(
        names.contains(&"announcement_created_by_student"),
        "names: {names:?}"
    );
    assert!(
        prefs.iter().all(|p| p["frequency"] == "daily"),
        "every category preference gets the frequency"
    );
    assert!(
        prefs.iter().all(|p| p["category"] == "announcement"),
        "only the requested category is returned"
    );

    // The discussion preference keeps its old frequency.
    let discussion: String = sqlx::query_scalar(
        "SELECT frequency FROM notification_preferences WHERE communication_channel_id = $1 AND notification = 'new_discussion_entry'",
    )
    .bind(channel_id)
    .fetch_one(&pool)
    .await
    .expect("stored discussion frequency");
    assert_eq!(discussion, "weekly", "other categories must be untouched");

    teardown(&pool, &[user_id]).await;
}

#[tokio::test]
async fn update_channel_preferences_by_category_unknown_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "upd-cat-unk", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .put("/api/v1/users/self/notification_preference_categories/not_a_category")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&json!({
            "notification_preferences": { "frequency": "daily" }
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown category must be 404 (Canvas RecordNotFound): {:#?}",
        resp.text()
    );

    teardown(&pool, &[user_id]).await;
}

// ---------------------------------------------------------------------------
// Tests — write gates (self-only updates)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn channel_preference_updates_are_self_only() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let address = format!("{prefix}-gate@example.edu");
    let owner_id = seed_user(&pool, &prefix, "gate-owner", "student").await;
    let other_id = seed_user(&pool, &prefix, "gate-other", "student").await;
    let admin_id = seed_user(&pool, &prefix, "gate-admin", "admin").await;
    seed_channel(&pool, owner_id, &address, "email", "active").await;

    let payload_update_all = json!({
        "notification_preferences": { "new_announcement": { "frequency": "daily" } }
    });
    let payload_single = json!({
        "notification_preferences": { "frequency": "daily" }
    });

    // Another student cannot update_all, single-update, or category-update
    // the owner's preferences.
    let token = auth_header(&pool, other_id, "student");
    let resp = server
        .put(&format!(
            "/api/v1/users/{owner_id}/communication_channels/email/{address}/notification_preferences"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload_update_all)
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "non-self update_all must be 403: {:#?}",
        resp.text()
    );

    let resp = server
        .put(&format!(
            "/api/v1/users/{owner_id}/communication_channels/email/{address}/notification_preferences/new_announcement"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload_single)
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "non-self single update must be 403: {:#?}",
        resp.text()
    );

    let resp = server
        .put(&format!(
            "/api/v1/users/{owner_id}/notification_preference_categories/announcement"
        ))
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&payload_single)
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "non-self category update must be 403: {:#?}",
        resp.text()
    );

    // Canvas keeps updates self-only even for admins
    // (render_unauthorized_action unless @user == @current_user).
    let admin_token = auth_header(&pool, admin_id, "admin");
    let resp = server
        .put(&format!(
            "/api/v1/users/{owner_id}/communication_channels/email/{address}/notification_preferences"
        ))
        .add_header(axum::http::header::AUTHORIZATION, admin_token.as_str())
        .json(&payload_update_all)
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "admin update_all for another user must be 403: {:#?}",
        resp.text()
    );

    teardown(&pool, &[owner_id, other_id, admin_id]).await;
}

#[tokio::test]
async fn channel_preference_updates_unknown_user_returns_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let user_id = seed_user(&pool, &prefix, "upd-unk-user", "student").await;
    let token = auth_header(&pool, user_id, "student");

    let resp = server
        .put("/api/v1/users/99999999/notification_preference_categories/announcement")
        .add_header(axum::http::header::AUTHORIZATION, token.as_str())
        .json(&json!({
            "notification_preferences": { "frequency": "daily" }
        }))
        .await;
    assert_eq!(resp.status_code(), 404, "unknown user must be 404");

    teardown(&pool, &[user_id]).await;
}
