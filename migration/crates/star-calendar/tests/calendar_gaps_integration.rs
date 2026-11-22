//! Integration tests for the P3 calendar gaps in star-calendar:
//!
//! - POST /calendar_events/{id}/reservations/{participant_id} (reserve —
//!   path-participant spelling of the sibling reservations route)
//! - GET  /calendar_events/{id}/participants
//! - GET/POST /courses/{courseId}/calendar_events/timetable (feature-off)
//! - POST /courses/{courseId}/calendar_events/timetable_events (feature-off)
//! - GET  /appointment_groups/next_appointment
//! - GET  /appointment_groups/{id}/groups
//!
//! Conventions mirror `tests/planner_overrides_integration.rs`: skipped
//! cleanly when DATABASE_URL is unset, unique per-run prefix, full teardown.
//!
//! Behavior pinned:
//! - the path-participant reserve route behaves exactly like the sibling
//!   body-participant route: self-reserve creates the child reservation event
//!   and locks the slot, reserving for someone else without manager rights is
//!   the Canvas 400 array shape, unknown slots are 404;
//! - participants lists the reservation holders for a manager of the slot's
//!   context (403 otherwise) and answers `[]` for plain events;
//! - the timetable endpoints answer the empty feature-off shapes for
//!   teachers, 403 for students and 404 for unknown courses;
//! - next_appointment answers the caller's earliest not-yet-ended
//!   reservation as the child-event JSON, 404 when there is none;
//! - groups lists the groups of the appointment group's group-category
//!   sub-contexts for anyone who can read the group.

use axum_test::TestServer;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "p3cal-{}-{}-{}",
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
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_calendar::router())
            .with_state(state),
    )
    .expect("build TestServer");
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

/// Active appointment group tied to the course (the Canvas Scheduler model).
async fn seed_appointment_group(pool: &PgPool, prefix: &str, name: &str, course_id: i64) -> i64 {
    let group: i64 = sqlx::query_scalar(
        "INSERT INTO appointment_groups (title, workflow_state) VALUES ($1, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed appointment group");
    sqlx::query(
        r#"
        INSERT INTO appointment_group_contexts
            (appointment_group_id, context_type, context_id, context_code)
        VALUES ($1, 'Course', $2, $3)
        "#,
    )
    .bind(group)
    .bind(course_id)
    .bind(format!("course_{course_id}"))
    .execute(pool)
    .await
    .expect("seed group context");
    group
}

/// A future time slot for the group.
async fn seed_slot(pool: &PgPool, name: &str, group_id: i64, hours_from_now: i64) -> i64 {
    let start = Utc::now() + Duration::hours(hours_from_now);
    let end = start + Duration::hours(1);
    sqlx::query_scalar(
        r#"
        INSERT INTO calendar_events
            (appointment_group_id, title, start_at, end_at, workflow_state)
        VALUES ($1, $2, $3, $4, 'active')
        RETURNING id
        "#,
    )
    .bind(group_id)
    .bind(name)
    .bind(start)
    .bind(end)
    .fetch_one(pool)
    .await
    .expect("seed slot")
}

