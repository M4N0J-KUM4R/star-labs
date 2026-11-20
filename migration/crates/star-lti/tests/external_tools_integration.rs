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

// -----------------------------------------------------------------------------
// New route tests: update / delete
// ---------------------------------------------------------------------------

async fn update_course_tool(
    server: &TestServer,
    course_id: i64,
    tool_id: i64,
    bearer: &str,
    body: Value,
) -> axum_test::TestResponse {
    server
        .put(&format!("/courses/{course_id}/external_tools/{tool_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&body)
        .await
}

async fn delete_course_tool(
    server: &TestServer,
    course_id: i64,
    tool_id: i64,
    bearer: &str,
) -> axum_test::TestResponse {
    server
        .delete(&format!("/courses/{course_id}/external_tools/{tool_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .await
}

async fn update_account_tool(
    server: &TestServer,
    account_id: i64,
    tool_id: i64,
    bearer: &str,
    body: Value,
) -> axum_test::TestResponse {
    server
        .put(&format!("/accounts/{account_id}/external_tools/{tool_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&body)
        .await
}

async fn delete_account_tool(
    server: &TestServer,
    account_id: i64,
    tool_id: i64,
    bearer: &str,
) -> axum_test::TestResponse {
    server
        .delete(&format!("/accounts/{account_id}/external_tools/{tool_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .await
}

#[tokio::test]
async fn update_and_delete_course_tool() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student_id = seed_user(&pool, &prefix, "student", "student").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;
    seed_enrollment(&pool, student_id, course_id, "student").await;

    // Student cannot update
    let resp = update_course_tool(
        &server,
        course_id,
        1,
        &auth(student_id, "student"),
        json!({ "name": "Hacked" }),
    )
    .await;
    assert_eq!(resp.status_code(), 403);

    // Create a tool
    let resp = create_course_tool(
        &server,
        course_id,
        &auth(teacher_id, "teacher"),
        json!({ "name": "Original", "url": "https://tool.example.com" }),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    let tool: Value = resp.json();
    let tool_id = tool["id"].as_i64().unwrap();

    // Update it
    let resp = update_course_tool(
        &server,
        course_id,
        tool_id,
        &auth(teacher_id, "teacher"),
        json!({ "name": "Updated Name", "description": "New desc" }),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["name"], "Updated Name");
    assert_eq!(updated["description"], "New desc");
    assert!(updated["updated_at"].as_str().is_some());

    // Student cannot delete
    let resp = delete_course_tool(&server, course_id, tool_id, &auth(student_id, "student")).await;
    assert_eq!(resp.status_code(), 403);

    // Delete it
    let resp = delete_course_tool(&server, course_id, tool_id, &auth(teacher_id, "teacher")).await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Verify gone
    let resp = server
        .get(&format!("/courses/{course_id}/external_tools/{tool_id}"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404);

    teardown(&pool, &[teacher_id, student_id], account_id, course_id).await;
}

#[tokio::test]
async fn update_and_delete_account_tool() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let admin_id = seed_user(&pool, &prefix, "admin", "student").await;
    let outsider_id = seed_user(&pool, &prefix, "outsider", "student").await;
    seed_account_user(&pool, account_id, admin_id).await;

    // Non-admin cannot update
    let resp = update_account_tool(
        &server,
        account_id,
        1,
        &auth(outsider_id, "student"),
        json!({ "name": "Nope" }),
    )
    .await;
    assert_eq!(resp.status_code(), 403);

    // Create tool
    let resp = server
        .post(&format!("/accounts/{account_id}/external_tools"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_id, "student"))
        .json(&json!({ "name": "Original", "url": "https://tool.example.com" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let tool: Value = resp.json();
    let tool_id = tool["id"].as_i64().unwrap();

    // Update
    let resp = update_account_tool(
        &server,
        account_id,
        tool_id,
        &auth(admin_id, "student"),
        json!({ "name": "Updated Account Tool" }),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["name"], "Updated Account Tool");

    // Delete
    let resp = delete_account_tool(&server, account_id, tool_id, &auth(admin_id, "student")).await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Verify gone
    let resp = server
        .get(&format!("/accounts/{account_id}/external_tools"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_id, "student"))
        .await;
    let tools: Value = resp.json();
    let ids: Vec<i64> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["id"].as_i64())
        .collect();
    assert!(!ids.contains(&tool_id), "tool should be deleted");

    teardown(&pool, &[admin_id, outsider_id], account_id, 0).await;
}

// ---------------------------------------------------------------------------
// New route tests: create_tool_with_verification
// ---------------------------------------------------------------------------

async fn create_course_tool_with_verification(
    server: &TestServer,
    course_id: i64,
    bearer: &str,
    body: Value,
) -> axum_test::TestResponse {
    server
        .post(&format!(
            "/courses/{course_id}/create_tool_with_verification"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&body)
        .await
}

async fn create_account_tool_with_verification(
    server: &TestServer,
    account_id: i64,
    bearer: &str,
    body: Value,
) -> axum_test::TestResponse {
    server
        .post(&format!(
            "/accounts/{account_id}/create_tool_with_verification"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&body)
        .await
}

#[tokio::test]
async fn create_course_tool_with_verification_returns_tool() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;

    let resp = create_course_tool_with_verification(
        &server,
        course_id,
        &auth(teacher_id, "teacher"),
        json!({
            "name": "Verified Tool",
            "url": "https://tool.example.com/lti",
            "privacy_level": "anonymous",
            "verification_url": "https://tool.example.com/lti/oidc/init"
        }),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tool: Value = resp.json();
    assert_eq!(tool["name"], "Verified Tool");
    assert_eq!(tool["workflow_state"], "active");

    teardown(&pool, &[teacher_id], account_id, course_id).await;
}

#[tokio::test]
async fn create_account_tool_with_verification_returns_tool() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let admin_id = seed_user(&pool, &prefix, "admin", "student").await;
    seed_account_user(&pool, account_id, admin_id).await;

    let resp = create_account_tool_with_verification(
        &server,
        account_id,
        &auth(admin_id, "student"),
        json!({
            "name": "Verified Account Tool",
            "url": "https://tool.example.com/lti",
            "privacy_level": "public",
            "verification_url": "https://tool.example.com/lti/oidc/init"
        }),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tool: Value = resp.json();
    assert_eq!(tool["name"], "Verified Account Tool");

    teardown(&pool, &[admin_id], account_id, 0).await;
}

// ---------------------------------------------------------------------------
// New route tests: sessionless_launch (stub → 503)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sessionless_launch_returns_503() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;
    let bearer = auth(teacher_id, "teacher");

    let resp = server
        .post(&format!(
            "/courses/{course_id}/external_tools/sessionless_launch?url=https://tool.example.com"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 503);

    let admin_id = seed_user(&pool, &prefix, "admin", "student").await;
    seed_account_user(&pool, account_id, admin_id).await;
    let resp = server
        .post(&format!(
            "/accounts/{account_id}/external_tools/sessionless_launch?url=https://tool.example.com"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_id, "student"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 503);

    teardown(&pool, &[teacher_id, admin_id], account_id, course_id).await;
}

// ---------------------------------------------------------------------------
// New route tests: top_nav_favorites
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_top_nav_favorites_get_add_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;
    let bearer = auth(teacher_id, "teacher");

    // Create a tool with top_nav placement
    let resp = create_course_tool(
        &server,
        course_id,
        &bearer,
        json!({
            "name": "Top Nav Tool",
            "url": "https://tool.example.com",
            "tool_configuration": { "placements": { "top_navigation": {} } }
        }),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    let tool: Value = resp.json();
    let tool_id = tool["id"].as_i64().unwrap();

    // GET list (empty)
    let resp = server
        .get(&format!(
            "/courses/{course_id}/external_tools/top_nav_favorites"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert!(list["top_nav_favorite_tool_ids"]
        .as_array()
        .unwrap()
        .is_empty());

    // Add favorite
    let resp = server
        .post(&format!(
            "/courses/{course_id}/external_tools/top_nav_favorites/{tool_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let fav: Value = resp.json();
    assert!(fav["top_nav_favorite_tool_ids"]
        .as_array()
        .unwrap()
        .contains(&json!(tool_id)));

    // Duplicate add → 400
    let resp = server
        .post(&format!(
            "/courses/{course_id}/external_tools/top_nav_favorites/{tool_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 400);

    // Remove
    let resp = server
        .delete(&format!(
            "/courses/{course_id}/external_tools/top_nav_favorites/{tool_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let after: Value = resp.json();
    assert!(after["top_nav_favorite_tool_ids"]
        .as_array()
        .unwrap()
        .is_empty());

    teardown(&pool, &[teacher_id], account_id, course_id).await;
}

#[tokio::test]
async fn account_top_nav_favorites_get_add_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let admin_id = seed_user(&pool, &prefix, "admin", "student").await;
    seed_account_user(&pool, account_id, admin_id).await;

    // Create tool
    let resp = server
        .post(&format!("/accounts/{account_id}/external_tools"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_id, "student"))
        .json(&json!({
            "name": "Account Top Nav",
            "url": "https://tool.example.com",
            "tool_configuration": { "placements": { "top_navigation": {} } }
        }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let tool: Value = resp.json();
    let tool_id = tool["id"].as_i64().unwrap();

    // GET
    let resp = server
        .get(&format!(
            "/accounts/{account_id}/external_tools/top_nav_favorites"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_id, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert!(resp.json()["top_nav_favorite_tool_ids"]
        .as_array()
        .unwrap()
        .is_empty());

    // Add
    let resp = server
        .post(&format!(
            "/accounts/{account_id}/external_tools/top_nav_favorites/{tool_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_id, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Delete
    let resp = server
        .delete(&format!(
            "/accounts/{account_id}/external_tools/top_nav_favorites/{tool_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_id, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    teardown(&pool, &[admin_id], account_id, 0).await;
}

// ---------------------------------------------------------------------------
// New route tests: migration_info
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_and_account_migration_info() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;
    let admin_id = seed_user(&pool, &prefix, "admin", "student").await;
    seed_account_user(&pool, account_id, admin_id).await;

    // Create tools
    let resp = create_course_tool(
        &server,
        course_id,
        &auth(teacher_id, "teacher"),
        json!({ "name": "Course Migration Tool", "url": "https://tool.example.com" }),
    )
    .await;
    let course_tool: Value = resp.json();
    let course_tool_id = course_tool["id"].as_i64().unwrap();

    let resp = server
        .post(&format!("/accounts/{account_id}/external_tools"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_id, "student"))
        .json(&json!({ "name": "Account Migration Tool", "url": "https://tool.example.com" }))
        .await;
    let account_tool: Value = resp.json();
    let account_tool_id = account_tool["id"].as_i64().unwrap();

    // Course migration info
    let resp = server
        .get(&format!(
            "/courses/{course_id}/external_tools/{course_tool_id}/migration"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let info: Value = resp.json();
    assert!(info["id"].is_number());
    assert!(info["created_at"].is_string());

    // Account migration info
    let resp = server
        .get(&format!(
            "/accounts/{account_id}/external_tools/{account_tool_id}/migration"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin_id, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    teardown(&pool, &[teacher_id, admin_id], account_id, course_id).await;
}

// ---------------------------------------------------------------------------
// New route tests: group tools
// ---------------------------------------------------------------------------

async fn seed_group(pool: &PgPool, prefix: &str, name: &str, course_id: i64) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (name, course_id) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(course_id)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

#[tokio::test]
async fn group_tools_returns_tools_with_context_id() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;
    let group_id = seed_group(&pool, &prefix, "group", course_id).await;
    let bearer = auth(teacher_id, "teacher");

    // Create a course tool visible to the group
    let resp = create_course_tool(
        &server,
        course_id,
        &bearer,
        json!({ "name": "Group Tool", "url": "https://tool.example.com" }),
    )
    .await;
    assert_eq!(resp.status_code(), 200);

    let resp = server
        .get(&format!("/groups/{group_id}/external_tools"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tools: Value = resp.json();
    let tool_names: Vec<&str> = tools
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        tool_names.contains(&"Group Tool"),
        "should contain Group Tool, got {tool_names:?}"
    );

    // Show single group tool
    let resp = create_course_tool(
        &server,
        course_id,
        &bearer,
        json!({ "name": "Visible Group Tool", "url": "https://tool.example.com/vg" }),
    )
    .await;
    let tool: Value = resp.json();
    let tool_id = tool["id"].as_i64().unwrap();

    let resp = server
        .get(&format!("/groups/{group_id}/external_tools/{tool_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["name"], "Visible Group Tool");

    teardown(&pool, &[teacher_id], account_id, course_id).await;
}

// ---------------------------------------------------------------------------
// New route tests: eula_launch (stub → 503)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn eula_launch_returns_503() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;

    let resp = create_course_tool(
        &server,
        course_id,
        &auth(teacher_id, "teacher"),
        json!({ "name": "Eula Tool", "url": "https://tool.example.com" }),
    )
    .await;
    let tool: Value = resp.json();
    let tool_id = tool["id"].as_i64().unwrap();

    let resp = server
        .get(&format!("/external_tools/{tool_id}/eula_accepted"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 503);

    teardown(&pool, &[teacher_id], account_id, course_id).await;
}

// ---------------------------------------------------------------------------
// New route tests: visible_course_nav_tools includes placements
// ---------------------------------------------------------------------------

#[tokio::test]
async fn visible_nav_tools_includes_placements() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;

    let resp = create_course_tool(
        &server,
        course_id,
        &auth(teacher_id, "teacher"),
        json!({
            "name": "Placed Tool",
            "url": "https://tool.example.com",
            "tool_configuration": {
                "placements": {
                    "course_navigation": {},
                    "global_navigation": {}
                }
            }
        }),
    )
    .await;
    assert_eq!(resp.status_code(), 200);

    let resp = server
        .get("/external_tools/visible_course_nav_tools")
        .add_query_params([("context_codes[]", format!("course_{course_id}"))])
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tools: Value = resp.json();
    let placed = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Placed Tool");
    assert!(
        placed.is_some(),
        "Placed Tool should be in visible nav tools"
    );
    let placed = placed.unwrap();
    let placements = placed["placements"].clone();
    assert!(
        placements.is_array(),
        "placements should be an array, got {placements}"
    );
    assert!(
        placements
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["placement"] == "course_navigation" || p["enabled"] == true),
        "should have course_navigation placement"
    );

    teardown(&pool, &[teacher_id], account_id, course_id).await;
}

// ---------------------------------------------------------------------------
// New route tests: update respects updated_only flag
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_course_tool_updated_only() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;

    let resp = create_course_tool(
        &server,
        course_id,
        &auth(teacher_id, "teacher"),
        json!({
            "name": "Orig Name",
            "description": "Orig Desc",
            "url": "https://tool.example.com"
        }),
    )
    .await;
    let tool: Value = resp.json();
    let tool_id = tool["id"].as_i64().unwrap();
    let orig_updated = tool["updated_at"].as_str().unwrap();

    // Update with updated_only=false (default): all fields merged
    let resp = update_course_tool(
        &server,
        course_id,
        tool_id,
        &auth(teacher_id, "teacher"),
        json!({ "name": "New Name", "updated_only": false }),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    let updated: Value = resp.json();
    assert_eq!(updated["name"], "New Name");

    // Update with updated_only=true: only supplied fields
    let resp = update_course_tool(
        &server,
        course_id,
        tool_id,
        &auth(teacher_id, "teacher"),
        json!({ "description": "New Desc", "updated_only": true }),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated2: Value = resp.json();
    assert_eq!(updated2["description"], "New Desc");
    assert_eq!(updated2["name"], "New Name"); // unchanged
    assert!(updated2["updated_at"].as_str().is_some());

    teardown(&pool, &[teacher_id], account_id, course_id).await;
}

#[tokio::test]
async fn external_tools_require_authentication() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let res = server.get("/courses/1/external_tools/1").await;
    assert_eq!(res.status_code(), 401);
}
