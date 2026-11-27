//! Integration tests for the Canvas direct-outcome routes
//! (`canvas-src/app/controllers/outcomes_api_controller.rb`):
//!
//! - `GET /outcomes/{id}` answers the Canvas `Outcome` JSON (title, url,
//!   mastery_points with the derived ratings, calculation_method) and `404`
//!   for an unknown or soft-deleted outcome;
//! - `PUT /outcomes/{id}` requires the update right (admin), updates the
//!   permitted fields and answers `400` for a blank title;
//! - `DELETE /outcomes/{id}` soft-deletes (workflow_state `deleted`) and
//!   answers the deleted outcome's JSON; a later show is `404`;
//! - `GET /courses/{id}/outcome_alignments`:
//!   - `assignment_id` (grader) lists the assignment's outcome alignments —
//!     Canvas models them as `content_tags` rows with
//!     `tag_type = 'learning_outcome'`, the outcome in
//!     `content_tags.learning_outcome_id` and the assignment in
//!     `content_id` (`fetch_assignment_alignments`);
//!   - `student_id` returns the aligned assignments **with a submission**
//!     for that student (`INNER JOIN submissions`, `outcome_alignments_for_student`);
//!     unpublished assignments are hidden from callers without grade rights;
//!   - neither parameter → `400` with the "student_id or assignment_id is
//!     required" body; a grader-less assignment query → `401`; another
//!     student's id → `401`; an assignment outside the course → `404`.
//!
//! Fleet rule: dedicated users, accounts and courses per test, every seeded
//! row is cleaned up. Skipped cleanly when DATABASE_URL is unset.

use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// The module is included by path so the suite compiles against the handler
// source itself (same pattern as outcomes_api_integration.rs). It resolves
// `crate::is_admin`, so the test crate root provides the same helper the lib
// root provides.
#[path = "../src/outcomes_direct_api.rs"]
mod outcomes_direct_api;

/// Mirror of `star-outcomes`' private `is_admin` (the included module imports
/// it from the crate root).
fn is_admin(auth: &star_auth::AuthUser) -> bool {
    auth.role == "admin"
}

/// A running test server plus the pool it uses.
struct TestApp {
    base_url: String,
    client: reqwest::Client,
    pool: PgPool,
}

impl TestApp {
    /// A bearer token for the user.
    fn token(&self, user_id: i64) -> String {
        star_auth::create_token(user_id, "student", "test-jwt-secret").expect("mint token")
    }
}

/// Build the server. Returns `None` when `DATABASE_URL` is not set.
async fn setup() -> Option<TestApp> {
    let db_url = std::env::var("DATABASE_URL").ok()?;
    if db_url.trim().is_empty() {
        println!("SKIP: DATABASE_URL is not set");
        return None;
    }

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
    let app: axum::Router = axum::Router::new()
        .nest("/api/v1", outcomes_direct_api::outcomes_api_routes())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the test listener");
    let address = listener.local_addr().expect("read the listener address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    Some(TestApp {
        base_url: format!("http://{address}"),
        client: reqwest::Client::new(),
        pool,
    })
}

/// The ids of one seeded fixture.
struct Fixture {
    account_id: i64,
    course_id: i64,
    admin_id: i64,
    teacher_id: i64,
    student_id: i64,
    other_student_id: i64,
    outcome_id: i64,
    /// The published assignment; the student submitted it.
    assignment_id: i64,
    /// The unpublished assignment; the student submitted it too.
    unpublished_assignment_id: i64,
    /// An aligned assignment without any submission.
    unsubmitted_assignment_id: i64,
}

