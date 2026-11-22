//! Integration tests for the communication_channels tail (#62).
//!
//! Canvas `CommunicationChannelsController` remainder:
//! - POST   /users/:user_id/communication_channels/:id        (reset_bounce_count)
//! - DELETE /users/:user_id/communication_channels/:type/:address (destroy)
//! - DELETE /users/self/communication_channels/push            (delete_push_token)
//! - GET    /accounts/:account_id/bounced_communication_channels.csv
//! - GET    /accounts/:account_id/unconfirmed_communication_channels[.csv]
//! - POST   /accounts/:account_id/bounced_communication_channels/reset
//! - POST   /accounts/:account_id/unconfirmed_communication_channels/confirm
//!
//! Same harness as communication_channels_integration.rs: DATABASE_URL-gated,
//! unique prefix, dedicated accounts (never account 1), full teardown.

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
        "cc62_{}_{}",
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

/// Create a dedicated account (never account 1) and return its id.
async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed_account")
}

/// Link a user to an account through account_users.
async fn link_user_to_account(pool: &PgPool, account_id: i64, user_id: i64, role: &str) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, $3)")
        .bind(account_id)
        .bind(user_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("link user to account");
}

fn auth_header(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Full teardown for a test's rows: channels of the given users, the users
/// (cascades account_users), and the dedicated account.
async fn cleanup(pool: &PgPool, user_ids: &[i64], account_id: i64) {
    for user_id in user_ids {
        let _ = sqlx::query("DELETE FROM communication_channels WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
    }
    for user_id in user_ids {
        let _ = sqlx::query("DELETE FROM account_users WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
    }
    for user_id in user_ids {
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
    }
    let _ = sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(account_id)
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// POST /users/{user_id}/communication_channels/{id} — reset_bounce_count
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reset_bounce_count_as_site_admin_resets_and_returns_channel() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "reset-acct").await;

    let admin = seed_user(&pool, &prefix, "reset-admin", "admin").await;
    let user = seed_user(&pool, &prefix, "reset-user", "student").await;
    let channel = seed_channel(&pool, user, "reset@example.edu", "email", "active").await;
    sqlx::query("UPDATE communication_channels SET bounce_count = 4, last_bounce_summary = 'bad mailbox' WHERE id = $1")
        .bind(channel)
        .execute(&pool)
        .await
        .expect("set bounce fields");

    let resp = server
        .post(&format!(
            "/api/v1/users/{user}/communication_channels/{channel}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "site admin resets bounce count: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(channel));
    assert_eq!(body["user_id"].as_i64(), Some(user));
    assert_eq!(body["bounce_count"].as_i64(), Some(0), "bounce_count reset");
    assert_eq!(body["last_bounce_summary"], json!("bad mailbox"));
    assert_eq!(body["type"], json!("email"));
    assert_eq!(body["address"], json!("reset@example.edu"));

    // persisted
    let (count,): (i32,) =
        sqlx::query_as("SELECT bounce_count FROM communication_channels WHERE id = $1")
            .bind(channel)
            .fetch_one(&pool)
            .await
            .expect("reload channel");
    assert_eq!(count, 0);

    cleanup(&pool, &[admin, user], account_id).await;
}

#[tokio::test]
async fn reset_bounce_count_as_plain_user_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "reset-acct-2").await;

    // Canvas policy: only site admins hold :reset_bounce_count — not even the
    // channel owner.
    let user = seed_user(&pool, &prefix, "owner", "student").await;
    let channel = seed_channel(&pool, user, "owner@example.edu", "email", "active").await;

    let resp = server
        .post(&format!(
            "/api/v1/users/{user}/communication_channels/{channel}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(user, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "non site-admin gets render_unauthorized_action: {}",
        resp.text()
    );

    cleanup(&pool, &[user], account_id).await;
}

#[tokio::test]
async fn reset_bounce_count_unknown_channel_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "reset-acct-3").await;

    let admin = seed_user(&pool, &prefix, "admin3", "admin").await;
    let user = seed_user(&pool, &prefix, "user3", "student").await;

    let resp = server
        .post(&format!(
            "/api/v1/users/{user}/communication_channels/999999999"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "unknown channel: {}", resp.text());

    cleanup(&pool, &[admin, user], account_id).await;
}

#[tokio::test]
async fn reset_bounce_count_retired_channel_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "reset-acct-4").await;

    let admin = seed_user(&pool, &prefix, "admin4", "admin").await;
    let user = seed_user(&pool, &prefix, "user4", "student").await;
    let channel = seed_channel(&pool, user, "retired@example.edu", "email", "retired").await;

    let resp = server
        .post(&format!(
            "/api/v1/users/{user}/communication_channels/{channel}"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "unretired scope excludes retired channels: {}",
        resp.text()
    );

    cleanup(&pool, &[admin, user], account_id).await;
}

