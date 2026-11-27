//! Integration tests for the Canvas quiz submission questions routes.
//!
//! Pinned behavior (ported from
//! `canvas-src/app/controllers/quizzes/quiz_submission_questions_controller.rb`
//! and `canvas-src/spec/apis/v1/quizzes/quiz_submission_questions_api_spec.rb`):
//! - `GET /quiz_submissions/{id}/questions` answers with
//!   `{"quiz_submission_questions": [...]}`; a student sees the censored
//!   question (no `points_possible`, no answer `weight`), a teacher sees the
//!   full question;
//! - the owner of the submission may read it, everybody else gets a 403 and a
//!   missing submission a 404;
//! - `POST /quiz_submissions/{id}/questions` stores the answers in the
//!   submission data (`question_<id>`) and answers with the answered
//!   questions; a wrong answer type is a 400, an unknown answer id a 400, a
//!   wrong attempt a 400 and a wrong validation token a 403;
//! - `PUT .../questions/{id}/flag` and `.../unflag` set and clear the
//!   `question_<id>_marked` flag of the question;
//! - `GET .../questions/{id}/formatted_answer` formats a numerical answer
//!   (4 decimals, or 16 significant digits for precision answers) and answers
//!   422 without the `answer` parameter;
//! - `GET .../questions/{id}` is routed by Canvas but the controller has no
//!   `show` action, so it answers 404.
//!
//! Fleet rule: dedicated users, courses and quizzes per test, every seeded row
//! is cleaned up. Skipped cleanly when DATABASE_URL is unset.
//!
//! The Canvas only quiz settings (`hide_results`, `one_question_at_a_time`,
//! ...) and `quiz_questions.question_data` are added by this test with the
//! same DDL the migration needs, so the suite runs before the migration lands;
//! `ADD COLUMN IF NOT EXISTS` keeps it a no-op afterwards.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

/// The DDL the quiz submission questions routes read.
const EXTRA_DDL: [&str; 2] = [
    "ALTER TABLE quizzes \
     ADD COLUMN IF NOT EXISTS one_question_at_a_time BOOLEAN NOT NULL DEFAULT false, \
     ADD COLUMN IF NOT EXISTS hide_results TEXT, \
     ADD COLUMN IF NOT EXISTS one_time_results BOOLEAN NOT NULL DEFAULT false, \
     ADD COLUMN IF NOT EXISTS require_lockdown_browser BOOLEAN NOT NULL DEFAULT false, \
     ADD COLUMN IF NOT EXISTS cant_go_back BOOLEAN NOT NULL DEFAULT false",
    "ALTER TABLE quiz_questions ADD COLUMN IF NOT EXISTS question_data JSONB",
];

/// The seeded multiple choice question with its two answers.
const MC_ANSWERS: &str =
    r#"[{"id":"1658","text":"A","weight":100},{"id":"1659","text":"B","weight":0}]"#;

/// The seeded fill in the blanks question with its two blanks.
const FIMB_ANSWERS: &str = r#"[{"id":"1","text":"","blank_id":"color","weight":100},{"id":"2","text":"","blank_id":"size","weight":100}]"#;

/// Canvas `AssessmentQuestion.variable_id("color")` — the digest of the blank
/// name, the key the answer of the blank is stored under.
const COLOR_BLANK_ID: &str = "1813d2a7223184cf43e19db6622df40b";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "qsq-tests-{}-{}",
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

    for statement in EXTRA_DDL {
        sqlx::query(statement)
            .execute(&pool)
            .await
            .expect("add the quiz settings columns");
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };

    let server = TestServer::new(
        axum::Router::new()
            .merge(star_quizzes::quiz_submission_questions_api::router())
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
        .bind(format!("{prefix}-{name}"))
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
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

/// A published quiz with one attempt.
async fn seed_quiz(pool: &PgPool, course_id: i64, prefix: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO quizzes (course_id, title, quiz_type, published, allowed_attempts) \
         VALUES ($1, $2, 'assignment', true, 1) RETURNING id",
    )
    .bind(course_id)
    .bind(format!("{prefix}-quiz"))
    .fetch_one(pool)
    .await
    .expect("seed quiz")
}

/// The data of a seeded question.
struct QuestionSeed<'a> {
    position: i32,
    name: &'a str,
    question_type: &'a str,
    points: f64,
    answers: &'a str,
    question_data: Option<Value>,
}

/// A question of a quiz. `question_data` is the Canvas question data hash.
async fn seed_question(pool: &PgPool, quiz_id: i64, seed: QuestionSeed<'_>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO quiz_questions \
           (quiz_id, position, question_name, question_type, question_text, points_possible, answers, question_data) \
         VALUES ($1, $2, $3, $4, $5, $6::numeric, $7, $8) RETURNING id",
    )
    .bind(quiz_id)
    .bind(seed.position)
    .bind(seed.name)
    .bind(seed.question_type)
    .bind(format!("{} text", seed.name))
    .bind(seed.points)
    .bind(seed.answers)
    .bind(seed.question_data)
    .fetch_one(pool)
    .await
    .expect("seed question")
}

