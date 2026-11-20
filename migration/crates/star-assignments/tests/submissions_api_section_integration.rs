//! Integration tests for the Canvas `submissions_api` routes the parity
//! report counted as missing: the section twins of the submissions API and
//! the course level bulk grade update.
//!
//! Pinned behavior:
//! - `POST /courses/{courseId}/submissions/update_grades` grades several
//!   assignments at once, `grade_data` being
//!   `{ assignment_id => { user_id => { posted_grade } } }`;
//! - the section twins answer the same shape and are scoped to the section;
//! - the rubric and document annotation read states are the Canvas user
//!   preferences of the student the submission belongs to;
//! - `clear_unread` is site-admin only;
//! - `/students/submissions` and `/submission_summary` answer the section
//!   scoped view.
//!
//! Fleet rule: dedicated users, courses, sections and assignments per test,
//! every seeded row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "subapi-tests-{}-{}",
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
            .merge(star_assignments::router())
            .with_state(state),
    )
    .expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(name)
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .fetch_one(pool)
    .await
    .expect("seed course")
}

async fn seed_section(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO course_sections (course_id, name) VALUES ($1, $2) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed course section")
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

async fn seed_section_enrollment(pool: &PgPool, user_id: i64, course_id: i64, section_id: i64) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, course_section_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'student', 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(section_id)
    .execute(pool)
    .await
    .expect("seed section enrollment");
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible, workflow_state) \
         VALUES ($1, $2, 100, 'published') RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_submission(pool: &PgPool, assignment_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO submissions (assignment_id, user_id, body) VALUES ($1, $2, 'answer') \
         RETURNING id",
    )
    .bind(assignment_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM submission_reads WHERE submission_id IN (
             SELECT s.id FROM submissions s
             JOIN assignments a ON a.id = s.assignment_id
             WHERE a.course_id IN (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM submissions WHERE assignment_id IN (
             SELECT id FROM assignments WHERE course_id IN (
                 SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM assignments WHERE course_id IN (
             SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM user_preferences WHERE user_id IN (
             SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM course_sections WHERE course_id IN (
             SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN (
             SELECT id FROM courses WHERE course_code LIKE $1)",
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
async fn the_course_bulk_grade_update_grades_every_assignment() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let first = seed_assignment(&pool, course, "First").await;
    let second = seed_assignment(&pool, course, "Second").await;

    // The teacher grades both assignments at once.
    let resp = server
        .post(&format!("/courses/{course}/submissions/update_grades"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "grade_data": {
                first.to_string(): { student.to_string(): { "posted_grade": 88 } },
                second.to_string(): { student.to_string(): { "posted_grade": 91 } },
            }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let progress: Value = resp.json();
    assert_eq!(progress["context_type"], "Course");
    assert_eq!(progress["completion"], 100);

    let scores: Vec<(i64, Option<f64>)> = sqlx::query_as(
        "SELECT assignment_id, score::float8 FROM submissions \
         WHERE user_id = $1 AND assignment_id = ANY($2) ORDER BY assignment_id",
    )
    .bind(student)
    .bind(vec![first, second])
    .fetch_all(&pool)
    .await
    .expect("read scores");
    assert_eq!(scores.len(), 2);
    assert_eq!(scores[0].1, Some(88.0));
    assert_eq!(scores[1].1, Some(91.0));

    // An unknown assignment is a 400 with the Canvas message.
    let resp = server
        .post(&format!("/courses/{course}/submissions/update_grades"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "grade_data": { "999999999": { student.to_string(): { "posted_grade": 5 } } }
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert!(resp.text().contains("Some assignments could not be found"));

    // A missing grade_data is a 400.
    let resp = server
        .post(&format!("/courses/{course}/submissions/update_grades"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_section_bulk_grade_update_is_scoped_to_the_section() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_section_enrollment(&pool, student, course, section).await;
    seed_enrollment(&pool, outsider, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Homework").await;

    // The section twin grades the students of the section.
    let resp = server
        .post(&format!(
            "/sections/{section}/assignments/{assignment}/submissions/update_grades"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "grade_data": { student.to_string(): { "posted_grade": 77 } }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let progress: Value = resp.json();
    assert_eq!(progress["context_type"], "CourseSection");
    assert_eq!(progress["context_id"], section);

    let score: Option<f64> = sqlx::query_scalar(
        "SELECT score::float8 FROM submissions WHERE assignment_id = $1 AND user_id = $2",
    )
    .bind(assignment)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("read score");
    assert_eq!(score, Some(77.0));

    // A student of the course but not of the section is not graded.
    let resp = server
        .post(&format!(
            "/sections/{section}/assignments/{assignment}/submissions/update_grades"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "grade_data": { outsider.to_string(): { "posted_grade": 12 } }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let outsider_score: Option<f64> = sqlx::query_scalar(
        "SELECT score::float8 FROM submissions WHERE assignment_id = $1 AND user_id = $2",
    )
    .bind(assignment)
    .bind(outsider)
    .fetch_one(&pool)
    .await
    .expect("read score");
    assert_eq!(outsider_score, None);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_read_states_answer_the_user_preference() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Homework").await;
    seed_submission(&pool, assignment, student).await;

    // Nothing is unread yet, so the submission counts as read.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}/rubric_comments/read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["read"], true);

    // The rubric assessment twin answers the same.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}/rubric_assessments/read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["read"], true);

    // A teacher may read the state of a student.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}/document_annotations/read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["read"], true);

    // Marking the rubric assessments read answers `{"read": true}`.
    let resp = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}/rubric_comments/read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["read"], true);

    // Only the owner may mark the annotations read.
    let resp = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}/document_annotations/read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}/document_annotations/read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["read"], true);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_section_read_state_routes_answer_the_same() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_section_enrollment(&pool, student, course, section).await;
    let assignment = seed_assignment(&pool, course, "Homework").await;
    seed_submission(&pool, assignment, student).await;

    let resp = server
        .get(&format!(
            "/sections/{section}/assignments/{assignment}/submissions/{student}/rubric_assessments/read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["read"], true);

    let resp = server
        .put(&format!(
            "/sections/{section}/assignments/{assignment}/submissions/{student}/rubric_assessments/read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["read"], true);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_submission_item_can_be_marked_read() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Homework").await;
    seed_submission(&pool, assignment, student).await;

    // The student marks the grade read.
    let resp = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}/read/grade"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    // An unknown item is a 400.
    let resp = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}/read/nonsense"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn clearing_the_unread_state_is_site_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_section_enrollment(&pool, student, course, section).await;
    let assignment = seed_assignment(&pool, course, "Homework").await;
    seed_submission(&pool, assignment, student).await;

    // A teacher may not clear the unread state.
    let resp = server
        .put(&format!(
            "/sections/{section}/submissions/{student}/clear_unread"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // The site admin may.
    let resp = server
        .put(&format!(
            "/sections/{section}/submissions/{student}/clear_unread"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_section_student_views_are_scoped_to_the_section() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let section = seed_section(&pool, course, "Section").await;
    seed_section_enrollment(&pool, student, course, section).await;
    seed_enrollment(&pool, outsider, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Homework").await;
    seed_submission(&pool, assignment, student).await;

    // The submissions of the section.
    let resp = server
        .get(&format!("/sections/{section}/students/submissions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let list = list.as_array().expect("array");
    assert!(list
        .iter()
        .all(|item| item["user_id"] == student), "{list:?}");

    // The summary of the assignment.
    let resp = server
        .get(&format!(
            "/sections/{section}/assignments/{assignment}/submission_summary"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let summary: Value = resp.json();
    assert!(summary.get("graded").is_some(), "{summary}");
    assert!(summary.get("ungraded").is_some(), "{summary}");
    assert!(summary.get("not_submitted").is_some(), "{summary}");

    // The bulk mark read of the section.
    let resp = server
        .put(&format!("/sections/{section}/submissions/bulk_mark_read"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "submissionIds": [] }))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
