//! Integration tests for the Rubric family + `Course.rubricsConnection`
//! and `DiscussionTopic.entriesConnection` (GraphQL parity gap
//! completion). Conventions mirror
//! `assignments_connection_integration.rs`.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-rb-{}-{}",
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

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

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

// ---------------------------------------------------------------------------
// Rubric family: nested criteria/ratings shape
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rubrics_connection_returns_nested_canvas_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "RubricCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let rubric: i64 = sqlx::query_scalar(
        r#"INSERT INTO rubrics (course_id, title, points_possible)
           VALUES ($1, 'Essay Rubric', 10) RETURNING id"#,
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed rubric");
    let crit1: i64 = sqlx::query_scalar(
        r#"INSERT INTO rubric_criteria (rubric_id, description, points, position)
           VALUES ($1, 'Thesis', 5, 1) RETURNING id"#,
    )
    .bind(rubric)
    .fetch_one(&pool)
    .await
    .expect("seed criterion 1");
    let crit2: i64 = sqlx::query_scalar(
        r#"INSERT INTO rubric_criteria (rubric_id, description, points, position)
           VALUES ($1, 'Grammar', 5, 2) RETURNING id"#,
    )
    .bind(rubric)
    .fetch_one(&pool)
    .await
    .expect("seed criterion 2");
    let rating_a: i64 = sqlx::query_scalar(
        r#"INSERT INTO rubric_ratings (criterion_id, description, points, position)
           VALUES ($1, 'Excellent', 5, 1) RETURNING id"#,
    )
    .bind(crit1)
    .fetch_one(&pool)
    .await
    .expect("seed rating a");
    sqlx::query(
        r#"INSERT INTO rubric_ratings (criterion_id, description, points, position)
           VALUES ($1, 'Poor', 1, 2)"#,
    )
    .bind(crit1)
    .execute(&pool)
    .await
    .expect("seed rating b");
    // A deleted rubric: must be filtered.
    sqlx::query(
        r#"INSERT INTO rubrics (course_id, title, points_possible, workflow_state)
           VALUES ($1, 'Deleted', 3, 'deleted')"#,
    )
    .bind(course)
    .execute(&pool)
    .await
    .expect("seed deleted rubric");

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { rubricsConnection(first: 10) { edges { cursor \
         node { id courseId title pointsPossible createdAt updatedAt \
         criteria { id description points position ratings { id description points position } } } \
         } pageInfo { hasNextPage } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    let conn = &body["data"]["course"]["rubricsConnection"];
    let edges = conn["edges"].as_array().expect("edges");
    assert_eq!(edges.len(), 1, "deleted rubric hidden: {body}");
    let node = &edges[0]["node"];
    assert_eq!(node["id"], rubric.to_string(), "{body}");
    assert_eq!(node["courseId"], course.to_string(), "{body}");
    assert_eq!(node["title"], "Essay Rubric", "{body}");
    assert_eq!(node["pointsPossible"], 10.0, "{body}");
    assert!(node["createdAt"].is_string(), "{body}");

    let criteria = node["criteria"].as_array().expect("criteria");
    assert_eq!(criteria.len(), 2, "{body}");
    assert_eq!(criteria[0]["id"], crit1.to_string(), "position order: {body}");
    assert_eq!(criteria[1]["id"], crit2.to_string(), "{body}");
    assert_eq!(criteria[0]["description"], "Thesis", "{body}");
    assert_eq!(criteria[0]["points"], 5.0, "{body}");

    let ratings = criteria[0]["ratings"].as_array().expect("ratings");
    assert_eq!(ratings.len(), 2, "{body}");
    assert_eq!(ratings[0]["id"], rating_a.to_string(), "{body}");
    assert_eq!(ratings[0]["description"], "Excellent", "{body}");
    assert_eq!(ratings[0]["points"], 5.0, "{body}");
    assert_eq!(ratings[1]["description"], "Poor", "{body}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], false, "{body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DiscussionEntry: entriesConnection shape + deleted filtering
// ---------------------------------------------------------------------------

#[tokio::test]
async fn discussion_topic_entries_connection_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "EntryCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let topic: i64 = sqlx::query_scalar(
        r#"INSERT INTO discussion_topics (course_id, title, message)
           VALUES ($1, 'Entries Topic', 'first post') RETURNING id"#,
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed topic");
    let root: i64 = sqlx::query_scalar(
        r#"INSERT INTO discussion_entries (topic_id, user_id, message)
           VALUES ($1, $2, 'root entry') RETURNING id"#,
    )
    .bind(topic)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("seed root entry");
    let reply: i64 = sqlx::query_scalar(
        r#"INSERT INTO discussion_entries (topic_id, parent_id, user_id, message)
           VALUES ($1, $2, $3, 'reply entry') RETURNING id"#,
    )
    .bind(topic)
    .bind(root)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("seed reply");
    sqlx::query(
        r#"INSERT INTO discussion_entries (topic_id, user_id, message, workflow_state)
           VALUES ($1, $2, 'deleted entry', 'deleted')"#,
    )
    .bind(topic)
    .bind(student)
    .execute(&pool)
    .await
    .expect("seed deleted entry");

    let body = gql_post(
        &server,
        &auth(student, "student"),
        "query ($id: ID!) { course(id: $id) { discussionsConnection(first: 10) { edges { node \
         { id title entriesConnection(first: 10) { edges { cursor node { id discussionTopicId \
         parentId userId message createdAt updatedAt } } pageInfo { hasNextPage endCursor } } } \
         } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    let edges = body["data"]["course"]["discussionsConnection"]["edges"]
        .as_array()
        .expect("topic edges");
    assert_eq!(edges.len(), 1, "{body}");
    let topic_node = &edges[0]["node"];
    assert_eq!(topic_node["id"], topic.to_string(), "{body}");

    let conn = &topic_node["entriesConnection"];
    let entry_edges = conn["edges"].as_array().expect("entry edges");
    assert_eq!(entry_edges.len(), 2, "deleted entry hidden: {body}");

    let first = &entry_edges[0]["node"];
    assert_eq!(first["id"], root.to_string(), "id order: {body}");
    assert_eq!(first["discussionTopicId"], topic.to_string(), "{body}");
    assert!(first["parentId"].is_null(), "root has null parent: {body}");
    assert_eq!(first["userId"], teacher.to_string(), "{body}");
    assert_eq!(first["message"], "root entry", "{body}");
    assert!(first["createdAt"].is_string(), "{body}");

    let second = &entry_edges[1]["node"];
    assert_eq!(second["id"], reply.to_string(), "{body}");
    assert_eq!(second["parentId"], root.to_string(), "reply parent: {body}");
    assert_eq!(second["userId"], student.to_string(), "{body}");
    assert_eq!(conn["pageInfo"]["hasNextPage"], false, "{body}");
    assert_eq!(
        conn["pageInfo"]["endCursor"],
        entry_edges[1]["cursor"],
        "{body}"
    );

    cleanup(&pool, &prefix).await;
}
