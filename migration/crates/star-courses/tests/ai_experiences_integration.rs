//! Integration tests for the Canvas AI Experiences and AI Conversations
//! routes (`AiExperiencesController`, `AiConversationsController`).
//!
//! Pinned behavior:
//! - the `ai_experiences` feature flag gates every route — with the flag
//!   off the controller answers the Canvas `404 {"error": "Resource Not
//!   Found"}`;
//! - `index` and `show` need a course manager or an enrolled member, every
//!   other route needs a course manager — a missing right is the Canvas
//!   `403 {"status":"unauthorized","errors":[{"message":"user not
//!   authorized to perform that action"}]}`;
//! - a student sees only the published experiences and gets the
//!   `submission_status`; the teacher facing fields (`facts`,
//!   `pedagogical_guidance`) are hidden from everybody but a manager;
//! - a validation failure answers `400` with the Rails error hash;
//! - an experience of another course, an unknown experience and a deleted
//!   experience answer `404`;
//! - the delete is a soft delete and the response is the deleted experience;
//! - the conversation routes need the external `llm-conversation` service, so
//!   they answer the Ruby rescue path `503`; deleting a conversation is local
//!   and completes it.
//!
//! Fleet rule: dedicated users, accounts, courses, experiences and
//! conversations per test, every seeded row is cleaned up. Skipped cleanly
//! when DATABASE_URL is unset or the AI tables are not migrated yet.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "ai-exp-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis(),
        n
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

    // The AI tables come from their own migration; skip until it landed.
    let exists: Option<(i32,)> = sqlx::query_as(
        "SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = 'ai_experiences'",
    )
    .fetch_optional(&pool)
    .await
    .expect("check for the ai_experiences table");
    if exists.is_none() {
        println!("SKIP: the ai_experiences table is not migrated yet");
        return None;
    }

    let state = AppState {
        db: pool.clone(),
        jwt_secret: "test-jwt-secret".to_string(),
    };
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_courses::router())
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

async fn seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
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

/// Enable a feature for the course (the course level override of Canvas).
async fn enable_feature(pool: &PgPool, course_id: i64, feature: &str) {
    sqlx::query(
        "INSERT INTO feature_flags (context_type, context_id, feature, display_name, state) \
         VALUES ('Course', $1, $2, $2, 'enabled') \
         ON CONFLICT (context_type, context_id, feature) DO UPDATE SET state = 'enabled'",
    )
    .bind(course_id)
    .bind(feature)
    .execute(pool)
    .await
    .expect("enable the feature");
}

async fn seed_experience(
    pool: &PgPool,
    prefix: &str,
    course_id: i64,
    account_id: i64,
    title: &str,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO ai_experiences \
         (course_id, account_id, root_account_id, title, description, facts, \
          learning_objective, pedagogical_guidance, workflow_state, context_index_status) \
         VALUES ($1, $2, $2, $3, $4, $5, $6, $7, $8, 'not_started') RETURNING id",
    )
    .bind(course_id)
    .bind(account_id)
    .bind(format!("{prefix}-{title}"))
    .bind("A description")
    .bind("Some facts")
    .bind("A learning objective")
    .bind("A pedagogical guidance")
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed experience")
}

