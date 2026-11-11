//! Independent behavioral tests for `star-learning-dates` date_details API.
//!
//! Covers Canvas `LearningObjectDatesController` contract:
//! - GET/PUT /courses/{courseId}/assignments/{assignmentId}/date_details
//! - GET/PUT .../quizzes/{quizId}/date_details
//! - GET/PUT .../discussion_topics/{topicId}/date_details
//! - GET/PUT .../pages/{urlOrId}/date_details
//!
//! Canvas reference: `LearningObjectDatesController`, `Api::V1::AssignmentOverride`.
//!
//! Independent from dev integration tests; covers permissions, graded flag
//! by asset type, response shape, override set_type derivation, empty-override
//! delete semantics, pages-ungraded contract, and 404 cases.
//!
//! Skipped when DATABASE_URL is not set.

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
        "lod_ind_{}_{}",
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
        r#"INSERT INTO users (name, email, password_hash, role)
           VALUES ($1, $2, $3, $4) RETURNING id"#,
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
// Permissions
// ---------------------------------------------------------------------------

/// Anonymous caller gets 401 regardless of URL validity.
#[tokio::test]
async fn anon_get_is_401() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://localhost/none")
        .expect("lazy pool");
    let server = TestServer::new(star_learning_dates::router().with_state(state(pool)))
        .expect("build TestServer");

    let resp = server.get("/courses/1/assignments/1/date_details").await;
    assert_eq!(resp.status_code(), 401);
}

/// Admin bypasses enrollment check on GET.
#[tokio::test]
async fn admin_can_get_without_enrollment() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "AdminGet").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;
    let bearer = auth(admin, "admin");

    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// Non-enrolled non-admin GET is 403.
#[tokio::test]
async fn non_enrolled_get_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "NoAccess").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;

    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// Student (enrolled, not teacher) on PUT is 403.
#[tokio::test]
async fn student_cannot_put_date_details() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "StudentPut").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;

    let resp = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "due_at": "2030-01-01T00:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// Teacher or admin PUT succeeds with 204 No Content.
#[tokio::test]
async fn teacher_put_is_204_no_content() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "TeacherPut").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;

    let resp = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "due_at": "2030-06-15T12:00:00Z" }))
        .await;
    assert_eq!(resp.status_code(), 204);
    assert_eq!(resp.text(), "");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// graded flag by asset type (Canvas contract)
// ---------------------------------------------------------------------------

/// Quiz returns graded=true.
#[tokio::test]
async fn quiz_graded_is_true() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "QuizGraded").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let quiz_id: i64 =
        sqlx::query_scalar("INSERT INTO quizzes (course_id, title) VALUES ($1, $2) RETURNING id")
            .bind(course)
            .bind(format!("{prefix}-Quiz"))
            .fetch_one(&pool)
            .await
            .expect("seed quiz");

    let details: Value = server
        .get(&format!("/courses/{course}/quizzes/{quiz_id}/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(details["graded"], json!(true), "Canvas: quizzes are graded");

    cleanup(&pool, &prefix).await;
}

/// DiscussionTopic returns graded=false.
#[tokio::test]
async fn discussion_topic_graded_is_false() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "TopicGraded").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let topic_id: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, title) VALUES ($1, $2) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-Topic"))
    .fetch_one(&pool)
    .await
    .expect("seed topic");

    let details: Value = server
        .get(&format!(
            "/courses/{course}/discussion_topics/{topic_id}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(
        details["graded"],
        json!(false),
        "Canvas: discussion_topics are ungraded unless checkpointed"
    );

    cleanup(&pool, &prefix).await;
}

/// WikiPage returns graded=false (pages are never graded).
#[tokio::test]
async fn page_graded_is_false() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PageGraded").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let page_id: i64 = sqlx::query_scalar(
        "INSERT INTO pages (course_id, title, url) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-Page"))
    .bind(format!("{prefix}-page"))
    .fetch_one(&pool)
    .await
    .expect("seed page");

    let details: Value = server
        .get(&format!("/courses/{course}/pages/{page_id}/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(
        details["graded"],
        json!(false),
        "Canvas: pages are never graded"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Response shape
// ---------------------------------------------------------------------------

/// GET response has all required LearningObjectDates fields.
#[tokio::test]
async fn date_details_response_has_required_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Shape").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;

    server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "due_at": "2030-01-01T00:00:00Z",
            "unlock_at": "2029-12-01T00:00:00Z",
            "lock_at": "2030-02-01T00:00:00Z",
            "only_visible_to_overrides": false
        }))
        .await;

    let details: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();

    for field in [
        "id",
        "due_at",
        "lock_at",
        "unlock_at",
        "only_visible_to_overrides",
        "graded",
        "visible_to_everyone",
        "overrides",
    ] {
        assert!(
            details.get(field).is_some(),
            "missing field: {field} in {details}"
        );
    }

    cleanup(&pool, &prefix).await;
}