/// An untaken submission of a student with a validation token.
async fn seed_submission(
    pool: &PgPool,
    quiz_id: i64,
    user_id: i64,
    attempt: i32,
    workflow_state: &str,
    submission_data: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO quiz_submissions \
           (quiz_id, user_id, attempt, workflow_state, submission_data, validation_token) \
         VALUES ($1, $2, $3, $4, $5, 'quiz-token') RETURNING id",
    )
    .bind(quiz_id)
    .bind(user_id)
    .bind(attempt)
    .bind(workflow_state)
    .bind(submission_data)
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
    for statement in [
        "DELETE FROM quiz_submissions WHERE quiz_id IN (SELECT id FROM quizzes WHERE title LIKE $1)",
        "DELETE FROM quiz_questions WHERE quiz_id IN (SELECT id FROM quizzes WHERE title LIKE $1)",
        "DELETE FROM quizzes WHERE title LIKE $1",
    ] {
        let _ = sqlx::query(statement)
            .bind(format!("{prefix}%"))
            .execute(pool)
            .await;
    }
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN \
                         (SELECT id FROM courses WHERE course_code LIKE $1)",
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

/// The seeded world: a course, a student, another student, a teacher, a quiz
/// with a multiple choice, a numerical, a precision numerical and an essay
/// question and the untaken submission of the student.
struct World {
    student: i64,
    other: i64,
    teacher: i64,
    quiz: i64,
    submission: i64,
    multiple_choice: i64,
    numerical: i64,
    precision_numerical: i64,
    essay: i64,
    fill_in_the_blanks: i64,
}

async fn seed_world(pool: &PgPool, prefix: &str) -> World {
    let student = seed_user(pool, prefix, "Student", "student").await;
    let other = seed_user(pool, prefix, "Other", "student").await;
    let teacher = seed_user(pool, prefix, "Teacher", "teacher").await;

    let course = seed_course(pool, prefix, "Course").await;
    seed_enrollment(pool, student, course, "student").await;
    seed_enrollment(pool, other, course, "student").await;
    seed_enrollment(pool, teacher, course, "teacher").await;

    let quiz = seed_quiz(pool, course, prefix).await;

    // The multiple choice question carries its Canvas question data, the
    // other questions only the columns of `quiz_questions`.
    let multiple_choice = seed_question(
        pool,
        quiz,
        QuestionSeed {
            position: 1,
            name: "Multiple Choice",
            question_type: "multiple_choice_question",
            points: 10.0,
            answers: MC_ANSWERS,
            question_data: Some(
                json!({ "answers": serde_json::from_str::<Value>(MC_ANSWERS).expect("answers") }),
            ),
        },
    )
    .await;
    let numerical = seed_question(
        pool,
        quiz,
        QuestionSeed {
            position: 2,
            name: "Numerical",
            question_type: "numerical_question",
            points: 5.0,
            answers: r#"[{"id":"1","text":"1","numerical_answer_type":"exact_answer"}]"#,
            question_data: None,
        },
    )
    .await;
    let precision_numerical = seed_question(
        pool,
        quiz,
        QuestionSeed {
            position: 3,
            name: "Precision Numerical",
            question_type: "numerical_question",
            points: 5.0,
            answers: r#"[{"id":"1","text":"1","numerical_answer_type":"precision_answer"}]"#,
            question_data: None,
        },
    )
    .await;
    let essay = seed_question(
        pool,
        quiz,
        QuestionSeed {
            position: 4,
            name: "Essay",
            question_type: "essay_question",
            points: 5.0,
            answers: "[]",
            question_data: None,
        },
    )
    .await;
    let fill_in_the_blanks = seed_question(
        pool,
        quiz,
        QuestionSeed {
            position: 5,
            name: "Fill In The Blanks",
            question_type: "fill_in_multiple_blanks_question",
            points: 5.0,
            answers: FIMB_ANSWERS,
            question_data: None,
        },
    )
    .await;

    let submission = seed_submission(pool, quiz, student, 1, "untaken", "{}").await;

    World {
        student,
        other,
        teacher,
        quiz,
        submission,
        multiple_choice,
        numerical,
        precision_numerical,
        essay,
        fill_in_the_blanks,
    }
}

