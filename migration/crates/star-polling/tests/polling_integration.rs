//! Integration tests for star-polling (issue #89, spec `specs/p5-rest-polling.md` §7).
//!
//! Pinned behavior:
//! - Poll lifecycle: array-wrapped create (200 + `polls` envelope with
//!   `meta.pagination`), show, update (the `is_correct` blank-preserve quirk —
//!   polls ignore the field entirely), validation 400s ("can't be blank" /
//!   255-char limit, Rails error bodies), delete → 204 → show 404.
//! - Choices: CRUD under an own poll, `position`-ordered listing, blank
//!   `is_correct` → false on create / preserved on update, cross-poll choice
//!   id → 404, cascade delete with the parent poll.
//! - Sessions: course/section binding (missing course, unknown course and a
//!   foreign section are 404; a section from another course on update is the
//!   upstream 400 validation body), `is_published` starts false, GET
//!   open/close transitions, global `opened`/`closed` lists scoped to the
//!   caller's polls.
//! - Submissions: enrolled student submits (200), a second submission by the
//!   same user is rejected (400 + "can only submit one choice per poll
//!   session."), non-enrolled → 403, closed session → 403, choice of another
//!   poll → 404, submitter/poll-owner read (200) vs other student (403).
//! - Permissions: anonymous 401 on all 21 routes; another user hits 404 on
//!   every out-of-scope poll-scoped route.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix plus
//! full teardown keeps shared-DB runs safe.

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
        "poll_{}_{}",
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

    let server = TestServer::new(star_polling::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_section(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO course_sections (course_id, name) VALUES ($1, $2) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed section")
}

async fn seed_enrollment(
    pool: &PgPool,
    user_id: i64,
    course_id: i64,
    role: &str,
    section: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, course_section_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .bind(section)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // polls cascade to choices/sessions/submissions (migration 0100); courses
    // cascade to sections and enrollments; users go last.
    let _ = sqlx::query(
        "DELETE FROM polls WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Small API helpers
// ---------------------------------------------------------------------------

async fn create_poll_ok(server: &TestServer, bearer: &str, question: &str) -> Value {
    let resp = server
        .post("/polls")
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&json!({ "polls": [{ "question": question }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    resp.json()
}

async fn create_choice_ok(server: &TestServer, bearer: &str, poll_id: i64, body: Value) -> Value {
    let resp = server
        .post(&format!("/polls/{poll_id}/poll_choices"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&json!({ "poll_choices": [body] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    resp.json()
}

async fn create_session_ok(server: &TestServer, bearer: &str, poll_id: i64, body: Value) -> Value {
    let resp = server
        .post(&format!("/polls/{poll_id}/poll_sessions"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&json!({ "poll_sessions": [body] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    resp.json()
}

fn first(body: &Value, root: &str) -> &Value {
    let items = body[root]
        .as_array()
        .unwrap_or_else(|| panic!("missing {root} array: {body}"));
    assert_eq!(items.len(), 1, "expected one item: {body}");
    &items[0]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn poll_lifecycle_and_update_quirk() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "teacher").await;
    let bearer = auth(owner, "teacher");

    // Create (array-wrapped) — 200 with the plural envelope and pagination.
    let created = create_poll_ok(&server, &bearer, "What is your favorite color?").await;
    assert_eq!(created["polls"].as_array().unwrap().len(), 1);
    let poll = first(&created, "polls");
    assert_eq!(poll["question"], "What is your favorite color?");
    assert_eq!(poll["user_id"], owner);
    assert!(poll["created_at"].is_string());
    assert_eq!(poll["total_results"], json!({}));
    assert_eq!(created["meta"]["pagination"]["total"], 1);
    let poll_id = poll["id"].as_i64().unwrap();

    // Canvas tolerates a plain (unwrapped) object body too.
    let resp = server
        .post("/polls")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "question": format!("{prefix} second") }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Validation: blank question and an over-long question are Rails 400s.
    let resp = server
        .post("/polls")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "polls": [{ "question": "   " }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["question"][0], "can't be blank");

    let resp = server
        .post("/polls")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "polls": [{ "question": "x".repeat(256) }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["question"][0], "is too long (maximum is 255 characters)");

    // Index: own polls only, newest first.
    let resp = server
        .get("/polls")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let items = listed["polls"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(listed["meta"]["pagination"]["total"], 2);
    assert_eq!(
        items[0]["question"],
        format!("{prefix} second"),
        "newest poll first: {listed}"
    );

    // Show.
    let resp = server
        .get(&format!("/polls/{poll_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(first(&shown, "polls")["id"], poll_id);
    assert!(shown["meta"]["pagination"].is_object());

    // Update — the is_correct blank-preserve quirk: polls#update drops a
    // blank is_correct (spec §6), and polls carry no such field at all, so
    // the update succeeds and simply ignores it (null, "" or true).
    let resp = server
        .put(&format!("/polls/{poll_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "polls": [{ "question": "Updated question", "is_correct": null }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(first(&updated, "polls")["question"], "Updated question");
    assert!(updated["polls"][0].get("is_correct").is_none());

    let resp = server
        .put(&format!("/polls/{poll_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "polls": [{ "description": "A description", "is_correct": "" }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        first(&resp.json::<Value>(), "polls")["description"],
        "A description"
    );

    // Update with a blank question is a 400.
    let resp = server
        .put(&format!("/polls/{poll_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "polls": [{ "question": "" }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Unknown ids are 404.
    let resp = server
        .put("/polls/999999999")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "polls": [{ "question": "x" }] }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Destroy: 204, then show is 404 (and a second destroy is 404).
    let resp = server
        .delete(&format!("/polls/{poll_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204);
    let resp = server
        .get(&format!("/polls/{poll_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .delete(&format!("/polls/{poll_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn choices_crud_cascade_and_scoping() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "teacher").await;
    let bearer = auth(owner, "teacher");
    let poll1 = create_poll_ok(&server, &bearer, "Poll one").await["polls"][0]["id"]
        .as_i64()
        .unwrap();
    let poll2 = create_poll_ok(&server, &bearer, "Poll two").await["polls"][0]["id"]
        .as_i64()
        .unwrap();

    // Create: string booleans coerce (Canvas param casting), blank is_correct
    // defaults to false, absent position is null.
    let created = create_choice_ok(
        &server,
        &bearer,
        poll1,
        json!({ "text": "Choice A", "is_correct": "true", "position": 2 }),
    )
    .await;
    let choice_a = first(&created, "poll_choices");
    assert_eq!(choice_a["text"], "Choice A");
    assert_eq!(choice_a["is_correct"], true);
    assert_eq!(choice_a["position"], 2);
    assert_eq!(choice_a["poll_id"], poll1);
    let choice_a_id = choice_a["id"].as_i64().unwrap();

    let created = create_choice_ok(&server, &bearer, poll1, json!({ "text": "Choice B" })).await;
    let choice_b = first(&created, "poll_choices");
    assert_eq!(choice_b["is_correct"], false);
    assert_eq!(choice_b["position"], Value::Null);

    let created = create_choice_ok(
        &server,
        &bearer,
        poll1,
        json!({ "text": "Choice C", "position": 1 }),
    )
    .await;
    let choice_c_id = created["poll_choices"][0]["id"].as_i64().unwrap();

    // A blank text is a 400.
    let resp = server
        .post(&format!("/polls/{poll1}/poll_choices"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_choices": [{ "text": "  " }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["text"][0], "can't be blank");

    // Index ordered by position (nulls last), id as tiebreaker.
    let resp = server
        .get(&format!("/polls/{poll1}/poll_choices"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let texts: Vec<&str> = listed["poll_choices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, vec!["Choice C", "Choice A", "Choice B"], "{listed}");
    assert_eq!(listed["meta"]["pagination"]["total"], 3);

    // Show.
    let resp = server
        .get(&format!("/polls/{poll1}/poll_choices/{choice_a_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(first(&resp.json::<Value>(), "poll_choices")["id"], choice_a_id);

    // A choice id of another poll is out of scope: 404 on show/update/delete.
    let resp = server
        .get(&format!("/polls/{poll2}/poll_choices/{choice_a_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .put(&format!("/polls/{poll2}/poll_choices/{choice_a_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_choices": [{ "text": "hijack" }] }))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .delete(&format!("/polls/{poll2}/poll_choices/{choice_a_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // Update: a blank/absent is_correct preserves the stored value (quirk).
    let resp = server
        .put(&format!("/polls/{poll1}/poll_choices/{choice_a_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_choices": [{ "text": "Choice A (edited)" }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated = first(&resp.json::<Value>(), "poll_choices");
    assert_eq!(updated["text"], "Choice A (edited)");
    assert_eq!(updated["is_correct"], true, "absent is_correct preserved");

    let resp = server
        .put(&format!("/polls/{poll1}/poll_choices/{choice_a_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_choices": [{ "is_correct": "" }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        first(&resp.json::<Value>(), "poll_choices")["is_correct"],
        true,
        "blank is_correct preserved"
    );

    // An explicit false does flip the value.
    let resp = server
        .put(&format!("/polls/{poll1}/poll_choices/{choice_a_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_choices": [{ "is_correct": false }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        first(&resp.json::<Value>(), "poll_choices")["is_correct"],
        false
    );

    // Destroy one choice: 204 then 404.
    let resp = server
        .delete(&format!("/polls/{poll1}/poll_choices/{choice_c_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204);
    let resp = server
        .get(&format!("/polls/{poll1}/poll_choices/{choice_c_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // Destroying the poll cascades to its choices (Canvas dependent: :destroy).
    let resp = server
        .delete(&format!("/polls/{poll1}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204);
    let (remaining,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM poll_choices WHERE poll_id = $1",
    )
    .bind(poll1)
    .fetch_one(&pool)
    .await
    .expect("count choices");
    assert_eq!(remaining, 0, "choices must cascade with the poll");
    let resp = server
        .get(&format!("/polls/{poll1}/poll_choices"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn sessions_state_transitions_and_global_lists() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let bearer = auth(teacher, "teacher");
    let student_bearer = auth(student, "student");

    let poll = create_poll_ok(&server, &bearer, "Session poll").await["polls"][0]["id"]
        .as_i64()
        .unwrap();
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section A").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    let other_section = seed_section(&pool, other_course, "Section B").await;

    // A session bound to course + section starts unpublished ("closed").
    let created =
        create_session_ok(&server, &bearer, poll, json!({ "course_id": course, "course_section_id": section })).await;
    let session = first(&created, "poll_sessions");
    assert_eq!(session["course_id"], course);
    assert_eq!(session["course_section_id"], section);
    assert_eq!(session["poll_id"], poll);
    assert_eq!(session["is_published"], false, "new sessions start closed");
    assert_eq!(session["has_public_results"], false, "blank → false");
    assert_eq!(session["results"], json!({}));
    assert_eq!(session["poll_submissions"], json!([]));
    assert_eq!(session["has_submitted"], false);
    let session_id = session["id"].as_i64().unwrap();

    // has_public_results: true round-trips.
    let created = create_session_ok(
        &server,
        &bearer,
        poll,
        json!({ "course_id": course, "has_public_results": true }),
    )
    .await;
    assert_eq!(first(&created, "poll_sessions")["has_public_results"], true);

    // Missing course, unknown course and a foreign section are all 404.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_sessions": [{ "has_public_results": true }] }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_sessions": [{ "course_id": 999999999 }] }))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_sessions": [{ "course_id": course, "course_section_id": other_section }] }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Index and show.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    assert!(listed["polls"].as_array().is_none()); // root is poll_sessions
    assert_eq!(listed["poll_sessions"].as_array().unwrap().len(), 2);
    assert_eq!(listed["meta"]["pagination"]["total"], 2);

    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(first(&resp.json::<Value>(), "poll_sessions")["id"], session_id);

    // Update: has_public_results flips; a section of another course on update
    // hits the upstream section_belongs_to_course validation → 400.
    let resp = server
        .put(&format!("/polls/{poll}/poll_sessions/{session_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_sessions": [{ "has_public_results": true }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        first(&resp.json::<Value>(), "poll_sessions")["has_public_results"],
        true
    );

    let resp = server
        .put(&format!("/polls/{poll}/poll_sessions/{session_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "poll_sessions": [{ "course_section_id": other_section }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["base"][0],
        "That course section does not belong to the existing course."
    );

    // GET open/close transitions (spec §6: GETs that mutate).
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session_id}/open"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(first(&resp.json::<Value>(), "poll_sessions")["is_published"], true);

    let resp = server
        .get("/poll_sessions/opened")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let opened: Value = resp.json();
    let opened_ids: Vec<i64> = opened["poll_sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_i64().unwrap())
        .collect();
    assert!(opened_ids.contains(&session_id), "session must be listed as opened: {opened}");
    assert!(opened["meta"]["pagination"].is_object());

    let resp = server
        .get("/poll_sessions/closed")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    let closed: Value = resp.json();
    let closed_ids: Vec<i64> = closed["poll_sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_i64().unwrap())
        .collect();
    assert!(!closed_ids.contains(&session_id), "open session must not be closed: {closed}");

    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session_id}/close"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(first(&resp.json::<Value>(), "poll_sessions")["is_published"], false);

    let resp = server
        .get("/poll_sessions/closed")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    let closed: Value = resp.json();
    let closed_ids: Vec<i64> = closed["poll_sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_i64().unwrap())
        .collect();
    assert!(closed_ids.contains(&session_id), "closed session listed: {closed}");
    let resp = server
        .get("/poll_sessions/opened")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    let opened: Value = resp.json();
    let opened_ids: Vec<i64> = opened["poll_sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_i64().unwrap())
        .collect();
    assert!(!opened_ids.contains(&session_id), "closed session not opened: {opened}");

    // The global lists only cover the caller's own polls: the student sees
    // nothing of the teacher's sessions.
    let resp = server
        .get("/poll_sessions/opened")
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let opened: Value = resp.json();
    assert_eq!(opened["poll_sessions"], json!([]), "no other-user sessions: {opened}");
    let resp = server
        .get("/poll_sessions/closed")
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.json::<Value>()["poll_sessions"], json!([]));

    // Non-owners are out of scope on every poll-scoped session route (404).
    for (method, path) in [
        ("get", format!("/polls/{poll}/poll_sessions")),
        ("get", format!("/polls/{poll}/poll_sessions/{session_id}")),
        ("put", format!("/polls/{poll}/poll_sessions/{session_id}")),
        ("delete", format!("/polls/{poll}/poll_sessions/{session_id}")),
        ("get", format!("/polls/{poll}/poll_sessions/{session_id}/open")),
        ("get", format!("/polls/{poll}/poll_sessions/{session_id}/close")),
    ] {
        let resp = match method {
            "get" => server
                .get(&path)
                .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
                .await,
            "put" => server
                .put(&path)
                .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
                .json(&json!({ "poll_sessions": [{ "has_public_results": true }] }))
                .await,
            _ => server
                .delete(&path)
                .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
                .await,
        };
        assert_eq!(resp.status_code(), 404, "{method} {path} as non-owner");
    }

    // Destroy: 204 then 404.
    let resp = server
        .delete(&format!("/polls/{poll}/poll_sessions/{session_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204);
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn submissions_flow_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student1 = seed_user(&pool, &prefix, "Student1", "student").await;
    let student2 = seed_user(&pool, &prefix, "Student2", "student").await;
    let student3 = seed_user(&pool, &prefix, "Student3", "student").await;
    let bearer = auth(teacher, "teacher");
    let s1 = auth(student1, "student");
    let s2 = auth(student2, "student");
    let s3 = auth(student3, "student");

    let poll = create_poll_ok(&server, &bearer, "Clicker poll").await["polls"][0]["id"]
        .as_i64()
        .unwrap();
    let other_poll = create_poll_ok(&server, &bearer, "Other poll").await["polls"][0]["id"]
        .as_i64()
        .unwrap();
    let choice1 = create_choice_ok(&server, &bearer, poll, json!({ "text": "Red" })).await
        ["poll_choices"][0]["id"]
        .as_i64()
        .unwrap();
    let choice2 = create_choice_ok(&server, &bearer, poll, json!({ "text": "Blue" })).await
        ["poll_choices"][0]["id"]
        .as_i64()
        .unwrap();
    let foreign_choice = create_choice_ok(&server, &bearer, other_poll, json!({ "text": "Green" }))
        .await["poll_choices"][0]["id"]
        .as_i64()
        .unwrap();

    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_enrollment(&pool, student1, course, "student", None).await;
    seed_enrollment(&pool, student2, course, "student", Some(section)).await;
    // student3 is deliberately not enrolled.

    let session = create_session_ok(&server, &bearer, poll, json!({ "course_id": course })).await
        ["poll_sessions"][0]["id"]
        .as_i64()
        .unwrap();

    // Submitting to a closed session is a 403 (the :submit ability requires
    // an open session).
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": choice1 }] }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Open the session.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}/open"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);

    // Enrolled course-wide student submits: 200 with the submission envelope.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": choice1 }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    let submission = first(&created, "poll_submissions");
    assert_eq!(submission["poll_choice_id"], choice1);
    assert_eq!(submission["user_id"], student1);
    assert_eq!(submission["poll_session_id"], session);
    assert!(submission["created_at"].is_string());
    assert!(created["meta"]["pagination"].is_object());
    let submission_id = submission["id"].as_i64().unwrap();

    // A second submission by the same user is rejected (spec §8 Q2 → reject,
    // upstream uniqueness validation) with the Rails error body.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": choice2 }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["user_id"][0],
        "can only submit one choice per poll session."
    );

    // The section-enrolled student can submit to the course-wide session.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s2.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": choice2 }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let s2_submission = resp.json::<Value>()["poll_submissions"][0]["id"]
        .as_i64()
        .unwrap();

    // Non-enrolled students are 403.
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s3.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": choice1 }] }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // A choice of another poll, a choice of another session's poll, an
    // unknown choice id and an unknown session are all 404s (guard chain
    // before authorization).
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": foreign_choice }] }))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": 999999999 }] }))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .json(&json!({ "poll_submissions": [{}] }))
        .await;
    assert_eq!(resp.status_code(), 404, "missing poll_choice_id → 404 (find(nil))");
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/999999999/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": choice1 }] }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Show: the submitter and the poll owner read it; other students get 403.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions/{submission_id}"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(first(&resp.json::<Value>(), "poll_submissions")["id"], submission_id);

    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions/{submission_id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "poll owner reads any submission");

    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions/{submission_id}"))
        .add_header(axum::http::header::AUTHORIZATION, s2.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "another student is 403");

    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // A submission of one session is not visible through another session's
    // path, and not through another poll's path.
    let resp = server
        .get(&format!("/polls/{other_poll}/poll_sessions/{session}/poll_submissions/{submission_id}"))
        .add_header(axum::http::header::AUTHORIZATION, s1.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // Closing the session blocks further submissions.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}/close"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s3.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": choice1 }] }))
        .await;
    assert_eq!(resp.status_code(), 403, "closed session is 403 for everyone");

    // The session JSON embeds results and submissions for the owner.
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{session}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    let session_json = first(&shown, "poll_sessions");
    assert_eq!(
        session_json["results"],
        json!({ choice1.to_string(): 1, choice2.to_string(): 1 }),
        "results aggregate per choice"
    );
    assert_eq!(session_json["poll_submissions"].as_array().unwrap().len(), 2);
    assert_eq!(session_json["has_submitted"], false, "owner has not submitted");

    // A section-bound session accepts the section's students.
    let section_session =
        create_session_ok(&server, &bearer, poll, json!({ "course_id": course, "course_section_id": section }))
            .await["poll_sessions"][0]["id"]
            .as_i64()
            .unwrap();
    let resp = server
        .get(&format!("/polls/{poll}/poll_sessions/{section_session}/open"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let resp = server
        .post(&format!("/polls/{poll}/poll_sessions/{section_session}/poll_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, s2.clone())
        .json(&json!({ "poll_submissions": [{ "poll_choice_id": choice1 }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "section student submits: {}", resp.text());

    // The poll JSON aggregates total_results across sessions.
    let resp = server
        .get(&format!("/polls/{poll}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    let poll_json = first(&resp.json::<Value>(), "polls");
    assert_eq!(
        poll_json["total_results"],
        json!({ choice1.to_string(): 2, choice2.to_string(): 1 }),
        "total_results counts across all sessions"
    );

    // Deleting the poll cascades the submissions away.
    let resp = server
        .delete(&format!("/polls/{poll}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204);
    let (remaining,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM poll_submissions WHERE poll_id = $1",
    )
    .bind(poll)
    .fetch_one(&pool)
    .await
    .expect("count submissions");
    assert_eq!(remaining, 0, "submissions must cascade with the poll");
    let _ = s2_submission;

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn anonymous_requests_are_401_on_every_route() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // All 21 routes (spec §2): anonymous callers are rejected by the auth
    // extractor before any handler logic runs.
    let gets = [
        "/polls",
        "/polls/1",
        "/polls/1/poll_choices",
        "/polls/1/poll_choices/2",
        "/polls/1/poll_sessions",
        "/polls/1/poll_sessions/2",
        "/polls/1/poll_sessions/2/open",
        "/polls/1/poll_sessions/2/close",
        "/poll_sessions/opened",
        "/poll_sessions/closed",
        "/polls/1/poll_sessions/2/poll_submissions/3",
    ];
    for path in gets {
        let resp = server.get(path).await;
        assert_eq!(resp.status_code(), 401, "GET {path} anonymous");
    }

    let posts = [
        "/polls",
        "/polls/1/poll_choices",
        "/polls/1/poll_sessions",
        "/polls/1/poll_sessions/2/poll_submissions",
    ];
    for path in posts {
        let resp = server.post(path).await;
        assert_eq!(resp.status_code(), 401, "POST {path} anonymous");
    }

    let puts = [
        "/polls/1",
        "/polls/1/poll_choices/2",
        "/polls/1/poll_sessions/2",
    ];
    for path in puts {
        let resp = server.put(path).await;
        assert_eq!(resp.status_code(), 401, "PUT {path} anonymous");
    }

    let deletes = [
        "/polls/1",
        "/polls/1/poll_choices/2",
        "/polls/1/poll_sessions/2",
    ];
    for path in deletes {
        let resp = server.delete(path).await;
        assert_eq!(resp.status_code(), 401, "DELETE {path} anonymous");
    }

    // Sanity: the seeded prefix is unused here, but the harness still owns
    // its rows (none created).
    assert!(prefix.starts_with("poll_"));
    let _ = pool;
    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn cross_user_polls_are_invisible() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "teacher").await;
    let intruder = seed_user(&pool, &prefix, "Intruder", "teacher").await;
    let owner_bearer = auth(owner, "teacher");
    let intruder_bearer = auth(intruder, "teacher");

    let poll = create_poll_ok(&server, &owner_bearer, "Private poll").await["polls"][0]["id"]
        .as_i64()
        .unwrap();
    let choice = create_choice_ok(&server, &owner_bearer, poll, json!({ "text": "A" })).await
        ["poll_choices"][0]["id"]
        .as_i64()
        .unwrap();
    let course = seed_course(&pool, &prefix, "Course").await;
    let session = create_session_ok(&server, &owner_bearer, poll, json!({ "course_id": course }))
        .await["poll_sessions"][0]["id"]
        .as_i64()
        .unwrap();

    // Poll-scoped routes: the intruder is out of scope everywhere (404, the
    // scoped-lookup semantics of spec §3).
    let cases: [(&str, String); 8] = [
        ("get", format!("/polls/{poll}")),
        ("put", format!("/polls/{poll}")),
        ("delete", format!("/polls/{poll}")),
        ("get", format!("/polls/{poll}/poll_choices")),
        ("post", format!("/polls/{poll}/poll_choices")),
        (
            "get",
            format!("/polls/{poll}/poll_choices/{choice}"),
        ),
        ("get", format!("/polls/{poll}/poll_sessions")),
        (
            "get",
            format!("/polls/{poll}/poll_sessions/{session}/open"),
        ),
    ];
    for (method, path) in cases {
        let resp = match method {
            "get" => server
                .get(&path)
                .add_header(axum::http::header::AUTHORIZATION, intruder_bearer.clone())
                .await,
            "put" => server
                .put(&path)
                .add_header(axum::http::header::AUTHORIZATION, intruder_bearer.clone())
                .json(&json!({ "polls": [{ "question": "hijack" }] }))
                .await,
            "post" => server
                .post(&path)
                .add_header(axum::http::header::AUTHORIZATION, intruder_bearer.clone())
                .json(&json!({ "poll_choices": [{ "text": "hijack" }] }))
                .await,
            _ => server
                .delete(&path)
                .add_header(axum::http::header::AUTHORIZATION, intruder_bearer.clone())
                .await,
        };
        assert_eq!(resp.status_code(), 404, "{method} {path} by non-owner");
    }

    // The intruder's own index lists only their own polls.
    let own = create_poll_ok(&server, &intruder_bearer, "Intruder poll").await;
    let own_id = own["polls"][0]["id"].as_i64().unwrap();
    let resp = server
        .get("/polls")
        .add_header(axum::http::header::AUTHORIZATION, intruder_bearer.clone())
        .await;
    let listed: Value = resp.json();
    let ids: Vec<i64> = listed["polls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![own_id], "index must be creator-scoped: {listed}");
    assert_eq!(listed["meta"]["pagination"]["total"], 1);

    cleanup(&pool, &prefix).await;
}
