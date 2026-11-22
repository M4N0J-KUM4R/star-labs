//! Integration tests for the Canvas group folder routes of the folders API.
//!
//! Pinned behavior (Canvas `app/controllers/folders_controller.rb`,
//! `lib/api/v1/folders.rb`):
//! - GET /groups/{groupId}/folders lists the flat set of the group folders for
//!   the members of the group and for the teachers of its course, and answers
//!   403 for a user that is neither.
//! - POST /groups/{groupId}/folders creates a folder below the Canvas `unfiled`
//!   folder, below an explicit parent or below a `parent_folder_path` (the
//!   missing folders of the path are created), rejects a body that sets both a
//!   parent id and a parent path with the Canvas 400 and answers 404 for a
//!   parent of another context.
//! - GET /groups/{groupId}/folders/media answers the "Uploaded Media" folder of
//!   the group for a caller that may manage the files of the group and the
//!   media folder of the caller themselves for everybody else.
//! - GET /groups/{groupId}/folders/{id}/duplicates is gated on the two site
//!   admin feature flags, answers 403 for a folder of another context and
//!   lists the sibling folders that share the name of the folder.
//! - The `by_path` resolvers of the slice answer the folder hierarchy from the
//!   root folder to the requested folder and a 404 for an unknown path.
//!
//! Fleet rule: dedicated users, accounts, courses, groups and folders per
//! test, every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is
//! unset.

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
        "fldgrp-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
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
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = TestServer::new(star_files::router().with_state(state)).expect("build TestServer");
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
    sqlx::query_scalar(
        "INSERT INTO groups (course_id, name, workflow_state) VALUES ($1, $2, 'available') RETURNING id",
    )
    .bind(course_id)
    .bind(format!("{prefix}-{name}"))
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

/// Seed a folder row (the folders table is polymorphic, there is no FK to the
/// context).
async fn seed_folder(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    name: &str,
    full_name: &str,
    parent_id: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO folders (name, full_name, context_type, context_id, parent_folder_id, \
         workflow_state, position) VALUES ($1, $2, $3, $4, $5, 'visible', 1) RETURNING id",
    )
    .bind(name)
    .bind(full_name)
    .bind(context_type)
    .bind(context_id)
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .expect("seed folder")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Enable the two a11y flags of the duplicates endpoint on the test's own
/// DEDICATED account (test isolation, issue #75 — never the shared root).
async fn enable_duplicates_flags(pool: &PgPool, account_id: i64) -> bool {
    for feature in ["files_a11y_rewrite", "files_a11y_folder_duplicates"] {
        sqlx::query(
            "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state) \
             VALUES ('Account', $1, $2, $2, 'enabled') \
             ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'enabled'",
        )
        .bind(account_id)
        .bind(feature)
        .execute(pool)
        .await
        .expect("enable the feature flag");
    }
    true
}

/// Disable the two a11y flags again — only on the given account.
async fn disable_duplicates_flags(pool: &PgPool, account_id: i64) {
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Account' AND context_id = $1 \
         AND feature IN ('files_a11y_rewrite', 'files_a11y_folder_duplicates')",
    )
    .execute(pool)
    .await;
}

