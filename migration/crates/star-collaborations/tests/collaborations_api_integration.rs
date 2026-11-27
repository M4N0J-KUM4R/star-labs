//! Integration tests for the Canvas collaborations routes.
//!
//! Pinned behavior:
//! - `GET /courses/{course_id}/collaborations` (and the group twin) answer a
//!   bare JSON array of `collaboration_json` hashes — `id`,
//!   `collaboration_type`, `document_id`, `user_id`, `context_id`,
//!   `context_type`, `url`, `created_at`, `updated_at`, `description`,
//!   `title`, `type`, `update_url`, plus `user_name` and `permissions`.
//!   Only `ExternalToolCollaboration` rows are listed and soft-deleted rows
//!   are dropped; `collaborator_ids` is **not** part of the payload.
//! - A caller without the course-content management right only sees the
//!   collaborations it created or collaborates on.
//! - `GET /collaborations/{id}/members` answers a flat array of
//!   `collaborator_json` hashes mixing user and group shapes; the
//!   `collaborator_lti_id` key only appears when it is requested.
//! - `GET /courses/{course_id}/potential_collaborators` answers the enrolled
//!   users sorted by `sortable_name`; the group twin adds the admins of the
//!   parent course.
//! - `DELETE /courses/{course_id}/collaborations/{id}` soft-deletes the row
//!   and answers the raw record hash (every column except `uuid`).
//! - Negatives: anonymous 401, a context without a collaboration provider
//!   403, a missing course/group/collaboration 404, a collaboration of
//!   another course 404, a caller without `:read_roster` 403.
//!
//! Fleet rule: dedicated users, accounts, courses and groups per test, every
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
        "collab-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
        n
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
        TestServer::new(star_collaborations::router().with_state(state)).expect("build TestServer");
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
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

async fn seed_group(pool: &PgPool, prefix: &str, name: &str, course_id: i64) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (name, course_id) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(course_id)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

async fn seed_group_membership(pool: &PgPool, group_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state) \
         VALUES ($1, $2, 'accepted')",
    )
    .bind(group_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed group membership");
}

