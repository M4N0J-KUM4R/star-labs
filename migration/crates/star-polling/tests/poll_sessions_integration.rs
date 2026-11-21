//! Integration tests for the Canvas poll session routes
//! (`Polling::PollSessionsController`).
//!
//! Pinned behaviour (verified against
//! `canvas-src/spec/apis/v1/polling/poll_sessions_api_spec.rb`):
//! - every answer is wrapped in `{"poll_sessions": [...]}` — a single record
//!   too (`serialize_jsonapi` uses `Array.wrap`) — and the ids are strings;
//! - a teacher sees `results` / `has_public_results` and every submission, a
//!   student sees neither the results nor the other students' submissions;
//! - a missing poll or session is a 404, a failed permission a 403, a failed
//!   validation a 400;
//! - `open` / `close` flip `is_published`;
//! - `opened` / `closed` only list the sessions of the courses (and sections)
//!   the caller is enrolled in;
//! - `Accept: application/vnd.api+json` adds the `meta.pagination` block.
//!
//! Fleet rule: dedicated users, courses, sections, polls and sessions per
//! test, every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is
//! unset.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// The polling tables of migration `0223_polling.sql`. The coordinator owns
/// the migration; the test creates them when they are missing so that the
/// suite is self contained. The shape matches the migration exactly.
const POLLING_DDL: [&str; 4] = [
    "CREATE TABLE IF NOT EXISTS polls ( \
        id         BIGSERIAL PRIMARY KEY, \
        user_id    BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE, \
        course_id  BIGINT REFERENCES courses(id) ON DELETE CASCADE, \
        question   TEXT NOT NULL, \
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now())",
    "CREATE TABLE IF NOT EXISTS poll_choices ( \
        id          BIGSERIAL PRIMARY KEY, \
        poll_id     BIGINT NOT NULL REFERENCES polls(id) ON DELETE CASCADE, \
        name        TEXT NOT NULL, \
        description TEXT, \
        created_at  TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_at  TIMESTAMPTZ NOT NULL DEFAULT now())",
    "CREATE TABLE IF NOT EXISTS poll_sessions ( \
        id                 BIGSERIAL PRIMARY KEY, \
        poll_id            BIGINT NOT NULL REFERENCES polls(id) ON DELETE CASCADE, \
        course_id          BIGINT NOT NULL REFERENCES courses(id) ON DELETE CASCADE, \
        course_section_id  BIGINT REFERENCES course_sections(id) ON DELETE SET NULL, \
        has_public_results BOOLEAN NOT NULL DEFAULT false, \
        is_published       BOOLEAN NOT NULL DEFAULT false, \
        created_at         TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_at         TIMESTAMPTZ NOT NULL DEFAULT now())",
    "CREATE TABLE IF NOT EXISTS poll_submissions ( \
        id              BIGSERIAL PRIMARY KEY, \
        poll_session_id BIGINT NOT NULL REFERENCES poll_sessions(id) ON DELETE CASCADE, \
        poll_choice_id  BIGINT NOT NULL REFERENCES poll_choices(id) ON DELETE CASCADE, \
        user_id         BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE, \
        created_at      TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_at      TIMESTAMPTZ NOT NULL DEFAULT now())",
];

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "poll-sess-{}-{}-{}",
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

    for statement in POLLING_DDL {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("create polling tables");
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_polling::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
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

async fn seed_section(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO course_sections (course_id, name) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed course section")
}

async fn seed_enrollment(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    section_id: Option<i64>,
    role: &str,
) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, course_section_id, role, workflow_state) \
         VALUES ($1, $2, $3, $4, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(section_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

async fn seed_poll(pool: &PgPool, prefix: &str, name: &str, user_id: i64) -> i64 {
    sqlx::query_scalar("INSERT INTO polls (user_id, question) VALUES ($1, $2) RETURNING id")
        .bind(user_id)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed poll")
}

async fn seed_choice(pool: &PgPool, poll_id: i64, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO poll_choices (poll_id, name) VALUES ($1, $2) RETURNING id")
        .bind(poll_id)
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("seed poll choice")
}

async fn seed_session(
    pool: &PgPool,
    poll_id: i64,
    course_id: i64,
    section_id: Option<i64>,
    published: bool,
    public_results: bool,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO poll_sessions \
           (poll_id, course_id, course_section_id, is_published, has_public_results) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(poll_id)
    .bind(course_id)
    .bind(section_id)
    .bind(published)
    .bind(public_results)
    .fetch_one(pool)
    .await
    .expect("seed poll session")
}

async fn seed_submission(pool: &PgPool, session_id: i64, choice_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO poll_submissions (poll_session_id, poll_choice_id, user_id) \
         VALUES ($1, $2, $3)",
    )
    .bind(session_id)
    .bind(choice_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed poll submission");
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let like = format!("{prefix}%");
    for statement in [
        "DELETE FROM poll_submissions WHERE poll_session_id IN \
           (SELECT ps.id FROM poll_sessions ps JOIN polls p ON p.id = ps.poll_id \
            WHERE p.question LIKE $1)",
        "DELETE FROM poll_sessions WHERE poll_id IN (SELECT id FROM polls WHERE question LIKE $1)",
        "DELETE FROM poll_choices WHERE poll_id IN (SELECT id FROM polls WHERE question LIKE $1)",
        "DELETE FROM polls WHERE question LIKE $1",
        "DELETE FROM enrollments WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM course_sections WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
        "DELETE FROM courses WHERE course_code LIKE $1",
    ] {
        let _ = sqlx::query(statement)
            .bind(&like)
            .execute(pool)
            .await;
    }
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{like}@test.local"))
        .execute(pool)
        .await;
}

/// The `poll_sessions` collection of a Canvas answer.
fn sessions_of(body: &Value) -> &Vec<Value> {
    body["poll_sessions"]
        .as_array()
        .expect("poll_sessions array")
}

#[tokio::test]
async fn index_lists_the_sessions_of_the_poll() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_enrollment(&pool, teacher, course, Some(section), "teacher").await;
    seed_enrollment(&pool, student, course, Some(section), "student").await;
    let poll = seed_poll(&pool, &prefix, "Poll", teacher).await;
    for _ in 0..3 {
        seed_session(&pool, poll, course, Some(section), false, false).await;
    }

    // The poll creator lists the sessions of the poll.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = sessions_of(&body);
    assert_eq!(items.len(), 3);
    for item in items {
        // Canvas stringifies the ids.
        assert!(item["id"].is_string(), "id is a string: {}", item);
        assert_eq!(item["poll_id"], poll.to_string());
        assert_eq!(item["course_id"], course.to_string());
        assert_eq!(item["course_section_id"], section.to_string());
        assert_eq!(item["is_published"], false);
        assert_eq!(item["has_submitted"], false);
    }

    // A student may not list the sessions of the poll.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An unknown poll is a 404.
    let resp = server
        .get("/polls/999999999/poll_sessions")
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn show_hides_the_results_from_the_students() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    let other_section = seed_section(&pool, course, "Other Section").await;
    seed_enrollment(&pool, teacher, course, Some(section), "teacher").await;
    seed_enrollment(&pool, student, course, Some(section), "student").await;
    seed_enrollment(&pool, other, course, Some(section), "student").await;
    let poll = seed_poll(&pool, &prefix, "Poll", teacher).await;
    let choice = seed_choice(&pool, poll, "Choice A").await;
    let session = seed_session(&pool, poll, course, Some(section), true, false).await;
    seed_submission(&pool, session, choice, student).await;
    seed_submission(&pool, session, choice, other).await;

    // The teacher sees the results and every submission.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = sessions_of(&body);
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item["id"], session.to_string());
    assert_eq!(item["is_published"], true);
    assert_eq!(item["has_public_results"], false);
    assert_eq!(item["results"][&choice.to_string()], json!(2));
    assert_eq!(item["poll_submissions"].as_array().unwrap().len(), 2);

    // The student sees no results and only their own submission.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = sessions_of(&body);
    let item = &items[0];
    assert!(item.get("results").is_none(), "no results: {}", item);
    assert!(
        item.get("has_public_results").is_none(),
        "no flag: {}",
        item
    );
    assert_eq!(item["has_submitted"], true);
    let submissions = item["poll_submissions"].as_array().unwrap();
    assert_eq!(submissions.len(), 1);
    assert_eq!(submissions[0]["user_id"], student.to_string());
    assert_eq!(submissions[0]["poll_session_id"], session.to_string());

    // A student that is not enrolled in the section of the session is a 403.
    sqlx::query(
        "UPDATE enrollments SET course_section_id = $1 WHERE user_id = $2 AND course_id = $3",
    )
    .bind(other_section)
    .bind(student)
    .bind(course)
    .execute(&pool)
    .await
    .expect("move the enrolment");
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An unknown session is a 404.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/999999999"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_binds_the_course_and_the_section() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_enrollment(&pool, teacher, course, Some(section), "teacher").await;
    seed_enrollment(&pool, student, course, Some(section), "student").await;
    let poll = seed_poll(&pool, &prefix, "Poll", teacher).await;

    // A teacher creates a session for a course section.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "poll_sessions": [{
                "course_id": course,
                "course_section_id": section,
                "has_public_results": true,
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = sessions_of(&body);
    assert_eq!(items.len(), 1);
    let _session_id = items[0]["id"].as_str().unwrap().parse::<i64>().unwrap();
    assert_eq!(items[0]["course_id"], course.to_string());
    assert_eq!(items[0]["course_section_id"], section.to_string());
    assert_eq!(items[0]["has_public_results"], true);
    assert_eq!(items[0]["is_published"], false);

    // A blank has_public_results is stored as false.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "poll_sessions": [{
                "course_id": course,
                "course_section_id": section,
                "has_public_results": "",
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(sessions_of(&body)[0]["has_public_results"], false);

    // A section that does not belong to the course is a 404.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "poll_sessions": [{
                "course_id": course,
                "course_section_id": section + 666,
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A missing course is a 404.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "poll_sessions": [{ "course_id": 999999999 }] }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A missing course_id is the Ruby "Course is required." 404.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "poll_sessions": [{ "has_public_results": true }] }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A student may not create a session.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "poll_sessions": [{ "course_id": course, "course_section_id": section }]
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_changes_the_session() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    let other_section = seed_section(&pool, course, "Other Section").await;
    seed_enrollment(&pool, teacher, course, Some(section), "teacher").await;
    seed_enrollment(&pool, student, course, Some(section), "student").await;
    let poll = seed_poll(&pool, &prefix, "Poll", teacher).await;
    let session = seed_session(&pool, poll, course, Some(section), false, false).await;

    // The poll creator moves the session to another section.
    let resp = server
        .put(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "poll_sessions": [{
                "course_section_id": other_section,
                "has_public_results": true,
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = sessions_of(&body);
    assert_eq!(items[0]["course_section_id"], other_section.to_string());
    assert_eq!(items[0]["has_public_results"], true);

    // A student may not update the session.
    let resp = server
        .put(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "poll_sessions": [{ "has_public_results": false }] }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A section of another course fails the Canvas validation (400).
    let other_course = seed_course(&pool, &prefix, "Other Course").await;
    let foreign_section = seed_section(&pool, other_course, "Foreign Section").await;
    let resp = server
        .put(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "poll_sessions": [{ "course_section_id": foreign_section }]
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn destroy_removes_the_session() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_enrollment(&pool, teacher, course, Some(section), "teacher").await;
    seed_enrollment(&pool, student, course, Some(section), "student").await;
    let poll = seed_poll(&pool, &prefix, "Poll", teacher).await;
    let session = seed_session(&pool, poll, course, Some(section), false, false).await;

    // A student may not delete the session.
    let resp = server
        .delete(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The poll creator deletes it — 204 No Content.
    let resp = server
        .delete(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn open_and_close_flip_the_published_state() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_enrollment(&pool, teacher, course, Some(section), "teacher").await;
    seed_enrollment(&pool, student, course, Some(section), "student").await;
    let poll = seed_poll(&pool, &prefix, "Poll", teacher).await;
    let session = seed_session(&pool, poll, course, Some(section), false, false).await;

    // A student may not open the session.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}/open"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The poll creator opens it.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}/open"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(sessions_of(&body)[0]["is_published"], true);

    // ... and closes it again.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}/close"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(sessions_of(&body)[0]["is_published"], false);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn opened_and_closed_list_the_available_sessions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    let other_course = seed_course(&pool, &prefix, "Other Course").await;
    let other_section = seed_section(&pool, other_course, "Other Section").await;
    seed_enrollment(&pool, student, course, Some(section), "student").await;
    seed_enrollment(
        &pool,
        stranger,
        other_course,
        Some(other_section),
        "student",
    )
    .await;
    let poll = seed_poll(&pool, &prefix, "Poll", student).await;
    let other_poll = seed_poll(&pool, &prefix, "Other Poll", stranger).await;
    let published = seed_session(&pool, poll, course, Some(section), true, false).await;
    let closed = seed_session(&pool, poll, course, Some(section), false, false).await;
    let foreign = seed_session(
        &pool,
        other_poll,
        other_course,
        Some(other_section),
        true,
        false,
    )
    .await;

    // The opened list holds the published session of the caller's course.
    let resp = server
        .get("/poll_sessions/opened")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let ids: Vec<String> = sessions_of(&body)
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&published.to_string()), "{:?}", ids);
    assert!(!ids.contains(&closed.to_string()), "{:?}", ids);
    assert!(!ids.contains(&foreign.to_string()), "{:?}", ids);

    // The closed list holds the closed session.
    let resp = server
        .get("/poll_sessions/closed")
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let ids: Vec<String> = sessions_of(&body)
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_string())
        .collect();
    assert!(ids.contains(&closed.to_string()), "{:?}", ids);
    assert!(!ids.contains(&published.to_string()), "{:?}", ids);
    assert!(!ids.contains(&foreign.to_string()), "{:?}", ids);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn jsonapi_requests_carry_the_pagination_meta() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_enrollment(&pool, teacher, course, Some(section), "teacher").await;
    let poll = seed_poll(&pool, &prefix, "Poll", teacher).await;
    seed_session(&pool, poll, course, Some(section), false, false).await;

    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .add_header(axum::http::header::ACCEPT, "application/vnd.api+json")
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body.get("meta").is_some(), "meta: {}", body);
    assert_eq!(body["meta"]["primaryCollection"], "poll_sessions");
    assert_eq!(body["meta"]["pagination"]["count"], 1);
    assert_eq!(body["meta"]["pagination"]["per_page"], 10);
    assert_eq!(body["meta"]["pagination"]["page"], 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn anonymous_requests_are_unauthorized() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.get("/poll_sessions/opened").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    let resp = server.get("/polls/1/poll_sessions").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
}
