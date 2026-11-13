//! Integration tests for the Submission type + `Assignment.submissionsConnection`
//! and the DiscussionTopic type + `Course.discussionsConnection` (issue #48).
//!
//! Mirrors the conventions of `graphql_integration.rs` /
//! `assignments_connection_integration.rs`: mounted at `/api/graphql` through
//! axum-test against a real Postgres, raw JSON `{ "query": ..., "variables":
//! ... }` bodies, unique-per-run email prefix with full teardown, SKIP when
//! DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness (copied from tests/assignments_connection_integration.rs)
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-sd-{}-{}",
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

    let app = axum::Router::new()
        .nest("/api/graphql", star_graphql::router())
        .with_state(state);
    let server = TestServer::new(app).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
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

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str, points: f64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, description, points_possible, due_at)
        VALUES ($1, $2, NULL, $3, now() + interval '7 days')
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(points)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

/// Create a submission directly in the DB. Returns submission id. A graded
/// score also stamps `graded_at`, mirroring the REST grade handler.
async fn seed_submission(
    pool: &PgPool,
    assignment_id: i64,
    user_id: i64,
    body: Option<&str>,
    score: Option<f64>,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO submissions (assignment_id, user_id, body, score, submission_type,
                                 workflow_state, graded_at)
        VALUES ($1, $2, $3, $4, 'online_text_entry', $5,
                CASE WHEN $4::float8 IS NOT NULL THEN now() ELSE NULL END)
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(body)
    .bind(score)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

/// Create a discussion topic directly in the DB. Returns topic id.
async fn seed_topic(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    message: Option<&str>,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO discussion_topics (course_id, title, message, discussion_type, workflow_state)
        VALUES ($1, $2, $3, 'threaded', $4)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(message)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed topic")
}

fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Submissions cascade on assignment/user deletion (both cascade from the
/// prefix cleanup); discussion_topics cascade on course deletion. Courses,
/// users and enrollments are cleaned by prefix exactly like the foundation
/// tests.
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

/// Collect the submissionsConnection nodes under the course's first
/// assignment. Panics on top-level GraphQL errors or a null course.
fn submission_nodes(body: &Value) -> Vec<&Value> {
    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let edges = body["data"]["course"]["assignmentsConnection"]["edges"][0]["node"]
        ["submissionsConnection"]["edges"]
        .as_array()
        .unwrap_or_else(|| panic!("submissions edges array missing: {body}"));
    edges.iter().map(|e| &e["node"]).collect()
}

// ---------------------------------------------------------------------------
// Field-name parity: camelCased Canvas shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submissions_connection_returns_canvas_field_names() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student_a = seed_user(&pool, &prefix, "StudentA", "student").await;
    let student_b = seed_user(&pool, &prefix, "StudentB", "student").await;
    let course = seed_course(&pool, &prefix, "SubCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student_a, course, "student").await;
    seed_enrollment(&pool, student_b, course, "student").await;
    let assignment = seed_assignment(&pool, course, "FieldParity", 12.5).await;
    seed_submission(
        &pool,
        assignment,
        student_a,
        Some("My essay"),
        Some(88.5),
        "graded",
    )
    .await;
    seed_submission(&pool, assignment, student_b, None, None, "submitted").await;

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { assignmentsConnection(first: 10) \
         { edges { node { submissionsConnection(first: 10) \
         { edges { node { id assignmentId userId attempt body score state submissionType \
         submittedAt gradedAt excused latePolicyStatus deductedPoints createdAt updatedAt } } \
         pageInfo { hasNextPage endCursor } } } } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let nodes = submission_nodes(&body);
    assert_eq!(nodes.len(), 2, "teacher sees both submissions: {body}");

    let find = |uid: i64| {
        nodes
            .iter()
            .find(|n| n["userId"] == uid.to_string())
            .unwrap_or_else(|| panic!("no submission node for user {uid}: {body}"))
    };
    let graded = find(student_a);
    assert_eq!(graded["assignmentId"], assignment.to_string());
    assert_eq!(graded["attempt"], 1);
    assert_eq!(graded["body"], "My essay");
    assert_eq!(graded["score"], 88.5);
    assert_eq!(graded["state"], "graded");
    assert_eq!(graded["submissionType"], "online_text_entry");
    assert_eq!(graded["excused"], false);
    assert!(
        graded["latePolicyStatus"].is_null(),
        "seeded NULL latePolicyStatus must serialize as null: {body}"
    );
    assert!(
        graded["deductedPoints"].is_null(),
        "seeded NULL deductedPoints must serialize as null: {body}"
    );
    for field in ["id", "submittedAt", "gradedAt", "createdAt", "updatedAt"] {
        assert!(
            graded[field].as_str().is_some_and(|v| !v.is_empty()),
            "{field} should be a non-empty string: {body}"
        );
    }

    let ungraded = find(student_b);
    assert!(ungraded["body"].is_null(), "body: {body}");
    assert!(ungraded["score"].is_null(), "score: {body}");
    assert!(ungraded["gradedAt"].is_null(), "gradedAt: {body}");
    assert_eq!(ungraded["state"], "submitted");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Roster isolation: student sees own, teacher sees all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn student_sees_own_submissions_teacher_sees_all() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student_a = seed_user(&pool, &prefix, "StudentA", "student").await;
    let student_b = seed_user(&pool, &prefix, "StudentB", "student").await;
    let student_c = seed_user(&pool, &prefix, "StudentC", "student").await;
    let course = seed_course(&pool, &prefix, "IsolationCourse").await;
    for uid in [teacher, student_a, student_b, student_c] {
        seed_enrollment(
            &pool,
            uid,
            course,
            if uid == teacher { "teacher" } else { "student" },
        )
        .await;
    }
    let assignment = seed_assignment(&pool, course, "Isolation", 10.0).await;
    seed_submission(
        &pool,
        assignment,
        student_a,
        Some("A work"),
        None,
        "submitted",
    )
    .await;
    seed_submission(
        &pool,
        assignment,
        student_b,
        Some("B work"),
        None,
        "submitted",
    )
    .await;

    let query = "query ($id: ID!) { course(id: $id) { assignmentsConnection(first: 10) \
                 { edges { node { submissionsConnection(first: 10) \
                 { edges { node { userId } } } } } } } }";

    // Student A: exactly their own submission.
    let body = gql_post(
        &server,
        &auth(student_a, "student"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    let nodes = submission_nodes(&body);
    assert_eq!(nodes.len(), 1, "student sees only own: {body}");
    assert_eq!(nodes[0]["userId"], student_a.to_string(), "{body}");

    // Teacher: every submission.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    let nodes = submission_nodes(&body);
    assert_eq!(nodes.len(), 2, "teacher sees all: {body}");

    // Student C (enrolled, not submitted): empty edges, not a null connection.
    let body = gql_post(
        &server,
        &auth(student_c, "student"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    let nodes = submission_nodes(&body);
    assert_eq!(
        nodes.len(),
        0,
        "unsubmitted student gets empty edges: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Authorization negatives: outsider gets a null course, admin sees
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outsider_gets_null_course_admin_sees_submissions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "HiddenSubCourse").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Secret", 1.0).await;
    seed_submission(
        &pool,
        assignment,
        student,
        Some("secret work"),
        None,
        "submitted",
    )
    .await;

    let query = "query ($id: ID!) { course(id: $id) { assignmentsConnection(first: 10) \
                 { edges { node { submissionsConnection(first: 10) \
                 { edges { node { userId } } } } } } } }";

    // Outsider: course itself is null, so the connection is unreachable.
    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unauthorized visibility is null, not an error: {body}"
    );
    assert!(body["data"]["course"].is_null(), "body: {body}");

    // Admin: sees the submission without an enrollment.
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    let nodes = submission_nodes(&body);
    assert_eq!(nodes.len(), 1, "admin sees the submission: {body}");
    assert_eq!(nodes[0]["userId"], student.to_string(), "{body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Cursor pagination across two pages (first/after)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submissions_connection_paginates_across_two_pages() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PagedSubCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, "Paged", 10.0).await;
    // Three students, inserted in id order (ORDER BY id ascending).
    for n in 1..=3 {
        let student = seed_user(&pool, &prefix, &format!("PagedStudent{n}"), "student").await;
        seed_enrollment(&pool, student, course, "student").await;
        seed_submission(
            &pool,
            assignment,
            student,
            Some(&format!("Paged{n}")),
            None,
            "submitted",
        )
        .await;
    }

    let query = "query ($id: ID!, $first: Int, $after: String) \
                 { course(id: $id) { assignmentsConnection(first: 10) { edges { node \
                 { submissionsConnection(first: $first, after: $after) \
                 { edges { cursor node { body } } pageInfo { hasNextPage endCursor } } } } } } }";

    // Page 1: first=2 over 3 submissions -> 2 edges + next.
    let page1 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2 }),
    )
    .await;
    let conn = &page1["data"]["course"]["assignmentsConnection"]["edges"][0]["node"]
        ["submissionsConnection"];
    let edges = conn["edges"].as_array().expect("page1 edges");
    assert_eq!(edges.len(), 2, "page1: {page1}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], true, "page1: {page1}");
    let end_cursor = conn["pageInfo"]["endCursor"]
        .as_str()
        .expect("page1 endCursor")
        .to_string();
    assert_eq!(
        edges[1]["cursor"].as_str().expect("edge cursor"),
        end_cursor,
        "endCursor must equal the last edge's cursor: {page1}"
    );

    // Page 2: after the first page's cursor -> the remaining submission.
    let page2 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2, "after": end_cursor }),
    )
    .await;
    let conn = &page2["data"]["course"]["assignmentsConnection"]["edges"][0]["node"]
        ["submissionsConnection"];
    let edges2 = conn["edges"].as_array().expect("page2 edges");
    assert_eq!(edges2.len(), 1, "page2: {page2}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], false, "page2: {page2}");

    // The two pages together cover every submission exactly once.
    let mut all: Vec<&str> = edges
        .iter()
        .chain(edges2.iter())
        .map(|e| e["node"]["body"].as_str().expect("body"))
        .collect();
    all.sort_unstable();
    assert_eq!(
        all,
        vec!["Paged1", "Paged2", "Paged3"],
        "pages: {page1} {page2}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Field-name parity: camelCased Canvas shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn discussions_connection_returns_canvas_field_names() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "TopicCourse").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_topic(&pool, course, "Welcome", Some("Hello class"), "active").await;
    seed_topic(&pool, course, "Unpublished", None, "unpublished").await;

    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { course(id: $id) { discussionsConnection(first: 10) \
         { edges { node { id contextId contextType title message published locked \
         discussionType createdAt updatedAt } } pageInfo { hasNextPage endCursor } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let edges = body["data"]["course"]["discussionsConnection"]["edges"]
        .as_array()
        .expect("edges array");
    assert_eq!(edges.len(), 2, "student sees both topics: {body}");

    let find = |title: &str| {
        edges
            .iter()
            .find(|e| e["node"]["title"] == title)
            .unwrap_or_else(|| panic!("no topic node titled {title}: {body}"))
    };
    let welcome = &find("Welcome")["node"];
    assert_eq!(welcome["contextId"], course.to_string());
    assert_eq!(welcome["contextType"], "Course");
    assert_eq!(welcome["message"], "Hello class");
    assert_eq!(welcome["published"], true);
    assert_eq!(welcome["locked"], false);
    assert_eq!(welcome["discussionType"], "threaded");
    for field in ["id", "createdAt", "updatedAt"] {
        assert!(
            welcome[field].as_str().is_some_and(|v| !v.is_empty()),
            "{field} should be a non-empty string: {body}"
        );
    }

    let unpublished = &find("Unpublished")["node"];
    assert_eq!(unpublished["published"], false, "body: {body}");
    assert!(unpublished["message"].is_null(), "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Scope parity: deleted excluded, unpublished kept
// ---------------------------------------------------------------------------

#[tokio::test]
async fn discussions_connection_scopes_deleted_keeps_unpublished() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "ScopeTopicCourse").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_topic(&pool, course, "Kept1", Some("one"), "active").await;
    seed_topic(&pool, course, "Kept2", Some("two"), "active").await;
    seed_topic(&pool, course, "Gone", Some("deleted"), "deleted").await;
    seed_topic(&pool, course, "Draft", None, "unpublished").await;

    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { course(id: $id) { discussionsConnection(first: 10) \
         { edges { node { title published } } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    assert!(
        body.get("errors").is_none(),
        "unexpected graphql errors: {body}"
    );
    let edges = body["data"]["course"]["discussionsConnection"]["edges"]
        .as_array()
        .expect("edges array");
    let titles: Vec<&str> = edges
        .iter()
        .map(|e| e["node"]["title"].as_str().expect("title"))
        .collect();
    assert_eq!(
        titles.len(),
        3,
        "deleted topics are excluded, unpublished kept: {body}"
    );
    assert!(!titles.contains(&"Gone"), "{titles:?}");
    assert!(
        titles.contains(&"Kept1") && titles.contains(&"Kept2"),
        "{titles:?}"
    );
    let draft = edges
        .iter()
        .find(|e| e["node"]["title"] == "Draft")
        .expect("unpublished topic present");
    assert_eq!(draft["node"]["published"], false, "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Authorization negatives: outsider gets a null course, admin sees
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outsider_gets_null_course_admin_sees_discussions() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let course = seed_course(&pool, &prefix, "HiddenTopicCourse").await;
    seed_topic(&pool, course, "Secret", Some("hidden"), "active").await;

    let query = "query ($id: ID!) { course(id: $id) { discussionsConnection(first: 10) \
                 { edges { node { title } } } } }";

    // Outsider: course itself is null, so the connection is unreachable.
    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    assert!(
        body.get("errors").is_none(),
        "unauthorized visibility is null, not an error: {body}"
    );
    assert!(body["data"]["course"].is_null(), "body: {body}");

    // Admin: sees the topic without an enrollment.
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        query,
        json!({ "id": course.to_string() }),
    )
    .await;
    let edges = body["data"]["course"]["discussionsConnection"]["edges"]
        .as_array()
        .expect("admin edges");
    assert_eq!(edges.len(), 1, "admin sees the topic: {body}");
    assert_eq!(edges[0]["node"]["title"], "Secret");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Cursor pagination across two pages (first/after)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn discussions_connection_paginates_across_two_pages() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "PagedTopicCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    // Three topics, inserted in id order (ORDER BY id ascending).
    for n in 1..=3 {
        seed_topic(&pool, course, &format!("Paged{n}"), Some("body"), "active").await;
    }

    let query = "query ($id: ID!, $first: Int, $after: String) \
                 { course(id: $id) { discussionsConnection(first: $first, after: $after) \
                 { edges { cursor node { title } } pageInfo { hasNextPage endCursor } } } }";

    // Page 1: first=2 over 3 topics -> 2 edges + next.
    let page1 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2 }),
    )
    .await;
    let conn = &page1["data"]["course"]["discussionsConnection"];
    let edges = conn["edges"].as_array().expect("page1 edges");
    assert_eq!(edges.len(), 2, "page1: {page1}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], true, "page1: {page1}");
    let end_cursor = conn["pageInfo"]["endCursor"]
        .as_str()
        .expect("page1 endCursor")
        .to_string();
    assert_eq!(
        edges[1]["cursor"].as_str().expect("edge cursor"),
        end_cursor,
        "endCursor must equal the last edge's cursor: {page1}"
    );

    // Page 2: after the first page's cursor -> the remaining topic.
    let page2 = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "id": course.to_string(), "first": 2, "after": end_cursor }),
    )
    .await;
    let conn = &page2["data"]["course"]["discussionsConnection"];
    let edges2 = conn["edges"].as_array().expect("page2 edges");
    assert_eq!(edges2.len(), 1, "page2: {page2}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], false, "page2: {page2}");

    // The two pages together cover every topic exactly once.
    let mut all: Vec<&str> = edges
        .iter()
        .chain(edges2.iter())
        .map(|e| e["node"]["title"].as_str().expect("title"))
        .collect();
    all.sort_unstable();
    assert_eq!(
        all,
        vec!["Paged1", "Paged2", "Paged3"],
        "pages: {page1} {page2}"
    );

    cleanup(&pool, &prefix).await;
}
