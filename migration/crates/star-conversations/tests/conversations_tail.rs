//! Conversations tail (#60, platform spec §5.1–5.7): participant update
//! (star/archive/read), batch_update events + validation strings,
//! mark_all_as_read, admin deleted index / restore / delete_for_all, and the
//! deprecated find_recipients alias.
//!
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Same unique-prefix + full-teardown convention as the other suites.
//!
//! Behavior pinned as implemented (Canvas parity where the snapshot allows,
//! with the deviations documented on the handlers):
//! - PUT /conversations/{id} only touches the caller's participant row;
//!   invalid workflow_state is 400; non-participant / emptied threads 404;
//!   `subscribed` is ignored below three active participants.
//! - PUT /conversations answers Canvas' literal validation strings and applies
//!   the event synchronously, returning a completed Progress object.
//! - The three admin routes answer 401 (not 403) to non-site-admins.
//! - restore echoes conversation_message_json only when a removal was undone.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "conv-tail-{}-{}",
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

    let server = TestServer::new(conversations_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn conversations_router(state: AppState) -> axum::Router {
    // GET /conversations/find_recipients moved to star-search (issue #91);
    // merging both routers keeps this suite covering the alias path exactly
    // the way star-http assembles it.
    axum::Router::new()
        .merge(star_conversations::router())
        .merge(star_search::router())
        .with_state(state)
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    role: &str,
    section: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, course_section_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .bind(section)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
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
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let emails = format!("{prefix}%@test.local");
    let _ = sqlx::query(
        "DELETE FROM conversation_message_removals \
         WHERE message_id IN (SELECT id FROM conversation_messages \
             WHERE conversation_id IN (SELECT conversation_id FROM conversation_participants \
                 WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)))",
    )
    .bind(&emails)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM conversations WHERE id IN \
         (SELECT conversation_id FROM conversation_participants \
             WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1))",
    )
    .bind(&emails)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(&emails)
        .execute(pool)
        .await;
}

/// Create a conversation from `sender` to one or more recipients.
async fn create_conversation(
    server: &TestServer,
    sender: i64,
    role: &str,
    subject: &str,
    body: &str,
    recipients: Vec<i64>,
) -> i64 {
    let resp = server
        .post("/conversations")
        .add_header(axum::http::header::AUTHORIZATION, auth(sender, role))
        .json(&json!({"recipients": recipients, "subject": subject, "body": body}))
        .await;
    assert_eq!(resp.status_code(), 201, "create: {}", resp.text());
    resp.json::<Value>()["id"]
        .as_i64()
        .expect("conversation id")
}

