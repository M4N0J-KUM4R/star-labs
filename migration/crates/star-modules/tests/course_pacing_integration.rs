//! Course pacing (Canvas `CoursePacesController` API v1) integration tests.
//!
//! Covers the eight Canvas routes under `/api/v1/courses/:course_id/course_pacing`:
//! create, new, api_show, update, destroy, publish, compress_dates and
//! bulk_create_enrollment_paces.
//!
//! The module is wired into the crate root, so the test drives the crate
//! router.
//!
//! Fleet rule: dedicated users, accounts, courses, modules and assignments
//! per test, every seeded row is cleaned up. Skipped cleanly when
//! DATABASE_URL is unset, and skipped cleanly while the `course_paces`
//! tables are not migrated yet.

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
        "cp-{}-{}-{}",
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

    // The course_paces tables arrive with their own migration. Until then
    // the slice has nothing to talk to.
    let tables: Option<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_name IN ('course_paces', 'course_pace_module_items')",
    )
    .fetch_one(&pool)
    .await
    .expect("count the course pace tables");
    if tables.0 < 2 {
        println!("SKIP course_pacing_integration: the course_paces tables are not migrated yet");
        return None;
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: JWT_SECRET.to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_modules::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
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
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id, workflow_state) \
         VALUES ($1, NULL, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id, workflow_state, start_at, end_at) \
         VALUES ($1, $2, $3, 'available', '2021-09-30T00:00:00Z', '2021-10-31T00:00:00Z') \
         RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

/// Canvas `Course#enable_course_paces` is a course setting.
async fn enable_course_paces(pool: &PgPool, course_id: i64) {
    sqlx::query(
        "INSERT INTO course_settings (course_id, settings) \
         VALUES ($1, '{\"enable_course_paces\": true}') \
         ON CONFLICT (course_id) DO UPDATE SET settings = EXCLUDED.settings",
    )
    .bind(course_id)
    .execute(pool)
    .await
    .expect("enable the course paces setting");
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active') RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

async fn seed_module(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO modules (course_id, name, position, workflow_state) \
         VALUES ($1, $2, 1, 'active') RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed module")
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible, due_at, workflow_state) \
         VALUES ($1, $2, 10, '2021-10-01T00:00:00Z', 'published') RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_module_item(
    pool: &PgPool,
    module_id: i64,
    item_type: &str,
    item_id: i64,
    position: i32,
    title: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO module_items (module_id, item_type, item_id, position, title, workflow_state) \
         VALUES ($1, $2, $3, $4, $5, 'active') RETURNING id",
    )
    .bind(module_id)
    .bind(item_type)
    .bind(item_id)
    .bind(position)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed module item")
}

async fn seed_feature(pool: &PgPool, account_id: i64, feature: &str) {
    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state) \
         VALUES ('Account', $1, $2, $2, 'on') \
         ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'on'",
    )
    .bind(account_id)
    .bind(feature)
    .execute(pool)
    .await
    .expect("enable the feature flag");
}

async fn seed_blackout_date(pool: &PgPool, course_id: i64, start: &str, end: &str) {
    sqlx::query(
        "INSERT INTO blackout_dates (context_id, context_type, title, start_date, end_date) \
         VALUES ($1, 'Course', 'Break', $2, $3)",
    )
    .bind(course_id)
    .bind(start)
    .bind(end)
    .execute(pool)
    .await
    .expect("seed blackout date");
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}%");
    let statements = [
        "DELETE FROM progress WHERE context_type = 'Course' \
           AND context_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM progress WHERE context_type = 'CoursePace' AND context_id IN \
             (SELECT id FROM course_paces WHERE course_id IN \
               (SELECT id FROM courses WHERE course_code LIKE $1))",
        "DELETE FROM assignment_overrides WHERE assignment_id IN \
             (SELECT id FROM assignments WHERE course_id IN \
               (SELECT id FROM courses WHERE course_code LIKE $1))",
        "DELETE FROM course_pace_module_items WHERE course_pace_id IN \
             (SELECT id FROM course_paces WHERE course_id IN \
               (SELECT id FROM courses WHERE course_code LIKE $1))",
        "DELETE FROM course_paces WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM module_items WHERE module_id IN \
             (SELECT id FROM modules WHERE course_id IN \
               (SELECT id FROM courses WHERE course_code LIKE $1))",
        "DELETE FROM modules WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM assignments WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM enrollments WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM course_settings WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM blackout_dates WHERE context_type = 'Course' AND context_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM courses WHERE course_code LIKE $1",
        "DELETE FROM users WHERE email LIKE $1",
        "DELETE FROM accounts WHERE name LIKE $1",
    ];
    for statement in statements {
        let _ = sqlx::query(statement).bind(&pattern).execute(pool).await;
    }
    // The feature flags of the seeded accounts.
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Account' \
           AND context_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
}

