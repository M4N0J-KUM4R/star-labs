//! Integration tests for the star-audit grade change audit log.
//!
//! Pinned behavior (Canvas `grade_change_audit_api_controller` parity):
//! - Every endpoint returns the compound document `{events, linked}`.
//! - Anonymous callers get 401; students enrolled in the course get 403
//!   (`course_authorized?` = manage_grades / view_all_grades).
//! - Unknown courses, assignments outside the course, and non-enrolled
//!   students/graders are 404.
//! - `GET /audit/grade_change` without `course_id`/`assignment_id` requires an
//!   admin (403 for teachers); an admin call with no condition at all is 400.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix and
//! full teardown keep repeat runs against a shared DB safe.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gcau_{}_{}",
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

    let server = TestServer::new(star_audit::router().with_state(state)).expect("build TestServer");
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

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, points_possible)
        VALUES ($1, $2, 10)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

#[allow(clippy::too_many_arguments)]
async fn seed_event(
    pool: &PgPool,
    uuid: &str,
    course_id: i64,
    assignment_id: i64,
    student_id: i64,
    grader_id: i64,
    grade_after: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO grade_change_events
            (uuid, course_id, assignment_id, student_id, grader_id,
             event_type, grade_before, grade_after, score_after,
             points_possible_after, version_number)
        VALUES ($1, $2, $3, $4, $5, 'grade_change', NULL, $6, 8.0, 10.0, 1)
        "#,
    )
    .bind(uuid)
    .bind(course_id)
    .bind(assignment_id)
    .bind(student_id)
    .bind(grader_id)
    .bind(grade_after)
    .execute(pool)
    .await
    .expect("seed event");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM grade_change_events WHERE uuid LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM assignments WHERE title LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn anonymous_is_401_student_is_403_and_missing_course_is_404() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "CourseAuth").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let url = format!("/audit/grade_change/courses/{course}");

    let resp = server.get(&url).await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let missing = format!("/audit/grade_change/courses/{}", course + 1_000_000);
    let resp = server
        .get(&missing)
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_events_are_compound_with_linked_objects() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "CourseCompound").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Essay")).await;
    seed_event(
        &pool,
        &format!("{prefix}-event-1"),
        course,
        assignment,
        student,
        teacher,
        "8",
    )
    .await;

    let resp = server
        .get(&format!("/audit/grade_change/courses/{course}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();

    let events = body["events"].as_array().expect("events array");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["id"], format!("{prefix}-event-1"));
    assert_eq!(events[0]["event_type"], "grade_change");
    assert_eq!(events[0]["grade_after"], "8");
    assert_eq!(events[0]["links"]["course"].as_i64(), Some(course));
    assert_eq!(events[0]["links"]["assignment"].as_i64(), Some(assignment));
    let expected_student = student.to_string();
    assert_eq!(
        events[0]["links"]["student"].as_str(),
        Some(expected_student.as_str())
    );
    let linked = &body["linked"];
    assert!(linked["courses"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"].as_i64() == Some(course)));
    assert!(linked["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["id"].as_i64() == Some(assignment)));
    assert!(linked["users"]
        .as_array()
        .unwrap()
        .iter()
        .any(|u| u["id"].as_i64() == Some(student)));
    assert_eq!(linked["page_views"].as_array().map(Vec::len), Some(0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn assignment_and_course_user_scoping() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "CourseScope").await;
    let other_course = seed_course(&pool, &prefix, "CourseOther").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Quiz")).await;
    let other_assignment =
        seed_assignment(&pool, other_course, &format!("{prefix}-OtherQuiz")).await;
    seed_event(
        &pool,
        &format!("{prefix}-event-1"),
        course,
        assignment,
        student,
        teacher,
        "9",
    )
    .await;

    let bearer = auth(teacher, "teacher");

    // Assignment from another course is a 404.
    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/assignments/{other_assignment}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Assignment-scoped query returns the matching event only.
    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/assignments/{assignment}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["events"].as_array().unwrap().len(), 1);

    // Student path: non-enrolled student is 404, enrolled returns the event.
    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/students/{outsider}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/students/{student}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["events"].as_array().unwrap().len(), 1);

    // Grader path mirrors Canvas `course.all_users.find`.
    seed_enrollment(&pool, outsider, other_course, "student").await;
    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/graders/{outsider}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/graders/{teacher}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["events"].as_array().unwrap().len(), 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn advanced_query_permission_and_condition_rules() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "CourseAdvanced").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Final")).await;
    seed_event(
        &pool,
        &format!("{prefix}-event-1"),
        course,
        assignment,
        student,
        teacher,
        "10",
    )
    .await;

    // Anonymous is 401.
    let resp = server.get("/audit/grade_change").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // Teacher without course/assignment scope is 403.
    let resp = server
        .get("/audit/grade_change")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Admin with no condition at all is 400 (Canvas message).
    let resp = server
        .get("/audit/grade_change")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Admin filtering by student only is allowed.
    let resp = server
        .get(&format!("/audit/grade_change?student_id={student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["events"].as_array().unwrap().len(), 1);

    // Teacher may query a course they teach.
    let resp = server
        .get(&format!(
            "/audit/grade_change?course_id={course}&assignment_id={assignment}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["events"].as_array().unwrap().len(), 1);
    assert_eq!(body["events"][0]["grade_after"], "10");

    // `assignment_id=override` is rejected explicitly.
    let resp = server
        .get(&format!(
            "/audit/grade_change?course_id={course}&assignment_id=override"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert!(resp.text().contains("override"));

    cleanup(&pool, &prefix).await;
}
