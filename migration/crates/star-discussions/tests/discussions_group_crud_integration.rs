//! Integration tests for the discussion_topics controller remainder (issue
//! #56): group-context topic CRUD, pinned-topic reorder and messageable
//! users.
//!
//! Canvas API v1 parity (see the handler doc comments for full references):
//! - Group topic create/update/destroy is granted to the group leader
//!   (membership `moderator` flag), the parent course's teachers and site
//!   admins (`create_forum` / `moderate_forum`); plain members may read and
//!   post entries but not manage topics. 401 for missing tokens, 403 for
//!   outsiders / plain members, 404 for unknown or foreign topics, and a
//!   soft-deleted topic disappears from the index and rejects updates but is
//!   still found by destroy (Canvas `all_discussion_topics` has no active
//!   filter).
//! - Destroy answers 200 with the destroyed topic plus the author's name
//!   (Canvas `as_json(include: { user: { only: :name } })`).
//! - The group index default order is Canvas `by_position_legacy`
//!   (position DESC, created_at DESC, id DESC; Postgres sorts NULL positions
//!   first on DESC) and a pin flip moves a topic to the bottom of the pinned
//!   list (`process_pin_parameters` + `add_to_list_bottom`).
//! - `POST .../discussion_topics/reorder` re-numbers the pinned topics:
//!   the listed ids get positions 1..N in the given order, the rest follow
//!   in display order; the response is `{"reorder": true, "order": [ids]}`
//!   with string ids. Canvas's `reject!` 400 shape is
//!   `{"status": "bad_request", "message": ...}` for a missing/unknown/
//!   duplicated order. Reorder never touches `updated_at` (Canvas uses
//!   `update_all`).
//! - `GET .../messageable_users` lists the users the caller could message in
//!   the topic's context — active/invited enrollments for a course topic,
//!   accepted memberships for a group topic — as
//!   `conversation_user_json` (`{id, name, full_name, pronouns}`), ordered by
//!   sortable name, filtered by whitespace-separated search terms (every
//!   term must match) and paginated.
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
        "dsgc_{}_{}",
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

async fn seed_enrollment_state(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    role: &str,
    state: &str,
) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .bind(state)
    .execute(pool)
    .await
    .expect("seed enrollment state");
}

async fn seed_group(pool: &PgPool, prefix: &str, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed group")
}

async fn seed_membership(pool: &PgPool, group_id: i64, user_id: i64, state: &str, moderator: bool) {
    sqlx::query(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state, moderator) VALUES ($1, $2, $3, $4)",
    )
    .bind(group_id)
    .bind(user_id)
    .bind(state)
    .bind(moderator)
    .execute(pool)
    .await
    .expect("seed membership");
}

