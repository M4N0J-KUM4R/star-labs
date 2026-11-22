//! Integration tests for the Batch-C mutations (GraphQL parity gap
//! completion). Conventions mirror
//! `graphql_mutations_assignments_integration.rs`.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-mc-{}-{}",
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

// ---------------------------------------------------------------------------
// Discussion topic/entry update + delete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn discussion_update_delete_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "MutCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let topic: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, title, message) \
         VALUES ($1, 'Draft', 'v1') RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed topic");
    let entry: i64 = sqlx::query_scalar(
        "INSERT INTO discussion_entries (topic_id, user_id, message) \
         VALUES ($1, $2, 'first draft') RETURNING id",
    )
    .bind(topic)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("seed entry");

    // updateDiscussionTopic (teacher): retitles + publishes.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: UpdateDiscussionTopicInput!) { updateDiscussionTopic(input: $input) \
         { discussionTopic { id title message published updatedAt } errors { message } } }",
        json!({ "input": {
            "discussionTopicId": topic.to_string(),
            "title": "Final",
            "published": true
        } }),
    )
    .await;
    let t = &body["data"]["updateDiscussionTopic"]["discussionTopic"];
    assert_eq!(t["title"], "Final", "{body}");
    assert_eq!(t["published"], true, "{body}");
    assert_eq!(t["message"], "v1", "{body}");

    // Student cannot update the topic.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: UpdateDiscussionTopicInput!) { updateDiscussionTopic(input: $input) \
         { discussionTopic { id } } }",
        json!({ "input": { "discussionTopicId": topic.to_string(), "title": "Hack" } }),
    )
    .await;
    assert!(body["data"]["updateDiscussionTopic"].is_null(), "{body}");

    // updateDiscussionEntry (author).
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: UpdateDiscussionEntryInput!) { updateDiscussionEntry(input: $input) \
         { discussionEntry { id discussionTopicId message updatedAt } errors { message } } }",
        json!({ "input": { "discussionEntryId": entry.to_string(), "message": "edited" } }),
    )
    .await;
    let e = &body["data"]["updateDiscussionEntry"]["discussionEntry"];
    assert_eq!(e["message"], "edited", "{body}");
    assert_eq!(e["discussionTopicId"], topic.to_string(), "{body}");

    // Another student cannot edit someone else's entry.
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    seed_enrollment(&pool, other, course, "student").await;
    let body = gql_post(
        &server,
        &auth(other, "student"),
        "mutation ($input: UpdateDiscussionEntryInput!) { updateDiscussionEntry(input: $input) \
         { discussionEntry { id } } }",
        json!({ "input": { "discussionEntryId": entry.to_string(), "message": "nope" } }),
    )
    .await;
    assert!(body["data"]["updateDiscussionEntry"].is_null(), "{body}");

    // Blank title on topic update → payload validation error.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: UpdateDiscussionTopicInput!) { updateDiscussionTopic(input: $input) \
         { discussionTopic { id } errors { attribute message } } }",
        json!({ "input": { "discussionTopicId": topic.to_string(), "title": "  " } }),
    )
    .await;
    let p = &body["data"]["updateDiscussionTopic"];
    assert!(p["discussionTopic"].is_null(), "{body}");
    assert_eq!(p["errors"][0]["attribute"], "title", "{body}");

    // Teacher deletes the entry (moderation), then the topic.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: DeleteDiscussionEntryInput!) { deleteDiscussionEntry(input: $input) \
         { success } }",
        json!({ "input": { "discussionEntryId": entry.to_string() } }),
    )
    .await;
    assert_eq!(
        body["data"]["deleteDiscussionEntry"]["success"], true,
        "{body}"
    );

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: DeleteDiscussionTopicInput!) { deleteDiscussionTopic(input: $input) \
         { success } }",
        json!({ "input": { "discussionTopicId": topic.to_string() } }),
    )
    .await;
    assert_eq!(
        body["data"]["deleteDiscussionTopic"]["success"], true,
        "{body}"
    );

    // Deleted topic: further updates report not found.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: UpdateDiscussionTopicInput!) { updateDiscussionTopic(input: $input) \
         { discussionTopic { id } } }",
        json!({ "input": { "discussionTopicId": topic.to_string(), "title": "Zombie" } }),
    )
    .await;
    assert!(body["data"]["updateDiscussionTopic"].is_null(), "{body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Learning outcome group + outcome CRUD
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcome_group_and_outcome_crud() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "OutCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // createLearningOutcomeGroup (teacher).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateLearningOutcomeGroupInput!) \
         { createLearningOutcomeGroup(input: $input) \
         { learningOutcomeGroup { id title description updatedAt } errors { message } } }",
        json!({ "input": {
            "courseId": course.to_string(),
            "title": "Skills",
            "description": "Core skills"
        } }),
    )
    .await;
    let group_id = body["data"]["createLearningOutcomeGroup"]["learningOutcomeGroup"]["id"]
        .as_str()
        .expect("group id")
        .to_string();
    assert_eq!(
        body["data"]["createLearningOutcomeGroup"]["learningOutcomeGroup"]["title"], "Skills",
        "{body}"
    );

    // Student cannot create a group.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: CreateLearningOutcomeGroupInput!) \
         { createLearningOutcomeGroup(input: $input) { learningOutcomeGroup { id } } }",
        json!({ "input": { "courseId": course.to_string(), "title": "Nope" } }),
    )
    .await;
    assert!(
        body["data"]["createLearningOutcomeGroup"].is_null(),
        "{body}"
    );

    // createLearningOutcome into the group.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateLearningOutcomeInput!) { createLearningOutcome(input: $input) \
         { learningOutcome { id title description displayName } errors { message } } }",
        json!({ "input": {
            "outcomeGroupId": group_id,
            "title": "Analysis",
            "displayName": "Analytical"
        } }),
    )
    .await;
    let outcome_id = body["data"]["createLearningOutcome"]["learningOutcome"]["id"]
        .as_str()
        .expect("outcome id")
        .to_string();
    assert_eq!(
        body["data"]["createLearningOutcome"]["learningOutcome"]["displayName"], "Analytical",
        "{body}"
    );

    // updateLearningOutcome.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: UpdateLearningOutcomeInput!) { updateLearningOutcome(input: $input) \
         { learningOutcome { id title description } errors { message } } }",
        json!({ "input": { "outcomeId": outcome_id, "title": "Deep Analysis" } }),
    )
    .await;
    assert_eq!(
        body["data"]["updateLearningOutcome"]["learningOutcome"]["title"], "Deep Analysis",
        "{body}"
    );

    // updateLearningOutcomeGroup.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: UpdateLearningOutcomeGroupInput!) \
         { updateLearningOutcomeGroup(input: $input) \
         { learningOutcomeGroup { id title } errors { message } } }",
        json!({ "input": { "outcomeGroupId": group_id, "title": "Core Skills" } }),
    )
    .await;
    assert_eq!(
        body["data"]["updateLearningOutcomeGroup"]["learningOutcomeGroup"]["title"], "Core Skills",
        "{body}"
    );

    // Blank title → payload validation error.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateLearningOutcomeInput!) { createLearningOutcome(input: $input) \
         { learningOutcome { id } errors { attribute message } } }",
        json!({ "input": { "outcomeGroupId": group_id, "title": "  " } }),
    )
    .await;
    let p = &body["data"]["createLearningOutcome"];
    assert!(p["learningOutcome"].is_null(), "{body}");
    assert_eq!(p["errors"][0]["attribute"], "title", "{body}");

    // deleteLearningOutcome, then deleteLearningOutcomeGroup.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: DeleteLearningOutcomeInput!) { deleteLearningOutcome(input: $input) \
         { success } }",
        json!({ "input": { "outcomeId": outcome_id } }),
    )
    .await;
    assert_eq!(
        body["data"]["deleteLearningOutcome"]["success"], true,
        "{body}"
    );

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: DeleteLearningOutcomeGroupInput!) \
         { deleteLearningOutcomeGroup(input: $input) { success } }",
        json!({ "input": { "outcomeGroupId": group_id } }),
    )
    .await;
    assert_eq!(
        body["data"]["deleteLearningOutcomeGroup"]["success"], true,
        "{body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// saveRubricAssessment / deleteSubmissionComment /
// updateNotificationPreferences / dismissAccountNotification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn rubric_comment_preference_dismiss_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "RubCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment: i64 = sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible) \
         VALUES ($1, 'Essay', 10) RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed assignment");
    let submission: i64 = sqlx::query_scalar(
        "INSERT INTO submissions (assignment_id, user_id, body) \
         VALUES ($1, $2, 'essay body') RETURNING id",
    )
    .bind(assignment)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("seed submission");
    let rubric: i64 = sqlx::query_scalar(
        "INSERT INTO rubrics (course_id, title, points_possible) \
         VALUES ($1, 'Essay rubric', 10) RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed rubric");
    let comment: i64 = sqlx::query_scalar(
        "INSERT INTO submission_comments (submission_id, author_id, comment) \
         VALUES ($1, $2, 'draft note') RETURNING id",
    )
    .bind(submission)
    .bind(teacher)
    .fetch_one(&pool)
    .await
    .expect("seed comment");
    let (account_id,): (i64,) =
        sqlx::query_as("INSERT INTO accounts (name) VALUES ('Batch C Account') RETURNING id")
            .fetch_one(&pool)
            .await
            .expect("seed account");
    let notification: i64 = sqlx::query_scalar(
        "INSERT INTO account_notifications (account_id, subject, message, start_at, end_at) \
         VALUES ($1, 'Notice', 'Hello', now() - interval '1 hour', now() + interval '1 day') \
         RETURNING id",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("seed notification");

    // saveRubricAssessment (teacher).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: SaveRubricAssessmentInput!) { saveRubricAssessment(input: $input) \
         { rubricAssessment { id rubricId submissionId assessorId score updatedAt } \
         errors { message } } }",
        json!({ "input": {
            "rubricId": rubric.to_string(),
            "submissionId": submission.to_string(),
            "score": 8.5,
            "data": { "criterion": "thesis", "points": 8.5 }
        } }),
    )
    .await;
    let ra = &body["data"]["saveRubricAssessment"]["rubricAssessment"];
    assert_eq!(ra["score"], 8.5, "{body}");
    assert_eq!(ra["assessorId"], teacher.to_string(), "{body}");
    assert_eq!(ra["rubricId"], rubric.to_string(), "{body}");

    // Student cannot save an assessment.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: SaveRubricAssessmentInput!) { saveRubricAssessment(input: $input) \
         { rubricAssessment { id } } }",
        json!({ "input": {
            "rubricId": rubric.to_string(),
            "submissionId": submission.to_string(),
            "score": 10.0
        } }),
    )
    .await;
    assert!(body["data"]["saveRubricAssessment"].is_null(), "{body}");

    // Rubric from another course is rejected.
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    let bad_rubric: i64 = sqlx::query_scalar(
        "INSERT INTO rubrics (course_id, title) VALUES ($1, 'Wrong') RETURNING id",
    )
    .bind(other_course)
    .fetch_one(&pool)
    .await
    .expect("seed bad rubric");
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: SaveRubricAssessmentInput!) { saveRubricAssessment(input: $input) \
         { rubricAssessment { id } } }",
        json!({ "input": {
            "rubricId": bad_rubric.to_string(),
            "submissionId": submission.to_string(),
            "score": 1.0
        } }),
    )
    .await;
    assert!(body["data"]["saveRubricAssessment"].is_null(), "{body}");

    // deleteSubmissionComment (author only).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: DeleteSubmissionCommentInput!) \
         { deleteSubmissionComment(input: $input) { success } }",
        json!({ "input": { "submissionCommentId": comment.to_string() } }),
    )
    .await;
    assert_eq!(
        body["data"]["deleteSubmissionComment"]["success"], true,
        "{body}"
    );

    // updateNotificationPreferences: upsert then re-upsert.
    let channel: i64 = sqlx::query_scalar(
        "INSERT INTO communication_channels (user_id, path, path_type) \
         VALUES ($1, $2, 'email') RETURNING id",
    )
    .bind(teacher)
    .bind(format!("{prefix}-teacher@test.local"))
    .fetch_one(&pool)
    .await
    .expect("seed channel");
    let mut_input = |freq: &str| {
        json!({ "input": {
            "communicationChannelId": channel.to_string(),
            "notification": "new_announcement",
            "frequency": freq
        } })
    };
    let query = "mutation ($input: UpdateNotificationPreferencesInput!) \
                 { updateNotificationPreferences(input: $input) \
                 { notificationPolicy { id communicationChannelId notification frequency } \
                 errors { attribute message } } }";
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        mut_input("daily"),
    )
    .await;
    let policy = &body["data"]["updateNotificationPreferences"]["notificationPolicy"];
    assert_eq!(policy["frequency"], "daily", "{body}");
    let policy_id = policy["id"].as_str().expect("policy id").to_string();

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        mut_input("never"),
    )
    .await;
    let policy = &body["data"]["updateNotificationPreferences"]["notificationPolicy"];
    assert_eq!(policy["frequency"], "never", "{body}");
    assert_eq!(policy["id"], policy_id, "upsert keeps the row: {body}");

    // Invalid frequency → payload validation error.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        query,
        json!({ "input": {
            "communicationChannelId": channel.to_string(),
            "notification": "new_announcement",
            "frequency": "sometimes"
        } }),
    )
    .await;
    let p = &body["data"]["updateNotificationPreferences"];
    assert!(p["notificationPolicy"].is_null(), "{body}");
    assert_eq!(p["errors"][0]["attribute"], "frequency", "{body}");

    // Someone else's channel → permission error.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        query,
        json!({ "input": {
            "communicationChannelId": channel.to_string(),
            "notification": "new_announcement",
            "frequency": "daily"
        } }),
    )
    .await;
    assert!(
        body["data"]["updateNotificationPreferences"].is_null(),
        "{body}"
    );

    // dismissAccountNotification: valid window succeeds.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: DismissAccountNotificationInput!) \
         { dismissAccountNotification(input: $input) { success } }",
        json!({ "input": { "accountNotificationId": notification.to_string() } }),
    )
    .await;
    assert_eq!(
        body["data"]["dismissAccountNotification"]["success"], true,
        "{body}"
    );

    // Unknown id → top-level error.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: DismissAccountNotificationInput!) \
         { dismissAccountNotification(input: $input) { success } }",
        json!({ "input": { "accountNotificationId": "999999999" } }),
    )
    .await;
    assert!(
        body["data"]["dismissAccountNotification"].is_null(),
        "{body}"
    );

    // Account cleanup.
    let _ = sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(account_id)
        .execute(&pool)
        .await;
    cleanup(&pool, &prefix).await;
}
