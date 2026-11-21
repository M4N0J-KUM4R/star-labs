//! Integration tests for the Canvas web-conferences routes.
//!
//! Pinned behavior (Canvas `ConferencesController`,
//! `canvas-src/app/controllers/conferences_controller.rb`, verified against
//! `canvas-src/spec/apis/v1/conferences_api_spec.rb`):
//!
//! - `GET /courses/{course_id}/conferences` and the group twin need a
//!   session (401 without one), a missing context is a 404 and a context the
//!   caller may not read is a 403;
//! - the listing shows the conferences of the context the caller participates
//!   in — a caller with one of the granular `manage_course_content_*`
//!   permissions sees all of them;
//! - without a configured conference provider (`plugin_settings`) Canvas'
//!   `require_config` fails: the API answers 403;
//! - `POST /courses/{course_id}/conferences` creates the conference, invites
//!   the members of the context and answers the model JSON (`user_ids`,
//!   `context_code`, …); an unknown `conference_type` is a 400 and a missing
//!   `web_conference` parameter is a 400;
//! - `GET /conferences` lists the conferences of the current user across
//!   their courses and groups, `state=live` keeps the live ones;
//! - `POST /courses/{course_id}/conferences/{conference_id}/recording_ready`
//!   needs no session: a valid JWT (signed with the provider secret) whose
//!   `meeting_id` is the conference key answers 202 `[]` and flips
//!   `recording_ready`; a wrong meeting id is a 422 and a broken token a 401.
//!
//! Fleet rule: dedicated users, courses, groups and provider settings per
//! test, every seeded row is cleaned up. Skipped cleanly when
//! DATABASE_URL is unset.
//!
//! The `web_conferences` and `web_conference_participants` tables are created
//! by this test (the same DDL the star-db migration has) so the suite runs
//! before the migration lands; `CREATE TABLE IF NOT EXISTS` keeps it a no-op
//! afterwards.

use std::sync::atomic::{AtomicU32, Ordering};

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// The DDL of the two tables the conference routes need.
const CONFERENCE_DDL: [&str; 2] = [
    r#"
    CREATE TABLE IF NOT EXISTS web_conferences (
        id                               BIGSERIAL PRIMARY KEY,
        title                            varchar(255) NOT NULL,
        conference_type                  varchar(255) NOT NULL,
        conference_key                   varchar(255),
        context_id                       bigint NOT NULL,
        context_type                     varchar(255) NOT NULL,
        user_ids                         varchar(255),
        added_user_ids                   varchar(255),
        user_id                          bigint,
        started_at                       timestamp,
        description                      text,
        duration                         double precision,
        created_at                       timestamp NOT NULL,
        updated_at                       timestamp NOT NULL,
        uuid                             varchar(255),
        invited_user_ids                 varchar(255),
        ended_at                         timestamp,
        start_at                         timestamp,
        end_at                           timestamp,
        context_code                     varchar(255),
        type                             varchar(255),
        settings                         text,
        recording_ready                  boolean,
        root_account_id                  bigint NOT NULL
    )
    "#,
    r#"
    CREATE TABLE IF NOT EXISTS web_conference_participants (
        id                               BIGSERIAL PRIMARY KEY,
        user_id                          bigint,
        web_conference_id                bigint,
        participation_type               varchar(255),
        created_at                       timestamp NOT NULL,
        updated_at                       timestamp NOT NULL,
        root_account_id                  bigint NOT NULL
    )
    "#,
];

/// The provider secret of the seeded plugin.
const PROVIDER_SECRET: &str = "conference-secret";

/// Serializes the tests that read the provider configuration — the
/// `require_config` behaviour depends on the rows other tests write.
static PROVIDER_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn run_prefix() -> String {
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "conf-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
        n
    )
}