#[tokio::test]
async fn reset_bounce_count_without_token_is_401() {
    let Some((server, _, _)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let resp = server
        .post("/api/v1/users/1/communication_channels/1")
        .await;
    assert_eq!(resp.status_code(), 401, "auth extractor: {}", resp.text());
}

// ---------------------------------------------------------------------------
// DELETE /users/{user_id}/communication_channels/{type}/{address} — destroy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn destroy_by_type_address_retires_and_returns_channel() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "destroy-acct").await;

    let user = seed_user(&pool, &prefix, "destroy-user", "student").await;
    let channel = seed_channel(&pool, user, "destroy@example.edu", "email", "active").await;

    let resp = server
        .delete(&format!(
            "/api/v1/users/{user}/communication_channels/email/destroy@example.edu"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(user, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "self destroys own channel by type+address: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(channel));
    assert_eq!(body["workflow_state"], json!("retired"));

    let (state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM communication_channels WHERE id = $1")
            .bind(channel)
            .fetch_one(&pool)
            .await
            .expect("reload channel");
    assert_eq!(state, "retired");

    cleanup(&pool, &[user], account_id).await;
}

#[tokio::test]
async fn destroy_by_type_address_admin_follows_crate_convention_403() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "destroy-acct-2").await;

    // Canvas allows admins via :manage_user_details, but the crate's
    // pre-existing convention for CommunicationChannelsController#destroy —
    // locked by delete_channel_other_user_returns_403 and shared by the
    // by-id route — is write = self only. The by-address route reuses
    // authorize_channel_operation so both forms of the same Canvas action
    // stay consistent (divergence documented in the PR body).
    let admin = seed_user(&pool, &prefix, "destroy-admin", "admin").await;
    let user = seed_user(&pool, &prefix, "destroy-target", "student").await;
    seed_channel(&pool, user, "target@example.edu", "email", "active").await;

    let resp = server
        .delete(&format!(
            "/api/v1/users/{user}/communication_channels/email/target@example.edu"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "crate convention: destroy is self-only, same as by-id: {}",
        resp.text()
    );

    // the channel is untouched
    let (state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM communication_channels WHERE user_id = $1")
            .bind(user)
            .fetch_one(&pool)
            .await
            .expect("reload channel");
    assert_eq!(state, "active");

    cleanup(&pool, &[admin, user], account_id).await;
}

#[tokio::test]
async fn destroy_by_type_address_stranger_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "destroy-acct-3").await;

    let user = seed_user(&pool, &prefix, "owner3", "student").await;
    let stranger = seed_user(&pool, &prefix, "stranger3", "student").await;
    seed_channel(&pool, user, "keep@example.edu", "email", "active").await;

    let resp = server
        .delete(&format!(
            "/api/v1/users/{user}/communication_channels/email/keep@example.edu"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(stranger, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "stranger cannot destroy another user's channel: {}",
        resp.text()
    );

    cleanup(&pool, &[user, stranger], account_id).await;
}

#[tokio::test]
async fn destroy_by_type_address_unknown_pair_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "destroy-acct-4").await;

    let user = seed_user(&pool, &prefix, "owner4", "student").await;

    let resp = server
        .delete(&format!(
            "/api/v1/users/{user}/communication_channels/sms/%2B15550001111"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(user, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "unknown (type, address) pair: {}",
        resp.text()
    );

    cleanup(&pool, &[user], account_id).await;
}

#[tokio::test]
async fn destroy_by_type_address_case_insensitive_match() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "destroy-acct-5").await;

    // Canvas by_path lowercases both sides.
    let user = seed_user(&pool, &prefix, "owner5", "student").await;
    seed_channel(&pool, user, "MixedCase@Example.EDU", "email", "active").await;

    let resp = server
        .delete(&format!(
            "/api/v1/users/{user}/communication_channels/email/mixedcase@example.edu"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(user, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "by_path is case-insensitive: {}",
        resp.text()
    );

    cleanup(&pool, &[user], account_id).await;
}

// ---------------------------------------------------------------------------
// DELETE /users/self/communication_channels/push — delete_push_token
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_push_token_retires_push_channel() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "push-acct").await;

    let user = seed_user(&pool, &prefix, "push-user", "student").await;
    let push = seed_channel(&pool, user, "token-abc", "push", "active").await;
    // an email channel must survive untouched
    let email = seed_channel(&pool, user, "push@example.edu", "email", "active").await;

    let resp = server
        .delete("/api/v1/users/self/communication_channels/push")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(user, "student"),
        )
        .json(&json!({ "push_token": "TOKEN-ABC" }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "push channel deleted: {}",
        resp.text()
    );
    assert_eq!(resp.json::<Value>(), json!({ "success": true }));

    let (state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM communication_channels WHERE id = $1")
            .bind(push)
            .fetch_one(&pool)
            .await
            .expect("reload push channel");
    assert_eq!(state, "retired");

    let (state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM communication_channels WHERE id = $1")
            .bind(email)
            .fetch_one(&pool)
            .await
            .expect("reload email channel");
    assert_eq!(state, "active", "email channel untouched");

    cleanup(&pool, &[user], account_id).await;
}

#[tokio::test]
async fn delete_push_token_without_push_channel_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "push-acct-2").await;

    let user = seed_user(&pool, &prefix, "nopush-user", "student").await;

    let resp = server
        .delete("/api/v1/users/self/communication_channels/push")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(user, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "no push channel: {}", resp.text());

    cleanup(&pool, &[user], account_id).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{account_id}/bounced_communication_channels.csv
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bounced_csv_report_returns_text_csv_with_headers() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "csv-acct").await;

    let admin = seed_user(&pool, &prefix, "csv-admin", "admin").await;
    link_user_to_account(&pool, account_id, admin, "admin").await;
    let user = seed_user(&pool, &prefix, "csv-user", "student").await;
    link_user_to_account(&pool, account_id, user, "student").await;

    let channel = seed_channel(&pool, user, "csv@example.edu", "email", "active").await;
    sqlx::query("UPDATE communication_channels SET bounce_count = 2, last_bounce_summary = 'mailbox full' WHERE id = $1")
        .bind(channel)
        .execute(&pool)
        .await
        .expect("set bounce fields");

    let resp = server
        .get(&format!(
            "/api/v1/accounts/{account_id}/bounced_communication_channels.csv"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "CSV report renders: {}",
        resp.text()
    );
    let content_type = resp.content_type();
    assert!(
        content_type.starts_with("text/csv"),
        "content-type must be text/csv, got {content_type}"
    );
    let body = resp.text();
    let first_line = body.lines().next().unwrap_or_default();
    assert_eq!(
        first_line,
        "User ID,Name,Communication channel ID,Type,Path,Date of most recent bounce,Bounce reason",
        "CSV header row must match Canvas column_headers"
    );
    assert!(
        body.contains("csv@example.edu"),
        "row must contain the bounced channel path"
    );
    assert!(
        body.contains("mailbox full"),
        "row must contain the bounce summary"
    );

    cleanup(&pool, &[admin, user], account_id).await;
}

#[tokio::test]
async fn bounced_csv_report_non_admin_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "csv-acct-2").await;

    let user = seed_user(&pool, &prefix, "csv-user-2", "student").await;
    link_user_to_account(&pool, account_id, user, "student").await;

    let resp = server
        .get(&format!(
            "/api/v1/accounts/{account_id}/bounced_communication_channels.csv"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(user, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "view_bounced_emails required: {}",
        resp.text()
    );

    cleanup(&pool, &[user], account_id).await;
}

#[tokio::test]
async fn bounced_csv_report_unknown_account_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "csv-acct-3").await;

    let admin = seed_user(&pool, &prefix, "csv-admin-3", "admin").await;

    let resp = server
        .get("/api/v1/accounts/999999999/bounced_communication_channels.csv")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "unknown account: {}", resp.text());

    cleanup(&pool, &[admin], account_id).await;
}