async fn cleanup(pool: &PgPool, prefix: &str, group_id: i64, course_id: i64, user_ids: &[i64]) {
    // The folders of the seeded contexts have no FK to the context rows.
    let _ = sqlx::query(
        "DELETE FROM folders WHERE (context_type = 'Group' AND context_id = $1) \
         OR (context_type = 'Course' AND context_id = $2) \
         OR (context_type = 'User' AND context_id = ANY($3))",
    )
    .bind(group_id)
    .bind(course_id)
    .bind(user_ids)
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM group_memberships WHERE group_id = $1")
        .bind(group_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM groups WHERE id = $1")
        .bind(group_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM enrollments WHERE course_id = $1")
        .bind(course_id)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Account' \\
           AND context_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn the_group_folders_are_listed_for_the_members_and_the_teachers() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, &prefix, "Group", course).await;
    seed_group_membership(&pool, group, member).await;

    let root = seed_folder(&pool, "Group", group, "files", "files", None).await;
    let alpha = seed_folder(&pool, "Group", group, "alpha", "files/alpha", Some(root)).await;
    seed_folder(
        &pool,
        "Group",
        group,
        "beta",
        "files/alpha/beta",
        Some(alpha),
    )
    .await;

    // The teacher of the course of the group reads the folder list.
    let resp = server
        .get(&format!("/groups/{group}/folders"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let list = list.as_array().expect("folder list");
    let names: Vec<&str> = list
        .iter()
        .filter_map(|folder| folder["name"].as_str())
        .collect();
    assert_eq!(names, vec!["alpha", "beta", "files"], "names: {names:?}");
    assert_eq!(list[0]["context_type"], "Group");
    assert_eq!(list[0]["context_id"], group);
    assert_eq!(list[0]["full_name"], "files/alpha");
    assert_eq!(
        list[0]["folders_url"],
        format!("/api/v1/folders/{alpha}/folders")
    );

    // A member of the group reads it as well.
    let resp = server
        .get(&format!("/groups/{group}/folders"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // A user that is neither a member nor a teacher may not.
    let resp = server
        .get(&format!("/groups/{group}/folders"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An unknown group is a 404.
    let resp = server
        .get("/groups/99999999/folders")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix, group, course, &[teacher, member, outsider]).await;
}

#[tokio::test]
async fn a_group_folder_is_created_below_the_unfiled_folder_or_a_path() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, &prefix, "Group", course).await;
    seed_group_membership(&pool, group, member).await;

    // Without a parent the folder lands in the Canvas "unfiled" folder.
    let resp = server
        .post(&format!("/groups/{group}/folders"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Notes" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let folder: Value = resp.json();
    assert_eq!(folder["name"], "Notes");
    assert_eq!(folder["full_name"], "files/unfiled/Notes");
    assert_eq!(folder["context_type"], "Group");
    assert_eq!(folder["context_id"], group);
    assert_eq!(folder["hidden"], false);

    // A second folder with the same name is deduplicated by Canvas.
    let resp = server
        .post(&format!("/groups/{group}/folders"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Notes" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["name"], "Notes 2");

    // A parent folder path creates the missing folders of the path.
    let resp = server
        .post(&format!("/groups/{group}/folders"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Deep", "parent_folder_path": "files/one/two" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let folder: Value = resp.json();
    assert_eq!(folder["full_name"], "files/one/two/Deep");

    // A parent id and a parent path together are the Canvas 400.
    let resp = server
        .post(&format!("/groups/{group}/folders"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Bad", "parent_folder_id": 1, "parent_folder_path": "files/one" }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert!(resp.text().contains("Can't set folder path and folder id"));

    // A parent of another context is a 404.
    let other_group = seed_group(&pool, &prefix, "OtherGroup", course).await;
    let foreign = seed_folder(&pool, "Group", other_group, "foreign", "files", None).await;
    let resp = server
        .post(&format!("/groups/{group}/folders"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "name": "Bad", "parent_folder_id": foreign }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A member of the group may not create folders in it.
    let resp = server
        .post(&format!("/groups/{group}/folders"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .json(&json!({ "name": "Nope" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix, group, course, &[teacher, member]).await;
    let _ = sqlx::query("DELETE FROM folders WHERE context_type = 'Group' AND context_id = $1")
        .bind(other_group)
        .execute(&pool)
        .await;
    let _ = sqlx::query("DELETE FROM groups WHERE id = $1")
        .bind(other_group)
        .execute(&pool)
        .await;
}

#[tokio::test]
async fn the_group_media_folder_belongs_to_the_caller_without_the_file_rights() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, &prefix, "Group", course).await;
    seed_group_membership(&pool, group, member).await;

    // The teacher manages the files of the group: the media folder is the
    // media folder of the group.
    let resp = server
        .get(&format!("/groups/{group}/folders/media"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let folder: Value = resp.json();
    assert_eq!(folder["name"], "Uploaded Media");
    assert_eq!(folder["context_type"], "Group");
    assert_eq!(folder["context_id"], group);
    assert_eq!(folder["hidden"], true);

    // A member of the group gets the media folder of their own user context.
    let resp = server
        .get(&format!("/groups/{group}/folders/media"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let folder: Value = resp.json();
    assert_eq!(folder["name"], "Uploaded Media");
    assert_eq!(folder["context_type"], "User");
    assert_eq!(folder["context_id"], member);

    // A user that is neither a member nor a teacher may not.
    let resp = server
        .get(&format!("/groups/{group}/folders/media"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix, group, course, &[teacher, member, outsider]).await;
}

#[tokio::test]
async fn the_group_duplicates_need_the_feature_flags() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, &prefix, "Group", course).await;
    seed_group_membership(&pool, group, member).await;

    let root = seed_folder(&pool, "Group", group, "files", "files", None).await;
    let dup = seed_folder(&pool, "Group", group, "dup", "files/dup", Some(root)).await;
    let twin = seed_folder(&pool, "Group", group, "dup", "files/dup", Some(root)).await;

    // Without the two a11y feature flags the endpoint is the Canvas 403.
    disable_duplicates_flags(&pool, account).await;
    let resp = server
        .get(&format!("/groups/{group}/folders/{dup}/duplicates"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    if enable_duplicates_flags(&pool, account).await {
        // With the flags the sibling folder of the same name is listed.
        let resp = server
            .get(&format!("/groups/{group}/folders/{dup}/duplicates"))
            .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
            .await;
        assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
        let body: Value = resp.json();
        let duplicates = body["duplicates"].as_array().expect("duplicates");
        assert_eq!(duplicates.len(), 1, "duplicates: {duplicates:?}");
        assert_eq!(duplicates[0]["id"], twin);
        assert_eq!(duplicates[0]["name"], "dup");

        // A folder of another context is the Canvas 403.
        let other_group = seed_group(&pool, &prefix, "OtherGroup", course).await;
        let foreign = seed_folder(&pool, "Group", other_group, "foreign", "files", None).await;
        let resp = server
            .get(&format!("/groups/{group}/folders/{foreign}/duplicates"))
            .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

        // An unknown folder is a 404.
        let resp = server
            .get(&format!("/groups/{group}/folders/99999999/duplicates"))
            .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
            .await;
        assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

        let _ = sqlx::query("DELETE FROM folders WHERE context_type = 'Group' AND context_id = $1")
            .bind(other_group)
            .execute(&pool)
            .await;
        let _ = sqlx::query("DELETE FROM groups WHERE id = $1")
            .bind(other_group)
            .execute(&pool)
            .await;
    }

    cleanup(&pool, &prefix, group, course, &[teacher, member]).await;
}

#[tokio::test]
async fn the_by_path_resolvers_answer_the_folder_hierarchy() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let group = seed_group(&pool, &prefix, "Group", course).await;
    seed_group_membership(&pool, group, member).await;

    let course_root = seed_folder(
        &pool,
        "Course",
        course,
        "course files",
        "course files",
        None,
    )
    .await;
    let course_alpha = seed_folder(
        &pool,
        "Course",
        course,
        "alpha",
        "course files/alpha",
        Some(course_root),
    )
    .await;
    let course_beta = seed_folder(
        &pool,
        "Course",
        course,
        "beta",
        "course files/alpha/beta",
        Some(course_alpha),
    )
    .await;

    let user_root = seed_folder(&pool, "User", member, "my files", "my files", None).await;
    let user_alpha = seed_folder(
        &pool,
        "User",
        member,
        "alpha",
        "my files/alpha",
        Some(user_root),
    )
    .await;

    let group_root = seed_folder(&pool, "Group", group, "files", "files", None).await;
    let group_alpha = seed_folder(
        &pool,
        "Group",
        group,
        "alpha",
        "files/alpha",
        Some(group_root),
    )
    .await;

    // The course path answers the hierarchy from the root folder down.
    let resp = server
        .get(&format!("/courses/{course}/folders/by_path/alpha/beta"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let hierarchy: Value = resp.json();
    let hierarchy = hierarchy.as_array().expect("hierarchy");
    assert_eq!(hierarchy.len(), 3, "hierarchy: {hierarchy:?}");
    assert_eq!(hierarchy[0]["id"], course_root);
    assert_eq!(hierarchy[1]["id"], course_alpha);
    assert_eq!(hierarchy[2]["id"], course_beta);

    // An unknown path is a 404.
    let resp = server
        .get(&format!("/courses/{course}/folders/by_path/alpha/nope"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The user path resolves the folders of the user themselves.
    let resp = server
        .get(&format!("/users/{member}/folders/by_path/alpha"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let hierarchy: Value = resp.json();
    let hierarchy = hierarchy.as_array().expect("hierarchy");
    assert_eq!(hierarchy.len(), 2, "hierarchy: {hierarchy:?}");
    assert_eq!(hierarchy[0]["id"], user_root);
    assert_eq!(hierarchy[1]["id"], user_alpha);

    // The group path resolves the folders of the group.
    let resp = server
        .get(&format!("/groups/{group}/folders/by_path/alpha"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let hierarchy: Value = resp.json();
    let hierarchy = hierarchy.as_array().expect("hierarchy");
    assert_eq!(hierarchy.len(), 2, "hierarchy: {hierarchy:?}");
    assert_eq!(hierarchy[0]["id"], group_root);
    assert_eq!(hierarchy[1]["id"], group_alpha);

    cleanup(&pool, &prefix, group, course, &[teacher, member]).await;
}
