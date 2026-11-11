//! Integration tests for the deep discussion endpoints: per-topic summaries
//! and the discussion-type management routes.
//!
//! Pinned behavior (Canvas API v1 parity):
//! - `GET .../summaries` answers the caller's last summary of the topic with
//!   Canvas' shape `{id, text, userInput, obsolete, usage:{currentCount, limit}}`.
//!   Without a generated summary it is a 404 with `{"error": "No summary found."}`.
//! - `POST .../summaries` generates (or reuses) a summary and answers
//!   `{id, text, usage:{currentCount, limit}}`. Canvas delegates the text to an
//!   LLM service; this implementation derives it locally from the entries.
//! - Both endpoints require course read access (403 otherwise), an existing
//!   topic of that course (404 otherwise) and the right to summarize, which in
//!   Canvas means course instructor or admin (403 for enrolled students).
//! - The group variants answer 403: Canvas' `user_can_summarize?` is false for
//!   group topics (a group topic has no course). A caller without group read
//!   access is a 403 and an unknown topic is a 404.
//! - `PUT .../update_discussion_types` needs the two arrays of topic ids and
//!   has to cover every side-comment topic of the course exactly once; it
//!   answers `{"success": "true"}` and 400 `{"error": "Missing data"}` /
//!   `{"error": "Invalid data", "count": N}` otherwise.
//! - `PUT .../migrate_disallow` turns every side-comment topic of the course
//!   into a threaded one and answers `{"update_count": N}`.
//!
//! Known deviations from Canvas: the site-admin feature-flag gate that Canvas
//! applies to the two discussion-type routes (404 when disabled) is not
//! reproduced because the workspace has no feature-flag store, and the summary
//! text is produced locally instead of by an LLM.
//!
//! Skipped cleanly when DATABASE_URL is not set; uses a unique per-run prefix
//! plus full teardown so repeat runs against a shared DB are safe.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "dscd_{}_{}",
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
        TestServer::new(star_discussions::router().with_state(state)).expect("build TestServer");
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
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO courses (name, course_code)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(course_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed enrollment");
}