/// Canvas `String#classify` for a plugin id (`big_blue_button` ->
/// `BigBlueButton`) — the port derives the conference type from the plugin
/// id the same way.
fn classify(plugin_id: &str) -> String {
    plugin_id
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("")
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

    for statement in CONFERENCE_DDL {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("create the conference tables");
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server =
        TestServer::new(star_conferences::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, 1) RETURNING id",
    )
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

async fn seed_group(pool: &PgPool, prefix: &str, course_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO groups (course_id, name, workflow_state) \
         VALUES ($1, $2, 'available') RETURNING id",
    )
    .bind(course_id)
    .bind(format!("{prefix}-group"))
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

/// The provider plugin of a test (`big_blue_button` with a secret).
///
/// Every test uses its own plugin id so the tests do not fight over the
/// shared `plugin_settings` table; the conference type is derived from the
/// plugin id exactly like the port does it.
async fn seed_provider(pool: &PgPool, prefix: &str) -> (String, String) {
    let plugin_id = format!("{prefix}_big_blue_button");
    let conference_type = classify(&plugin_id);
    sqlx::query(
        "INSERT INTO plugin_settings (name, settings, created_at, updated_at, disabled) \
         VALUES ($1, $2, NOW(), NOW(), FALSE)",
    )
    .bind(plugin_id.as_str())
    .bind(json!({ "domain": "bbb.test", "secret_dec": PROVIDER_SECRET }).to_string())
    .execute(pool)
    .await
    .expect("seed provider");
    (plugin_id, conference_type)
}

/// The number of enabled conference providers.
async fn provider_count(pool: &PgPool) -> i64 {
    let (count,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM plugin_settings \
         WHERE disabled IS NOT TRUE AND settings IS NOT NULL",
    )
    .fetch_one(pool)
    .await
    .expect("count providers");
    count
}

/// A conference of a context.
async fn seed_conference(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    user_id: i64,
    title: &str,
    conference_type: &str,
    conference_key: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO web_conferences ( \
             title, conference_type, conference_key, context_id, context_type, user_id, \
             created_at, updated_at, root_account_id \
         ) VALUES ($1, $2, $3, $4, $5, $6, NOW(), NOW(), 1) RETURNING id",
    )
    .bind(title)
    .bind(conference_type)
    .bind(conference_key)
    .bind(context_id)
    .bind(context_type)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed conference")
}

/// A conference that started `minutes` ago (and optionally ends later).
async fn seed_timed_conference(
    pool: &PgPool,
    context_type: &str,
    context_id: i64,
    user_id: i64,
    title: &str,
    conference_type: &str,
    started_minutes_ago: i64,
    ends_in_minutes: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO web_conferences ( \
             title, conference_type, context_id, context_type, user_id, started_at, ended_at, \
             created_at, updated_at, root_account_id \
         ) VALUES ( \
             $1, $2, $3, $4, $5, NOW() - ($6::text || ' minutes')::interval, \
             CASE WHEN $7::text IS NULL THEN NULL \
                  ELSE NOW() + (($7::text || ' minutes')::interval) END, \
             NOW(), NOW(), 1 \
         ) RETURNING id",
    )
    .bind(title)
    .bind(conference_type)
    .bind(context_id)
    .bind(context_type)
    .bind(user_id)
    .bind(started_minutes_ago.to_string())
    .bind(ends_in_minutes.map(|minutes| minutes.to_string()))
    .fetch_one(pool)
    .await
    .expect("seed conference")
}

