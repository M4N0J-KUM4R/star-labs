//! Integration tests for the Canvas nested rubric route aliases.
//!
//! Exercises GET/PUT/DELETE `/courses/{courseId}/rubrics/{id}` through the real
//! star-rubrics router against Postgres. The aliases must only delegate to the
//! flat `/rubrics/{id}` handlers when the rubric belongs to the given course;
//! a missing or cross-course rubric is a 404, and the flat handlers' authz is
//! unchanged.
//!
//! Skipped cleanly when DATABASE_URL is not set. Run with:
//! DATABASE_URL=postgres://... cargo test -p star-rubrics

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "rub-tests-{}-{}",
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
    let server = TestServer::new(
        axum::Router::new()
            .merge(star_rubrics::router())
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

async fn seed_course(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(name)
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(course_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed enrollment");
}

async fn seed_rubric(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO rubrics (course_id, title, points_possible) VALUES ($1, $2, 100.0) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed rubric")
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments (course_id, title, points_possible) VALUES ($1, $2, 100.0) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_submission(pool: &PgPool, assignment_id: i64, user_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO submissions (assignment_id, user_id, body) VALUES ($1, $2, 'answer') RETURNING id",
    )
    .bind(assignment_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM rubrics WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}%"))
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

#[tokio::test]
async fn nested_aliases_get_update_delete_a_rubric_in_its_course() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let rubric = seed_rubric(&pool, course, "Original").await;

    // GET alias returns exactly what the flat route returns.
    let alias_get = server
        .get(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(alias_get.status_code(), 200, "body: {}", alias_get.text());
    let alias_body: Value = alias_get.json();
    assert_eq!(alias_body["id"].as_i64(), Some(rubric));
    assert_eq!(alias_body["course_id"].as_i64(), Some(course));

    let flat_get = server
        .get(&format!("/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(flat_get.status_code(), 200);
    let flat_body: Value = flat_get.json();
    assert_eq!(alias_body, flat_body);

    // PUT alias updates through the flat handler.
    let alias_put = server
        .put(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "title": "Renamed" }))
        .await;
    assert_eq!(alias_put.status_code(), 200, "body: {}", alias_put.text());
    let updated: Value = alias_put.json();
    assert_eq!(updated["title"].as_str(), Some("Renamed"));

    // DELETE alias soft-deletes through the flat handler.
    let alias_delete = server
        .delete(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(
        alias_delete.status_code(),
        204,
        "body: {}",
        alias_delete.text()
    );
    let (workflow_state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM rubrics WHERE id = $1")
            .bind(rubric)
            .fetch_one(&pool)
            .await
            .expect("fetch rubric state");
    assert_eq!(workflow_state, "deleted");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_aliases_return_404_for_missing_or_cross_course_rubrics() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let other_course = seed_course(&pool, &prefix, "Other").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let rubric = seed_rubric(&pool, course, "Original").await;
    let missing_rubric = 999_999_999_i64;

    // Cross-course requests are 404 even though the teacher could otherwise
    // manage the rubric in its real course.
    for method in ["get", "put", "delete"] {
        let mut req = match method {
            "get" => server.get(&format!("/courses/{other_course}/rubrics/{rubric}")),
            "put" => server.put(&format!("/courses/{other_course}/rubrics/{rubric}")),
            _ => server.delete(&format!("/courses/{other_course}/rubrics/{rubric}")),
        }
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"));
        if method == "put" {
            req = req.json(&json!({ "title": "Hijacked" }));
        }
        let resp = req.await;
        assert_eq!(
            resp.status_code(),
            404,
            "{method} cross-course must be 404, body: {}",
            resp.text()
        );
    }

    // Unknown rubric ids under the correct course are also 404.
    for method in ["get", "put", "delete"] {
        let mut req = match method {
            "get" => server.get(&format!("/courses/{course}/rubrics/{missing_rubric}")),
            "put" => server.put(&format!("/courses/{course}/rubrics/{missing_rubric}")),
            _ => server.delete(&format!("/courses/{course}/rubrics/{missing_rubric}")),
        }
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"));
        if method == "put" {
            req = req.json(&json!({ "title": "Hijacked" }));
        }
        let resp = req.await;
        assert_eq!(
            resp.status_code(),
            404,
            "{method} missing rubric must be 404, body: {}",
            resp.text()
        );
    }

    // The rejected cross-course writes left the rubric untouched.
    let (title, workflow_state): (String, String) =
        sqlx::query_as("SELECT title, workflow_state FROM rubrics WHERE id = $1")
            .bind(rubric)
            .fetch_one(&pool)
            .await
            .expect("fetch rubric");
    assert_eq!(title, "Original");
    assert_eq!(workflow_state, "active");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn nested_aliases_preserve_flat_route_authorization() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, student, course, "student").await;
    let rubric = seed_rubric(&pool, course, "Guarded").await;

    // A student gets the same 403 from the alias and the flat route.
    let flat_get = server
        .get(&format!("/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(flat_get.status_code(), 403);

    let alias_get = server
        .get(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(alias_get.status_code(), 403);

    let alias_put = server
        .put(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "title": "Nope" }))
        .await;
    assert_eq!(alias_put.status_code(), 403);

    let alias_delete = server
        .delete(&format!("/courses/{course}/rubrics/{rubric}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(alias_delete.status_code(), 403);

    // Unauthenticated alias requests are still rejected by AuthUser.
    let anonymous = server
        .get(&format!("/courses/{course}/rubrics/{rubric}"))
        .await;
    assert_eq!(anonymous.status_code(), 401);

    let (title, workflow_state): (String, String) =
        sqlx::query_as("SELECT title, workflow_state FROM rubrics WHERE id = $1")
            .bind(rubric)
            .fetch_one(&pool)
            .await
            .expect("fetch rubric");
    assert_eq!(title, "Guarded");
    assert_eq!(workflow_state, "active");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_scoped_assessments_create_list_and_read() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let other_course = seed_course(&pool, &prefix, "Other").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let submission = seed_submission(&pool, assignment, student).await;
    let rubric = seed_rubric(&pool, course, "Essay Rubric").await;

    // Course-scoped create accepts the submission-backed assessment payload.
    let created = server
        .post(&format!("/courses/{course}/rubrics/{rubric}/assessments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "submission_id": submission,
            "score": 85.0,
            "data": [{ "criterion_id": 1, "rating_id": 2, "points": 85.0 }]
        }))
        .await;
    assert_eq!(created.status_code(), 201, "body: {}", created.text());
    let created_body: Value = created.json();
    let assessment_id = created_body["id"].as_i64().expect("assessment id");
    assert_eq!(created_body["rubric_id"].as_i64(), Some(rubric));
    assert_eq!(created_body["course_id"].as_i64(), Some(course));
    assert_eq!(created_body["submission_id"].as_i64(), Some(submission));
    assert_eq!(created_body["user_id"].as_i64(), Some(student));
    assert_eq!(created_body["assessor_id"].as_i64(), Some(teacher));
    assert_eq!(created_body["assessment_type"].as_str(), Some("grading"));

    // Course-scoped list returns it for the teacher.
    let listed = server
        .get(&format!("/courses/{course}/rubrics/{rubric}/assessments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(listed.status_code(), 200, "body: {}", listed.text());
    let listed_body: Value = listed.json();
    let rows = listed_body.as_array().expect("assessment list");
    assert!(rows.iter().any(|r| r["id"].as_i64() == Some(assessment_id)));

    // The assessed student can read the flat route; the teacher can too.
    for user in [teacher, student] {
        let role = if user == teacher {
            "teacher"
        } else {
            "student"
        };
        let fetched = server
            .get(&format!("/rubric_assessments/{assessment_id}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(user, role))
            .await;
        assert_eq!(fetched.status_code(), 200, "body: {}", fetched.text());
        let fetched_body: Value = fetched.json();
        assert_eq!(fetched_body["id"].as_i64(), Some(assessment_id));
        assert_eq!(fetched_body["score"].as_f64(), Some(85.0));
    }

    // Wrong-course nested access is a 404.
    let wrong_course = server
        .get(&format!(
            "/courses/{other_course}/rubrics/{rubric}/assessments"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(wrong_course.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn rubric_assessment_update_and_delete_roundtrip() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let submission = seed_submission(&pool, assignment, student).await;
    let rubric = seed_rubric(&pool, course, "Essay Rubric").await;

    // Create through the pre-existing flat route.
    let created = server
        .post(&format!("/rubrics/{rubric}/assessments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "submission_id": submission, "score": 70.0, "data": [] }))
        .await;
    assert_eq!(created.status_code(), 201, "body: {}", created.text());
    let assessment_id = created.json::<Value>()["id"]
        .as_i64()
        .expect("assessment id");

    // Partial update: score, comments, type.
    let updated = server
        .put(&format!("/rubric_assessments/{assessment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "score": 92.5,
            "comments": "Strong thesis",
            "assessment_type": "peer_review"
        }))
        .await;
    assert_eq!(updated.status_code(), 200, "body: {}", updated.text());
    let updated_body: Value = updated.json();
    assert_eq!(updated_body["score"].as_f64(), Some(92.5));
    assert_eq!(updated_body["comments"].as_str(), Some("Strong thesis"));
    assert_eq!(
        updated_body["assessment_type"].as_str(),
        Some("peer_review")
    );

    // Invalid assessment types are rejected without touching the row.
    let invalid = server
        .put(&format!("/rubric_assessments/{assessment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "assessment_type": "curve" }))
        .await;
    assert_eq!(invalid.status_code(), 422);

    // Delete removes the row and returns the deleted assessment (Canvas shape).
    let deleted = server
        .delete(&format!("/rubric_assessments/{assessment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(deleted.status_code(), 200, "body: {}", deleted.text());
    assert_eq!(deleted.json::<Value>()["id"].as_i64(), Some(assessment_id));

    let gone = server
        .get(&format!("/rubric_assessments/{assessment_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(gone.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn rubric_assessment_routes_enforce_canvas_access_policy() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, outsider, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let submission = seed_submission(&pool, assignment, student).await;
    let rubric = seed_rubric(&pool, course, "Essay Rubric").await;

    let created = server
        .post(&format!("/courses/{course}/rubrics/{rubric}/assessments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "submission_id": submission, "score": 80.0 }))
        .await;
    assert_eq!(created.status_code(), 201, "body: {}", created.text());
    let assessment_id = created.json::<Value>()["id"]
        .as_i64()
        .expect("assessment id");

    // A different enrolled student cannot read or modify someone's assessment.
    for method in ["get", "put", "delete"] {
        let request = match method {
            "get" => server.get(&format!("/rubric_assessments/{assessment_id}")),
            "put" => server.put(&format!("/rubric_assessments/{assessment_id}")),
            _ => server.delete(&format!("/rubric_assessments/{assessment_id}")),
        }
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"));
        let response = if method == "put" {
            request.json(&json!({ "score": 100.0 })).await
        } else {
            request.await
        };
        assert_eq!(
            response.status_code(),
            403,
            "{method} must be forbidden for an unrelated student"
        );
    }

    // Students may not create assessments either (existing teacher-only rule).
    let student_create = server
        .post(&format!("/courses/{course}/rubrics/{rubric}/assessments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "submission_id": submission, "score": 100.0 }))
        .await;
    assert_eq!(student_create.status_code(), 403);

    // Unauthenticated access is rejected.
    let anonymous = server
        .get(&format!("/rubric_assessments/{assessment_id}"))
        .await;
    assert_eq!(anonymous.status_code(), 401);

    // The rejected writes left the assessment untouched.
    let (score,): (Option<f64>,) =
        sqlx::query_as("SELECT score FROM rubric_assessments WHERE id = $1")
            .bind(assessment_id)
            .fetch_one(&pool)
            .await
            .expect("fetch assessment");
    assert_eq!(score, Some(80.0));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn rubric_association_create_used_locations_and_soft_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let rubric = seed_rubric(&pool, course, "Essay Rubric").await;

    // Canvas RubricAssociationsController#create returns
    // { rubric, rubric_association } with a 200.
    let created = server
        .post(&format!("/courses/{course}/rubrics/{rubric}/associations"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": {
                "association_id": assignment,
                "association_type": "Assignment",
                "purpose": "grading",
                "use_for_grading": true
            }
        }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let created_body: Value = created.json();
    let association_id = created_body["rubric_association"]["id"]
        .as_i64()
        .expect("association id");
    let expected_context_code = format!("course_{course}");
    assert_eq!(created_body["rubric"]["id"].as_i64(), Some(rubric));
    assert_eq!(
        created_body["rubric_association"]["rubric_id"].as_i64(),
        Some(rubric)
    );
    assert_eq!(
        created_body["rubric_association"]["association_id"].as_i64(),
        Some(assignment)
    );
    assert_eq!(
        created_body["rubric_association"]["association_type"].as_str(),
        Some("Assignment")
    );
    assert_eq!(
        created_body["rubric_association"]["context_id"].as_i64(),
        Some(course)
    );
    assert_eq!(
        created_body["rubric_association"]["context_type"].as_str(),
        Some("Course")
    );
    assert_eq!(
        created_body["rubric_association"]["context_code"].as_str(),
        Some(expected_context_code.as_str())
    );
    assert_eq!(
        created_body["rubric_association"]["title"].as_str(),
        Some("Essay")
    );
    assert_eq!(
        created_body["rubric_association"]["use_for_grading"].as_bool(),
        Some(true)
    );
    assert_eq!(
        created_body["rubric_association"]["bookmarked"].as_bool(),
        Some(true)
    );
    assert_eq!(
        created_body["rubric_association"]["workflow_state"].as_str(),
        Some("active")
    );
    assert_eq!(
        created_body["rubric_association"]["association_count"].as_i64(),
        Some(1)
    );
    assert_eq!(
        created_body["rubric_association"]["rubric_assessments"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );

    // used_locations groups by course: id, name, concluded?, assignments.
    let locations = server
        .get(&format!(
            "/courses/{course}/rubrics/{rubric}/used_locations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(locations.status_code(), 200, "body: {}", locations.text());
    let locations_body: Value = locations.json();
    let courses = locations_body.as_array().expect("used locations array");
    assert_eq!(courses.len(), 1);
    assert_eq!(courses[0]["id"].as_i64(), Some(course));
    assert_eq!(courses[0]["name"].as_str(), Some("Course"));
    // Canvas serializes the `concluded?` method name verbatim.
    assert_eq!(courses[0]["concluded?"].as_bool(), Some(false));
    assert_eq!(
        courses[0]["assignments"][0]["id"].as_i64(),
        Some(assignment)
    );
    assert_eq!(
        courses[0]["assignments"][0]["title"].as_str(),
        Some("Essay")
    );

    // DELETE soft-deletes and returns the association (Canvas shape).
    let deleted = server
        .delete(&format!("/rubric_associations/{association_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(deleted.status_code(), 200, "body: {}", deleted.text());
    let deleted_body: Value = deleted.json();
    assert_eq!(deleted_body["id"].as_i64(), Some(association_id));
    assert_eq!(deleted_body["workflow_state"].as_str(), Some("deleted"));

    let (stored_state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM rubric_associations WHERE id = $1")
            .bind(association_id)
            .fetch_one(&pool)
            .await
            .expect("fetch association");
    assert_eq!(stored_state, "deleted");

    // Deleted associations drop out of used_locations and cannot be deleted twice.
    let empty = server
        .get(&format!(
            "/courses/{course}/rubrics/{rubric}/used_locations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(empty.status_code(), 200);
    assert_eq!(empty.json::<Value>().as_array().map(Vec::len), Some(0));

    let again = server
        .delete(&format!("/rubric_associations/{association_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(again.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn grading_associations_are_unique_per_assignment_and_repoint_the_rubric() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let other_assignment = seed_assignment(&pool, course, "Lab").await;
    let rubric_a = seed_rubric(&pool, course, "Rubric A").await;
    let rubric_b = seed_rubric(&pool, course, "Rubric B").await;

    let first = server
        .post(&format!(
            "/courses/{course}/rubrics/{rubric_a}/associations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": {
                "association_id": assignment,
                "association_type": "Assignment",
                "purpose": "grading"
            }
        }))
        .await;
    assert_eq!(first.status_code(), 200, "body: {}", first.text());
    let first_id = first.json::<Value>()["rubric_association"]["id"]
        .as_i64()
        .expect("association id");

    // A grading association is unique per assignment: re-associating re-points
    // the existing row at the new rubric instead of inserting a second one.
    let second = server
        .post(&format!(
            "/courses/{course}/rubrics/{rubric_b}/associations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": {
                "rubric_id": rubric_b,
                "association_id": assignment,
                "association_type": "Assignment",
                "purpose": "grading"
            }
        }))
        .await;
    assert_eq!(second.status_code(), 200, "body: {}", second.text());
    let second_body: Value = second.json();
    assert_eq!(
        second_body["rubric_association"]["id"].as_i64(),
        Some(first_id)
    );
    assert_eq!(
        second_body["rubric_association"]["rubric_id"].as_i64(),
        Some(rubric_b)
    );
    assert_eq!(second_body["rubric"]["id"].as_i64(), Some(rubric_b));

    let (active_grading,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM rubric_associations \
         WHERE association_type = 'Assignment' AND association_id = $1 \
           AND purpose = 'grading' AND workflow_state <> 'deleted'",
    )
    .bind(assignment)
    .fetch_one(&pool)
    .await
    .expect("count grading associations");
    assert_eq!(active_grading, 1);

    let (count_a,): (i32,) = sqlx::query_as("SELECT association_count FROM rubrics WHERE id = $1")
        .bind(rubric_a)
        .fetch_one(&pool)
        .await
        .expect("fetch rubric A counters");
    assert_eq!(count_a, 0);
    let (count_b,): (i32,) = sqlx::query_as("SELECT association_count FROM rubrics WHERE id = $1")
        .bind(rubric_b)
        .fetch_one(&pool)
        .await
        .expect("fetch rubric B counters");
    assert_eq!(count_b, 1);

    // hide_points wins over use_for_grading/hide_score_total (Canvas update).
    let hidden = server
        .post(&format!(
            "/courses/{course}/rubrics/{rubric_b}/associations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": {
                "association_id": other_assignment,
                "association_type": "Assignment",
                "purpose": "grading",
                "use_for_grading": true,
                "hide_score_total": true,
                "hide_points": true
            }
        }))
        .await;
    assert_eq!(hidden.status_code(), 200, "body: {}", hidden.text());
    let hidden_body: Value = hidden.json();
    assert_eq!(
        hidden_body["rubric_association"]["hide_points"].as_bool(),
        Some(true)
    );
    assert_eq!(
        hidden_body["rubric_association"]["use_for_grading"].as_bool(),
        Some(false)
    );
    assert_eq!(
        hidden_body["rubric_association"]["hide_score_total"].as_bool(),
        Some(false)
    );

    // Bookmark associations are not deduplicated.
    let mut bookmark_ids = Vec::new();
    for _ in 0..2 {
        let bookmarked = server
            .post(&format!(
                "/courses/{course}/rubrics/{rubric_b}/associations"
            ))
            .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
            .json(&json!({
                "rubric_association": {
                    "association_id": assignment,
                    "association_type": "Assignment",
                    "purpose": "bookmark"
                }
            }))
            .await;
        assert_eq!(bookmarked.status_code(), 200, "body: {}", bookmarked.text());
        let body: Value = bookmarked.json();
        assert_eq!(
            body["rubric_association"]["bookmarked"].as_bool(),
            Some(true)
        );
        bookmark_ids.push(
            body["rubric_association"]["id"]
                .as_i64()
                .expect("bookmark association id"),
        );
    }
    assert_ne!(bookmark_ids[0], bookmark_ids[1]);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn rubric_association_routes_enforce_canvas_permissions_and_validation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "Course").await;
    let other_course = seed_course(&pool, &prefix, "Other").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let assignment = seed_assignment(&pool, course, "Essay").await;
    let other_assignment = seed_assignment(&pool, other_course, "Foreign").await;
    let rubric = seed_rubric(&pool, course, "Essay Rubric").await;

    let association_path = format!("/courses/{course}/rubrics/{rubric}/associations");
    let locations_path = format!("/courses/{course}/rubrics/{rubric}/used_locations");

    // Students cannot manage associations or inspect used locations.
    let student_create = server
        .post(&association_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({
            "rubric_association": {
                "association_id": assignment,
                "association_type": "Assignment",
                "purpose": "grading"
            }
        }))
        .await;
    assert_eq!(student_create.status_code(), 403);

    let student_locations = server
        .get(&locations_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(student_locations.status_code(), 403);

    // A rubric from another course is never reachable through this course.
    let cross_course = server
        .post(&format!(
            "/courses/{other_course}/rubrics/{rubric}/associations"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": {
                "association_id": assignment,
                "association_type": "Assignment",
                "purpose": "grading"
            }
        }))
        .await;
    assert_eq!(cross_course.status_code(), 404);

    // Association objects must belong to the course context.
    let foreign_assignment = server
        .post(&association_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": {
                "association_id": other_assignment,
                "association_type": "Assignment",
                "purpose": "grading"
            }
        }))
        .await;
    assert_eq!(foreign_assignment.status_code(), 404);

    // Canvas association types/purposes are validated.
    let account_type = server
        .post(&association_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": {
                "association_id": assignment,
                "association_type": "Account"
            }
        }))
        .await;
    assert_eq!(account_type.status_code(), 400);

    let missing_id = server
        .post(&association_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": { "association_type": "Assignment" }
        }))
        .await;
    assert_eq!(missing_id.status_code(), 400);

    let bad_purpose = server
        .post(&association_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": {
                "association_id": assignment,
                "association_type": "Assignment",
                "purpose": "curve"
            }
        }))
        .await;
    assert_eq!(bad_purpose.status_code(), 422);

    // A valid association can only be deleted by course managers.
    let created = server
        .post(&association_path)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "rubric_association": {
                "association_id": assignment,
                "association_type": "Assignment",
                "purpose": "grading"
            }
        }))
        .await;
    assert_eq!(created.status_code(), 200, "body: {}", created.text());
    let association_id = created.json::<Value>()["rubric_association"]["id"]
        .as_i64()
        .expect("association id");

    let student_delete = server
        .delete(&format!("/rubric_associations/{association_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(student_delete.status_code(), 403);

    let (stored_state,): (String,) =
        sqlx::query_as("SELECT workflow_state FROM rubric_associations WHERE id = $1")
            .bind(association_id)
            .fetch_one(&pool)
            .await
            .expect("fetch association");
    assert_eq!(stored_state, "active");

    cleanup(&pool, &prefix).await;
}
