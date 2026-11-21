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
//! The routes of `learning_object_dates_api` (the seven routes the crate's
//! first pass did not register) are pinned as well:
//! - `GET .../modules/{id}/date_details` needs `manage_assign_to` (403 for a
//!   student) and answers `id, unlock_at, only_visible_to_overrides,
//!   visible_to_everyone, graded, overrides` — no `due_at`, no `lock_at`.
//! - `GET/PUT .../files/{id}/date_details` answer 400
//!   `{"error": "This API does not support files."}` while the site-admin
//!   `differentiated_files` feature is off.
//! - `PUT .../date_details/convert_tag_overrides` replaces the differentiation
//!   tag overrides of the learning object with ad-hoc overrides that carry the
//!   students of the tag and answers 204.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix and full
//! teardown keep repeat runs against a shared DB safe.

use axum_test::TestServer;
use chrono::{DateTime, Utc};
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

async fn seed_context_module(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO context_modules \
             (context_id, context_type, name, created_at, updated_at, workflow_state) \
         VALUES ($1, 'Course', $2, now(), now(), 'active') RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed context module")
}

async fn seed_attachment(pool: &PgPool, course_id: i64, filename: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO attachments \
             (context_id, context_type, filename, display_name, file_state, workflow_state, \
              unlock_at, lock_at, only_visible_to_overrides, created_at, updated_at) \
         VALUES ($1, 'Course', $2, $2, 'available', 'active', \
                 '2022-01-05 00:00:00', '2022-03-05 00:00:00', false, now(), now()) RETURNING id",
    )
    .bind(course_id)
    .bind(filename)
    .fetch_one(pool)
    .await
    .expect("seed attachment")
}

async fn seed_group_category(
    pool: &PgPool,
    course_id: i64,
    name: &str,
    non_collaborative: bool,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO group_categories (course_id, name, non_collaborative, workflow_state) \
         VALUES ($1, $2, $3, 'available') RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(non_collaborative)
    .fetch_one(pool)
    .await
    .expect("seed group category")
}

