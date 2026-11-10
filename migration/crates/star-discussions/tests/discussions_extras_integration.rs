//! Integration tests for the star-discussions extras: single-entry
//! read/update/delete, entry replies, and topic subscriptions.
//!
//! Pinned behavior (Canvas API v1 parity):
//! - `GET/PUT/DELETE .../entries/{entryId}` are scoped to the topic in the
//!   URL; a missing, soft-deleted, or cross-topic entry is a 404.
//! - Entry `PUT`/`DELETE` require the entry's author, a course teacher, or an
//!   admin (`DiscussionEntry` :update/:delete policy); other enrolled users
//!   get 403.
//! - `DELETE .../entries/{entryId}` is a soft delete that also clears the
//!   author (`user_id`) and body (`message`), as Canvas documents.
//! - `POST .../entries/{entryId}/replies` nests the new entry under the
//!   parent; replies to replies nest one level deeper. A missing/deleted/
//!   cross-topic parent is a 404, a blank message is a 422 validation error.
//! - `PUT/DELETE .../subscribed` answer 204 with an empty body and upsert one
//!   row per (topic, user). A request that matches the implicit default
//!   (unsubscribed) is a no-op that writes no row, mirroring Canvas
//!   `change_subscribed_state`.
//!
//! Skipped cleanly when DATABASE_URL is not set; uses a unique per-run prefix
//! plus full teardown so repeat runs against a shared DB are safe.

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
        "dscx_{}_{}",
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

    let server =
        TestServer::new(star_discussions::router().with_state(state)).expect("build TestServer");
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
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(course_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed enrollment");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn create_topic_ok(server: &TestServer, course_id: i64, bearer: &str, title: &str) -> Value {
    let resp = server
        .post(&format!("/courses/{course_id}/discussion_topics"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&json!({ "title": title }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    resp.json()
}

async fn create_entry_ok(
    server: &TestServer,
    course_id: i64,
    topic_id: i64,
    bearer: &str,
    message: &str,
) -> Value {
    let resp = server
        .post(&format!(
            "/courses/{course_id}/discussion_topics/{topic_id}/entries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&json!({ "message": message }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    resp.json()
}

/// Delete every row created by this run, in FK-safe order.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM discussion_entries WHERE topic_id IN
         (SELECT id FROM discussion_topics WHERE title LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM discussion_topics WHERE title LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET/PUT/DELETE .../entries/{entryId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn entry_get_and_moderator_update_delete_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "EntryCrud").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let teacher_bearer = auth(teacher, "teacher");
    let student_bearer = auth(student, "student");

    let topic = create_topic_ok(
        &server,
        course,
        &teacher_bearer,
        &format!("{prefix}-EntryCrudTopic"),
    )
    .await;
    let topic_id = topic["id"].as_i64().unwrap();
    let entry = create_entry_ok(&server, course, topic_id, &student_bearer, "original body").await;
    let entry_id = entry["id"].as_i64().unwrap();
    let url = format!("/courses/{course}/discussion_topics/{topic_id}/entries/{entry_id}");

    // GET one returns the canonical entry shape for an enrolled student.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(
        resp.status_code(),
        403,
        "GET by an unenrolled user must be 403; body: {}",
        resp.text()
    );

    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let got: Value = resp.json();
    assert_eq!(got["id"], entry_id);
    assert_eq!(got["topic_id"], topic_id);
    assert_eq!(got["parent_id"], Value::Null);
    assert_eq!(got["user_id"], student);
    assert_eq!(got["message"], "original body");
    assert_eq!(got["workflow_state"], "active");

    // GET through the wrong course path is a 404, even for a valid entry.
    let other_course = seed_course(&pool, &prefix, "EntryCrudOther").await;
    seed_enrollment(&pool, student, other_course, "student").await;
    let resp = server
        .get(&format!(
            "/courses/{other_course}/discussion_topics/{topic_id}/entries/{entry_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.text(), "not found");

    // A course teacher (forum moderator) may edit another user's entry.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "message": "moderated body" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["id"], entry_id);
    assert_eq!(updated["message"], "moderated body");
    assert_eq!(updated["user_id"], student, "author must not change");

    // A moderator may delete the entry: 204, soft delete, author and body
    // cleared.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    assert_eq!(resp.text(), "", "204 must carry an empty body");

    let (state, user_id, message): (String, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT workflow_state, user_id, message FROM discussion_entries WHERE id = $1",
    )
    .bind(entry_id)
    .fetch_one(&pool)
    .await
    .expect("soft-deleted row must still exist");
    assert_eq!(state, "deleted");
    assert_eq!(user_id, None, "delete must clear the author");
    assert_eq!(message, None, "delete must clear the body");

    // The deleted entry is gone for GET and absent from the topic listing.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/entries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer)
        .await;
    assert_eq!(resp.status_code(), 200);
    let list: Value = resp.json();
    assert!(
        !list
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["id"].as_i64() == Some(entry_id)),
        "deleted entry must be absent from the listing: {list}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn entry_update_delete_require_author_or_moderator_and_nonblank_message() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "EntryAuthz").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other, course, "student").await;
    let student_bearer = auth(student, "student");
    let other_bearer = auth(other, "student");

    let topic = create_topic_ok(
        &server,
        course,
        &auth(teacher, "teacher"),
        &format!("{prefix}-EntryAuthzTopic"),
    )
    .await;
    let topic_id = topic["id"].as_i64().unwrap();
    let entry = create_entry_ok(&server, course, topic_id, &student_bearer, "authored body").await;
    let entry_id = entry["id"].as_i64().unwrap();
    let url = format!("/courses/{course}/discussion_topics/{topic_id}/entries/{entry_id}");

    // Enrolled non-authors cannot edit or delete someone else's entry.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, other_bearer.clone())
        .json(&json!({ "message": "hijack" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(resp.text(), "forbidden");
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, other_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The author may update the message; blank bodies are rejected with the
    // crate's plain-text 422 validation body.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "message": "edited by author" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["message"], "edited by author");

    for body in [json!({ "message": "   " }), json!({})] {
        let resp = server
            .put(&url)
            .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
            .json(&body)
            .await;
        assert_eq!(
            resp.status_code(),
            422,
            "body {body}; response: {}",
            resp.text()
        );
        assert_eq!(resp.text(), "validation: message cannot be empty");
    }

    // Missing entries are a 404 for both update and delete.
    let missing = format!("/courses/{course}/discussion_topics/{topic_id}/entries/999999999");
    let resp = server
        .put(&missing)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "message": "ghost" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .delete(&missing)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The author may delete their own entry.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer)
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST .../entries/{entryId}/replies
// ---------------------------------------------------------------------------

#[tokio::test]
async fn add_reply_nests_replies_and_404s_for_foreign_parents() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Replies").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let student_bearer = auth(student, "student");

    let topic = create_topic_ok(
        &server,
        course,
        &auth(teacher, "teacher"),
        &format!("{prefix}-RepliesTopic"),
    )
    .await;
    let topic_id = topic["id"].as_i64().unwrap();
    let root = create_entry_ok(&server, course, topic_id, &student_bearer, "root").await;
    let root_id = root["id"].as_i64().unwrap();
    let replies_url =
        format!("/courses/{course}/discussion_topics/{topic_id}/entries/{root_id}/replies");

    // A reply is created under the parent path entry (201, parent_id set).
    let resp = server
        .post(&replies_url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "message": "first reply" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let reply: Value = resp.json();
    assert_eq!(reply["topic_id"], topic_id);
    assert_eq!(reply["parent_id"], root_id);
    assert_eq!(reply["user_id"], student);
    assert_eq!(reply["message"], "first reply");
    let reply_id = reply["id"].as_i64().unwrap();

    // Replying to a reply nests one level deeper.
    let resp = server
        .post(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/entries/{reply_id}/replies"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "message": "reply to the reply" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let nested: Value = resp.json();
    assert_eq!(nested["parent_id"], reply_id);

    // The threaded listing reflects both levels.
    let resp = server
        .get(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/entries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let list: Value = resp.json();
    assert_eq!(list[0]["id"], root_id);
    assert_eq!(list[0]["replies"][0]["id"], reply_id);
    assert_eq!(list[0]["replies"][0]["replies"][0]["id"], nested["id"]);

    // Blank messages are rejected with the crate's 422 validation body.
    let resp = server
        .post(&replies_url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "message": "  " }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert_eq!(resp.text(), "validation: message cannot be empty");

    // Missing parents and parents from another topic are a 404.
    let resp = server
        .post(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/entries/999999999/replies"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "message": "ghost" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let other_topic = create_topic_ok(
        &server,
        course,
        &auth(teacher, "teacher"),
        &format!("{prefix}-RepliesOther"),
    )
    .await;
    let other_topic_id = other_topic["id"].as_i64().unwrap();
    let other_root = create_entry_ok(
        &server,
        course,
        other_topic_id,
        &student_bearer,
        "other root",
    )
    .await;
    let resp = server
        .post(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/entries/{}/replies",
            other_root["id"]
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "message": "wrong topic" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.text(), "not found");

    // Unenrolled callers are a 403.
    let resp = server
        .post(&replies_url)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "message": "sneaking in" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT/DELETE .../subscribed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscribe_unsubscribe_is_idempotent_and_enrollment_guarded() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let never = seed_user(&pool, &prefix, "Never", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Subscriptions").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, never, course, "student").await;
    let student_bearer = auth(student, "student");
    let teacher_bearer = auth(teacher, "teacher");

    let topic = create_topic_ok(
        &server,
        course,
        &teacher_bearer,
        &format!("{prefix}-Subscribe"),
    )
    .await;
    let topic_id = topic["id"].as_i64().unwrap();
    let url = format!("/courses/{course}/discussion_topics/{topic_id}/subscribed");

    // PUT subscribes: 204 with an empty body and one subscribed row.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    assert_eq!(resp.text(), "", "204 must carry an empty body");
    let (subscribed, count): (bool, i64) = sqlx::query_as(
        "SELECT subscribed, count(*) OVER () FROM discussion_subscriptions
         WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic_id)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("subscription row must exist after PUT");
    assert!(subscribed);
    assert_eq!(count, 1);

    // Repeating PUT stays a single subscribed row.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM discussion_subscriptions WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic_id)
    .bind(student)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "subscribe must upsert, not duplicate");

    // DELETE unsubscribes by flipping the existing row.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let subscribed: bool = sqlx::query_scalar(
        "SELECT subscribed FROM discussion_subscriptions WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic_id)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("row must survive unsubscribe");
    assert!(!subscribed);

    // QUIRK: the implicit default is unsubscribed, so DELETE for a user with
    // no row is a 204 no-op that writes nothing (Canvas change_subscribed_state
    // returns early when the state already matches).
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(never, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM discussion_subscriptions WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic_id)
    .bind(never)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "no-op unsubscribe must not create a row");

    // Unenrolled callers are a 403; deleted topics are a 404.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .delete(&format!("/courses/{course}/discussion_topics/{topic_id}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer)
        .await;
    assert_eq!(resp.status_code(), 204);
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
