//! Integration tests for the quiz GraphQL slice (issue #78).
//!
//! Same harness and conventions as `terms_sections_integration.rs`: real
//! Postgres via DATABASE_URL (SKIP-clean without it), unique run prefix,
//! full teardown, raw JSON `{ "query", "variables" }` POSTs with Bearer
//! tokens.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness (copied from tests/terms_sections_integration.rs)
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-qz-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the GraphQL router
/// mounted the same way star-http mounts it. Returns None when DATABASE_URL
/// is unset so callers can skip cleanly.
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

    // Same nesting as star-http main.rs: /api/graphql outside /api/v1.
    let app = axum::Router::new()
        .nest("/api/graphql", star_graphql::router())
        .with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Create a user directly in the DB with the given role. Returns user id.
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

/// Create a course directly in the DB. Returns course id.
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

/// Give a user an active enrollment in a course with the given role.
async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO enrollments (user_id, course_id, role, workflow_state)
        VALUES ($1, $2, $3, 'active')
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed enrollment")
}

/// Create a quiz directly in the DB (migrations 0006 + 0015). Returns id.
async fn seed_quiz(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO quizzes (course_id, title, description, quiz_type,
                             points_possible, published)
        VALUES ($1, $2, 'seed description', 'assignment', 12.50, true)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed quiz")
}

/// Create a quiz submission directly in the DB (migrations 0006 + 0068).
async fn seed_quiz_submission(
    pool: &PgPool,
    quiz_id: i64,
    user_id: i64,
    score: Option<f64>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO quiz_submissions (quiz_id, user_id, attempt, score,
                                      workflow_state)
        VALUES ($1, $2, 1, $3, 'complete')
        RETURNING id
        "#,
    )
    .bind(quiz_id)
    .bind(user_id)
    .bind(score)
    .fetch_one(pool)
    .await
    .expect("seed quiz submission")
}

/// Create a quiz question directly in the DB (migration 0037). Returns id.
async fn seed_quiz_question(pool: &PgPool, quiz_id: i64, position: i32) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO quiz_questions (quiz_id, position, question_name,
                                    question_type, question_text,
                                    points_possible, answers)
        VALUES ($1, $2, 'Question', 'multiple_choice_question',
                'Pick one', 2.50, '[]')
        RETURNING id
        "#,
    )
    .bind(quiz_id)
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed quiz question")
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run: courses cascade quizzes, which
/// cascade quiz_submissions/quiz_questions; enrollments + users by prefix.
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

/// POST a raw JSON GraphQL request `{ query, variables }` with the given
/// bearer token. Returns the full JSON body (data + errors).
async fn gql_post(server: &TestServer, token: &str, query: &str, variables: Value) -> Value {
    let resp = server
        .post("/api/graphql")
        .add_header(axum::http::header::AUTHORIZATION, token)
        .json(&json!({ "query": query, "variables": variables }))
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "graphql post should succeed; body: {}",
        resp.text()
    );
    resp.json()
}