/// A course with a teacher, a student, a module and one assignment item.
struct Fixture {
    account_id: i64,
    course_id: i64,
    teacher_id: i64,
    student_id: i64,
    student_enrollment_id: i64,
    assignment_id: i64,
    module_item_id: i64,
}

async fn seed_fixture(pool: &PgPool, prefix: &str) -> Fixture {
    let account_id = seed_account(pool, prefix, "Account").await;
    let course_id = seed_course(pool, prefix, "Course", account_id).await;
    enable_course_paces(pool, course_id).await;

    let teacher_id = seed_user(pool, prefix, "Teacher", "teacher").await;
    seed_enrollment(pool, teacher_id, course_id, "teacher").await;
    let student_id = seed_user(pool, prefix, "Student", "student").await;
    let student_enrollment_id = seed_enrollment(pool, student_id, course_id, "student").await;

    let module_id = seed_module(pool, course_id, "Module").await;
    let assignment_id = seed_assignment(pool, course_id, "Assignment").await;
    let module_item_id = seed_module_item(
        pool,
        module_id,
        "assignment",
        assignment_id,
        1,
        "Assignment",
    )
    .await;

    Fixture {
        account_id,
        course_id,
        teacher_id,
        student_id,
        student_enrollment_id,
        assignment_id,
        module_item_id,
    }
}