#[tokio::test]
async fn the_student_reads_the_questions_of_their_own_submission() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let world = seed_world(&pool, &prefix).await;

    let resp = server
        .get(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let questions = body["quiz_submission_questions"]
        .as_array()
        .expect("questions");
    assert_eq!(questions.len(), 5, "body: {body}");

    // The multiple choice question is censored for a student: no points and
    // no answer weights.
    let first = &questions[0];
    assert_eq!(first["id"], world.multiple_choice);
    assert_eq!(first["question_type"], "multiple_choice_question");
    assert_eq!(first["quiz_id"], world.quiz);
    assert_eq!(first["flagged"], false);
    assert_eq!(first["answer"], Value::Null);
    assert_eq!(first["points_possible"], Value::Null);
    assert_eq!(first["answers"].as_array().expect("answers").len(), 2);
    assert_eq!(first["answers"][0]["id"], "1658");
    assert_eq!(first["answers"][0]["text"], "A");
    assert_eq!(first["answers"][0]["weight"], Value::Null);

    // A question without answers loses the answers of the censored question.
    assert_eq!(questions[1]["answers"], Value::Null);
    assert_eq!(questions[1]["question_name"], "Numerical");

    // `include[]=quiz_question` adds the questions and the meta collection.
    let resp = server
        .get(&format!(
            "/quiz_submissions/{}/questions?include[]=quiz_question",
            world.submission
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    // The world fixture seeds five questions (multiple choice, numerical,
    // precision numerical, essay and fill-in-the-blanks), so the
    // include[]=quiz_question side collection carries all five.
    assert_eq!(
        body["quiz_questions"].as_array().expect("questions").len(),
        5
    );
    assert_eq!(
        body["meta"]["primaryCollection"],
        "quiz_submission_questions"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_teacher_reads_the_questions_uncensored() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let world = seed_world(&pool, &prefix).await;

    let resp = server
        .get(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let questions = body["quiz_submission_questions"]
        .as_array()
        .expect("questions");
    assert_eq!(questions[0]["points_possible"], 10.0);
    assert_eq!(questions[0]["answers"][0]["weight"], 100.0);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn another_student_may_not_read_the_submission() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let world = seed_world(&pool, &prefix).await;

    let resp = server
        .get(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.other, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["status"], "unauthorized");

    // A missing submission is a 404.
    let resp = server
        .get("/quiz_submissions/999999999/questions")
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn answering_a_question_stores_the_answer() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let world = seed_world(&pool, &prefix).await;

    let resp = server
        .post(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({
            "attempt": 1,
            "validation_token": "quiz-token",
            "quiz_questions": [{ "id": world.multiple_choice, "answer": "1658" }],
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let questions = body["quiz_submission_questions"]
        .as_array()
        .expect("questions");
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["id"], world.multiple_choice);
    assert_eq!(questions[0]["answer"], "1658");

    // The answer is stored in the submission data and read back.
    let stored: String =
        sqlx::query_scalar("SELECT submission_data FROM quiz_submissions WHERE id = $1")
            .bind(world.submission)
            .fetch_one(&pool)
            .await
            .expect("submission data");
    assert!(
        stored.contains(&format!("\"question_{}\":\"1658\"", world.multiple_choice)),
        "submission data: {stored}"
    );

    let resp = server
        .get(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["quiz_submission_questions"][0]["answer"], "1658");

    // An essay answer is sanitized and stored as text.
    let resp = server
        .post(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({
            "attempt": 1,
            "validation_token": "quiz-token",
            "quiz_questions": [{ "id": world.essay, "answer": "Foobar" }],
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["quiz_submission_questions"][0]["answer"], "Foobar");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn answering_a_blank_is_stored_under_the_canvas_digest_key() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let world = seed_world(&pool, &prefix).await;

    let resp = server
        .post(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({
            "attempt": 1,
            "validation_token": "quiz-token",
            "quiz_questions": [{
                "id": world.fill_in_the_blanks,
                "answer": { "color": "Red" },
            }],
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    // The answer of the blank is read back by the name of the blank.
    assert_eq!(
        body["quiz_submission_questions"][0]["answer"]["color"],
        "red"
    );
    assert_eq!(
        body["quiz_submission_questions"][0]["answer"]["size"],
        Value::Null
    );

    // Canvas stores it under the digest of the blank name.
    let stored: String =
        sqlx::query_scalar("SELECT submission_data FROM quiz_submissions WHERE id = $1")
            .bind(world.submission)
            .fetch_one(&pool)
            .await
            .expect("submission data");
    assert!(
        stored.contains(&format!(
            "\"question_{}_{}\":\"red\"",
            world.fill_in_the_blanks, COLOR_BLANK_ID
        )),
        "submission data: {stored}"
    );

    // An unknown blank is a 400.
    let resp = server
        .post(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({
            "attempt": 1,
            "validation_token": "quiz-token",
            "quiz_questions": [{
                "id": world.fill_in_the_blanks,
                "answer": { "material": "wood" },
            }],
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "Unknown blank 'material'");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn answering_with_a_wrong_answer_is_a_400() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let world = seed_world(&pool, &prefix).await;

    // A wrong type.
    let resp = server
        .post(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({
            "attempt": 1,
            "validation_token": "quiz-token",
            "quiz_questions": [{ "id": world.multiple_choice, "answer": "not a number" }],
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "answer must be of type Integer");
    assert_eq!(body["status"], "bad_request");

    // An unknown answer id.
    let resp = server
        .post(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({
            "attempt": 1,
            "validation_token": "quiz-token",
            "quiz_questions": [{ "id": world.multiple_choice, "answer": "9999" }],
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "Unknown answer '9999'");

    // A wrong attempt.
    let resp = server
        .post(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({
            "attempt": 2,
            "validation_token": "quiz-token",
            "quiz_questions": [{ "id": world.multiple_choice, "answer": "1658" }],
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "attempt 2 can not be modified");

    // A wrong validation token.
    let resp = server
        .post(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({
            "attempt": 1,
            "validation_token": "wrong",
            "quiz_questions": [{ "id": world.multiple_choice, "answer": "1658" }],
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "invalid token");

    // Another student may not answer.
    let resp = server
        .post(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.other, "student"),
        )
        .json(&json!({
            "attempt": 1,
            "validation_token": "quiz-token",
            "quiz_questions": [{ "id": world.multiple_choice, "answer": "1658" }],
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn flagging_and_unflagging_a_question() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let world = seed_world(&pool, &prefix).await;

    let resp = server
        .put(&format!(
            "/quiz_submissions/{}/questions/{}/flag",
            world.submission, world.multiple_choice
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({ "attempt": 1, "validation_token": "quiz-token" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let questions = body["quiz_submission_questions"]
        .as_array()
        .expect("questions");
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0]["id"], world.multiple_choice);
    assert_eq!(questions[0]["flagged"], true);

    let resp = server
        .get(&format!("/quiz_submissions/{}/questions", world.submission))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["quiz_submission_questions"][0]["flagged"], true);

    let resp = server
        .put(&format!(
            "/quiz_submissions/{}/questions/{}/unflag",
            world.submission, world.multiple_choice
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({ "attempt": 1, "validation_token": "quiz-token" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["quiz_submission_questions"][0]["flagged"], false);

    // Another student may not flag.
    let resp = server
        .put(&format!(
            "/quiz_submissions/{}/questions/{}/flag",
            world.submission, world.multiple_choice
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.other, "student"),
        )
        .json(&json!({ "attempt": 1, "validation_token": "quiz-token" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // A question of another quiz is a 404.
    let resp = server
        .put(&format!(
            "/quiz_submissions/{}/questions/999999999/flag",
            world.submission
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .json(&json!({ "attempt": 1, "validation_token": "quiz-token" }))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_formatted_answer_of_a_numerical_question() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let world = seed_world(&pool, &prefix).await;

    // A numerical question without precision answers is rounded to four
    // decimals.
    let resp = server
        .get(&format!(
            "/quiz_submissions/{}/questions/{}/formatted_answer?answer=12.34567890123456789",
            world.submission, world.numerical
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["formatted_answer"], "12.3457");

    // A precision answer keeps sixteen significant digits.
    let resp = server
        .get(&format!(
            "/quiz_submissions/{}/questions/{}/formatted_answer?answer=12.34567890123456789",
            world.submission, world.precision_numerical
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["formatted_answer"], "12.34567890123457");

    // Insignificant zeros are stripped.
    let resp = server
        .get(&format!(
            "/quiz_submissions/{}/questions/{}/formatted_answer?answer=99.9000000",
            world.submission, world.numerical
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["formatted_answer"], "99.9");

    // A value that is not a number is returned unchanged.
    let resp = server
        .get(&format!(
            "/quiz_submissions/{}/questions/{}/formatted_answer?answer=abcd",
            world.submission, world.numerical
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["formatted_answer"], "abcd");

    // The answer parameter is required.
    let resp = server
        .get(&format!(
            "/quiz_submissions/{}/questions/{}/formatted_answer",
            world.submission, world.numerical
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "answer param is required");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_show_route_is_not_implemented() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let world = seed_world(&pool, &prefix).await;

    // Canvas routes this path but the controller has no `show` action, so
    // Rails answers `ActionNotFound`.
    let resp = server
        .get(&format!(
            "/quiz_submissions/{}/questions/{}",
            world.submission, world.multiple_choice
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(world.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["errors"][0]["error_code"], "not_found");

    // Rails raises the `ActionNotFound` before the login filter, so the
    // answer does not depend on the caller.
    let resp = server
        .get(&format!(
            "/quiz_submissions/{}/questions/{}",
            world.submission, world.multiple_choice
        ))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
