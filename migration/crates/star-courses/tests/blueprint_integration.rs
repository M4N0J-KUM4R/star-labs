//! Integration tests for the Phase 5 blueprint-course (master_courses) routes:
//! - GET  /courses/{courseId}/blueprint_templates/{templateId}
//! - GET  /courses/{courseId}/blueprint_templates/{templateId}/associated_courses
//! - PUT  /courses/{courseId}/blueprint_templates/{templateId}/update_associations
//! - GET  /courses/{courseId}/blueprint_templates/{templateId}/unsynced_changes
//! - POST/GET /courses/{courseId}/blueprint_templates/{templateId}/migrations
//! - GET  /courses/{courseId}/blueprint_templates/{templateId}/migrations/{id}
//! - GET  /courses/{courseId}/blueprint_templates/{templateId}/migrations/{id}/details
//! - PUT  /courses/{courseId}/blueprint_templates/{templateId}/restrict_item
//! - GET  /courses/{courseId}/blueprint_subscriptions
//! - GET  /courses/{courseId}/blueprint_subscriptions/{subId}/migrations
//! - GET  /courses/{courseId}/blueprint_subscriptions/{subId}/migrations/{id}
//! - GET  /courses/{courseId}/blueprint_subscriptions/{subId}/migrations/{id}/details
//!
//! Same harness conventions as tests/courses_deep_integration.rs: skip cleanly
//! when DATABASE_URL is unset, unique per-run prefix, full teardown.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "bp-{}-{}",
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
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_courses::router())
            .with_state(state),
    )
    .expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id"#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id"#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(r#"INSERT INTO accounts (name) VALUES ($1) RETURNING id"#)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_template(pool: &PgPool, course_id: i64) -> i64 {
    sqlx::query_scalar(r#"INSERT INTO master_templates (course_id) VALUES ($1) RETURNING id"#)
        .bind(course_id)
        .fetch_one(pool)
        .await
        .expect("seed template")
}