/// Seed an account, a course, an admin, a teacher, two students, one outcome,
/// three aligned assignments and the student's submissions.
///
/// The alignment rows follow the Canvas shape: `content_tags.tag_type =
/// 'learning_outcome'`, `learning_outcome_id` = the outcome, `content_id` =
/// the aligned assignment.
async fn seed(pool: &PgPool, prefix: &str) -> Fixture {
    let account_id: i64 =
        sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
            .bind(format!("{prefix}-account"))
            .fetch_one(pool)
            .await
            .expect("seed account");

    let course_id: i64 = sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-course"))
    .bind(format!("{prefix}-course"))
    .bind(account_id)
    .fetch_one(pool)
    .await
    .expect("seed course");

    let admin_id = seed_user(pool, prefix, "admin", "admin").await;
    let teacher_id = seed_user(pool, prefix, "teacher", "teacher").await;
    let student_id = seed_user(pool, prefix, "student", "student").await;
    let other_student_id = seed_user(pool, prefix, "other-student", "student").await;
    seed_enrollment(pool, teacher_id, course_id, "teacher").await;
    seed_enrollment(pool, student_id, course_id, "student").await;
    seed_enrollment(pool, other_student_id, course_id, "student").await;

    let outcome_id: i64 = sqlx::query_scalar(
        "INSERT INTO learning_outcomes
            (title, description, display_name, vendor_guid, mastery_points,
             calculation_method, workflow_state)
         VALUES ($1, 'a canvas outcome', 'friendly name', 'vendor-86', 4, 'highest', 'active')
         RETURNING id",
    )
    .bind(format!("{prefix}-outcome"))
    .fetch_one(pool)
    .await
    .expect("seed outcome");

    let published: i64 = sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible, workflow_state)
         VALUES ($1, $2, 10, 'published') RETURNING id",
    )
    .bind(course_id)
    .bind(format!("{prefix}-published"))
    .fetch_one(pool)
    .await
    .expect("seed published assignment");

    let unpublished: i64 = sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible, workflow_state)
         VALUES ($1, $2, 10, 'unpublished') RETURNING id",
    )
    .bind(course_id)
    .bind(format!("{prefix}-unpublished"))
    .fetch_one(pool)
    .await
    .expect("seed unpublished assignment");

    let unsubmitted: i64 = sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible, workflow_state)
         VALUES ($1, $2, 10, 'published') RETURNING id",
    )
    .bind(course_id)
    .bind(format!("{prefix}-unsubmitted"))
    .fetch_one(pool)
    .await
    .expect("seed unsubmitted assignment");

    for assignment_id in [published, unpublished, unsubmitted] {
        sqlx::query(
            "INSERT INTO content_tags
                (content_id, content_type, context_id, context_type, title, tag_type,
                 learning_outcome_id, workflow_state, root_account_id)
             VALUES ($1, 'Assignment', $2, 'Course', $3, 'learning_outcome',
                     $4, 'active', $5)",
        )
        .bind(assignment_id)
        .bind(course_id)
        .bind(format!("{prefix}-alignment"))
        .bind(outcome_id)
        .bind(account_id)
        .execute(pool)
        .await
        .expect("seed the alignment content tag");
    }

    // The student submitted the published and the unpublished assignment.
    for assignment_id in [published, unpublished] {
        sqlx::query(
            "INSERT INTO submissions (assignment_id, user_id, score, workflow_state)
             VALUES ($1, $2, 8, 'submitted')",
        )
        .bind(assignment_id)
        .bind(student_id)
        .execute(pool)
        .await
        .expect("seed the student submission");
    }

    Fixture {
        account_id,
        course_id,
        admin_id,
        teacher_id,
        student_id,
        other_student_id,
        outcome_id,
        assignment_id: published,
        unpublished_assignment_id: unpublished,
        unsubmitted_assignment_id: unsubmitted,
    }
}

/// Remove every seeded row (children first; `submissions` cascade with their
/// assignment but the content tags are deleted explicitly).
async fn cleanup(pool: &PgPool, fixture: &Fixture) {
    sqlx::query("DELETE FROM content_tags WHERE context_id = $1 AND context_type = 'Course'")
        .bind(fixture.course_id)
        .execute(pool)
        .await
        .expect("cleanup content tags");
    sqlx::query("DELETE FROM submissions WHERE assignment_id = ANY($1)")
        .bind(vec![
            fixture.assignment_id,
            fixture.unpublished_assignment_id,
            fixture.unsubmitted_assignment_id,
        ])
        .execute(pool)
        .await
        .expect("cleanup submissions");
    sqlx::query("DELETE FROM assignments WHERE id = ANY($1)")
        .bind(vec![
            fixture.assignment_id,
            fixture.unpublished_assignment_id,
            fixture.unsubmitted_assignment_id,
        ])
        .execute(pool)
        .await
        .expect("cleanup assignments");
    sqlx::query("DELETE FROM learning_outcomes WHERE id = $1")
        .bind(fixture.outcome_id)
        .execute(pool)
        .await
        .expect("cleanup outcome");
    sqlx::query("DELETE FROM enrollments WHERE course_id = $1")
        .bind(fixture.course_id)
        .execute(pool)
        .await
        .expect("cleanup enrollments");
    sqlx::query("DELETE FROM courses WHERE id = $1")
        .bind(fixture.course_id)
        .execute(pool)
        .await
        .expect("cleanup course");
    sqlx::query("DELETE FROM accounts WHERE id = $1")
        .bind(fixture.account_id)
        .execute(pool)
        .await
        .expect("cleanup account");
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(vec![
            fixture.admin_id,
            fixture.teacher_id,
            fixture.student_id,
            fixture.other_student_id,
        ])
        .execute(pool)
        .await
        .expect("cleanup users");
}

/// Seed a user.
async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role)
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}@test.local"))
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

/// Seed an enrollment.
async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state)
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// A unique prefix for the seeded rows.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "od-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
        n
    )
}

