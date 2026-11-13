//! Integration tests for the Canvas peer reviews API in star-assignments
//! (PeerReviewsApiController port).
//!
//! Covers the Phase 5 routes:
//! - GET    /courses/{courseId}/assignments/{assignmentId}/peer_reviews
//! - GET    /courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews
//! - POST   /courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews
//! - DELETE /courses/{courseId}/assignments/{assignmentId}/submissions/{submissionId}/peer_reviews
//!
//! Skipped cleanly when DATABASE_URL is not set, mirroring
//! submissions_api_integration.rs. Uses a unique email/course prefix per run
//! and full teardown so it is safe to run repeatedly against a shared
//! database.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "prapi-tests-{}-{}",
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

    let server = TestServer::new(peer_reviews_router(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

fn peer_reviews_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_assignments::router())
        .with_state(state)
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    // Populate sortable_name the way /auth/register computes it ("Last,
    // First"): star-users decodes it as a non-Option String, and NULL rows
    // leak into the shared test database break its GET /users.
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, 'x', $4)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(sortable_name)
    .bind(format!("{prefix}-{name}@test.local"))
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

/// GET a path with a bearer token, asserting 200 and returning the JSON.
async fn get_json(server: &TestServer, token: &str, path: &str) -> Value {
    let resp = server
        .get(path)
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(resp.status_code(), 200, "GET {path}: {}", resp.text());
    resp.json()
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str, points: f64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, points_possible)
        VALUES ($1, $2, $3)
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

async fn seed_submission(pool: &PgPool, assignment_id: i64, user_id: i64, body: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO submissions (assignment_id, user_id, body)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(body)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let user_ids = "SELECT id FROM users WHERE email LIKE $1";
    let course_ids = "SELECT id FROM courses WHERE course_code LIKE $1";
    let _ = sqlx::query(&format!(
        "DELETE FROM submissions WHERE user_id IN ({user_ids}) \
         OR assignment_id IN (SELECT id FROM assignments WHERE course_id IN ({course_ids}))",
    ))
    .bind(format!("{prefix}%@test.local"))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM assignments WHERE course_id IN ({course_ids})",
    ))
    .bind(format!("{prefix}%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(&format!(
        "DELETE FROM enrollments WHERE user_id IN ({user_ids})",
    ))
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
// GET peer_reviews (index): teacher-sees-all vs student-sees-own
// ---------------------------------------------------------------------------

