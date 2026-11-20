//! Integration tests for the Canvas `RubricsApiController` account scope
//! routes served by `star_rubrics::rubrics_api`.
//!
//! Pinned behavior:
//! - `GET /accounts/{accountId}/rubrics` lists the active rubrics of the
//!   account's courses; a course teacher is not an account administrator and
//!   gets a 403, an unknown account a 404;
//! - `GET /accounts/{accountId}/rubrics/{id}` renders the rubric, answers
//!   `{"message": "Rubric not found"}` with a 404 for an unknown rubric and
//!   reports Canvas' `validate_args` errors with a 400;
//! - `GET /accounts/{accountId}/rubrics/{id}/used_locations` lists the
//!   courses/assignments the rubric is used in;
//! - `POST /accounts/{accountId}/rubrics/upload` creates the rubric import
//!   (`{"message": "No file attached"}` + 400 without a file) and the
//!   `upload/{id}` / `upload/{id}/rubrics` routes report its status and the
//!   rubrics it created.
//!
//! Fleet rule: dedicated users, accounts, courses, assignments and imports per
//! test, every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is
//! unset.
//!
//! The routes under test are the `RubricsApiController` account scope routes;
//! they are served by the crate router once the coordinator has wired the
//! `rubrics_api` module into it.

use axum::http::header::AUTHORIZATION;
use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "rub-api-{}-{}-{}",
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
            .merge(star_rubrics::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_account_user(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO account_users (account_id, user_id, role, workflow_state) \
         VALUES ($1, $2, 'admin', 'active')",
    )
    .bind(account_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed account user");
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

async fn seed_rubric(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rubrics (course_id, title, points_possible) VALUES ($1, $2, 100.0) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed rubric")
}

async fn seed_rubric_with_state(pool: &PgPool, course_id: i64, title: &str, state: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rubrics (course_id, title, points_possible, workflow_state) \
         VALUES ($1, $2, 100.0, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(state)
    .fetch_one(pool)
    .await
    .expect("seed rubric")
}

async fn seed_rubric_from_import(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    import_id: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rubrics (course_id, title, points_possible, rubric_imports_id) \
         VALUES ($1, $2, 100.0, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(import_id)
    .fetch_one(pool)
    .await
    .expect("seed rubric")
}

async fn seed_criterion(pool: &PgPool, rubric_id: i64, description: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rubric_criteria (rubric_id, description, points) VALUES ($1, $2, 10.0) RETURNING id",
    )
    .bind(rubric_id)
    .bind(description)
    .fetch_one(pool)
    .await
    .expect("seed criterion")
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, $2, 100.0) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_association(
    pool: &PgPool,
    rubric_id: i64,
    assignment_id: i64,
    course_id: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rubric_associations \
           (rubric_id, association_id, association_type, context_id, context_type, purpose, bookmarked, workflow_state) \
         VALUES ($1, $2, 'Assignment', $3, 'Course', 'grading', true, 'active') RETURNING id",
    )
    .bind(rubric_id)
    .bind(assignment_id)
    .bind(course_id)
    .fetch_one(pool)
    .await
    .expect("seed rubric association")
}

async fn seed_import(
    pool: &PgPool,
    context_id: i64,
    context_type: &str,
    user_id: i64,
    attachment_id: i64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rubric_imports \
           (context_id, context_type, attachment_id, user_id, workflow_state) \
         VALUES ($1, $2, $3, $4, 'created') RETURNING id",
    )
    .bind(context_id)
    .bind(context_type)
    .bind(attachment_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed rubric import")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM rubric_associations WHERE rubric_id IN \
           (SELECT id FROM rubrics WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM rubrics WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM rubric_imports WHERE \
           (context_id IN (SELECT id FROM courses WHERE course_code LIKE $1) AND context_type = 'Course') \
           OR (context_id IN (SELECT id FROM accounts WHERE name LIKE $1) AND context_type = 'Account')",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM assignments WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM account_users WHERE account_id IN \
           (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn account_rubrics_are_listed_for_account_administrators() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let other_account = seed_account(&pool, &prefix, "OtherAccount").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse", other_account).await;
    seed_account_user(&pool, account, admin).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let rubric = seed_rubric(&pool, course, "Account rubric").await;
    // A rubric of another account and an archived rubric are not listed.
    let _other = seed_rubric(&pool, other_course, "Other rubric").await;
    let _archived = seed_rubric_with_state(&pool, course, "Archived rubric", "archived").await;

    // The account administrator gets the account's active rubrics.
    let resp = server
        .get(&format!("/accounts/{account}/rubrics"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let items = list.as_array().expect("array");
    assert_eq!(items.len(), 1, "body: {list}");
    assert_eq!(items[0]["id"].as_i64(), Some(rubric));
    assert_eq!(items[0]["title"], "Account rubric");
    assert_eq!(items[0]["course_id"].as_i64(), Some(course));

    // A course teacher is not an account administrator.
    let resp = server
        .get(&format!("/accounts/{account}/rubrics"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // An unknown account is a 404.
    let resp = server
        .get("/accounts/987654321/rubrics")
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_rubric_is_shown_with_its_used_locations() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_account_user(&pool, account, admin).await;
    let rubric = seed_rubric(&pool, course, "Shown rubric").await;
    let criterion = seed_criterion(&pool, rubric, "Criterion").await;
    let assignment = seed_assignment(&pool, course, "Rubric assignment").await;
    seed_association(&pool, rubric, assignment, course).await;

    // The rubric is rendered with its nested criteria.
    let resp = server
        .get(&format!("/accounts/{account}/rubrics/{rubric}"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(rubric));
    assert_eq!(body["title"], "Shown rubric");
    let criteria = body["criteria"].as_array().expect("criteria");
    assert_eq!(criteria.len(), 1);
    assert_eq!(criteria[0]["id"].as_i64(), Some(criterion));

    // An unknown rubric is a 404 with Canvas' message.
    let resp = server
        .get(&format!("/accounts/{account}/rubrics/987654321"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "Rubric not found");

    // An invalid style is a 400 (Canvas `validate_args`).
    let resp = server
        .get(&format!("/accounts/{account}/rubrics/{rubric}?style=bogus"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    let style_errors = body["style"].as_array().expect("style errors");
    assert!(style_errors.iter().any(|message| message
        .as_str()
        .unwrap_or_default()
        .contains("invalid style requested")));

    // A style without an assessment include is a 400 as well.
    let resp = server
        .get(&format!("/accounts/{account}/rubrics/{rubric}?style=full"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // The used locations list the course and the assignment.
    let resp = server
        .get(&format!(
            "/accounts/{account}/rubrics/{rubric}/used_locations"
        ))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let locations = body.as_array().expect("locations");
    assert_eq!(locations.len(), 1, "body: {body}");
    assert_eq!(locations[0]["id"].as_i64(), Some(course));
    assert!(locations[0]["concluded?"].is_boolean());
    let assignments = locations[0]["assignments"].as_array().expect("assignments");
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0]["id"].as_i64(), Some(assignment));
    assert_eq!(assignments[0]["title"], "Rubric assignment");

    // An unknown rubric has no used locations either.
    let resp = server
        .get(&format!(
            "/accounts/{account}/rubrics/987654321/used_locations"
        ))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_rubric_upload_creates_an_import_and_reports_its_status() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    seed_account_user(&pool, account, admin).await;

    // The upload creates the rubric import.
    let resp = server
        .post(&format!("/accounts/{account}/rubrics/upload"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .json(&json!({ "attachment_id": 4242 }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let import: Value = resp.json();
    let import_id = import["id"].as_i64().expect("import id");
    assert_eq!(import["workflow_state"], "created");
    assert_eq!(import["attachment"]["id"].as_i64(), Some(4242));
    assert_eq!(import["user"]["id"].as_i64(), Some(admin));

    // Without a file the upload is a 400.
    let resp = server
        .post(&format!("/accounts/{account}/rubrics/upload"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "No file attached");

    // The status is available by id and as "latest".
    let resp = server
        .get(&format!("/accounts/{account}/rubrics/upload/{import_id}"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(import_id));

    let resp = server
        .get(&format!("/accounts/{account}/rubrics/upload/latest"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(import_id));

    // An unknown import is a 404 with the ActiveRecord message.
    let resp = server
        .get(&format!("/accounts/{account}/rubrics/upload/987654321"))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["message"],
        "Couldn't find RubricImport with 'id'=987654321"
    );

    // The import did not create any rubric yet.
    let resp = server
        .get(&format!(
            "/accounts/{account}/rubrics/upload/{import_id}/rubrics"
        ))
        .add_header(AUTHORIZATION, auth(admin, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().map(|items| items.len()), Some(0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_upload_status_and_rubrics_by_import_id_need_a_teacher() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let import_id = seed_import(&pool, course, "Course", teacher, 77).await;
    let rubric = seed_rubric_from_import(&pool, course, "Imported rubric", import_id).await;

    // The teacher reads the import status.
    let resp = server
        .get(&format!("/courses/{course}/rubrics/upload/{import_id}"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(import_id));
    assert_eq!(body["attachment"]["id"].as_i64(), Some(77));

    // "latest" returns the same import.
    let resp = server
        .get(&format!("/courses/{course}/rubrics/upload/latest"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(import_id));

    // An unknown import is a 404.
    let resp = server
        .get(&format!("/courses/{course}/rubrics/upload/987654321"))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["message"],
        "Couldn't find RubricImport with 'id'=987654321"
    );

    // The rubrics created by the import are listed.
    let resp = server
        .get(&format!(
            "/courses/{course}/rubrics/upload/{import_id}/rubrics"
        ))
        .add_header(AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("array");
    assert_eq!(items.len(), 1, "body: {body}");
    assert_eq!(items[0]["id"].as_i64(), Some(rubric));

    // A student may not read the import status.
    let resp = server
        .get(&format!("/courses/{course}/rubrics/upload/{import_id}"))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A student may not list the rubrics of the import either.
    let resp = server
        .get(&format!(
            "/courses/{course}/rubrics/upload/{import_id}/rubrics"
        ))
        .add_header(AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