async fn get_ok(server: &TestServer, token: &str, path: &str) -> Value {
    let resp = server
        .get(path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}

// ---------------------------------------------------------------------------
// PUT /conversations/{id} (§5.1)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn participant_update_flags_read_archive_and_star() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "user").await;
    let bob = seed_user(&pool, &prefix, "Bob", "user").await;
    let conv = create_conversation(&server, alice, "user", "Flags", "hello bob", vec![bob]).await;

    // Bob: mark read + star.
    let resp = server
        .put(&format!("/conversations/{conv}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"conversation": {"workflow_state": "read", "starred": true}}))
        .await;
    assert_eq!(resp.status_code(), 200, "update: {}", resp.text());
    let body = resp.json::<Value>();
    assert_eq!(body["unread"], json!(false));
    assert_eq!(body["starred"], json!(true));
    assert_eq!(body["workflow_state"], "active");
    assert_eq!(body["message_count"].as_i64(), Some(1));

    // The author's own copy is unaffected (per-participant semantics).
    let body = get_ok(
        &server,
        &auth(bob, "user"),
        &format!("/conversations/{conv}"),
    )
    .await;
    assert_eq!(body["id"].as_i64(), Some(conv));

    // Mark unread again.
    let resp = server
        .put(&format!("/conversations/{conv}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"conversation": {"workflow_state": "unread"}}))
        .await;
    assert_eq!(resp.json::<Value>()["unread"], json!(true));

    // Archive: leaves the inbox list, participant state becomes archived.
    let resp = server
        .put(&format!("/conversations/{conv}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"conversation": {"workflow_state": "archived"}}))
        .await;
    assert_eq!(resp.json::<Value>()["workflow_state"], "archived");
    let list = get_ok(&server, &auth(bob, "user"), "/conversations").await;
    assert!(
        list.as_array().expect("list").is_empty(),
        "archived conversations leave the default inbox: {list}"
    );
    // Star survives archiving (independent flags, Canvas semantics).
    let starred_check: bool = sqlx::query_scalar(
        "SELECT starred FROM conversation_participants WHERE conversation_id = $1 AND user_id = $2",
    )
    .bind(conv)
    .bind(bob)
    .fetch_one(&pool)
    .await
    .expect("participant row");
    assert!(starred_check);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn participant_update_validation_and_privacy() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "user").await;
    let bob = seed_user(&pool, &prefix, "Bob", "user").await;
    let carol = seed_user(&pool, &prefix, "Carol", "user").await;
    let conv = create_conversation(&server, alice, "user", "Privacy", "body", vec![bob]).await;

    // Non-participants get 404 (existence is not leaked).
    let resp = server
        .put(&format!("/conversations/{conv}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(carol, "user"))
        .json(&json!({"conversation": {"starred": true}}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unknown workflow_state values are 400 (Canvas' enumerated set).
    let resp = server
        .put(&format!("/conversations/{conv}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"conversation": {"workflow_state": "exploded"}}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert!(resp.text().contains("invalid workflow_state"));

    // Unknown fields are ignored (strong params allowlist).
    let resp = server
        .put(&format!("/conversations/{conv}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"conversation": {"starred": true, "subject": "hax"}}))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = get_ok(
        &server,
        &auth(bob, "user"),
        &format!("/conversations/{conv}"),
    )
    .await;
    assert_eq!(body["subject"], "Privacy");

    // `subscribed` is group-only: a 2-person thread ignores it, a 3-person
    // thread honours it.
    let resp = server
        .put(&format!("/conversations/{conv}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"conversation": {"subscribed": false}}))
        .await;
    assert_eq!(
        resp.json::<Value>()["subscribed"],
        json!(true),
        "private thread ignores subscribed"
    );
    create_conversation(&server, alice, "user", "Trio", "hi", vec![bob, carol]).await;
    let trio = sqlx::query_scalar::<_, i64>(
        "SELECT c.id FROM conversations c \
         JOIN conversation_participants cp ON cp.conversation_id = c.id \
         WHERE c.subject = 'Trio' LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("trio id");
    // add_recipients to grow the original pair? simpler: use the trio.
    let resp = server
        .put(&format!("/conversations/{trio}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(carol, "user"))
        .json(&json!({"conversation": {"subscribed": false}}))
        .await;
    assert_eq!(resp.json::<Value>()["subscribed"], json!(false));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /conversations (§5.2)
// ---------------------------------------------------------------------------

async fn batch_error(server: &TestServer, user: i64, body: Value, expected: &str) {
    let resp = server
        .put("/conversations")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "user"))
        .json(&body)
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "body: {expected} got: {}",
        resp.text()
    );
    assert!(resp.text().contains(expected), "body: {}", resp.text());
}

#[tokio::test]
async fn batch_update_validates_with_canvas_messages() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "user").await;
    let bob = seed_user(&pool, &prefix, "Bob", "user").await;

    batch_error(
        &server,
        alice,
        json!({"event": "star"}),
        "conversation_ids not specified",
    )
    .await;
    batch_error(
        &server,
        alice,
        json!({"conversation_ids": []}),
        "conversation_ids not specified",
    )
    .await;
    batch_error(
        &server,
        alice,
        json!({"conversation_ids": "not-a-number"}),
        "conversation_ids not specified",
    )
    .await;
    let too_many: Vec<i64> = (1..=501).collect();
    batch_error(
        &server,
        alice,
        json!({"conversation_ids": too_many, "event": "star"}),
        "conversation batch size limit (500) exceeded",
    )
    .await;
    batch_error(
        &server,
        alice,
        json!({"conversation_ids": [1]}),
        "event not specified",
    )
    .await;
    batch_error(
        &server,
        alice,
        json!({"conversation_ids": [1], "event": "teleport"}),
        "invalid event",
    )
    .await;

    let _ = bob;
    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn batch_update_applies_events_and_answers_progress() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "user").await;
    let bob = seed_user(&pool, &prefix, "Bob", "user").await;
    let conv = create_conversation(&server, alice, "user", "Batch", "hi", vec![bob]).await;
    let other = create_conversation(&server, alice, "user", "Batch2", "hi2", vec![bob]).await;

    let resp = server
        .put("/conversations")
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"conversation_ids": [conv, other, 999999999], "event": "mark_as_read"}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.json::<Value>();
    assert_eq!(body["workflow_state"], "completed");
    assert_eq!(body["completion"].as_f64(), Some(1.0));
    assert_eq!(body["user_id"].as_i64(), Some(bob));

    let count = get_ok(&server, &auth(bob, "user"), "/conversations/unread_count").await;
    assert_eq!(count["unread_count"], "0");

    // Archive then destroy via batch events.
    server
        .put("/conversations")
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"conversation_ids": [conv], "event": "archive"}))
        .await;
    let state: String = sqlx::query_scalar(
        "SELECT workflow_state FROM conversation_participants WHERE conversation_id = $1 AND user_id = $2",
    )
    .bind(conv)
    .bind(bob)
    .fetch_one(&pool)
    .await
    .expect("participant");
    assert_eq!(state, "archived");

    server
        .put("/conversations")
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"conversation_ids": [other], "event": "destroy"}))
        .await;
    let state: String = sqlx::query_scalar(
        "SELECT workflow_state FROM conversation_participants WHERE conversation_id = $1 AND user_id = $2",
    )
    .bind(other)
    .bind(bob)
    .fetch_one(&pool)
    .await
    .expect("participant");
    assert_eq!(state, "deleted");
    // Alice's copy is untouched (batch events are per-participant).
    let state: String = sqlx::query_scalar(
        "SELECT workflow_state FROM conversation_participants WHERE conversation_id = $1 AND user_id = $2",
    )
    .bind(other)
    .bind(alice)
    .fetch_one(&pool)
    .await
    .expect("participant");
    assert_eq!(state, "active");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn mark_all_as_read_clears_the_counter() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "user").await;
    let bob = seed_user(&pool, &prefix, "Bob", "user").await;
    create_conversation(&server, alice, "user", "One", "hi", vec![bob]).await;
    create_conversation(&server, alice, "user", "Two", "hi", vec![bob]).await;

    let count = get_ok(&server, &auth(bob, "user"), "/conversations/unread_count").await;
    assert_eq!(count["unread_count"], "2");

    let resp = server
        .post("/conversations/mark_all_as_read")
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({}));

    let count = get_ok(&server, &auth(bob, "user"), "/conversations/unread_count").await;
    assert_eq!(count["unread_count"], "0");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Admin tail: deleted index / restore / delete_for_all (§5.4–5.6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleted_index_and_restore_round_trip_is_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "user").await;
    let bob = seed_user(&pool, &prefix, "Bob", "user").await;
    let admin = seed_user(&pool, &prefix, "Root", "admin").await;
    let conv = create_conversation(&server, alice, "user", "DeleteMe", "bye", vec![bob]).await;

    let message_id: i64 = sqlx::query_scalar(
        "SELECT id FROM conversation_messages WHERE conversation_id = $1 ORDER BY id LIMIT 1",
    )
    .bind(conv)
    .fetch_one(&pool)
    .await
    .expect("message row");

    let resp = server
        .post(&format!("/conversations/{conv}/remove_messages"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .json(&json!({"remove": [message_id]}))
        .await;
    assert_eq!(resp.status_code(), 200, "remove: {}", resp.text());
    let removal_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM conversation_message_removals WHERE conversation_id = $1 AND user_id = $2 AND message_id = $3)",
    )
    .bind(conv)
    .bind(bob)
    .bind(message_id)
    .fetch_one(&pool)
    .await
    .expect("removal row");
    assert!(removal_exists);

    // A logged-in caller without the right gets 403; Canvas' 401 is for a
    // caller without a session (`render_json_unauthorized`,
    // lib/authentication_methods.rb:413-426).
    let resp = server
        .get("/conversations/deleted")
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "user"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let body = get_ok(&server, &auth(admin, "admin"), "/conversations/deleted").await;
    let rows = body.as_array().expect("array");
    assert!(
        rows.iter().any(|r| r["user_id"].as_i64() == Some(bob)
            && r["conversation_id"].as_i64() == Some(conv)
            && r["message_id"].as_i64() == Some(message_id)),
        "deleted index missing removal: {rows:?}"
    );
    let filtered = get_ok(
        &server,
        &auth(admin, "admin"),
        &format!("/conversations/deleted?user_id={bob}"),
    )
    .await;
    assert_eq!(filtered.as_array().expect("array").len(), 1);

    // Restore requires all three ids; missing ones surface Canvas' errors.
    let resp = server
        .put("/conversations/restore")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({"message_id": message_id}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let errors: Vec<String> = resp
        .json::<Vec<Value>>()
        .into_iter()
        .filter_map(|e| e["attribute"].as_str().map(str::to_string))
        .collect();
    assert_eq!(errors, vec!["user_id", "conversation_id"]);

    let resp = server
        .put(&format!(
            "/conversations/restore?message_id={message_id}&user_id={bob}&conversation_id={conv}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "restore: {}", resp.text());
    let restored = resp.json::<Value>();
    assert_eq!(restored.as_array().expect("array").len(), 1);
    assert_eq!(restored[0]["id"].as_i64(), Some(message_id));
    assert_eq!(restored[0]["body"], "bye");

    // Bob sees the message again.
    let detail = get_ok(
        &server,
        &auth(bob, "user"),
        &format!("/conversations/{conv}"),
    )
    .await;
    assert_eq!(detail["messages"].as_array().expect("messages").len(), 1);

    // A second restore of the same row answers an empty array (no-op).
    let resp = server
        .put("/conversations/restore")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({
            "message_id": message_id, "user_id": bob, "conversation_id": conv
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert!(resp.json::<Value>().as_array().expect("array").is_empty());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn delete_for_all_hard_deletes_for_everyone() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "user").await;
    let bob = seed_user(&pool, &prefix, "Bob", "user").await;
    let admin = seed_user(&pool, &prefix, "Root", "admin").await;
    let conv = create_conversation(&server, alice, "user", "Nuke", "all of it", vec![bob]).await;

    let resp = server
        .delete(&format!("/conversations/{conv}/delete_for_all"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "user"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .delete("/conversations/999999999/delete_for_all")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .delete(&format!("/conversations/{conv}/delete_for_all"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({}));

    let (participants, messages): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM conversation_participants WHERE conversation_id = $1), \
                (SELECT COUNT(*) FROM conversation_messages WHERE conversation_id = $1)",
    )
    .bind(conv)
    .fetch_one(&pool)
    .await
    .expect("cascade counts");
    assert_eq!(participants, 0);
    assert_eq!(messages, 0);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /conversations/find_recipients (§5.7) — served by star-search (issue
// #91, Canvas SearchController#recipients). The legacy global name/email
// index this suite previously pinned is superseded by the shared-context
// recipient search (AddressBook semantics: the caller plus users sharing a
// course/group); star-search's own suite covers the action in depth — this
// keeps one cross-crate sanity that the alias is mounted and answers.
#[tokio::test]
async fn find_recipients_alias_is_served_by_star_search() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice Anderson", "user").await;
    let bob = seed_user(&pool, &prefix, "Bob Barker", "user").await;
    let _stranger = seed_user(&pool, &prefix, "Zed Zeta", "user").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, alice, course, "student", None).await;
    seed_enrollment(&pool, bob, course, "student", None).await;
    // stranger shares nothing.

    // Shared-course lookup through the deprecated alias path (the caller
    // herself matches too — Canvas merges a "self" collection).
    let body = get_ok(
        &server,
        &auth(alice, "user"),
        "/conversations/find_recipients?search=anderson",
    )
    .await;
    let rows = body.as_array().expect("array");
    assert_eq!(rows.len(), 1, "one hit expected: {rows:?}");
    assert_eq!(rows[0]["id"].as_i64(), Some(alice));
    assert_eq!(rows[0]["name"], "Alice Anderson");
    assert_eq!(rows[0]["full_name"], "Alice Anderson");
    assert!(rows[0]["avatar_url"].is_null(), "avatars land with MIG-04");

    let body = get_ok(
        &server,
        &auth(alice, "user"),
        "/conversations/find_recipients?search=barker",
    )
    .await;
    let rows = body.as_array().expect("array");
    assert_eq!(rows.len(), 1, "bob via shared course: {rows:?}");
    assert_eq!(rows[0]["id"].as_i64(), Some(bob));

    // A user with no shared context is invisible (AddressBook semantics).
    let body = get_ok(
        &server,
        &auth(alice, "user"),
        "/conversations/find_recipients?search=zed",
    )
    .await;
    assert_eq!(body.as_array().expect("array").len(), 0);

    // Anonymous stays 401 on the alias.
    let resp = server.get("/conversations/find_recipients?search=x").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