#[tokio::test]
async fn peer_reviews_index_teacher_sees_all_student_sees_own() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let carol = seed_user(&pool, &prefix, "Carol", "student").await;
    let course = seed_course(&pool, &prefix, "PeerIndex").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    seed_enrollment(&pool, carol, course, "student").await;

    let assignment = seed_assignment(&pool, course, "Essay", 10.0).await;
    let alice_sub = seed_submission(&pool, assignment, alice, "alice answer").await;
    let bob_sub = seed_submission(&pool, assignment, bob, "bob answer").await;

    // Bob reviews Alice; Carol reviews Alice; Alice reviews Bob.
    seed_review(&pool, assignment, alice_sub, alice, bob).await;
    seed_review(&pool, assignment, alice_sub, alice, carol).await;
    seed_review(&pool, assignment, bob_sub, bob, alice).await;

    let teacher_token = auth(teacher, "teacher");

    // Teachers see every review on the assignment.
    let all = get_json(
        &server,
        &teacher_token,
        &format!("/courses/{course}/assignments/{assignment}/peer_reviews"),
    )
    .await;
    let entries = all.as_array().expect("array");
    assert_eq!(entries.len(), 3, "teacher sees all: {all}");
    assert!(entries
        .iter()
        .all(|entry| entry["asset_type"] == "Submission"));
    assert!(entries
        .iter()
        .any(|entry| entry["user_id"] == alice && entry["assessor_id"] == bob));
    assert!(entries
        .iter()
        .any(|entry| entry["user_id"] == alice && entry["assessor_id"] == carol));
    assert!(entries
        .iter()
        .any(|entry| entry["user_id"] == bob && entry["assessor_id"] == alice));
    assert!(entries
        .iter()
        .all(|entry| entry["workflow_state"] == "assigned"));

    // Alice (assessee) only sees reviews of her own submissions.
    let own = get_json(
        &server,
        &auth(alice, "student"),
        &format!("/courses/{course}/assignments/{assignment}/peer_reviews"),
    )
    .await;
    let own_entries = own.as_array().expect("array");
    assert_eq!(own_entries.len(), 2, "alice sees own two: {own}");
    assert!(own_entries
        .iter()
        .all(|entry| entry["user_id"] == alice && entry["asset_id"] == alice_sub));

    // The submission-scoped route narrows teachers to one asset
    // (Canvas for_asset).
    let scoped = get_json(
        &server,
        &teacher_token,
        &format!("/courses/{course}/assignments/{assignment}/submissions/{alice_sub}/peer_reviews"),
    )
    .await;
    let scoped_entries = scoped.as_array().expect("array");
    assert_eq!(scoped_entries.len(), 2, "{scoped}");
    assert!(scoped_entries
        .iter()
        .all(|entry| entry["asset_id"] == alice_sub));

    // include[]=user adds the Canvas user display objects.
    let with_user = get_json(
        &server,
        &teacher_token,
        &format!(
            "/courses/{course}/assignments/{assignment}/submissions/{alice_sub}/peer_reviews?include[]=user"
        ),
    )
    .await;
    let with_user_entries = with_user.as_array().expect("array");
    assert_eq!(with_user_entries.len(), 2);
    let bob_review = with_user_entries
        .iter()
        .find(|entry| entry["assessor_id"] == bob)
        .expect("bob review");
    assert_eq!(bob_review["user"]["id"], alice);
    assert_eq!(bob_review["user"]["display_name"], "Alice");
    assert_eq!(bob_review["assessor"]["id"], bob);
    assert_eq!(bob_review["assessor"]["display_name"], "Bob");

    // include[]=submission_comments returns the reviewed submission's
    // comments (Canvas attaches the asset's comments).
    seed_comment(&pool, alice_sub, teacher, "please review carefully").await;
    let with_comments = get_json(
        &server,
        &teacher_token,
        &format!(
            "/courses/{course}/assignments/{assignment}/submissions/{alice_sub}/peer_reviews?include[]=submission_comments"
        ),
    )
    .await;
    let with_comments_entries = with_comments.as_array().expect("array");
    let bob_review = with_comments_entries
        .iter()
        .find(|entry| entry["assessor_id"] == bob)
        .expect("bob review");
    let comments = bob_review["submission_comments"].as_array().expect("array");
    assert_eq!(comments.len(), 1, "{bob_review}");
    assert_eq!(comments[0]["comment"], "please review carefully");
    assert_eq!(comments[0]["author_id"], teacher);

    // Unenrolled users are rejected (403).
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let forbidden = server
        .get(&format!(
            "/courses/{course}/assignments/{assignment}/peer_reviews"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403, "{}", forbidden.text());

    // An unknown assignment in a known course is a 404.
    let missing = server
        .get(&format!(
            "/courses/{course}/assignments/999999/peer_reviews"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .await;
    assert_eq!(missing.status_code(), 404, "{}", missing.text());

    cleanup(&pool, &prefix).await;
}

/// Insert an assessment request directly (reviewer assigned to assessee).
async fn seed_review(
    pool: &PgPool,
    assignment_id: i64,
    submission_id: i64,
    assessee_id: i64,
    reviewer_id: i64,
) -> i64 {
    // Canvas assign_peer_review also creates the reviewer's own submission.
    let reviewer_sub: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO submissions (assignment_id, user_id)
        VALUES ($1, $2)
        ON CONFLICT (assignment_id, user_id)
        DO UPDATE SET updated_at = submissions.updated_at
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(reviewer_id)
    .fetch_one(pool)
    .await
    .expect("seed reviewer submission");

    sqlx::query_scalar::<_, i64>(
        r#"
        INSERT INTO assessment_requests (user_id, assessor_id, asset_id, asset_type, workflow_state)
        VALUES ($1, $2, $3, 'Submission', 'assigned')
        RETURNING id
        "#,
    )
    .bind(assessee_id)
    .bind(reviewer_id)
    .bind(submission_id)
    .fetch_one(pool)
    .await
    .expect("seed assessment request");

    reviewer_sub
}

async fn seed_comment(pool: &PgPool, submission_id: i64, author_id: i64, comment: &str) {
    sqlx::query(
        "INSERT INTO submission_comments (submission_id, author_id, comment) VALUES ($1, $2, $3)",
    )
    .bind(submission_id)
    .bind(author_id)
    .bind(comment)
    .execute(pool)
    .await
    .expect("seed comment");
}

// ---------------------------------------------------------------------------
// POST peer_reviews (create)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn peer_review_create_assigns_reviewer_and_bumps_count() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let course = seed_course(&pool, &prefix, "PeerCreate").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;

    let assignment = seed_assignment(&pool, course, "Essay", 10.0).await;
    let alice_sub = seed_submission(&pool, assignment, alice, "alice answer").await;

    let path =
        format!("/courses/{course}/assignments/{assignment}/submissions/{alice_sub}/peer_reviews");
    let teacher_token = auth(teacher, "teacher");

    // Happy path: teacher assigns Bob as Alice's reviewer (JSON body).
    let created = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "user_id": bob }))
        .await;
    assert_eq!(created.status_code(), 201, "{}", created.text());
    let created: Value = created.json();
    assert_eq!(created["user_id"], alice);
    assert_eq!(created["assessor_id"], bob);
    assert_eq!(created["asset_id"], alice_sub);
    assert_eq!(created["asset_type"], "Submission");
    assert_eq!(created["workflow_state"], "assigned");
    assert!(created["id"].is_i64());

    // The reviewer also gets a submission row (Canvas assign_peer_review).
    let bob_submission: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM submissions WHERE assignment_id = $1 AND user_id = $2",
    )
    .bind(assignment)
    .bind(bob)
    .fetch_one(&pool)
    .await
    .expect("count bob submissions");
    assert_eq!(bob_submission, 1, "reviewer submission created lazily");

    // peer_review_count on the assignment reflects the assigned reviews.
    let count: i32 = sqlx::query_scalar("SELECT peer_review_count FROM assignments WHERE id = $1")
        .bind(assignment)
        .fetch_one(&pool)
        .await
        .expect("read peer_review_count");
    assert_eq!(count, 1, "peer_review_count bumped on create");

    // Creating the same (submission, reviewer) pair again is idempotent
    // (Canvas assigns the reviewer again without duplicating).
    let again = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "user_id": bob }))
        .await;
    assert_eq!(again.status_code(), 201, "{}", again.text());
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM assessment_requests WHERE asset_id = $1 AND assessor_id = $2",
    )
    .bind(alice_sub)
    .bind(bob)
    .fetch_one(&pool)
    .await
    .expect("count reviews");
    assert_eq!(total, 1, "ON CONFLICT keeps a single review");

    // Form-encoded body is accepted too.
    let carol = seed_user(&pool, &prefix, "Carol", "student").await;
    seed_enrollment(&pool, carol, course, "student").await;
    let form_created = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .text(format!("user_id={carol}"))
        .await;
    assert_eq!(form_created.status_code(), 201, "{}", form_created.text());
    assert_eq!(form_created.json::<Value>()["assessor_id"], carol);

    // reviewer == assessee → 400 "Create failed".
    let self_review = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "user_id": alice }))
        .await;
    assert_eq!(self_review.status_code(), 400, "{}", self_review.text());
    assert!(self_review.text().contains("Create failed"));

    // A non-teacher cannot assign reviews (403).
    let student_create = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .json(&json!({ "user_id": bob }))
        .await;
    assert_eq!(
        student_create.status_code(),
        403,
        "{}",
        student_create.text()
    );

    // A reviewer without an active student enrollment in the course is a 404
    // (Canvas peer_review_assets resolves among visible students).
    let unenrolled = seed_user(&pool, &prefix, "Unenrolled", "student").await;
    let not_student = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "user_id": unenrolled }))
        .await;
    assert_eq!(not_student.status_code(), 404, "{}", not_student.text());

    // The teacher (a valid user but not a student) is also a 404 reviewer.
    let teacher_reviewer = server
        .post(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "user_id": teacher }))
        .await;
    assert_eq!(
        teacher_reviewer.status_code(),
        404,
        "{}",
        teacher_reviewer.text()
    );

    // A submission from another assignment does not resolve (404).
    let other_assignment = seed_assignment(&pool, course, "Other", 5.0).await;
    let other_sub = seed_submission(&pool, other_assignment, alice, "other").await;
    let wrong_asset = server
        .post(&format!(
            "/courses/{course}/assignments/{assignment}/submissions/{other_sub}/peer_reviews"
        ))
        .add_header(axum::http::header::AUTHORIZATION, teacher_token)
        .json(&json!({ "user_id": bob }))
        .await;
    assert_eq!(wrong_asset.status_code(), 404, "{}", wrong_asset.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// DELETE peer_reviews (destroy)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn peer_review_destroy_removes_review_and_recomputes_count() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let carol = seed_user(&pool, &prefix, "Carol", "student").await;
    let course = seed_course(&pool, &prefix, "PeerDestroy").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, alice, course, "student").await;
    seed_enrollment(&pool, bob, course, "student").await;
    seed_enrollment(&pool, carol, course, "student").await;

    let assignment = seed_assignment(&pool, course, "Essay", 10.0).await;
    let alice_sub = seed_submission(&pool, assignment, alice, "alice answer").await;
    seed_review(&pool, assignment, alice_sub, alice, bob).await;
    seed_review(&pool, assignment, alice_sub, alice, carol).await;
    sqlx::query("UPDATE assignments SET peer_review_count = 2 WHERE id = $1")
        .bind(assignment)
        .execute(&pool)
        .await
        .expect("seed count");

    let path =
        format!("/courses/{course}/assignments/{assignment}/submissions/{alice_sub}/peer_reviews");
    let teacher_token = auth(teacher, "teacher");

    // Happy path: delete Bob's review.
    let deleted = server
        .delete(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "user_id": bob }))
        .await;
    assert_eq!(deleted.status_code(), 200, "{}", deleted.text());
    let deleted: Value = deleted.json();
    assert_eq!(deleted["assessor_id"], bob);
    assert_eq!(deleted["user_id"], alice);
    assert_eq!(deleted["asset_id"], alice_sub);

    // The review is gone and the count was recomputed.
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM assessment_requests WHERE asset_id = $1")
            .bind(alice_sub)
            .fetch_one(&pool)
            .await
            .expect("count remaining");
    assert_eq!(remaining, 1);
    let count: i32 = sqlx::query_scalar("SELECT peer_review_count FROM assignments WHERE id = $1")
        .bind(assignment)
        .fetch_one(&pool)
        .await
        .expect("read peer_review_count");
    assert_eq!(count, 1, "peer_review_count recomputed on delete");

    // Deleting the same (submission, reviewer, assessee) triple again is a
    // 400 "Delete failed".
    let missing = server
        .delete(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "user_id": bob }))
        .await;
    assert_eq!(missing.status_code(), 400, "{}", missing.text());
    assert!(missing.text().contains("Delete failed"));

    // A reviewer who never had this review is also a 400.
    let stranger = server
        .delete(&path)
        .add_header(axum::http::header::AUTHORIZATION, teacher_token.clone())
        .json(&json!({ "user_id": alice }))
        .await;
    assert_eq!(stranger.status_code(), 400, "{}", stranger.text());

    // Non-teachers cannot delete.
    let student_delete = server
        .delete(&path)
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .json(&json!({ "user_id": carol }))
        .await;
    assert_eq!(
        student_delete.status_code(),
        403,
        "{}",
        student_delete.text()
    );

    cleanup(&pool, &prefix).await;
}