#[tokio::test]
async fn create_publishes_the_pace_and_answers_the_presenter() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!("/courses/{}/course_pacing", fixture.course_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "course_pace": {
                "workflow_state": "active",
                "course_pace_module_items_attributes": [
                    { "module_item_id": fixture.module_item_id, "duration": 2 }
                ]
            }
        }))
        .await;

    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();

    let pace = &body["course_pace"];
    assert_eq!(pace["course_id"], fixture.course_id);
    assert_eq!(pace["workflow_state"], "active");
    assert_eq!(pace["context_type"], "Course");
    assert_eq!(pace["exclude_weekends"], false);
    assert!(pace["id"].as_i64().is_some(), "the pace is saved");

    let items = pace["modules"][0]["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["module_item_id"], fixture.module_item_id);
    assert_eq!(items[0]["duration"], 2);
    assert_eq!(items[0]["module_item_type"], "Assignment");
    assert_eq!(items[0]["published"], true);
    assert_eq!(items[0]["assignment_id"], fixture.assignment_id);
    assert_eq!(items[0]["assignment_title"], "Assignment");
    assert_eq!(
        items[0]["assignment_link"],
        format!(
            "/courses/{}/modules/items/{}",
            fixture.course_id, fixture.module_item_id
        )
    );

    // The publish progress is queued, exactly like the Canvas answer.
    assert_eq!(body["progress"]["workflow_state"], "queued");
    assert_eq!(body["progress"]["context_type"], "CoursePace");
    assert_eq!(body["progress"]["tag"], "course_pace_publish");
    assert!(body["progress"]["id"].as_i64().is_some());

    // The pace is published in the database.
    let workflow_state: Option<(String,)> =
        sqlx::query_as("SELECT workflow_state FROM course_paces WHERE id = $1")
            .bind(pace["id"].as_i64().unwrap())
            .fetch_optional(&pool)
            .await
            .expect("load the pace");
    assert_eq!(workflow_state.expect("pace row").0, "active");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_routes_are_404_without_the_enable_course_paces_setting() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let account_id = seed_account(&pool, &prefix, "Account").await;
    let course_id = seed_course(&pool, &prefix, "Course", account_id).await;
    let teacher_id = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    seed_enrollment(&pool, teacher_id, course_id, "teacher").await;

    let resp = server
        .get(&format!("/courses/{course_id}/course_pacing/new"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_answers_the_pace_and_its_latest_progress() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let created = server
        .post(&format!("/courses/{}/course_pacing", fixture.course_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "course_pace": { "workflow_state": "active" }
        }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let pace_id = created.json::<Value>()["course_pace"]["id"]
        .as_i64()
        .expect("pace id");

    let resp = server
        .get(&format!(
            "/courses/{}/course_pacing/{}",
            fixture.course_id, pace_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["course_pace"]["id"], pace_id);
    assert_eq!(body["course_pace"]["course_id"], fixture.course_id);
    assert_eq!(body["progress"]["workflow_state"], "queued");
    assert_eq!(body["progress"]["context_id"], pace_id);

    // An unknown pace is a 404.
    let resp = server
        .get(&format!(
            "/courses/{}/course_pacing/{}",
            fixture.course_id,
            pace_id + 999_999
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn new_scaffolds_a_pace_when_the_course_has_none() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .get(&format!("/courses/{}/course_pacing/new", fixture.course_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();

    let pace = &body["course_pace"];
    assert!(pace["id"].is_null(), "the scaffold is not saved");
    assert_eq!(pace["course_id"], fixture.course_id);
    assert_eq!(pace["workflow_state"], "unpublished");

    let items = pace["modules"][0]["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["module_item_id"], fixture.module_item_id);
    assert_eq!(items[0]["duration"], 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_changes_the_durations_and_queues_a_publish() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let created = server
        .post(&format!("/courses/{}/course_pacing", fixture.course_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "course_pace": {
                "workflow_state": "active",
                "course_pace_module_items_attributes": [
                    { "module_item_id": fixture.module_item_id, "duration": 1 }
                ]
            }
        }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let created_body: Value = created.json();
    let pace_id = created_body["course_pace"]["id"].as_i64().expect("pace id");
    let pace_item_id = created_body["course_pace"]["modules"][0]["items"][0]["id"]
        .as_i64()
        .expect("pace item id");

    let resp = server
        .put(&format!(
            "/courses/{}/course_pacing/{}",
            fixture.course_id, pace_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "course_pace": {
                "course_pace_module_items_attributes": [
                    { "id": pace_item_id, "module_item_id": fixture.module_item_id, "duration": 5 }
                ]
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["course_pace"]["modules"][0]["items"][0]["duration"], 5,
        "body: {body}"
    );
    assert_eq!(body["progress"]["workflow_state"], "queued");

    let duration: Option<(i32,)> =
        sqlx::query_as("SELECT duration FROM course_pace_module_items WHERE id = $1")
            .bind(pace_item_id)
            .fetch_optional(&pool)
            .await
            .expect("load the pace item");
    assert_eq!(duration.expect("pace item row").0, 5);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn destroy_soft_deletes_a_pace_and_refuses_the_default_pace() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    // The published primary pace cannot be deleted.
    let created = server
        .post(&format!("/courses/{}/course_pacing", fixture.course_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({ "course_pace": { "workflow_state": "active" } }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let primary_id = created.json::<Value>()["course_pace"]["id"]
        .as_i64()
        .expect("pace id");

    let resp = server
        .delete(&format!(
            "/courses/{}/course_pacing/{}",
            fixture.course_id, primary_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["success"], false);
    assert_eq!(
        body["errors"],
        json!("You cannot delete the default course pace.")
    );

    // A section pace is deleted softly.
    let section_id: i64 = sqlx::query_scalar(
        "INSERT INTO course_sections (course_id, name, workflow_state) \
         VALUES ($1, 'Section', 'active') RETURNING id",
    )
    .bind(fixture.course_id)
    .fetch_one(&pool)
    .await
    .expect("seed section");

    let created = server
        .post(&format!("/courses/{}/course_pacing", fixture.course_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "course_pace": { "workflow_state": "active", "course_section_id": section_id }
        }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let section_pace_id = created.json::<Value>()["course_pace"]["id"]
        .as_i64()
        .expect("pace id");

    let resp = server
        .delete(&format!(
            "/courses/{}/course_pacing/{}",
            fixture.course_id, section_pace_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["course_pace"]["workflow_state"],
        "deleted"
    );

    let workflow_state: Option<(String,)> =
        sqlx::query_as("SELECT workflow_state FROM course_paces WHERE id = $1")
            .bind(section_pace_id)
            .fetch_optional(&pool)
            .await
            .expect("load the pace");
    assert_eq!(workflow_state.expect("pace row").0, "deleted");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn publish_queues_a_publish_progress() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let created = server
        .post(&format!("/courses/{}/course_pacing", fixture.course_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({ "course_pace": { "workflow_state": "active" } }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let pace_id = created.json::<Value>()["course_pace"]["id"]
        .as_i64()
        .expect("pace id");

    let resp = server
        .post(&format!(
            "/courses/{}/course_pacing/{}/publish",
            fixture.course_id, pace_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["context_type"], "CoursePace");
    assert_eq!(body["context_id"], pace_id);
    assert_eq!(body["workflow_state"], "queued");
    assert_eq!(body["tag"], "course_pace_publish");
    assert_eq!(body["completion"], 0);
    assert!(body["url"]
        .as_str()
        .unwrap()
        .starts_with("/api/v1/progress/"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_create_enrollment_paces_creates_a_pace_per_enrollment() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!(
            "/courses/{}/course_pacing/bulk_create_enrollment_paces",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "enrollment_ids": [fixture.student_enrollment_id],
            "course_pace": { "workflow_state": "active" }
        }))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    let bulk_progress: Option<(i64,)> = sqlx::query_as(
        "SELECT COUNT(*) FROM progress WHERE tag = 'bulk_assign_paces' AND context_id = $1",
    )
    .bind(fixture.course_id)
    .fetch_optional(&pool)
    .await
    .expect("count the bulk progress");
    assert_eq!(bulk_progress.expect("count").0, 1);

    let pace_user: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT user_id FROM course_paces WHERE course_id = $1 AND user_id = $2")
            .bind(fixture.course_id)
            .bind(fixture.student_id)
            .fetch_optional(&pool)
            .await
            .expect("load the enrollment pace");
    assert_eq!(
        pace_user.expect("enrollment pace").0,
        Some(fixture.student_id)
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn compress_dates_answers_the_compressed_due_dates() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;
    seed_feature(&pool, fixture.account_id, "course_paces_skip_selected_days").await;
    seed_blackout_date(
        &pool,
        fixture.course_id,
        "2021-10-03T00:00:00Z",
        "2021-10-03T00:00:00Z",
    )
    .await;

    // A second assignment item for the two item compression.
    let module_id: i64 = sqlx::query_as("SELECT id FROM modules WHERE course_id = $1")
        .bind(fixture.course_id)
        .fetch_one(&pool)
        .await
        .expect("load the module");
    let second_assignment = seed_assignment(&pool, fixture.course_id, "Second").await;
    let second_item = seed_module_item(
        &pool,
        module_id,
        "assignment",
        second_assignment,
        2,
        "Second",
    )
    .await;

    let resp = server
        .post(&format!(
            "/courses/{}/course_pacing/compress_dates",
            fixture.course_id
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "course_pace": {
                "start_date": "2021-09-30",
                "end_date": "2021-10-05",
                "selected_days_to_skip": ["mon", "tue", "thu"],
                "course_pace_module_items_attributes": [
                    { "module_item_id": fixture.module_item_id, "duration": 1 },
                    { "module_item_id": second_item, "duration": 10 }
                ]
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let mut expected = serde_json::Map::new();
    expected.insert(
        fixture.module_item_id.to_string(),
        json!("2021-10-01T00:00:00Z"),
    );
    expected.insert(second_item.to_string(), json!("2021-10-02T00:00:00Z"));
    assert_eq!(body, Value::Object(expected), "body: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_pace_with_a_section_and_a_user_is_rejected() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;
    let section_id: i64 = sqlx::query_scalar(
        "INSERT INTO course_sections (course_id, name, workflow_state) \
         VALUES ($1, 'Section', 'active') RETURNING id",
    )
    .bind(fixture.course_id)
    .fetch_one(&pool)
    .await
    .expect("seed section");

    let resp = server
        .post(&format!("/courses/{}/course_pacing", fixture.course_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.teacher_id, "teacher"),
        )
        .json(&json!({
            "course_pace": {
                "course_section_id": section_id,
                "user_id": fixture.student_id
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["success"], false);
    assert_eq!(
        body["errors"],
        json!(["Only one of course_section_id and user_id can be given"])
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_may_not_manage_the_course_paces() {
    let Some((server, pool, prefix)) = setup().await else {
        return;
    };
    let fixture = seed_fixture(&pool, &prefix).await;

    let resp = server
        .get(&format!("/courses/{}/course_pacing/new", fixture.course_id))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(fixture.student_id, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
