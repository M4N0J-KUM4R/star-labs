//! Integration tests for the star-grading-standards Canvas nested aliases.
//!
//! Exercises the real grading-standards router
//! (GET/DELETE /courses/{courseId}/grading_standards/{id}) through axum-test
//! against a real Postgres.
//!
//! Spec: the aliases delegate to the flat handlers only when the standard's
//! `grading_standards.course_id` matches the course in the path; cross-course
//! and missing ids are 404, and authz is identical to the flat routes.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-grading-standards
//!
//! Test cases:
//!   1. Alias GET + DELETE succeed for the owning course, and the delete is
//!      visible through the flat GET (404 afterwards).
//!   2. Alias GET + DELETE return 404 for a cross-course id (standard stays
//!      active) and for a missing id.
//!   3. Alias authz matches the flat routes: non-enrolled student -> 403,
//!      admin -> 200.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gs-tests-{}-{}",
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
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_grading_standards::router())
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

async fn seed_standard(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO grading_standards (course_id, title) VALUES ($1, $2) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed grading standard")
}

async fn seed_entry(pool: &PgPool, standard_id: i64, name: &str, min_score: f64, position: i32) {
    sqlx::query(
        "INSERT INTO grading_scheme_entries (grading_standard_id, name, min_score, position)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(standard_id)
    .bind(name)
    .bind(min_score)
    .bind(position)
    .execute(pool)
    .await
    .expect("seed scheme entry");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
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
async fn nested_alias_get_and_delete_succeed_for_owning_course() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let standard = seed_standard(&pool, course, "Alias Scheme").await;
    seed_entry(&pool, standard, "A", 0.90, 0).await;
    seed_entry(&pool, standard, "F", 0.0, 1).await;

    let resp = server
        .get(&format!("/courses/{course}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;

    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"].as_i64(), Some(standard));
    assert_eq!(body["title"], "Alias Scheme");
    assert_eq!(body["grading_scheme"].as_array().map(|a| a.len()), Some(2));

    let resp = server
        .delete(&format!("/courses/{course}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // The alias shares the flat handler's soft-delete: flat GET now 404s.
    let resp = server
        .get(&format!("/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_alias_is_404_for_cross_course_and_missing_ids() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let own_course = seed_course(&pool, &prefix, "OwnCourse").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, teacher, own_course, "teacher").await;
    seed_enrollment(&pool, teacher, other_course, "teacher").await;
    let standard = seed_standard(&pool, own_course, "Owned Scheme").await;

    // Cross-course GET and DELETE both 404, even for a teacher of both courses.
    let resp = server
        .get(&format!(
            "/courses/{other_course}/grading_standards/{standard}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .delete(&format!(
            "/courses/{other_course}/grading_standards/{standard}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A missing id under the owning course is also 404.
    let resp = server
        .get(&format!(
            "/courses/{own_course}/grading_standards/{}",
            standard + 1_000_000
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // The cross-course DELETE must not have touched the standard.
    let resp = server
        .get(&format!("/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["workflow_state"], "active");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_alias_preserves_flat_route_authz() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let standard = seed_standard(&pool, course, "Authz Scheme").await;

    // Non-enrolled student: flat and alias routes both 403.
    for path in [
        format!("/grading_standards/{standard}"),
        format!("/courses/{course}/grading_standards/{standard}"),
    ] {
        let resp = server
            .get(&path)
            .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
            .await;
        assert_eq!(resp.status_code(), 403, "GET {path}: {}", resp.text());
    }
    for path in [
        format!("/grading_standards/{standard}"),
        format!("/courses/{course}/grading_standards/{standard}"),
    ] {
        let resp = server
            .delete(&path)
            .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
            .await;
        assert_eq!(resp.status_code(), 403, "DELETE {path}: {}", resp.text());
    }

    // Admin bypass is unchanged through the alias.
    let resp = server
        .get(&format!("/courses/{course}/grading_standards/{standard}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
