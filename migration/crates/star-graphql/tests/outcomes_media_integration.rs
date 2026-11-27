//! Integration tests for the LearningOutcome family, MediaObject and
//! MediaTrack (GraphQL parity gap completion). Conventions mirror
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
        "gql-om-{}-{}",
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
        .bind(name)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active') RETURNING id",
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

#[tokio::test]
async fn outcomes_and_groups_nested_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "OutCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let group: i64 = sqlx::query_scalar(
        "INSERT INTO learning_outcome_groups (course_id, title) \
         VALUES ($1, 'Core Skills') RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed group");
    let o1: i64 = sqlx::query_scalar(
        "INSERT INTO learning_outcomes (title, description, display_name) \
         VALUES ('Analysis', 'Break down problems', 'Analytical skill') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("seed outcome 1");
    let o2: i64 = sqlx::query_scalar(
        "INSERT INTO learning_outcomes (title) VALUES ('Communication') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .expect("seed outcome 2");
    for oid in [o1, o2] {
        sqlx::query(
            "INSERT INTO learning_outcome_links (outcome_id, outcome_group_id) VALUES ($1, $2)",
        )
        .bind(oid)
        .bind(group)
        .execute(&pool)
        .await
        .expect("seed link");
    }
    // A deleted link: the outcome must not appear.
    let o3: i64 =
        sqlx::query_scalar("INSERT INTO learning_outcomes (title) VALUES ('Ghost') RETURNING id")
            .fetch_one(&pool)
            .await
            .expect("seed outcome 3");
    sqlx::query(
        "INSERT INTO learning_outcome_links (outcome_id, outcome_group_id, workflow_state) \
         VALUES ($1, $2, 'deleted')",
    )
    .bind(o3)
    .bind(group)
    .execute(&pool)
    .await
    .expect("seed deleted link");

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "query ($id: ID!) { course(id: $id) { outcomeGroups { id title \
         outcomes { id title description displayName createdAt updatedAt } } \
         outcomesConnection(first: 10) { edges { node { id title } } \
         pageInfo { hasNextPage } } } }",
        json!({ "id": course.to_string() }),
    )
    .await;

    let groups = body["data"]["course"]["outcomeGroups"]
        .as_array()
        .expect("groups");
    assert_eq!(groups.len(), 1, "{body}");
    let g = &groups[0];
    assert_eq!(g["id"], group.to_string(), "{body}");
    assert_eq!(g["title"], "Core Skills", "{body}");
    let outcomes = g["outcomes"].as_array().expect("outcomes");
    assert_eq!(outcomes.len(), 2, "deleted link filtered: {body}");
    assert_eq!(outcomes[0]["id"], o1.to_string(), "{body}");
    assert_eq!(outcomes[0]["displayName"], "Analytical skill", "{body}");
    assert_eq!(outcomes[0]["description"], "Break down problems", "{body}");
    assert!(outcomes[0]["createdAt"].is_string(), "{body}");
    assert!(outcomes[1]["displayName"].is_null(), "{body}");

    let conn_edges = body["data"]["course"]["outcomesConnection"]["edges"]
        .as_array()
        .expect("conn edges");
    assert_eq!(conn_edges.len(), 2, "course outcomes via links: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn media_object_owner_scoped_with_tracks() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let media = format!("{prefix}-media-1");
    let (mobj,): (i64,) = sqlx::query_as(
        "INSERT INTO media_objects (media_id, media_type, title, user_entered_title, user_id) \
         VALUES ($1, 'video', 'Lecture 1', 'My lecture', $2) RETURNING id",
    )
    .bind(&media)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .expect("seed media object");
    let t1: i64 = sqlx::query_scalar(
        "INSERT INTO media_tracks (media_object_id, user_id, kind, locale, content) \
         VALUES ($1, $2, 'subtitles', 'en', 'Hello') RETURNING id",
    )
    .bind(mobj)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .expect("seed track 1");
    sqlx::query(
        "INSERT INTO media_tracks (media_object_id, user_id, kind, locale, content, workflow_state) \
         VALUES ($1, $2, 'captions', 'fr', 'Bonjour', 'deleted')",
    )
    .bind(mobj)
    .bind(owner)
    .execute(&pool)
    .await
    .expect("seed deleted track");

    let query = "query ($id: String!) { mediaObject(mediaId: $id) { id mediaId mediaType title \
                 userEnteredTitle createdAt mediaTracks { id kind locale content workflowState } } }";

    // Owner sees it with only active tracks.
    let body = gql_post(
        &server,
        &auth(owner, "student"),
        query,
        json!({ "id": media }),
    )
    .await;
    let mo = &body["data"]["mediaObject"];
    assert_eq!(mo["mediaId"], media, "{body}");
    assert_eq!(mo["mediaType"], "video", "{body}");
    assert_eq!(mo["title"], "Lecture 1", "{body}");
    assert_eq!(mo["userEnteredTitle"], "My lecture", "{body}");
    let tracks = mo["mediaTracks"].as_array().expect("tracks");
    assert_eq!(tracks.len(), 1, "deleted track filtered: {body}");
    assert_eq!(tracks[0]["id"], t1.to_string(), "{body}");
    assert_eq!(tracks[0]["kind"], "subtitles", "{body}");
    assert_eq!(tracks[0]["locale"], "en", "{body}");
    assert_eq!(tracks[0]["content"], "Hello", "{body}");
    assert_eq!(tracks[0]["workflowState"], "active", "{body}");

    // Stranger gets null (owner-scoped).
    let body = gql_post(
        &server,
        &auth(stranger, "student"),
        query,
        json!({ "id": media }),
    )
    .await;
    assert!(body["data"]["mediaObject"].is_null(), "{body}");

    // Admin sees it (self-or-admin).
    let body = gql_post(
        &server,
        &auth(admin, "admin"),
        query,
        json!({ "id": media }),
    )
    .await;
    assert!(body["data"]["mediaObject"].is_object(), "{body}");

    // Unknown media id → null.
    let body = gql_post(
        &server,
        &auth(owner, "student"),
        query,
        json!({ "id": format!("{prefix}-missing") }),
    )
    .await;
    assert!(body["data"]["mediaObject"].is_null(), "{body}");

    cleanup(&pool, &prefix).await;
}
