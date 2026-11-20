//! Integration tests for the group and user content export contexts, the
//! ePub / web zip export routes and the deprecated course-copy API.
//!
//! Canvas serves the same actions for a course, a group and a user, and the
//! port stored the context of a content export in `course_id` alone, so the
//! group and user variants were missing.
//!
//! Pinned behavior:
//! - a group member lists, creates and reads their own exports;
//! - another user sees nothing and gets 403;
//! - `fail` and the course export update are site-admin only;
//! - `/content_list` answers the type buckets and the items of a type;
//! - an ePub export is created with a job progress and is readable through
//!   the course;
//! - a course copy is a content migration and reports its status.
//!
//! Fleet rule: dedicated accounts, courses, groups and users per test, every
//! seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "cexp-{}-{}",
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

    let server =
        TestServer::new(star_content::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

async fn seed_group(pool: &PgPool, prefix: &str, name: &str, course_id: i64) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed group")
}

async fn seed_membership(pool: &PgPool, group_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state) \
         VALUES ($1, $2, 'accepted')",
    )
    .bind(group_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed membership");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM progress WHERE context_type = 'EpubExport' \
           AND context_id IN (SELECT id FROM epub_exports WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM epub_exports WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM content_exports WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM content_migrations WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
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

#[tokio::test]
async fn a_group_member_lists_and_creates_group_exports() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let member = seed_user(&pool, &prefix, "Member", "teacher").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, member, course, "teacher").await;
    let group = seed_group(&pool, &prefix, "Group", course).await;
    seed_membership(&pool, group, member).await;

    // POST /groups/{groupId}/content_exports — only a zip export is valid.
    let resp = server
        .post(&format!("/groups/{group}/content_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "teacher"))
        .json(&json!({ "export_type": "zip" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let export: Value = resp.json();
    assert_eq!(export["export_type"], "zip");
    assert_eq!(export["workflow_state"], "created");
    let export_id = export["id"].as_i64().expect("export id");

    // A group export carries no course id.
    assert!(export.get("course_id").is_none(), "{export}");

    // GET /groups/{groupId}/content_exports lists it.
    let resp = server
        .get(&format!("/groups/{group}/content_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let ids: Vec<i64> = list
        .as_array()
        .expect("array")
        .iter()
        .map(|item| item["id"].as_i64().expect("id"))
        .collect();
    assert_eq!(ids, vec![export_id]);

    // GET /groups/{groupId}/content_exports/{id} serves the export.
    let resp = server
        .get(&format!("/groups/{group}/content_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["id"], export_id);

    // A user without the group may not read the exports.
    let resp = server
        .get(&format!("/groups/{group}/content_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // A missing group is 404.
    let resp = server
        .get("/groups/999999999/content_exports")
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_user_export_is_scoped_to_its_owner() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let user = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;

    let resp = server
        .post(&format!("/users/{user}/content_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "export_type": "zip" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let export: Value = resp.json();
    let export_id = export["id"].as_i64().expect("export id");

    // The owner reads the export.
    let resp = server
        .get(&format!("/users/{user}/content_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["id"], export_id);

    // Another user may not read it.
    let resp = server
        .get(&format!("/users/{user}/content_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .get(&format!("/users/{user}/content_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list.as_array().map(|items| items.len()), Some(1));

    // Another user may not list them.
    let resp = server
        .get(&format!("/users/{user}/content_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // A common cartridge export is not valid for a user context.
    let resp = server
        .post(&format!("/users/{user}/content_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "export_type": "common_cartridge" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn failing_an_export_is_site_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .post(&format!("/courses/{course}/content_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "export_type": "common_cartridge" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let export_id = resp.json::<Value>()["id"].as_i64().expect("export id");

    // A teacher may not fail an export.
    let resp = server
        .put(&format!(
            "/courses/{course}/content_exports/{export_id}/fail"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The site admin may.
    let resp = server
        .put(&format!(
            "/courses/{course}/content_exports/{export_id}/fail"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "failed");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_course_export_can_be_updated_by_a_site_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .post(&format!("/courses/{course}/content_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "export_type": "common_cartridge" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let export_id = resp.json::<Value>()["id"].as_i64().expect("export id");

    // A teacher may not update the export.
    let resp = server
        .put(&format!("/courses/{course}/content_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "content_export": { "new_quizzes_export_state": "completed" } }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The site admin may, and the new-quizzes settings are echoed.
    let resp = server
        .put(&format!("/courses/{course}/content_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "content_export": { "new_quizzes_export_state": "completed" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let export: Value = resp.json();
    assert_eq!(export["new_quizzes_export_state"], "completed");

    // A missing content_export key is a 400.
    let resp = server
        .put(&format!("/courses/{course}/content_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_content_list_answers_buckets_and_items() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    sqlx::query("INSERT INTO assignments (course_id, title) VALUES ($1, $2)")
        .bind(course)
        .bind("Pick one")
        .execute(&pool)
        .await
        .expect("seed assignment");

    // A student may not read the content list.
    let resp = server
        .get(&format!("/courses/{course}/content_list"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The buckets of the course.
    let resp = server
        .get(&format!("/courses/{course}/content_list"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let buckets: Value = resp.json();
    let list = buckets.as_array().expect("array");
    assert!(list.iter().any(|item| item["type"] == "course_settings"
        && item["property"] == "select[all_course_settings]"));
    let assignments = list
        .iter()
        .find(|item| item["type"] == "assignments")
        .expect("assignments bucket");
    assert_eq!(assignments["count"], 1);
    assert_eq!(
        assignments["sub_items_url"],
        format!("/api/v1/courses/{course}/content_list?type=assignments")
    );

    // The items of one type.
    let resp = server
        .get(&format!("/courses/{course}/content_list?type=assignments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let items: Value = resp.json();
    let items = items.as_array().expect("array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["type"], "assignments");
    assert_eq!(items[0]["title"], "Pick one");
    assert!(items[0]["id"]
        .as_str()
        .expect("id")
        .starts_with("assignment_"));

    // An unknown type is a 400.
    let resp = server
        .get(&format!("/courses/{course}/content_list?type=nonsense"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn an_epub_export_is_created_with_a_progress() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Canvas `EpubExportsController#check_feature_enabled`: the root account
    // has to allow the epub export feature.
    let root_account: i64 = sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL ORDER BY id ASC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("read the root account");
    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state)          VALUES ('Account', $1, 'epub_export', 'epub_export', 'enabled')          ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'enabled'",
    )
    .bind(root_account)
    .execute(&pool)
    .await
    .expect("enable the epub export feature");

    // A student may not create an export.
    let resp = server
        .post(&format!("/courses/{course}/epub_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The teacher may.
    let resp = server
        .post(&format!("/courses/{course}/epub_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["id"], course);
    let export = &created["epub_export"];
    assert_eq!(export["workflow_state"], "created");
    assert_eq!(export["course_id"], course);
    assert!(export["progress_id"].as_i64().is_some(), "{export}");
    assert_eq!(
        export["progress_url"],
        format!(
            "/api/v1/progress/{}",
            export["progress_id"].as_i64().unwrap()
        )
    );
    let export_id = export["id"].as_i64().expect("export id");

    // The export is readable through the course.
    let resp = server
        .get(&format!("/courses/{course}/epub_exports/{export_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["epub_export"]["id"], export_id);

    // The index lists the courses of the caller.
    let resp = server
        .get("/epub_exports")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let index: Value = resp.json();
    let courses = index["courses"].as_array().expect("courses");
    assert!(courses
        .iter()
        .any(|item| item["id"] == course && item["epub_export"]["id"] == export_id));

    // The web zip export list of the course is empty but served.
    let resp = server
        .get(&format!("/courses/{course}/web_zip_exports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert!(resp.json::<Value>().as_array().expect("array").is_empty());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_course_copy_reports_its_status() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let source = seed_course(&pool, &prefix, "Source").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, teacher, source, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // A student may not start a copy.
    let resp = server
        .post(&format!("/courses/{course}/course_copy"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "source_course": source.to_string() }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The teacher may.
    let resp = server
        .post(&format!("/courses/{course}/course_copy"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "source_course": source.to_string() }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let copy: Value = resp.json();
    assert_eq!(copy["workflow_state"], "created");
    let copy_id = copy["id"].as_i64().expect("copy id");
    assert_eq!(
        copy["status_url"],
        format!("/api/v1/courses/{course}/course_copy/{copy_id}")
    );

    // The status route answers the same body.
    let resp = server
        .get(&format!("/courses/{course}/course_copy/{copy_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let status: Value = resp.json();
    assert_eq!(status["id"], copy_id);
    assert_eq!(status["workflow_state"], "created");

    // `only` and `except` may not be used together.
    let resp = server
        .post(&format!("/courses/{course}/course_copy"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "source_course": source.to_string(), "only": ["files"], "except": ["quizzes"] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // A missing source course is a 404.
    let resp = server
        .post(&format!("/courses/{course}/course_copy"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "source_course": "999999999" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