async fn seed_group_topic(
    pool: &PgPool,
    prefix: &str,
    course_id: i64,
    group_id: i64,
    title: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO discussion_topics (course_id, group_id, title, discussion_type, workflow_state)
        VALUES ($1, $2, $3, 'side_comment', 'active')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(group_id)
    .bind(format!("{prefix}-{title}"))
    .fetch_one(pool)
    .await
    .expect("seed group topic")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Delete every row created by this run, in FK-safe order. Topics, summaries
/// and memberships cascade from their parents.
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
    let _ = sqlx::query(
        "DELETE FROM group_memberships WHERE group_id IN
         (SELECT id FROM groups WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM groups WHERE name LIKE $1")
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
// Group-context topic CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn group_topic_crud_lifecycle_and_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let course = seed_course(&pool, &prefix, "CrudCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, member, course, "student").await;
    let group = seed_group(&pool, &prefix, course, "CrudGroup").await;
    seed_membership(&pool, group, member, "accepted", false).await;
    let base = format!("/groups/{group}/discussion_topics");

    // A parent-course teacher creates an unpinned topic.
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "title": format!("{prefix}-Teacher Topic"), "message": "hello" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let teacher_topic: Value = resp.json();
    assert_eq!(teacher_topic["title"], format!("{prefix}-Teacher Topic"));
    assert_eq!(teacher_topic["workflow_state"], "active");
    assert_eq!(teacher_topic["discussion_type"], "side_comment");
    assert_eq!(teacher_topic["pinned"], false);
    assert_eq!(teacher_topic["locked"], false);
    assert_eq!(teacher_topic["course_id"], course);
    let teacher_topic_id = teacher_topic["id"].as_i64().expect("topic id");

    // Canvas records the author (@topic.user = @current_user).
    let author: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM discussion_topics WHERE id = $1")
            .bind(teacher_topic_id)
            .fetch_one(&pool)
            .await
            .expect("topic row");
    assert_eq!(author, Some(teacher));

    // A plain member may not create topics (403)...
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .json(&json!({ "title": format!("{prefix}-Member Topic") }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // ...but the group leader may, and a pinned create lands at the bottom of
    // the pinned list (position 1 for the first pinned topic).
    let leader = seed_user(&pool, &prefix, "Leader", "student").await;
    seed_enrollment(&pool, leader, course, "student").await;
    seed_membership(&pool, group, leader, "accepted", true).await;
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(leader, "student"))
        .json(&json!({ "title": format!("{prefix}-Leader Topic"), "pinned": true }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let leader_topic: Value = resp.json();
    let leader_topic_id = leader_topic["id"].as_i64().expect("topic id");
    let position: Option<i64> =
        sqlx::query_scalar("SELECT position FROM discussion_topics WHERE id = $1")
            .bind(leader_topic_id)
            .fetch_one(&pool)
            .await
            .expect("topic row");
    assert_eq!(position, Some(1));

    // The member reads the index: Canvas by_position_legacy (position DESC,
    // created_at DESC, id DESC) — Postgres sorts NULL positions first on
    // DESC, so the unpinned teacher topic precedes the pinned leader topic.
    let resp = server
        .get(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Vec<Value> = resp.json();
    assert_eq!(list.len(), 2, "list: {list:?}");
    assert_eq!(list[0]["id"], json!(teacher_topic_id));
    assert_eq!(list[1]["id"], json!(leader_topic_id));

    // The teacher pins the unpinned topic: the flip resets the position to
    // the bottom of the pinned list (1 -> already taken, so 2) and the index
    // order flips with it.
    let resp = server
        .put(&format!("{base}/{teacher_topic_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "pinned": true, "locked": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["pinned"], true);
    assert_eq!(updated["locked"], true);
    let position: Option<i64> =
        sqlx::query_scalar("SELECT position FROM discussion_topics WHERE id = $1")
            .bind(teacher_topic_id)
            .fetch_one(&pool)
            .await
            .expect("topic row");
    assert_eq!(position, Some(2));

    let resp = server
        .get(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    let list: Vec<Value> = resp.json();
    assert_eq!(list[0]["id"], json!(teacher_topic_id));
    assert_eq!(list[1]["id"], json!(leader_topic_id));

    // Unpinning clears the position again (process_pin_parameters).
    let resp = server
        .put(&format!("{base}/{teacher_topic_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "pinned": false }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let position: Option<i64> =
        sqlx::query_scalar("SELECT position FROM discussion_topics WHERE id = $1")
            .bind(teacher_topic_id)
            .fetch_one(&pool)
            .await
            .expect("topic row");
    assert_eq!(position, None);

    // Destroy answers 200 with the destroyed topic plus the author's name
    // (Canvas as_json(include: { user: { only: :name } })).
    let resp = server
        .delete(&format!("{base}/{teacher_topic_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let destroyed: Value = resp.json();
    assert_eq!(destroyed["id"], json!(teacher_topic_id));
    assert_eq!(destroyed["workflow_state"], "deleted");
    assert_eq!(destroyed["user"]["name"], "Teacher");

    // The soft-deleted topic is invisible in the index...
    let resp = server
        .get(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    let list: Vec<Value> = resp.json();
    assert_eq!(list.len(), 1);

    // ...rejects updates with 404 (Canvas update scopes to active)...
    let resp = server
        .put(&format!("{base}/{teacher_topic_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "title": "nope" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // ...but destroy still finds it (Canvas all_discussion_topics).
    let resp = server
        .delete(&format!("{base}/{teacher_topic_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_topic_crud_permission_matrix() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "PermCourse").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, member, course, "student").await;
    seed_enrollment(&pool, outsider, course, "student").await;
    let group = seed_group(&pool, &prefix, course, "PermGroup").await;
    let other_group = seed_group(&pool, &prefix, other_course, "OtherGroup").await;
    seed_membership(&pool, group, member, "accepted", false).await;
    seed_membership(&pool, other_group, teacher, "accepted", false).await;
    let topic = seed_group_topic(&pool, &prefix, course, group, "PermTopic").await;
    let other_topic =
        seed_group_topic(&pool, &prefix, other_course, other_group, "ForeignTopic").await;
    let base = format!("/groups/{group}/discussion_topics");

    // No token: 401 before anything else runs.
    for (method, url) in [
        ("get", base.clone()),
        ("post", base.clone()),
        ("put", format!("{base}/{topic}")),
        ("delete", format!("{base}/{topic}")),
    ] {
        let resp = match method {
            "get" => server.get(&url).await,
            "post" => server.post(&url).json(&json!({ "title": "x" })).await,
            "put" => server.put(&url).json(&json!({ "title": "x" })).await,
            _ => server.delete(&url).await,
        };
        assert_eq!(resp.status_code(), 401, "{method} {url}");
    }

    // An enrolled non-member (outsider) has no group read access: 403.
    for (method, url) in [
        ("get", base.clone()),
        ("post", base.clone()),
        ("put", format!("{base}/{topic}")),
        ("delete", format!("{base}/{topic}")),
    ] {
        let header = auth(outsider, "student");
        let resp = match method {
            "get" => {
                server
                    .get(&url)
                    .add_header(axum::http::header::AUTHORIZATION, header)
                    .await
            }
            "post" => {
                server
                    .post(&url)
                    .add_header(axum::http::header::AUTHORIZATION, header)
                    .json(&json!({ "title": "x" }))
                    .await
            }
            "put" => {
                server
                    .put(&url)
                    .add_header(axum::http::header::AUTHORIZATION, header)
                    .json(&json!({ "title": "x" }))
                    .await
            }
            _ => {
                server
                    .delete(&url)
                    .add_header(axum::http::header::AUTHORIZATION, header)
                    .await
            }
        };
        assert_eq!(resp.status_code(), 403, "{method} {url}");
    }

    // A plain member reads but may not manage topics.
    let member_header = auth(member, "student");
    let resp = server
        .get(&base)
        .add_header(axum::http::header::AUTHORIZATION, member_header.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, member_header.clone())
        .json(&json!({ "title": format!("{prefix}-Member Topic") }))
        .await;
    assert_eq!(resp.status_code(), 403);
    let resp = server
        .put(&format!("{base}/{topic}"))
        .add_header(axum::http::header::AUTHORIZATION, member_header.clone())
        .json(&json!({ "title": "nope" }))
        .await;
    assert_eq!(resp.status_code(), 403);
    let resp = server
        .delete(&format!("{base}/{topic}"))
        .add_header(axum::http::header::AUTHORIZATION, member_header)
        .await;
    assert_eq!(resp.status_code(), 403);

    // A site admin manages without any enrollment or membership.
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "title": format!("{prefix}-Admin Topic") }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());

    // The parent-course teacher updates, including a topic they did not
    // author; a topic of another group is a 404.
    let teacher_header = auth(teacher, "teacher");
    let resp = server
        .put(&format!("{base}/{topic}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_header.clone())
        .json(&json!({ "title": format!("{prefix}-Renamed") }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let renamed: Value = resp.json();
    assert_eq!(renamed["title"], format!("{prefix}-Renamed"));

    let resp = server
        .put(&format!("{base}/{other_topic}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_header.clone())
        .json(&json!({ "title": "Foreign" }))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .delete(&format!("{base}/999999"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_header)
        .await;
    assert_eq!(resp.status_code(), 404);

    // Validation: an empty title and a bad discussion type are 422.
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "title": "   " }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "title": "Bad Type", "discussion_type": "round" }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_without_course_rejects_create() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let orphan: i64 =
        sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES (NULL, $1) RETURNING id")
            .bind(format!("{prefix}-OrphanGroup"))
            .fetch_one(&pool)
            .await
            .expect("seed orphan group");

    // Account-level groups are out of scope (migration 0018); creating a
    // topic must be a clean 422 rather than a 500 from the NOT NULL
    // course_id constraint.
    let resp = server
        .post(&format!("/groups/{orphan}/discussion_topics"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "title": "Orphan Topic" }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Reorder pinned topics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reorder_pinned_topics_course_context() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "OrderCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let base = format!("/courses/{course}/discussion_topics");

    // Three pinned topics created through the API take positions 1..3 in
    // creation order.
    let mut ids = Vec::new();
    for name in ["Alpha", "Bravo", "Charlie"] {
        let resp = server
            .post(&base)
            .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
            .json(&json!({ "title": format!("{prefix}-{name}"), "pinned": true }))
            .await;
        assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
        let topic: Value = resp.json();
        ids.push(topic["id"].as_i64().expect("topic id"));
    }
    let [alpha, bravo, charlie] = [ids[0], ids[1], ids[2]];

    let before: (chrono::DateTime<chrono::Utc>,) =
        sqlx::query_as("SELECT updated_at FROM discussion_topics WHERE id = $1")
            .bind(alpha)
            .fetch_one(&pool)
            .await
            .expect("topic row");

    // Reorder with the bracket form of the order parameter.
    let resp = server
        .post(&format!(
            "{base}/reorder?order[]={charlie}&order[]={alpha}&order[]={bravo}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["reorder"], true);
    assert_eq!(
        body["order"],
        json!([charlie.to_string(), alpha.to_string(), bravo.to_string()]),
        "body: {body}"
    );

    let positions: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, position FROM discussion_topics WHERE id = ANY($1) ORDER BY id")
            .bind(&ids)
            .fetch_all(&pool)
            .await
            .expect("positions");
    let by_id: std::collections::HashMap<i64, Option<i64>> = positions.into_iter().collect();
    assert_eq!(by_id.get(&alpha), Some(&Some(2)));
    assert_eq!(by_id.get(&bravo), Some(&Some(3)));
    assert_eq!(by_id.get(&charlie), Some(&Some(1)));

    // Canvas writes positions with update_all — updated_at is untouched.
    let after: (chrono::DateTime<chrono::Utc>,) =
        sqlx::query_as("SELECT updated_at FROM discussion_topics WHERE id = $1")
            .bind(alpha)
            .fetch_one(&pool)
            .await
            .expect("topic row");
    assert_eq!(before.0, after.0, "reorder must not touch updated_at");

    // A partial order (comma-separated form) puts the listed topics first and
    // keeps the remaining ones after, in display order.
    let resp = server
        .post(&format!("{base}/reorder?order={bravo},{charlie}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["order"],
        json!([bravo.to_string(), charlie.to_string(), alpha.to_string()]),
        "body: {body}"
    );

    // A student may not reorder.
    let resp = server
        .post(&format!("{base}/reorder?order={alpha}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // No token: 401.
    let resp = server.post(&format!("{base}/reorder?order={alpha}")).await;
    assert_eq!(resp.status_code(), 401);

    // Canvas reject! shapes: missing order, unknown id, duplicate id, an
    // unpinned topic id and a foreign topic id are all 400 "topics not found"
    // (or "order parameter required" when there is nothing to match).
    let teacher_header = auth(teacher, "teacher");
    let resp = server
        .post(&format!("{base}/reorder"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_header.clone())
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["message"], "order parameter required");

    for query in [
        "order=999999".to_string(),
        format!("order={alpha}&order[]={alpha}"),
        "order=abc".to_string(),
    ] {
        let resp = server
            .post(&format!("{base}/reorder?{query}"))
            .add_header(axum::http::header::AUTHORIZATION, teacher_header.clone())
            .await;
        assert_eq!(resp.status_code(), 400, "query: {query}");
        assert_eq!(resp.json::<Value>()["message"], "topics not found");
    }

    // An unpinned course topic is not reorderable.
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, teacher_header.clone())
        .json(&json!({ "title": format!("{prefix}-Unpinned") }))
        .await;
    let unpinned: Value = resp.json();
    let unpinned_id = unpinned["id"].as_i64().expect("topic id");
    let resp = server
        .post(&format!("{base}/reorder?order={unpinned_id}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_header.clone())
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.json::<Value>()["message"], "topics not found");

    // A pinned topic of another course is foreign.
    let other_course = seed_course(&pool, &prefix, "ForeignCourse").await;
    let resp = server
        .post(&format!("/courses/{other_course}/discussion_topics"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_header.clone())
        .json(&json!({ "title": format!("{prefix}-Foreign"), "pinned": true }))
        .await;
    let foreign: Value = resp.json();
    let foreign_id = foreign["id"].as_i64().expect("topic id");
    let resp = server
        .post(&format!("{base}/reorder?order={foreign_id}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_header)
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(resp.json::<Value>()["message"], "topics not found");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn reorder_pinned_topics_group_context() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let leader = seed_user(&pool, &prefix, "Leader", "student").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let course = seed_course(&pool, &prefix, "GroupOrderCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, leader, course, "student").await;
    seed_enrollment(&pool, member, course, "student").await;
    let group = seed_group(&pool, &prefix, course, "GroupOrderGroup").await;
    seed_membership(&pool, group, leader, "accepted", true).await;
    seed_membership(&pool, group, member, "accepted", false).await;
    let base = format!("/groups/{group}/discussion_topics");

    let leader_header = auth(leader, "student");
    let mut ids = Vec::new();
    for name in ["Alpha", "Bravo"] {
        let resp = server
            .post(&base)
            .add_header(axum::http::header::AUTHORIZATION, leader_header.clone())
            .json(&json!({ "title": format!("{prefix}-Group{name}"), "pinned": true }))
            .await;
        assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
        let topic: Value = resp.json();
        ids.push(topic["id"].as_i64().expect("topic id"));
    }
    let [alpha, bravo] = [ids[0], ids[1]];

    // A plain member may not reorder; the leader and the parent-course
    // teacher may.
    let resp = server
        .post(&format!("{base}/reorder?order={bravo}&order[]={alpha}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    let resp = server
        .post(&format!("{base}/reorder?order={bravo},{alpha}"))
        .add_header(axum::http::header::AUTHORIZATION, leader_header)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["order"],
        json!([bravo.to_string(), alpha.to_string()]),
        "body: {body}"
    );

    let resp = server
        .post(&format!("{base}/reorder?order={alpha},{bravo}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["order"],
        json!([alpha.to_string(), bravo.to_string()]),
        "body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Messageable users
// ---------------------------------------------------------------------------

#[tokio::test]
async fn messageable_users_course_context() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Zed Zulu", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice Archer", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob Baker", "student").await;
    let carol = seed_user(&pool, &prefix, "Carol Cee", "student").await;
    let dee = seed_user(&pool, &prefix, "Dee Dee", "student").await;
    let eve = seed_user(&pool, &prefix, "Eve Evil", "student").await;
    let course = seed_course(&pool, &prefix, "MsgCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    seed_enrollment_state(&pool, carol, course, "student", "completed").await;
    seed_enrollment_state(&pool, dee, course, "student", "deleted").await;

    let topic_id: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, title, discussion_type, workflow_state) VALUES ($1, $2, 'side_comment', 'active') RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-MsgTopic"))
    .fetch_one(&pool)
    .await
    .expect("seed topic");
    let url = format!("/courses/{course}/discussion_topics/{topic_id}/messageable_users");

    // Enrolled users see the active/invited enrollments of the course — the
    // caller included — but not completed, deleted or absent enrollments.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let users: Vec<Value> = resp.json();
    assert_eq!(
        users
            .iter()
            .map(|user| user["id"].as_i64().expect("id"))
            .collect::<Vec<i64>>(),
        vec![alice, bob, teacher],
        "users: {users:?}"
    );
    let shape = &users[0];
    assert_eq!(shape["name"], "Alice Archer");
    assert_eq!(shape["full_name"], "Alice Archer");
    assert!(shape.get("pronouns").is_some() && shape["pronouns"].is_null());

    // A sortable_name (Canvas' actual sort key) wins over the display name.
    sqlx::query("UPDATE users SET sortable_name = '000' WHERE id = $1")
        .bind(bob)
        .execute(&pool)
        .await
        .expect("set sortable name");
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    let users: Vec<Value> = resp.json();
    assert_eq!(
        users
            .iter()
            .map(|user| user["id"].as_i64().expect("id"))
            .collect::<Vec<i64>>(),
        vec![bob, alice, teacher]
    );

    // Search: case-insensitive, and every whitespace-separated term must
    // match the name.
    let resp = server
        .get(&format!("{url}?search=alice"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    let users: Vec<Value> = resp.json();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["id"], json!(alice));

    let resp = server
        .get(&format!("{url}?search=bob%20baker"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    let users: Vec<Value> = resp.json();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["id"], json!(bob));

    let resp = server
        .get(&format!("{url}?search=bob%20zed"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    let users: Vec<Value> = resp.json();
    assert!(users.is_empty(), "users: {users:?}");

    // Pagination.
    let resp = server
        .get(&format!("{url}?per_page=1&page=2"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    let users: Vec<Value> = resp.json();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["id"], json!(alice));

    // A user without course read access is a 403 (before the topic lookup).
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(eve, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // No token: 401.
    let resp = server.get(&url).await;
    assert_eq!(resp.status_code(), 401);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn messageable_users_group_context() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice Archer", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob Baker", "student").await;
    let carol = seed_user(&pool, &prefix, "Carol Cee", "student").await;
    let dee = seed_user(&pool, &prefix, "Dee Dee", "student").await;
    let outsider = seed_user(&pool, &prefix, "Eve Evil", "student").await;
    let course = seed_course(&pool, &prefix, "MsgGroupCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, outsider, course, "student").await;
    let group = seed_group(&pool, &prefix, course, "MsgGroup").await;
    seed_membership(&pool, group, alice, "accepted", false).await;
    seed_membership(&pool, group, bob, "accepted", true).await;
    seed_membership(&pool, group, carol, "invited", false).await;
    seed_membership(&pool, group, dee, "deleted", false).await;

    let topic_id: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, group_id, title, discussion_type, workflow_state) VALUES ($1, $2, $3, 'side_comment', 'active') RETURNING id",
    )
    .bind(course)
    .bind(group)
    .bind(format!("{prefix}-MsgGroupTopic"))
    .fetch_one(&pool)
    .await
    .expect("seed topic");
    let url = format!("/groups/{group}/discussion_topics/{topic_id}/messageable_users");

    // Only accepted memberships count (Canvas group_user_scope), the caller
    // included, ordered by sortable name.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let users: Vec<Value> = resp.json();
    assert_eq!(
        users
            .iter()
            .map(|user| user["id"].as_i64().expect("id"))
            .collect::<Vec<i64>>(),
        vec![alice, bob],
        "users: {users:?}"
    );

    // The parent-course teacher (not a member) can call the route.
    let resp = server
        .get(&format!("{url}?search=bob"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let users: Vec<Value> = resp.json();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["id"], json!(bob));

    // An enrolled non-member gets 403; an unknown or foreign topic is 404.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    let resp = server
        .get(&format!(
            "/groups/{group}/discussion_topics/999999/messageable_users"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // A course topic (no group) is foreign to the group context.
    let course_topic: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, title, discussion_type, workflow_state) VALUES ($1, $2, 'side_comment', 'active') RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-CourseTopic"))
    .fetch_one(&pool)
    .await
    .expect("seed course topic");
    let resp = server
        .get(&format!(
            "/groups/{group}/discussion_topics/{course_topic}/messageable_users"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // A soft-deleted group topic is 404; an empty search result stays 200.
    let deleted_topic: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, group_id, title, discussion_type, workflow_state) VALUES ($1, $2, $3, 'side_comment', 'deleted') RETURNING id",
    )
    .bind(course)
    .bind(group)
    .bind(format!("{prefix}-DeletedTopic"))
    .fetch_one(&pool)
    .await
    .expect("seed deleted topic");
    let resp = server
        .get(&format!(
            "/groups/{group}/discussion_topics/{deleted_topic}/messageable_users"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    let resp = server
        .get(&format!("{url}?search=zzz"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let users: Vec<Value> = resp.json();
    assert!(users.is_empty());

    // No token: 401.
    let resp = server.get(&url).await;
    assert_eq!(resp.status_code(), 401);

    cleanup(&pool, &prefix).await;
}
