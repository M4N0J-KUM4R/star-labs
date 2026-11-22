//! Integration tests for the gradebook history routes (Canvas
//! `gradebook_history_api_controller`, spec p5 slice B, issue #32).
//!
//! Data source: `grade_change_events` (the snapshot's audit trail) is mapped
//! to submission versions — see `src/gradebook_history.rs` for the deviation
//! note.
//!
//! Pinned behavior:
//! - All four routes require course `manage_grades`: anonymous 401, student
//!   403, unknown course 404.
//! - `days` groups events by UTC calendar day (Star courses carry no
//!   per-course time zone), newest day first, one grader entry per grader
//!   with the assignments they graded that day.
//! - `day_details` scopes the same aggregation to one day; a bad date is 400.
//! - `submissions` returns `{submission_id, versions: [...]}` groups with
//!   only score-changing versions, `previous_*` nil on the first scored
//!   version, `new_*` aliases, newest-first.
//! - `feed` is the paginated uncollated list ordered by version id
//!   descending unless `ascending=true`, honoring `assignment_id`/`user_id`
//!   filters (404 on unknown ids).
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix and
//! full teardown keep repeat runs against a shared DB safe.

use axum_test::TestServer;
use serde_json::Value;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "gbh_{}_{}",
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
        jwt_secret: JWT_SECRET.to_string(),
    };

    let server = TestServer::new(star_audit::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
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
    .bind("not-a-real-hash")
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
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(course_id)
        .bind(role)
        .execute(pool)
        .await
        .expect("seed enrollment");
}

async fn seed_assignment(pool: &PgPool, course_id: i64, title: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO assignments (course_id, title, points_possible)
        VALUES ($1, $2, 10)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_submission(pool: &PgPool, assignment_id: i64, student_id: i64) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO submissions (assignment_id, user_id, score, graded_at, grader_id, workflow_state)
        VALUES ($1, $2, 8.0, now(), NULL, 'graded')
        RETURNING id
        "#,
    )
    .bind(assignment_id)
    .bind(student_id)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

/// Seed one grade change event with explicit grader/score/version fields.
#[allow(clippy::too_many_arguments)]
async fn seed_history_event(
    pool: &PgPool,
    uuid: &str,
    course_id: i64,
    assignment_id: i64,
    student_id: i64,
    grader_id: Option<i64>,
    score_after: Option<f64>,
    grade_after: Option<&str>,
    version_number: i32,
    created_at: chrono::DateTime<chrono::Utc>,
) {
    sqlx::query(
        r#"
        INSERT INTO grade_change_events
            (uuid, course_id, assignment_id, student_id, grader_id,
             event_type, grade_after, score_after, points_possible_after,
             version_number, created_at)
        VALUES ($1, $2, $3, $4, $5, 'grade_change', $6, $7, 10.0, $8, $9)
        "#,
    )
    .bind(uuid)
    .bind(course_id)
    .bind(assignment_id)
    .bind(student_id)
    .bind(grader_id)
    .bind(grade_after)
    .bind(score_after)
    .bind(version_number)
    .bind(created_at)
    .execute(pool)
    .await
    .expect("seed history event");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM grade_change_events WHERE uuid LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM submissions WHERE assignment_id IN (SELECT id FROM assignments WHERE title LIKE $1)")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM assignments WHERE title LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM enrollments WHERE course_id IN (SELECT id FROM courses WHERE course_code LIKE $1)")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
}

/// The seeded world shared by the happy-path tests: a course with a teacher
/// (grader), a student, a submission, and events on two days — day 1 graded
/// by the teacher, day 2 regraded (score changed) by a second grader plus an
/// autograded event under grader id <= 0.
struct World {
    course: i64,
    teacher: i64,
    grader2: i64,
    student: i64,
    assignment: i64,
    submission: i64,
    day1: String,
    day2: String,
}

