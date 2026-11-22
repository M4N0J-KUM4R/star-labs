//! Integration tests for star-conferences (issue #91, spec
//! `specs/p5-rest-conferences.md` §7).
//!
//! Pinned behavior:
//! - index: manager sees all, participant-only member sees just theirs,
//!   non-member 403, unknown context 404, anonymous 401; `recordings`
//!   [] until the webhook fires.
//! - create: 200 with the `conferences` array body; invitees →
//!   web_conference_participants rows + initiator row; `user` map /
//!   `user[all]` / `observers[remove]` invite semantics; truthy
//!   `web_conference[calendar_event]` + start_at → linked
//!   calendar_events row (courses only); unknown conference_type → 400.
//! - for_user: participant rows across contexts; `state=live` filter.
//! - recording_ready: valid JWT (meeting_id == conference_key) → 202 `[]`
//!   and recording_ready + recordings entry; wrong meeting → 422 pinned
//!   body; garbage token → 401 pinned body; NO Authorization header.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix
//! plus full teardown keeps shared-DB runs safe.

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
        "conf_{}_{}",
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

    let server =
        TestServer::new(star_conferences::router().with_state(state)).expect("build TestServer");
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
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

async fn seed_group(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed group")
}

async fn seed_group_member(pool: &PgPool, group_id: i64, user_id: i64, moderator: bool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO group_memberships (group_id, user_id, moderator) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(group_id)
    .bind(user_id)
    .bind(moderator)
    .fetch_one(pool)
    .await
    .expect("seed group membership")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Sign an HS256 provider JWT with the test secret (the webhook payload).
fn provider_jwt(claims: &Value) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        claims,
        &jsonwebtoken::EncodingKey::from_secret(JWT_SECRET.as_bytes()),
    )
    .expect("sign jwt")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // web_conferences cascade is NOT declared on the snapshot tables, so
    // delete participants explicitly; calendar_events reference
    // conferences via ON DELETE SET NULL (0229).
    let _ = sqlx::query(
        "DELETE FROM web_conference_participants WHERE web_conference_id IN \
         (SELECT id FROM web_conferences WHERE user_id IN \
          (SELECT id FROM users WHERE email LIKE $1))",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "UPDATE calendar_events SET web_conference_id = NULL \
         WHERE web_conference_id IN (SELECT id FROM web_conferences WHERE user_id IN \
          (SELECT id FROM users WHERE email LIKE $1))",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM calendar_events WHERE course_id IN \
         (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM web_conferences WHERE user_id IN \
         (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM groups WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
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
// Helpers
// ---------------------------------------------------------------------------

async fn create_ok(server: &TestServer, bearer: &str, path: &str, body: Value) -> Value {
    let resp = server
        .post(path)
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&body)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    resp.json()
}

fn first<'a>(body: &'a Value) -> &'a Value {
    let items = body["conferences"]
        .as_array()
        .unwrap_or_else(|| panic!("missing conferences array: {body}"));
    assert_eq!(items.len(), 1, "expected one item: {body}");
    &items[0]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_index_create_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let t = auth(teacher, "teacher");
    let s = auth(student, "student");
    let o = auth(outsider, "student");

    // Anonymous: 401.
    let resp = server.get(&format!("/courses/{course}/conferences")).await;
    assert_eq!(resp.status_code(), 401);
    let resp = server
        .post(&format!("/courses/{course}/conferences"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 401);

    // Unknown course: 404.
    let resp = server
        .get("/courses/999999999/conferences")
        .add_header(axum::http::header::AUTHORIZATION, t.clone())
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .post("/courses/999999999/conferences")
        .add_header(axum::http::header::AUTHORIZATION, t.clone())
        .json(&json!({ "web_conference": {} }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Non-member: 403 on index and create.
    let resp = server
        .get(&format!("/courses/{course}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, o.clone())
        .await;
    assert_eq!(resp.status_code(), 403);
    let resp = server
        .post(&format!("/courses/{course}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, o.clone())
        .json(&json!({ "web_conference": {} }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Student create: 403 (no :create ability).
    let resp = server
        .post(&format!("/courses/{course}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, s.clone())
        .json(&json!({ "web_conference": {} }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Unknown conference_type: 400 Rails body.
    let resp = server
        .post(&format!("/courses/{course}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, t.clone())
        .json(&json!({ "web_conference": { "conference_type": "Zoom",
            "title": "X" } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["conference_type"][0],
        "is not included in the list"
    );

    // Over-long title: 400.
    let resp = server
        .post(&format!("/courses/{course}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, t.clone())
        .json(&json!({ "web_conference": { "title": "x".repeat(256) } }))
        .await;
    assert_eq!(resp.status_code(), 400);
    assert_eq!(
        resp.json::<Value>()["title"][0],
        "is too long (maximum is 255 characters)"
    );

    // Create with invitees + calendar_event.
    let created = create_ok(
        &server,
        &t,
        &format!("/courses/{course}/conferences"),
        json!({
            "web_conference": {
                "title": "Lecture hall",
                "description": "Weekly sync",
                "conference_type": "BigBlueButton",
                "duration": 60,
                "start_at": "2026-11-22T10:00:00Z",
                "end_at": "2026-11-22T11:00:00Z",
                "calendar_event": true,
                "user_settings": { "record": true }
            },
            "user": { student.to_string(): "1" }
        }),
    )
    .await;
    let conf = first(&created);
    let conf_id = conf["id"].as_i64().unwrap();
    assert_eq!(conf["title"], "Lecture hall");
    assert_eq!(conf["conference_type"], "BigBlueButton");
    assert_eq!(conf["context_type"], "Course");
    assert_eq!(conf["context_id"], course);
    assert_eq!(conf["duration"], 60);
    assert_eq!(conf["long_running"], false);
    assert_eq!(conf["has_advanced_settings"], false);
    assert_eq!(conf["user_settings"], json!({ "record": true }));
    assert_eq!(conf["recordings"], json!([]));
    assert_eq!(conf["started_at"], Value::Null);
    assert_eq!(conf["users"], json!([student, teacher]));
    assert_eq!(conf["invitees"], json!([student]));

    // Participants rows: invitee + initiator.
    let (invitee_cnt,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM web_conference_participants \
         WHERE web_conference_id = $1 AND participation_type = 'invitee'",
    )
    .bind(conf_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(invitee_cnt, 1);
    let (initiator_cnt,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM web_conference_participants \
         WHERE web_conference_id = $1 AND participation_type = 'initiator'",
    )
    .bind(conf_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(initiator_cnt, 1);

    // Linked calendar event row.
    let (cal_cnt,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM calendar_events WHERE web_conference_id = $1")
            .bind(conf_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cal_cnt, 1, "calendar_event param must link an event");

    // Blank title defaults (Course Web Conference) and no calendar event
    // without the param / start_at.
    let created = create_ok(
        &server,
        &t,
        &format!("/courses/{course}/conferences"),
        json!({ "web_conference": {} }),
    )
    .await;
    let conf2 = first(&created);
    assert_eq!(conf2["title"], "Course Web Conference");
    let conf2_id = conf2["id"].as_i64().unwrap();
    let (cal_cnt,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM calendar_events WHERE web_conference_id = $1")
            .bind(conf2_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cal_cnt, 0, "no calendar_event param → no event");

    // user[all] invites the whole context (teacher + student).
    let created = create_ok(
        &server,
        &t,
        &format!("/courses/{course}/conferences"),
        json!({ "web_conference": { "title": "All hands" }, "user": { "all": "1" } }),
    )
    .await;
    let conf3 = first(&created);
    let mut invitees: Vec<i64> = conf3["invitees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_i64().unwrap())
        .collect();
    invitees.sort_unstable();
    let mut expected = vec![student, teacher];
    expected.sort_unstable();
    assert_eq!(invitees, expected);

    // Index: teacher (manager) sees all three; student (participant of
    // two) sees only their own; each with recordings [].
    let resp = server
        .get(&format!("/courses/{course}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, t.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    assert_eq!(listed["conferences"].as_array().unwrap().len(), 3);

    let resp = server
        .get(&format!("/courses/{course}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, s.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    let items = listed["conferences"].as_array().unwrap();
    assert_eq!(items.len(), 2, "student sees only participated ones");
    assert!(items.iter().all(|c| c["recordings"] == json!([])));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_twin_index_create() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let leader = seed_user(&pool, &prefix, "Leader", "student").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let group = seed_group(&pool, &prefix, "Group").await;
    seed_group_member(&pool, group, leader, true).await;
    seed_group_member(&pool, group, member, false).await;
    let l = auth(leader, "student");
    let m = auth(member, "student");
    let o = auth(outsider, "student");

    // Anonymous and unknown group.
    let resp = server.get(&format!("/groups/{group}/conferences")).await;
    assert_eq!(resp.status_code(), 401);
    let resp = server
        .get("/groups/999999999/conferences")
        .add_header(axum::http::header::AUTHORIZATION, l.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // Non-member: 403.
    let resp = server
        .get(&format!("/groups/{group}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, o.clone())
        .await;
    assert_eq!(resp.status_code(), 403);

    // Non-moderator member create: 403.
    let resp = server
        .post(&format!("/groups/{group}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, m.clone())
        .json(&json!({ "web_conference": {} }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Moderator create: 200, Group context, default group title, no
    // calendar event even with the param (groups never get one).
    let created = create_ok(
        &server,
        &l,
        &format!("/groups/{group}/conferences"),
        json!({
            "web_conference": {
                "title": "Study circle",
                "start_at": "2026-11-22T18:00:00Z",
                "calendar_event": true
            }
        }),
    )
    .await;
    let conf = first(&created);
    let conf_id = conf["id"].as_i64().unwrap();
    assert_eq!(conf["context_type"], "Group");
    assert_eq!(conf["context_id"], group);
    assert_eq!(conf["title"], "Study circle");
    let (cal_cnt,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM calendar_events WHERE web_conference_id = $1")
            .bind(conf_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cal_cnt, 0, "group conferences never create calendar events");

    // Index: member (participant) sees it.
    let resp = server
        .get(&format!("/groups/{group}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, m.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    assert_eq!(listed["conferences"].as_array().unwrap().len(), 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn for_user_lists_and_live_filter() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let t = auth(teacher, "teacher");
    let s = auth(student, "student");

    // Anonymous: 401.
    let resp = server.get("/conferences").await;
    assert_eq!(resp.status_code(), 401);

    let invited = create_ok(
        &server,
        &t,
        &format!("/courses/{course}/conferences"),
        json!({
            "web_conference": { "title": "Invited", "duration": 30 },
            "user": { student.to_string(): "1" }
        }),
    )
    .await;
    let invited_id = first(&invited)["id"].as_i64().unwrap();

    let solo = create_ok(
        &server,
        &t,
        &format!("/courses/{course}/conferences"),
        json!({ "web_conference": { "title": "Solo", "duration": 30 } }),
    )
    .await;
    let solo_id = first(&solo)["id"].as_i64().unwrap();

    // for_user: teacher sees both; student only the invited one; the
    // unrelated teacher sees none.
    let resp = server
        .get("/conferences")
        .add_header(axum::http::header::AUTHORIZATION, t.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    assert_eq!(listed["conferences"].as_array().unwrap().len(), 2);

    let resp = server
        .get("/conferences")
        .add_header(axum::http::header::AUTHORIZATION, s.clone())
        .await;
    let listed: Value = resp.json();
    let ids: Vec<i64> = listed["conferences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![invited_id]);

    let resp = server
        .get("/conferences")
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "teacher"))
        .await;
    let listed: Value = resp.json();
    assert_eq!(listed["conferences"], json!([]));

    // state=live: mark the invited one live (started 1h ago, not ended).
    sqlx::query(
        "UPDATE web_conferences SET started_at = NOW() - INTERVAL '1 hour' \
         WHERE id = $1",
    )
    .bind(invited_id)
    .execute(&pool)
    .await
    .unwrap();
    // And the solo one finished (started + ended).
    sqlx::query(
        "UPDATE web_conferences SET started_at = NOW() - INTERVAL '2 hours', \
         ended_at = NOW() - INTERVAL '1 hour' WHERE id = $1",
    )
    .bind(solo_id)
    .execute(&pool)
    .await
    .unwrap();

    let resp = server
        .get("/conferences?state=live")
        .add_header(axum::http::header::AUTHORIZATION, t.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    let ids: Vec<i64> = listed["conferences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![invited_id], "state=live keeps only live ones");

    // Un-started conferences are not live either.
    let resp = server
        .get("/conferences?state=live")
        .add_header(axum::http::header::AUTHORIZATION, s.clone())
        .await;
    let listed: Value = resp.json();
    assert_eq!(listed["conferences"], json!([]));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn recording_ready_webhook_jwt() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let t = auth(teacher, "teacher");

    let created = create_ok(
        &server,
        &t,
        &format!("/courses/{course}/conferences"),
        json!({ "web_conference": { "title": "Recorded", "duration": 30 } }),
    )
    .await;
    let conf_id = first(&created)["id"].as_i64().unwrap();
    // The provider meeting id is the conference_key (assigned on join in
    // Canvas; star-lms assigns it up front so the webhook is testable).
    sqlx::query("UPDATE web_conferences SET conference_key = $1 WHERE id = $2")
        .bind(format!("meeting-{conf_id}"))
        .bind(conf_id)
        .execute(&pool)
        .await
        .unwrap();
    let key = format!("meeting-{conf_id}");

    let course_path = format!("/courses/{course}/conferences/{conf_id}/recording_ready");

    // Unknown context / conference: 404 BEFORE JWT verification.
    let resp = server
        .post(&format!(
            "/courses/999999999/conferences/{conf_id}/recording_ready"
        ))
        .json(&json!({ "signed_parameters": "x" }))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .post(&format!(
            "/courses/{course}/conferences/999999999/recording_ready"
        ))
        .json(&json!({ "signed_parameters": "x" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Invalid JWT: 401 with the pinned Canvas body; no auth header used.
    let resp = server
        .post(&course_path)
        .json(&json!({ "signed_parameters": "garbage.token.here" }))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "unauthorized");
    assert_eq!(body["errors"][0]["message"], "JWT signature invalid");

    // Wrong-secret JWT is also invalid.
    let wrong = jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &json!({ "meeting_id": key }),
        &jsonwebtoken::EncodingKey::from_secret(b"other-secret"),
    )
    .unwrap();
    let resp = server
        .post(&course_path)
        .json(&json!({ "signed_parameters": wrong }))
        .await;
    assert_eq!(resp.status_code(), 401);

    // Valid JWT, wrong meeting_id: 422 pinned body.
    let token = provider_jwt(&json!({ "meeting_id": "other-meeting" }));
    let resp = server
        .post(&course_path)
        .json(&json!({ "signed_parameters": token }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "unprocessable entity");
    assert_eq!(body["errors"][0]["message"], "Signed meeting id invalid");

    // Valid JWT, matching meeting_id: 202 [] + flag + recordings entry.
    let token = provider_jwt(&json!({ "meeting_id": key }));
    let resp = server
        .post(&course_path)
        .json(&json!({ "signed_parameters": token }))
        .await;
    assert_eq!(resp.status_code(), 202, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!([]));

    let (ready,): (bool,) = sqlx::query_as(
        "SELECT COALESCE(recording_ready, false) FROM web_conferences WHERE id = $1",
    )
    .bind(conf_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(ready, "recording_ready flag must be set");

    // The recordings array is now served by index.
    let resp = server
        .get(&format!("/courses/{course}/conferences"))
        .add_header(axum::http::header::AUTHORIZATION, t.clone())
        .await;
    let listed: Value = resp.json();
    let recordings = &listed["conferences"][0]["recordings"];
    assert_eq!(recordings.as_array().unwrap().len(), 1);
    assert_eq!(recordings[0]["title"], "Recorded");
    assert!(recordings[0]["playback_url"].is_null());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_recording_ready_twin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let leader = seed_user(&pool, &prefix, "Leader", "student").await;
    let group = seed_group(&pool, &prefix, "Group").await;
    seed_group_member(&pool, group, leader, true).await;
    let l = auth(leader, "student");

    let created = create_ok(
        &server,
        &l,
        &format!("/groups/{group}/conferences"),
        json!({ "web_conference": { "title": "Group conf", "duration": 15 } }),
    )
    .await;
    let conf_id = first(&created)["id"].as_i64().unwrap();
    sqlx::query("UPDATE web_conferences SET conference_key = $1 WHERE id = $2")
        .bind(format!("grp-{conf_id}"))
        .bind(conf_id)
        .execute(&pool)
        .await
        .unwrap();

    let path = format!("/groups/{group}/conferences/{conf_id}/recording_ready");

    // Invalid JWT → 401; wrong meeting → 422.
    let resp = server
        .post(&path)
        .json(&json!({ "signed_parameters": "bad" }))
        .await;
    assert_eq!(resp.status_code(), 401);
    let token = provider_jwt(&json!({ "meeting_id": "nope" }));
    let resp = server
        .post(&path)
        .json(&json!({ "signed_parameters": token }))
        .await;
    assert_eq!(resp.status_code(), 422);

    // Valid → 202 + flag.
    let token = provider_jwt(&json!({ "meeting_id": format!("grp-{conf_id}") }));
    let resp = server
        .post(&path)
        .json(&json!({ "signed_parameters": token }))
        .await;
    assert_eq!(resp.status_code(), 202, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!([]));
    let (ready,): (bool,) = sqlx::query_as(
        "SELECT COALESCE(recording_ready, false) FROM web_conferences WHERE id = $1",
    )
    .bind(conf_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(ready);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn anonymous_401_on_all_auth_routes() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // Routes 1-5 (index/create/for_user) all require a user. The
    // recording_ready webhooks (6-7) deliberately do NOT.
    for path in [
        "/courses/1/conferences",
        "/groups/1/conferences",
        "/conferences",
    ] {
        let resp = server.get(path).await;
        assert_eq!(resp.status_code(), 401, "GET {path} anonymous");
    }
    for path in ["/courses/1/conferences", "/groups/1/conferences"] {
        let resp = server.post(path).json(&json!({})).await;
        assert_eq!(resp.status_code(), 401, "POST {path} anonymous");
    }

    assert!(prefix.starts_with("conf_"));
    let _ = pool;
    cleanup(&pool, &prefix).await;
}
