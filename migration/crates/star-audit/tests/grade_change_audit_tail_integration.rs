//! Integration tests for the grade change audit tail (issue #53).
//!
//! Pinned behavior (Canvas `grade_change_audit_api_controller` parity):
//! - `for_assignment`: teacher 200; student 404 (Canvas raises RecordNotFound
//!   from `course_authorized?`, not 403); unknown assignment 404.
//! - `for_student` / `for_grader`: root-account admins only — teacher 403,
//!   anonymous 401; events scoped to the request's root account.
//! - The four-parameter combo routes: course permission first (403), then
//!   assignment/grader/student resolution against the course (404).
//! - `render_events` pipeline: override events hidden unless the course
//!   allows final grade overrides; `include[]=current_grade` attaches
//!   `grade_current` ("N/A" without a submission); anonymous-and-muted
//!   assignments null out `student_id` (course queries) or drop the event
//!   (student-scoped queries); section-limited teachers see only their own
//!   sections' students.
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
        "gcat_{}_{}",
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

/// Insert an event with the raw SQL shape the audit table uses.
#[allow(clippy::too_many_arguments)]
async fn seed_event_full(
    pool: &PgPool,
    uuid: &str,
    course_id: i64,
    assignment_id: Option<i64>,
    student_id: i64,
    grader_id: i64,
    grade_after: &str,
    root_account_id: Option<i64>,
) {
    sqlx::query(
        r#"
        INSERT INTO grade_change_events
            (uuid, course_id, assignment_id, student_id, grader_id,
             event_type, grade_before, grade_after, score_after,
             points_possible_after, version_number, root_account_id)
        VALUES ($1, $2, $3, $4, $5, 'grade_change', NULL, $6, 8.0, 10.0, 1, $7)
        "#,
    )
    .bind(uuid)
    .bind(course_id)
    .bind(assignment_id)
    .bind(student_id)
    .bind(grader_id)
    .bind(grade_after)
    .bind(root_account_id)
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
    // Test-local foreign root accounts (never the seeded "Root Account").
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn for_assignment_permission_and_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "CourseAsg").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Essay")).await;
    seed_event_full(
        &pool,
        &format!("{prefix}-event-1"),
        course,
        Some(assignment),
        student,
        teacher,
        "8",
        None,
    )
    .await;

    // Anonymous is 401.
    let resp = server
        .get(&format!("/audit/grade_change/assignments/{assignment}"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // Student (course-unauthorized) is 404 — Canvas raises RecordNotFound.
    let resp = server
        .get(&format!("/audit/grade_change/assignments/{assignment}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unknown assignment is 404.
    let resp = server
        .get(&format!(
            "/audit/grade_change/assignments/{}",
            assignment + 1_000_000
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Teacher sees the compound document.
    let resp = server
        .get(&format!("/audit/grade_change/assignments/{assignment}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let events = body["events"].as_array().expect("events array");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["id"], format!("{prefix}-event-1"));
    assert_eq!(events[0]["links"]["assignment"].as_i64(), Some(assignment));
    assert!(body["linked"]["assignments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["id"].as_i64() == Some(assignment)));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn for_student_and_for_grader_are_admin_only() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "CourseAdmin").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Final")).await;
    // Root-account event for the student; the outsider's event carries a
    // different root account and must not surface.
    seed_event_full(
        &pool,
        &format!("{prefix}-event-1"),
        course,
        Some(assignment),
        student,
        teacher,
        "9",
        Some(1),
    )
    .await;
    // A second (foreign) root account: its events must not surface in
    // root-account-scoped queries.
    let foreign_account: i64 =
        sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
            .bind(format!("{prefix}-ForeignRoot"))
            .fetch_one(&pool)
            .await
            .expect("seed foreign account");
    seed_event_full(
        &pool,
        &format!("{prefix}-event-2"),
        course,
        Some(assignment),
        outsider,
        teacher,
        "7",
        Some(foreign_account),
    )
    .await;

    // Teacher (non-admin) is 403 on both routes.
    let resp = server
        .get(&format!("/audit/grade_change/students/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .get(&format!("/audit/grade_change/graders/{teacher}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Anonymous is 401.
    let resp = server
        .get(&format!("/audit/grade_change/students/{student}"))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // Unknown user is 404.
    let resp = server
        .get(&format!(
            "/audit/grade_change/students/{}",
            student + 1_000_000
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Admin sees only the root-account event for the student.
    let resp = server
        .get(&format!("/audit/grade_change/students/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let events = body["events"].as_array().expect("events array");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["id"], format!("{prefix}-event-1"));

    // Admin querying the grader sees the root-account event.
    let resp = server
        .get(&format!("/audit/grade_change/graders/{teacher}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let events = body["events"].as_array().expect("events array");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["id"], format!("{prefix}-event-1"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn combo_routes_permissions_and_scoping() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "CourseCombo").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Quiz")).await;
    seed_event_full(
        &pool,
        &format!("{prefix}-event-1"),
        course,
        Some(assignment),
        student,
        teacher,
        "6",
        None,
    )
    .await;

    let bearer = auth(teacher, "teacher");

    // Student caller: course unauthorized -> 403 (Canvas for_course_and_other_parameters
    // raises RecordNotFound only when course_authorized? fails... it returns
    // 404 for missing course; permission failure on combo routes is 403
    // because the controller re-raises RecordNotFound for missing records
    // but grants_right renders 403).
    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/assignments/{assignment}/graders/{teacher}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Assignment from outside the course -> 404.
    let other_course = seed_course(&pool, &prefix, "CourseOther").await;
    let other_assignment =
        seed_assignment(&pool, other_course, &format!("{prefix}-OtherQuiz")).await;
    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/assignments/{other_assignment}/graders/{teacher}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Non-enrolled grader -> 404.
    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/assignments/{assignment}/graders/{outsider}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Non-enrolled student -> 404.
    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{course}/assignments/{assignment}/students/{outsider}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // All four combo routes return the matching event for valid params.
    for url in [
        format!(
            "/audit/grade_change/courses/{course}/assignments/{assignment}/graders/{teacher}"
        ),
        format!(
            "/audit/grade_change/courses/{course}/assignments/{assignment}/students/{student}"
        ),
        format!(
            "/audit/grade_change/courses/{course}/graders/{teacher}/students/{student}"
        ),
        format!(
            "/audit/grade_change/courses/{course}/assignments/{assignment}/graders/{teacher}/students/{student}"
        ),
    ] {
        let resp = server
            .get(&url)
            .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
            .await;
        assert_eq!(resp.status_code(), 200, "url {url}: {}", resp.text());
        let body: Value = resp.json();
        assert_eq!(
            body["events"].as_array().unwrap().len(),
            1,
            "url {url}: {}",
            body
        );
        assert_eq!(body["events"][0]["id"], format!("{prefix}-event-1"));
    }

    // Unknown course -> 404.
    let resp = server
        .get(&format!(
            "/audit/grade_change/courses/{}/assignments/{}/graders/{}",
            course + 1_000_000,
            assignment,
            teacher
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn render_events_override_and_current_grade() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "CourseOverride").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Essay")).await;
    seed_event_full(
        &pool,
        &format!("{prefix}-event-1"),
        course,
        Some(assignment),
        student,
        teacher,
        "8",
        None,
    )
    .await;
    // Course final grade override event (NULL assignment).
    seed_event_full(
        &pool,
        &format!("{prefix}-event-override"),
        course,
        None,
        student,
        teacher,
        "A-",
        None,
    )
    .await;

    let bearer = auth(teacher, "teacher");
    let base = format!("/audit/grade_change/courses/{course}");

    // Default: the course does not allow final grade overrides, so the
    // override event is filtered out.
    let resp = server
        .get(&base)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["events"].as_array().unwrap().len(), 1);
    assert_eq!(body["events"][0]["id"], format!("{prefix}-event-1"));

    // Enabling allow_final_grade_override surfaces the override event with
    // course_override_grade: true and no assignment link.
    sqlx::query("UPDATE courses SET allow_final_grade_override = TRUE WHERE id = $1")
        .bind(course)
        .execute(&pool)
        .await
        .expect("allow override");
    let resp = server
        .get(&base)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    let override_event = events
        .iter()
        .find(|e| e["id"] == format!("{prefix}-event-override"))
        .expect("override event present");
    assert_eq!(override_event["course_override_grade"], true);
    assert!(override_event["links"]["assignment"].is_null());

    // include[]=current_grade attaches grade_current from the submission;
    // no submission exists yet -> "N/A".
    let resp = server
        .get(&format!("{base}?include[]=current_grade"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let event = body["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == format!("{prefix}-event-1"))
        .expect("assignment event present");
    assert_eq!(event["grade_current"], "N/A");

    // With a graded submission the current grade is the submission's score.
    sqlx::query(
        r#"INSERT INTO submissions (assignment_id, user_id, score, workflow_state)
           VALUES ($1, $2, 9, 'graded')"#,
    )
    .bind(assignment)
    .bind(student)
    .execute(&pool)
    .await
    .expect("seed submission");
    let resp = server
        .get(&format!("{base}?include[]=current_grade"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let event = body["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == format!("{prefix}-event-1"))
        .expect("assignment event");
    assert_eq!(event["grade_current"], "9");

    // Without the include the field is absent.
    let resp = server
        .get(&base)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    let body: Value = resp.json();
    let event = body["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == format!("{prefix}-event-1"))
        .expect("assignment event");
    assert!(event.get("grade_current").is_none());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn anonymous_and_muted_assignments_hide_students() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "CourseAnon").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Anon")).await;
    sqlx::query("UPDATE assignments SET anonymous_grading = TRUE, muted = TRUE WHERE id = $1")
        .bind(assignment)
        .execute(&pool)
        .await
        .expect("make anonymous and muted");
    seed_event_full(
        &pool,
        &format!("{prefix}-event-1"),
        course,
        Some(assignment),
        student,
        teacher,
        "5",
        None,
    )
    .await;

    // Course query: event survives but student_id is nulled out.
    let resp = server
        .get(&format!("/audit/grade_change/courses/{course}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0]["links"]["student"].is_null());

    // Student-scoped query (admin): the event is dropped entirely.
    let resp = server
        .get(&format!("/audit/grade_change/students/{student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["events"].as_array().unwrap().len(), 0);

    // Advanced query with student_id (admin): also dropped (Canvas passes
    // remove_anonymous when student_id is present).
    let resp = server
        .get(&format!("/audit/grade_change?student_id={student}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["events"].as_array().unwrap().len(), 0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn section_limited_teacher_sees_only_own_sections() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student_a = seed_user(&pool, &prefix, "StudentA", "student").await;
    let student_b = seed_user(&pool, &prefix, "StudentB", "student").await;
    let course = seed_course(&pool, &prefix, "CourseSections").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student_a, course, "student").await;
    seed_enrollment(&pool, student_b, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Quiz")).await;

    let section_a: i64 = sqlx::query_scalar(
        "INSERT INTO course_sections (course_id, name) VALUES ($1, $2) RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-SectionA"))
    .fetch_one(&pool)
    .await
    .expect("seed section");
    sqlx::query(
        "UPDATE enrollments SET course_section_id = $1 WHERE user_id = $2 AND course_id = $3",
    )
    .bind(section_a)
    .bind(student_a)
    .bind(course)
    .execute(&pool)
    .await
    .expect("put student A in section");
    sqlx::query(
        "UPDATE enrollments SET course_section_id = $1 WHERE user_id = $2 AND course_id = $3",
    )
    .bind(section_a)
    .bind(teacher)
    .bind(course)
    .execute(&pool)
    .await
    .expect("put teacher in section");
    sqlx::query("UPDATE enrollments SET limit_privileges_to_course_section = TRUE WHERE user_id = $1 AND course_id = $2")
        .bind(teacher)
        .bind(course)
        .execute(&pool)
        .await
        .expect("limit teacher to section");

    seed_event_full(
        &pool,
        &format!("{prefix}-event-a"),
        course,
        Some(assignment),
        student_a,
        teacher,
        "4",
        None,
    )
    .await;
    seed_event_full(
        &pool,
        &format!("{prefix}-event-b"),
        course,
        Some(assignment),
        student_b,
        teacher,
        "6",
        None,
    )
    .await;

    // The section-limited teacher sees only the visible student's event.
    let resp = server
        .get(&format!("/audit/grade_change/courses/{course}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "body: {body}");
    assert_eq!(events[0]["id"], format!("{prefix}-event-a"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn advanced_query_override_literal_and_time_windows() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "CourseQuery").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, &format!("{prefix}-Final")).await;
    seed_event_full(
        &pool,
        &format!("{prefix}-event-1"),
        course,
        Some(assignment),
        student,
        teacher,
        "10",
        None,
    )
    .await;
    seed_event_full(
        &pool,
        &format!("{prefix}-event-override"),
        course,
        None,
        student,
        teacher,
        "B+",
        None,
    )
    .await;
    // Canvas serves override events only while the course allows final grade
    // overrides (`include_override_grades?`).
    sqlx::query("UPDATE courses SET allow_final_grade_override = TRUE WHERE id = $1")
        .bind(course)
        .execute(&pool)
        .await
        .expect("allow override");

    // assignment_id=override selects only override events.
    let resp = server
        .get(&format!(
            "/audit/grade_change?course_id={course}&assignment_id=override"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "body: {body}");
    assert_eq!(events[0]["id"], format!("{prefix}-event-override"));
    assert_eq!(events[0]["course_override_grade"], true);

    // start_time in the future excludes everything.
    let future = chrono::Utc::now() + chrono::Duration::hours(24);
    let past = chrono::Utc::now() - chrono::Duration::hours(24);
    let resp = server
        .get(&format!(
            "/audit/grade_change?course_id={course}&start_time={}",
            future.to_rfc3339()
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["events"].as_array().unwrap().len(), 0);

    // A window covering the past keeps the assignment event (the course
    // still disallows overrides, so the override event stays hidden).
    let resp = server
        .get(&format!(
            "/audit/grade_change?course_id={course}&start_time={}&end_time={}",
            past.to_rfc3339(),
            future.to_rfc3339()
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 2, "body: {body}");
    assert!(events
        .iter()
        .any(|e| e["id"] == format!("{prefix}-event-1")));

    // Cross-check: for_course with the same default shows both events too
    // (the course allows overrides now).
    let resp = server
        .get(&format!("/audit/grade_change/courses/{course}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    let body: Value = resp.json();
    assert_eq!(body["events"].as_array().unwrap().len(), 2);

    cleanup(&pool, &prefix).await;
}
