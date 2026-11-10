//! Phase 3 tests: quiz submissions API (show/update) and quiz reports
//! (index/create) — the Canvas `quiz_submissions_api` / `quiz_reports`
//! endpoints added alongside the existing quiz routes.
//!
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-quizzes
//! - Same unique-prefix + full-teardown convention as quizzes_integration.rs.
//!
//! Behavior pinned as implemented (Canvas parity):
//! - GET .../submissions/{id} wraps the row in `{"quiz_submissions": [...]}`
//!   and emits Canvas fields (attempt, workflow_state, attempts_left,
//!   quiz_points_possible, html_url). Grade managers read any submission;
//!   a student reads only their own.
//! - PUT .../submissions/{id} is teacher-only manual scoring: a
//!   `quiz_submissions` array is required (400 otherwise), the attempt must be
//!   present, match the stored attempt and already be complete. Fudge points
//!   are tallied on top of the graded score and question scores/comments land
//!   in `submission_data`.
//! - GET .../reports returns both Canvas report types (student_analysis,
//!   item_analysis) and persists them in `quiz_statistics`; POST rejects
//!   unknown report types with 400 and reuses a still-current report.

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
        "quiz-sub-reports-{}-{}",
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

    let server = TestServer::new(quizzes_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn quizzes_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_quizzes::router())
        .with_state(state)
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
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
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

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM quiz_submissions \
         WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1) \
            OR quiz_id IN (SELECT id FROM quizzes WHERE course_id IN \
                (SELECT id FROM courses WHERE course_code LIKE $2))",
    )
    .bind(format!("{prefix}%@test.local"))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM quizzes WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
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
// Fixtures / helpers
// ---------------------------------------------------------------------------

/// Teacher + student + course + an unpublished 10-point quiz. Returns
/// (teacher_id, student_id, course_id, quiz_id).
async fn seed_fixture(
    server: &TestServer,
    pool: &PgPool,
    prefix: &str,
    course_name: &str,
) -> (i64, i64, i64, i64) {
    let teacher = seed_user(pool, prefix, "Teacher", "teacher").await;
    let student = seed_user(pool, prefix, "Student", "student").await;
    let course = seed_course(pool, prefix, course_name).await;
    seed_enrollment(pool, teacher, course, "teacher").await;
    seed_enrollment(pool, student, course, "student").await;

    let resp = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "title": "Reported Quiz", "points_possible": 10.0 }))
        .await;
    assert_eq!(resp.status_code(), 201, "create quiz: {}", resp.text());
    let quiz = resp.json::<Value>()["id"].as_i64().expect("quiz id");
    (teacher, student, course, quiz)
}

