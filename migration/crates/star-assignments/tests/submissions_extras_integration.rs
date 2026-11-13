//! Integration tests for the Canvas submissions API extras in star-assignments
//! (Phase 5): single-submission includes, submission_summary, read/unread,
//! and update_grades.
//!
//! Skipped cleanly when DATABASE_URL is not set, mirroring
//! submissions_api_integration.rs.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "subx-tests-{}-{}",
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
    // Populate sortable_name the way /auth/register computes it ("Last,
    // First"): star-users decodes it as a non-Option String, and NULL rows
    // leak into the shared test database break its GET /users.
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    sqlx::query_scalar(
        "INSERT INTO users (name, sortable_name, email, password_hash, role) VALUES ($1, $2, $3, 'x', $4) RETURNING id",
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
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, $3, 'active')")
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

async fn get_json(server: &TestServer, token: &str, path: &str) -> Value {
    let resp = server
        .get(path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str, points: f64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(points)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let user_ids = "SELECT id FROM users WHERE email LIKE $1";
    let course_ids = "SELECT id FROM courses WHERE course_code LIKE $1";
    let _ = sqlx::query(&format!(
        "DELETE FROM submission_comments WHERE submission_id IN (SELECT id FROM submissions WHERE user_id IN ({user_ids}) OR assignment_id IN (SELECT id FROM assignments WHERE course_id IN ({course_ids})))"
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
        "DELETE FROM rubric_assessments WHERE course_id IN ({course_ids})",
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM rubrics WHERE course_id IN ({course_ids})",
    ))
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
// GET .../submissions/{userId} include[] support
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_submission_includes_history_comments_rubric_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let course = seed_course(&pool, &prefix, "Includes").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", 10.0).await;

    let submission: i64 = sqlx::query_scalar(
        "INSERT INTO submissions (assignment_id, user_id, body, submission_type, workflow_state) \
         VALUES ($1, $2, 'alice answer', 'online_text_entry', 'submitted') RETURNING id",
    )
    .bind(assignment)
    .bind(alice)
    .fetch_one(&pool)
    .await
    .expect("seed submission");

    sqlx::query("INSERT INTO submission_comments (submission_id, author_id, comment) VALUES ($1, $2, 'nice work')")
        .bind(submission)
        .bind(teacher)
        .execute(&pool)
        .await
        .expect("seed comment");

    // rubric assessment with the JSONB criterion array shape.
    let rubric: i64 = sqlx::query_scalar(
        "INSERT INTO rubrics (course_id, title, points_possible) VALUES ($1, 'R', 5.0) RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed rubric");
    sqlx::query(
        r#"INSERT INTO rubric_assessments
             (rubric_id, course_id, submission_id, artifact_id, artifact_type,
              assessment_type, user_id, assessor_id, score, data)
           VALUES ($1, $2, $3, $3, 'Submission', 'grading', $4, $5, 4.0,
                   $6::jsonb)"#,
    )
    .bind(rubric)
    .bind(course)
    .bind(submission)
    .bind(alice)
    .bind(teacher)
    .bind(r#"[{"criterion_id": 101, "rating_id": 7, "points": 4.0, "comments": "good"}]"#)
    .execute(&pool)
    .await
    .expect("seed rubric assessment");

    let path = format!(
        "/courses/{course}/assignments/{assignment}/submissions/{alice}\
         ?include[]=submission_history&include[]=submission_comments\
         &include[]=rubric_assessment&include[]=user"
    );
    let entry = get_json(&server, &auth(teacher, "teacher"), &path).await;

    let history = entry["submission_history"].as_array().expect("history");
    assert_eq!(history.len(), 1, "single attempt: {entry}");
    assert_eq!(history[0]["id"], submission);
    assert_eq!(history[0]["body"], "alice answer");

    let comments = entry["submission_comments"].as_array().expect("comments");
    assert_eq!(comments.len(), 1, "{entry}");
    assert_eq!(comments[0]["comment"], "nice work");
    assert_eq!(comments[0]["author_id"], teacher);
    assert_eq!(comments[0]["author_name"], "Teacher");

    let rubric_assessment = &entry["rubric_assessment"];
    assert_eq!(rubric_assessment["101"]["rating_id"], 7, "{entry}");
    assert_eq!(rubric_assessment["101"]["points"], 4.0);
    assert_eq!(rubric_assessment["101"]["comments"], "good");

    assert_eq!(entry["user"]["id"], alice);
    assert_eq!(entry["user"]["display_name"], "Alice");

    // Baseline fetch without includes has none of the extras.
    let plain = get_json(
        &server,
        &auth(teacher, "teacher"),
        &format!("/courses/{course}/assignments/{assignment}/submissions/{alice}"),
    )
    .await;
    assert!(plain.get("submission_comments").is_none());
    assert!(plain.get("user").is_none());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn read_status_include_marks_submission_read() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let course = seed_course(&pool, &prefix, "ReadStatus").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", 10.0).await;

    let submission: i64 = sqlx::query_scalar(
        "INSERT INTO submissions (assignment_id, user_id, body, score, grader_id, workflow_state) \
         VALUES ($1, $2, 'graded answer', 8.0, $3, 'graded') RETURNING id",
    )
    .bind(assignment)
    .bind(alice)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("seed submission");

    let path = format!(
        "/courses/{course}/assignments/{assignment}/submissions/{alice}?include[]=read_status"
    );

    // Graded submission is unread for Alice until she reads it.
    let first = get_json(&server, &auth(alice, "student"), &path).await;
    assert_eq!(first["read_state"], "unread", "{first}");

    // Including read_status marked it read: the next read reports read.
    let second = get_json(&server, &auth(alice, "student"), &path).await;
    assert_eq!(second["read_state"], "read", "{second}");

    let marker: (String,) = sqlx::query_as(
        "SELECT read_state FROM submission_reads WHERE submission_id = $1 AND user_id = $2",
    )
    .bind(submission)
    .bind(alice)
    .fetch_one(&pool)
    .await
    .expect("read marker");
    assert_eq!(marker.0, "read");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET .../submission_summary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submission_summary_counts_graded_ungraded_not_submitted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let carol = seed_user(&pool, &prefix, "Carol", "student").await;
    let dave = seed_user(&pool, &prefix, "Dave", "student").await;
    let course = seed_course(&pool, &prefix, "Summary").await;
    for (id, role) in [
        (teacher, "teacher"),
        (alice, "student"),
        (bob, "student"),
        (carol, "student"),
        (dave, "student"),
    ] {
        seed_enrollment(&pool, id, course, role).await;
    }
    let assignment = seed_assignment(&pool, course, "Essay", 10.0).await;

    // Alice: graded (score + graded state).
    // Bob: submitted, no score → needs grading.
    // Carol: excused → graded bucket.
    // Dave: no submission → not_submitted.
    for (user, score, state, excused) in [
        (alice, Some(8.0), "graded", false),
        (bob, None, "submitted", false),
        (carol, None, "submitted", true),
    ] {
        sqlx::query(
            "INSERT INTO submissions (assignment_id, user_id, body, submission_type, score, workflow_state, excused) \
             VALUES ($1, $2, 'x', 'online_text_entry', $3, $4, $5)",
        )
        .bind(assignment)
        .bind(user)
        .bind(score)
        .bind(state)
        .bind(excused)
        .execute(&pool)
        .await
        .expect("seed submission");
    }

    let summary = get_json(
        &server,
        &auth(teacher, "teacher"),
        &format!("/courses/{course}/assignments/{assignment}/submission_summary"),
    )
    .await;
    assert_eq!(summary["graded"], 2, "alice + excused carol: {summary}");
    assert_eq!(summary["ungraded"], 1, "bob needs grading: {summary}");
    assert_eq!(summary["not_submitted"], 1, "dave: {summary}");

    // Teacher-only: students are forbidden.
    let forbidden = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/submission_summary"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT/DELETE .../submissions/{userId}/read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mark_submission_read_and_unread_flip_state() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let course = seed_course(&pool, &prefix, "ReadUnread").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", 10.0).await;

    let submission: i64 = sqlx::query_scalar(
        "INSERT INTO submissions (assignment_id, user_id, body) VALUES ($1, $2, 'a') RETURNING id",
    )
    .bind(assignment)
    .bind(alice)
    .fetch_one(&pool)
    .await
    .expect("seed submission");

    let read_path = format!("/courses/{course}/assignments/{assignment}/submissions/{alice}/read");

    // Owner marks read → 204.
    let read = server
        .put(&read_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .await;
    assert_eq!(read.status_code(), 204, "{}", read.text());

    let (state,): (String,) = sqlx::query_as(
        "SELECT read_state FROM submission_reads WHERE submission_id = $1 AND user_id = $2",
    )
    .bind(submission)
    .bind(alice)
    .fetch_one(&pool)
    .await
    .expect("read marker");
    assert_eq!(state, "read");

    // Teacher can also flip Alice's state to unread → 204.
    let unread = server
        .delete(&read_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(unread.status_code(), 204, "{}", unread.text());

    let (state,): (String,) = sqlx::query_as(
        "SELECT read_state FROM submission_reads WHERE submission_id = $1 AND user_id = $2",
    )
    .bind(submission)
    .bind(alice)
    .fetch_one(&pool)
    .await
    .expect("read marker");
    assert_eq!(state, "unread");

    // Another student cannot touch Alice's marker (403).
    let forbidden = server
        .put(&read_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    // A user with no submission on the assignment is a 404.
    let missing = server
        .put(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{bob}/read"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .await;
    assert_eq!(missing.status_code(), 404, "{}", missing.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST .../submissions/update_grades
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_grades_bulk_applies_scores_synchronously() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let course = seed_course(&pool, &prefix, "BulkGrades").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay", 10.0).await;

    let path = format!("/courses/{course}/assignments/{assignment}/submissions/update_grades");
    let teacher_token = auth(teacher, "teacher");

    // Bulk grade two students; Bob has no submission row yet (lazily created).
    let progress = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({
            "grade_data": {
                alice.to_string(): { "posted_grade": "8.5" },
                bob.to_string(): { "posted_grade": 9 },
            }
        }))
        .await;
    assert_eq!(progress.status_code(), 200, "{}", progress.text());
    let progress: Value = progress.json();
    // Simplified synchronous Progress: already completed when returned.
    assert_eq!(progress["workflow_state"], "completed");
    assert_eq!(progress["completion"], 100);
    assert_eq!(progress["tag"], "submissions_update");

    for (user, score) in [(alice, 8.5), (bob, 9.0)] {
        let (state, got_score): (String, Option<f64>) = sqlx::query_as(
            "SELECT workflow_state, score::float8 FROM submissions \
             WHERE assignment_id = $1 AND user_id = $2",
        )
        .bind(assignment)
        .bind(user)
        .fetch_one(&pool)
        .await
        .expect("graded submission");
        assert_eq!(state, "graded");
        assert_eq!(got_score, Some(score));
    }

    // grade_data is required.
    let missing = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({}))
        .await;
    assert_eq!(missing.status_code(), 400, "{}", missing.text());

    // Non-teachers are forbidden.
    let forbidden = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(alice, "student"))
        .json(&json!({ "grade_data": { alice.to_string(): { "posted_grade": 10 } } }))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    // An unknown user id in grade_data is a 404 (Canvas api_find semantics).
    let unknown = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token)
        .json(&json!({ "grade_data": { "999999": { "posted_grade": 1 } } }))
        .await;
    assert_eq!(unknown.status_code(), 404, "{}", unknown.text());

    cleanup(&pool, &prefix).await;
}
