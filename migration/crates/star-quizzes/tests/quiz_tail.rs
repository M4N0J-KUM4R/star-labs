//! Slice G tail (#54): quiz submission `complete` + `time`, quiz report
//! `show` + `abort`, and quiz group `reorder`.
//!
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-quizzes
//! - Same unique-prefix + full-teardown convention as the other suites.
//!
//! Behavior pinned as implemented (Canvas parity, with the deviations the
//! handlers document):
//! - POST .../submissions/{id}/complete is owner-only (403 otherwise). The
//!   attempt must match the stored attempt (400 "invalid attempt" for stale
//!   or missing values) and the Canvas `validation_token` issued at take time
//!   must be echoed (403 otherwise). Completing twice is 400.
//! - GET .../submissions/{id}/time answers the owner or a grade manager and
//!   returns `{end_at, time_left}`; unlimited quizzes report `null`/`null`,
//!   finished attempts report their frozen clock with time_left 0.
//! - GET .../reports/{id} adds `progress` (100 once synchronously generated)
//!   and a null `file_url` (binary attachments are pending MIG-04, #45).
//! - DELETE .../reports/{id} destroys completed/queued reports (204), answers
//!   422 "report generation can not be aborted" for running ones and 422
//!   "report is not being generated" for failed ones.
//! - POST .../groups/{id}/reorder takes `order` of question ids: duplicates
//!   are 400, foreign ids are 404, ordered questions get positions 1..n and
//!   left-out group questions keep their relative order afterwards. 204.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Harness (mirrors quiz_submissions_reports.rs)
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "quiz-tail-{}-{}",
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
        "DELETE FROM quiz_statistics \
         WHERE quiz_id IN (SELECT id FROM quizzes WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM quiz_submissions \
         WHERE quiz_id IN (SELECT id FROM quizzes WHERE course_id IN \
             (SELECT id FROM courses WHERE course_code LIKE $1))",
    )
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

