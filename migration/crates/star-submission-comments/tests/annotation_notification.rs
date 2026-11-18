//! Integration tests for the Canvas annotation_notification route in
//! star-submission-comments (issue #55).
//!
//! Exercises the real router against a real Postgres:
//! - POST /courses/{courseId}/assignments/{assignmentId}/submissions/{userId}/annotation_notification
//!
//! Canvas gates the route on `send_messages` over the Site Admin account
//! (star's global admin role), resolves the assignment / author / user and
//! answers `{}` — or Canvas' documented 400 body when the user has no
//! submission. Skipped cleanly when DATABASE_URL is not set.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "annot-{}-{}",
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
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_submission_comments::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, 'x', $4)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(sortable_name)
    .bind(format!("{prefix}-{name}@test.local"))
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

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO assignments (course_id, title) VALUES ($1, $2) RETURNING id")
        .bind(course_id)
        .bind(title)
        .fetch_one(pool)
        .await
        .expect("seed assignment")
}

async fn seed_submission(pool: &PgPool, assignment_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO submissions (assignment_id, user_id, body) VALUES ($1, $2, 'annotated') \
         RETURNING id",
    )
    .bind(assignment_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let user_ids = "SELECT id FROM users WHERE email LIKE $1";
    let course_ids = "SELECT id FROM courses WHERE course_code LIKE $1";
    let _ = sqlx::query(&format!(
        "DELETE FROM submission_comments WHERE submission_id IN \
         (SELECT id FROM submissions WHERE user_id IN ({user_ids}) \
          OR assignment_id IN (SELECT id FROM assignments WHERE course_id IN ({course_ids})))"
    ))
    .bind(format!("{prefix}%@test.local"))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM submissions WHERE user_id IN ({user_ids}) \
         OR assignment_id IN (SELECT id FROM assignments WHERE course_id IN ({course_ids}))",
    ))
    .bind(format!("{prefix}%@test.local"))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM assignments WHERE course_id IN ({course_ids})"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM enrollments WHERE user_id IN ({user_ids})"
    ))
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn annotation_notification_site_admin_contract() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let author = seed_user(&pool, &prefix, "Author", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let no_submission = seed_user(&pool, &prefix, "NoSubmission", "student").await;
    let course = seed_course(&pool, &prefix, "AnnotNotify").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, author, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, no_submission, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Annotate Me").await;
    let _submission = seed_submission(&pool, assignment, student).await;

    let base = format!(
        "/courses/{course}/assignments/{assignment}/submissions/{student}/annotation_notification"
    );

    // Site admin: 200 with the empty object (author_id as a query param,
    // the Canvas -F form spelling).
    let resp = server
        .post(&format!("{base}?author_id={author}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({}));

    // author_id in a JSON body also resolves.
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .content_type("application/json")
        .json(&json!({ "author_id": author }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({}));

    // Non-site-admin callers are forbidden (spec: "annotation_notification
    // by non-site-admin → unauthorized").
    for (user, role) in [
        (teacher, "teacher"),
        (author, "teacher"),
        (student, "student"),
    ] {
        let resp = server
            .post(&format!("{base}?author_id={author}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
            .await;
        assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    }

    // No author_id at all: Canvas api_find(User, nil) is a 404.
    let resp = server
        .post(&base)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unknown author: 404.
    let resp = server
        .post(&format!("{base}?author_id=424242"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // User without a submission: Canvas' 400 body.
    let no_sub_path = format!(
        "/courses/{course}/assignments/{assignment}/submissions/{no_submission}/annotation_notification"
    );
    let resp = server
        .post(&format!("{no_sub_path}?author_id={author}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json(),
        json!({ "error": format!("Couldn't find Submission for user with API id {no_submission}") })
    );

    // Unknown assignment / unenrolled user / unknown course: 404s.
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/424242/submissions/{student}/annotation_notification?author_id={author}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{outsider}/annotation_notification?author_id={author}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .post(&format!(
            "/courses/424242/assignments/{assignment}/submissions/{student}/annotation_notification?author_id={author}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The body stays unused when the query param carries author_id; a JSON
    // body must not break the query-param path (mixed spellings).
    let resp = server
        .post(&format!("{base}?author_id={author}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .content_type("application/json")
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({}));

    cleanup(&pool, &prefix).await;
}
