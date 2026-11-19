//! Integration tests for Canvas moderated / provisional grading (issue #31).
//!
//! Covers the slice F routes (spec p5-rest-assessments §3.2–3.4, §4.2, §6.2):
//! - GET/POST .../assignments/{id}/moderated_students (moderation_set)
//! - GET  .../assignments/{id}/provisional_grades/status
//! - PUT  .../assignments/{id}/provisional_grades/bulk_select
//! - POST .../assignments/{id}/provisional_grades/publish
//! - PUT  .../assignments/{id}/provisional_grades/{pgid}/select
//! - GET  .../assignments/{id}/anonymous_provisional_grades/status
//! - submission[provisional]/[final] param work on submissions update
//! - include[]=provisional_grades on show / for_students / gradeable_students
//!
//! Skipped cleanly when DATABASE_URL is not set. Unique prefix per run with
//! full teardown, mirroring submissions_api_integration.rs.

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
        "modgr-tests-{}-{}",
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
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, email, password_hash, role)
        VALUES ($1, $2, 'x', $3)
        RETURNING id
        "#,
    )
    .bind(name)
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

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

/// Seed a (moderated) assignment; `final_grader_id` and `anonymous_grading`
/// are optional. Canvas validates grader_count > 0 for moderated assignments,
/// so moderated seeds use 2.
#[allow(clippy::too_many_arguments)]
async fn seed_assignment(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    moderated: bool,
    final_grader_id: Option<i64>,
    anonymous_grading: bool,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments
            (course_id, title, points_possible, moderated_grading, final_grader_id,
             grader_count, anonymous_grading)
        VALUES ($1, $2, 10.0, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(moderated)
    .bind(final_grader_id)
    .bind(if moderated { 2 } else { 0 })
    .bind(anonymous_grading)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

/// Seed a submission with an explicit anonymous id.
async fn seed_submission(
    pool: &PgPool,
    assignment_id: i64,
    user_id: i64,
    anonymous_id: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO submissions (assignment_id, user_id, body, anonymous_id)
        VALUES ($1, $2, 'submitted', $3)
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

async fn cleanup(pool: &PgPool, prefix: &str) {
    let user_ids = "SELECT id FROM users WHERE email LIKE $1";
    let course_ids = "SELECT id FROM courses WHERE course_code LIKE $1";
    let _ = sqlx::query(&format!(
        "DELETE FROM anonymous_or_moderation_events WHERE assignment_id IN \
         (SELECT id FROM assignments WHERE course_id IN ({course_ids}))"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM provisional_grades WHERE submission_id IN \
         (SELECT id FROM submissions WHERE user_id IN ({user_ids}) \
          OR assignment_id IN (SELECT id FROM assignments WHERE course_id IN ({course_ids})))"
    ))
    .bind(format!("{prefix}%@test.local"))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM moderation_graders WHERE assignment_id IN \
         (SELECT id FROM assignments WHERE course_id IN ({course_ids}))"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM moderated_grading_selections WHERE assignment_id IN \
         (SELECT id FROM assignments WHERE course_id IN ({course_ids}))"
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
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
         OR assignment_id IN (SELECT id FROM assignments WHERE course_id IN ({course_ids}))"
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
        "DELETE FROM account_users WHERE user_id IN ({user_ids})",
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
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

async fn get_json(server: &TestServer, token: &str, path: &str) -> Value {
    let resp = server
        .get(path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}

async fn put_json(
    server: &TestServer,
    token: &str,
    path: &str,
    body: &Value,
) -> axum_test::TestResponse {
    server
        .put(path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(body)
        .await
}

/// PUT .../submissions/{userId} with a Canvas submission payload.
async fn grade_provisional(
    server: &TestServer,
    token: &str,
    course: i64,
    assignment: i64,
    student: i64,
    posted_grade: Value,
    provisional: bool,
    final_flag: bool,
) -> axum_test::TestResponse {
    let mut submission = json!({ "posted_grade": posted_grade, "provisional": provisional });
    if final_flag {
        submission["final"] = json!(true);
    }
    put_json(
        server,
        token,
        &format!("/courses/{course}/assignments/{assignment}/submissions/{student}"),
        &json!({ "submission": submission }),
    )
    .await
}

// ---------------------------------------------------------------------------
// Permission matrix (spec §4.2 / §7: 401 for non-moderators)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn moderation_routes_require_final_grader_or_select_final_grade() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Perms");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;
    let submission = seed_submission(&pool, assignment, student, "anon1").await;

    let teacher_token = auth(teacher, "teacher");

    // moderation_set index/create → 401 for a plain teacher.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/moderated_students"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &teacher_token)
        .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/moderated_students"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &teacher_token)
        .json(&json!({ "student_ids": [student] }))
        .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    // select/bulk_select/publish → 401 for a plain teacher.
    let pg: i64 = sqlx::query_scalar(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 5.0, '5', now()) RETURNING id",
    )
    .bind(submission)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("seed pg");

    let resp = put_json(
        &server,
        &teacher_token,
        &format!("/courses/{course}/assignments/{assignment}/provisional_grades/{pg}/select"),
        &json!({}),
    )
    .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    let resp = put_json(
        &server,
        &teacher_token,
        &format!("/courses/{course}/assignments/{assignment}/provisional_grades/bulk_select"),
        &json!({ "provisional_grade_ids": [pg] }),
    )
    .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/publish"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &teacher_token)
        .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    // A student is rejected too (no manage_grades, not the final grader).
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/publish"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    // Unknown assignment → 404 before any permission logic.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/999999/moderated_students"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &teacher_token)
        .await;
    assert_eq!(resp.status_code(), 404, "{}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_admin_with_membership_can_moderate() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Dedicated account (never account 1) + account membership.
    let account: i64 = sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-account"))
        .fetch_one(&pool)
        .await
        .expect("seed account");

    let admin = seed_user(&pool, &prefix, "Admin", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "AdminCourse");
    sqlx::query("UPDATE courses SET account_id = $1 WHERE id = $2")
        .bind(account)
        .bind(course)
        .execute(&pool)
        .await
        .expect("point course at account");
    seed_enrollment(&pool, student, course, "student").await;
    // No course enrollment for the admin: the right comes from the account
    // membership (select_final_grade), like Canvas account admins.
    sqlx::query("INSERT INTO account_users (account_id, user_id) VALUES ($1, $2)")
        .bind(account)
        .bind(admin)
        .execute(&pool)
        .await
        .expect("seed account user");

    let assignment = seed_assignment(&pool, course, "Essay", true, None, false).await;

    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/moderated_students"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "teacher"))
        .json(&json!({ "student_ids": [student] }))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let created = resp.json();
    assert_eq!(created.as_array().unwrap().len(), 1, "{created}");
    assert_eq!(created[0]["id"], student);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Moderation set (§6.2.2)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn moderation_set_create_blank_ids_is_400_empty_array() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let course = seed_course(&pool, &prefix, "Blank");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;

    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/moderated_students"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(moderator, "teacher"),
        )
        .json(&json!({ "student_ids": [] }))
        .await;
    assert_eq!(resp.status_code(), 400, "{}", resp.text());
    assert_eq!(resp.text().trim(), "[]", "Canvas renders [] with the 400");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn moderation_set_lifecycle_is_incremental() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Lifecycle");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    // `outsider` is NOT enrolled: requested but not visible, so not created.
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;

    let token = auth(moderator, "teacher");

    // First create: only Alice is visible.
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/moderated_students"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .json(&json!({ "student_ids": [alice, outsider] }))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let created = resp.json();
    assert_eq!(created.as_array().unwrap().len(), 1, "{created}");
    assert_eq!(created[0]["id"], alice);
    assert!(created[0].get("name").is_some(), "user JSON shape");

    // Incremental create keeps Alice, adds Bob.
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/moderated_students"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .json(&json!({ "student_ids": [alice, bob] }))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());

    let listed = get_json(
        &server,
        &token,
        &format!("/courses/{course}/assignments/{assignment}/moderated_students"),
    )
    .await;
    let entries = listed.as_array().expect("array");
    assert_eq!(entries.len(), 2, "alice + bob only: {listed}");
    assert_eq!(entries[0]["id"], alice, "ordered by id");
    assert_eq!(entries[1]["id"], bob);

    // Direct SQL: exactly two selection rows, outsider absent.
    let selections: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM moderated_grading_selections WHERE assignment_id = $1",
    )
    .bind(assignment)
    .fetch_one(&pool)
    .await
    .expect("count selections");
    assert_eq!(selections, 2);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Provisional grading via submissions update (§6.2.1)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_provisional_grade_on_unpublished_moderated_assignment_is_401() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Reject");
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, None, false).await;

    // Non-provisional posted_grade → 401 (Canvas render_unauthorized_action).
    let resp = grade_provisional(
        &server,
        &auth(teacher, "teacher"),
        course,
        assignment,
        student,
        json!(8.0),
        false,
        false,
    )
    .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    // Provisional → 200, the submission itself stays unscored, one pg row.
    let resp = grade_provisional(
        &server,
        &auth(teacher, "teacher"),
        course,
        assignment,
        student,
        json!(8.0),
        true,
        false,
    )
    .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let body: Value = resp.json();
    assert!(
        body["score"].is_null(),
        "provisional grading never scores the submission: {body}"
    );
    assert!(
        body["provisional_grades"].as_array().unwrap().len() == 1,
        "moderated assignments embed the provisional grades: {body}"
    );

    let (score, pg_count): (Option<f64>, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT score::float8 FROM submissions
                 WHERE assignment_id = $1 AND user_id = $2),
               (SELECT COUNT(*) FROM provisional_grades pg
                 JOIN submissions s ON s.id = pg.submission_id
                 WHERE s.assignment_id = $1 AND s.user_id = $2)
        "#,
    )
    .bind(assignment)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("check state");
    assert!(score.is_none(), "submission score untouched");
    assert_eq!(pg_count, 1);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn final_flag_silently_downgraded_without_moderation_right() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Downgrade");
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, None, false).await;

    // Teacher is NOT the final grader and holds no account membership:
    // final=true is silently downgraded (no final row, 200 response).
    let resp = grade_provisional(
        &server,
        &auth(teacher, "teacher"),
        course,
        assignment,
        student,
        json!(9.0),
        true,
        true,
    )
    .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());

    let final_rows: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM provisional_grades pg
        JOIN submissions s ON s.id = pg.submission_id
        WHERE s.assignment_id = $1 AND s.user_id = $2 AND pg.final
        "#,
    )
    .bind(assignment)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("count final");
    assert_eq!(final_rows, 0, "final flag silently downgraded");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn final_grader_gives_final_grade_after_provisional() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Final");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;

    // Final grade with no other provisional grades → 400 (Canvas
    // must_have_non_final_provisional_grade_to_create_final).
    let resp = grade_provisional(
        &server,
        &auth(moderator, "teacher"),
        course,
        assignment,
        student,
        json!(7.0),
        true,
        true,
    )
    .await;
    assert_eq!(resp.status_code(), 400, "{}", resp.text());

    // A provisional grade from the teacher, then the moderator's final.
    let resp = grade_provisional(
        &server,
        &auth(teacher, "teacher"),
        course,
        assignment,
        student,
        json!(8.0),
        true,
        false,
    )
    .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());

    let resp = grade_provisional(
        &server,
        &auth(moderator, "teacher"),
        course,
        assignment,
        student,
        json!(9.0),
        true,
        true,
    )
    .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());

    let (final_count, scorer): (i64, i64) = sqlx::query_as(
        r#"
        SELECT COUNT(*), COALESCE(MAX(scorer_id), 0)
        FROM provisional_grades pg
        JOIN submissions s ON s.id = pg.submission_id
        WHERE s.assignment_id = $1 AND s.user_id = $2 AND pg.final
        "#,
    )
    .bind(assignment)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("final row");
    assert_eq!(final_count, 1);
    assert_eq!(scorer, moderator);

    // Moderation audit trail: created + updated events exist.
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM anonymous_or_moderation_events WHERE assignment_id = $1",
    )
    .bind(assignment)
    .fetch_one(&pool)
    .await
    .expect("count events");
    assert!(events >= 2, "provisional grade audit events recorded");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Selection (§6.2.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn select_route_shapes_and_moderation_set_gate() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let course = seed_course(&pool, &prefix, "Select");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;
    let alice_sub = seed_submission(&pool, assignment, alice, "anonA").await;

    // Alice joins the moderation set; Bob does not.
    sqlx::query(
        "INSERT INTO moderated_grading_selections (assignment_id, student_id) VALUES ($1, $2)",
    )
    .bind(assignment)
    .bind(alice)
    .execute(&pool)
    .await
    .expect("seed selection");

    let pg: i64 = sqlx::query_scalar(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 8.0, '8', now()) RETURNING id",
    )
    .bind(alice_sub)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("seed pg");

    let token = auth(moderator, "teacher");

    // Unknown provisional grade id → 404.
    let resp = put_json(
        &server,
        &token,
        &format!("/courses/{course}/assignments/{assignment}/provisional_grades/999999/select"),
        &json!({}),
    )
    .await;
    assert_eq!(resp.status_code(), 404, "{}", resp.text());

    // Happy path.
    let resp = put_json(
        &server,
        &token,
        &format!("/courses/{course}/assignments/{assignment}/provisional_grades/{pg}/select"),
        &json!({}),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let selected: Value = resp.json();
    assert_eq!(selected["assignment_id"], assignment, "{selected}");
    assert_eq!(selected["student_id"], alice);
    assert_eq!(selected["selected_provisional_grade_id"], pg);

    // A moderation event was recorded for the selection.
    let events: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM anonymous_or_moderation_events
        WHERE assignment_id = $1 AND event_type = 'provisional_grade_selected'
        "#,
    )
    .bind(assignment)
    .fetch_one(&pool)
    .await
    .expect("count selection events");
    assert_eq!(events, 1);

    // Bob's grade (Bob not in the moderation set) → 400 message.
    let bob_sub = seed_submission(&pool, assignment, bob, "anonB").await;
    let bob_pg: i64 = sqlx::query_scalar(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 5.0, '5', now()) RETURNING id",
    )
    .bind(bob_sub)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("seed bob pg");
    let resp = put_json(
        &server,
        &token,
        &format!("/courses/{course}/assignments/{assignment}/provisional_grades/{bob_pg}/select"),
        &json!({}),
    )
    .await;
    assert_eq!(resp.status_code(), 400, "{}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "student not in moderation set");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn bulk_select_returns_only_changed_selections() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let course = seed_course(&pool, &prefix, "Bulk");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;
    let alice_sub = seed_submission(&pool, assignment, alice, "anonA").await;
    let bob_sub = seed_submission(&pool, assignment, bob, "anonB").await;

    for student in [alice, bob] {
        sqlx::query(
            "INSERT INTO moderated_grading_selections (assignment_id, student_id) VALUES ($1, $2)",
        )
        .bind(assignment)
        .bind(student)
        .execute(&pool)
        .await
        .expect("seed selection");
    }

    let alice_pg: i64 = sqlx::query_scalar(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 8.0, '8', now()) RETURNING id",
    )
    .bind(alice_sub)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("seed alice pg");
    let bob_pg: i64 = sqlx::query_scalar(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 5.0, '5', now()) RETURNING id",
    )
    .bind(bob_sub)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("seed bob pg");

    let token = auth(moderator, "teacher");

    let resp = put_json(
        &server,
        &token,
        &format!("/courses/{course}/assignments/{assignment}/provisional_grades/bulk_select"),
        &json!({ "provisional_grade_ids": [alice_pg, bob_pg] }),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let selected: Value = resp.json();
    let entries = selected.as_array().expect("array");
    assert_eq!(entries.len(), 2, "{selected}");
    assert_eq!(entries[0]["student_id"], alice, "request order");
    assert_eq!(entries[0]["selected_provisional_grade_id"], alice_pg);
    assert_eq!(entries[1]["student_id"], bob);
    assert_eq!(entries[1]["selected_provisional_grade_id"], bob_pg);

    // Re-selecting the same grades changes nothing: empty response, no new
    // moderation events.
    let resp = put_json(
        &server,
        &token,
        &format!("/courses/{course}/assignments/{assignment}/provisional_grades/bulk_select"),
        &json!({ "provisional_grade_ids": [alice_pg, bob_pg] }),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    assert_eq!(resp.text().trim(), "[]", "only changed selections returned");

    let events: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM anonymous_or_moderation_events
        WHERE assignment_id = $1 AND event_type = 'provisional_grade_selected'
        "#,
    )
    .bind(assignment)
    .fetch_one(&pool)
    .await
    .expect("count events");
    assert_eq!(events, 2, "one event per changed selection");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Status (§6.2.4)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_short_circuits_and_401s_for_non_moderators() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Status");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;
    let submission = seed_submission(&pool, assignment, student, "anonS").await;

    // The teacher holds grading rights but is not the moderator: the short
    // form carries only needs_provisional_grade.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/status?student_id={student}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let status: Value = resp.json();
    assert_eq!(status["needs_provisional_grade"], true, "{status}");
    assert!(status.get("provisional_grades").is_none(), "{status}");

    // A stale/changed last_updated_at asks for the full list: plain graders
    // are rejected (Canvas render_unauthorized_action).
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/status?student_id={student}&last_updated_at=2000-01-01T00:00:00Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    // A student (no grading rights) is rejected outright.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/status?student_id={student}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    // Unknown student → 404 (no submission for that user).
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/status?student_id=999999"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "{}", resp.text());

    // The moderator sees the full list: final grade split out, selected and
    // readonly flags, scorer names.
    sqlx::query(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 8.0, '8', now())",
    )
    .bind(submission)
    .bind(teacher)
    .execute(&pool)
    .await
    .expect("seed pg");
    sqlx::query(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at, final) \
         VALUES ($1, $2, 9.0, '9', now(), TRUE)",
    )
    .bind(submission)
    .bind(moderator)
    .execute(&pool)
    .await
    .expect("seed final pg");

    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/status?student_id={student}&last_updated_at=2000-01-01T00:00:00Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(moderator, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let status: Value = resp.json();
    let pgs = status["provisional_grades"].as_array().expect("pg list");
    assert_eq!(pgs.len(), 1, "{status}");
    assert_eq!(pgs[0]["scorer_id"], teacher);
    // readonly: not the caller's own grade and not final.
    assert_eq!(pgs[0]["readonly"], true, "{status}");
    assert!(pgs[0].get("scorer_name").is_some(), "{status}");
    let final_pg = &status["final_provisional_grade"];
    assert_eq!(final_pg["final"], true, "{status}");
    assert_eq!(final_pg["scorer_id"], moderator);
    assert_eq!(
        final_pg["readonly"], false,
        "final grades are never readonly"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn status_rejects_non_moderated_and_published_assignments() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Status400");
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Plain assignment → 400 "does not use moderated grading".
    let plain = seed_assignment(&pool, course, "Plain", false, None, false).await;
    seed_submission(&pool, plain, student, "anonP").await;
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{plain}/provisional_grades/status?student_id={student}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "{}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "Assignment does not use moderated grading");

    // Published moderated assignment → 400 "already been published".
    let published = seed_assignment(&pool, course, "Published", true, None, false).await;
    sqlx::query("UPDATE assignments SET grades_published_at = now() WHERE id = $1")
        .bind(published)
        .execute(&pool)
        .await
        .expect("publish");
    seed_submission(&pool, published, student, "anonQ").await;
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{published}/provisional_grades/status?student_id={student}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 400, "{}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["message"],
        "Assignment grades have already been published"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn anonymous_status_locates_submission_by_anonymous_id() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "AnonStatus");
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, None, false).await;
    seed_submission(&pool, assignment, student, "zke30fk1").await;

    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/anonymous_provisional_grades/status?anonymous_id=zke30fk1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let status: Value = resp.json();
    assert_eq!(status["needs_provisional_grade"], true, "{status}");

    // Unknown anonymous id → 404.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/anonymous_provisional_grades/status?anonymous_id=nope1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "{}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Publish (§6.2.5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn publish_uses_selection_and_fallback_and_is_irreversible() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let course = seed_course(&pool, &prefix, "Publish");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;
    let alice_sub = seed_submission(&pool, assignment, alice, "anonA").await;
    let bob_sub = seed_submission(&pool, assignment, bob, "anonB").await;

    // Alice: two graded provisional grades + a selection choosing one.
    sqlx::query(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 6.0, '6', now())",
    )
    .bind(alice_sub)
    .bind(teacher)
    .execute(&pool)
    .await
    .expect("pg1");
    let alice_pg2: i64 = sqlx::query_scalar(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 8.0, '8', now()) RETURNING id",
    )
    .bind(alice_sub)
    .bind(moderator)
    .fetch_one(&pool)
    .await
    .expect("pg2");
    sqlx::query(
        r#"
        INSERT INTO moderated_grading_selections (assignment_id, student_id, selected_provisional_grade_id)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(assignment)
    .bind(alice)
    .bind(alice_pg2)
    .execute(&pool)
    .await
    .expect("selection");

    // A provisional comment on Alice's chosen grade: published as a copy.
    sqlx::query(
        r#"
        INSERT INTO submission_comments (submission_id, author_id, comment, provisional_grade_id)
        VALUES ($1, $2, 'provisional note', $3)
        "#,
    )
    .bind(alice_sub)
    .bind(teacher)
    .bind(alice_pg2)
    .execute(&pool)
    .await
    .expect("provisional comment");

    // Bob: exactly one graded provisional grade (fallback path).
    sqlx::query(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 7.5, '7.5', now())",
    )
    .bind(bob_sub)
    .bind(teacher)
    .execute(&pool)
    .await
    .expect("bob pg");

    let token = auth(moderator, "teacher");
    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/publish"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "OK");

    // Alice got her SELECTED grade (8.0), Bob his only one (7.5).
    let (alice_score, bob_score): (Option<f64>, Option<f64>) = sqlx::query_as(
        r#"
        SELECT (SELECT score::float8 FROM submissions WHERE id = $1),
               (SELECT score::float8 FROM submissions WHERE id = $2)
        "#,
    )
    .bind(alice_sub)
    .bind(bob_sub)
    .fetch_one(&pool)
    .await
    .expect("scores");
    assert_eq!(alice_score, Some(8.0), "selected grade wins");
    assert_eq!(bob_score, Some(7.5), "single graded fallback");

    // The provisional comment was copied to a published comment.
    let published_comments: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM submission_comments WHERE submission_id = $1 AND provisional_grade_id IS NULL",
    )
    .bind(alice_sub)
    .fetch_one(&pool)
    .await
    .expect("count comments");
    assert_eq!(published_comments, 1);

    // grades_published_at is set; a second publish → 400.
    let published_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT grades_published_at FROM assignments WHERE id = $1")
            .bind(assignment)
            .fetch_one(&pool)
            .await
            .expect("published at");
    assert!(published_at.is_some());

    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/publish"
        ))
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 400, "{}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn publish_422_when_no_resolvable_grade() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Publish422");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;
    let submission = seed_submission(&pool, assignment, student, "anonS").await;

    // Two graded provisional grades, no selection: ambiguous → 422.
    for scorer in [teacher, moderator] {
        sqlx::query(
            "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
             VALUES ($1, $2, 5.0, '5', now())",
        )
        .bind(submission)
        .bind(scorer)
        .execute(&pool)
        .await
        .expect("seed pg");
    }

    let resp = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/publish"
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(moderator, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 422, "{}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["message"],
        "All submissions must have a selected grade"
    );

    // Nothing was published.
    let published_at: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT grades_published_at FROM assignments WHERE id = $1")
            .bind(assignment)
            .fetch_one(&pool)
            .await
            .expect("published at");
    assert!(published_at.is_none());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// include[]=provisional_grades (§6.2.6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn show_include_provisional_grades_filters_by_role() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ShowInc");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;
    let submission = seed_submission(&pool, assignment, student, "anonS").await;

    sqlx::query(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 8.0, '8', now())",
    )
    .bind(submission)
    .bind(teacher)
    .execute(&pool)
    .await
    .expect("teacher pg");
    sqlx::query(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at, final) \
         VALUES ($1, $2, 9.0, '9', now(), TRUE)",
    )
    .bind(submission)
    .bind(moderator)
    .execute(&pool)
    .await
    .expect("final pg");

    // The plain teacher sees only their own grade.
    let shown = get_json(
        &server,
        &auth(teacher, "teacher"),
        &format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}?include[]=provisional_grades"
        ),
    )
    .await;
    let pgs = shown["provisional_grades"].as_array().expect("pg array");
    assert_eq!(pgs.len(), 1, "{shown}");
    assert_eq!(pgs[0]["scorer_id"], teacher);
    assert!(pgs[0]["provisional_grade_id"].as_i64().unwrap() > 0);
    assert_eq!(pgs[0]["score"], 8.0);

    // The moderator sees everything with the final grade last.
    let shown = get_json(
        &server,
        &auth(moderator, "teacher"),
        &format!(
            "/courses/{course}/assignments/{assignment}/submissions/{student}?include[]=provisional_grades"
        ),
    )
    .await;
    let pgs = shown["provisional_grades"].as_array().expect("pg array");
    assert_eq!(pgs.len(), 2, "{shown}");
    assert_eq!(pgs[0]["final"], false, "non-final first");
    assert_eq!(pgs[1]["final"], true, "final sorted last");

    // Non-moderated assignments do not carry the include.
    let plain = seed_assignment(&pool, course, "Plain", false, None, false).await;
    seed_submission(&pool, plain, student, "anonP").await;
    let shown = get_json(
        &server,
        &auth(teacher, "teacher"),
        &format!(
            "/courses/{course}/assignments/{plain}/submissions/{student}?include[]=provisional_grades"
        ),
    )
    .await;
    assert!(shown.get("provisional_grades").is_none(), "{shown}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn gradeable_students_include_gated_by_permits_moderation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let course = seed_course(&pool, &prefix, "GradeableInc");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;
    let alice_sub = seed_submission(&pool, assignment, alice, "anonA").await;

    let alice_pg: i64 = sqlx::query_scalar(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 8.0, '8', now()) RETURNING id",
    )
    .bind(alice_sub)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("pg");
    sqlx::query(
        r#"
        INSERT INTO moderated_grading_selections
            (assignment_id, student_id, selected_provisional_grade_id)
        VALUES ($1, $2, $3)
        "#,
    )
    .bind(assignment)
    .bind(alice)
    .bind(alice_pg)
    .execute(&pool)
    .await
    .expect("selection");

    // Plain teacher: 401 (the include requires permits_moderation).
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/gradeable_students?include[]=provisional_grades"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    // Moderator: in_moderation_set + selected id + the pg list.
    let listed = get_json(
        &server,
        &auth(moderator, "teacher"),
        &format!(
            "/courses/{course}/assignments/{assignment}/gradeable_students?include[]=provisional_grades"
        ),
    )
    .await;
    let entries = listed.as_array().expect("array");
    assert_eq!(entries.len(), 1, "{listed}");
    assert_eq!(entries[0]["id"], alice);
    assert_eq!(entries[0]["in_moderation_set"], true, "{listed}");
    assert_eq!(entries[0]["selected_provisional_grade_id"], alice_pg);
    let pgs = entries[0]["provisional_grades"].as_array().expect("pgs");
    assert_eq!(pgs.len(), 1);

    // Without the include the plain response has no moderation fields.
    let listed = get_json(
        &server,
        &auth(teacher, "teacher"),
        &format!("/courses/{course}/assignments/{assignment}/gradeable_students"),
    )
    .await;
    let entries = listed.as_array().expect("array");
    assert_eq!(entries.len(), 1);
    assert!(entries[0].get("in_moderation_set").is_none(), "{listed}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Anonymity swap (§6.1.5 / §6.2.3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anonymity_swaps_student_and_grader_ids() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let course = seed_course(&pool, &prefix, "Anon");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    // Anonymous + moderated + unpublished → names hidden.
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), true).await;
    let alice_sub = seed_submission(&pool, assignment, alice, "zka91d").await;

    let alice_pg: i64 = sqlx::query_scalar(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 8.0, '8', now()) RETURNING id",
    )
    .bind(alice_sub)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("pg");
    // The grader's anonymous identity (moderation_graders).
    sqlx::query(
        "INSERT INTO moderation_graders (assignment_id, user_id, anonymous_id, slot_taken) \
         VALUES ($1, $2, 'grA01', TRUE)",
    )
    .bind(assignment)
    .bind(teacher)
    .execute(&pool)
    .await
    .expect("moderation grader");
    sqlx::query(
        "INSERT INTO moderated_grading_selections (assignment_id, student_id) VALUES ($1, $2)",
    )
    .bind(assignment)
    .bind(alice)
    .execute(&pool)
    .await
    .expect("selection");

    // Selection JSON: anonymous_id replaces student_id while names are
    // hidden (anonymous_grading + moderated + unpublished).
    let resp = put_json(
        &server,
        &auth(moderator, "teacher"),
        &format!("/courses/{course}/assignments/{assignment}/provisional_grades/{alice_pg}/select"),
        &json!({}),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let selected: Value = resp.json();
    assert!(selected.get("student_id").is_none(), "{selected}");
    assert_eq!(selected["anonymous_id"], "zka91d");
    assert_eq!(selected["selected_provisional_grade_id"], alice_pg);

    // Status from a plain teacher (no grader identities): the other grader's
    // provisional grade shows anonymous_grader_id instead of scorer_id.
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/provisional_grades/status?student_id={alice}&last_updated_at=2000-01-01T00:00:00Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    // Plain teachers cannot fetch the full list → 401; the moderator can.
    assert_eq!(resp.status_code(), 401, "{}", resp.text());

    // Anonymous twin status by the moderator... names hidden: grader
    // identities stay visible to the final grader only when
    // grader_names_visible_to_final_grader (default TRUE).
    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/anonymous_provisional_grades/status?anonymous_id=zka91d&last_updated_at=2000-01-01T00:00:00Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(moderator, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let status: Value = resp.json();
    let pgs = status["provisional_grades"].as_array().expect("pgs");
    assert_eq!(pgs.len(), 1, "{status}");
    assert_eq!(
        pgs[0]["scorer_id"], teacher,
        "default: names visible to final grader"
    );

    // With grader_names_visible_to_final_grader = FALSE the scorer id is
    // swapped for the anonymous grader id.
    sqlx::query(
        "UPDATE assignments SET grader_names_visible_to_final_grader = FALSE WHERE id = $1",
    )
    .bind(assignment)
    .execute(&pool)
    .await
    .expect("hide names");

    let resp = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/anonymous_provisional_grades/status?anonymous_id=zka91d&last_updated_at=2000-01-01T00:00:00Z"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(moderator, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "{}", resp.text());
    let status: Value = resp.json();
    let pgs = status["provisional_grades"].as_array().expect("pgs");
    assert_eq!(pgs.len(), 1, "{status}");
    assert!(pgs[0].get("scorer_id").is_none(), "{status}");
    assert_eq!(pgs[0]["anonymous_grader_id"], "grA01", "{status}");
    // The speedgrader URL points at the anonymous id while names are hidden.
    assert!(
        pgs[0]["speedgrader_url"]
            .as_str()
            .unwrap()
            .contains("anonymous_id=zka91d"),
        "{status}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// for_students index include (§6.2.6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn for_students_index_includes_provisional_grades() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let moderator = seed_user(&pool, &prefix, "Moderator", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "IndexInc");
    seed_enrollment(&pool, moderator, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", true, Some(moderator), false).await;
    let submission = seed_submission(&pool, assignment, student, "anonS").await;

    sqlx::query(
        "INSERT INTO provisional_grades (submission_id, scorer_id, score, grade, graded_at) \
         VALUES ($1, $2, 8.0, '8', now())",
    )
    .bind(submission)
    .bind(moderator)
    .execute(&pool)
    .await
    .expect("pg");

    let listed = get_json(
        &server,
        &auth(moderator, "teacher"),
        &format!(
            "/courses/{course}/students/submissions?student_ids={student}&assignment_ids={assignment}&include[]=provisional_grades"
        ),
    )
    .await;
    let entries = listed.as_array().expect("array");
    assert_eq!(entries.len(), 1, "{listed}");
    let pgs = entries[0]["provisional_grades"].as_array().expect("pgs");
    assert_eq!(pgs.len(), 1, "{listed}");
    assert_eq!(pgs[0]["scorer_id"], moderator);

    // Without the include: no key.
    let listed = get_json(
        &server,
        &auth(moderator, "teacher"),
        &format!(
            "/courses/{course}/students/submissions?student_ids={student}&assignment_ids={assignment}"
        ),
    )
    .await;
    let entries = listed.as_array().expect("array");
    assert!(entries[0].get("provisional_grades").is_none(), "{listed}");

    cleanup(&pool, &prefix).await;
}