async fn seed_conversation(
    pool: &PgPool,
    prefix: &str,
    experience_id: i64,
    user_id: i64,
    course_id: i64,
    account_id: i64,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO ai_conversations \
         (root_account_id, account_id, course_id, user_id, ai_experience_id, \
          llm_conversation_id, workflow_state) \
         VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
    )
    .bind(account_id)
    .bind(account_id)
    .bind(course_id)
    .bind(user_id)
    .bind(experience_id)
    .bind(format!("{prefix}-llm-{experience_id}-{user_id}"))
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed conversation")
}

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM ai_conversations WHERE ai_experience_id IN \
           (SELECT id FROM ai_experiences WHERE title LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM ai_experience_context_files WHERE ai_experience_id IN \
           (SELECT id FROM ai_experiences WHERE title LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM ai_experiences WHERE title LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_type = 'Course' AND context_id IN \
           (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
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
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

#[tokio::test]
async fn a_manager_creates_shows_updates_and_deletes_an_experience() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    enable_feature(&pool, course, "ai_experiences").await;

    // The manager creates an experience.
    let resp = server
        .post(&format!("/courses/{course}/ai_experiences"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "ai_experience": {
                "title": format!("{prefix}-Customer Service Simulation"),
                "description": "Practice customer service skills",
                "facts": "You are a customer service representative",
                "learning_objective": "Students practise active listening",
                "pedagogical_guidance": "A customer is calling about a billing issue",
            },
        }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(
        created["title"],
        format!("{prefix}-Customer Service Simulation")
    );
    assert_eq!(created["workflow_state"], "unpublished");
    assert_eq!(created["course_id"], course);
    assert_eq!(created["context_index_status"], "not_started");
    // Canvas only renders `can_manage` when the caller passed it.
    assert!(created.get("can_manage").is_none());
    // The teacher facing fields are hidden without `can_manage`.
    assert!(created.get("facts").is_none());
    assert!(created.get("pedagogical_guidance").is_none());
    let experience_id = created["id"].as_i64().expect("experience id");

    // The manager lists the experience.
    let resp = server
        .get(&format!("/courses/{course}/ai_experiences"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list["can_manage"], true);
    let experiences = list["experiences"].as_array().expect("experiences");
    assert_eq!(experiences.len(), 1);
    assert_eq!(experiences[0]["id"], experience_id);
    assert_eq!(experiences[0]["can_manage"], true);
    assert_eq!(experiences[0]["context_ready"], true);
    assert!(experiences[0].get("can_unpublish").is_some());
    assert!(experiences[0].get("facts").is_some());

    // The manager reads the experience.
    let resp = server
        .get(&format!("/courses/{course}/ai_experiences/{experience_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let shown: Value = resp.json();
    assert_eq!(shown["id"], experience_id);
    assert_eq!(shown["can_manage"], true);

    // The manager publishes the experience.
    let resp = server
        .put(&format!("/courses/{course}/ai_experiences/{experience_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "ai_experience": { "workflow_state": "published" },
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["workflow_state"], "published");

    // The manager deletes the experience — a soft delete.
    let resp = server
        .delete(&format!("/courses/{course}/ai_experiences/{experience_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let deleted: Value = resp.json();
    assert_eq!(deleted["workflow_state"], "deleted");

    // A deleted experience is gone.
    let resp = server
        .get(&format!("/courses/{course}/ai_experiences/{experience_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let missing: Value = resp.json();
    assert_eq!(missing["error"], "Resource Not Found");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_student_sees_only_the_published_experiences() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, student, course, "student").await;
    enable_feature(&pool, course, "ai_experiences").await;

    seed_experience(&pool, &prefix, course, account, "Published", "published").await;
    let unpublished = seed_experience(
        &pool,
        &prefix,
        course,
        account,
        "Unpublished",
        "unpublished",
    )
    .await;

    // The student lists the published experience only.
    let resp = server
        .get(&format!("/courses/{course}/ai_experiences"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    assert_eq!(list["can_manage"], false);
    let experiences = list["experiences"].as_array().expect("experiences");
    assert_eq!(experiences.len(), 1);
    assert_eq!(experiences[0]["title"], format!("{prefix}-Published"));
    assert_eq!(experiences[0]["can_manage"], false);
    assert_eq!(experiences[0]["submission_status"], "not_started");
    assert!(experiences[0].get("facts").is_none());
    assert!(experiences[0].get("pedagogical_guidance").is_none());

    // The student may read the published experience.
    let resp = server
        .get(&format!(
            "/courses/{course}/ai_experiences/{}",
            experiences[0]["id"].as_i64().unwrap()
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // The student may not read the unpublished one — the Canvas policy.
    let resp = server
        .get(&format!("/courses/{course}/ai_experiences/{unpublished}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let denied: Value = resp.json();
    assert_eq!(denied["status"], "unauthorized");
    assert_eq!(
        denied["errors"][0]["message"],
        "user not authorized to perform that action"
    );

    // The student may not create an experience.
    let resp = server
        .post(&format!("/courses/{course}/ai_experiences"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "ai_experience": {
                "title": "T",
                "learning_objective": "L",
                "pedagogical_guidance": "G",
            },
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn a_validation_failure_answers_400_with_the_rails_errors() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    enable_feature(&pool, course, "ai_experiences").await;

    let resp = server
        .post(&format!("/courses/{course}/ai_experiences"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "ai_experience": { "description": "no title" } }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let errors: Value = resp.json();
    assert_eq!(errors["title"][0], "can't be blank");
    assert_eq!(errors["learning_objective"][0], "can't be blank");
    assert_eq!(errors["pedagogical_guidance"][0], "can't be blank");

    // An unknown workflow state is not in the model list.
    let resp = server
        .post(&format!("/courses/{course}/ai_experiences"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "ai_experience": {
                "title": "T",
                "learning_objective": "L",
                "pedagogical_guidance": "G",
                "workflow_state": "archived",
            },
        }))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let errors: Value = resp.json();
    assert_eq!(errors["workflow_state"][0], "is not included in the list");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_feature_flag_off_answers_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    // The `ai_experiences` feature is not enabled for this course.

    let resp = server
        .get(&format!("/courses/{course}/ai_experiences"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let missing: Value = resp.json();
    assert_eq!(missing["error"], "Resource Not Found");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_teacher_conversation_list_reports_the_students_without_a_conversation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other, course, "student").await;
    enable_feature(&pool, course, "ai_experiences").await;

    let experience =
        seed_experience(&pool, &prefix, course, account, "Experience", "published").await;
    seed_conversation(
        &pool, &prefix, experience, student, course, account, "active",
    )
    .await;

    // The teacher lists the conversations of the students.
    let resp = server
        .get(&format!(
            "/courses/{course}/ai_experiences/{experience}/ai_conversations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let list: Value = resp.json();
    let conversations = list["conversations"].as_array().expect("conversations");
    assert_eq!(conversations.len(), 2);

    let with_conversation = conversations
        .iter()
        .find(|entry| entry.get("has_conversation").is_none())
        .expect("the student with a conversation");
    assert_eq!(with_conversation["user_id"], student);
    assert_eq!(with_conversation["workflow_state"], "active");
    assert!(with_conversation["llm_conversation_id"].is_string());
    assert_eq!(with_conversation["student"]["id"], student);
    assert_eq!(with_conversation["student"]["name"], "Student");

    let without_conversation = conversations
        .iter()
        .find(|entry| entry.get("has_conversation").is_some())
        .expect("the student without a conversation");
    assert!(without_conversation["id"].is_null());
    assert_eq!(without_conversation["user_id"], other.to_string());
    assert_eq!(without_conversation["has_conversation"], false);
    assert_eq!(
        without_conversation["student"]["id"],
        other.to_string(),
        "the short student object renders the id as a string"
    );

    // A student may not read the teacher conversation list.
    let resp = server
        .get(&format!(
            "/courses/{course}/ai_experiences/{experience}/ai_conversations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn deleting_a_conversation_completes_it() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    enable_feature(&pool, course, "ai_experiences").await;

    let experience =
        seed_experience(&pool, &prefix, course, account, "Experience", "published").await;
    let conversation = seed_conversation(
        &pool, &prefix, experience, student, course, account, "active",
    )
    .await;

    // The teacher deletes the conversation — the row is completed.
    let resp = server
        .delete(&format!(
            "/courses/{course}/ai_experiences/{experience}/conversations/{conversation}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["message"], "Conversation completed successfully");

    let state: (String,) =
        sqlx::query_as("SELECT workflow_state FROM ai_conversations WHERE id = $1")
            .bind(conversation)
            .fetch_one(&pool)
            .await
            .expect("read the conversation");
    assert_eq!(state.0, "completed");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn the_conversation_routes_answer_503_without_the_llm_service() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let account = seed_account(&pool, &prefix, "Account").await;
    let course = seed_course(&pool, &prefix, "Course", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    enable_feature(&pool, course, "ai_experiences").await;

    let experience =
        seed_experience(&pool, &prefix, course, account, "Experience", "published").await;
    let conversation = seed_conversation(
        &pool, &prefix, experience, student, course, account, "active",
    )
    .await;

    // The active conversation of a student without one is an empty object.
    let resp = server
        .get(&format!(
            "/courses/{course}/ai_experiences/{experience}/conversations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body, json!({}));

    // The message history needs the external service.
    let resp = server
        .get(&format!(
            "/courses/{course}/ai_experiences/{experience}/conversations/{conversation}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 503, "body: {}", resp.text());
    let body: Value = resp.json();
    assert!(body["error"].is_string());

    // The evaluation needs the external service too.
    let resp = server
        .get(&format!(
            "/courses/{course}/ai_experiences/{experience}/conversations/{conversation}/evaluation"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 503, "body: {}", resp.text());

    // A message without a body is the Canvas 400.
    let resp = server
        .post(&format!(
            "/courses/{course}/ai_experiences/{experience}/conversations/{conversation}/messages"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "message is required");

    // A student may not read a conversation of another user.
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    seed_enrollment(&pool, other, course, "student").await;
    let resp = server
        .get(&format!(
            "/courses/{course}/ai_experiences/{experience}/conversations/{conversation}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["error"], "Resource Not Found");

    cleanup(&pool, &prefix).await;
}
