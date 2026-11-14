//! Integration tests for slice E (#30): anonymous submissions section twins
//! and `gradeable_students?allow_new_anonymous_id=true` semantics.
//!
//! Covers:
//! - GET/PUT /sections/{sectionId}/assignments/{assignmentId}/anonymous_submissions/{anonymousId}
//!   (Canvas show_anonymous / update_anonymous, section variants)
//! - GET /courses/{courseId}/assignments/{assignmentId}/gradeable_students with
//!   `allow_new_anonymous_id=true`: AnonymousUserDisplay when names are hidden,
//!   UserDisplay otherwise, anonymous_id ordering when hidden.
//!
//! Skipped cleanly when DATABASE_URL is not set, mirroring
//! submissions_read_states_integration.rs. Unique prefix per run, full teardown.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "anone-tests-{}-{}",
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

    let server = TestServer::new(submissions_router(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

fn submissions_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_assignments::router())
        .with_state(state)
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    // Populate sortable_name the way /auth/register computes it ("Last,
    // First"): star-users decodes it as a non-Option String, and NULL rows
    // leaking into the shared test database break its GET /users.
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

async fn seed_section(pool: &PgPool, course_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO course_sections (course_id, name)
        VALUES ($1, $2)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("seed course section")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// Enroll a user in a specific section of a course (enrollments.course_section_id).
async fn seed_section_enrollment(pool: &PgPool, user_id: i64, course_id: i64, section_id: i64) {
    sqlx::query(
        r#"
        INSERT INTO enrollments (user_id, course_id, course_section_id, role, workflow_state)
        VALUES ($1, $2, $3, 'student', 'active')
        "#,
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

async fn seed_submission(
    pool: &PgPool,
    assignment_id: i64,
    user_id: i64,
    anonymous_id: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO submissions (assignment_id, user_id, body, anonymous_id)
        VALUES ($1, $2, 'answer', $3)
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(anonymous_id)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let user_ids = "SELECT id FROM users WHERE email LIKE $1";
    let course_ids = "SELECT id FROM courses WHERE course_code LIKE $1";
    let _ = sqlx::query(&format!(
        "DELETE FROM submissions WHERE user_id IN ({user_ids}) \
         OR assignment_id IN (SELECT id FROM assignments WHERE course_id IN ({course_ids}))",
    ))
    .bind(format!("{prefix}%@test.local"))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM assignments WHERE course_id IN ({course_ids})",
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM enrollments WHERE user_id IN ({user_ids})",
    ))
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM course_sections WHERE course_id IN ({course_ids})",
    ))
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

// ---------------------------------------------------------------------------
// GET/PUT /sections/{sectionId}/.../anonymous_submissions/{anonymousId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn section_anonymous_submission_twin_happy_path_matches_course_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "SectionAnon").await;
    let section = seed_section(&pool, course, "S1").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_section_enrollment(&pool, student, course, section).await;
    let assignment = seed_assignment(&pool, course, "Anon").await;
    let submission = seed_submission(&pool, assignment, student, Some("anon-sec-1")).await;

    let path =
        format!("/sections/{section}/assignments/{assignment}/anonymous_submissions/anon-sec-1");
    let teacher_token = auth(teacher, "teacher");

    // The section twin returns the same shape as the course twin: user_id is
    // absent, the anonymous_id addresses the student.
    let shown = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .await;
    assert_eq!(shown.status_code(), 200, "{}", shown.text());
    let shown: Value = shown.json();
    assert_eq!(shown["id"], submission);
    assert_eq!(shown["anonymous_id"], "anon-sec-1");
    assert_eq!(shown["assignment_id"], assignment);
    assert!(shown.get("user_id").is_none(), "{shown}");
    assert!(shown["score"].is_null(), "{shown}");
    assert_eq!(
        shown["url"],
        format!("/courses/{course}/assignments/{assignment}/anonymous_submissions/anon-sec-1")
    );

    // The course twin on the same submission returns an identical body.
    let course_shown = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/anonymous_submissions/anon-sec-1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .await;
    assert_eq!(course_shown.status_code(), 200, "{}", course_shown.text());
    let course_shown: Value = course_shown.json();
    assert_eq!(shown, course_shown, "section twin must match course twin");

    // Grading by anonymous id through the section twin.
    let graded = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "score": 8.25 }))
        .await;
    assert_eq!(graded.status_code(), 200, "{}", graded.text());
    let graded: Value = graded.json();
    assert_eq!(graded["score"].as_f64(), Some(8.25));
    assert_eq!(graded["grade"], "8.25");
    assert_eq!(graded["workflow_state"], "graded");
    assert!(graded.get("user_id").is_none(), "{graded}");

    let (score, grader): (f64, i64) =
        sqlx::query_as("SELECT score::float8, grader_id FROM submissions WHERE id = $1")
            .bind(submission)
            .fetch_one(&pool)
            .await
            .expect("graded row");
    assert_eq!(score, 8.25);
    assert_eq!(grader, teacher);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn section_anonymous_submission_rejects_students_and_outsiders() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let other_student = seed_user(&pool, &prefix, "OtherStudent", "student").await;
    let course = seed_course(&pool, &prefix, "SectionNeg").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourseNeg").await;
    let section = seed_section(&pool, course, "S1").await;
    let other_section = seed_section(&pool, other_course, "OtherS1").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_section_enrollment(&pool, student, course, section).await;
    seed_enrollment(&pool, other_student, other_course, "student").await;
    let assignment = seed_assignment(&pool, course, "Anon").await;
    seed_submission(&pool, assignment, student, Some("anon-neg-1")).await;

    let path =
        format!("/sections/{section}/assignments/{assignment}/anonymous_submissions/anon-neg-1");

    // Unknown section: 404 before anything else (Canvas get_course_from_section).
    let missing_section = server
        .get(&format!(
            "/sections/0/assignments/{assignment}/anonymous_submissions/anon-neg-1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(
        missing_section.status_code(),
        404,
        "{}",
        missing_section.text()
    );

    // Assignment from another course under this section: 404.
    let wrong_assignment = seed_assignment(&pool, other_course, "Elsewhere").await;
    let foreign = server
        .get(&format!(
            "/sections/{section}/assignments/{wrong_assignment}/anonymous_submissions/anon-neg-1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(foreign.status_code(), 404, "{}", foreign.text());

    // Unknown anonymous id: 404 (no oracle to enumerate ids).
    let unknown = server
        .get(&format!(
            "/sections/{section}/assignments/{assignment}/anonymous_submissions/nope"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(unknown.status_code(), 404, "{}", unknown.text());

    // A student outside the course entirely: 403.
    let outsider_resp = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(outsider_resp.status_code(), 403, "{}", outsider_resp.text());

    // A student of the section sees their own submission; a student from
    // another course's section may not grade (PUT is teacher-only).
    let own = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(own.status_code(), 200, "{}", own.text());

    let student_put = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "score": 1.0 }))
        .await;
    assert_eq!(student_put.status_code(), 403, "{}", student_put.text());

    // A student enrolled in the course but NOT in the addressed section is not
    // visible through the section twin: 404, like Canvas'
    // get_user_considering_section scoping.
    let cross = seed_user(&pool, &prefix, "CrossSection", "student").await;
    seed_enrollment(&pool, cross, course, "student").await;
    seed_submission(&pool, assignment, cross, Some("anon-cross-1")).await;
    let cross_resp = server
        .get(&format!(
            "/sections/{section}/assignments/{assignment}/anonymous_submissions/anon-cross-1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(cross_resp.status_code(), 404, "{}", cross_resp.text());

    // The same submission IS addressable through the student's own section of
    // the same course (enrollments.course_section_id must match the addressed
    // section, not merely the course).
    let cross_section = seed_section(&pool, course, "S2").await;
    sqlx::query(
        "UPDATE enrollments SET course_section_id = $1 WHERE user_id = $2 AND course_id = $3",
    )
    .bind(cross_section)
    .bind(cross)
    .bind(course)
    .execute(&pool)
    .await
    .expect("move cross student to section S2");
    let cross_own = server
        .get(&format!(
            "/sections/{cross_section}/assignments/{assignment}/anonymous_submissions/anon-cross-1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(cross_own.status_code(), 200, "{}", cross_own.text());

    // ...but a section of a DIFFERENT course cannot address it: the section
    // resolves to another course and the assignment does not belong to it.
    let cross_foreign = server
        .get(&format!(
            "/sections/{other_section}/assignments/{assignment}/anonymous_submissions/anon-cross-1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(cross_foreign.status_code(), 404, "{}", cross_foreign.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET .../gradeable_students?allow_new_anonymous_id=true
// ---------------------------------------------------------------------------

#[tokio::test]
async fn gradeable_students_allow_new_anonymous_id_hides_names_and_sorts_by_anonymous_id() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alpha = seed_user(&pool, &prefix, "Alpha", "student").await;
    let beta = seed_user(&pool, &prefix, "Beta", "student").await;
    let course = seed_course(&pool, &prefix, "GradeableAnon").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alpha, course, "student").await;
    seed_enrollment(&pool, beta, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Exam").await;
    seed_submission(&pool, assignment, alpha, Some("zz-anon")).await;
    seed_submission(&pool, assignment, beta, Some("aa-anon")).await;

    let path = format!("/courses/{course}/assignments/{assignment}/gradeable_students");

    // Student callers are rejected (manage_grades/view_all_grades required).
    let forbidden = server
        .get(&format!("{path}?allow_new_anonymous_id=true"))
        .add_header(axum::http::header::AUTHORIZATION, auth(alpha, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    // Without anonymous_grading the flag changes nothing: UserDisplay shape.
    let plain = server
        .get(&format!("{path}?allow_new_anonymous_id=true"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(plain.status_code(), 200, "{}", plain.text());
    let plain: Vec<Value> = plain.json();
    assert_eq!(plain.len(), 2, "{plain:?}");
    assert_eq!(plain[0]["id"], alpha, "{plain:?}");
    assert_eq!(plain[0]["display_name"], "Alpha", "{plain:?}");

    // Enable anonymous grading: one ungraded (unposted) submission exists, so
    // can_view_student_names? is false and the response must anonymize.
    sqlx::query("UPDATE assignments SET anonymous_grading = TRUE WHERE id = $1")
        .bind(assignment)
        .execute(&pool)
        .await
        .expect("enable anonymous grading");

    let anonymous = server
        .get(&format!("{path}?allow_new_anonymous_id=true"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let anonymous: Vec<Value> = anonymous.json();
    let ids: Vec<&str> = anonymous
        .iter()
        .map(|s| s["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids, vec!["aa-anon", "zz-anon"], "{anonymous:?}");
    for entry in &anonymous {
        assert_eq!(entry["display_name"], "Student", "{anonymous:?}");
        assert!(entry["avatar_image_url"].is_null(), "{anonymous:?}");
        assert!(entry["html_url"].is_null(), "{anonymous:?}");
        assert!(entry["pronouns"].is_null(), "{anonymous:?}");
    }

    // sort=name cannot sort by real name when names are hidden: Canvas falls
    // back to anonymous_id ordering (descending here).
    let by_name = server
        .get(&format!(
            "{path}?allow_new_anonymous_id=true&sort=name&order=desc"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let by_name: Vec<Value> = by_name.json();
    let ids: Vec<&str> = by_name
        .iter()
        .map(|s| s["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids, vec!["zz-anon", "aa-anon"], "{by_name:?}");

    // Without the flag the same teacher still sees names (Canvas:
    // can_view_student_names = allow_new_anonymous_id ? ... : true).
    let visible = server
        .get(&format!("{path}?sort=name"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let visible: Vec<Value> = visible.json();
    let names: Vec<&str> = visible
        .iter()
        .map(|s| s["display_name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, vec!["Alpha", "Beta"], "{visible:?}");

    // Once every submission is graded (posted), anonymize_students? is false
    // again and names come back even with the flag.
    sqlx::query("UPDATE submissions SET graded_at = now(), score = 5 WHERE assignment_id = $1")
        .bind(assignment)
        .execute(&pool)
        .await
        .expect("grade every submission");

    let reposted = server
        .get(&format!("{path}?allow_new_anonymous_id=true"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let reposted: Vec<Value> = reposted.json();
    assert_eq!(reposted[0]["display_name"], "Alpha", "{reposted:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn gradeable_students_sorts_by_sortable_name_when_names_visible() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    // seed_user derives sortable_name "Zed, Zeta" and "Able, Abe" from the
    // full names, so surname order is the order sort=name must expose.
    let zeta = seed_user(&pool, &prefix, "Zeta Zed", "student").await;
    let abe = seed_user(&pool, &prefix, "Abe Able", "student").await;
    let course = seed_course(&pool, &prefix, "GradeableSort").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, zeta, course, "student").await;
    seed_enrollment(&pool, abe, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Quiz").await;
    seed_submission(&pool, assignment, zeta, None).await;
    seed_submission(&pool, assignment, abe, None).await;

    let path = format!("/courses/{course}/assignments/{assignment}/gradeable_students");
    let listed = server
        .get(&format!("{path}?sort=name"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(listed.status_code(), 200, "{}", listed.text());
    let listed: Vec<Value> = listed.json();
    let ids: Vec<i64> = listed
        .iter()
        .map(|s| s["id"].as_i64().unwrap_or_default())
        .collect();
    // "Able, Abe" sorts before "Zed, Zeta" by sortable_name; a first-name
    // order would put "Abe Able" first as well, so also assert descending.
    assert_eq!(ids, vec![abe, zeta], "{listed:?}");

    let reversed = server
        .get(&format!("{path}?sort=name&order=desc"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let reversed: Vec<Value> = reversed.json();
    let ids: Vec<i64> = reversed
        .iter()
        .map(|s| s["id"].as_i64().unwrap_or_default())
        .collect();
    assert_eq!(ids, vec![zeta, abe], "{reversed:?}");

    let default_order = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    let default_order: Vec<Value> = default_order.json();
    let ids: Vec<i64> = default_order
        .iter()
        .map(|s| s["id"].as_i64().unwrap_or_default())
        .collect();
    let mut expected = vec![abe, zeta];
    expected.sort_unstable();
    assert_eq!(ids, expected, "{default_order:?}");

    cleanup(&pool, &prefix).await;
}