/// An LTI tool with the `collaboration` placement in the course — this is
/// what makes `Collaboration.any_collaborations_configured?` true.
async fn seed_collaboration_tool(pool: &PgPool, prefix: &str, course_id: i64, account_id: i64) {
    let tool_id: i64 = sqlx::query_scalar(
        "INSERT INTO context_external_tools \
           (context_id, context_type, shared_secret, consumer_key, name, workflow_state, \
            root_account_id, created_at, updated_at) \
         VALUES ($1, 'Course', 'secret', 'key', $2, 'active', $3, \
                 (now() AT TIME ZONE 'UTC'), (now() AT TIME ZONE 'UTC')) \
         RETURNING id",
    )
    .bind(course_id)
    .bind(format!("{prefix}-collab-tool"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed context external tool");

    sqlx::query(
        "INSERT INTO context_external_tool_placements (placement_type, context_external_tool_id) \
         VALUES ('collaboration', $1)",
    )
    .bind(tool_id)
    .execute(pool)
    .await
    .expect("seed tool placement");
}

/// One `collaborations` row.
#[allow(clippy::too_many_arguments)]
async fn seed_collaboration(
    pool: &PgPool,
    prefix: &str,
    context_type: &str,
    context_id: i64,
    user_id: i64,
    title: &str,
    kind: &str,
    workflow_state: &str,
    data: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO collaborations \
           (context_type, context_id, user_id, title, url, type, workflow_state, data, \
            created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, \
                 (now() AT TIME ZONE 'UTC'), (now() AT TIME ZONE 'UTC')) \
         RETURNING id",
    )
    .bind(context_type)
    .bind(context_id)
    .bind(user_id)
    .bind(format!("{prefix}-{title}"))
    .bind(format!("https://tool.example/{title}"))
    .bind(kind)
    .bind(workflow_state)
    .bind(data)
    .fetch_one(pool)
    .await
    .expect("seed collaboration")
}

/// One `collaborators` row (a user collaborator or a group collaborator).
async fn seed_collaborator(
    pool: &PgPool,
    collaboration_id: i64,
    user_id: Option<i64>,
    group_id: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO collaborators (collaboration_id, user_id, group_id, created_at, updated_at) \
         VALUES ($1, $2, $3, (now() AT TIME ZONE 'UTC'), (now() AT TIME ZONE 'UTC'))",
    )
    .bind(collaboration_id)
    .bind(user_id)
    .bind(group_id)
    .execute(pool)
    .await
    .expect("seed collaborator");
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let like = format!("{prefix}%");
    let _ = sqlx::query(
        "DELETE FROM collaborators WHERE collaboration_id IN \
           (SELECT id FROM collaborations WHERE title LIKE $1)",
    )
    .bind(&like)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM collaborations WHERE title LIKE $1")
        .bind(&like)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM context_external_tool_placements WHERE context_external_tool_id IN \
           (SELECT id FROM context_external_tools WHERE name LIKE $1)",
    )
    .bind(&like)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM context_external_tools WHERE name LIKE $1")
        .bind(&like)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM group_memberships WHERE group_id IN \
           (SELECT id FROM groups WHERE name LIKE $1)",
    )
    .bind(&like)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM groups WHERE name LIKE $1")
        .bind(&like)
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(&like)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(&like)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(&like)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn course_index_lists_the_external_tool_collaborations() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let active = seed_collaboration(
        &pool,
        &prefix,
        "Course",
        course,
        teacher,
        "Active",
        "ExternalToolCollaboration",
        "active",
        Some(r#"{"updateUrl":"https://tool.example/update"}"#),
    )
    .await;
    seed_collaboration(
        &pool,
        &prefix,
        "Course",
        course,
        teacher,
        "Removed",
        "ExternalToolCollaboration",
        "deleted",
        None,
    )
    .await;
    seed_collaboration(
        &pool,
        &prefix,
        "Course",
        course,
        teacher,
        "Google",
        "GoogleDocsCollaboration",
        "active",
        None,
    )
    .await;

    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1, "body: {list}");
    let item = &items[0];
    assert_eq!(item["id"], active);
    assert_eq!(item["title"], format!("{prefix}-Active"));
    assert_eq!(item["context_type"], "Course");
    assert_eq!(item["context_id"], course);
    assert_eq!(item["user_id"], teacher);
    assert_eq!(item["user_name"], "Teacher");
    assert_eq!(item["type"], "ExternalToolCollaboration");
    assert_eq!(item["collaboration_type"], Value::Null);
    assert_eq!(item["document_id"], Value::Null);
    assert_eq!(item["update_url"], "https://tool.example/update");
    assert_eq!(item["permissions"]["update"], true);
    assert_eq!(item["permissions"]["delete"], true);
    // `collaboration_json` has no `collaborator_ids`.
    assert!(item.get("collaborator_ids").is_none());
    assert!(item.get("created_at").is_some());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_index_hides_the_collaborations_of_other_users_from_students() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let own = seed_collaboration(
        &pool,
        &prefix,
        "Course",
        course,
        student,
        "Own",
        "ExternalToolCollaboration",
        "active",
        None,
    )
    .await;
    let invited = seed_collaboration(
        &pool,
        &prefix,
        "Course",
        course,
        teacher,
        "Invited",
        "ExternalToolCollaboration",
        "active",
        None,
    )
    .await;
    let foreign = seed_collaboration(
        &pool,
        &prefix,
        "Course",
        course,
        other,
        "Foreign",
        "ExternalToolCollaboration",
        "active",
        None,
    )
    .await;
    seed_collaborator(&pool, invited, Some(student), None).await;

    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    let ids: Vec<i64> = items
        .iter()
        .map(|item| item["id"].as_i64().expect("id"))
        .collect();
    assert!(ids.contains(&own), "body: {list}");
    assert!(ids.contains(&invited), "body: {list}");
    assert!(!ids.contains(&foreign), "body: {list}");
    // A student does not manage the course content.
    assert_eq!(items[0]["permissions"]["update"], false);
    assert_eq!(items[0]["permissions"]["delete"], false);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_index_lists_the_groups_collaborations() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let group = seed_group(&pool, &prefix, "Group", course).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let collaboration = seed_collaboration(
        &pool,
        &prefix,
        "Group",
        group,
        teacher,
        "GroupCollab",
        "ExternalToolCollaboration",
        "active",
        None,
    )
    .await;

    let resp = server
        .get(&format!("/groups/{group}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1, "body: {list}");
    assert_eq!(items[0]["id"], collaboration);
    assert_eq!(items[0]["context_type"], "Group");
    assert_eq!(items[0]["context_id"], group);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn members_lists_user_and_group_collaborators() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let group = seed_group(&pool, &prefix, "Group", course).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let collaboration = seed_collaboration(
        &pool,
        &prefix,
        "Course",
        course,
        teacher,
        "Members",
        "ExternalToolCollaboration",
        "active",
        None,
    )
    .await;
    seed_collaborator(&pool, collaboration, Some(student), None).await;
    seed_collaborator(&pool, collaboration, None, Some(group)).await;

    let resp = server
        .get(&format!("/collaborations/{collaboration}/members"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 2, "body: {list}");

    let user_item = items
        .iter()
        .find(|item| item["type"] == "user")
        .expect("user collaborator");
    assert_eq!(user_item["collaborator_id"], student);
    assert_eq!(user_item["name"], "Student");
    assert!(user_item.get("collaborator_lti_id").is_none());
    assert!(user_item.get("avatar_image_url").is_none());

    let group_item = items
        .iter()
        .find(|item| item["type"] == "group")
        .expect("group collaborator");
    assert_eq!(group_item["collaborator_id"], group);
    assert_eq!(group_item["name"], format!("{prefix}-Group"));

    // `include[]=collaborator_lti_id` adds the key; without a past LTI id the
    // value is null.
    let resp = server
        .get(&format!(
            "/collaborations/{collaboration}/members?include[]=collaborator_lti_id"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    assert_eq!(items[0]["collaborator_lti_id"], Value::Null);
    assert!(items[0].get("avatar_image_url").is_none());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_potential_collaborators_lists_the_enrolled_users() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let resp = server
        .get(&format!("/courses/{course}/potential_collaborators"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    let ids: Vec<i64> = items
        .iter()
        .map(|item| item["id"].as_i64().expect("id"))
        .collect();
    assert_eq!(ids.len(), 2, "body: {list}");
    assert!(ids.contains(&teacher));
    assert!(ids.contains(&student));
    assert!(!ids.contains(&outsider));
    // The list is ordered by `sortable_name`.
    assert_eq!(items[0]["name"], "Student");
    assert_eq!(items[1]["name"], "Teacher");
    assert!(items[0].get("sortable_name").is_some());
    assert!(items[0].get("short_name").is_some());
    assert!(items[0].get("created_at").is_some());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_potential_collaborators_adds_the_course_admins() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let unenrolled = seed_user(&pool, &prefix, "Unenrolled", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let group = seed_group(&pool, &prefix, "Group", course).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, member, course, "student").await;
    seed_group_membership(&pool, group, member).await;
    seed_group_membership(&pool, group, unenrolled).await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let resp = server
        .get(&format!("/groups/{group}/potential_collaborators"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    let ids: Vec<i64> = items
        .iter()
        .map(|item| item["id"].as_i64().expect("id"))
        .collect();
    // The group member and the admin of the parent course.
    assert!(ids.contains(&member), "body: {list}");
    assert!(ids.contains(&teacher), "body: {list}");
    // The group member that is not enrolled in the course is dropped.
    assert!(!ids.contains(&unenrolled), "body: {list}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn destroy_soft_deletes_the_collaboration() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let collaboration = seed_collaboration(
        &pool,
        &prefix,
        "Course",
        course,
        teacher,
        "Doomed",
        "ExternalToolCollaboration",
        "active",
        None,
    )
    .await;

    let resp = server
        .delete(&format!(
            "/courses/{course}/collaborations/{collaboration}?delete_doc=true"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], collaboration);
    assert_eq!(body["workflow_state"], "deleted");
    assert!(body["deleted_at"].is_string());
    assert_eq!(body["context_type"], "Course");
    // `uuid` is in `Collaboration.serialization_excludes`.
    assert!(body.get("uuid").is_none());

    // The soft-deleted row disappears from the index.
    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert!(list.as_array().expect("array").is_empty(), "body: {list}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn destroy_of_a_collaboration_of_another_course_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let collaboration = seed_collaboration(
        &pool,
        &prefix,
        "Course",
        other_course,
        teacher,
        "Foreign",
        "ExternalToolCollaboration",
        "active",
        None,
    )
    .await;

    let resp = server
        .delete(&format!("/courses/{course}/collaborations/{collaboration}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn anonymous_requests_are_401() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let group = seed_group(&pool, &prefix, "Group", course).await;

    for path in [
        format!("/courses/{course}/collaborations"),
        format!("/groups/{group}/collaborations"),
        format!("/courses/{course}/potential_collaborators"),
        format!("/groups/{group}/potential_collaborators"),
        "/collaborations/1/members".to_string(),
    ] {
        let resp = server.get(&path).await;
        assert_eq!(resp.status_code(), 401, "path {path}: {}", resp.text());
    }

    for path in [
        format!("/courses/{course}/collaborations/1"),
        format!("/groups/{group}/collaborations/1"),
    ] {
        let resp = server.delete(&path).await;
        assert_eq!(resp.status_code(), 401, "path {path}: {}", resp.text());
    }

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_context_without_a_collaboration_provider_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    // No collaboration tool is seeded for this course.

    let resp = server
        .get(&format!("/courses/{course}/collaborations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["errors"],
        json!(["Collaborations have not been enabled for this Canvas site"])
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_cannot_read_the_potential_collaborators() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let resp = server
        .get(&format!("/courses/{course}/potential_collaborators"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_missing_course_or_collaboration_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_collaboration_tool(&pool, &prefix, course, account).await;

    let resp = server
        .get("/courses/99999999/collaborations")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .get("/collaborations/99999999/members")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
