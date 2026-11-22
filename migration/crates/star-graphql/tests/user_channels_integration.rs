//! Integration tests for CommunicationChannel, Notification and
//! NotificationPolicy on `User` (GraphQL parity gap completion).
//!
//! Conventions mirror `assignments_connection_integration.rs`.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-uc-{}-{}",
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

    let app = axum::Router::new()
        .nest("/api/graphql", star_graphql::router())
        .with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
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
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

/// Channels/preferences/notifications cascade on user deletion.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

async fn gql_post(server: &TestServer, token: &str, query: &str, variables: Value) -> Value {
    let resp = server
        .post("/api/graphql")
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "query": query, "variables": variables }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "graphql post should succeed; body: {}",
        resp.text()
    );
    resp.json()
}

#[tokio::test]
async fn communication_channels_and_policies_canvas_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let chan1: i64 = sqlx::query_scalar(
        r#"INSERT INTO communication_channels (user_id, path, path_type, position, workflow_state)
           VALUES ($1, $2, 'email', 0, 'active') RETURNING id"#,
    )
    .bind(owner)
    .bind(format!("{prefix}-owner@test.local"))
    .fetch_one(&pool)
    .await
    .expect("seed channel 1");
    let chan2: i64 = sqlx::query_scalar(
        r#"INSERT INTO communication_channels (user_id, path, path_type, position, workflow_state)
           VALUES ($1, $2, 'sms', 1, 'unconfirmed') RETURNING id"#,
    )
    .bind(owner)
    .bind("+15550001")
    .fetch_one(&pool)
    .await
    .expect("seed channel 2");
    // A retired channel: must be hidden.
    sqlx::query(
        r#"INSERT INTO communication_channels (user_id, path, path_type, position, workflow_state)
           VALUES ($1, $2, 'email', 2, 'retired')"#,
    )
    .bind(owner)
    .bind(format!("{prefix}-old@test.local"))
    .execute(&pool)
    .await
    .expect("seed retired channel");
    // A policy on the active channel (0045 preference row).
    sqlx::query(
        r#"INSERT INTO notification_preferences (user_id, communication_channel_id, notification, frequency)
           VALUES ($1, $2, 'new_announcement', 'daily')"#,
    )
    .bind(owner)
    .bind(chan1)
    .execute(&pool)
    .await
    .expect("seed preference");
    // A policy on a retired channel: must be hidden with the channel.
    let retired_chan: i64 = sqlx::query_scalar(
        r#"INSERT INTO communication_channels (user_id, path, path_type, position, workflow_state)
           VALUES ($1, $2, 'email', 3, 'retired') RETURNING id"#,
    )
    .bind(owner)
    .bind(format!("{prefix}-older@test.local"))
    .fetch_one(&pool)
    .await
    .expect("seed retired channel 2");
    sqlx::query(
        r#"INSERT INTO notification_preferences (user_id, communication_channel_id, notification, frequency)
           VALUES ($1, $2, 'new_message', 'immediately')"#,
    )
    .bind(owner)
    .bind(retired_chan)
    .execute(&pool)
    .await
    .expect("seed hidden preference");

    let body = gql_post(
        &server,
        &auth(owner, "student"),
        "query ($id: ID!) { user(id: $id) { communicationChannels { id path pathType \
         position workflowState createdAt updatedAt } \
         notificationPreferences { id communicationChannelId notification frequency } } }",
        json!({ "id": owner.to_string() }),
    )
    .await;

    let channels = body["data"]["user"]["communicationChannels"]
        .as_array()
        .expect("channels");
    assert_eq!(channels.len(), 2, "retired hidden: {body}");
    assert_eq!(channels[0]["id"], chan1.to_string(), "position order: {body}");
    assert_eq!(channels[0]["pathType"], "email", "{body}");
    assert_eq!(channels[0]["workflowState"], "active", "{body}");
    assert!(channels[0]["createdAt"].is_string(), "{body}");
    assert_eq!(channels[1]["id"], chan2.to_string(), "{body}");
    assert_eq!(channels[1]["pathType"], "sms", "{body}");
    assert_eq!(channels[1]["workflowState"], "unconfirmed", "{body}");

    let prefs = body["data"]["user"]["notificationPreferences"]
        .as_array()
        .expect("prefs");
    assert_eq!(prefs.len(), 1, "retired-channel policy hidden: {body}");
    assert_eq!(prefs[0]["communicationChannelId"], chan1.to_string(), "{body}");
    assert_eq!(prefs[0]["notification"], "new_announcement", "{body}");
    assert_eq!(prefs[0]["frequency"], "daily", "{body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn notifications_canvas_shape_and_privacy() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let first: i64 = sqlx::query_scalar(
        r#"INSERT INTO notifications (user_id, subject, body, category)
           VALUES ($1, 'First', 'read body', 'announcement') RETURNING id"#,
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .expect("seed first");
    sqlx::query(
        r#"INSERT INTO notifications (user_id, subject, body, category, workflow_state)
           VALUES ($1, 'Second', 'unread body', 'other', 'unread')"#,
    )
    .bind(owner)
    .execute(&pool)
    .await
    .expect("seed second");
    sqlx::query(
        r#"INSERT INTO notifications (user_id, subject, body, category, workflow_state)
           VALUES ($1, 'Deleted', 'gone', 'other', 'deleted')"#,
    )
    .bind(owner)
    .execute(&pool)
    .await
    .expect("seed deleted");

    let query = "query ($id: ID!) { user(id: $id) { notifications { id subject body \
                 category workflowState createdAt } } }";

    // Self sees the non-deleted notifications.
    let body = gql_post(&server, &auth(owner, "student"), query, json!({ "id": owner.to_string() })).await;
    let notes = body["data"]["user"]["notifications"].as_array().expect("notes");
    assert_eq!(notes.len(), 2, "deleted hidden: {body}");
    // Newest first: 'Second' (inserted later) before 'First'.
    assert_eq!(notes[0]["subject"], "Second", "{body}");
    assert_eq!(notes[1]["id"], first.to_string(), "{body}");
    assert_eq!(notes[0]["workflowState"], "unread", "{body}");
    assert_eq!(notes[1]["workflowState"], "unread", "default: {body}");
    assert_eq!(notes[1]["category"], "announcement", "{body}");
    assert!(notes[1]["createdAt"].is_string(), "{body}");

    // A stranger gets null for every private field.
    let body = gql_post(&server, &auth(stranger, "student"), query, json!({ "id": owner.to_string() })).await;
    assert!(body["data"]["user"]["notifications"].is_null(), "{body}");
    assert!(
        body["data"]["user"]["communicationChannels"].is_null(),
        "{body}"
    );

    // Admin sees them (self-or-admin).
    let body = gql_post(&server, &auth(admin, "admin"), query, json!({ "id": owner.to_string() })).await;
    assert_eq!(
        body["data"]["user"]["notifications"].as_array().unwrap().len(),
        2,
        "{body}"
    );

    cleanup(&pool, &prefix).await;
}