#[tokio::test]
async fn bounced_csv_report_self_account_maps_to_root() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "csv-acct-4").await;

    // `self` maps to the domain root account (id 1); a site admin gets a 200
    // (rows depend on shared DB state, so only the status is asserted).
    let admin = seed_user(&pool, &prefix, "csv-admin-4", "admin").await;

    let resp = server
        .get("/api/v1/accounts/self/bounced_communication_channels.csv")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "account_id self maps to root account: {}",
        resp.text()
    );

    cleanup(&pool, &[admin], account_id).await;
}

// ---------------------------------------------------------------------------
// GET /accounts/{account_id}/unconfirmed_communication_channels[.csv]
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unconfirmed_json_report_returns_header_plus_rows() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "unc-acct").await;

    let admin = seed_user(&pool, &prefix, "unc-admin", "admin").await;
    link_user_to_account(&pool, account_id, admin, "admin").await;
    let user = seed_user(&pool, &prefix, "unc-user", "student").await;
    link_user_to_account(&pool, account_id, user, "student").await;

    seed_channel(
        &pool,
        user,
        "unconfirmed@example.edu",
        "email",
        "unconfirmed",
    )
    .await;
    seed_channel(&pool, user, "active@example.edu", "email", "active").await;

    let resp = server
        .get(&format!(
            "/api/v1/accounts/{account_id}/unconfirmed_communication_channels"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "JSON report renders: {}",
        resp.text()
    );
    let body: Value = resp.json();
    let rows = body.as_array().expect("json_report is an array of arrays");
    assert_eq!(
        rows[0],
        json!([
            "User ID",
            "Name",
            "Communication channel ID",
            "Type",
            "Path",
            "Created at"
        ]),
        "first row is the Canvas column_headers"
    );
    let matched: Vec<&Value> = rows
        .iter()
        .skip(1)
        .filter(|r| r.get(4).and_then(|c| c.as_str()) == Some("unconfirmed@example.edu"))
        .collect();
    assert_eq!(matched.len(), 1, "only the unconfirmed channel appears");
    let excluded: Vec<&Value> = rows
        .iter()
        .skip(1)
        .filter(|r| r.get(4).and_then(|c| c.as_str()) == Some("active@example.edu"))
        .collect();
    assert!(
        excluded.is_empty(),
        "active channels never appear in the Confirm report"
    );

    cleanup(&pool, &[admin, user], account_id).await;
}