// ---------------------------------------------------------------------------
// GET /outcomes/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcome_show_answers_the_canvas_json_and_404s_when_deleted() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/outcomes/{}",
            app.base_url, fixture.outcome_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("show the outcome");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["id"], fixture.outcome_id);
    assert_eq!(body["title"], format!("{prefix}-outcome"));
    assert_eq!(body["description"], "a canvas outcome");
    assert_eq!(body["display_name"], "friendly name");
    assert_eq!(body["vendor_guid"], "vendor-86");
    assert_eq!(
        body["url"],
        format!("/api/v1/outcomes/{}", fixture.outcome_id)
    );
    assert_eq!(body["mastery_points"], 4.0);
    // The derived criterion ratings (mastery_points, points+1, points/2, 0).
    let ratings = body["ratings"].as_array().expect("the ratings array");
    assert_eq!(ratings.len(), 4);
    assert_eq!(ratings[0]["description"], "Exceeds Mastery");
    assert_eq!(ratings[0]["points"], 5.0);
    assert_eq!(ratings[1]["description"], "Mastery");
    assert_eq!(ratings[1]["points"], 4.0);
    assert_eq!(body["calculation_method"], "highest");

    // Unknown id → 404.
    let missing = app
        .client
        .get(format!("{}/api/v1/outcomes/999999", app.base_url))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("show the missing outcome");
    assert_eq!(missing.status(), 404);

    // A soft-deleted outcome is not readable (`LearningOutcome.active.find`).
    sqlx::query("UPDATE learning_outcomes SET workflow_state = 'deleted' WHERE id = $1")
        .bind(fixture.outcome_id)
        .execute(&app.pool)
        .await
        .expect("soft-delete the outcome");
    let deleted = app
        .client
        .get(format!(
            "{}/api/v1/outcomes/{}",
            app.base_url, fixture.outcome_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("show the deleted outcome");
    assert_eq!(deleted.status(), 404);

    cleanup(&app.pool, &fixture).await;
}

// ---------------------------------------------------------------------------
// PUT /outcomes/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcome_update_requires_an_admin_and_updates_the_permitted_fields() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    // A non-admin (teacher) is refused (`update` right on the outcome).
    let forbidden = app
        .client
        .put(format!(
            "{}/api/v1/outcomes/{}",
            app.base_url, fixture.outcome_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .json(&json!({ "title": "nope" }))
        .send()
        .await
        .expect("update as teacher");
    assert_eq!(forbidden.status(), 403);

    // An admin updates the permitted fields; fields not provided stay as-is.
    let response = app
        .client
        .put(format!(
            "{}/api/v1/outcomes/{}",
            app.base_url, fixture.outcome_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.admin_id)),
        )
        .json(&json!({
            "title": "renamed outcome",
            "vendor_guid": "vendor-86-updated",
            "calculation_method": "decaying_average",
            "calculation_int": 65
        }))
        .send()
        .await
        .expect("update as admin");
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["title"], "renamed outcome");
    assert_eq!(body["vendor_guid"], "vendor-86-updated");
    assert_eq!(body["calculation_method"], "decaying_average");
    assert_eq!(body["calculation_int"], 65);
    // Fields not provided are left alone.
    assert_eq!(body["description"], "a canvas outcome");
    assert_eq!(body["display_name"], "friendly name");

    // A blank title fails validation (`render errors, :bad_request`).
    let blank = app
        .client
        .put(format!(
            "{}/api/v1/outcomes/{}",
            app.base_url, fixture.outcome_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.admin_id)),
        )
        .json(&json!({ "title": "   " }))
        .send()
        .await
        .expect("update with a blank title");
    assert_eq!(blank.status(), 400);

    cleanup(&app.pool, &fixture).await;
}

// ---------------------------------------------------------------------------
// DELETE /outcomes/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcome_destroy_soft_deletes_and_answers_the_deleted_json() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    // A non-admin is refused.
    let forbidden = app
        .client
        .delete(format!(
            "{}/api/v1/outcomes/{}",
            app.base_url, fixture.outcome_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.student_id)),
        )
        .send()
        .await
        .expect("destroy as student");
    assert_eq!(forbidden.status(), 403);

    let response = app
        .client
        .delete(format!(
            "{}/api/v1/outcomes/{}",
            app.base_url, fixture.outcome_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.admin_id)),
        )
        .send()
        .await
        .expect("destroy as admin");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    assert_eq!(body["title"], format!("{prefix}-outcome"));

    // The row is soft-deleted: a later show is 404.
    let state: (String,) =
        sqlx::query_as("SELECT workflow_state FROM learning_outcomes WHERE id = $1")
            .bind(fixture.outcome_id)
            .fetch_one(&app.pool)
            .await
            .expect("read the workflow state");
    assert_eq!(state.0, "deleted");

    let gone = app
        .client
        .get(format!(
            "{}/api/v1/outcomes/{}",
            app.base_url, fixture.outcome_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.admin_id)),
        )
        .send()
        .await
        .expect("show the destroyed outcome");
    assert_eq!(gone.status(), 404);

    cleanup(&app.pool, &fixture).await;
}

