//! Integration tests for star-learning-dates (`date_details`).
//!
//! Pinned behavior:
//! - `GET .../date_details` for a learning object missing from the course is a
//!   404; callers that are neither enrolled nor admin get 403.
//! - Anonymous requests are rejected with 401 by the `AuthUser` extractor.
//! - `PUT .../date_details` requires a course teacher or admin (403 otherwise)
//!   and answers `204 No Content` with an empty body.
//! - The GET response is Canvas `LearningObjectDates`:
//!   `id, due_at, lock_at, unlock_at, only_visible_to_overrides, graded,
//!   visible_to_everyone, overrides`; `graded` is true only for assignments
//!   and quizzes.
//! - `assignment_overrides` in the PUT body replaces the whole override list;
//!   an assignment PUT also mirrors `due_at` onto the assignments row.
//! - Pages resolve by numeric id or by `url` slug.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix and full
//! teardown keep repeat runs against a shared DB safe.

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
        "lod_{}_{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

fn state(pool: PgPool) -> AppState {
    AppState {
        db: pool,
        jwt_secret: JWT_SECRET.to_string(),
    }
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

    let server = TestServer::new(star_learning_dates::router().with_state(state(pool.clone())))
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
    .bind("not-a-real-hash")
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
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

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO assignments (course_id, title) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(title)
        .fetch_one(pool)
        .await
        .expect("seed assignment")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM learning_object_dates WHERE course_id IN
         (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
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
// Tests
// ---------------------------------------------------------------------------

/// No DB needed: the `AuthUser` extractor rejects anonymous callers with 401.
#[tokio::test]
async fn date_details_requires_authentication() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://localhost/none")
        .expect("lazy pool");
    let server = TestServer::new(star_learning_dates::router().with_state(state(pool)))
        .expect("build TestServer");

    let resp = server.get("/courses/1/assignments/1/date_details").await;
    assert_eq!(resp.status_code(), 401);
}

#[tokio::test]
async fn assignment_date_details_round_trip_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Dates").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Essay")).await;
    let teacher_bearer = auth(teacher, "teacher");
    let student_bearer = auth(student, "student");
    let outsider_bearer = auth(outsider, "student");

    // Missing assignment and non-enrolled caller.
    let missing = server
        .get(&format!(
            "/courses/{course}/assignments/999999999/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .await;
    assert_eq!(missing.status_code(), 404, "body: {}", missing.text());

    let forbidden = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, outsider_bearer.clone())
        .await;
    assert_eq!(forbidden.status_code(), 403, "body: {}", forbidden.text());

    // Student cannot update.
    let denied = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .json(&json!({ "due_at": "2030-01-02T03:04:05Z" }))
        .await;
    assert_eq!(denied.status_code(), 403, "body: {}", denied.text());

    // Teacher writes dates plus one section override.
    let put = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({
            "due_at": "2030-01-02T03:04:05Z",
            "unlock_at": "2029-12-01T00:00:00Z",
            "lock_at": "2030-02-01T00:00:00Z",
            "only_visible_to_overrides": true,
            "assignment_overrides": [
                {
                    "title": "Section A",
                    "course_section_id": 42,
                    "due_at": "2030-01-03T03:04:05Z"
                }
            ]
        }))
        .await;
    assert_eq!(put.status_code(), 204, "body: {}", put.text());
    assert_eq!(put.text(), "");

    let details: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, student_bearer.clone())
        .await
        .json();
    assert_eq!(details["id"], json!(assignment));
    assert_eq!(details["due_at"], json!("2030-01-02T03:04:05Z"));
    assert_eq!(details["unlock_at"], json!("2029-12-01T00:00:00Z"));
    assert_eq!(details["lock_at"], json!("2030-02-01T00:00:00Z"));
    assert_eq!(details["only_visible_to_overrides"], json!(true));
    assert_eq!(details["visible_to_everyone"], json!(false));
    assert_eq!(details["graded"], json!(true));
    let overrides = details["overrides"].as_array().expect("overrides array");
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0]["title"], json!("Section A"));
    assert_eq!(overrides[0]["course_section_id"], json!(42));
    assert_eq!(overrides[0]["group_id"], Value::Null);

    // The assignment row mirrors due_at, as Canvas does.
    let assignment_due: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT due_at FROM assignments WHERE id = $1")
            .bind(assignment)
            .fetch_one(&pool)
            .await
            .expect("read assignment");
    assert!(assignment_due.is_some());

    // A second PUT replaces the override list wholesale.
    let replace = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer.clone())
        .json(&json!({
            "assignment_overrides": [
                { "title": "Adhoc", "student_ids": [student] }
            ]
        }))
        .await;
    assert_eq!(replace.status_code(), 204, "body: {}", replace.text());

    let details: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_bearer)
        .await
        .json();
    let overrides = details["overrides"].as_array().expect("overrides array");
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0]["title"], json!("Adhoc"));
    assert_eq!(overrides[0]["student_ids"], json!([student]));
    // Omitted keys keep their stored value.
    assert_eq!(details["due_at"], json!("2030-01-02T03:04:05Z"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn quiz_topic_and_page_date_details() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Other").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let quiz: i64 =
        sqlx::query_scalar("INSERT INTO quizzes (course_id, title) VALUES ($1, $2) RETURNING id")
            .bind(course)
            .bind(format!("{prefix}-Quiz"))
            .fetch_one(&pool)
            .await
            .expect("seed quiz");
    let topic: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, title) VALUES ($1, $2) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-Topic"))
    .fetch_one(&pool)
    .await
    .expect("seed topic");
    let page_slug = format!("{prefix}-page");
    let page: i64 = sqlx::query_scalar(
        "INSERT INTO pages (course_id, title, url) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-Page"))
    .bind(&page_slug)
    .fetch_one(&pool)
    .await
    .expect("seed page");

    let quiz_put = server
        .put(&format!("/courses/{course}/quizzes/{quiz}/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "due_at": "2031-05-06T07:08:09Z" }))
        .await;
    assert_eq!(quiz_put.status_code(), 204, "body: {}", quiz_put.text());

    let quiz_details: Value = server
        .get(&format!("/courses/{course}/quizzes/{quiz}/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await
        .json();
    assert_eq!(quiz_details["graded"], json!(true));
    assert_eq!(quiz_details["due_at"], json!("2031-05-06T07:08:09Z"));
    assert_eq!(quiz_details["overrides"], json!([]));

    let topic_details: Value = server
        .get(&format!(
            "/courses/{course}/discussion_topics/{topic}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await
        .json();
    assert_eq!(topic_details["graded"], json!(false));
    assert_eq!(topic_details["id"], json!(topic));

    // Pages resolve by slug and by numeric id; ungraded, so due_at stays null
    // even if supplied (Canvas: pages are never graded).
    let page_put = server
        .put(&format!("/courses/{course}/pages/{page_slug}/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "unlock_at": "2031-01-01T00:00:00Z" }))
        .await;
    assert_eq!(page_put.status_code(), 204, "body: {}", page_put.text());

    let page_details: Value = server
        .get(&format!("/courses/{course}/pages/{page_slug}/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await
        .json();
    assert_eq!(page_details["id"], json!(page));
    assert_eq!(page_details["graded"], json!(false));
    assert_eq!(page_details["unlock_at"], json!("2031-01-01T00:00:00Z"));

    let by_id: Value = server
        .get(&format!("/courses/{course}/pages/{page}/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await
        .json();
    assert_eq!(by_id["id"], json!(page));

    let unknown = server
        .get(&format!(
            "/courses/{course}/pages/{prefix}-missing/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(unknown.status_code(), 404, "body: {}", unknown.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn assignment_without_dates_row_falls_back_to_assignment_due_at() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Fallback").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let assignment: i64 = sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, due_at)
         VALUES ($1, $2, '2032-03-04T05:06:07Z') RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-Legacy"))
    .fetch_one(&pool)
    .await
    .expect("seed assignment");

    let details: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await
        .json();
    assert_eq!(details["due_at"], json!("2032-03-04T05:06:07Z"));
    assert_eq!(details["visible_to_everyone"], json!(true));
    assert_eq!(details["overrides"], json!([]));

    cleanup(&pool, &prefix).await;
}
