//! Integration tests for the batch-1 GraphQL object types: Module,
//! ModuleItem, Page, File, Folder, Group, GroupMembership, GroupSet,
//! Conversation, ConversationMessage, ConversationParticipant, Progress.
//!
//! Same harness conventions as `graphql_integration.rs`: axum-test against
//! the real router mounted at `/api/graphql`, raw JSON POST bodies, unique
//! per-run prefix, full teardown, SKIP when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-obj-{}-{}",
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

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        r#"
        INSERT INTO enrollments (user_id, course_id, role, workflow_state)
        VALUES ($1, $2, $3, 'active')
        "#,
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
    // Courses cascade modules/pages/folders/files/groups/group_categories.
    let _ = sqlx::query(
        "DELETE FROM conversation_participants WHERE conversation_id IN \
         (SELECT id FROM conversations WHERE subject LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM conversations WHERE subject LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM progress WHERE tag LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
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
// Modules + module items
// ---------------------------------------------------------------------------

#[tokio::test]
async fn modules_and_module_items_resolvable() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let module_id: i64 = sqlx::query_scalar(
        "INSERT INTO modules (course_id, name, position, workflow_state) \
         VALUES ($1, $2, 1, 'active') RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-m1"))
    .fetch_one(&pool)
    .await
    .expect("seed module");

    let page_id: i64 = sqlx::query_scalar(
        "INSERT INTO pages (course_id, title, workflow_state) VALUES ($1, $2, 'published') RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-p1"))
    .fetch_one(&pool)
    .await
    .expect("seed page");

    // Module item WITHOUT an explicit title: falls back to the page title
    // (Canvas ContentTag#title fallback).
    let item_id: i64 = sqlx::query_scalar(
        "INSERT INTO module_items (module_id, item_type, item_id, position, indent, workflow_state) \
         VALUES ($1, 'Page', $2, 1, 0, 'active') RETURNING id",
    )
    .bind(module_id)
    .bind(page_id)
    .fetch_one(&pool)
    .await
    .expect("seed module item");

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        r#"
        query ($id: ID!, $item: ID!) {
          course(id: $id) {
            modulesConnection(first: 10) {
              edges { node { id name position published moduleItemsTotalCount } }
            }
          }
          moduleItem(id: $item) { id title indent published module { id name } }
        }
        "#,
        json!({ "id": course.to_string(), "item": item_id.to_string() }),
    )
    .await;

    let edges = &body["data"]["course"]["modulesConnection"]["edges"];
    assert!(edges.as_array().unwrap().len() == 1, "body: {body}");
    let node = &edges[0]["node"];
    assert_eq!(node["name"], json!(format!("{prefix}-m1")));
    assert_eq!(node["published"], json!(true));
    assert_eq!(node["moduleItemsTotalCount"], json!(1));

    let item = &body["data"]["moduleItem"];
    // Title falls back to the linked page title.
    assert_eq!(item["title"], json!(format!("{prefix}-p1")), "body: {body}");
    assert_eq!(item["published"], json!(true));
    assert_eq!(item["module"]["id"], json!(module_id.to_string()));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn module_item_hidden_from_unenrolled_user() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "outsider", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let module_id: i64 = sqlx::query_scalar(
        "INSERT INTO modules (course_id, name, workflow_state) VALUES ($1, $2, 'active') RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-m1"))
    .fetch_one(&pool)
    .await
    .expect("seed module");
    let item_id: i64 = sqlx::query_scalar(
        "INSERT INTO module_items (module_id, item_type, item_id, workflow_state) \
         VALUES ($1, 'Assignment', 1, 'active') RETURNING id",
    )
    .bind(module_id)
    .fetch_one(&pool)
    .await
    .expect("seed item");

    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        "query ($item: ID!) { moduleItem(id: $item) { id } }",
        json!({ "item": item_id.to_string() }),
    )
    .await;
    assert_eq!(body["data"]["moduleItem"], json!(null), "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pages_connection_lists_course_pages() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, student, course, "student").await;

    sqlx::query(
        "INSERT INTO pages (course_id, title, body, url, workflow_state, front_page) \
         VALUES ($1, $2, 'body', '/x', 'published', true)",
    )
    .bind(course)
    .bind(format!("{prefix}-front"))
    .execute(&pool)
    .await
    .expect("seed page");

    let body = gql_post(
        &server,
        &auth(student, "student"),
        r#"
        query ($id: ID!) {
          course(id: $id) {
            pagesConnection(first: 10) {
              edges { node { id title published frontPage url } }
            }
          }
        }
        "#,
        json!({ "id": course.to_string() }),
    )
    .await;

    let edges = &body["data"]["course"]["pagesConnection"]["edges"];
    assert_eq!(edges.as_array().unwrap().len(), 1, "body: {body}");
    let node = &edges[0]["node"];
    assert_eq!(node["title"], json!(format!("{prefix}-front")));
    assert_eq!(node["published"], json!(true));
    assert_eq!(node["frontPage"], json!(true));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Files + folders
// ---------------------------------------------------------------------------

#[tokio::test]
async fn folders_files_and_root_folder_query() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let folder_id: i64 = sqlx::query_scalar(
        "INSERT INTO folders (name, full_name, context_type, context_id, workflow_state) \
         VALUES ($1, $2, 'Course', $3, 'visible') RETURNING id",
    )
    .bind(format!("{prefix}-docs"))
    .bind(format!("{prefix}/docs"))
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed folder");

    sqlx::query(
        "INSERT INTO files (course_id, uploader_id, filename, display_name, content_type, size, folder_id) \
         VALUES ($1, $2, $3, $4, 'application/pdf', 1234, $5)",
    )
    .bind(course)
    .bind(teacher)
    .bind(format!("{prefix}-syllabus.pdf"))
    .bind("Syllabus.pdf")
    .bind(folder_id)
    .execute(&pool)
    .await
    .expect("seed file");

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        r#"
        query ($id: ID!, $folder: ID!) {
          course(id: $id) {
            foldersConnection(first: 10) {
              edges { node { id name fullName rootFolder filesCount files { id displayName title contentType mimeClass size sizeBytes } } }
            }
          }
          folder(id: $folder) { id name filesCount }
        }
        "#,
        json!({ "id": course.to_string(), "folder": folder_id.to_string() }),
    )
    .await;

    let node = &body["data"]["course"]["foldersConnection"]["edges"][0]["node"];
    assert_eq!(
        node["name"],
        json!(format!("{prefix}-docs")),
        "body: {body}"
    );
    assert_eq!(node["rootFolder"], json!(true));
    assert_eq!(node["filesCount"], json!(1));
    let file = &node["files"][0];
    assert_eq!(file["title"], json!(format!("{prefix}-syllabus.pdf")));
    assert_eq!(file["size"], json!("1234"));
    assert_eq!(file["sizeBytes"], json!(1234));
    assert_eq!(file["mimeClass"], json!("pdf"));

    assert_eq!(body["data"]["folder"]["id"], json!(folder_id.to_string()));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Groups + group sets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn groups_group_sets_and_members() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "student", "student").await;
    let course = seed_course(&pool, &prefix, "course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let set_id: i64 = sqlx::query_scalar(
        "INSERT INTO group_categories (course_id, name, self_signup, group_limit, auto_leader) \
         VALUES ($1, $2, 'enabled', 4, 'random') RETURNING id",
    )
    .bind(course)
    .bind(format!("{prefix}-set"))
    .fetch_one(&pool)
    .await
    .expect("seed group set");

    let group_id: i64 = sqlx::query_scalar(
        "INSERT INTO groups (course_id, group_category_id, name, workflow_state) \
         VALUES ($1, $2, $3, 'available') RETURNING id",
    )
    .bind(course)
    .bind(set_id)
    .bind(format!("{prefix}-team"))
    .fetch_one(&pool)
    .await
    .expect("seed group");

    sqlx::query(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state) \
         VALUES ($1, $2, 'accepted')",
    )
    .bind(group_id)
    .bind(student)
    .execute(&pool)
    .await
    .expect("seed membership");

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        r#"
        query ($id: ID!) {
          course(id: $id) {
            groupSetsConnection(first: 10) {
              edges { node { id name memberLimit selfSignup autoLeader groups { id name membersCount } } }
            }
            groupsConnection(first: 10) {
              edges { node { id name state membersConnection(first: 10) {
                edges { node { state user { id name } } }
              } } }
            }
          }
        }
        "#,
        json!({ "id": course.to_string() }),
    )
    .await;

    let set_node = &body["data"]["course"]["groupSetsConnection"]["edges"][0]["node"];
    assert_eq!(
        set_node["name"],
        json!(format!("{prefix}-set")),
        "body: {body}"
    );
    assert_eq!(set_node["memberLimit"], json!(4));
    assert_eq!(set_node["selfSignup"], json!("enabled"));
    assert_eq!(set_node["autoLeader"], json!("random"));
    assert_eq!(
        set_node["groups"][0]["name"],
        json!(format!("{prefix}-team"))
    );
    assert_eq!(set_node["groups"][0]["membersCount"], json!(1));

    let group_node = &body["data"]["course"]["groupsConnection"]["edges"][0]["node"];
    assert_eq!(group_node["state"], json!("available"));
    let member = &group_node["membersConnection"]["edges"][0]["node"];
    assert_eq!(member["state"], json!("accepted"));
    assert_eq!(member["user"]["id"], json!(student.to_string()));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Conversations
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conversations_and_participant_scoping() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let alice = seed_user(&pool, &prefix, "alice", "student").await;
    let bob = seed_user(&pool, &prefix, "bob", "student").await;

    let conv_id: i64 = sqlx::query_scalar(
        "INSERT INTO conversations (subject, workflow_state) VALUES ($1, 'active') RETURNING id",
    )
    .bind(format!("{prefix}-hello"))
    .fetch_one(&pool)
    .await
    .expect("seed conversation");

    for uid in [alice, bob] {
        sqlx::query(
            "INSERT INTO conversation_participants (conversation_id, user_id, workflow_state) \
             VALUES ($1, $2, 'active')",
        )
        .bind(conv_id)
        .bind(uid)
        .execute(&pool)
        .await
        .expect("seed participant");
    }

    sqlx::query(
        "INSERT INTO conversation_messages (conversation_id, author_id, body) VALUES ($1, $2, 'hi')",
    )
    .bind(conv_id)
    .bind(alice)
    .execute(&pool)
    .await
    .expect("seed message");

    let body = gql_post(
        &server,
        &auth(alice, "student"),
        r#"
        query ($me: ID!, $other: ID!) {
          me: user(id: $me) {
            conversationsConnection(first: 10) {
              edges { node { conversation { subject isPrivate conversationMessagesCount
                conversationMessagesConnection(first: 5) { edges { node { body author { id } } } }
                conversationParticipantsConnection(first: 5) { edges { node { userId workflowState user { id name } } } }
              } } }
            }
          }
          other: user(id: $other) { conversationsConnection(first: 10) { edges { node { id } } } }
        }
        "#,
        json!({ "me": alice.to_string(), "other": bob.to_string() }),
    )
    .await;

    let node = &body["data"]["me"]["conversationsConnection"]["edges"][0]["node"];
    let conv = &node["conversation"];
    assert_eq!(
        conv["subject"],
        json!(format!("{prefix}-hello")),
        "body: {body}"
    );
    assert_eq!(conv["isPrivate"], json!(true));
    assert_eq!(conv["conversationMessagesCount"], json!(1));
    assert_eq!(
        conv["conversationMessagesConnection"]["edges"][0]["node"]["body"],
        json!("hi")
    );
    let parts = conv["conversationParticipantsConnection"]["edges"]
        .as_array()
        .unwrap();
    assert_eq!(parts.len(), 2);

    // Bob's conversations are invisible to Alice (per-user scoping:
    // user_type.rb:431 has no admin override either).
    let other_conn = &body["data"]["other"]["conversationsConnection"];
    let edges = other_conn["edges"].as_array();
    assert!(
        other_conn.is_null() || edges.map(|e| e.is_empty()).unwrap_or(true),
        "alice must not see bob's conversations; body: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

#[tokio::test]
async fn progress_query_requires_ownership() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let owner = seed_user(&pool, &prefix, "owner", "student").await;
    let other = seed_user(&pool, &prefix, "other", "student").await;

    let progress_id: i64 = sqlx::query_scalar(
        "INSERT INTO progress (tag, workflow_state, completion, user_id) \
         VALUES ($1, 'completed', 100, $2) RETURNING id",
    )
    .bind(format!("{prefix}-tag"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .expect("seed progress");

    let body = gql_post(
        &server,
        &auth(owner, "student"),
        "query ($id: ID!) { progress(id: $id) { id tag state completion } }",
        json!({ "id": progress_id.to_string() }),
    )
    .await;
    let p = &body["data"]["progress"];
    assert_eq!(p["tag"], json!(format!("{prefix}-tag")), "body: {body}");
    assert_eq!(p["state"], json!("completed"));
    assert_eq!(p["completion"], json!(100));

    // Another user cannot read it.
    let body = gql_post(
        &server,
        &auth(other, "student"),
        "query ($id: ID!) { progress(id: $id) { id } }",
        json!({ "id": progress_id.to_string() }),
    )
    .await;
    assert_eq!(body["data"]["progress"], json!(null), "body: {body}");

    cleanup(&pool, &prefix).await;
}