#[tokio::test]
async fn unconfirmed_csv_report_matches_json_rows() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "unc-acct-2").await;

    let admin = seed_user(&pool, &prefix, "unc-admin-2", "admin").await;
    link_user_to_account(&pool, account_id, admin, "admin").await;
    let user = seed_user(&pool, &prefix, "unc-user-2", "student").await;
    link_user_to_account(&pool, account_id, user, "student").await;

    seed_channel(&pool, user, "wait@example.edu", "email", "unconfirmed").await;

    let resp = server
        .get(&format!(
            "/api/v1/accounts/{account_id}/unconfirmed_communication_channels.csv"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let content_type = resp.content_type();
    assert!(
        content_type.starts_with("text/csv"),
        "content-type must be text/csv, got {content_type}"
    );
    let body = resp.text();
    let first_line = body.lines().next().unwrap_or_default();
    assert_eq!(
        first_line, "User ID,Name,Communication channel ID,Type,Path,Created at",
        "CSV header row must match Canvas Confirm column_headers"
    );
    assert!(
        body.contains("wait@example.edu"),
        "row must contain the unconfirmed channel"
    );

    cleanup(&pool, &[admin, user], account_id).await;
}

#[tokio::test]
async fn unconfirmed_report_account_admin_via_account_users() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "unc-acct-3").await;

    // A non-site-admin user who is an account_users admin of the account
    // holds view_bounced_emails in star-lms' approximation.
    let acct_admin = seed_user(&pool, &prefix, "acct-admin", "student").await;
    link_user_to_account(&pool, account_id, acct_admin, "admin").await;

    let resp = server
        .get(&format!(
            "/api/v1/accounts/{account_id}/unconfirmed_communication_channels"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(acct_admin, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "account admin sees the report: {}",
        resp.text()
    );

    cleanup(&pool, &[acct_admin], account_id).await;
}

// ---------------------------------------------------------------------------
// POST /accounts/{account_id}/bounced_communication_channels/reset
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bulk_reset_bounce_counts_resets_matching_channels() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "reset-bulk").await;

    let admin = seed_user(&pool, &prefix, "bulk-admin", "admin").await;
    link_user_to_account(&pool, account_id, admin, "admin").await;
    let user = seed_user(&pool, &prefix, "bulk-user", "student").await;
    link_user_to_account(&pool, account_id, user, "student").await;

    let c1 = seed_channel(&pool, user, "bulk1@example.edu", "email", "active").await;
    let c2 = seed_channel(&pool, user, "bulk2@example.edu", "email", "active").await;
    for c in [c1, c2] {
        sqlx::query("UPDATE communication_channels SET bounce_count = 5 WHERE id = $1")
            .bind(c)
            .execute(&pool)
            .await
            .expect("set bounce_count");
    }

    let resp = server
        .post(&format!(
            "/api/v1/accounts/{account_id}/bounced_communication_channels/reset"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "site admin bulk reset: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(
        body["scheduled_reset_approximate_count"].as_i64(),
        Some(2),
        "Canvas payload is the approximate scheduled count"
    );

    for c in [c1, c2] {
        let (count,): (i32,) =
            sqlx::query_as("SELECT bounce_count FROM communication_channels WHERE id = $1")
                .bind(c)
                .fetch_one(&pool)
                .await
                .expect("reload channel");
        assert_eq!(count, 0, "channel {c} reset");
    }

    cleanup(&pool, &[admin, user], account_id).await;
}