async fn seed_group(pool: &PgPool, course_id: i64, name: &str, category_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO groups (course_id, name, group_category_id, workflow_state) \
         VALUES ($1, $2, $3, 'available') RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(category_id)
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

/// The `learning_object_dates` row of a learning object.
async fn seed_learning_object_dates(
    pool: &PgPool,
    course_id: i64,
    kind: &str,
    object_id: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO learning_object_dates \
             (course_id, learning_object_type, learning_object_id) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(kind)
    .bind(object_id)
    .fetch_one(pool)
    .await
    .expect("seed learning object dates")
}

/// One `learning_object_date_overrides` row.
async fn seed_override(
    pool: &PgPool,
    date_id: i64,
    title: &str,
    set_type: &str,
    set_id: Option<i64>,
    student_ids: Vec<i64>,
    due_at: Option<DateTime<Utc>>,
) {
    sqlx::query(
        "INSERT INTO learning_object_date_overrides \
             (learning_object_date_id, title, set_type, set_id, student_ids, due_at) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(date_id)
    .bind(title)
    .bind(set_type)
    .bind(set_id)
    .bind(student_ids)
    .bind(due_at)
    .execute(pool)
    .await
    .expect("seed override");
}

/// Canvas `Account.site_admin.feature_enabled?` — the site admin is the first
/// root account.
async fn set_site_admin_feature(pool: &PgPool, prefix: &str, feature: &str, state: &str) {
    let account_id: i64 = match sqlx::query_scalar(
        "SELECT id FROM accounts WHERE parent_account_id IS NULL AND workflow_state <> 'deleted' \
         ORDER BY id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .expect("query root account")
    {
        Some(id) => id,
        None => sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
            .bind(format!("{prefix}-root-account"))
            .fetch_one(pool)
            .await
            .expect("seed root account"),
    };

    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state) \
         VALUES ('Account', $1, $2, $2, $3) \
         ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = $3",
    )
    .bind(account_id)
    .bind(feature)
    .bind(state)
    .execute(pool)
    .await
    .expect("set site admin feature");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let pattern = format!("{prefix}-%");
    let _ = sqlx::query(
        "DELETE FROM learning_object_dates WHERE course_id IN
         (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM attachments WHERE context_id IN
         (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM context_modules WHERE context_id IN
         (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM group_memberships WHERE group_id IN
         (SELECT id FROM groups WHERE course_id IN
          (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM groups WHERE course_id IN
         (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM group_categories WHERE course_id IN
         (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(&pattern)
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Account' \
           AND context_id = (SELECT id FROM accounts WHERE parent_account_id IS NULL \
                              AND workflow_state <> 'deleted' ORDER BY id ASC LIMIT 1) \
           AND feature IN ('differentiated_files', 'discussion_checkpoints')",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(&pattern)
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(&pattern)
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

// ---------------------------------------------------------------------------
// learning_object_dates_api — the seven routes the first pass did not register
// ---------------------------------------------------------------------------

#[tokio::test]
async fn module_date_details_needs_manage_assign_to() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Modules").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let module_id = seed_context_module(&pool, course, &format!("{prefix}-Module")).await;
    let bearer = auth(teacher, "teacher");

    // A student has no `manage_course_content_edit`, so no `manage_assign_to`.
    let forbidden = server
        .get(&format!(
            "/courses/{course}/modules/{module_id}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "body: {}", forbidden.text());

    let details: Value = server
        .get(&format!(
            "/courses/{course}/modules/{module_id}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await
        .json();
    assert_eq!(details["id"], json!(module_id));
    assert_eq!(details["unlock_at"], Value::Null);
    assert_eq!(details["only_visible_to_overrides"], json!(false));
    assert_eq!(details["visible_to_everyone"], json!(true));
    assert_eq!(details["graded"], json!(false));
    assert_eq!(details["overrides"], json!([]));

    // A module of another course is a 404, and the 404 comes before the 403.
    let missing = server
        .get(&format!("/courses/{course}/modules/987654321/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(missing.status_code(), 404, "body: {}", missing.text());

    // An anonymous caller is a 401.
    let anonymous = server
        .get(&format!(
            "/courses/{course}/modules/{module_id}/date_details"
        ))
        .await;
    assert_eq!(anonymous.status_code(), 401, "body: {}", anonymous.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn module_date_details_lists_its_overrides() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "ModuleOverrides").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let module_id = seed_context_module(&pool, course, &format!("{prefix}-Module")).await;
    let bearer = auth(teacher, "teacher");

    let category = seed_group_category(&pool, course, &format!("{prefix}-Tags"), true).await;
    let group = seed_group(&pool, course, &format!("{prefix}-Tag"), category).await;
    let date_id = seed_learning_object_dates(&pool, course, "ContextModule", module_id).await;
    seed_override(
        &pool,
        date_id,
        &format!("{prefix}-Tag"),
        "Group",
        Some(group),
        vec![],
        None,
    )
    .await;

    let details: Value = server
        .get(&format!(
            "/courses/{course}/modules/{module_id}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await
        .json();
    assert_eq!(details["only_visible_to_overrides"], json!(true));
    assert_eq!(details["visible_to_everyone"], json!(false));
    assert_eq!(
        details["overrides"][0]["context_module_id"],
        json!(module_id)
    );
    assert_eq!(details["overrides"][0]["group_id"], json!(group));
    assert_eq!(
        details["overrides"][0]["title"],
        json!(format!("{prefix}-Tag"))
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn file_date_details_needs_the_differentiated_files_feature() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Files").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let attachment = seed_attachment(&pool, course, &format!("{prefix}-coolpdf.pdf")).await;
    let bearer = auth(teacher, "teacher");

    // The site admin feature is off: Canvas answers 400.
    set_site_admin_feature(&pool, &prefix, "differentiated_files", "disabled").await;
    let unsupported = server
        .get(&format!(
            "/courses/{course}/files/{attachment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(
        unsupported.status_code(),
        400,
        "body: {}",
        unsupported.text()
    );
    assert_eq!(
        unsupported.json::<Value>(),
        json!({ "error": "This API does not support files." })
    );

    // With the feature on a student still has no `manage_files_edit`.
    set_site_admin_feature(&pool, &prefix, "differentiated_files", "enabled").await;
    let forbidden = server
        .get(&format!(
            "/courses/{course}/files/{attachment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "body: {}", forbidden.text());

    let details: Value = server
        .get(&format!(
            "/courses/{course}/files/{attachment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await
        .json();
    assert_eq!(details["id"], json!(attachment));
    assert_eq!(details["unlock_at"], json!("2022-01-05T00:00:00Z"));
    assert_eq!(details["lock_at"], json!("2022-03-05T00:00:00Z"));
    assert_eq!(details["only_visible_to_overrides"], json!(false));
    assert_eq!(details["visible_to_everyone"], json!(true));
    assert_eq!(details["graded"], json!(false));
    assert_eq!(details["overrides"], json!([]));

    // A file of another course is a 404.
    let missing = server
        .get(&format!("/courses/{course}/files/987654321/date_details"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(missing.status_code(), 404, "body: {}", missing.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn file_date_details_put_updates_the_attachment() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "FilePut").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let attachment = seed_attachment(&pool, course, &format!("{prefix}-coolpdf.pdf")).await;
    let bearer = auth(teacher, "teacher");
    set_site_admin_feature(&pool, &prefix, "differentiated_files", "enabled").await;

    let put = server
        .put(&format!(
            "/courses/{course}/files/{attachment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({
            "unlock_at": "2030-01-01T00:00:00Z",
            "lock_at": "2030-02-01T00:00:00Z",
            "only_visible_to_overrides": true,
            "assignment_overrides": [
                { "title": "Section A", "course_section_id": 42 }
            ]
        }))
        .await;
    assert_eq!(put.status_code(), 204, "body: {}", put.text());

    let details: Value = server
        .get(&format!(
            "/courses/{course}/files/{attachment}/date_details"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await
        .json();
    assert_eq!(details["unlock_at"], json!("2030-01-01T00:00:00Z"));
    assert_eq!(details["lock_at"], json!("2030-02-01T00:00:00Z"));
    assert_eq!(details["only_visible_to_overrides"], json!(true));
    assert_eq!(details["visible_to_everyone"], json!(false));
    assert_eq!(
        details["overrides"].as_array().map(|list| list.len()),
        Some(1)
    );
    assert_eq!(details["overrides"][0]["course_section_id"], json!(42));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn convert_tag_overrides_turns_tag_overrides_into_adhoc_ones() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "Convert").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Essay")).await;
    let bearer = auth(teacher, "teacher");

    // A differentiation tag: a group of a non-collaborative group category.
    let category = seed_group_category(&pool, course, &format!("{prefix}-Tags"), true).await;
    let group = seed_group(&pool, course, &format!("{prefix}-Tag"), category).await;
    seed_group_membership(&pool, group, student).await;
    seed_group_membership(&pool, group, other).await;

    let date_id = seed_learning_object_dates(&pool, course, "Assignment", assignment).await;
    let due_at: DateTime<Utc> = "2030-03-01T00:00:00Z".parse().expect("parse due at");
    seed_override(
        &pool,
        date_id,
        &format!("{prefix}-Tag"),
        "Group",
        Some(group),
        vec![],
        Some(due_at),
    )
    .await;

    let put = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details/convert_tag_overrides"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(put.status_code(), 204, "body: {}", put.text());

    let overrides: Vec<OverrideProbe> = sqlx::query_as(
        "SELECT set_type, set_id, student_ids, due_at FROM learning_object_date_overrides \
         WHERE learning_object_date_id = $1 ORDER BY id",
    )
    .bind(date_id)
    .fetch_all(&pool)
    .await
    .expect("read overrides");

    // The tag override is gone; one ad-hoc override carries the students.
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].set_type, "ADHOC");
    assert_eq!(overrides[0].set_id, None);
    assert_eq!(overrides[0].student_ids, vec![student, other]);
    assert_eq!(overrides[0].due_at, Some(due_at));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn convert_tag_overrides_is_a_noop_without_tag_overrides() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ConvertNoop").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Essay")).await;
    let bearer = auth(teacher, "teacher");

    // A group of a collaborative category is not a differentiation tag.
    let category = seed_group_category(&pool, course, &format!("{prefix}-Groups"), false).await;
    let group = seed_group(&pool, course, &format!("{prefix}-Group"), category).await;
    seed_group_membership(&pool, group, student).await;

    let date_id = seed_learning_object_dates(&pool, course, "Assignment", assignment).await;
    seed_override(
        &pool,
        date_id,
        &format!("{prefix}-Group"),
        "Group",
        Some(group),
        vec![],
        None,
    )
    .await;

    let put = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details/convert_tag_overrides"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(put.status_code(), 204, "body: {}", put.text());

    let overrides: Vec<(String,)> = sqlx::query_as(
        "SELECT set_type FROM learning_object_date_overrides \
         WHERE learning_object_date_id = $1 ORDER BY id",
    )
    .bind(date_id)
    .fetch_all(&pool)
    .await
    .expect("read overrides");

    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].0, "Group");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn convert_tag_overrides_answers_404_and_403_like_canvas() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ConvertGuard").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Essay")).await;

    // A student has no `manage_assign_to`.
    let forbidden = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/date_details/convert_tag_overrides"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "body: {}", forbidden.text());

    // A missing assignment is a 404.
    let missing = server
        .put(&format!(
            "/courses/{course}/assignments/987654321/date_details/convert_tag_overrides"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(missing.status_code(), 404, "body: {}", missing.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn convert_tag_overrides_answers_204_for_every_learning_object() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "ConvertAll").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let bearer = auth(teacher, "teacher");

    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Essay")).await;
    let quiz: i64 =
        sqlx::query_scalar("INSERT INTO quizzes (course_id, title) VALUES ($1, $2) RETURNING id")
            .bind(course)
            .bind(format!("{prefix}-Quiz"))
            .fetch_one(&pool)
            .await
            .expect("seed quiz");
    let discussion: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, title) VALUES ($1, $2) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-Topic"))
    .fetch_one(&pool)
    .await
    .expect("seed discussion topic");
    let page: i64 = sqlx::query_scalar(
        "INSERT INTO pages (course_id, title, body, url) VALUES ($1, $2, '', $3) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-Page"))
    .bind(format!("{prefix}-page-url"))
    .fetch_one(&pool)
    .await
    .expect("seed page");

    for path in [
        format!("/courses/{course}/assignments/{assignment}/date_details/convert_tag_overrides"),
        format!("/courses/{course}/quizzes/{quiz}/date_details/convert_tag_overrides"),
        format!(
            "/courses/{course}/discussion_topics/{discussion}/date_details/convert_tag_overrides"
        ),
        format!("/courses/{course}/pages/{page}/date_details/convert_tag_overrides"),
    ] {
        let response = server
            .put(&path)
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .await;
        assert_eq!(
            response.status_code(),
            204,
            "path: {path} body: {}",
            response.text()
        );
    }

    cleanup(&pool, &prefix).await;
}