async fn seed_world(pool: &PgPool, prefix: &str) -> World {
    let teacher = seed_user(pool, prefix, "Teacher", "teacher").await;
    let grader2 = seed_user(pool, prefix, "Grader2", "teacher").await;
    let student = seed_user(pool, prefix, "Student", "student").await;
    let course = seed_course(pool, prefix, "CourseGBH").await;
    seed_enrollment(pool, teacher, course, "teacher").await;
    seed_enrollment(pool, student, course, "student").await;
    let assignment = seed_assignment(pool, course, &format!("{prefix}-A1")).await;
    let submission = seed_submission(pool, assignment, student).await;

    let day1 = chrono::Utc::now() - chrono::Duration::days(2);
    let day2 = chrono::Utc::now() - chrono::Duration::days(1);
    let d1 = day1.format("%Y-%m-%d").to_string();
    let d2 = day2.format("%Y-%m-%d").to_string();

    seed_history_event(
        pool,
        &format!("{prefix}-v1"),
        course,
        assignment,
        student,
        Some(teacher),
        Some(5.0),
        Some("5"),
        1,
        day1,
    )
    .await;
    seed_history_event(
        pool,
        &format!("{prefix}-v2"),
        course,
        assignment,
        student,
        Some(grader2),
        Some(8.0),
        Some("8"),
        2,
        day2,
    )
    .await;
    seed_history_event(
        pool,
        &format!("{prefix}-v3"),
        course,
        assignment,
        student,
        None,
        Some(9.0),
        Some("9"),
        3,
        day2,
    )
    .await;

    World {
        course,
        teacher,
        grader2,
        student,
        assignment,
        submission,
        day1: d1,
        day2: d2,
    }
}