async fn seed_participant(
    pool: &PgPool,
    conference_id: i64,
    user_id: i64,
    participation_type: &str,
) {
    sqlx::query(
        "INSERT INTO web_conference_participants \
             (user_id, web_conference_id, participation_type, created_at, updated_at, \
              root_account_id) \
         VALUES ($1, $2, $3, NOW(), NOW(), 1)",
    )
    .bind(user_id)
    .bind(conference_id)
    .bind(participation_type)
    .execute(pool)
    .await
    .expect("seed participant");
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

fn sign(secret: &str, claims: Value) -> String {
    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("sign the parameters")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    sqlx::query(
        "DELETE FROM web_conference_participants WHERE web_conference_id IN ( \
             SELECT id FROM web_conferences WHERE title LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query("DELETE FROM web_conferences WHERE title LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM plugin_settings WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
    sqlx::query(
        "DELETE FROM group_memberships WHERE group_id IN (SELECT id FROM groups WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query("DELETE FROM groups WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
    sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await
    .ok();
    sqlx::query("DELETE FROM courses WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await
        .ok();
}

// ---------------------------------------------------------------------------
// index
// ---------------------------------------------------------------------------

#[tokio::test]
async fn index_requires_a_session() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let course_id = seed_course(&pool, &prefix, "course").await;

    let response = server.get(&format!("/courses/{course_id}/conferences")).await;
    assert_eq!(response.status_code(), 401);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn index_is_404_for_a_missing_context_and_403_for_a_non_reader() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "outsider", "student").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;

    let missing = server
        .get("/courses/987654321/conferences")
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;
    assert_eq!(missing.status_code(), 404);

    let forbidden = server
        .get(&format!("/courses/{course_id}/conferences"))
        .add_header("Authorization", auth(outsider, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn index_lists_the_conferences_of_the_participant() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    let (_, conference_type) = seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;
    seed_enrollment(&pool, student, course_id, "student").await;

    let mine = seed_conference(
        &pool,
        "Course",
        course_id,
        teacher,
        &format!("{prefix}-mine"),
        &conference_type,
        None,
    )
    .await;
    let other = seed_conference(
        &pool,
        "Course",
        course_id,
        teacher,
        &format!("{prefix}-other"),
        &conference_type,
        None,
    )
    .await;
    seed_participant(&pool, mine, student, "attendee").await;

    let response = server
        .get(&format!("/courses/{course_id}/conferences"))
        .add_header("Authorization", auth(student, "student"))
        .await;
    assert_eq!(response.status_code(), 200);
    let body: Value = response.json();
    let conferences = body["conferences"].as_array().expect("conferences");
    assert_eq!(conferences.len(), 1, "only the joined conference is listed");
    let conference = &conferences[0];
    assert_eq!(conference["id"], json!(mine));
    assert_eq!(conference["title"], json!(format!("{prefix}-mine")));
    assert_eq!(conference["conference_type"], json!(conference_type));
    assert_eq!(conference["context_type"], json!("Course"));
    assert_eq!(conference["context_id"], json!(course_id));
    assert_eq!(conference["users"], json!([student]));
    assert_eq!(conference["invitees"], json!([]));
    assert_eq!(conference["attendees"], json!([]));
    assert_eq!(conference["has_advanced_settings"], json!(false));
    assert_eq!(conference["long_running"], json!(false));
    assert_eq!(conference["join_url"], Value::Null);
    let url = conference["url"].as_str().expect("url");
    assert!(
        url.ends_with(&format!("/courses/{course_id}/conferences/{mine}")),
        "unexpected url: {url}"
    );

    // The teacher is not a participant of the second conference and holds no
    // granular content permission, so the listing stays empty for him.
    let teacher_body: Value = server
        .get(&format!("/courses/{course_id}/conferences"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(
        teacher_body["conferences"].as_array().expect("c").len(),
        0,
        "the teacher participates in no conference"
    );
    assert_ne!(other, mine);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn index_without_a_provider_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;

    let configured = provider_count(&pool).await > 0;
    let response = server
        .get(&format!("/courses/{course_id}/conferences"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .await;

    if configured {
        // Another suite configured a provider — the route stays reachable.
        assert_eq!(response.status_code(), 200);
    } else {
        // Canvas `require_config` fails without a configured provider.
        assert_eq!(response.status_code(), 403);
        let body: Value = response.json();
        assert_eq!(
            body["errors"][0]["message"],
            json!("Web conferencing has not been enabled for this Canvas site")
        );
    }

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_makes_a_conference_and_invites_the_context() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    let (_, conference_type) = seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;
    seed_enrollment(&pool, student, course_id, "student").await;

    let response = server
        .post(&format!("/courses/{course_id}/conferences"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .json(&json!({
            "web_conference": {
                "title": format!("{prefix}-created"),
                "conference_type": conference_type,
                "duration": 60,
                "description": "a conference"
            }
        }))
        .await;
    assert_eq!(response.status_code(), 200);
    let body: Value = response.json();
    assert_eq!(body["title"], json!(format!("{prefix}-created")));
    assert_eq!(body["conference_type"], json!(conference_type));
    assert_eq!(body["context_type"], json!("Course"));
    assert_eq!(body["context_id"], json!(course_id));
    assert_eq!(body["context_code"], json!(format!("course_{course_id}")));
    assert_eq!(body["duration"], json!(60.0));
    assert_eq!(body["join_url"], Value::Null);

    // The teacher and the student are both members of the course, so both
    // are invited.
    let users = body["user_ids"].as_array().expect("user ids");
    assert_eq!(users.len(), 2);
    assert!(users.contains(&json!(teacher)));
    assert!(users.contains(&json!(student)));

    let conference_id = body["id"].as_i64().expect("id");
    let participants: Vec<(i64, String)> = sqlx::query_as(
        "SELECT user_id, participation_type FROM web_conference_participants \
         WHERE web_conference_id = $1 ORDER BY user_id",
    )
    .bind(conference_id)
    .fetch_all(&pool)
    .await
    .expect("participants");
    assert_eq!(participants.len(), 2);
    assert!(participants
        .iter()
        .all(|(_, participation_type)| participation_type == "invitee"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_rejects_an_unknown_conference_type_and_a_missing_parameter() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;

    let unknown_type = server
        .post(&format!("/courses/{course_id}/conferences"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .json(&json!({
            "web_conference": { "title": format!("{prefix}-unknown"), "conference_type": "Zoom" }
        }))
        .await;
    assert_eq!(unknown_type.status_code(), 400);
    let body: Value = unknown_type.json();
    assert_eq!(body["conference_type"], json!(["can't be blank"]));

    let missing_parameter = server
        .post(&format!("/courses/{course_id}/conferences"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .json(&json!({}))
        .await;
    assert_eq!(missing_parameter.status_code(), 400);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_in_a_group_uses_the_group_context() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    let (_, conference_type) = seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;
    let group_id = seed_group(&pool, &prefix, course_id).await;
    seed_group_membership(&pool, group_id, teacher).await;

    let response = server
        .post(&format!("/groups/{group_id}/conferences"))
        .add_header("Authorization", auth(teacher, "teacher"))
        .json(&json!({
            "web_conference": {
                "title": format!("{prefix}-group"),
                "conference_type": conference_type
            }
        }))
        .await;
    assert_eq!(response.status_code(), 200);
    let body: Value = response.json();
    assert_eq!(body["context_type"], json!("Group"));
    assert_eq!(body["context_id"], json!(group_id));
    assert_eq!(body["context_code"], json!(format!("group_{group_id}")));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// for_user
// ---------------------------------------------------------------------------

#[tokio::test]
async fn for_user_lists_the_conferences_of_the_caller() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    let (_, conference_type) = seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let other = seed_user(&pool, &prefix, "other", "student").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;
    seed_enrollment(&pool, student, course_id, "student").await;

    let conference_id = seed_conference(
        &pool,
        "Course",
        course_id,
        teacher,
        &format!("{prefix}-for-user"),
        &conference_type,
        None,
    )
    .await;
    seed_participant(&pool, conference_id, student, "attendee").await;

    let body: Value = server
        .get("/conferences")
        .add_header("Authorization", auth(student, "student"))
        .await
        .json();
    let ids: Vec<i64> = body["conferences"]
        .as_array()
        .expect("conferences")
        .iter()
        .filter_map(|conference| conference["id"].as_i64())
        .collect();
    assert_eq!(ids, vec![conference_id]);

    // A user without a participation sees nothing.
    let other_body: Value = server
        .get("/conferences")
        .add_header("Authorization", auth(other, "student"))
        .await
        .json();
    assert!(other_body["conferences"].as_array().expect("c").is_empty());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn for_user_filters_the_live_conferences() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    let (_, conference_type) = seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;
    seed_enrollment(&pool, student, course_id, "student").await;

    let live = seed_timed_conference(
        &pool,
        "Course",
        course_id,
        teacher,
        &format!("{prefix}-live"),
        &conference_type,
        30,
        Some(30),
    )
    .await;
    let zombie = seed_timed_conference(
        &pool,
        "Course",
        course_id,
        teacher,
        &format!("{prefix}-zombie"),
        &conference_type,
        60 * 24 * 2,
        None,
    )
    .await;
    let future = seed_timed_conference(
        &pool,
        "Course",
        course_id,
        teacher,
        &format!("{prefix}-future"),
        &conference_type,
        -60,
        None,
    )
    .await;
    seed_participant(&pool, live, student, "attendee").await;
    seed_participant(&pool, zombie, student, "attendee").await;
    seed_participant(&pool, future, student, "attendee").await;

    let body: Value = server
        .get("/conferences?state=live")
        .add_header("Authorization", auth(student, "student"))
        .await
        .json();
    let ids: Vec<i64> = body["conferences"]
        .as_array()
        .expect("conferences")
        .iter()
        .filter_map(|conference| conference["id"].as_i64())
        .collect();
    // Only the conference that started half an hour ago is live.
    assert_eq!(ids, vec![live]);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// recording_ready
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recording_ready_flips_the_flag() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    let (_, conference_type) = seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;
    let conference_id = seed_conference(
        &pool,
        "Course",
        course_id,
        teacher,
        &format!("{prefix}-recording"),
        &conference_type,
        Some("conf_key"),
    )
    .await;

    // No session at all — the provider authenticates itself.
    let token = sign(PROVIDER_SECRET, json!({ "meeting_id": "conf_key" }));
    let response = server
        .post(&format!(
            "/courses/{course_id}/conferences/{conference_id}/recording_ready"
        ))
        .json(&json!({ "signed_parameters": token }))
        .await;
    assert_eq!(response.status_code(), 202);
    assert_eq!(response.text(), "[]");

    let (ready,): (Option<bool>,) =
        sqlx::query_as("SELECT recording_ready FROM web_conferences WHERE id = $1")
            .bind(conference_id)
            .fetch_one(&pool)
            .await
            .expect("read the flag");
    assert_eq!(ready, Some(true));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn recording_ready_rejects_a_wrong_meeting_and_a_broken_token() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    let (_, conference_type) = seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;
    let conference_id = seed_conference(
        &pool,
        "Course",
        course_id,
        teacher,
        &format!("{prefix}-recording"),
        &conference_type,
        Some("conf_key"),
    )
    .await;

    let wrong_meeting = server
        .post(&format!(
            "/courses/{course_id}/conferences/{conference_id}/recording_ready"
        ))
        .json(&json!({
            "signed_parameters": sign(PROVIDER_SECRET, json!({ "meeting_id": "other_key" }))
        }))
        .await;
    assert_eq!(wrong_meeting.status_code(), 422);
    let body: Value = wrong_meeting.json();
    assert_eq!(body["status"], json!("unprocessable entity"));
    assert_eq!(body["errors"][0]["message"], json!("Signed meeting id invalid"));

    let broken_token = server
        .post(&format!(
            "/courses/{course_id}/conferences/{conference_id}/recording_ready"
        ))
        .json(&json!({ "signed_parameters": "not-a-jwt" }))
        .await;
    assert_eq!(broken_token.status_code(), 401);
    let body: Value = broken_token.json();
    assert_eq!(body["status"], json!("unauthorized"));
    assert_eq!(body["errors"][0]["message"], json!("JWT signature invalid"));

    // A token signed with the wrong secret is invalid, too.
    let wrong_secret = server
        .post(&format!(
            "/courses/{course_id}/conferences/{conference_id}/recording_ready"
        ))
        .json(&json!({
            "signed_parameters": sign("other-secret", json!({ "meeting_id": "conf_key" }))
        }))
        .await;
    assert_eq!(wrong_secret.status_code(), 401);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn recording_ready_is_404_for_a_conference_of_another_context() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let _guard = PROVIDER_LOCK.lock().await;
    let (_, conference_type) = seed_provider(&pool, &prefix).await;
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let course_id = seed_course(&pool, &prefix, "course").await;
    let other_course_id = seed_course(&pool, &prefix, "other-course").await;
    seed_enrollment(&pool, teacher, course_id, "teacher").await;
    let conference_id = seed_conference(
        &pool,
        "Course",
        other_course_id,
        teacher,
        &format!("{prefix}-elsewhere"),
        &conference_type,
        Some("conf_key"),
    )
    .await;

    let response = server
        .post(&format!(
            "/courses/{course_id}/conferences/{conference_id}/recording_ready"
        ))
        .json(&json!({ "signed_parameters": "not-a-jwt" }))
        .await;
    assert_eq!(response.status_code(), 404);

    cleanup(&pool, &prefix).await;
}
