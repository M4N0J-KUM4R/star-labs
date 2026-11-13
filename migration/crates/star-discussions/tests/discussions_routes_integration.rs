//! Integration tests for the group-context, read-all, view, entry-list and
//! duplicate routes plus the pinned/locked topic flags.
//!
//! Canvas API v1 parity:
//! - Entries CRUD: the author and a forum moderator (teacher/admin) may
//!   update/delete an entry; an enrolled student may not moderate another
//!   user's entry (403). Deletion is a soft delete: the entry disappears from
//!   the list and single-entry GET answers 404.
//! - Read state is per-user: marking a topic or entry read never touches
//!   another user's read rows (the API keys every row on the caller), and
//!   `read_all` marks the topic plus all of its entries for the caller.
//! - `PUT/DELETE .../subscribed` upsert the caller's subscription row (204).
//! - `POST .../duplicate` is teacher/admin-only in a course context and resets
//!   `pinned`/`locked` on the copy.
//! - `view` answers `{view, participants, unread_entries, forced_entries,
//!   entry_ratings, new_entries}`; `entry_list` returns the requested entries
//!   in id order and flags deleted ones with `deleted: true`.
//! - `pinned`/`locked` survive updates that do not mention them and flip when
//!   they do (Canvas PUT semantics for `published`/`pinned`/`locked`).
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
        "dscr_{}_{}",
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