#[tokio::test]
async fn bulk_reset_bounce_counts_non_admin_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "reset-bulk-2").await;

    let user = seed_user(&pool, &prefix, "bulk-user-2", "student").await;
    link_user_to_account(&pool, account_id, user, "student").await;

    let resp = server
        .post(&format!(
            "/api/v1/accounts/{account_id}/bounced_communication_channels/reset"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(user, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "site-admin read_messages required: {}",
        resp.text()
    );

    cleanup(&pool, &[user], account_id).await;
}

// ---------------------------------------------------------------------------
// POST /accounts/{account_id}/unconfirmed_communication_channels/confirm
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bulk_confirm_activates_unconfirmed_channels() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "confirm-bulk").await;

    let admin = seed_user(&pool, &prefix, "confirm-admin", "admin").await;
    link_user_to_account(&pool, account_id, admin, "admin").await;
    let user = seed_user(&pool, &prefix, "confirm-user", "student").await;
    link_user_to_account(&pool, account_id, user, "student").await;

    let c1 = seed_channel(&pool, user, "confirm1@example.edu", "email", "unconfirmed").await;
    let c2 = seed_channel(&pool, user, "confirm2@example.edu", "email", "unconfirmed").await;
    let active = seed_channel(&pool, user, "already@example.edu", "email", "active").await;

    let resp = server
        .post(&format!(
            "/api/v1/accounts/{account_id}/unconfirmed_communication_channels/confirm"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "site admin bulk confirm: {}",
        resp.text()
    );
    let body: Value = resp.json();
    assert_eq!(
        body["confirmed_count"].as_i64(),
        Some(2),
        "Canvas payload is the confirmed row count"
    );

    for c in [c1, c2] {
        let (state,): (String,) =
            sqlx::query_as("SELECT workflow_state FROM communication_channels WHERE id = $1")
                .bind(c)
                .fetch_one(&pool)
                .await
                .expect("reload channel");
        assert_eq!(state, "active", "channel {c} confirmed");
    }
    let (state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM communication_channels WHERE id = $1")
            .bind(active)
            .fetch_one(&pool)
            .await
            .expect("reload active channel");
    assert_eq!(state, "active", "already-active channel unchanged");

    cleanup(&pool, &[admin, user], account_id).await;
}

#[tokio::test]
async fn bulk_confirm_non_admin_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "confirm-bulk-2").await;

    let user = seed_user(&pool, &prefix, "confirm-user-2", "student").await;
    link_user_to_account(&pool, account_id, user, "student").await;
    let acct_admin = seed_user(&pool, &prefix, "acct-admin-2", "student").await;
    link_user_to_account(&pool, account_id, acct_admin, "admin").await;

    // Even an account_users admin is not a site admin: read_messages on
    // Account.site_admin is the stricter gate for the mutating twin.
    let resp = server
        .post(&format!(
            "/api/v1/accounts/{account_id}/unconfirmed_communication_channels/confirm"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(acct_admin, "student"),
        )
        .await;
    assert_eq!(
        resp.status_code(),
        401,
        "account admin is not site admin: {}",
        resp.text()
    );

    cleanup(&pool, &[user, acct_admin], account_id).await;
}