async fn seed_subscription(pool: &PgPool, template_id: i64, child_course_id: i64) -> i64 {
    sqlx::query_scalar(
        r#"INSERT INTO master_course_subscriptions (master_template_id, child_course_id)
           VALUES ($1, $2) RETURNING id"#,
    )
    .bind(template_id)
    .bind(child_course_id)
    .fetch_one(pool)
    .await
    .expect("seed subscription")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // master_migrations and master_course_subscriptions cascade from
    // master_templates; master_templates rows reference courses without
    // cascade, so they go first.
    let _ = sqlx::query(
        r#"DELETE FROM master_templates WHERE course_id IN
           (SELECT id FROM courses WHERE course_code LIKE $1)"#,
    )
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
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET blueprint_templates/{templateId} (show)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn blueprint_template_show_happy_path_and_non_admin_forbidden() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let blueprint_id = seed_course(&pool, &prefix, "blueprint", account_id).await;
    let child_id = seed_course(&pool, &prefix, "child", account_id).await;
    let template_id = seed_template(&pool, blueprint_id).await;
    seed_subscription(&pool, template_id, child_id).await;
    let admin = seed_user(&pool, &prefix, "admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;

    // Teacher without account admin rights -> 403.
    let forbidden = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    // Admin -> 200 with master_template_json shape.
    let resp = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(template_id));
    assert_eq!(body["course_id"].as_i64(), Some(blueprint_id));
    assert_eq!(body["associated_course_count"].as_i64(), Some(1));
    assert!(
        body.get("last_export_completed_at").is_some(),
        "key present (null before first sync)"
    );
    assert!(
        body.get("latest_migration").is_some(),
        "key present (null without a running migration)"
    );

    // `default` resolves the course's single template too.
    let default_resp = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/default"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(default_resp.status_code(), 200);
    let default_body: Value = default_resp.json();
    assert_eq!(default_body["id"].as_i64(), Some(template_id));

    // Wrong course -> 404.
    let missing = server
        .get(&format!(
            "/courses/{child_id}/blueprint_templates/{template_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET associated_courses + PUT update_associations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn blueprint_associated_courses_and_update_associations() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let blueprint_id = seed_course(&pool, &prefix, "blueprint", account_id).await;
    let child_id = seed_course(&pool, &prefix, "child", account_id).await;
    let other_account = seed_account(&pool, &prefix, "other-acct").await;
    let foreign_child = seed_course(&pool, &prefix, "foreign", other_account).await;
    let template_id = seed_template(&pool, blueprint_id).await;
    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    // Empty list before any association.
    let empty = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/associated_courses"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(empty.status_code(), 200);
    assert_eq!(empty.json::<Value>(), serde_json::json!([]));

    // Add a course from another account -> 400.
    let foreign = server
        .put(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/update_associations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&serde_json::json!({ "course_ids_to_add": [foreign_child] }))
        .await;
    assert_eq!(foreign.status_code(), 400, "{}", foreign.text());

    // Add the in-account child -> success.
    let add = server
        .put(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/update_associations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&serde_json::json!({ "course_ids_to_add": [child_id] }))
        .await;
    assert_eq!(add.status_code(), 200, "{}", add.text());
    assert_eq!(add.json::<Value>(), serde_json::json!({ "success": true }));

    // associated_courses now lists the child with the course summary shape.
    let list = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/associated_courses"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(list.status_code(), 200);
    let courses: Value = list.json();
    assert_eq!(courses.as_array().map(Vec::len), Some(1));
    assert_eq!(courses[0]["id"].as_i64(), Some(child_id));
    assert!(courses[0].get("name").is_some());
    assert!(courses[0].get("course_code").is_some());

    // Remove it again -> success and the list empties.
    let remove = server
        .put(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/update_associations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&serde_json::json!({ "course_ids_to_remove": [child_id] }))
        .await;
    assert_eq!(remove.status_code(), 200, "{}", remove.text());
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM master_course_subscriptions WHERE master_template_id = $1",
    )
    .bind(template_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "association removed");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST/GET migrations, show, details, unsynced_changes, subscriptions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn blueprint_migration_lifecycle_and_subscriptions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let blueprint_id = seed_course(&pool, &prefix, "blueprint", account_id).await;
    let child_id = seed_course(&pool, &prefix, "child", account_id).await;
    let template_id = seed_template(&pool, blueprint_id).await;
    let subscription_id = seed_subscription(&pool, template_id, child_id).await;
    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    // No subscriptions -> 400.
    sqlx::query("DELETE FROM master_course_subscriptions WHERE id = $1")
        .bind(subscription_id)
        .execute(&pool)
        .await
        .unwrap();
    let no_subs = server
        .post(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/migrations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&serde_json::json!({ "comment": "no subs" }))
        .await;
    assert_eq!(no_subs.status_code(), 400, "{}", no_subs.text());
    seed_subscription(&pool, template_id, child_id).await;

    // Queue a migration -> created directly in the completed state.
    let queue = server
        .post(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/migrations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&serde_json::json!({ "comment": "sync things" }))
        .await;
    assert_eq!(queue.status_code(), 200, "{}", queue.text());
    let migration: Value = queue.json();
    let migration_id = migration["id"].as_i64().expect("migration id");
    assert_eq!(migration["workflow_state"].as_str(), Some("completed"));
    assert_eq!(migration["template_id"].as_i64(), Some(template_id));
    assert_eq!(migration["user_id"].as_i64(), Some(admin));
    assert_eq!(migration["comment"].as_str(), Some("sync things"));
    assert!(
        migration["exports_started_at"].is_string(),
        "structural simplification sets export timestamps"
    );
    assert!(
        migration["imports_completed_at"].is_string(),
        "structural simplification sets import timestamps"
    );
    assert!(
        migration["user"].get("display_name").is_some(),
        "user_display_json subset embedded"
    );

    // List shows it; show returns the same shape.
    let list = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/migrations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(list.status_code(), 200);
    let items: Value = list.json();
    assert_eq!(items.as_array().map(Vec::len), Some(1));
    assert_eq!(items[0]["id"].as_i64(), Some(migration_id));

    let show = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/migrations/{migration_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(show.status_code(), 200);
    let shown: Value = show.json();
    assert_eq!(shown["id"].as_i64(), Some(migration_id));

    // Details exist (empty ChangeRecord list).
    let details = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/migrations/{migration_id}/details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(details.status_code(), 200);
    assert_eq!(details.json::<Value>(), serde_json::json!([]));

    // Unknown migration id -> 404.
    let missing = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/migrations/999999999"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing.status_code(), 404);

    // Template show now reflects the completed migration.
    let template_json = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    let template_body: Value = template_json.json();
    assert_eq!(
        template_body["associated_course_count"].as_i64(),
        Some(1),
        "subscription re-added"
    );

    // unsynced_changes after a completed sync is a bare array.
    let unsynced = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/unsynced_changes"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(unsynced.status_code(), 200, "{}", unsynced.text());
    let changes: Value = unsynced.json();
    assert!(changes.is_array(), "bare ChangeRecord array");

    // restrict_item validates the content type and item existence.
    let bad_type = server
        .put(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/restrict_item"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&serde_json::json!({
            "content_type": "bogus", "content_id": 1, "restricted": true
        }))
        .await;
    assert_eq!(bad_type.status_code(), 400);
    let missing_item = server
        .put(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/restrict_item"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&serde_json::json!({
            "content_type": "assignment", "content_id": 999999999, "restricted": true
        }))
        .await;
    assert_eq!(missing_item.status_code(), 404);
    let assignment_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO assignments (course_id, title) VALUES ($1, 'bp quiz') RETURNING id"#,
    )
    .bind(blueprint_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let restrict = server
        .put(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/restrict_item"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&serde_json::json!({
            "content_type": "assignment", "content_id": assignment_id, "restricted": true
        }))
        .await;
    assert_eq!(restrict.status_code(), 200, "{}", restrict.text());
    assert_eq!(
        restrict.json::<Value>(),
        serde_json::json!({ "success": true })
    );

    // Subscriptions index (associated side) on the child course.
    let subs = server
        .get(&format!("/courses/{child_id}/blueprint_subscriptions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(subs.status_code(), 200, "{}", subs.text());
    let subscriptions: Value = subs.json();
    assert_eq!(subscriptions.as_array().map(Vec::len), Some(1));
    assert_eq!(subscriptions[0]["template_id"].as_i64(), Some(template_id));
    assert_eq!(
        subscriptions[0]["blueprint_course"]["id"].as_i64(),
        Some(blueprint_id)
    );
    assert!(subscriptions[0]["blueprint_course"].get("name").is_some());
    assert!(subscriptions[0]["blueprint_course"]
        .get("course_code")
        .is_some());

    // Imports index / show / details on the child side.
    let imports = server
        .get(&format!(
            "/courses/{child_id}/blueprint_subscriptions/default/migrations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(imports.status_code(), 200, "{}", imports.text());
    let import_items: Value = imports.json();
    assert_eq!(import_items.as_array().map(Vec::len), Some(1));
    assert_eq!(
        import_items[0]["subscription_id"].as_i64(),
        Some(subscription_id),
        "child side swaps template_id for subscription_id"
    );
    assert!(
        import_items[0].get("template_id").is_none(),
        "template_id absent on the child side"
    );

    let import_show = server
        .get(&format!(
            "/courses/{child_id}/blueprint_subscriptions/{subscription_id}/migrations/{migration_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(import_show.status_code(), 200);
    let import_shown: Value = import_show.json();
    assert_eq!(import_shown["id"].as_i64(), Some(migration_id));

    let import_details = server
        .get(&format!(
            "/courses/{child_id}/blueprint_subscriptions/{subscription_id}/migrations/{migration_id}/details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(import_details.status_code(), 200);
    assert_eq!(import_details.json::<Value>(), serde_json::json!([]));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// unsynced_changes before any sync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn blueprint_unsynced_changes_initial_sync_before_any_migration() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let account_id = seed_account(&pool, &prefix, "acct").await;
    let blueprint_id = seed_course(&pool, &prefix, "blueprint", account_id).await;
    let template_id = seed_template(&pool, blueprint_id).await;
    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    let resp = server
        .get(&format!(
            "/courses/{blueprint_id}/blueprint_templates/{template_id}/unsynced_changes"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let changes: Value = resp.json();
    assert_eq!(
        changes,
        serde_json::json!([{ "asset_name": format!("{prefix}-blueprint"), "change_type": "initial_sync" }]),
        "no completed sync yet -> initial_sync record"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Unknown course / template
// ---------------------------------------------------------------------------

#[tokio::test]
async fn blueprint_routes_unknown_course_or_template_are_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "admin", "admin").await;

    let missing_course = server
        .get("/courses/999999999/blueprint_templates/default")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing_course.status_code(), 404);

    let account_id = seed_account(&pool, &prefix, "acct").await;
    let course_id = seed_course(&pool, &prefix, "plain", account_id).await;
    let missing_template = server
        .get(&format!("/courses/{course_id}/blueprint_templates/default"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing_template.status_code(), 404);

    let missing_subs = server
        .get(&format!("/courses/{course_id}/blueprint_subscriptions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing_subs.status_code(), 200);
    assert_eq!(missing_subs.json::<Value>(), serde_json::json!([]));

    cleanup(&pool, &prefix).await;
}