// ---------------------------------------------------------------------------
// GET /courses/{course_id}/outcome_alignments
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcome_alignments_for_assignment_lists_the_aligned_outcomes() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_alignments?assignment_id={}",
            app.base_url, fixture.course_id, fixture.assignment_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the assignment alignments");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    let alignments = body.as_array().expect("the alignments array");
    assert_eq!(alignments.len(), 1);
    // Parity pin: the outcome comes from content_tags.learning_outcome_id —
    // NOT from content_id (the aligned assignment).
    assert_eq!(alignments[0]["learning_outcome_id"], fixture.outcome_id);
    assert_eq!(alignments[0]["assignment_id"], fixture.assignment_id);
    assert_eq!(
        alignments[0]["url"],
        format!(
            "/courses/{}/assignments/{}",
            fixture.course_id, fixture.assignment_id
        )
    );

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_alignments_assignment_branch_requires_grader_rights() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_alignments?assignment_id={}",
            app.base_url, fixture.course_id, fixture.assignment_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.student_id)),
        )
        .send()
        .await
        .expect("read the assignment alignments as a student");

    assert_eq!(response.status(), 401);

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_alignments_rejects_a_request_without_student_or_assignment() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_alignments",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the alignments without parameters");

    assert_eq!(response.status(), 400);
    // `ApiError::BadRequest` renders the message as the plain-text body.
    let body = response.text().await.expect("decode the body");
    assert!(
        body.contains("student_id or assignment_id is required"),
        "unexpected body: {body}"
    );

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_alignments_404s_for_an_assignment_outside_the_course() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_alignments?assignment_id=999999",
            app.base_url, fixture.course_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the alignments of a foreign assignment");

    assert_eq!(response.status(), 404);

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_alignments_for_student_returns_the_submitted_alignments() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    // The student reads their own alignments: only the assignments WITH a
    // submission (`INNER JOIN submissions`) and — without grade rights — no
    // unpublished assignments. The aligned-but-never-submitted assignment is
    // absent.
    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_alignments?student_id={}",
            app.base_url, fixture.course_id, fixture.student_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.student_id)),
        )
        .send()
        .await
        .expect("read the student's own alignments");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    let alignments = body.as_array().expect("the alignments array");
    assert_eq!(alignments.len(), 1);
    assert_eq!(alignments[0]["learning_outcome_id"], fixture.outcome_id);
    assert_eq!(alignments[0]["assignment_id"], fixture.assignment_id);
    assert_eq!(alignments[0]["submission_state"], "submitted");

    // A grader (teacher) sees the submitted unpublished assignment too.
    let teacher_view = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_alignments?student_id={}",
            app.base_url, fixture.course_id, fixture.student_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the student's alignments as a teacher");
    assert_eq!(teacher_view.status(), 200);
    let teacher_body: Value = teacher_view.json().await.expect("decode the body");
    let teacher_alignments = teacher_body.as_array().expect("the alignments array");
    let mut assignment_ids: Vec<i64> = teacher_alignments
        .iter()
        .map(|a| a["assignment_id"].as_i64().expect("assignment id"))
        .collect();
    assignment_ids.sort_unstable();
    assert_eq!(
        assignment_ids,
        vec![fixture.assignment_id, fixture.unpublished_assignment_id]
    );

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_alignments_student_branch_rejects_another_students_id() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_alignments?student_id={}",
            app.base_url, fixture.course_id, fixture.other_student_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.student_id)),
        )
        .send()
        .await
        .expect("read another student's alignments");

    assert_eq!(response.status(), 401);

    cleanup(&app.pool, &fixture).await;
}

#[tokio::test]
async fn outcome_alignments_student_branch_filters_by_assignment_id() {
    let Some(app) = setup().await else {
        return;
    };
    let prefix = run_prefix();
    let fixture = seed(&app.pool, &prefix).await;

    // student_id + assignment_id narrows to that one submission.
    let response = app
        .client
        .get(format!(
            "{}/api/v1/courses/{}/outcome_alignments?student_id={}&assignment_id={}",
            app.base_url, fixture.course_id, fixture.student_id, fixture.assignment_id
        ))
        .header(
            "Authorization",
            format!("Bearer {}", app.token(fixture.teacher_id)),
        )
        .send()
        .await
        .expect("read the filtered alignments");

    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.expect("decode the body");
    let alignments = body.as_array().expect("the alignments array");
    assert_eq!(alignments.len(), 1);
    assert_eq!(alignments[0]["assignment_id"], fixture.assignment_id);
    assert_eq!(alignments[0]["submission_state"], "submitted");

    cleanup(&app.pool, &fixture).await;
}
