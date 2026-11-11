//! Integration tests for the external tools (Canvas `ContextExternalTool`)
//! routes added by migration 0085.
//!
//! Pinned behavior (Canvas API v1 parity):
//! - `POST /courses/{course_id}/external_tools` requires course-teacher /
//!   account-admin / global-admin; an enrolled student gets 403. A successful
//!   create answers 200 with the Canvas tool JSON (including `is_rce_favorite`).
//! - Create validation returns Canvas' `400 {"errors": [...]}` for a blank
//!   name and for an unknown `privacy_level`.
//! - `POST /accounts/{account_id}/external_tools/rce_favorites/{id}` answers
//!   `400 {"message": ...}` when the tool has no `editor_button` placement,
//!   otherwise 200 with `rce_favorite_tool_ids`. `DELETE` returns the same
//!   shape with the tool removed.
//! - `GET /external_tools/visible_course_nav_tools` returns a flat array with
//!   `context_name` added, rejects a non-`course_` context code with 400, and
//!   404s for an unknown course. An unknown tool id 404s; a non-member reading
//!   a course tool gets 403.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix plus
//! full teardown so repeat runs against a shared DB are safe.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "xt_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

async fn setup() -> Option<(TestServer, PgPool, String)> {
    let db_url = match std::env::var("DATABASE_URL") {
        Ok(url) => url,
        Err(_) => {
            println!("SKIP: DATABASE_URL not set");
            return None;
        }
    };

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

    let server = TestServer::new(star_lti::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) \
         RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
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
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
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

async fn seed_account_user(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account user");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn teardown(pool: &PgPool, user_ids: &[i64], account_id: i64, course_id: i64) {
    for user_id in user_ids {
        sqlx::query("DELETE FROM external_tools WHERE created_by_id = $1")
            .bind(user_id)
            .execute(pool)
            .await
            .ok();
    }
    sqlx::query("DELETE FROM external_tool_favorites WHERE context_id IN ($1, $2)")
        .bind(account_id)
        .bind(course_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM enrollments WHERE course_id = $1")
        .bind(course_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM courses WHERE id = $1")
        .bind(course_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM account_users WHERE account_id = $1")
        .bind(account_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM external_tools WHERE account_id = $1")
        .bind(account_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(account_id)
        .execute(pool)
        .await
        .ok();
    for user_id in user_ids {
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await
            .ok();
    }
}

async fn create_course_tool(
    server: &TestServer,
    course_id: i64,
    bearer: &str,
    body: Value,
) -> axum_test::TestResponse {
    server
        .post(&format!("/courses/{course_id}/external_tools"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&body)
        .await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn teacher_creates_course_tool_student_is_forbidden() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student_id = seed_user(&pool, &prefix, "student", "student").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;
    seed_enrollment(&pool, student_id, course_id, "student").await;

    let resp = create_course_tool(
        &server,
        course_id,
        &auth(teacher_id, "teacher"),
        json!({ "name": "Test Tool", "url": "https://tool.example.com" }),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tool: Value = resp.json();
    assert_eq!(tool["name"], "Test Tool");
    assert_eq!(tool["privacy_level"], "anonymous");
    assert_eq!(tool["context_type"], "Course");
    assert_eq!(tool["context_id"], json!(course_id));
    assert_eq!(tool["workflow_state"], "active");
    assert_eq!(tool["is_rce_favorite"], false);

    let resp = create_course_tool(
        &server,
        course_id,
        &auth(student_id, "student"),
        json!({ "name": "Nope", "url": "https://tool.example.com" }),
    )
    .await;
    assert_eq!(resp.status_code(), 403);

    teardown(&pool, &[teacher_id, student_id], account_id, course_id).await;
}

#[tokio::test]
async fn create_validates_name_and_privacy_level() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;
    let bearer = auth(teacher_id, "teacher");

    let resp = create_course_tool(
        &server,
        course_id,
        &bearer,
        json!({ "url": "https://tool.example.com" }),
    )
    .await;
    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(body["errors"], json!(["Name can't be blank"]));

    let resp = create_course_tool(
        &server,
        course_id,
        &bearer,
        json!({
            "name": "Bad Privacy",
            "url": "https://tool.example.com",
            "privacy_level": "secret"
        }),
    )
    .await;
    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(
        body["errors"],
        json!(["Privacy level is not included in the list: secret"])
    );

    teardown(&pool, &[teacher_id], account_id, course_id).await;
}

#[tokio::test]
async fn rce_favorite_requires_editor_button_placement() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let admin_id = seed_user(&pool, &prefix, "acctadmin", "student").await;
    seed_account_user(&pool, account_id, admin_id).await;
    let bearer = auth(admin_id, "student");

    let resp = server
        .post(&format!("/accounts/{account_id}/external_tools"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "name": "No Placement", "url": "https://tool.example.com" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let plain: Value = resp.json();
    let plain_id = plain["id"].as_i64().expect("id");

    let resp = server
        .post(&format!(
            "/accounts/{account_id}/external_tools/rce_favorites/{plain_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400);
    let body: Value = resp.json();
    assert_eq!(
        body["message"],
        "Tool does not have an editor_button placement"
    );

    let resp = server
        .post(&format!("/accounts/{account_id}/external_tools"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({
            "name": "Editor Tool",
            "url": "https://tool.example.com/editor",
            "tool_configuration": { "placements": { "editor_button": {} } }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let editor: Value = resp.json();
    let editor_id = editor["id"].as_i64().expect("id");

    let resp = server
        .post(&format!(
            "/accounts/{account_id}/external_tools/rce_favorites/{editor_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["rce_favorite_tool_ids"], json!([editor_id]));

    let resp = server
        .delete(&format!(
            "/accounts/{account_id}/external_tools/rce_favorites/{editor_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["rce_favorite_tool_ids"], json!([]));

    teardown(&pool, &[admin_id], account_id, course_id).await;
}

#[tokio::test]
async fn visible_nav_tools_shape_and_error_cases() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let outsider_id = seed_user(&pool, &prefix, "outsider", "student").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;
    let bearer = auth(teacher_id, "teacher");

    let resp = create_course_tool(
        &server,
        course_id,
        &bearer,
        json!({ "name": "Nav Tool", "url": "https://tool.example.com/nav" }),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tool: Value = resp.json();
    let tool_id = tool["id"].as_i64().expect("id");

    let resp = server
        .get("/external_tools/visible_course_nav_tools")
        .add_query_params([("context_codes[]", format!("course_{course_id}"))])
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let data: Value = resp.json();
    let item = data
        .as_array()
        .expect("array")
        .iter()
        .find(|item| item["id"] == json!(tool_id))
        .expect("nav tool present");
    assert_eq!(item["context_id"], json!(course_id));
    assert!(item["context_name"].is_string());

    let resp = server
        .get("/external_tools/visible_course_nav_tools")
        .add_query_params([("context_codes[]", "account_1".to_string())])
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400);

    let resp = server
        .get("/external_tools/visible_course_nav_tools")
        .add_query_params([("context_codes[]", "course_999999999".to_string())])
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    let resp = server
        .get(&format!("/courses/{course_id}/external_tools/{tool_id}"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(outsider_id, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403);

    let resp = server
        .get(&format!("/courses/{course_id}/external_tools/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    teardown(&pool, &[teacher_id, outsider_id], account_id, course_id).await;
}
