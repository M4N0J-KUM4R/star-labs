//! Integration tests for star-discussions (discussion topics + threaded entries).
//!
//! These tests build the crate's real router (`star_discussions::router()`)
//! over a real Postgres (migrations run, state seeded directly in the DB) and
//! pin ACTUAL endpoint behavior, including quirks:
//!
//! - `GET .../discussion_topics/{id}` hides soft-deleted topics (404), like
//!   the list endpoint and the entries endpoints.
//! - `DELETE .../discussion_topics/{id}` is repeatable: deleting an
//!   already-deleted topic returns 204 again (the existence check ignores
//!   workflow_state).
//! - `PUT .../discussion_topics/{id}` verifies existence before authorization,
//!   so an unauthorized caller gets 404 (not 403) for a missing topic.
//! - `ApiError` renders as a plain-text body: 401 "unauthorized",
//!   403 "forbidden", 404 "not found", 500 "database error: ...".
//! - `Validation` maps to 422 (Unprocessable Entity), not 400, with a
//!   plain-text "validation: <message>" body.
//! - `per_page` is clamped into 1..=50, so `per_page=0` behaves like 1.
//! - An admin creating a topic in a nonexistent course trips the FK
//!   constraint and gets 500 with the bare body "database error" (not 404;
//!   the underlying sqlx message is only logged server-side).
//! - Entry `parent_id` must reference a non-deleted entry of the same topic,
//!   otherwise 422 "validation: parent_id must belong to the same topic".
//! - Listing entries returns only top-level entries with replies nested
//!   recursively under `replies`.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
//!   cargo test -p star-discussions
//! - Uses a unique per-run data prefix + full teardown so it is safe to run
//!   repeatedly against a shared database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// JWT secret shared by the test AppState and the tokens minted below.
const JWT_SECRET: &str = "test-jwt-secret";

/// Milliseconds to sleep between creates so `created_at` ordering is
/// deterministic (list endpoints order by created_at DESC/ASC).
const STAGGER_MS: u64 = 25;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB never
/// collide on the users.email / courses.course_code uniqueness constraints.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "disc_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the discussions
/// router. Returns None when DATABASE_URL is unset so callers can skip cleanly.
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

/// Create a user directly in the DB with the given role. Returns user id.
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

/// Create a course directly in the DB. Returns course id.
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

/// Enroll a user in a course with the given enrollment role (student|teacher).
async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(course_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed enrollment");
}

/// Mint a real bearer token the same way star-auth does at login time.
fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

/// Deterministic spacing for created_at-based ordering assertions.
async fn stagger() {
    tokio::time::sleep(std::time::Duration::from_millis(STAGGER_MS)).await;
}

/// POST /courses/{courseId}/discussion_topics as the given bearer, asserting
/// the real happy-path response (201 + JSON body).
async fn create_topic_ok(server: &TestServer, course_id: i64, bearer: &str, title: &str) -> Value {
    let resp = server
        .post(&format!("/courses/{course_id}/discussion_topics"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&json!({ "title": title }))
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "POST topic '{title}' should be 201; body: {}",
        resp.text()
    );
    resp.json()
}