// ---------------------------------------------------------------------------
// Route-table sanity: the new registrations coexist with the old ones
// (matchit panics on conflicting paths, so building the router is the test).
// ---------------------------------------------------------------------------

#[test]
fn communication_channel_tail_routes_build_without_conflicts() {
    let _ = star_notifications::router();
}

// ---------------------------------------------------------------------------
// Soft-delete invisibility for the reports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bounced_csv_report_excludes_retired_channels() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "csv-acct-5").await;

    let admin = seed_user(&pool, &prefix, "csv-admin-5", "admin").await;
    link_user_to_account(&pool, account_id, admin, "admin").await;
    let user = seed_user(&pool, &prefix, "csv-user-5", "student").await;
    link_user_to_account(&pool, account_id, user, "student").await;

    let retired = seed_channel(&pool, user, "gone@example.edu", "email", "retired").await;
    sqlx::query("UPDATE communication_channels SET bounce_count = 7 WHERE id = $1")
        .bind(retired)
        .execute(&pool)
        .await
        .expect("set bounce_count");

    let resp = server
        .get(&format!(
            "/api/v1/accounts/{account_id}/bounced_communication_channels.csv"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth_header(admin, "admin"),
        )
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert_eq!(
        body.lines().count(),
        1,
        "only the header row — retired channels are invisible: {body}"
    );

    cleanup(&pool, &[admin, user], account_id).await;
}