/// Teacher + student + course + quiz. `payload` goes into POST /quizzes.
async fn seed_fixture(
    server: &TestServer,
    pool: &PgPool,
    prefix: &str,
    course_name: &str,
    quiz_payload: Value,
) -> (i64, i64, i64, i64) {
    let teacher = seed_user(pool, prefix, "Teacher", "teacher").await;
    let student = seed_user(pool, prefix, "Student", "student").await;
    let course = seed_course(pool, prefix, course_name).await;
    seed_enrollment(pool, teacher, course, "teacher").await;
    seed_enrollment(pool, student, course, "student").await;

    let resp = server
        .post(&format!("/courses/{course}/quizzes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&quiz_payload)
        .await;
    assert_eq!(resp.status_code(), 201, "create quiz: {}", resp.text());
    let quiz = resp.json::<Value>()["id"].as_i64().expect("quiz id");
    (teacher, student, course, quiz)
}

/// Start the quiz as the student and return (submission id, validation token).
async fn take_quiz(
    server: &TestServer,
    pool: &PgPool,
    student: i64,
    quiz_id: i64,
) -> (i64, String) {
    let resp = server
        .post(&format!("/quizzes/{quiz_id}/start"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 201, "start quiz: {}", resp.text());
    let submission = resp.json::<Value>()["id"].as_i64().expect("submission id");
    let token: String =
        sqlx::query_scalar("SELECT validation_token FROM quiz_submissions WHERE id = $1")
            .bind(submission)
            .fetch_one(pool)
            .await
            .expect("take issues a validation token");
    assert_eq!(
        token.len(),
        20,
        "Canvas token is SecureRandom.hex(10): {token}"
    );
    (submission, token)
}

async fn complete_ok(server: &TestServer, student: i64, path: &str, body: &Value) -> Value {
    let resp = server
        .post(path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(body)
        .await;
    assert_eq!(resp.status_code(), 200, "complete: {}", resp.text());
    resp.json()
}

// ---------------------------------------------------------------------------
// POST .../submissions/{id}/complete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn complete_enforces_attempt_then_token_and_flips_state() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (_teacher, student, course, quiz) = seed_fixture(
        &server,
        &pool,
        &prefix,
        "CmpQ",
        json!({"title": "Done Quiz"}),
    )
    .await;
    let (submission, token) = take_quiz(&server, &pool, student, quiz).await;
    let path = format!("/courses/{course}/quizzes/{quiz}/submissions/{submission}/complete");

    // Stale/missing attempt is 400 "invalid attempt" — checked before token.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({"attempt": 1, "validation_token": token}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert!(
        resp.text().contains("invalid attempt"),
        "Canvas names this error: {}",
        resp.text()
    );

    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({"validation_token": token}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // The stored attempt is 0 (Star take flow); wrong or missing token is 403.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({"attempt": 0, "validation_token": "not-the-token"}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({"attempt": 0}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Canvas also accepts the quiz_submissions[0]{...} wrapper.
    let body = complete_ok(
        &server,
        student,
        &path,
        &json!({"quiz_submissions": [{"attempt": 0, "validation_token": token}]}),
    )
    .await;
    let entry = &body["quiz_submissions"][0];
    assert_eq!(entry["id"].as_i64(), Some(submission));
    assert_eq!(entry["workflow_state"], "complete");
    assert!(
        entry.get("finished_at").is_some(),
        "turn-in stamps finished_at"
    );
    assert!(
        entry["time_spent"].as_i64().unwrap_or(-1) >= 0,
        "time_spent is stamped: {entry}"
    );

    // Completing again is 400 ("already completed"), regardless of token.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({"attempt": 0, "validation_token": token}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn complete_is_owner_only_and_scoped_to_the_path_quiz() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (teacher, student, course, quiz) = seed_fixture(
        &server,
        &pool,
        &prefix,
        "CmpOwn",
        json!({"title": "Owner Quiz"}),
    )
    .await;
    let (submission, token) = take_quiz(&server, &pool, student, quiz).await;
    let path = format!("/courses/{course}/quizzes/{quiz}/submissions/{submission}/complete");

    // Canvas `quiz_submission_allowed?` limits complete to the taking student.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({"attempt": 0, "validation_token": token}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A submission outside the path quiz is 404 (nested find).
    let other = seed_fixture(
        &server,
        &pool,
        &prefix,
        "CmpOther",
        json!({"title": "Other"}),
    )
    .await;
    let resp = server
        .post(&format!(
            "/courses/{course}/quizzes/{}/submissions/{submission}/complete",
            other.3
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({"attempt": 0, "validation_token": token}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET .../submissions/{id}/time
// ---------------------------------------------------------------------------

#[tokio::test]
async fn time_reports_clock_for_timed_and_unlimited_quizzes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // 30-minute limit (Canvas stores time_limit in seconds).
    let (teacher, student, course, quiz) = seed_fixture(
        &server,
        &pool,
        &prefix,
        "TimeQ",
        json!({"title": "Timed Quiz", "time_limit": 1800}),
    )
    .await;
    let (submission, _token) = take_quiz(&server, &pool, student, quiz).await;
    let path = format!("/courses/{course}/quizzes/{quiz}/submissions/{submission}/time");

    let body = get_ok(&server, &auth(student, "student"), &path).await;
    assert!(
        body["end_at"].is_string(),
        "timed quiz reports an end_at: {body}"
    );
    let left = body["time_left"].as_i64().expect("time_left");
    assert!(
        (1600..=1800).contains(&left),
        "time_left should be ~1800s: {left}"
    );

    // Grade managers may record events too; strangers may not.
    let body = get_ok(&server, &auth(teacher, "teacher"), &path).await;
    assert!(body["end_at"].is_string());
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let resp = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unlimited quiz: null clock (Canvas emits nulls, not zeros).
    let (_t2, student2, course2, quiz2) = seed_fixture(
        &server,
        &pool,
        &prefix,
        "NoLimit",
        json!({"title": "Untimed"}),
    )
    .await;
    let (sub2, _tok2) = take_quiz(&server, &pool, student2, quiz2).await;
    let body = get_ok(
        &server,
        &auth(student2, "student"),
        &format!("/courses/{course2}/quizzes/{quiz2}/submissions/{sub2}/time"),
    )
    .await;
    assert!(body["end_at"].is_null());
    assert!(body["time_left"].is_null());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn time_freezes_after_completion() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (_teacher, student, course, quiz) = seed_fixture(
        &server,
        &pool,
        &prefix,
        "TimeDone",
        json!({"title": "Freeze Quiz", "time_limit": 600}),
    )
    .await;
    let (submission, token) = take_quiz(&server, &pool, student, quiz).await;
    let base = format!("/courses/{course}/quizzes/{quiz}/submissions/{submission}");

    complete_ok(
        &server,
        student,
        &format!("{base}/complete"),
        &json!({"attempt": 0, "validation_token": token}),
    )
    .await;

    let body = get_ok(&server, &auth(student, "student"), &format!("{base}/time")).await;
    assert_eq!(body["time_left"].as_i64(), Some(0));
    assert!(
        body["end_at"].is_string(),
        "end_at frozen by complete: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET/DELETE .../reports/{id}
// ---------------------------------------------------------------------------

async fn create_report(server: &TestServer, teacher: i64, course: i64, quiz: i64) -> i64 {
    let resp = server
        .post(&format!("/courses/{course}/quizzes/{quiz}/reports"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({"quiz_report": {"report_type": "student_analysis"}}))
        .await;
    assert_eq!(resp.status_code(), 200, "create report: {}", resp.text());
    resp.json::<Value>()["id"].as_i64().expect("report id")
}

#[tokio::test]
async fn report_show_exposes_progress_and_abort_destroys() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (teacher, student, course, quiz) = seed_fixture(
        &server,
        &pool,
        &prefix,
        "RptQ",
        json!({"title": "Report Quiz"}),
    )
    .await;
    let report = create_report(&server, teacher, course, quiz).await;
    let path = format!("/courses/{course}/quizzes/{quiz}/reports/{report}");

    let body = get_ok(&server, &auth(teacher, "teacher"), &path).await;
    assert_eq!(body["id"].as_i64(), Some(report));
    assert_eq!(body["report_type"], "student_analysis");
    assert_eq!(body["progress"].as_i64(), Some(100));
    assert!(body["file_url"].is_null(), "CSV link lands with MIG-04");

    // Students get 403 (same rule as index/create); a report outside the path
    // quiz is 404.
    let resp = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let other = seed_fixture(&server, &pool, &prefix, "RptOther", json!({"title": "O"})).await;
    let resp = server
        .get(&format!(
            "/courses/{course}/quizzes/{}/reports/{report}",
            other.3
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Abort destroys a completed (synchronously generated) report.
    let resp = server
        .delete(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 204, "abort: {}", resp.text());
    let resp = server
        .get(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn abort_distinguishes_running_and_failed_reports() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (teacher, _student, course, quiz) = seed_fixture(
        &server,
        &pool,
        &prefix,
        "RptSt",
        json!({"title": "States Quiz"}),
    )
    .await;

    for (state_name, expect_status, expect_text) in [
        ("running", 422, "report generation can not be aborted"),
        ("failed", 422, "report is not being generated"),
        ("queued", 204, ""),
    ] {
        let report: i64 = sqlx::query_scalar(
            "INSERT INTO quiz_statistics \
                 (quiz_id, report_type, includes_all_versions, includes_sis_ids, \
                  anonymous, generation_state) \
             VALUES ($1, 'item_analysis', true, false, false, $2) RETURNING id",
        )
        .bind(quiz)
        .bind(state_name)
        .fetch_one(&pool)
        .await
        .expect("seed report row");

        let resp = server
            .delete(&format!(
                "/courses/{course}/quizzes/{quiz}/reports/{report}"
            ))
            .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
            .await;
        assert_eq!(
            resp.status_code(),
            expect_status,
            "{state_name}: {}",
            resp.text()
        );
        if !expect_text.is_empty() {
            assert!(resp.text().contains(expect_text), "body: {}", resp.text());
            // A refused abort keeps the report.
            let body = get_ok(
                &server,
                &auth(teacher, "teacher"),
                &format!("/courses/{course}/quizzes/{quiz}/reports/{report}"),
            )
            .await;
            assert_eq!(body["id"].as_i64(), Some(report));
        }
    }

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST .../groups/{id}/reorder
// ---------------------------------------------------------------------------

async fn seed_group_with_questions(
    _server: &TestServer,
    pool: &PgPool,
    teacher: i64,
    _course: i64,
    quiz: i64,
) -> (i64, Vec<i64>) {
    let group: i64 = sqlx::query_scalar(
        "INSERT INTO quiz_question_groups (quiz_id, name, pick_count, position) \
         VALUES ($1, 'Tail Group', 0, 1) RETURNING id",
    )
    .bind(quiz)
    .fetch_one(pool)
    .await
    .expect("seed group");

    let mut questions = Vec::new();
    for idx in 1..=3 {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO quiz_questions \
                 (quiz_id, quiz_group_id, position, question_name, question_text) \
             VALUES ($1, $2, $3, $4, 'Body') RETURNING id",
        )
        .bind(quiz)
        .bind(group)
        .bind(idx)
        .bind(format!("Q{idx}"))
        .fetch_one(pool)
        .await
        .expect("seed question");
        questions.push(id);
    }
    let _ = teacher;
    (group, questions)
}

async fn question_order(server: &TestServer, teacher: i64, course: i64, quiz: i64) -> Vec<i64> {
    let body = get_ok(
        &server,
        &auth(teacher, "teacher"),
        &format!("/courses/{course}/quizzes/{quiz}/questions"),
    )
    .await;
    body.as_array()
        .expect("questions array")
        .iter()
        .map(|q| q["id"].as_i64().expect("question id"))
        .collect()
}

#[tokio::test]
async fn group_reorder_sets_positions_and_appends_the_rest() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (teacher, _student, course, quiz) = seed_fixture(
        &server,
        &pool,
        &prefix,
        "RdrQ",
        json!({"title": "Reorder Quiz"}),
    )
    .await;
    let seeded = seed_group_with_questions(&server, &pool, teacher, course, quiz).await;
    let group = seeded.0;
    let questions = seeded.1;

    let path = format!("/courses/{course}/quizzes/{quiz}/groups/{group}/reorder");
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({"order": [questions[2], questions[0]]}))
        .await;
    assert_eq!(resp.status_code(), 204, "reorder: {}", resp.text());

    // third first, first second, the untouched second keeps its relative place.
    let order = question_order(&server, teacher, course, quiz).await;
    assert_eq!(order, vec![questions[2], questions[0], questions[1]]);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn group_reorder_validation_matrix() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let (teacher, student, course, quiz) = seed_fixture(
        &server,
        &pool,
        &prefix,
        "RdrV",
        json!({"title": "Rereorder"}),
    )
    .await;
    let seeded = seed_group_with_questions(&server, &pool, teacher, course, quiz).await;
    let group = seeded.0;
    let questions = seeded.1;
    let path = format!("/courses/{course}/quizzes/{quiz}/groups/{group}/reorder");
    let teacher_auth = auth(teacher, "teacher");

    // Requires quiz :update — students are refused.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({"order": questions}))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Missing order array is 400.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_auth.clone())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Duplicates are 400.
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_auth.clone())
        .json(&json!({"order": [questions[0], questions[0]]}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Foreign ids are 404 (Canvas association scope raises RecordNotFound).
    let (_t2, _s2, course2, quiz2) =
        seed_fixture(&server, &pool, &prefix, "RdrF", json!({"title": "F"})).await;
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_auth.clone())
        .json(&json!({"order": [questions[0], 999999999]}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // A group outside the path quiz is 404 too.
    let resp = server
        .post(&format!(
            "/courses/{course}/quizzes/{quiz2}/groups/{group}/reorder"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_auth.clone())
        .json(&json!({"order": [questions[0]]}))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let _ = course2;

    // Full order works, including numeric strings (Canvas form payloads).
    let resp = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_auth)
        .json(&json!({"order": [questions[2].to_string(), questions[0], questions[1]]}))
        .await;
    assert_eq!(resp.status_code(), 204, "body: {}", resp.text());
    let order = question_order(&server, teacher, course, quiz).await;
    assert_eq!(order, vec![questions[2], questions[0], questions[1]]);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// shared assertion helper
// ---------------------------------------------------------------------------

async fn get_ok(server: &TestServer, token: &str, path: &str) -> Value {
    let resp = server
        .get(path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}