/// POST an entry as the given bearer, asserting the real happy path (201).
async fn create_entry_ok(
    server: &TestServer,
    course_id: i64,
    topic_id: i64,
    bearer: &str,
    body: Value,
) -> Value {
    let resp = server
        .post(&format!(
            "/courses/{course_id}/discussion_topics/{topic_id}/entries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&body)
        .await;
    assert_eq!(
        resp.status_code(),
        201,
        "POST entry should be 201; body: {}",
        resp.text()
    );
    resp.json()
}

/// Extract the ids from a JSON array response.
fn ids(list: &Value) -> Vec<i64> {
    list.as_array()
        .expect("expected a JSON array")
        .iter()
        .map(|e| e["id"].as_i64().expect("numeric id"))
        .collect()
}

/// Find an element by its "id" field in a JSON array.
fn by_id(list: &Value, id: i64) -> &Value {
    list.as_array()
        .expect("expected a JSON array")
        .iter()
        .find(|e| e["id"].as_i64() == Some(id))
        .unwrap_or_else(|| panic!("element with id {id} not found in {list}"))
}

/// Delete every row created by this run, in FK-safe order. Entries and topics
/// cascade from their course, and enrollments cascade from both courses and
/// users; the explicit topic/entry deletes are belt-and-braces so cleanup
/// stays deterministic even if the course delete were to fail.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM discussion_entries WHERE topic_id IN
         (SELECT id FROM discussion_topics WHERE title LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM discussion_topics WHERE title LIKE $1")
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
// POST /courses/{courseId}/discussion_topics — create
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_topic_returns_201_with_canonical_shape_and_defaults() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "CreateShape").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    // Default discussion_type is "side_comment" and workflow_state "active".
    let topic = create_topic_ok(&server, course, &bearer, &format!("{prefix}-Shape-1")).await;
    assert!(topic["id"].is_i64(), "id must be numeric: {topic}");
    assert_eq!(topic["course_id"], course);
    assert_eq!(topic["title"], format!("{prefix}-Shape-1"));
    assert!(
        topic["message"].is_null(),
        "no message was sent so message must be null"
    );
    assert_eq!(topic["workflow_state"], "active");
    assert_eq!(topic["discussion_type"], "side_comment");
    assert!(
        topic["created_at"].is_string(),
        "created_at must be an RFC3339 string: {topic}"
    );
    assert!(
        topic["updated_at"].is_string(),
        "updated_at must be an RFC3339 string: {topic}"
    );

    // An explicit discussion_type is echoed back.
    let resp = server
        .post(&format!("/courses/{course}/discussion_topics"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({
            "title": format!("{prefix}-Shape-2"),
            "message": "hello",
            "discussion_type": "threaded"
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let topic2: Value = resp.json();
    assert_eq!(topic2["discussion_type"], "threaded");
    assert_eq!(topic2["message"], "hello");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_topic_admin_bypasses_enrollment() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // A global admin with NO enrollment in the course may still create topics.
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "AdminCreate").await;
    let bearer = auth(admin, "admin");

    let topic = create_topic_ok(&server, course, &bearer, &format!("{prefix}-AdminTopic")).await;
    assert_eq!(topic["course_id"], course);
    assert_eq!(topic["workflow_state"], "active");

    // Admin may also list the (enrolled-or-admin) collection.
    let resp = server
        .get(&format!("/courses/{course}/discussion_topics"))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert!(ids(&list).contains(&topic["id"].as_i64().unwrap()));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_topic_rejects_blank_title_and_unknown_type_with_422() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "CreateValidation").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let url = format!("/courses/{course}/discussion_topics");

    // Empty title -> 422 with the exact plain-text validation body.
    for bad_title in ["", "   "] {
        let resp = server
            .post(&url)
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .json(&json!({ "title": bad_title }))
            .await;
        assert_eq!(
            resp.status_code(),
            422,
            "title {bad_title:?}; body: {}",
            resp.text()
        );
        assert_eq!(resp.text(), "validation: title cannot be empty");
    }

    // Unknown discussion_type -> 422 with the exact plain-text body.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "title": "ok", "discussion_type": "weird" }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert_eq!(
        resp.text(),
        "validation: discussion_type must be 'side_comment', 'threaded', 'flat', or 'not_threaded'"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_topic_requires_teacher_or_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "CreateAuthz").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let url = format!("/courses/{course}/discussion_topics");

    // An enrolled (but non-teacher) student is rejected with 403.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "title": format!("{prefix}-nope") }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(resp.text(), "forbidden");

    // A user with no relationship to the course is rejected with 403 too.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({ "title": format!("{prefix}-nope") }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(resp.text(), "forbidden");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_topic_admin_on_missing_course_hits_fk_and_returns_500() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // QUIRK: there is no course-existence check, so an admin (who bypasses the
    // enrollment lookup) inserting a topic for a nonexistent course trips the
    // FK constraint and surfaces as a 500 "database error: ..." response.
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let bearer = auth(admin, "admin");

    let resp = server
        .post("/courses/999999999/discussion_topics")
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .json(&json!({ "title": format!("{prefix}-GhostCourse") }))
        .await;
    assert_eq!(resp.status_code(), 500, "body: {}", resp.text());
    // QUIRK: ApiError::Db renders as the bare string "database error" — the
    // underlying sqlx message is only logged server-side, never in the body.
    assert_eq!(resp.text(), "database error");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/discussion_topics — list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_topics_orders_newest_first_and_excludes_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ListOrder").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let bearer = auth(teacher, "teacher");

    // Three staggered creates -> deterministic created_at ordering.
    let t1 = create_topic_ok(&server, course, &bearer, &format!("{prefix}-T1")).await;
    stagger().await;
    let t2 = create_topic_ok(&server, course, &bearer, &format!("{prefix}-T2")).await;
    stagger().await;
    let t3 = create_topic_ok(&server, course, &bearer, &format!("{prefix}-T3")).await;

    // Soft-delete the middle one.
    let id2 = t2["id"].as_i64().unwrap();
    let resp = server
        .delete(&format!("/courses/{course}/discussion_topics/{id2}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204);

    // The list (readable by an enrolled student too) is newest-first and
    // excludes the deleted topic.
    let resp = server
        .get(&format!("/courses/{course}/discussion_topics"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(
        ids(&list),
        vec![t3["id"].as_i64().unwrap(), t1["id"].as_i64().unwrap()],
        "list must be newest-first and exclude soft-deleted topics: {list}"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn list_topics_honors_pagination_and_clamps_per_page() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "ListPages").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let mut created = Vec::new();
    for i in 1..=3 {
        created.push(create_topic_ok(&server, course, &bearer, &format!("{prefix}-P{i}")).await);
        stagger().await;
    }
    let t3 = created[2]["id"].as_i64().unwrap();
    let t2 = created[1]["id"].as_i64().unwrap();
    let t1 = created[0]["id"].as_i64().unwrap();

    let url = format!("/courses/{course}/discussion_topics");

    // page=1&per_page=2 -> the two newest topics.
    let resp = server
        .get(&url)
        .add_query_params([("page", "1"), ("per_page", "2")])
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(
        ids(&resp.json()),
        vec![t3, t2],
        "page 1 must hold the two newest"
    );

    // page=2&per_page=2 -> the oldest topic.
    let resp = server
        .get(&url)
        .add_query_params([("page", "2"), ("per_page", "2")])
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(ids(&resp.json()), vec![t1], "page 2 must hold the oldest");

    // page past the end -> an empty array (not an error).
    let resp = server
        .get(&url)
        .add_query_params([("page", "3"), ("per_page", "2")])
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(
        ids(&resp.json()),
        Vec::<i64>::new(),
        "past-the-end page must be empty"
    );

    // QUIRK: per_page is clamped into 1..=50, so per_page=0 behaves as 1.
    let resp = server
        .get(&url)
        .add_query_params([("page", "1"), ("per_page", "0")])
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(ids(&resp.json()), vec![t3], "per_page=0 must clamp to 1");

    // per_page larger than the cap is clamped down too (all 3 topics fit).
    let resp = server
        .get(&url)
        .add_query_params([("page", "1"), ("per_page", "1000")])
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(ids(&resp.json()), vec![t3, t2, t1]);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/discussion_topics/{id} — get one
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_topic_returns_topic_for_enrolled_student() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "GetOne").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let topic = create_topic_ok(
        &server,
        course,
        &auth(teacher, "teacher"),
        &format!("{prefix}-GetMe"),
    )
    .await;
    let id = topic["id"].as_i64().unwrap();

    // An enrolled student (not just the teacher) can read a single topic.
    let resp = server
        .get(&format!("/courses/{course}/discussion_topics/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let got: Value = resp.json();
    assert_eq!(got["id"], id);
    assert_eq!(got["course_id"], course);
    assert_eq!(got["title"], format!("{prefix}-GetMe"));
    assert_eq!(got["workflow_state"], "active");
    assert_eq!(got["discussion_type"], "side_comment");
    assert!(
        got["message"].is_null(),
        "no message was sent so message must be null"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_topic_of_another_course_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student_b = seed_user(&pool, &prefix, "StudentB", "student").await;
    let course_a = seed_course(&pool, &prefix, "CourseA").await;
    let course_b = seed_course(&pool, &prefix, "CourseB").await;
    seed_enrollment(&pool, teacher, course_a, "teacher").await;
    seed_enrollment(&pool, student_b, course_b, "student").await;

    let topic_a = create_topic_ok(
        &server,
        course_a,
        &auth(teacher, "teacher"),
        &format!("{prefix}-InA"),
    )
    .await;
    let id_a = topic_a["id"].as_i64().unwrap();

    // The student is legitimately enrolled in course B, but the topic belongs
    // to course A -> the (id, course_id) lookup misses -> 404.
    let resp = server
        .get(&format!("/courses/{course_b}/discussion_topics/{id_a}"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(student_b, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.text(), "not found");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn get_topic_of_soft_deleted_topic_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    // GET one must hide soft-deleted topics just like the list endpoint does.
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "GetDeleted").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let topic = create_topic_ok(&server, course, &bearer, &format!("{prefix}-Deleted")).await;
    let id = topic["id"].as_i64().unwrap();
    let url = format!("/courses/{course}/discussion_topics/{id}");

    // Before the delete the single-topic endpoint answers 200.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204);

    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "get-one must hide a soft-deleted topic; body: {}",
        resp.text()
    );
    assert_eq!(resp.text(), "not found");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /courses/{courseId}/discussion_topics/{id} — update
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_topic_merges_partial_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UpdateMerge").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let topic = create_topic_ok(&server, course, &bearer, &format!("{prefix}-Original")).await;
    let id = topic["id"].as_i64().unwrap();
    let url = format!("/courses/{course}/discussion_topics/{id}");

    // Update only the title: message and discussion_type must be preserved.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "title": format!("{prefix}-Renamed") }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["id"], id);
    assert_eq!(updated["title"], format!("{prefix}-Renamed"));
    assert!(updated["message"].is_null(), "unset message must stay null");
    assert_eq!(updated["discussion_type"], "side_comment");
    assert_eq!(updated["workflow_state"], "active");

    // Update only the message: the earlier title must be preserved.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "message": "new body" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["title"], format!("{prefix}-Renamed"));
    assert_eq!(updated["message"], "new body");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_topic_rejects_blank_title_with_422() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "UpdateValidation").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let topic = create_topic_ok(&server, course, &bearer, &format!("{prefix}-Keep")).await;
    let id = topic["id"].as_i64().unwrap();

    // A whitespace-only merged title is rejected with the exact 422 body.
    let resp = server
        .put(&format!("/courses/{course}/discussion_topics/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .json(&json!({ "title": "   " }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert_eq!(resp.text(), "validation: title cannot be empty");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_and_delete_authz_and_404_ordering() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "MutationAuthz").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let teacher_bearer = auth(teacher, "teacher");
    let student_bearer = auth(student, "student");

    let topic = create_topic_ok(
        &server,
        course,
        &teacher_bearer,
        &format!("{prefix}-Guarded"),
    )
    .await;
    let id = topic["id"].as_i64().unwrap();
    let url = format!("/courses/{course}/discussion_topics/{id}");
    let missing = format!("/courses/{course}/discussion_topics/999999999");

    // Enrolled student cannot update an existing topic -> 403.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "title": "hijack" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(resp.text(), "forbidden");

    // QUIRK: update verifies existence BEFORE authorization, so even the
    // unauthorized student gets 404 (not 403) for a missing topic.
    let resp = server
        .put(&missing)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "title": "hijack" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.text(), "not found");

    // The authorized teacher gets 404 for a missing topic as well.
    let resp = server
        .put(&missing)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({ "title": "nope" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Enrolled student cannot delete an existing topic -> 403.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(resp.text(), "forbidden");

    // Teacher deleting a missing topic -> 404.
    let resp = server
        .delete(&missing)
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.text(), "not found");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE /courses/{courseId}/discussion_topics/{id} — soft delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_topic_soft_deletes_hides_from_list_and_is_repeatable() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "DeleteFlow").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let topic = create_topic_ok(&server, course, &bearer, &format!("{prefix}-Doomed")).await;
    let id = topic["id"].as_i64().unwrap();
    let url = format!("/courses/{course}/discussion_topics/{id}");

    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    assert_eq!(resp.text(), "", "204 must carry an empty body");

    // Soft delete: the row remains, only its workflow_state changes.
    let state: (String,) =
        sqlx::query_as("SELECT workflow_state FROM discussion_topics WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("soft-deleted row must still exist");
    assert_eq!(state.0, "deleted");

    // QUIRK: deleting an already-deleted topic succeeds again (204) because
    // the existence check ignores workflow_state.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(
        resp.status_code(),
        204,
        "repeat delete must be 204 (pinned quirk); body: {}",
        resp.text()
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST .../discussion_topics/{topicId}/entries — post entries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_entry_enrolled_student_gets_201_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "EntryShape").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let topic = create_topic_ok(
        &server,
        course,
        &auth(teacher, "teacher"),
        &format!("{prefix}-EntryTopic"),
    )
    .await;
    let topic_id = topic["id"].as_i64().unwrap();

    let entry = create_entry_ok(
        &server,
        course,
        topic_id,
        &auth(student, "student"),
        json!({ "message": "first!" }),
    )
    .await;
    assert!(entry["id"].is_i64(), "entry id must be numeric: {entry}");
    assert_eq!(entry["topic_id"], topic_id);
    assert_eq!(entry["parent_id"], Value::Null, "root entry has no parent");
    assert_eq!(entry["user_id"], student, "entry must record its author");
    assert_eq!(entry["message"], "first!");
    assert_eq!(entry["workflow_state"], "active");
    assert!(entry["created_at"].is_string());
    assert_eq!(
        entry["replies"].as_array().map(Vec::len),
        Some(0),
        "fresh entry has empty replies"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_entry_unenrolled_403_cross_course_and_deleted_topic_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student_a = seed_user(&pool, &prefix, "StudentA", "student").await;
    let student_b = seed_user(&pool, &prefix, "StudentB", "student").await;
    let course_a = seed_course(&pool, &prefix, "EntryCourseA").await;
    let course_b = seed_course(&pool, &prefix, "EntryCourseB").await;
    seed_enrollment(&pool, teacher, course_a, "teacher").await;
    seed_enrollment(&pool, student_a, course_a, "student").await;
    seed_enrollment(&pool, student_b, course_b, "student").await;

    let topic_a = create_topic_ok(
        &server,
        course_a,
        &auth(teacher, "teacher"),
        &format!("{prefix}-TopicA"),
    )
    .await;
    let topic_a_id = topic_a["id"].as_i64().unwrap();
    let entries_a = format!("/courses/{course_a}/discussion_topics/{topic_a_id}/entries");

    // A user not enrolled in course A is rejected with 403.
    let resp = server
        .post(&entries_a)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(student_b, "student"),
        )
        .json(&json!({ "message": "sneaking in" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    assert_eq!(resp.text(), "forbidden");

    // Addressing a course-A topic through the course-B path is a 404 (the
    // topic's course is compared against the path course).
    let resp = server
        .post(&format!(
            "/courses/{course_b}/discussion_topics/{topic_a_id}/entries"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(student_b, "student"),
        )
        .json(&json!({ "message": "wrong door" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.text(), "not found");

    // Reading entries through the wrong course path is a 404 too.
    let resp = server
        .get(&format!(
            "/courses/{course_b}/discussion_topics/{topic_a_id}/entries"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(student_b, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // After the topic is soft-deleted, both posting and listing entries 404.
    let resp = server
        .delete(&format!(
            "/courses/{course_a}/discussion_topics/{topic_a_id}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204);

    let resp = server
        .post(&entries_a)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(student_a, "student"),
        )
        .json(&json!({ "message": "too late" }))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "posting to a deleted topic must 404; body: {}",
        resp.text()
    );
    assert_eq!(resp.text(), "not found");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn create_entry_rejects_blank_message_and_foreign_or_missing_parent() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "EntryValidation").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let teacher_bearer = auth(teacher, "teacher");
    let student_bearer = auth(student, "student");

    let topic = create_topic_ok(&server, course, &teacher_bearer, &format!("{prefix}-Main")).await;
    let topic_id = topic["id"].as_i64().unwrap();
    let other_topic =
        create_topic_ok(&server, course, &teacher_bearer, &format!("{prefix}-Other")).await;
    let other_topic_id = other_topic["id"].as_i64().unwrap();
    let url = format!("/courses/{course}/discussion_topics/{topic_id}/entries");

    // Blank message -> 422 with the exact plain-text body.
    for bad in ["", "   "] {
        let resp = server
            .post(&url)
            .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
            .json(&json!({ "message": bad }))
            .await;
        assert_eq!(
            resp.status_code(),
            422,
            "message {bad:?}; body: {}",
            resp.text()
        );
        assert_eq!(resp.text(), "validation: message cannot be empty");
    }

    // A parent entry from a DIFFERENT topic of the same course is rejected.
    let foreign_parent = create_entry_ok(
        &server,
        course,
        other_topic_id,
        &student_bearer,
        json!({ "message": "in the other topic" }),
    )
    .await;
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({
            "message": "cross-topic reply",
            "parent_id": foreign_parent["id"]
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert_eq!(
        resp.text(),
        "validation: parent_id must belong to the same topic"
    );

    // A nonexistent parent id is rejected with the same 422.
    let resp = server
        .post(&url)
        .add_header(axum::http::header::AUTHORIZATION, student_bearer)
        .json(&json!({ "message": "ghost parent", "parent_id": 999999999 }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    assert_eq!(
        resp.text(),
        "validation: parent_id must belong to the same topic"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET .../discussion_topics/{topicId}/entries — threaded listing
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_entries_returns_two_level_nested_replies() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Threaded").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let student_bearer = auth(student, "student");

    let topic = create_topic_ok(
        &server,
        course,
        &auth(teacher, "teacher"),
        &format!("{prefix}-Thread"),
    )
    .await;
    let topic_id = topic["id"].as_i64().unwrap();

    // Two root entries; a reply to the first; a reply to that reply.
    let root1 = create_entry_ok(
        &server,
        course,
        topic_id,
        &student_bearer,
        json!({ "message": "root one" }),
    )
    .await;
    stagger().await;
    let root2 = create_entry_ok(
        &server,
        course,
        topic_id,
        &student_bearer,
        json!({ "message": "root two" }),
    )
    .await;
    let reply1 = create_entry_ok(
        &server,
        course,
        topic_id,
        &student_bearer,
        json!({ "message": "reply to root one", "parent_id": root1["id"] }),
    )
    .await;
    let reply2 = create_entry_ok(
        &server,
        course,
        topic_id,
        &student_bearer,
        json!({ "message": "reply to the reply", "parent_id": reply1["id"] }),
    )
    .await;

    let resp = server
        .get(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/entries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();

    // Only top-level entries are returned, with replies nested recursively.
    let top = ids(&list);
    assert_eq!(
        top.len(),
        2,
        "grandchild must NOT appear at top level: {list}"
    );
    let r1 = by_id(&list, root1["id"].as_i64().unwrap());
    assert_eq!(r1["parent_id"], Value::Null);
    assert_eq!(
        r1["replies"].as_array().map(Vec::len),
        Some(1),
        "one direct reply: {list}"
    );
    let child = &r1["replies"][0];
    assert_eq!(child["id"], reply1["id"]);
    assert_eq!(child["parent_id"], root1["id"]);
    assert_eq!(
        child["replies"].as_array().map(Vec::len),
        Some(1),
        "grandchild must be nested inside the reply: {list}"
    );
    assert_eq!(child["replies"][0]["id"], reply2["id"]);
    assert_eq!(child["replies"][0]["parent_id"], reply1["id"]);

    // The second root has no replies.
    let r2 = by_id(&list, root2["id"].as_i64().unwrap());
    assert_eq!(
        r2["replies"].as_array().map(Vec::len),
        Some(0),
        "root two must have empty replies: {list}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Full lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_lifecycle_topic_entry_reply_then_soft_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Lifecycle").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let teacher_bearer = auth(teacher, "teacher");
    let student_bearer = auth(student, "student");

    // 1. Teacher creates the topic.
    let topic = create_topic_ok(
        &server,
        course,
        &teacher_bearer,
        &format!("{prefix}-Lifecycle"),
    )
    .await;
    let topic_id = topic["id"].as_i64().unwrap();

    // 2. Student posts an entry.
    let entry = create_entry_ok(
        &server,
        course,
        topic_id,
        &student_bearer,
        json!({ "message": "discussion opener" }),
    )
    .await;

    // 3. Teacher replies to the entry (teachers are enrolled-or-admin too).
    let reply = create_entry_ok(
        &server,
        course,
        topic_id,
        &teacher_bearer,
        json!({ "message": "teacher answer", "parent_id": entry["id"] }),
    )
    .await;
    assert_eq!(reply["parent_id"], entry["id"]);
    assert_eq!(reply["user_id"], teacher);

    // 4. The threaded listing shows the exchange nested.
    let resp = server
        .get(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/entries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let list: Value = resp.json();
    assert_eq!(ids(&list).len(), 1, "one top-level entry expected: {list}");
    let top = &list[0];
    assert_eq!(top["id"], entry["id"]);
    assert_eq!(top["replies"][0]["id"], reply["id"]);

    // 5. Teacher soft-deletes the topic.
    let resp = server
        .delete(&format!("/courses/{course}/discussion_topics/{topic_id}"))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer)
        .await;
    assert_eq!(resp.status_code(), 204);

    // 6. Get-one hides the deleted topic, like the list endpoint.
    let resp = server
        .get(&format!("/courses/{course}/discussion_topics/{topic_id}"))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    assert_eq!(resp.text(), "not found");

    // 7. The entries endpoints treat the deleted topic as gone.
    let resp = server
        .get(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/entries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "entries of a deleted topic must 404"
    );
    let resp = server
        .post(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/entries"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "message": "afterlife" }))
        .await;
    assert_eq!(
        resp.status_code(),
        404,
        "posting to a deleted topic must 404"
    );

    // 8. The topic list no longer shows it.
    let resp = server
        .get(&format!("/courses/{course}/discussion_topics"))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer)
        .await;
    assert_eq!(resp.status_code(), 200);
    assert!(
        !ids(&resp.json()).contains(&topic_id),
        "deleted topic must be absent from the list"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// 401 without a token on every route
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_route_rejects_anonymous_callers_with_401() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let course = seed_course(&pool, &prefix, "AnonCourse").await;
    let base = format!("/courses/{course}/discussion_topics");
    let one = format!("{base}/1");
    let entries = format!("{base}/1/entries");

    // GET list.
    let resp = server.get(&base).await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    assert_eq!(resp.text(), "unauthorized");

    // POST create topic.
    let resp = server
        .post(&base)
        .json(&json!({ "title": "anonymous" }))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    assert_eq!(resp.text(), "unauthorized");

    // GET one.
    let resp = server.get(&one).await;
    assert_eq!(resp.status_code(), 401);
    assert_eq!(resp.text(), "unauthorized");

    // PUT update.
    let resp = server
        .put(&one)
        .json(&json!({ "title": "anonymous" }))
        .await;
    assert_eq!(resp.status_code(), 401);
    assert_eq!(resp.text(), "unauthorized");

    // DELETE.
    let resp = server.delete(&one).await;
    assert_eq!(resp.status_code(), 401);
    assert_eq!(resp.text(), "unauthorized");

    // POST entry.
    let resp = server
        .post(&entries)
        .json(&json!({ "message": "anonymous" }))
        .await;
    assert_eq!(resp.status_code(), 401);
    assert_eq!(resp.text(), "unauthorized");

    // GET entries.
    let resp = server.get(&entries).await;
    assert_eq!(resp.status_code(), 401);
    assert_eq!(resp.text(), "unauthorized");

    cleanup(&pool, &prefix).await;
}
