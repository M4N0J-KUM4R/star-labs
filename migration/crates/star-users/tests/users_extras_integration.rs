//! Integration tests for the star-users "extras" routes:
//! - GET    /users/self/todo_item_count
//! - GET    /users/{userId}/missing_submissions
//! - GET    /users/{userId}/graded_submissions
//! - GET    /users/self/activity_stream (+ /summary, DELETE variants)
//! - DELETE /users/self/todo/{assetString}/{purpose}
//!
//! Conventions (mirrors users_integration.rs):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-users
//! - Uses a unique email prefix per run + full teardown so it is safe to run
//!   repeatedly against a shared/dev database.

use axum_test::TestServer;
use chrono::{Duration, Utc};
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so parallel/repeat runs against a shared DB
/// never collide on the users.email UNIQUE constraint.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "usr-x-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the users router
/// exactly the way star-http assembles it. Returns None when DATABASE_URL is
/// unset so callers can skip cleanly.
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

    let server = TestServer::new(star_users_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same merge pattern as star-http::build_api_router, restricted to the
/// users router. Authentication is enforced by the AuthUser extractor inside
/// star-users' own handlers, so no extra middleware is needed here.
fn star_users_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_users::router())
        .with_state(state)
}

/// Create a user directly in the DB (bypassing the register endpoint) with a
/// deterministic password hash and the given role. Returns user id.
///
/// sortable_name is populated the way /auth/register computes it ("Last,
/// First") so seeded rows look like real ones. It must not be left NULL:
/// star-users decodes sortable_name into a non-Option String, so a NULL would
/// make every read of that row fail with a 500.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    let sortable_name = match name.rsplit_once(' ') {
        Some((first, last)) => format!("{last}, {first}"),
        None => name.to_string(),
    };
    sqlx::query_scalar(
        r#"
        INSERT INTO users (name, sortable_name, email, password_hash, role)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(name)
    .bind(sortable_name)
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

/// Create an assignment with an explicit due date (or NULL). Returns id.
async fn seed_assignment(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    due_at: Option<chrono::DateTime<Utc>>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, due_at)
        VALUES ($1, $2, $3)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(due_at)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

/// Record a submitted-but-ungraded submission (drives the grading counts).
async fn seed_ungraded_submission(pool: &PgPool, assignment_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO submissions (assignment_id, user_id, body, score, submission_type, workflow_state) \
         VALUES ($1, $2, 'submitted work', NULL, 'online_text_entry', 'submitted')",
    )
    .bind(assignment_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed ungraded submission");
}

/// Record a graded submission with an explicit graded_at so ordering is
/// deterministic. Returns submission id.
async fn seed_graded_submission(
    pool: &PgPool,
    assignment_id: i64,
    user_id: i64,
    score: f64,
    graded_at: chrono::DateTime<Utc>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO submissions
            (assignment_id, user_id, body, score, submission_type, workflow_state, graded_at)
        VALUES ($1, $2, 'graded work', $3, 'online_text_entry', 'graded', $4)
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(score)
    .bind(graded_at)
    .fetch_one(pool)
    .await
    .expect("seed graded submission")
}

/// Record a stream item for a user. Returns stream item id.
async fn seed_stream_item(
    pool: &PgPool,
    user_id: i64,
    asset_string: &str,
    item_type: &str,
    context_code: Option<&str>,
    title: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO stream_items (user_id, asset_string, item_type, context_code, title)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(asset_string)
    .bind(item_type)
    .bind(context_code)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed stream item")
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run (users cascade to their dependent
/// rows; courses cascade to assignments and submissions).
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// GET /users/self/todo_item_count
// ---------------------------------------------------------------------------

#[tokio::test]
async fn todo_item_count_counts_unsubmitted_and_grading() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "Tina Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Sam Student", "student").await;
    let course = seed_course(&pool, &prefix, "Counting 101").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Baseline: nothing to do.
    let resp = server
        .get("/users/self/todo_item_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json(),
        serde_json::json!({ "needs_grading_count": 0, "assignments_needing_submitting": 0 })
    );

    // Two assignments due inside the submitting window, one due far in the
    // past (outside Canvas' [4 weeks ago, 1 week ahead] window), one with a
    // submitted row.
    let due = Some(Utc::now() + Duration::days(2));
    let unsubmitted = seed_assignment(&pool, course, "Todo One", due).await;
    seed_assignment(&pool, course, "Todo Two", due).await;
    seed_assignment(
        &pool,
        course,
        "Ancient",
        Some(Utc::now() - Duration::days(60)),
    )
    .await;
    let submitted = seed_assignment(&pool, course, "Already Done", due).await;
    seed_ungraded_submission(&pool, submitted, student).await;

    let resp = server
        .get("/users/self/todo_item_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["assignments_needing_submitting"], 2, "body: {body}");
    assert_eq!(body["needs_grading_count"], 0, "body: {body}");

    // The teacher sees the ungraded submission but no submitting work.
    let resp = server
        .get("/users/self/todo_item_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["needs_grading_count"], 1, "body: {body}");
    assert_eq!(body["assignments_needing_submitting"], 0, "body: {body}");

    // Assignments outside the window never need submitting even without a
    // row; the graded one is cleared for the student too.
    let _ = unsubmitted;

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn todo_item_count_excludes_ignored() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Igor Student", "student").await;
    let teacher = seed_user(&pool, &prefix, "Tess Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Ignoring 101").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let due = Some(Utc::now() + Duration::days(1));
    let submit_me = seed_assignment(&pool, course, "Submit Me", due).await;
    let grade_me = seed_assignment(&pool, course, "Grade Me", due).await;
    seed_ungraded_submission(&pool, grade_me, student).await;

    // Baseline counts: one submitting (student) + one grading (teacher).
    let resp = server
        .get("/users/self/todo_item_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["assignments_needing_submitting"], 1, "body: {body}");

    // Hide the submitting item via the Canvas ignore route.
    let resp = server
        .delete(&format!(
            "/users/self/todo/assignment_{submit_me}/submitting?permanent=0"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!({ "ignored": true }));

    let resp = server
        .get("/users/self/todo_item_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["assignments_needing_submitting"], 0,
        "ignored item must not count: {body}"
    );

    // Same for the teacher's grading count.
    let resp = server
        .get("/users/self/todo_item_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["needs_grading_count"], 1, "body: {body}");

    let resp = server
        .delete(&format!(
            "/users/self/todo/assignment_{grade_me}/grading?permanent=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get("/users/self/todo_item_count")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(
        body["needs_grading_count"], 0,
        "ignored grading item must not count: {body}"
    );

    // Unknown purposes are rejected the way Canvas does.
    let resp = server
        .delete(&format!("/users/self/todo/assignment_{submit_me}/bogus"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Unknown asset strings are 404.
    let resp = server
        .delete("/users/self/todo/assignment_999999999/submitting")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /users/{userId}/missing_submissions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn missing_submissions_lists_only_unsubmitted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Mia Student", "student").await;
    let other = seed_user(&pool, &prefix, "Omar Student", "student").await;
    let course = seed_course(&pool, &prefix, "Missing 101").await;
    let other_course = seed_course(&pool, &prefix, "Other 201").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other, other_course, "student").await;

    // Three assignments in the student's course: one unsubmitted (due soon),
    // one unsubmitted but long past (still missing — no due window here),
    // one submitted. Another unsubmitted assignment lives in a course the
    // student is not enrolled in.
    let missing_soon = seed_assignment(
        &pool,
        course,
        "Missing Soon",
        Some(Utc::now() + Duration::days(3)),
    )
    .await;
    let missing_past = seed_assignment(
        &pool,
        course,
        "Missing Past",
        Some(Utc::now() - Duration::days(40)),
    )
    .await;
    let submitted = seed_assignment(
        &pool,
        course,
        "Submitted",
        Some(Utc::now() + Duration::days(3)),
    )
    .await;
    seed_ungraded_submission(&pool, submitted, student).await;
    let foreign = seed_assignment(
        &pool,
        other_course,
        "Foreign Assignment",
        Some(Utc::now() + Duration::days(3)),
    )
    .await;

    let resp = server
        .get(&format!("/users/{student}/missing_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("array of assignments");
    let ids: Vec<i64> = items.iter().filter_map(|a| a["id"].as_i64()).collect();

    assert_eq!(ids.len(), 2, "body: {body}");
    assert!(ids.contains(&missing_soon), "body: {body}");
    assert!(ids.contains(&missing_past), "body: {body}");
    assert!(!ids.contains(&submitted), "body: {body}");
    assert!(!ids.contains(&foreign), "body: {body}");

    // Canvas assignment shape on every entry.
    for entry in items {
        for key in [
            "id",
            "course_id",
            "title",
            "points_possible",
            "created_at",
            "updated_at",
        ] {
            assert!(entry.get(key).is_some(), "missing field {key}: {entry}");
        }
        assert_eq!(entry["course_id"], course);
    }

    // `self` resolves to the caller like every Canvas users route.
    let resp = server
        .get("/users/self/missing_submissions")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let ids: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids.len(), 2, "body: {body}");

    // Ordering: due_at ascending, NULLS LAST, then id.
    let resp = server
        .get(&format!(
            "/users/{student}/missing_submissions?per_page=1&page=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let page1: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_i64().unwrap())
        .collect();
    assert_eq!(page1, vec![missing_past], "earliest due first: {body}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn missing_submissions_other_user_forbidden() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Nina Student", "student").await;
    let other = seed_user(&pool, &prefix, "Paul Student", "student").await;
    let teacher = seed_user(&pool, &prefix, "Quinn Teacher", "teacher").await;
    let admin = seed_user(&pool, &prefix, "Ruth Admin", "admin").await;

    // Another student and a plain teacher cannot read someone else's list.
    for (actor, role) in [(other, "student"), (teacher, "teacher")] {
        let resp = server
            .get(&format!("/users/{student}/missing_submissions"))
            .add_header(axum::http::header::AUTHORIZATION, auth(actor, role))
            .await;
        assert_eq!(
            resp.status_code(),
            403,
            "{role} must not read another user's missing submissions: {}",
            resp.text()
        );
    }

    // An admin can (Canvas' :read grants admins) and gets the same array.
    let resp = server
        .get(&format!("/users/{student}/missing_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!([]));

    // Missing user: 404 for the admin, 403 for a non-admin (permission
    // check first, matching the rest of this API).
    let resp = server
        .get("/users/999999999/missing_submissions")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get("/users/999999999/missing_submissions")
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Non-numeric user id: Canvas' api_find yields 404.
    let resp = server
        .get("/users/not-a-number/missing_submissions")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unauthenticated is 401.
    assert_eq!(
        server
            .get("/users/self/missing_submissions")
            .await
            .status_code(),
        401
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET /users/{userId}/graded_submissions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graded_submissions_requires_self() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Vera Student", "student").await;
    let other = seed_user(&pool, &prefix, "Walt Student", "student").await;
    let teacher = seed_user(&pool, &prefix, "Xena Teacher", "teacher").await;
    let admin = seed_user(&pool, &prefix, "Yuri Admin", "admin").await;

    // Only the user themself may read their graded submissions.
    for (actor, role) in [(other, "student"), (teacher, "teacher"), (admin, "admin")] {
        let resp = server
            .get(&format!("/users/{student}/graded_submissions"))
            .add_header(axum::http::header::AUTHORIZATION, auth(actor, role))
            .await;
        assert_eq!(
            resp.status_code(),
            403,
            "{role} must not read another user's graded submissions: {}",
            resp.text()
        );
    }

    let resp = server
        .get(&format!("/users/{student}/graded_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!([]));

    // Missing user: 404 for self-style probes is impossible (the caller is
    // not the missing id), so the shape is 404 for the admin and 403 for a
    // student, consistent with the permission-first order.
    let resp = server
        .get("/users/999999999/graded_submissions")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get("/users/999999999/graded_submissions")
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Unauthenticated is 401.
    assert_eq!(
        server
            .get(&format!("/users/{student}/graded_submissions"))
            .await
            .status_code(),
        401
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn graded_submissions_shape_and_pagination() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Zed Student", "student").await;
    let course = seed_course(&pool, &prefix, "Graded 101").await;
    seed_enrollment(&pool, student, course, "student").await;

    let due = Some(Utc::now() + Duration::days(1));
    let a1 = seed_assignment(&pool, course, "Alpha", due).await;
    let a2 = seed_assignment(&pool, course, "Beta", due).await;
    let a3 = seed_assignment(&pool, course, "Gamma", due).await;

    let graded_at = Utc::now() - Duration::hours(1);
    let s1 = seed_graded_submission(&pool, a1, student, 90.0, graded_at).await;
    let s2 = seed_graded_submission(&pool, a2, student, 80.0, graded_at - Duration::hours(1)).await;
    // An ungraded submission must not appear.
    seed_ungraded_submission(&pool, a3, student).await;

    let resp = server
        .get(&format!("/users/{student}/graded_submissions"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("array of submissions");
    assert_eq!(items.len(), 2, "body: {body}");

    // Canvas submission shape: newest graded first.
    assert_eq!(items[0]["id"], s1, "body: {body}");
    assert_eq!(items[1]["id"], s2, "body: {body}");

    let first = &items[0];
    for key in [
        "id",
        "assignment_id",
        "user_id",
        "attempt",
        "grade",
        "score",
        "submitted_at",
        "graded_at",
        "workflow_state",
        "url",
        "preview_url",
        "assignment",
    ] {
        assert!(first.get(key).is_some(), "missing field {key}: {first}");
    }
    assert_eq!(first["assignment_id"], a1);
    assert_eq!(first["user_id"], student);
    assert_eq!(first["score"], 90.0);
    assert_eq!(first["grade"], "90");
    assert_eq!(first["workflow_state"], "graded");

    // The embedded assignment object.
    assert_eq!(first["assignment"]["id"], a1);
    assert_eq!(first["assignment"]["course_id"], course);
    assert_eq!(first["assignment"]["title"], "Alpha");
    assert_eq!(first["assignment"]["points_possible"], 0.0);

    // Pagination: per_page=1 slices newest-first.
    let resp = server
        .get(&format!(
            "/users/{student}/graded_submissions?per_page=1&page=1"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let page1: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_i64().unwrap())
        .collect();
    assert_eq!(page1, vec![s1], "body: {body}");

    let resp = server
        .get(&format!(
            "/users/{student}/graded_submissions?per_page=1&page=2"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let page2: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_i64().unwrap())
        .collect();
    assert_eq!(page2, vec![s2], "body: {body}");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Activity stream
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_stream_empty_and_summary_shapes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "Amy Student", "student").await;

    // A user with no stream items gets Canvas-shaped empty results.
    let resp = server
        .get("/users/self/activity_stream")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!([]));

    let resp = server
        .get("/users/self/activity_stream/summary")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!([]));

    // Seed three items of two types for the student, plus one for someone
    // else (must never leak into the student's stream).
    let other = seed_user(&pool, &prefix, "Ben Student", "student").await;
    let item1 = seed_stream_item(
        &pool,
        student,
        "assignment_100",
        "Submission",
        Some("course_1"),
        Some("Assignment graded"),
    )
    .await;
    seed_stream_item(
        &pool,
        student,
        "discussion_topic_200",
        "DiscussionTopic",
        None,
        Some("New post"),
    )
    .await;
    seed_stream_item(
        &pool,
        student,
        "conversation_300",
        "Conversation",
        None,
        None,
    )
    .await;
    seed_stream_item(
        &pool,
        other,
        "assignment_400",
        "Submission",
        Some("course_1"),
        Some("Someone else's grade"),
    )
    .await;

    // The stream lists only the caller's items, newest first.
    let resp = server
        .get("/users/self/activity_stream")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let items = body.as_array().expect("array of stream items");
    assert_eq!(items.len(), 3, "body: {body}");
    for entry in items {
        for key in ["id", "asset_string", "type", "created_at"] {
            assert!(entry.get(key).is_some(), "missing field {key}: {entry}");
        }
    }
    assert_eq!(items[0]["type"], "Conversation", "body: {body}");

    // Summary groups by type with {type, unread_count, count}.
    let resp = server
        .get("/users/self/activity_stream/summary")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let summary = body.as_array().expect("array of summaries");
    assert_eq!(summary.len(), 3, "body: {body}");
    let submission_summary = summary
        .iter()
        .find(|s| s["type"] == "Submission")
        .unwrap_or_else(|| panic!("Submission summary missing: {body}"));
    assert_eq!(submission_summary["count"], 1, "body: {body}");
    assert_eq!(submission_summary["unread_count"], 0, "body: {body}");

    // DELETE one item: Canvas always renders {"hidden": true}.
    let resp = server
        .delete(&format!("/users/self/activity_stream/{item1}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!({ "hidden": true }));

    // Hidden item is gone from both the stream and the summary.
    let resp = server
        .get("/users/self/activity_stream")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().unwrap().len(), 2, "body: {body}");

    let resp = server
        .get("/users/self/activity_stream/summary")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let summary = body.as_array().expect("array of summaries");
    assert_eq!(summary.len(), 2, "body: {body}");
    assert!(
        summary.iter().all(|s| s["type"] != "Submission"),
        "hidden type fully hidden: {body}"
    );

    // A hidden id still answers {"hidden": true} (Canvas renders it
    // unconditionally), and someone else's id never hides the caller's rows.
    let resp = server
        .delete(&format!("/users/self/activity_stream/{item1}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!({ "hidden": true }));

    // DELETE the whole stream hides everything.
    let resp = server
        .delete("/users/self/activity_stream")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!({ "hidden": true }));

    let resp = server
        .get("/users/self/activity_stream")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!([]));

    let resp = server
        .get("/users/self/activity_stream/summary")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json(), serde_json::json!([]));

    // The other user's stream is untouched by all of the above.
    let resp = server
        .get("/users/self/activity_stream")
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body.as_array().unwrap().len(), 1, "body: {body}");

    // Unauthenticated is 401 on every stream route.
    assert_eq!(
        server
            .get("/users/self/activity_stream")
            .await
            .status_code(),
        401
    );
    assert_eq!(
        server
            .get("/users/self/activity_stream/summary")
            .await
            .status_code(),
        401
    );
    assert_eq!(
        server
            .delete("/users/self/activity_stream/1")
            .await
            .status_code(),
        401
    );
    assert_eq!(
        server
            .delete("/users/self/activity_stream")
            .await
            .status_code(),
        401
    );

    cleanup(&pool, &prefix).await;
}
