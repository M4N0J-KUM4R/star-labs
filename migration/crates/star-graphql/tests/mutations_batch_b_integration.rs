//! Integration tests for the Batch-B mutations (GraphQL parity gap
//! completion). Conventions mirror
//! `graphql_mutations_assignments_integration.rs`: raw JSON bodies at
//! `/api/graphql`, unique prefix, full teardown, SKIP without DATABASE_URL.
//!
//! Error channels per the CONTRACT: authorization failures and missing
//! records are top-level `errors` (data.<mutation> nulls out);
//! validation failures are payload `errors: [{ attribute, message }]`
//! inside HTTP 200.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gql-mb-{}-{}",
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
// createModule / createGroupSet / createGroupInSet
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_module_group_set_group_and_permission_split() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "MutCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // createModule as teacher.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateModuleInput!) { createModule(input: $input) { module \
         { id courseId name position state createdAt } errors { attribute message } } }",
        json!({ "input": { "courseId": course.to_string(), "name": "Unit 1", "position": 1 } }),
    )
    .await;
    let payload = &body["data"]["createModule"];
    assert!(payload["errors"].as_array().unwrap().is_empty(), "{body}");
    let module_id = payload["module"]["id"].as_str().expect("module id").to_string();
    assert_eq!(payload["module"]["name"], "Unit 1", "{body}");
    assert_eq!(payload["module"]["state"], "active", "{body}");
    assert_eq!(payload["module"]["courseId"], course.to_string(), "{body}");

    // createModule as student → top-level error, data null.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: CreateModuleInput!) { createModule(input: $input) { module { id } } }",
        json!({ "input": { "courseId": course.to_string(), "name": "Nope" } }),
    )
    .await;
    assert!(body["data"]["createModule"].is_null(), "{body}");
    let msg = body["errors"][0]["message"].as_str().unwrap_or_default();
    assert_eq!(msg, "insufficient permission", "{body}");

    // createGroupSet as teacher.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateGroupSetInput!) { createGroupSet(input: $input) { groupSet \
         { id name createdAt } errors { message } } }",
        json!({ "input": { "courseId": course.to_string(), "name": "Labs" } }),
    )
    .await;
    let set_id = body["data"]["createGroupSet"]["groupSet"]["id"]
        .as_str()
        .expect("set id")
        .to_string();
    assert_eq!(body["data"]["createGroupSet"]["groupSet"]["name"], "Labs", "{body}");

    // createGroupInSet as teacher.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateGroupInSetInput!) { createGroupInSet(input: $input) { group \
         { id groupSetId name } errors { message } } }",
        json!({ "input": { "groupSetId": set_id, "name": "Lab A" } }),
    )
    .await;
    let group = &body["data"]["createGroupInSet"]["group"];
    assert_eq!(group["name"], "Lab A", "{body}");
    assert_eq!(group["groupSetId"], set_id, "{body}");

    // createGroupInSet on an unknown set → top-level error.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateGroupInSetInput!) { createGroupInSet(input: $input) { group { id } } }",
        json!({ "input": { "groupSetId": "999999999", "name": "X" } }),
    )
    .await;
    assert!(body["data"]["createGroupInSet"].is_null(), "{body}");
    assert!(
        body["errors"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("group set not found"),
        "{body}"
    );

    // Blank name → payload-level validation error inside HTTP 200.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateModuleInput!) { createModule(input: $input) { module { id } \
         errors { attribute message } } }",
        json!({ "input": { "courseId": course.to_string(), "name": "  " } }),
    )
    .await;
    let payload = &body["data"]["createModule"];
    assert!(payload["module"].is_null(), "{body}");
    assert_eq!(payload["errors"][0]["attribute"], "name", "{body}");
    assert_eq!(payload["errors"][0]["message"], "name cannot be blank", "{body}");

    let _ = module_id;
    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// createDiscussionTopic / createDiscussionEntry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_discussion_topic_and_entry() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "DiscCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Teacher publishes.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateDiscussionTopicInput!) { createDiscussionTopic(input: $input) \
         { discussionTopic { id courseId title message published } errors { message } } }",
        json!({ "input": {
            "courseId": course.to_string(),
            "title": "Welcome",
            "message": "Say hi",
            "published": true
        } }),
    )
    .await;
    let topic = &body["data"]["createDiscussionTopic"]["discussionTopic"];
    assert_eq!(topic["title"], "Welcome", "{body}");
    assert_eq!(topic["published"], true, "{body}");
    let topic_id = topic["id"].as_str().expect("topic id").to_string();

    // Student creates unpublished (publish flag ignored without the right).
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: CreateDiscussionTopicInput!) { createDiscussionTopic(input: $input) \
         { discussionTopic { id published } errors { message } } }",
        json!({ "input": {
            "courseId": course.to_string(),
            "title": "Student topic",
            "published": true
        } }),
    )
    .await;
    assert_eq!(
        body["data"]["createDiscussionTopic"]["discussionTopic"]["published"],
        false,
        "{body}"
    );

    // Outsider (not enrolled) → permission error.
    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        "mutation ($input: CreateDiscussionTopicInput!) { createDiscussionTopic(input: $input) \
         { discussionTopic { id } } }",
        json!({ "input": { "courseId": course.to_string(), "title": "X" } }),
    )
    .await;
    assert!(body["data"]["createDiscussionTopic"].is_null(), "{body}");

    // Student replies with an entry, then a nested reply.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: CreateDiscussionEntryInput!) { createDiscussionEntry(input: $input) \
         { discussionEntry { id discussionTopicId parentId userId message } errors { message } } }",
        json!({ "input": { "discussionTopicId": topic_id, "message": "hi!" } }),
    )
    .await;
    let entry = &body["data"]["createDiscussionEntry"]["discussionEntry"];
    assert_eq!(entry["message"], "hi!", "{body}");
    assert!(entry["parentId"].is_null(), "{body}");
    assert_eq!(entry["userId"], student.to_string(), "{body}");
    let entry_id = entry["id"].as_str().expect("entry id").to_string();

    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateDiscussionEntryInput!) { createDiscussionEntry(input: $input) \
         { discussionEntry { id parentId } errors { message } } }",
        json!({ "input": {
            "discussionTopicId": topic_id,
            "message": "welcome!",
            "parentId": entry_id
        } }),
    )
    .await;
    let reply = &body["data"]["createDiscussionEntry"]["discussionEntry"];
    assert_eq!(reply["parentId"], entry_id, "{body}");

    // Entry on unknown topic → top-level error.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: CreateDiscussionEntryInput!) { createDiscussionEntry(input: $input) \
         { discussionEntry { id } } }",
        json!({ "input": { "discussionTopicId": "999999999", "message": "x" } }),
    )
    .await;
    assert!(body["data"]["createDiscussionEntry"].is_null(), "{body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// createConversation / addConversationMessage / deleteConversation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn conversation_mutations_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;

    // createConversation.
    let body = gql_post(
        &server,
        &auth(alice, "student"),
        "mutation ($input: CreateConversationInput!) { createConversation(input: $input) \
         { conversation { id subject messages { id authorId body } } errors { message } } }",
        json!({ "input": {
            "subject": "Project",
            "body": "Kickoff notes",
            "recipientIds": [bob.to_string()]
        } }),
    )
    .await;
    let conv = &body["data"]["createConversation"]["conversation"];
    assert_eq!(conv["subject"], "Project", "{body}");
    assert_eq!(conv["messages"][0]["body"], "Kickoff notes", "{body}");
    assert_eq!(conv["messages"][0]["authorId"], alice.to_string(), "{body}");
    let conv_id = conv["id"].as_str().expect("conv id").to_string();

    // Bob (participant) adds a message.
    let body = gql_post(
        &server,
        &auth(bob, "student"),
        "mutation ($input: AddConversationMessageInput!) { addConversationMessage(input: $input) \
         { conversation { id messages { authorId body } } errors { message } } }",
        json!({ "input": { "conversationId": conv_id, "body": "Sounds good" } }),
    )
    .await;
    let msgs = body["data"]["addConversationMessage"]["conversation"]["messages"]
        .as_array()
        .unwrap();
    assert_eq!(msgs.len(), 1, "only the new message: {body}");
    assert_eq!(msgs[0]["body"], "Sounds good", "{body}");

    // Empty body → payload validation error.
    let body = gql_post(
        &server,
        &auth(bob, "student"),
        "mutation ($input: AddConversationMessageInput!) { addConversationMessage(input: $input) \
         { conversation { id } errors { attribute message } } }",
        json!({ "input": { "conversationId": conv_id, "body": "   " } }),
    )
    .await;
    let payload = &body["data"]["addConversationMessage"];
    assert!(payload["conversation"].is_null(), "{body}");
    assert_eq!(payload["errors"][0]["attribute"], "body", "{body}");

    // A stranger cannot post.
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let body = gql_post(
        &server,
        &auth(stranger, "student"),
        "mutation ($input: AddConversationMessageInput!) { addConversationMessage(input: $input) \
         { conversation { id } } }",
        json!({ "input": { "conversationId": conv_id, "body": "spam" } }),
    )
    .await;
    assert!(body["data"]["addConversationMessage"].is_null(), "{body}");

    // Alice deletes (soft, per-user).
    let body = gql_post(
        &server,
        &auth(alice, "student"),
        "mutation ($input: DeleteConversationInput!) { deleteConversation(input: $input) \
         { success } }",
        json!({ "input": { "conversationId": conv_id } }),
    )
    .await;
    assert_eq!(body["data"]["deleteConversation"]["success"], true, "{body}");

    // Alice's participant row is deleted; Bob still active.
    let (alice_state,): (String,) = sqlx::query_as(
        "SELECT workflow_state FROM conversation_participants \
         WHERE conversation_id = $1 AND user_id = $2",
    )
    .bind(conv_id.parse::<i64>().unwrap())
    .bind(alice)
    .fetch_one(&pool)
    .await
    .expect("alice row");
    assert_eq!(alice_state, "deleted");

    // Deleting again → not found (her row is gone).
    let body = gql_post(
        &server,
        &auth(alice, "student"),
        "mutation ($input: DeleteConversationInput!) { deleteConversation(input: $input) \
         { success } }",
        json!({ "input": { "conversationId": conv_id } }),
    )
    .await;
    assert!(body["data"]["deleteConversation"].is_null(), "{body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// createSubmission / createSubmissionComment / updateSubmissionGrade /
// markSubmissionCommentsRead
// ---------------------------------------------------------------------------

#[tokio::test]
async fn submission_mutations_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "SubCourse").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment: i64 = sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible) \
         VALUES ($1, 'Essay', 100) RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed assignment");

    // Student submits (self).
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: CreateSubmissionInput!) { createSubmission(input: $input) \
         { submission { id assignmentId userId body score submittedAt } errors { message } } }",
        json!({ "input": { "assignmentId": assignment.to_string(), "body": "My essay" } }),
    )
    .await;
    let sub = &body["data"]["createSubmission"]["submission"];
    assert_eq!(sub["body"], "My essay", "{body}");
    assert_eq!(sub["userId"], student.to_string(), "{body}");
    assert!(sub["score"].is_null(), "{body}");
    let sub_id = sub["id"].as_str().expect("sub id").to_string();

    // Resubmit upserts (UNIQUE assignment+user) — same id, new body.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: CreateSubmissionInput!) { createSubmission(input: $input) \
         { submission { id body } errors { message } } }",
        json!({ "input": { "assignmentId": assignment.to_string(), "body": "Revised essay" } }),
    )
    .await;
    assert_eq!(
        body["data"]["createSubmission"]["submission"]["id"],
        sub_id,
        "upsert keeps the row: {body}"
    );
    assert_eq!(
        body["data"]["createSubmission"]["submission"]["body"],
        "Revised essay",
        "{body}"
    );

    // Outsider cannot submit for themselves (not enrolled).
    let body = gql_post(
        &server,
        &auth(outsider, "student"),
        "mutation ($input: CreateSubmissionInput!) { createSubmission(input: $input) \
         { submission { id } } }",
        json!({ "input": { "assignmentId": assignment.to_string(), "body": "x" } }),
    )
    .await;
    assert!(body["data"]["createSubmission"].is_null(), "{body}");

    // Teacher comments on the submission.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateSubmissionCommentInput!) { createSubmissionComment(input: $input) \
         { submissionComment { id submissionId authorId comment } errors { message } } }",
        json!({ "input": { "submissionId": sub_id, "comment": "Good work" } }),
    )
    .await;
    let comment = &body["data"]["createSubmissionComment"]["submissionComment"];
    assert_eq!(comment["comment"], "Good work", "{body}");
    assert_eq!(comment["authorId"], teacher.to_string(), "{body}");

    // Student marks comments read.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: MarkSubmissionCommentsReadInput!) \
         { markSubmissionCommentsRead(input: $input) { success } }",
        json!({ "input": { "submissionId": sub_id } }),
    )
    .await;
    assert_eq!(body["data"]["markSubmissionCommentsRead"]["success"], true, "{body}");

    // Teacher grades with a numeric grade string (Canvas parity: string).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: UpdateSubmissionGradeInput!) { updateSubmissionGrade(input: $input) \
         { submission { id score grade } errors { message } } }",
        json!({ "input": { "submissionId": sub_id, "grade": "85" } }),
    )
    .await;
    let graded = &body["data"]["updateSubmissionGrade"]["submission"];
    assert_eq!(graded["score"], 85.0, "{body}");
    assert_eq!(graded["grade"], "85", "{body}");

    // Student cannot grade.
    let body = gql_post(
        &server,
        &auth(student, "student"),
        "mutation ($input: UpdateSubmissionGradeInput!) { updateSubmissionGrade(input: $input) \
         { submission { id } } }",
        json!({ "input": { "submissionId": sub_id, "score": 100.0 } }),
    )
    .await;
    assert!(body["data"]["updateSubmissionGrade"].is_null(), "{body}");

    // Neither grade nor score → payload validation error.
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: UpdateSubmissionGradeInput!) { updateSubmissionGrade(input: $input) \
         { submission { id } errors { attribute message } } }",
        json!({ "input": { "submissionId": sub_id } }),
    )
    .await;
    let payload = &body["data"]["updateSubmissionGrade"];
    assert!(payload["submission"].is_null(), "{body}");
    assert_eq!(payload["errors"][0]["attribute"], "grade", "{body}");

    // Teacher submits on behalf of the student (grading-role parity).
    let body = gql_post(
        &server,
        &auth(teacher, "teacher"),
        "mutation ($input: CreateSubmissionInput!) { createSubmission(input: $input) \
         { submission { userId body } errors { message } } }",
        json!({ "input": {
            "assignmentId": assignment.to_string(),
            "userId": student.to_string(),
            "body": "On-behalf submission"
        } }),
    )
    .await;
    assert_eq!(
        body["data"]["createSubmission"]["submission"]["body"],
        "On-behalf submission",
        "{body}"
    );
    assert_eq!(
        body["data"]["createSubmission"]["submission"]["userId"],
        student.to_string(),
        "{body}"
    );

    cleanup(&pool, &prefix).await;
}