async fn seed_topic(
    pool: &PgPool,
    prefix: &str,
    course_id: i64,
    title: &str,
    discussion_type: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO discussion_topics (course_id, title, discussion_type, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(format!("{prefix}-{title}"))
    .bind(discussion_type)
    .fetch_one(pool)
    .await
    .expect("seed topic")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Delete every row created by this run, in FK-safe order.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM discussion_topic_summaries WHERE discussion_topic_id IN
         (SELECT id FROM discussion_topics WHERE title LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM discussion_topics WHERE title LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM group_memberships WHERE group_id IN
         (SELECT id FROM groups WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM groups WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET/POST .../summaries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_summary_is_generated_reused_and_flagged_obsolete() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "SummaryCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let teacher_bearer = auth(teacher, "teacher");

    let topic_id = seed_topic(&pool, &prefix, course, "SummaryTopic", "side_comment").await;
    let url = format!("/courses/{course}/discussion_topics/{topic_id}/summaries");

    // No summary yet: Canvas answers 404 with its JSON error body.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["error"], "No summary found.");

    // POST generates the summary and answers Canvas' create shape.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "userInput": "Give me a brief summary of the discussion." }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    let summary_id = created["id"].as_i64().expect("summary id");
    assert!(created["text"].as_str().unwrap().contains("SummaryTopic"));
    assert_eq!(created["usage"]["currentCount"], 1);
    assert_eq!(created["usage"]["limit"], 25);
    assert!(
        created.get("userInput").is_none(),
        "the create response has no userInput: {created}"
    );

    // The topic is flagged as summarized, like Canvas does.
    let enabled: bool =
        sqlx::query_scalar("SELECT summary_enabled FROM discussion_topics WHERE id = $1")
            .bind(topic_id)
            .fetch_one(&pool)
            .await
            .expect("topic row");
    assert!(enabled);

    // GET returns the same summary with Canvas' find shape.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let last: Value = resp.json();
    assert_eq!(last["id"], summary_id);
    assert_eq!(
        last["userInput"],
        "Give me a brief summary of the discussion."
    );
    assert_eq!(last["obsolete"], false);
    assert_eq!(last["usage"]["currentCount"], 1);

    // A new entry changes the discussion content, so the summary is obsolete.
    sqlx::query(
        "INSERT INTO discussion_entries (topic_id, user_id, message, workflow_state)
         VALUES ($1, $2, 'a late reply', 'active')",
    )
    .bind(topic_id)
    .bind(teacher)
    .execute(&pool)
    .await
    .expect("seed entry");

    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let last: Value = resp.json();
    assert_eq!(last["id"], summary_id);
    assert_eq!(last["obsolete"], true);

    // A repeated POST with the same focus now regenerates the summary because
    // the discussion content changed.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "userInput": "Give me a brief summary of the discussion." }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let regenerated: Value = resp.json();
    assert_ne!(
        regenerated["id"], summary_id,
        "changed discussion content regenerates the summary"
    );
    assert_eq!(regenerated["usage"]["currentCount"], 2);

    // Repeating the POST again, with unchanged content, reuses the summary.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "userInput": "Give me a brief summary of the discussion." }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let reused: Value = resp.json();
    assert_eq!(
        reused["id"], regenerated["id"],
        "an unchanged discussion reuses the summary"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_summaries_require_read_access_topic_and_summarize_right() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "SummaryAuthz").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let teacher_bearer = auth(teacher, "teacher");
    let student_bearer = auth(student, "student");

    let topic_id = seed_topic(&pool, &prefix, course, "SummaryAuthzTopic", "side_comment").await;
    let url = format!("/courses/{course}/discussion_topics/{topic_id}/summaries");

    // A caller without course read access is a 403, before the topic lookup.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(resp.text(), "forbidden");

    // An enrolled student may read the course but not summarize it.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer)
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A topic of another course is a 404.
    let other_course = seed_course(&pool, &prefix, "SummaryAuthzOther").await;
    seed_enrollment(&pool, teacher, other_course, "teacher").await;
    let resp = server
        .get(&format!(
            "/courses/{other_course}/discussion_topics/{topic_id}/summaries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.text(), "not found");

    // A missing topic is a 404 for the teacher as well.
    let resp = server
        .get(&format!(
            "/courses/{course}/discussion_topics/999999999/summaries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_summaries_are_forbidden_after_context_and_topic_checks() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let member = seed_user(&pool, &prefix, "Member", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "SummaryGroup").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, member, course, "student").await;

    let group_id: i64 =
        sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
            .bind(course)
            .bind(format!("{prefix}-SummaryGroup"))
            .fetch_one(&pool)
            .await
            .expect("seed group");

    sqlx::query(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state)
         VALUES ($1, $2, 'accepted')",
    )
    .bind(group_id)
    .bind(member)
    .execute(&pool)
    .await
    .expect("seed membership");

    let topic_id: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, group_id, title, discussion_type, workflow_state)
         VALUES ($1, $2, $3, 'side_comment', 'active') RETURNING id",
    )
    .bind(course)
    .bind(group_id)
    .bind(format!("{prefix}-SummaryGroupTopic"))
    .fetch_one(&pool)
    .await
    .expect("seed group topic");

    let url = format!("/groups/{group_id}/discussion_topics/{topic_id}/summaries");

    // Canvas does not allow summaries in group contexts: 403 for a member.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A caller without group read access is a 403 as well.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An unknown topic of the group is a 404.
    let resp = server
        .get(&format!(
            "/groups/{group_id}/discussion_topics/999999999/summaries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A course topic is not reachable through the group path.
    let course_topic = seed_topic(
        &pool,
        &prefix,
        course,
        "SummaryGroupCourseTopic",
        "side_comment",
    )
    .await;
    let resp = server
        .get(&format!(
            "/groups/{group_id}/discussion_topics/{course_topic}/summaries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT .../update_discussion_types and PUT .../migrate_disallow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_discussion_types_covers_every_side_comment_and_migrate_disallow_converts_them() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "TypesCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let teacher_bearer = auth(teacher, "teacher");

    let first = seed_topic(&pool, &prefix, course, "TypesFirst", "side_comment").await;
    let second = seed_topic(&pool, &prefix, course, "TypesSecond", "side_comment").await;
    let keep = seed_topic(&pool, &prefix, course, "TypesKeep", "threaded").await;

    let url = format!("/courses/{course}/discussion_topics/update_discussion_types");

    // A student may not manage the discussion types.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "threaded": [first], "not_threaded": [second] }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(resp.text(), "forbidden");

    // Without the topic arrays Canvas answers 400 "Missing data".
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["error"], "Missing data");

    // A partial list is invalid data: the count of side-comment topics is 2.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "threaded": [first] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "Invalid data");
    assert_eq!(body["count"], 2);

    // Covering every side-comment topic is accepted.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "threaded": [first], "not_threaded": [second] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["success"], "true");

    let (first_type, second_type, keep_type): (String, String, String) = sqlx::query_as(
        "SELECT
            (SELECT discussion_type FROM discussion_topics WHERE id = $1),
            (SELECT discussion_type FROM discussion_topics WHERE id = $2),
            (SELECT discussion_type FROM discussion_topics WHERE id = $3)",
    )
    .bind(first)
    .bind(second)
    .bind(keep)
    .fetch_one(&pool)
    .await
    .expect("topic rows");
    assert_eq!(first_type, "threaded");
    assert_eq!(second_type, "not_threaded");
    assert_eq!(keep_type, "threaded", "untouched topics keep their type");

    // migrate_disallow turns the remaining side comments into threaded ones.
    let third = seed_topic(&pool, &prefix, course, "TypesThird", "side_comment").await;
    let resp = server
        .put(&format!(
            "/courses/{course}/discussion_topics/migrate_disallow"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["update_count"], 1);

    let third_type: String =
        sqlx::query_scalar("SELECT discussion_type FROM discussion_topics WHERE id = $1")
            .bind(third)
            .fetch_one(&pool)
            .await
            .expect("topic row");
    assert_eq!(third_type, "threaded");

    // A student may not migrate the discussion types either.
    let resp = server
        .put(&format!(
            "/courses/{course}/discussion_topics/migrate_disallow"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