/// Start the quiz as the student and return the submission id.
async fn start_submission(server: &TestServer, student: i64, quiz_id: i64) -> i64 {
    let resp = server
        .post(&format!("/quizzes/{quiz_id}/start"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 201, "start quiz: {}", resp.text());
    resp.json::<Value>()["id"].as_i64().expect("submission id")
}

/// Mark a submission as a finished attempt 1 with a graded score.
async fn complete_submission(pool: &PgPool, submission_id: i64, score: f64) {
    sqlx::query(
        "UPDATE quiz_submissions \
         SET attempt = 1, workflow_state = 'complete', score = $2, kept_score = $2, \
             finished_at = now(), completed_at = now() \
         WHERE id = $1",
    )
    .bind(submission_id)
    .bind(score)
    .execute(pool)
    .await
    .expect("complete submission");
}

async fn get_ok(server: &TestServer, token: &str, path: &str) -> Value {
    let resp = server
        .get(path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}

// ---------------------------------------------------------------------------
// GET /courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn show_quiz_submission_canvas_shape_and_access_rules() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (teacher, student, course, quiz) = seed_fixture(&server, &pool, &prefix, "ShowSub").await;
    let submission = start_submission(&server, student, quiz).await;

    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    seed_enrollment(&pool, outsider, course, "student").await;

    // Teacher sees any submission, wrapped in Canvas' envelope.
    let body = get_ok(
        &server,
        &auth(teacher, "teacher"),
        &format!("/courses/{course}/quizzes/{quiz}/submissions/{submission}"),
    )
    .await;
    let list = body["quiz_submissions"]
        .as_array()
        .expect("quiz_submissions array");
    assert_eq!(list.len(), 1);
    let entry = &list[0];
    assert_eq!(entry["id"].as_i64(), Some(submission));
    assert_eq!(entry["quiz_id"].as_i64(), Some(quiz));
    assert_eq!(entry["user_id"].as_i64(), Some(student));
    assert_eq!(entry["workflow_state"], "untaken");
    assert_eq!(entry["attempt"].as_i64(), Some(0));
    // allowed_attempts defaults to 1, so the untaken submission has 1 left.
    assert_eq!(entry["attempts_left"].as_i64(), Some(1));
    assert_eq!(entry["quiz_points_possible"].as_f64(), Some(10.0));
    assert_eq!(
        entry["html_url"],
        format!("/courses/{course}/quizzes/{quiz}/submissions/{submission}")
    );
    // Untouched submissions have no score/finished_at keys.
    assert!(entry.get("score").is_none());
    assert!(entry.get("finished_at").is_none());

    // The owner may read their own submission; another student may not.
    let own = get_ok(
        &server,
        &auth(student, "student"),
        &format!("/courses/{course}/quizzes/{quiz}/submissions/{submission}"),
    )
    .await;
    assert_eq!(own["quiz_submissions"][0]["id"].as_i64(), Some(submission));

    let resp = server
        .get(&format!(
            "/courses/{course}/quizzes/{quiz}/submissions/{submission}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unknown submission id (and a submission outside the path quiz) is 404.
    let resp = server
        .get(&format!(
            "/courses/{course}/quizzes/{quiz}/submissions/999999999"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /courses/{courseId}/quizzes/{quizId}/submissions/{submissionId}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn update_quiz_submission_manual_scoring_and_validation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (teacher, student, course, quiz) = seed_fixture(&server, &pool, &prefix, "ScoreSub").await;
    let submission = start_submission(&server, student, quiz).await;
    complete_submission(&pool, submission, 8.0).await;

    let teacher_token = auth(teacher, "teacher");
    let path = format!("/courses/{course}/quizzes/{quiz}/submissions/{submission}");

    // Fudge points stack on the graded score; question scores/comments persist.
    let resp = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({
            "quiz_submissions": [{
                "attempt": 1,
                "fudge_points": 2.5,
                "questions": { "7": { "score": 1.5, "comment": "nice work" } }
            }]
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let entry = &body["quiz_submissions"][0];
    assert_eq!(entry["attempt"].as_i64(), Some(1));
    assert_eq!(entry["fudge_points"].as_f64(), Some(2.5));
    assert_eq!(entry["score"].as_f64(), Some(10.5));
    assert_eq!(entry["kept_score"].as_f64(), Some(10.5));
    assert_eq!(entry["workflow_state"], "complete");
    assert!(entry["finished_at"].is_string());

    let (data, score): (String, f64) =
        sqlx::query_as("SELECT submission_data, score::float8 FROM quiz_submissions WHERE id = $1")
            .bind(submission)
            .fetch_one(&pool)
            .await
            .expect("fetch scored submission");
    let data: Value = serde_json::from_str(&data).expect("submission_data json");
    assert_eq!(data["questions"]["7"]["score"].as_f64(), Some(1.5));
    assert_eq!(data["questions"]["7"]["comment"], "nice work");
    assert!((score - 10.5).abs() < 1e-9);

    // A second fudge replaces the first instead of compounding it.
    let resp = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "quiz_submissions": [{ "attempt": 1, "fudge_points": -1.0 }] }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["quiz_submissions"][0]["score"].as_f64(), Some(7.0));

    // Missing quiz_submissions key / attempt, unknown attempt and an
    // incomplete attempt are all 400.
    let resp = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "fudge_points": 1 }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    let resp = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "quiz_submissions": [{ "fudge_points": 1 }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    let resp = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "quiz_submissions": [{ "attempt": 2, "fudge_points": 1 }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // An untaken submission cannot be scored (Canvas requires a complete one).
    let second = seed_user(&pool, &prefix, "Second", "student").await;
    seed_enrollment(&pool, second, course, "student").await;
    let pending = start_submission(&server, second, quiz).await;
    let resp = server
        .put(&format!(
            "/courses/{course}/quizzes/{quiz}/submissions/{pending}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "quiz_submissions": [{ "attempt": 0, "fudge_points": 1 }] }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Students cannot manually score (403), and unknown ids are 404.
    let resp = server
        .put(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "quiz_submissions": [{ "attempt": 1, "fudge_points": 1 }] }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .put(&format!(
            "/courses/{course}/quizzes/{quiz}/submissions/999999999"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_token)
        .json(&json!({ "quiz_submissions": [{ "attempt": 1, "fudge_points": 1 }] }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET/POST /courses/{courseId}/quizzes/{quizId}/reports
// ---------------------------------------------------------------------------

#[tokio::test]
async fn quiz_reports_index_create_teacher_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (teacher, student, course, quiz) = seed_fixture(&server, &pool, &prefix, "Reports").await;
    let teacher_token = auth(teacher, "teacher");
    let path = format!("/courses/{course}/quizzes/{quiz}/reports");

    // Canvas returns one report per known type; rows are persisted.
    let body = get_ok(&server, &teacher_token, &path).await;
    let reports = body.as_array().expect("report array");
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0]["report_type"], "student_analysis");
    assert_eq!(reports[0]["readable_type"], "Student Analysis");
    assert_eq!(reports[1]["report_type"], "item_analysis");
    assert_eq!(reports[1]["readable_type"], "Item Analysis");
    assert_eq!(reports[0]["quiz_id"].as_i64(), Some(quiz));
    assert_eq!(reports[0]["includes_all_versions"].as_bool(), Some(false));
    // item_analysis always considers all versions.
    assert_eq!(reports[1]["includes_all_versions"].as_bool(), Some(true));
    assert_eq!(reports[0]["generatable"].as_bool(), Some(true));
    assert_eq!(reports[0]["anonymous"].as_bool(), Some(false));
    let report_id = reports[0]["id"].as_i64().expect("persisted report id");
    assert_eq!(
        reports[0]["url"],
        format!("/api/v1/courses/{course}/quizzes/{quiz}/reports/{report_id}")
    );

    let stored: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM quiz_statistics WHERE quiz_id = $1 AND report_type = 'student_analysis'",
    )
    .bind(quiz)
    .fetch_one(&pool)
    .await
    .expect("count reports");
    assert_eq!(stored, 1);

    // includes_all_versions=true selects/creates the all-versions variant.
    let body = get_ok(
        &server,
        &teacher_token,
        &format!("{path}?includes_all_versions=true"),
    )
    .await;
    assert_eq!(
        body[0]["includes_all_versions"].as_bool(),
        Some(true),
        "body: {body}"
    );

    // POST returns a single report object and reuses a current one.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "quiz_report": { "report_type": "item_analysis" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["report_type"], "item_analysis");
    assert_eq!(created["includes_all_versions"].as_bool(), Some(true));
    let created_id = created["id"].as_i64().expect("report id");

    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "quiz_report": { "report_type": "item_analysis" } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["id"].as_i64(),
        Some(created_id),
        "still-current report is reused"
    );

    // Unknown report types are 400; students get 403 on both verbs.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "quiz_report": { "report_type": "bogus" } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    let resp = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "quiz_report": { "report_type": "student_analysis" } }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