async fn seed_topic(
    pool: &PgPool,
    prefix: &str,
    course_id: i64,
    title: &str,
    discussion_type: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO discussion_topics (course_id, title, discussion_type, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(format!("{prefix}-{title}"))
    .bind(discussion_type)
    .fetch_one(pool)
    .await
    .expect("seed topic")
}

async fn seed_entry(pool: &PgPool, topic_id: i64, user_id: i64, message: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO discussion_entries (topic_id, user_id, message, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id
        "#,
    )
    .bind(topic_id)
    .bind(user_id)
    .bind(message)
    .fetch_one(pool)
    .await
    .expect("seed entry")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Delete every row created by this run, in FK-safe order. Entries, read
/// rows, ratings and subscriptions cascade from their topics.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM discussion_topic_summaries WHERE discussion_topic_id IN
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
// Entries CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn entries_crud_happy_path_and_student_cannot_moderate() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let author = seed_user(&pool, &prefix, "Author", "student").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "CrudCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, author, course, "student").await;
    seed_enrollment(&pool, student, course, "student").await;

    let topic = seed_topic(&pool, &prefix, course, "CrudTopic", "threaded").await;
    let base = format!("/courses/{course}/discussion_topics/{topic}");

    // The author posts an entry, then a nested reply.
    let resp = server
        .post(&format!("{base}/entries"))
        .add_header(axum::http::header::AUTHORIZATION, auth(author, "student"))
        .json(&json!({ "message": "first post" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let entry: Value = resp.json();
    let entry_id = entry["id"].as_i64().expect("entry id");
    assert_eq!(entry["message"], "first post");

    let resp = server
        .post(&format!("{base}/entries"))
        .add_header(axum::http::header::AUTHORIZATION, auth(author, "student"))
        .json(&json!({ "message": "a reply", "parent_id": entry_id }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    // The list nests the reply under its parent.
    let resp = server
        .get(&format!("{base}/entries"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    assert_eq!(list[0]["replies"].as_array().map(Vec::len), Some(1));

    // Single-entry GET works for any enrolled user.
    let resp = server
        .get(&format!("{base}/entries/{entry_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["message"], "first post");

    // The author may update their own entry.
    let resp = server
        .put(&format!("{base}/entries/{entry_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(author, "student"))
        .json(&json!({ "message": "edited post" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["message"], "edited post");

    // An enrolled student may NOT moderate another user's entry.
    let resp = server
        .put(&format!("{base}/entries/{entry_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "message": "hijacked" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .delete(&format!("{base}/entries/{entry_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A teacher is a moderator and may edit and then soft-delete the entry.
    let resp = server
        .put(&format!("{base}/entries/{entry_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "message": "moderated post" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let resp = server
        .delete(&format!("{base}/entries/{entry_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // The soft-deleted entry is gone from the list and the single GET.
    let resp = server
        .get(&format!("{base}/entries/{entry_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(author, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get(&format!("{base}/entries"))
        .add_header(axum::http::header::AUTHORIZATION, auth(author, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>().as_array().map(Vec::len), Some(0));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Read state (per-user) + read_all variants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn read_state_is_per_user_and_read_all_marks_topic_and_entries() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let first = seed_user(&pool, &prefix, "First", "student").await;
    let second = seed_user(&pool, &prefix, "Second", "student").await;
    let course = seed_course(&pool, &prefix, "ReadCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, first, course, "student").await;
    seed_enrollment(&pool, second, course, "student").await;

    let topic = seed_topic(&pool, &prefix, course, "ReadTopic", "threaded").await;
    let entry_one = seed_entry(&pool, topic, teacher, "one").await;
    let entry_two = seed_entry(&pool, topic, teacher, "two").await;
    let base = format!("/courses/{course}/discussion_topics/{topic}");

    // The first student marks the topic and one entry read.
    let resp = server
        .put(&format!("{base}/read"))
        .add_header(axum::http::header::AUTHORIZATION, auth(first, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let resp = server
        .put(&format!("{base}/entries/{entry_one}/read"))
        .add_header(axum::http::header::AUTHORIZATION, auth(first, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // Only the caller's rows exist: the second student stays unread.
    let states: Vec<(i64, String)> = sqlx::query_as(
        "SELECT user_id, workflow_state FROM discussion_topic_reads WHERE topic_id = $1",
    )
    .bind(topic)
    .fetch_all(&pool)
    .await
    .expect("topic read rows");
    assert_eq!(states, vec![(first, "read".to_string())]);

    // The view reports the caller's unread entries (order-insensitive).
    let resp = server
        .get(&format!("{base}/view"))
        .add_header(axum::http::header::AUTHORIZATION, auth(first, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let mut unread: Vec<i64> = resp.json::<Value>()["unread_entries"]
        .as_array()
        .expect("unread_entries")
        .iter()
        .filter_map(|value| value.as_i64())
        .collect();
    unread.sort_unstable();
    assert_eq!(unread, vec![entry_two]);

    let resp = server
        .get(&format!("{base}/view"))
        .add_header(axum::http::header::AUTHORIZATION, auth(second, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let mut unread: Vec<i64> = resp.json::<Value>()["unread_entries"]
        .as_array()
        .expect("unread_entries")
        .iter()
        .filter_map(|value| value.as_i64())
        .collect();
    unread.sort_unstable();
    assert_eq!(unread, vec![entry_one, entry_two]);

    // read_all (topic + entries) marks everything read for the caller only.
    let resp = server
        .put(&format!("{base}/read_all"))
        .add_header(axum::http::header::AUTHORIZATION, auth(second, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let resp = server
        .get(&format!("{base}/view"))
        .add_header(axum::http::header::AUTHORIZATION, auth(second, "student"))
        .await;
    assert_eq!(resp.json::<Value>()["unread_entries"], json!([]));

    let first_unread = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM discussion_entry_reads WHERE user_id = $1 AND workflow_state = 'unread'",
    )
    .bind(first)
    .fetch_one(&pool)
    .await
    .expect("first student's rows");
    assert_eq!(
        first_unread, 0,
        "the first student has no unread rows: none were written for them by the second"
    );
    let second_read = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM discussion_entry_reads WHERE user_id = $1 AND workflow_state = 'read'",
    )
    .bind(second)
    .fetch_one(&pool)
    .await
    .expect("second student's rows");
    assert_eq!(
        second_read, 2,
        "read_all marked both entries for the caller"
    );

    // Context-level read_all marks every active topic of the course.
    let resp = server
        .put(&format!("/courses/{course}/discussion_topics/read_all"))
        .add_header(axum::http::header::AUTHORIZATION, auth(first, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let topic_state: String = sqlx::query_scalar(
        "SELECT workflow_state FROM discussion_topic_reads WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic)
    .bind(first)
    .fetch_one(&pool)
    .await
    .expect("topic read row");
    assert_eq!(topic_state, "read");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Subscription
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscribe_and_unsubscribe_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "SubCourse").await;
    seed_enrollment(&pool, student, course, "student").await;
    let topic = seed_topic(&pool, &prefix, course, "SubTopic", "threaded").await;
    let url = format!("/courses/{course}/discussion_topics/{topic}/subscribed");

    // Subscribing writes the caller's row; repeating it stays a 204 no-op.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let subscribed: bool = sqlx::query_scalar(
        "SELECT subscribed FROM discussion_subscriptions WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("subscription row");
    assert!(subscribed);

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // Unsubscribing flips the row back.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let subscribed: bool = sqlx::query_scalar(
        "SELECT subscribed FROM discussion_subscriptions WHERE topic_id = $1 AND user_id = $2",
    )
    .bind(topic)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("subscription row");
    assert!(!subscribed);

    // A caller without course read access is a 403.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Duplicate
// ---------------------------------------------------------------------------

#[tokio::test]
async fn duplicate_is_teacher_only_and_resets_pinned_and_locked() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "DupCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let topic = seed_topic(&pool, &prefix, course, "DupTopic", "threaded").await;

    // Pin and lock the source topic first.
    let resp = server
        .put(&format!("/courses/{course}/discussion_topics/{topic}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "pinned": true, "locked": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // A student may not duplicate.
    let resp = server
        .post(&format!(
            "/courses/{course}/discussion_topics/{topic}/duplicate"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A teacher duplicates into the same course; flags reset on the copy.
    let resp = server
        .post(&format!(
            "/courses/{course}/discussion_topics/{topic}/duplicate"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let copy: Value = resp.json();
    let copy_id = copy["id"].as_i64().expect("copy id");
    assert_ne!(copy_id, topic);
    assert_eq!(copy["title"], format!("{prefix}-DupTopic"));
    assert_eq!(copy["pinned"], false, "duplicate resets pinned");
    assert_eq!(copy["locked"], false, "duplicate resets locked");
    assert_eq!(copy["workflow_state"], "active");

    // The copy lives in the same course and shows up in the topic list.
    let resp = server
        .get(&format!("/courses/{course}/discussion_topics"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let ids: Vec<i64> = resp
        .json::<Value>()
        .as_array()
        .expect("topic list")
        .iter()
        .filter_map(|t| t["id"].as_i64())
        .collect();
    assert!(ids.contains(&topic) && ids.contains(&copy_id));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// view + entry_list shapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn view_and_entry_list_shapes() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ViewCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let topic = seed_topic(&pool, &prefix, course, "ViewTopic", "threaded").await;

    let top = seed_entry(&pool, topic, teacher, "top level").await;
    let reply = seed_entry(&pool, topic, student, "a reply").await;
    sqlx::query("UPDATE discussion_entries SET parent_id = $1 WHERE id = $2")
        .bind(top)
        .bind(reply)
        .execute(&pool)
        .await
        .expect("nest reply");
    let base = format!("/courses/{course}/discussion_topics/{topic}");

    // view: nested entries, the participants, the caller's unread entries and
    // ratings map.
    let resp = server
        .get(&format!("{base}/view"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["view"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["view"][0]["id"], top);
    assert_eq!(body["view"][0]["replies"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["participants"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["unread_entries"], json!([top, reply]));
    assert_eq!(body["forced_entries"], json!([]));
    assert_eq!(body["entry_ratings"], json!({}));
    assert_eq!(body["new_entries"], json!([]));

    // Rating the entry surfaces it in the caller's view ratings map.
    let resp = server
        .post(&format!("{base}/entries/{top}/rating"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "rating": 1 }))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let resp = server
        .get(&format!("{base}/view"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    let body: Value = resp.json();
    assert_eq!(body["entry_ratings"][top.to_string()], 1);

    // entry_list: the requested entries in id order.
    let resp = server
        .get(&format!("{base}/entry_list?ids[]={top}&ids[]={reply}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let entries: Value = resp.json();
    assert_eq!(entries.as_array().map(Vec::len), Some(2));
    assert_eq!(entries[0]["id"], top);
    assert_eq!(entries[1]["id"], reply);
    assert!(entries[0].get("deleted").is_none());

    // A soft-deleted entry stays addressable and is flagged with deleted.
    sqlx::query(
        "UPDATE discussion_entries SET workflow_state = 'deleted', user_id = NULL, message = NULL WHERE id = $1",
    )
    .bind(reply)
    .execute(&pool)
    .await
    .expect("soft delete reply");
    let resp = server
        .get(&format!("{base}/entry_list?ids[]={reply}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let entries: Value = resp.json();
    assert_eq!(entries.as_array().map(Vec::len), Some(1));
    assert_eq!(entries[0]["deleted"], true);
    assert_eq!(entries[0]["message"], Value::Null);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// pinned / locked on update
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pinned_and_locked_survive_update() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "FlagCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let topic = seed_topic(&pool, &prefix, course, "FlagTopic", "threaded").await;
    let url = format!("/courses/{course}/discussion_topics/{topic}");

    // Fresh topics are neither pinned nor locked.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["pinned"], false);
    assert_eq!(body["locked"], false);

    // Set both flags and unpublish in one update.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "pinned": true, "locked": true, "published": false }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["pinned"], true);
    assert_eq!(body["locked"], true);
    assert_eq!(body["workflow_state"], "unpublished");

    // An update that does not mention the flags keeps them.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "title": format!("{prefix}-FlagTopicRenamed") }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["title"], format!("{prefix}-FlagTopicRenamed"));
    assert_eq!(body["pinned"], true, "pinned survives an unrelated update");
    assert_eq!(body["locked"], true, "locked survives an unrelated update");

    // Mentioning one flag only changes that flag.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "pinned": false }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["pinned"], false);
    assert_eq!(
        body["locked"], true,
        "locked is untouched by a pinned update"
    );

    // GET reflects the stored flags.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let body: Value = resp.json();
    assert_eq!(body["pinned"], false);
    assert_eq!(body["locked"], true);

    cleanup(&pool, &prefix).await;
}