/// A seeded reservation (child event) for `user` on `slot`.
async fn seed_reservation(
    pool: &PgPool,
    name: &str,
    user_id: i64,
    group_id: i64,
    slot_id: i64,
    hours_from_now: i64,
) -> i64 {
    let start = Utc::now() + Duration::hours(hours_from_now);
    let end = start + Duration::hours(1);
    sqlx::query_scalar(
        r#"
        INSERT INTO calendar_events
            (user_id, appointment_group_id, parent_event_id, title,
             start_at, end_at, workflow_state)
        VALUES ($1, $2, $3, $4, $5, $6, 'locked')
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(group_id)
    .bind(slot_id)
    .bind(name)
    .bind(start)
    .bind(end)
    .fetch_one(pool)
    .await
    .expect("seed reservation")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM calendar_events WHERE title LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM groups WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM group_categories WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM appointment_groups WHERE title LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
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
// Fixtures
// ---------------------------------------------------------------------------

/// Teacher + student + course + active appointment group + a future slot.
async fn scheduler_fixture(pool: &PgPool, prefix: &str) -> (i64, i64, i64, i64, i64) {
    let teacher = seed_user(pool, prefix, "Teacher", "teacher").await;
    let student = seed_user(pool, prefix, "Student", "student").await;
    let course = seed_course(pool, prefix, "Course").await;
    seed_enrollment(pool, teacher, course, "teacher").await;
    seed_enrollment(pool, student, course, "student").await;
    let group = seed_appointment_group(pool, prefix, "Group", course).await;
    let slot = seed_slot(pool, &format!("{prefix}-slot"), group, 3).await;
    (teacher, student, course, group, slot)
}

// ---------------------------------------------------------------------------
// POST /calendar_events/{id}/reservations/{participant_id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reserve_for_participant_creates_the_child_reservation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let (teacher, student, _course, _group, slot) = scheduler_fixture(&pool, &prefix).await;

    let resp = server
        .post(&format!("/calendar_events/{slot}/reservations/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["parent_event_id"], json!(slot));
    assert_eq!(body["user_id"], json!(student));
    assert_eq!(body["workflow_state"], json!("locked"));
    assert_eq!(body["type"], json!("event"));
    assert_eq!(body["child_events"], json!([]));
    assert!(body["appointment_group_id"].as_i64().is_some());

    // Canvas locks the slot once it has a reservation.
    let (slot_state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM calendar_events WHERE id = $1")
            .bind(slot)
            .fetch_one(&pool)
            .await
            .expect("reload slot");
    assert_eq!(slot_state, "locked");

    // Unknown slot: 404.
    let resp = server
        .post(&format!("/calendar_events/99999999/reservations/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Reserving for someone else without manager rights: the Canvas 400
    // array shape.
    let resp = server
        .post(&format!("/calendar_events/{slot}/reservations/{teacher}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    let first = body
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(first["attribute"], json!("reservation"));
    assert_eq!(first["type"], json!("calendar_event"));
    assert_eq!(first["message"], json!("invalid participant"));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /calendar_events/{id}/participants
// ---------------------------------------------------------------------------

#[tokio::test]
async fn participants_list_requires_a_manager_of_the_context() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let (teacher, student, course, group, slot) = scheduler_fixture(&pool, &prefix).await;
    seed_reservation(&pool, &format!("{prefix}-res"), student, group, slot, 3).await;

    let resp = server
        .get(&format!("/calendar_events/{slot}/participants"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let participants = body["participants"].as_array().cloned().unwrap_or_default();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0]["id"], json!(student));
    assert_eq!(participants[0]["name"], json!("Student"));

    // Students cannot see who signed up.
    let resp = server
        .get(&format!("/calendar_events/{slot}/participants"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // A slot without reservations answers the empty list.
    let empty_slot = seed_slot(&pool, &format!("{prefix}-slot2"), group, 4).await;
    let resp = server
        .get(&format!("/calendar_events/{empty_slot}/participants"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["participants"], json!([]));

    // A plain course event (no appointment group) answers [] for its teacher.
    let plain: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO calendar_events (course_id, title, start_at, end_at, workflow_state)
        VALUES ($1, $2, now() + interval '1 day', now() + interval '2 hours' + interval '1 day', 'active')
        RETURNING id
        "#,
    )
    .bind(course)
    .bind(format!("{prefix}-plain"))
    .fetch_one(&pool)
    .await
    .expect("seed plain event");
    let resp = server
        .get(&format!("/calendar_events/{plain}/participants"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["participants"], json!([]));

    // Unknown event: 404.
    let resp = server
        .get("/calendar_events/99999999/participants")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// timetable feature-off routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn timetable_routes_answer_feature_off_shapes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let (teacher, student, course, _group, _slot) = scheduler_fixture(&pool, &prefix).await;

    let resp = server
        .get(&format!("/courses/{course}/calendar_events/timetable"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({ "timetable": [] }));

    let resp = server
        .post(&format!("/courses/{course}/calendar_events/timetable"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "timetable": { "1": ["monday"] } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({ "timetable": [] }));

    let resp = server
        .post(&format!(
            "/courses/{course}/calendar_events/timetable_events"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "timetable": { "1": ["monday"] } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({ "timetable_events": [] }));

    // Students manage no calendar: 403.
    let resp = server
        .get(&format!("/courses/{course}/calendar_events/timetable"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Unknown course: 404.
    let resp = server
        .get("/courses/99999999/calendar_events/timetable")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /appointment_groups/next_appointment
// ---------------------------------------------------------------------------

#[tokio::test]
async fn next_appointment_answers_the_earliest_upcoming_reservation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let (_teacher, student, _course, group, slot) = scheduler_fixture(&pool, &prefix).await;
    let later_slot = seed_slot(&pool, &format!("{prefix}-slot-late"), group, 5).await;

    // Without a reservation: 404 (Canvas' "no next appointment").
    let resp = server
        .get("/appointment_groups/next_appointment")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    let earlier = seed_reservation(
        &pool,
        &format!("{prefix}-res-early"),
        student,
        group,
        slot,
        3,
    )
    .await;
    let _later = seed_reservation(
        &pool,
        &format!("{prefix}-res-late"),
        student,
        group,
        later_slot,
        5,
    )
    .await;
    // A reservation that already ended never wins.
    let past_slot = seed_slot(&pool, &format!("{prefix}-slot-past"), group, -5).await;
    let _past = seed_reservation(
        &pool,
        &format!("{prefix}-res-past"),
        student,
        group,
        past_slot,
        -5,
    )
    .await;

    let resp = server
        .get("/appointment_groups/next_appointment")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], json!(earlier), "earliest upcoming wins");
    assert_eq!(body["user_id"], json!(student));
    assert_eq!(body["appointment_group_id"], json!(group));
    assert_eq!(body["parent_event_id"], json!(slot));
    assert_eq!(body["type"], json!("event"));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /appointment_groups/{id}/groups
// ---------------------------------------------------------------------------

#[tokio::test]
async fn appointment_group_groups_lists_category_groups() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let (_teacher, student, course, group, _slot) = scheduler_fixture(&pool, &prefix).await;

    // No group-category sub-contexts: empty eligible list.
    let resp = server
        .get(&format!("/appointment_groups/{group}/groups"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["groups"], json!([]));

    // A group-category sub-context makes its groups eligible.
    let category: i64 = sqlx::query_scalar(
        "INSERT INTO group_categories (course_id, name) VALUES ($1, $2) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-category"))
    .fetch_one(&pool)
    .await
    .expect("seed category");
    for name in ["A", "B"] {
        sqlx::query("INSERT INTO groups (name, group_category_id) VALUES ($1, $2)")
            .bind(format!("{prefix}-group-{name}"))
            .bind(category)
            .execute(&pool)
            .await
            .expect("seed group");
    }
    sqlx::query(
        r#"
        INSERT INTO appointment_group_sub_contexts
            (appointment_group_id, sub_context_type, sub_context_id, sub_context_code, participant_type)
        VALUES ($1, 'GroupCategory', $2, $3, 'Group')
        "#,
    )
    .bind(group)
    .bind(category)
    .bind(format!("group_category_{category}"))
    .execute(&pool)
    .await
    .expect("seed sub-context");

    let resp = server
        .get(&format!("/appointment_groups/{group}/groups"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let groups = body["groups"].as_array().cloned().unwrap_or_default();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["group_category_id"], json!(category));
    assert_eq!(groups[0]["name"], json!(format!("{prefix}-group-A")));

    // Unknown group: 404.
    let resp = server
        .get("/appointment_groups/99999999/groups")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}