// ---------------------------------------------------------------------------
// Teacher sees quizzesConnection with seeded fields + quiz questions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn teacher_sees_course_quizzes_and_questions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "QuizCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let quiz = seed_quiz(&pool, course, "Quiz One").await;
    let question = seed_quiz_question(&pool, quiz, 1).await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { quizzesConnection { \
         edges { cursor node { id courseId title description quizType \
         pointsPossible published allowedAttempts \
         questionsConnection { edges { node { id quizId position \
         questionType pointsPossible answers workflowState } } } } } \
         pageInfo { hasNextPage endCursor } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let conn = &body["data"]["course"]["quizzesConnection"];
    assert_eq!(
        conn["edges"].as_array().map(Vec::len),
        Some(1),
        "body: {body}"
    );
    let node = &conn["edges"][0]["node"];
    assert_eq!(node["id"], quiz.to_string());
    assert_eq!(node["courseId"], course.to_string());
    assert_eq!(node["title"], "Quiz One");
    assert_eq!(node["description"], "seed description");
    assert_eq!(node["quizType"], "assignment");
    assert_eq!(node["pointsPossible"], 12.5);
    assert_eq!(node["published"], true);
    assert_eq!(node["allowedAttempts"], 1);
    assert!(
        !conn["pageInfo"]["hasNextPage"].as_bool().unwrap(),
        "body: {body}"
    );
    assert!(
        conn["pageInfo"]["endCursor"].as_str().is_some(),
        "endCursor should be present: {body}"
    );
    let q_node = &node["questionsConnection"]["edges"][0]["node"];
    assert_eq!(q_node["id"], question.to_string());
    assert_eq!(q_node["quizId"], quiz.to_string());
    assert_eq!(q_node["questionType"], "multiple_choice_question");
    assert_eq!(q_node["pointsPossible"], 2.5);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Student sees quizzes but NOT other students' quiz submissions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn student_sees_quizzes_but_only_own_submissions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "OtherStudent", "student").await;
    let teacher = seed_user(&pool, &prefix, "QuizTeacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "SubCourse").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other, course, "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let quiz = seed_quiz(&pool, course, "Sub Quiz").await;
    let _own = seed_quiz_submission(&pool, quiz, student, Some(9.0)).await;
    let _theirs = seed_quiz_submission(&pool, quiz, other, Some(7.0)).await;

    // Student sees the quiz connection (content parity with teachers)...
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { course(id: $id) { quizzesConnection { \
         edges { node { title submissionsConnection { \
         edges { node { userId score workflowState } } } } } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let edges = &body["data"]["course"]["quizzesConnection"]["edges"];
    assert_eq!(edges.as_array().map(Vec::len), Some(1), "body: {body}");
    assert_eq!(edges[0]["node"]["title"], "Sub Quiz");

    // ...but the nested submissionsConnection is scoped to self: only the
    // student's own submission, never the other student's.
    let sub_edges = &edges[0]["node"]["submissionsConnection"]["edges"];
    assert_eq!(
        sub_edges.as_array().map(Vec::len),
        Some(1),
        "student must see exactly their own submission: {body}"
    );
    assert_eq!(sub_edges[0]["node"]["userId"], student.to_string());
    assert_eq!(sub_edges[0]["node"]["score"], 9.0);
    assert_eq!(sub_edges[0]["node"]["workflowState"], "complete");

    // The teacher sees both submissions (self-or-teacher-or-admin scope).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { quizzesConnection { \
         edges { node { submissionsConnection { edges { node { userId } } } } } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let sub_edges = &body["data"]["course"]["quizzesConnection"]["edges"][0]["node"]
        ["submissionsConnection"]["edges"];
    assert_eq!(sub_edges.as_array().map(Vec::len), Some(2), "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Pagination: 5 quizzes, first: 2, cursor follow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn quiz_connection_pagination_first_two() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Pager", "student").await;
    let course = seed_course(&pool, &prefix, "PagedCourse").await;
    seed_enrollment(&pool, student, course, "student").await;
    for i in 1..=5 {
        seed_quiz(&pool, course, &format!("Paged Quiz {i}")).await;
    }

    let query = "query ($id: ID!, $first: Int, $after: String) { course(id: $id) \
                 { quizzesConnection(first: $first, after: $after) { \
                 edges { node { title } } \
                 pageInfo { hasNextPage endCursor } } } }";

    let body = gql_post(
        &server,
        &auth(student, "student"),
        query,
        json!({ "id": course.to_string(), "first": 2 }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let conn = &body["data"]["course"]["quizzesConnection"];
    assert_eq!(
        conn["edges"].as_array().map(Vec::len),
        Some(2),
        "body: {body}"
    );
    assert_eq!(conn["edges"][0]["node"]["title"], "Paged Quiz 1");
    assert_eq!(conn["edges"][1]["node"]["title"], "Paged Quiz 2");
    assert_eq!(conn["pageInfo"]["hasNextPage"], true);
    let cursor = conn["pageInfo"]["endCursor"].as_str().expect("endCursor");

    let body = gql_post(
        &server,
        &auth(student, "student"),
        query,
        json!({ "id": course.to_string(), "first": 2, "after": cursor }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let conn = &body["data"]["course"]["quizzesConnection"];
    assert_eq!(
        conn["edges"].as_array().map(Vec::len),
        Some(2),
        "body: {body}"
    );
    assert_eq!(conn["edges"][0]["node"]["title"], "Paged Quiz 3");
    assert_eq!(conn["edges"][1]["node"]["title"], "Paged Quiz 4");
    assert_eq!(conn["pageInfo"]["hasNextPage"], true);
    let cursor = conn["pageInfo"]["endCursor"].as_str().expect("endCursor2");

    let body = gql_post(
        &server,
        &auth(student, "student"),
        query,
        json!({ "id": course.to_string(), "first": 2, "after": cursor }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let conn = &body["data"]["course"]["quizzesConnection"];
    assert_eq!(
        conn["edges"].as_array().map(Vec::len),
        Some(1),
        "body: {body}"
    );
    assert_eq!(conn["edges"][0]["node"]["title"], "Paged Quiz 5");
    assert_eq!(conn["pageInfo"]["hasNextPage"], false);
    assert!(
        conn["pageInfo"]["endCursor"].is_string(),
        "last page still carries an endCursor: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Unenrolled non-admin user: course null (parity gate)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unenrolled_user_gets_null_course() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "HiddenQuizCourse").await;
    seed_quiz(&pool, course, "Hidden Quiz").await;

    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        "query ($id: ID!) { course(id: $id) { quizzesConnection { edges { node { title } } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unauthorized visibility is null, not an error: {body}"
    );
    assert!(
        body["data"]["course"].is_null(),
        "unenrolled non-admin must get a null course: {body}"
    );

    cleanup(&pool, &prefix).await;
}
