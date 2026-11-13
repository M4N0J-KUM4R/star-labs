//! Integration tests for star-pages: update toggles, revert-to-revision and
//! the group wiki routes.
//!
//! - PUT /pages/{id} accepts `published` and the deprecated `hidden`
//!   booleans with Canvas precedence published > hidden > workflow_state
//!   ('active' counts as 'published'); a toggle alone is a valid update.
//! - POST /courses/{courseId}/pages/{urlOrId}/revisions/{revisionId} restores
//!   that revision's title/body/url, appends the restored state as a NEW
//!   revision and answers the current version's revision JSON with content;
//!   unknown revisions are 404 and students 403.
//! - The /groups/{groupId} pages routes run Canvas' context+auth checks
//!   (group member read / parent-course teacher manage / admin) and then
//!   decline with 403: wiki pages are course-scoped in this schema (`pages`
//!   has no group column), mirroring the star-discussions group-summaries
//!   pattern. Unknown groups answer 404.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-pages

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "paggrp-tests-{}-{}",
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
    let server = TestServer::new(star_pages::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
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
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
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

async fn seed_page(pool: &PgPool, course_id: i64, title: &str, url: &str, body: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO pages (course_id, title, body, url) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(body)
    .bind(url)
    .fetch_one(pool)
    .await
    .expect("seed page")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Delete every row created by this run, in FK-safe order.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM group_memberships WHERE group_id IN
         (SELECT id FROM groups WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM groups WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// PUT /pages/{id} hidden/published toggles
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_page_hidden_and_published_toggles() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ToggleCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let page = seed_page(
        &pool,
        course,
        &format!("{prefix}-Toggle"),
        &format!("{prefix}-toggle"),
        "toggle-body",
    )
    .await;
    let bearer = auth(teacher, "teacher");
    let url = format!("/pages/{page}");

    // `published` is the boolean form of workflow_state.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "published": false }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "unpublished");

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "published": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "published");

    // `hidden` is Canvas' deprecated inverse of `published`.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "hidden": true }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "unpublished");

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "hidden": false }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "published");

    // Canvas precedence: published > hidden > workflow_state, and
    // workflow_state 'active' means 'published'.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({
            "published": true,
            "hidden": true,
            "workflow_state": "unpublished"
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "published");

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "hidden": true, "workflow_state": "published" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "unpublished");

    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "workflow_state": "active" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "published");

    // A toggle-only update is not an empty payload, and untouched fields
    // survive it.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let live: Value = resp.json();
    assert_eq!(live["title"], format!("{prefix}-Toggle"));
    assert_eq!(live["body"], "toggle-body");

    // Students still cannot toggle the page.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "published": true }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST .../revisions/{revisionId} (revert)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn revert_restores_prior_revision() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "RevertCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let teacher_bearer = auth(teacher, "teacher");

    // POST creates revision 1; the PUTs append revisions 2 and 3.
    let resp = server
        .post(&format!("/courses/{course}/pages"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "title": "Revert Page", "body": "v1" }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let page: Value = resp.json();
    let page_id = page["id"].as_i64().expect("page id");

    let resp = server
        .put(&format!("/pages/{page_id}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "title": "Revert Page Two", "body": "v2" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .put(&format!("/pages/{page_id}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "body": "v3" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Revert to revision 1: the answer is the NEW current revision (4),
    // including content, edited by the caller.
    let revert_url = format!("/courses/{course}/pages/{page_id}/revisions/1");
    let resp = server
        .post(&revert_url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let reverted: Value = resp.json();
    assert_eq!(reverted["revision_id"], 4);
    assert_eq!(reverted["latest"], true);
    assert_eq!(reverted["title"], "Revert Page");
    assert_eq!(reverted["body"], "v1");
    assert_eq!(reverted["url"], "revert-page");
    assert_eq!(reverted["edited_by"]["id"], teacher);

    // The live page carries the restored state (title, body AND url slug).
    let resp = server
        .get(&format!("/pages/{page_id}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let live: Value = resp.json();
    assert_eq!(live["title"], "Revert Page");
    assert_eq!(live["body"], "v1");
    assert_eq!(live["url"], "revert-page");

    // History was appended, not rewritten: 4 revisions, newest first.
    let resp = server
        .get(&format!("/courses/{course}/pages/revert-page/revisions"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let history: Value = resp.json();
    let items = history.as_array().expect("history array");
    assert_eq!(
        items.len(),
        4,
        "create + two updates + revert; body: {history}"
    );
    assert_eq!(items[0]["revision_id"], 4);
    assert_eq!(items[1]["revision_id"], 3);
    assert_eq!(items[2]["revision_id"], 2);
    assert_eq!(items[3]["revision_id"], 1);
    assert_eq!(items[0]["latest"], true);
    assert_eq!(items[3]["latest"], false);

    // Unknown revisions are 404, and students may not revert.
    let resp = server
        .post(&format!("/courses/{course}/pages/{page_id}/revisions/99"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .post(&revert_url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// /groups/{groupId} wiki routes (declined in this schema)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn group_page_routes_decline_without_group_wiki() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "GroupCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, member, course, "student").await;

    let group_id: i64 =
        sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
            .bind(course)
            .bind(format!("{prefix}-Group"))
            .fetch_one(&pool)
            .await
            .expect("seed group");

    sqlx::query(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state)
         VALUES ($1, $2, 'accepted')",
    )
    .bind(group_id)
    .bind(member)
    .execute(&pool)
    .await
    .expect("seed membership");

    let member_bearer = auth(member, "student");
    let teacher_bearer = auth(teacher, "teacher");
    let admin_bearer = auth(admin, "admin");

    // Reads: a group member, the parent-course teacher and an admin all pass
    // Canvas' context+read checks — the routes still decline with 403
    // (pages are course-scoped in this schema).
    for url in [
        format!("/groups/{group_id}/pages"),
        format!("/groups/{group_id}/pages/{prefix}-irrelevant"),
        format!("/groups/{group_id}/front_page"),
    ] {
        for bearer in [&member_bearer, &teacher_bearer, &admin_bearer] {
            let resp = server
                .get(&url)
                .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
                .await;
            assert_eq!(resp.status_code(), 403, "{url}; body: {}", resp.text());
        }
    }

    // Writes: the parent-course teacher holds manage rights — declined too.
    for url in [
        format!("/groups/{group_id}/pages"),
        format!("/groups/{group_id}/pages/{prefix}-irrelevant"),
    ] {
        let resp = server
            .post(&url)
            .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 403, "{url}; body: {}", resp.text());
    }
    for url in [
        format!("/groups/{group_id}/pages/{prefix}-irrelevant"),
        format!("/groups/{group_id}/front_page"),
    ] {
        let resp = server
            .put(&url)
            .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 403, "{url}; body: {}", resp.text());
    }
    let resp = server
        .delete(&format!("/groups/{group_id}/pages/{prefix}-irrelevant"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A group member does not hold manage rights: 403 from the auth check.
    let resp = server
        .post(&format!("/groups/{group_id}/pages"))
        .add_header(axum::http::header::AUTHORIZATION, member_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An outsider fails the read check as well.
    let resp = server
        .get(&format!("/groups/{group_id}/pages"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unknown group context: 404 (Canvas `get_context` -> Group.find), even
    // for admins.
    for url in [
        "/groups/999999999/pages".to_string(),
        format!("/groups/999999999/pages/{prefix}-irrelevant"),
        "/groups/999999999/front_page".to_string(),
    ] {
        let resp = server
            .get(&url)
            .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 404, "{url}; body: {}", resp.text());
    }

    cleanup(&pool, &prefix).await;
}