/// Override in GET response has all required AssignmentOverride fields.
#[tokio::test]
async fn override_response_has_required_fields() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "OverrideFields").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;

    server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "assignment_overrides": [{
                "title": "Section Override",
                "course_section_id": 77,
                "due_at": "2030-01-10T00:00:00Z"
            }]
        }))
        .await;

    let details: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();

    let overrides = details["overrides"].as_array().expect("overrides array");
    assert!(!overrides.is_empty(), "expected at least one override");
    let ov = &overrides[0];
    for field in [
        "id",
        "title",
        "student_ids",
        "group_id",
        "course_section_id",
        "noop_id",
        "due_at",
        "unlock_at",
        "lock_at",
    ] {
        assert!(ov.get(field).is_some(), "missing override field: {field}");
    }

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Override set_type derivation
// ---------------------------------------------------------------------------

/// group_id without explicit set_type → set_type is derived as "Group".
/// Canvas: Api::V1::AssignmentOverride derives set_type from key presence.
#[tokio::test]
async fn override_set_type_derived_from_group_id() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "SetType").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;

    server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "assignment_overrides": [{
                "title": "Group Override",
                "group_id": 123
                // NO set_type field - implementation must derive it as "Group"
            }]
        }))
        .await;

    let details: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();

    let overrides = details["overrides"].as_array().expect("overrides array");
    assert_eq!(overrides.len(), 1);
    assert_eq!(
        overrides[0]["group_id"],
        json!(123),
        "group_id should be preserved as set_id in output"
    );
    assert_eq!(
        overrides[0]["course_section_id"],
        Value::Null,
        "course_section_id should not be set when group_id was used"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Empty override list deletes all overrides (Canvas contract)
// ---------------------------------------------------------------------------

/// Canvas PUT contract: "Providing an empty list will delete all of the
/// object's overrides."
#[tokio::test]
async fn empty_override_list_deletes_all_overrides() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "EmptyOverride").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;

    // Create an override first.
    server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "assignment_overrides": [{
                "title": "To Be Deleted",
                "course_section_id": 99
            }]
        }))
        .await;

    let before: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(
        before["overrides"].as_array().unwrap().len(),
        1,
        "sanity: one override exists"
    );

    // Send empty list - Canvas: "Providing an empty list will delete all"
    server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "assignment_overrides": [] }))
        .await;

    let after: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(
        after["overrides"].as_array().unwrap().len(),
        0,
        "empty list should delete all overrides"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Pages: ungraded, due_at always null (Canvas contract)
// ---------------------------------------------------------------------------

/// Pages PUT accepts unlock_at but due_at stays null in response.
/// Canvas: "never applicable for ungraded discussions, pages, and files".
#[tokio::test]
async fn page_due_at_always_null() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PageDue").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let page_url = format!("{prefix}-page-due");
    sqlx::query_scalar(
        "INSERT INTO pages (course_id, title, url) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-PageDue"))
    .bind(&page_url)
    .fetch_one(&pool)
    .await
    .expect("seed page");

    // PUT with unlock_at only (pages are never graded)
    server
        .put(&format!("/courses/{course}/pages/{page_url}/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "unlock_at": "2031-01-01T00:00:00Z" }))
        .await;

    let details: Value = server
        .get(&format!("/courses/{course}/pages/{page_url}/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();
    assert_eq!(
        details["due_at"],
        Value::Null,
        "pages are never graded; due_at must be null"
    );
    assert_eq!(details["unlock_at"], json!("2031-01-01T00:00:00Z"));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// 404 for missing objects
// ---------------------------------------------------------------------------

/// Non-existent quiz in a real course returns 404.
#[tokio::test]
async fn missing_quiz_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "MissingQuiz").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .get(&format!("/courses/{course}/quizzes/999888777/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

/// Non-existent discussion_topic in a real course returns 404.
#[tokio::test]
async fn missing_discussion_topic_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "MissingTopic").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .get(&format!(
            "/courses/{course}/discussion_topics/999888777/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// visible_to_everyone / only_visible_to_overrides inverse
// ---------------------------------------------------------------------------

#[tokio::test]
async fn visible_to_everyone_is_inverse_of_only_visible_to_overrides() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Visibility").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;

    server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "only_visible_to_overrides": true }))
        .await;

    let details: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();

    assert_eq!(details["only_visible_to_overrides"], json!(true));
    assert_eq!(details["visible_to_everyone"], json!(false));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn visible_to_everyone_defaults_to_true_without_dates_row() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "DefaultVisible").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    // No dates row - just an assignment
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-A")).await;

    let details: Value = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await
        .json();

    assert_eq!(
        details["visible_to_everyone"],
        json!(true),
        "no dates row - visible_to_everyone defaults to true"
    );

    cleanup(&pool, &prefix).await;
}
