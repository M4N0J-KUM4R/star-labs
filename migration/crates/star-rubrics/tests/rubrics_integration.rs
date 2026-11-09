//! Integration tests for the Canvas nested rubric route aliases.
//!
//! Exercises GET/PUT/DELETE `/courses/{courseId}/rubrics/{id}` through the real
//! star-rubrics router against Postgres. The aliases must only delegate to the
//! flat `/rubrics/{id}` handlers when the rubric belongs to the given course;
//! a missing or cross-course rubric is a 404, and the flat handlers' authz is
//! unchanged.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-rubrics

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "rub-tests-{}-{}",
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

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM rubrics WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
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
async fn nested_aliases_get_update_delete_a_rubric_in_its_course() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let rubric = seed_rubric(&pool, course, "Original").await;

    // GET alias returns exactly what the flat route returns.
    let alias_get = server
        .get(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(alias_get.status_code(), 200, "body: {}", alias_get.text());
    let alias_body: Value = alias_get.json();
    assert_eq!(alias_body["id"].as_i64(), Some(rubric));
    assert_eq!(alias_body["course_id"].as_i64(), Some(course));

    let flat_get = server
        .get(&format!("/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(flat_get.status_code(), 200);
    let flat_body: Value = flat_get.json();
    assert_eq!(alias_body, flat_body);

    // PUT alias updates through the flat handler.
    let alias_put = server
        .put(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "title": "Renamed" }))
        .await;
    assert_eq!(alias_put.status_code(), 200, "body: {}", alias_put.text());
    let updated: Value = alias_put.json();
    assert_eq!(updated["title"].as_str(), Some("Renamed"));

    // DELETE alias soft-deletes through the flat handler.
    let alias_delete = server
        .delete(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(
        alias_delete.status_code(),
        204,
        "body: {}",
        alias_delete.text()
    );
    let (workflow_state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM rubrics WHERE id = $1")
            .bind(rubric)
            .fetch_one(&pool)
            .await
            .expect("fetch rubric state");
    assert_eq!(workflow_state, "deleted");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_aliases_return_404_for_missing_or_cross_course_rubrics() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let other_course = seed_course(&pool, &prefix, "Other").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let rubric = seed_rubric(&pool, course, "Original").await;
    let missing_rubric = 999_999_999_i64;

    // Cross-course requests are 404 even though the teacher could otherwise
    // manage the rubric in its real course.
    for method in ["get", "put", "delete"] {
        let mut req = match method {
            "get" => server.get(&format!("/courses/{other_course}/rubrics/{rubric}")),
            "put" => server.put(&format!("/courses/{other_course}/rubrics/{rubric}")),
            _ => server.delete(&format!("/courses/{other_course}/rubrics/{rubric}")),
        }
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"));
        if method == "put" {
            req = req.json(&json!({ "title": "Hijacked" }));
        }
        let resp = req.await;
        assert_eq!(
            resp.status_code(),
            404,
            "{method} cross-course must be 404, body: {}",
            resp.text()
        );
    }

    // Unknown rubric ids under the correct course are also 404.
    for method in ["get", "put", "delete"] {
        let mut req = match method {
            "get" => server.get(&format!("/courses/{course}/rubrics/{missing_rubric}")),
            "put" => server.put(&format!("/courses/{course}/rubrics/{missing_rubric}")),
            _ => server.delete(&format!("/courses/{course}/rubrics/{missing_rubric}")),
        }
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"));
        if method == "put" {
            req = req.json(&json!({ "title": "Hijacked" }));
        }
        let resp = req.await;
        assert_eq!(
            resp.status_code(),
            404,
            "{method} missing rubric must be 404, body: {}",
            resp.text()
        );
    }

    // The rejected cross-course writes left the rubric untouched.
    let (title, workflow_state): (String, String) =
        sqlx::query_as("SELECT title, workflow_state FROM rubrics WHERE id = $1")
            .bind(rubric)
            .fetch_one(&pool)
            .await
            .expect("fetch rubric");
    assert_eq!(title, "Original");
    assert_eq!(workflow_state, "active");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_aliases_preserve_flat_route_authorization() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, student, course, "student").await;
    let rubric = seed_rubric(&pool, course, "Guarded").await;

    // A student gets the same 403 from the alias and the flat route.
    let flat_get = server
        .get(&format!("/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(flat_get.status_code(), 403);

    let alias_get = server
        .get(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(alias_get.status_code(), 403);

    let alias_put = server
        .put(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "title": "Nope" }))
        .await;
    assert_eq!(alias_put.status_code(), 403);

    let alias_delete = server
        .delete(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(alias_delete.status_code(), 403);

    // Unauthenticated alias requests are still rejected by AuthUser.
    let anonymous = server
        .get(&format!("/courses/{course}/rubrics/{rubric}"))
        .await;
    assert_eq!(anonymous.status_code(), 401);

    let (title, workflow_state): (String, String) =
        sqlx::query_as("SELECT title, workflow_state FROM rubrics WHERE id = $1")
            .bind(rubric)
            .fetch_one(&pool)
            .await
            .expect("fetch rubric");
    assert_eq!(title, "Guarded");
    assert_eq!(workflow_state, "active");

    cleanup(&pool, &prefix).await;
}