#[tokio::test]
async fn days_groups_by_day_graders_and_assignments() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let w = seed_world(&pool, &prefix).await;

    let url = format!("/courses/{}/gradebook_history/days", w.course);
    let resp = server
        .get(&url)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");

    let days = body.as_array().expect("array");
    assert_eq!(days.len(), 2, "two distinct days: {}", resp.text());
    // Newest day first (Canvas sort_by date .reverse).
    assert_eq!(days[0]["date"], serde_json::json!(w.day2));
    assert_eq!(days[1]["date"], serde_json::json!(w.day1));

    // Day 1: one grader (teacher), one assignment.
    let d1_graders = days[1]["graders"].as_array().expect("graders");
    assert_eq!(d1_graders.len(), 1);
    assert_eq!(d1_graders[0]["id"], serde_json::json!(w.teacher));
    assert_eq!(d1_graders[0]["name"], serde_json::json!("Teacher"));
    let assignments = d1_graders[0]["assignments"]
        .as_array()
        .expect("assignments");
    assert_eq!(assignments.len(), 1);
    assert_eq!(assignments[0]["id"], serde_json::json!(w.assignment));
    assert_eq!(
        assignments[0]["name"],
        serde_json::json!(format!("{prefix}-A1"))
    );

    // Day 2: grader2 plus the autograded bucket (grader id <= 0 -> 0).
    let d2_graders = days[0]["graders"].as_array().expect("graders");
    assert_eq!(d2_graders.len(), 2, "body: {}", resp.text());
    let ids: Vec<i64> = d2_graders.iter().filter_map(|g| g["id"].as_i64()).collect();
    assert!(ids.contains(&w.grader2));
    assert!(ids.contains(&0), "autograded bucket is grader 0");
    let zero = d2_graders.iter().find(|g| g["id"] == 0).unwrap();
    assert_eq!(zero["name"], serde_json::json!("Graded on submission"));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn days_empty_course_returns_empty_array() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let course = seed_course(&pool, &prefix, "CourseEmpty").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let resp = server
        .get(&format!("/courses/{course}/gradebook_history/days"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    assert!(body.as_array().unwrap().is_empty());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn days_permission_negatives() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "CourseAuth").await;
    seed_enrollment(&pool, student, course, "student").await;

    let url = format!("/courses/{course}/gradebook_history/days");

    let resp = server.get(&url).await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    let resp = server
        .get(&format!(
            "/courses/{}/gradebook_history/days",
            course + 1_000_000
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn day_details_scopes_to_one_day_and_rejects_bad_dates() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let w = seed_world(&pool, &prefix).await;

    let url = format!("/courses/{}/gradebook_history/{}", w.course, w.day2);
    let resp = server
        .get(&url)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    let graders = body.as_array().expect("grader array");
    assert_eq!(
        graders.len(),
        2,
        "day2 has grader2 + autograded: {}",
        resp.text()
    );
    let ids: Vec<i64> = graders.iter().filter_map(|g| g["id"].as_i64()).collect();
    assert!(ids.contains(&w.grader2));
    assert!(ids.contains(&0));

    // A day with no events returns [] (Canvas json_for_date on an empty set).
    let future = (chrono::Utc::now() + chrono::Duration::days(30))
        .format("%Y-%m-%d")
        .to_string();
    let resp = server
        .get(&format!("/courses/{}/gradebook_history/{future}", w.course))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    assert!(body.as_array().unwrap().is_empty());

    // Bad date format -> 400 (Canvas Date.strptime raises).
    let resp = server
        .get(&format!(
            "/courses/{}/gradebook_history/not-a-date",
            w.course
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());

    // Permission negatives.
    let resp = server
        .get(&format!(
            "/courses/{}/gradebook_history/{}",
            w.course, w.day2
        ))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .get(&format!(
            "/courses/{}/gradebook_history/{}",
            w.course, w.day2
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn submissions_collates_score_changing_versions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let w = seed_world(&pool, &prefix).await;

    // Seed a same-score regrade on day 2 (Canvas drops same-score versions).
    seed_history_event(
        &pool,
        &format!("{prefix}-v4"),
        w.course,
        w.assignment,
        w.student,
        Some(w.grader2),
        Some(8.0),
        Some("8"),
        4,
        chrono::Utc::now() - chrono::Duration::days(1),
    )
    .await;

    let url = format!(
        "/courses/{}/gradebook_history/{}/graders/{}/assignments/{}/submissions",
        w.course, w.day2, w.grader2, w.assignment
    );
    let resp = server
        .get(&url)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    let groups = body.as_array().expect("submission groups");
    assert_eq!(groups.len(), 1, "one submission group: {}", resp.text());
    assert_eq!(groups[0]["submission_id"], serde_json::json!(w.submission));

    let versions = groups[0]["versions"].as_array().expect("versions");
    assert_eq!(versions.len(), 1, "same-score v4 dropped: {}", resp.text());
    let v = &versions[0];
    assert_eq!(v["new_score"], serde_json::json!(8.0));
    assert_eq!(v["new_grade"], serde_json::json!("8"));
    assert_eq!(v["score"], serde_json::json!(8.0));
    assert_eq!(
        v["assignment_name"],
        serde_json::json!(format!("{prefix}-A1"))
    );
    assert_eq!(v["user_id"], serde_json::json!(w.student));
    // First kept version for this grader filter has no previous.
    assert!(v["previous_grade"].is_null());
    assert!(v["previous_graded_at"].is_null());
    assert!(v["previous_grader"].is_null());

    // Unknown assignment -> 404 after the gate.
    let resp = server
        .get(&format!(
            "/courses/{}/gradebook_history/{}/graders/{}/assignments/99999999/submissions",
            w.course, w.day2, w.grader2
        ))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Student caller -> 403; anonymous -> 401.
    let resp = server.get(&url).await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .get(&url)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn submissions_previous_version_carries_prior_grader() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let w = seed_world(&pool, &prefix).await;

    // Same teacher regrades on day 2 with a new score: v1 (5.0) -> v5 (7.0).
    seed_history_event(
        &pool,
        &format!("{prefix}-v5"),
        w.course,
        w.assignment,
        w.student,
        Some(w.teacher),
        Some(7.0),
        Some("7"),
        5,
        chrono::Utc::now() - chrono::Duration::days(1),
    )
    .await;

    let url = format!(
        "/courses/{}/gradebook_history/{}/graders/{}/assignments/{}/submissions",
        w.course, w.day2, w.teacher, w.assignment
    );
    let resp = server
        .get(&url)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    let versions = body[0]["versions"].as_array().expect("versions");
    // Day 2 only has v5 for the teacher (v1 was day 1); single version.
    assert_eq!(versions.len(), 1);

    // Now the day-1 view: only v1, no previous.
    let url = format!(
        "/courses/{}/gradebook_history/{}/graders/{}/assignments/{}/submissions",
        w.course, w.day1, w.teacher, w.assignment
    );
    let resp = server
        .get(&url)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    let versions = body[0]["versions"].as_array().expect("versions");
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0]["score"], serde_json::json!(5.0));
    assert!(versions[0]["previous_grade"].is_null());

    // Autograded grader id 0 selects the grader_id <= 0 bucket.
    let url = format!(
        "/courses/{}/gradebook_history/{}/graders/0/assignments/{}/submissions",
        w.course, w.day2, w.assignment
    );
    let resp = server
        .get(&url)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    let versions = body[0]["versions"].as_array().expect("versions");
    assert_eq!(versions.len(), 1, "autograded v3: {}", resp.text());
    assert_eq!(
        versions[0]["grader"],
        serde_json::json!("Graded on submission")
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn feed_orders_filters_and_paginates() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let w = seed_world(&pool, &prefix).await;

    let base = format!("/courses/{}/gradebook_history/feed", w.course);

    // Default: version id descending, uncollated (3 events).
    let resp = server
        .get(&base)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    let feed = body.as_array().expect("feed");
    assert_eq!(feed.len(), 3, "body: {}", resp.text());
    let vnums: Vec<i64> = feed
        .iter()
        .filter_map(|v| v["version_number"].as_i64())
        .collect();
    assert_eq!(vnums, vec![3, 2, 1], "descending by version id");

    // ascending=true flips the order.
    let resp = server
        .get(&format!("{base}?ascending=true"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    let vnums: Vec<i64> = body
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["version_number"].as_i64())
        .collect();
    assert_eq!(vnums, vec![1, 2, 3], "ascending");

    // Plain grade/grader fields, no new_/previous_ aliases.
    let first = &body.as_array().unwrap()[0];
    assert_eq!(first["grade"], serde_json::json!("5"));
    assert!(first.get("new_grade").is_none());
    assert!(first.get("previous_grade").is_none());

    // assignment_id filter.
    let resp = server
        .get(&format!("{base}?assignment_id={}", w.assignment))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    assert_eq!(body.as_array().unwrap().len(), 3);

    // user_id filter (other user -> empty but 200).
    let resp = server
        .get(&format!("{base}?user_id={}", w.teacher))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    assert!(body.as_array().unwrap().is_empty());

    // Unknown user_id / assignment_id -> 404 (Canvas api_find).
    let resp = server
        .get(&format!("{base}?user_id=99999999"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .get(&format!("{base}?assignment_id=99999999"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Pagination: per_page=2 page=1 then page=2.
    let resp = server
        .get(&format!("{base}?per_page=2&page=1"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    assert_eq!(body.as_array().unwrap().len(), 2);
    let resp = server
        .get(&format!("{base}?per_page=2&page=2"))
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.teacher, "teacher"),
        )
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = serde_json::from_str(&resp.text()).expect("json");
    assert_eq!(body.as_array().unwrap().len(), 1);

    // Permission negatives.
    let resp = server.get(&base).await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .get(&base)
        .add_header(
            axum::http::header::AUTHORIZATION,
            auth(w.student, "student"),
        )
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
