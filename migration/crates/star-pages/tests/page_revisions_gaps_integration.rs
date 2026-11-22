//! Integration tests for the group-context wiki page revision routes
//! (P5 gap): GET /groups/{groupId}/pages/{urlOrId}/revisions,
//! .../revisions/latest, .../revisions/{revisionId} and the POST revert.
//!
//! The routes run Canvas' context and read/manage checks (group member read,
//! parent-course teacher manage, admin both) and then decline with 403 —
//! wiki pages are course-scoped in this schema (`pages` has no group
//! column), exactly like the sibling group wiki routes pinned in
//! tests/pages_groups.rs. Unknown groups answer 404.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-pages

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "pagrev-tests-{}-{}",
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

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Delete every row created by this run, in FK-safe order.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM group_memberships WHERE group_id IN \
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
// Group revision routes (declined in this schema)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn group_page_revision_routes_decline_without_group_wiki() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "RevGroupCourse").await;
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
    let page_ref = format!("{prefix}-irrelevant");

    // Reads: a group member, the parent-course teacher and an admin all pass
    // the context+read gates — the revision routes still decline with 403
    // (no page can belong to a group in this schema).
    for url in [
        format!("/groups/{group_id}/pages/{page_ref}/revisions"),
        format!("/groups/{group_id}/pages/{page_ref}/revisions/latest"),
        format!("/groups/{group_id}/pages/{page_ref}/revisions/1"),
    ] {
        for bearer in [&member_bearer, &teacher_bearer, &admin_bearer] {
            let resp = server
                .get(&url)
                .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
                .await;
            assert_eq!(resp.status_code(), 403, "{url}; body: {}", resp.text());
        }
    }

    // Revert (POST): Canvas requires :read_revisions and :update, i.e. group
    // manage rights — the parent-course teacher and an admin pass the gate
    // and still get the 403 decline.
    let revert_url = format!("/groups/{group_id}/pages/{page_ref}/revisions/1");
    for bearer in [&teacher_bearer, &admin_bearer] {
        let resp = server
            .post(&revert_url)
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    }

    // Unknown group: Canvas' context check answers 404 before anything else.
    for url in [
        format!("/groups/999999999/pages/{page_ref}/revisions"),
        format!("/groups/999999999/pages/{page_ref}/revisions/latest"),
        format!("/groups/999999999/pages/{page_ref}/revisions/1"),
    ] {
        let resp = server
            .get(&url)
            .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 404, "{url}; body: {}", resp.text());
        let _ = &resp.json::<Value>();
    }
    let resp = server
        .post("/groups/999999999/pages/x/revisions/1")
        .add_header(axum::http::header::AUTHORIZATION, admin_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
