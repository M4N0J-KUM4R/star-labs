//! Integration tests for star-dashboard.
//!
//! Exercises the real dashboard router (GET /dashboard) through axum-test
//! against a real Postgres.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://... cargo test -p star-dashboard
//! - Uses a unique email prefix per run + full teardown so it is safe to run
//!   repeatedly against a shared/dev database.
//!
//! The handler serves three sections, and the scenarios below seed courses +
//! enrollments + assignments + announcements directly in the DB to build each
//! window:
//! - upcoming: assignments with due_at in [now, now + 7 days] UNION
//!   announcements created within the last 7 days, ordered by date ASC,
//!   capped at 5 items.
//! - courses: enrolled courses, ordered by name ASC.
//! - recent_announcements: the last 3 announcements across enrolled courses,
//!   ordered by created_at DESC (no time window at all).
//!
//! Quirks of the actual SQL that these tests pin down (and comment inline):
//! - Overdue assignments (due_at < now()) are NOT included in upcoming; only
//!   the forward 7-day window counts.
//! - Assignments with a NULL due_at never appear in upcoming.
//! - Submitted/completed work is NOT filtered out: the upcoming query never
//!   joins submissions, so an assignment with a graded submission still
//!   shows up while it is inside the due-date window.
//! - recent_announcements has no recency filter: an announcement older than
//!   7 days is absent from upcoming but still present in the last-3 list.
//! - Neither query filters enrollments.workflow_state or
//!   announcements.workflow_state, so soft-deleted rows would still surface.

use axum_test::TestServer;
use chrono::{DateTime, Duration, Utc};
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
        "dash-tests-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis() + n as i64
    )
}

/// Connect, run migrations, and build a TestServer over the dashboard
/// router exactly the way star-http assembles it. Returns None when
/// DATABASE_URL is unset so callers can skip cleanly.
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

    let server = TestServer::new(dashboard_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

/// Same merge pattern as star-http::build_api_router, restricted to the
/// dashboard router under test.
fn dashboard_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_dashboard::router())
        .with_state(state)
}

/// Create a user directly in the DB (bypassing the register endpoint) with a
/// deterministic password hash and the given role. Returns user id.
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
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) VALUES ($1, $2, $3, 'active')",
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
    due_at: Option<DateTime<Utc>>,
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

/// Create an announcement with an explicit created_at so window/cap scenarios
/// are deterministic. Returns id.
async fn seed_announcement(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    message: &str,
    created_at: DateTime<Utc>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO announcements (course_id, title, message, created_at)
        VALUES ($1, $2, $3, $4)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(title)
    .bind(message)
    .bind(created_at)
    .fetch_one(pool)
    .await
    .expect("seed announcement")
}

/// Record a graded submission for an assignment by a user.
async fn seed_submission(pool: &PgPool, assignment_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO submissions (assignment_id, user_id, body, score) VALUES ($1, $2, 'submitted', 100)",
    )
    .bind(assignment_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed submission");
}

/// Record a submitted-but-ungraded submission (drives grading todo items).
async fn seed_ungraded_submission(pool: &PgPool, assignment_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO submissions (assignment_id, user_id, body, score) VALUES ($1, $2, 'submitted work', NULL)",
    )
    .bind(assignment_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed ungraded submission");
}

/// Create a calendar event scoped to a course or a user. Returns id.
async fn seed_calendar_event(
    pool: &PgPool,
    course_id: Option<i64>,
    user_id: Option<i64>,
    title: &str,
    start_at: DateTime<Utc>,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO calendar_events (course_id, user_id, title, start_at, end_at, workflow_state)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING id
        "#,
    )
    .bind(course_id)
    .bind(user_id)
    .bind(title)
    .bind(start_at)
    .bind(start_at + Duration::hours(1))
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed calendar event")
}

/// Set the user's dashboard position for a course (Canvas dashboard order).
async fn seed_dashboard_position(pool: &PgPool, user_id: i64, course_id: i64, position: i32) {
    sqlx::query(
        r#"
        INSERT INTO dashboard_positions (user_id, course_id, position)
        VALUES ($1, $2, $3)
        ON CONFLICT (user_id, course_id) DO UPDATE SET position = EXCLUDED.position
        "#,
    )
    .bind(user_id)
    .bind(course_id)
    .bind(position)
    .execute(pool)
    .await
    .expect("seed dashboard position");
}

/// Mint a real bearer token the same way star-auth does at login time.
fn token_for(user_id: i64, role: &str) -> String {
    star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
}

fn auth(user_id: i64, role: &str) -> String {
    format!("Bearer {}", token_for(user_id, role))
}

/// Delete every row created by this run (users cascade to enrollments and
/// submissions; courses cascade to assignments and announcements).
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

/// GET /dashboard with the given bearer token, asserting 200.
async fn get_dashboard(server: &TestServer, token: &str) -> Value {
    let resp = server
        .get("/dashboard")
        .add_header(axum::http::header::AUTHORIZATION, token)
        .await;
    assert_eq!(
        resp.status_code(),
        200,
        "dashboard should succeed; body: {}",
        resp.text()
    );
    resp.json()
}

/// Parse a serialized DateTime<Utc> (RFC 3339) from a JSON value.
fn parse_rfc3339(v: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(v.as_str().expect("date/time as string"))
        .expect("parse RFC 3339 date/time")
        .with_timezone(&Utc)
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dashboard_without_token_is_401() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.get("/dashboard").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
}

#[tokio::test]
async fn dashboard_with_garbage_token_is_401() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server
        .get("/dashboard")
        .add_header(axum::http::header::AUTHORIZATION, "Bearer garbage-token")
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    // A token signed with a different secret is equally rejected.
    let wrong_secret = star_auth::create_token(1, "student", "other-secret").expect("mint token");
    let resp = server
        .get("/dashboard")
        .add_header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {wrong_secret}"),
        )
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
}

// ---------------------------------------------------------------------------
// Empty account
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_account_returns_200_with_empty_sections() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Fresh user with no enrollments, and courses in the DB they are NOT
    // enrolled in, so an over-broad query would return non-empty sections.
    let user = seed_user(&pool, &prefix, "Lonely", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let course = seed_course(&pool, &prefix, "NotMyCourse").await;
    seed_enrollment(&pool, other, course, "student").await;
    seed_assignment(
        &pool,
        course,
        "SomeoneElsesHomework",
        Some(Utc::now() + Duration::days(3)),
    )
    .await;
    seed_announcement(&pool, course, "SomeoneElsesNews", "hello", Utc::now()).await;

    let body = get_dashboard(&server, &auth(user, "student")).await;

    // The payload has exactly the three sections, all empty arrays.
    let obj = body.as_object().expect("dashboard object");
    assert_eq!(obj.len(), 3, "unexpected extra sections: {body}");
    for section in ["upcoming", "courses", "recent_announcements"] {
        let arr = body[section]
            .as_array()
            .unwrap_or_else(|| panic!("{section} must be an array, got: {}", body[section]));
        assert!(arr.is_empty(), "{section} should be empty: {arr:?}");
    }

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Courses section
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enrolled_courses_appear_in_courses_section() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "Enrolled", "student").await;
    let algebra = seed_course(&pool, &prefix, "Algebra").await;
    let biology = seed_course(&pool, &prefix, "Biology").await;
    // A course the user is NOT enrolled in must not leak into the section.
    let chemistry = seed_course(&pool, &prefix, "Chemistry").await;

    seed_enrollment(&pool, user, algebra, "student").await;
    seed_enrollment(&pool, user, biology, "teacher").await;

    let body = get_dashboard(&server, &auth(user, "student")).await;
    let courses = body["courses"].as_array().expect("courses array");

    // Ordered by name ASC, and the role comes from the enrollment row.
    assert_eq!(courses.len(), 2, "courses: {courses:?}");
    assert_eq!(courses[0]["id"], algebra);
    assert_eq!(courses[0]["name"], "Algebra");
    assert_eq!(courses[0]["course_code"], format!("{prefix}-Algebra"));
    assert_eq!(courses[0]["role"], "student");
    assert_eq!(courses[1]["id"], biology);
    assert_eq!(courses[1]["name"], "Biology");
    assert_eq!(courses[1]["course_code"], format!("{prefix}-Biology"));
    assert_eq!(courses[1]["role"], "teacher");
    assert!(
        courses
            .iter()
            .all(|c| c["id"] != chemistry && c["name"] != "Chemistry"),
        "unenrolled course leaked into courses: {courses:?}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Upcoming: assignment due-date window
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upcoming_lists_only_assignments_due_within_7_days() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "Student", "student").await;
    let course = seed_course(&pool, &prefix, "WindowCourse").await;
    seed_enrollment(&pool, user, course, "student").await;

    let in_window_at = Utc::now() + Duration::days(3);
    let in_window = seed_assignment(&pool, course, "DueIn3Days", Some(in_window_at)).await;
    seed_assignment(
        &pool,
        course,
        "DueIn30Days",
        Some(Utc::now() + Duration::days(30)),
    )
    .await;
    // Overdue: the SQL requires due_at >= now(), so past-due items drop out.
    seed_assignment(
        &pool,
        course,
        "OverdueYesterday",
        Some(Utc::now() - Duration::days(1)),
    )
    .await;
    // Undated assignments never surface in upcoming.
    seed_assignment(&pool, course, "NoDueDate", None).await;
    // Quirk: the upcoming query never joins submissions, so already-submitted
    // (graded) work still shows up while it is inside the window.
    seed_submission(&pool, in_window, user).await;

    let body = get_dashboard(&server, &auth(user, "student")).await;
    let upcoming = body["upcoming"].as_array().expect("upcoming array");

    assert_eq!(upcoming.len(), 1, "upcoming: {upcoming:?}");
    let item = &upcoming[0];
    assert_eq!(item["id"], in_window);
    assert_eq!(item["title"], "DueIn3Days");
    assert_eq!(item["type"], "assignment", "item: {item}");
    assert_eq!(item["course_name"], "WindowCourse");
    let returned_at = parse_rfc3339(&item["date"]);
    assert!(
        (returned_at - in_window_at).num_seconds().abs() < 60,
        "date drifted from the seeded due_at: {returned_at} vs {in_window_at}"
    );
    assert!(
        upcoming.iter().all(|i| i["title"] != "DueIn30Days"),
        "assignment beyond the 7-day window must not appear"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Upcoming: announcement recency window
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upcoming_includes_recent_announcements_but_not_old_ones() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "Reader", "student").await;
    let course = seed_course(&pool, &prefix, "NewsCourse").await;
    seed_enrollment(&pool, user, course, "student").await;

    let fresh = seed_announcement(&pool, course, "FreshNews", "fresh body", Utc::now()).await;
    // 10 days old: outside the "last 7 days" window for upcoming, but still a
    // valid row for recent_announcements (which has no time filter).
    let old = seed_announcement(
        &pool,
        course,
        "OldNews",
        "old body",
        Utc::now() - Duration::days(10),
    )
    .await;

    let body = get_dashboard(&server, &auth(user, "student")).await;

    let upcoming = body["upcoming"].as_array().expect("upcoming array");
    assert_eq!(upcoming.len(), 1, "upcoming: {upcoming:?}");
    assert_eq!(upcoming[0]["id"], fresh);
    assert_eq!(upcoming[0]["title"], "FreshNews");
    assert_eq!(upcoming[0]["type"], "announcement", "item: {}", upcoming[0]);
    assert_eq!(upcoming[0]["course_name"], "NewsCourse");
    assert!(
        upcoming.iter().all(|i| i["title"] != "OldNews"),
        "announcement older than 7 days must not appear in upcoming"
    );

    // The old announcement still shows in the unbounded last-3 list.
    let recent = body["recent_announcements"]
        .as_array()
        .expect("recent_announcements array");
    assert_eq!(recent.len(), 2, "recent: {recent:?}");
    assert_eq!(recent[0]["title"], "FreshNews", "newest first: {recent:?}");
    assert_eq!(recent[1]["title"], "OldNews");
    assert_eq!(recent[0]["id"], fresh);
    assert_eq!(recent[1]["id"], old);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// recent_announcements cap
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recent_announcements_capped_at_3_newest_first() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "Subscriber", "student").await;
    let course = seed_course(&pool, &prefix, "ChattyCourse").await;
    seed_enrollment(&pool, user, course, "student").await;

    // Four announcements at distinct, staggered timestamps.
    let newest_at = Utc::now() - Duration::minutes(10);
    let second_at = Utc::now() - Duration::minutes(20);
    let third_at = Utc::now() - Duration::minutes(30);
    let oldest_at = Utc::now() - Duration::minutes(40);
    let newest = seed_announcement(&pool, course, "Ann10Min", "newest", newest_at).await;
    let second = seed_announcement(&pool, course, "Ann20Min", "second", second_at).await;
    let third = seed_announcement(&pool, course, "Ann30Min", "third", third_at).await;
    let _oldest = seed_announcement(&pool, course, "Ann40Min", "oldest", oldest_at).await;

    let body = get_dashboard(&server, &auth(user, "student")).await;
    let recent = body["recent_announcements"]
        .as_array()
        .expect("recent_announcements array");

    // Capped at 3, newest first, oldest dropped.
    assert_eq!(recent.len(), 3, "recent: {recent:?}");
    let titles: Vec<&str> = recent
        .iter()
        .map(|a| a["title"].as_str().expect("title"))
        .collect();
    assert_eq!(titles, vec!["Ann10Min", "Ann20Min", "Ann30Min"]);

    // Full field shape of an announcement item.
    assert_eq!(recent[0]["id"], newest);
    assert_eq!(recent[0]["course_id"], course);
    assert_eq!(recent[0]["course_name"], "ChattyCourse");
    assert_eq!(recent[0]["message"], "newest");
    let created = parse_rfc3339(&recent[0]["created_at"]);
    assert!(
        (created - newest_at).num_seconds().abs() < 60,
        "created_at drifted from the seeded value: {created} vs {newest_at}"
    );
    assert_eq!(recent[1]["id"], second);
    assert_eq!(recent[2]["id"], third);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Multiple courses merge into every section
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_course_enrollments_merge_across_sections() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "Multi", "student").await;
    let alpha = seed_course(&pool, &prefix, "AlphaCourse").await;
    let beta = seed_course(&pool, &prefix, "BetaCourse").await;
    seed_enrollment(&pool, user, alpha, "student").await;
    seed_enrollment(&pool, user, beta, "student").await;

    // Alpha: assignment due in 5 days, announcement 2 minutes old.
    // Beta: assignment due in 2 days, announcement 1 minute old.
    seed_assignment(
        &pool,
        alpha,
        "AlphaEssay",
        Some(Utc::now() + Duration::days(5)),
    )
    .await;
    seed_announcement(
        &pool,
        alpha,
        "AlphaNews",
        "alpha body",
        Utc::now() - Duration::minutes(2),
    )
    .await;
    seed_assignment(
        &pool,
        beta,
        "BetaEssay",
        Some(Utc::now() + Duration::days(2)),
    )
    .await;
    seed_announcement(
        &pool,
        beta,
        "BetaNews",
        "beta body",
        Utc::now() - Duration::minutes(1),
    )
    .await;

    let body = get_dashboard(&server, &auth(user, "student")).await;

    // Courses section: both, ordered by name ASC.
    let courses = body["courses"].as_array().expect("courses array");
    let names: Vec<&str> = courses
        .iter()
        .map(|c| c["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, vec!["AlphaCourse", "BetaCourse"]);

    // Upcoming: merged from both courses, ordered by date ASC — the two
    // announcements (minutes old) first, then Beta's assignment (+2 days),
    // then Alpha's (+5 days).
    let upcoming = body["upcoming"].as_array().expect("upcoming array");
    let pairs: Vec<(String, String)> = upcoming
        .iter()
        .map(|i| {
            (
                i["type"].as_str().expect("type").to_string(),
                i["course_name"].as_str().expect("course_name").to_string(),
            )
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("announcement".to_string(), "AlphaCourse".to_string()),
            ("announcement".to_string(), "BetaCourse".to_string()),
            ("assignment".to_string(), "BetaCourse".to_string()),
            ("assignment".to_string(), "AlphaCourse".to_string()),
        ],
        "upcoming must merge both courses in date ASC order"
    );

    // recent_announcements: both courses, newest first.
    let recent = body["recent_announcements"]
        .as_array()
        .expect("recent_announcements array");
    let recent_names: Vec<&str> = recent
        .iter()
        .map(|a| a["course_name"].as_str().expect("course_name"))
        .collect();
    assert_eq!(recent_names, vec!["BetaCourse", "AlphaCourse"]);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Authorization: unenrolled course content stays invisible
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unenrolled_user_does_not_see_other_course_content() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let insider = seed_user(&pool, &prefix, "Insider", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let secret = seed_course(&pool, &prefix, "SecretCourse").await;
    let mine = seed_course(&pool, &prefix, "MyCourse").await;

    seed_enrollment(&pool, insider, secret, "student").await;
    seed_enrollment(&pool, outsider, mine, "student").await;

    // Both courses have in-window assignments and fresh announcements; only
    // the enrollment differs.
    seed_assignment(
        &pool,
        secret,
        "SecretHomework",
        Some(Utc::now() + Duration::days(3)),
    )
    .await;
    seed_announcement(&pool, secret, "SecretNews", "secret body", Utc::now()).await;
    seed_assignment(
        &pool,
        mine,
        "MyHomework",
        Some(Utc::now() + Duration::days(2)),
    )
    .await;
    seed_announcement(&pool, mine, "MyNews", "my body", Utc::now()).await;

    let body = get_dashboard(&server, &auth(outsider, "student")).await;

    // Courses: only their own.
    let courses = body["courses"].as_array().expect("courses array");
    assert_eq!(courses.len(), 1, "courses: {courses:?}");
    assert_eq!(courses[0]["id"], mine);

    // Upcoming: their own items only, never SecretCourse's.
    let upcoming = body["upcoming"].as_array().expect("upcoming array");
    assert_eq!(upcoming.len(), 2, "upcoming: {upcoming:?}");
    assert!(upcoming.iter().all(|i| i["course_name"] == "MyCourse"));
    let upcoming_titles: Vec<&str> = upcoming
        .iter()
        .map(|i| i["title"].as_str().expect("title"))
        .collect();
    assert_eq!(upcoming_titles, vec!["MyNews", "MyHomework"]);

    // recent_announcements: no leakage from the unenrolled course.
    let recent = body["recent_announcements"]
        .as_array()
        .expect("recent_announcements array");
    assert_eq!(recent.len(), 1, "recent: {recent:?}");
    assert_eq!(recent[0]["course_id"], mine);
    assert_eq!(recent[0]["title"], "MyNews");
    assert_eq!(recent[0]["message"], "my body");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Phase 3: GET /users/self/todo
// ---------------------------------------------------------------------------

#[tokio::test]
async fn todo_lists_grading_and_submitting_items_with_canvas_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "TodoTeacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "TodoStudent", "student").await;
    let peer = seed_user(&pool, &prefix, "TodoPeer", "student").await;

    let grading_course = seed_course(&pool, &prefix, "GradingCourse").await;
    let submitting_course = seed_course(&pool, &prefix, "SubmittingCourse").await;
    seed_enrollment(&pool, teacher, grading_course, "teacher").await;
    seed_enrollment(&pool, student, submitting_course, "student").await;
    seed_enrollment(&pool, peer, grading_course, "student").await;

    // Grading: one ungraded submission; the graded one must not appear.
    let grading_due = Utc::now() + Duration::days(3);
    let grading_assignment =
        seed_assignment(&pool, grading_course, "NeedsGrading", Some(grading_due)).await;
    seed_ungraded_submission(&pool, grading_assignment, peer).await;
    let graded = seed_assignment(
        &pool,
        grading_course,
        "AlreadyGraded",
        Some(Utc::now() + Duration::days(2)),
    )
    .await;
    seed_submission(&pool, graded, peer).await;

    // Submitting: in-window unsubmitted work (including a recent past-due one),
    // plus out-of-window and already-submitted rows that must be excluded.
    let recent_past = seed_assignment(
        &pool,
        submitting_course,
        "RecentPast",
        Some(Utc::now() - Duration::days(2)),
    )
    .await;
    let submitting_due = Utc::now() + Duration::days(2);
    let submitting_assignment = seed_assignment(
        &pool,
        submitting_course,
        "NeedsSubmitting",
        Some(submitting_due),
    )
    .await;
    seed_assignment(
        &pool,
        submitting_course,
        "FarFuture",
        Some(Utc::now() + Duration::days(20)),
    )
    .await;
    seed_assignment(
        &pool,
        submitting_course,
        "FarPast",
        Some(Utc::now() - Duration::days(40)),
    )
    .await;

    // Teacher: exactly one grading item with the Canvas todo shape.
    let resp = server
        .get("/users/self/todo")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let todo: Value = resp.json();
    let items = todo.as_array().expect("todo array");
    assert_eq!(items.len(), 1, "teacher todo: {todo}");
    let item = &items[0];
    assert_eq!(item["type"], "grading");
    assert_eq!(item["context_type"], "Course");
    assert_eq!(item["course_id"], grading_course);
    assert_eq!(item["context_name"], "GradingCourse");
    assert_eq!(
        item["context_short_name"],
        format!("{prefix}-GradingCourse")
    );
    assert_eq!(item["needs_grading_count"], 1);
    assert_eq!(item["assignment"]["id"], grading_assignment);
    assert_eq!(item["assignment"]["course_id"], grading_course);
    assert_eq!(item["assignment"]["title"], "NeedsGrading");
    assert_eq!(
        item["html_url"],
        format!(
            "/courses/{grading_course}/gradebook/speed_grader?assignment_id={grading_assignment}"
        )
    );
    assert_eq!(
        item["ignore"],
        format!("/api/v1/users/self/todo/assignment_{grading_assignment}/grading?permanent=0")
    );
    assert_eq!(
        item["ignore_permanently"],
        format!("/api/v1/users/self/todo/assignment_{grading_assignment}/grading?permanent=1")
    );

    // Student: submitting items ordered by due date, no needs_grading_count.
    let resp = server
        .get("/users/self/todo")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let todo: Value = resp.json();
    let items = todo.as_array().expect("todo array");
    assert_eq!(items.len(), 2, "student todo: {todo}");
    assert_eq!(items[0]["type"], "submitting");
    assert_eq!(items[0]["assignment"]["id"], recent_past);
    assert_eq!(items[1]["type"], "submitting");
    assert_eq!(items[1]["assignment"]["id"], submitting_assignment);
    assert_eq!(items[1]["assignment"]["title"], "NeedsSubmitting");
    assert!(
        items.iter().all(|i| i.get("needs_grading_count").is_none()),
        "submitting items must omit needs_grading_count: {todo}"
    );
    assert_eq!(
        items[1]["html_url"],
        format!("/courses/{submitting_course}/assignments/{submitting_assignment}#submit")
    );

    // Canvas returns an empty list when course_ids[] filters everything out.
    let resp = server
        .get("/users/self/todo?course_ids%5B%5D=0")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let filtered: Value = resp.json();
    assert_eq!(filtered.as_array().expect("todo array").len(), 0);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Phase 3: GET /users/self/upcoming_events
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upcoming_events_merges_assignments_and_visible_calendar_events() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "Upcoming", "student").await;
    let course = seed_course(&pool, &prefix, "UpcomingCourse").await;
    let other_course = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, user, course, "student").await;

    let base = Utc::now();
    let due_at = base + Duration::days(2);
    let assignment = seed_assignment(&pool, course, "UpcomingHomework", Some(due_at)).await;
    let course_event = seed_calendar_event(
        &pool,
        Some(course),
        None,
        "CourseEvent",
        base + Duration::days(1),
        "active",
    )
    .await;
    let personal_event = seed_calendar_event(
        &pool,
        None,
        Some(user),
        "PersonalEvent",
        base + Duration::days(3),
        "active",
    )
    .await;
    // Deleted, past, out-of-window, and unenrolled-course events are excluded.
    seed_calendar_event(
        &pool,
        Some(course),
        None,
        "DeletedEvent",
        base + Duration::days(1),
        "deleted",
    )
    .await;
    seed_calendar_event(
        &pool,
        Some(course),
        None,
        "PastEvent",
        base - Duration::days(1),
        "active",
    )
    .await;
    seed_calendar_event(
        &pool,
        Some(course),
        None,
        "TooFarEvent",
        base + Duration::days(10),
        "active",
    )
    .await;
    seed_calendar_event(
        &pool,
        Some(other_course),
        None,
        "UnenrolledEvent",
        base + Duration::days(1),
        "active",
    )
    .await;

    let resp = server
        .get("/users/self/upcoming_events")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let events = body.as_array().expect("upcoming_events array");

    assert_eq!(events.len(), 3, "events: {body}");
    // Ordered by start_at: course event, assignment, personal event.
    assert_eq!(events[0]["type"], "event");
    assert_eq!(events[0]["id"], course_event);
    assert_eq!(events[0]["title"], "CourseEvent");
    assert_eq!(events[0]["context_code"], format!("course_{course}"));
    assert_eq!(events[0]["context_name"], "UpcomingCourse");
    assert_eq!(events[0]["all_day"], false);
    assert_eq!(
        events[0]["url"],
        format!("/api/v1/calendar_events/{course_event}")
    );

    assert_eq!(events[1]["type"], "assignment");
    assert_eq!(events[1]["id"], format!("assignment_{assignment}"));
    assert_eq!(events[1]["assignment"]["id"], assignment);
    assert_eq!(events[1]["context_code"], format!("course_{course}"));
    assert_eq!(
        events[1]["html_url"],
        format!("/courses/{course}/assignments/{assignment}")
    );
    assert_eq!(
        events[1]["url"],
        format!("/api/v1/calendar_events/assignment_{assignment}")
    );
    let start = parse_rfc3339(&events[1]["start_at"]);
    assert!(
        (start - due_at).num_seconds().abs() < 60,
        "assignment start_at: {start}"
    );

    assert_eq!(events[2]["type"], "event");
    assert_eq!(events[2]["id"], personal_event);
    assert_eq!(events[2]["title"], "PersonalEvent");
    assert_eq!(events[2]["context_code"], format!("user_{user}"));

    assert!(
        events.iter().all(|e| e["title"] != "DeletedEvent"
            && e["title"] != "PastEvent"
            && e["title"] != "TooFarEvent"
            && e["title"] != "UnenrolledEvent"),
        "excluded events leaked: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Phase 3: GET /dashboard/dashboard_cards
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dashboard_cards_return_menu_cards_ordered_by_position() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "CardsUser", "student").await;
    let alpha = seed_course(&pool, &prefix, "AlphaCards").await;
    let zebra = seed_course(&pool, &prefix, "ZebraCards").await;
    let hidden = seed_course(&pool, &prefix, "HiddenCards").await;
    seed_enrollment(&pool, user, alpha, "teacher").await;
    seed_enrollment(&pool, user, zebra, "student").await;

    // Positions override the name ordering: Zebra (position 1) before Alpha (2).
    seed_dashboard_position(&pool, user, zebra, 1).await;
    seed_dashboard_position(&pool, user, alpha, 2).await;

    let resp = server
        .get("/dashboard/dashboard_cards")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let cards = body.as_array().expect("dashboard_cards array");

    assert_eq!(cards.len(), 2, "cards: {body}");
    let zebra_code = format!("{prefix}-ZebraCards");
    let card = &cards[0];
    assert_eq!(card["id"], zebra);
    assert_eq!(card["longName"], format!("ZebraCards - {zebra_code}"));
    assert_eq!(card["shortName"], zebra_code);
    assert_eq!(card["originalName"], "ZebraCards");
    assert_eq!(card["courseCode"], zebra_code);
    assert_eq!(card["assetString"], format!("course_{zebra}"));
    assert_eq!(card["href"], format!("/courses/{zebra}"));
    assert!(card["term"].is_null(), "term: {}", card["term"]);
    assert_eq!(card["subtitle"], "enrolled as Student");
    assert_eq!(card["enrollmentState"], "active");
    assert_eq!(card["enrollmentType"], "StudentEnrollment");
    assert!(card["observee"].is_null());
    assert_eq!(card["isFavorited"], false);
    assert_eq!(card["isK5Subject"], false);
    assert_eq!(card["isHomeroom"], false);
    assert_eq!(card["useClassicFont"], false);
    assert_eq!(card["canManage"], false);
    assert_eq!(card["canReadAnnouncements"], true);
    assert!(card["image"].is_null());
    assert!(card["color"].is_null());
    assert_eq!(card["position"], 1);
    assert_eq!(card["published"], true);
    assert_eq!(card["canChangeCoursePublishState"], false);
    assert_eq!(card["defaultView"], "modules");
    assert_eq!(card["pagesUrl"], format!("/courses/{zebra}/pages"));
    assert!(card["frontPageTitle"].is_null());
    let links = card["links"].as_array().expect("links array");
    let link_ids: Vec<&str> = links
        .iter()
        .map(|l| l["css_class"].as_str().expect("css_class"))
        .collect();
    assert_eq!(
        link_ids,
        vec!["discussions", "assignments", "announcements", "files"]
    );
    assert_eq!(links[1]["path"], format!("/courses/{zebra}/assignments"));
    assert_eq!(links[1]["label"], "Assignments");
    assert_eq!(links[1]["hidden"], false);

    let card = &cards[1];
    assert_eq!(card["id"], alpha);
    assert_eq!(card["position"], 2);
    assert_eq!(card["subtitle"], "enrolled as Teacher");
    assert_eq!(card["enrollmentType"], "TeacherEnrollment");
    assert_eq!(card["canManage"], true);
    assert_eq!(card["canChangeCoursePublishState"], true);
    assert!(
        cards.iter().all(|c| c["id"] != hidden),
        "unenrolled course leaked: {body}"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Phase 3: auth on the new routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn new_dashboard_routes_require_authentication() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    for path in [
        "/users/self/todo",
        "/users/self/upcoming_events",
        "/dashboard/dashboard_cards",
    ] {
        let resp = server.get(path).await;
        assert_eq!(resp.status_code(), 401, "{path} without token");
        let resp = server
            .get(path)
            .add_header(axum::http::header::AUTHORIZATION, "Bearer garbage-token")
            .await;
        assert_eq!(resp.status_code(), 401, "{path} with garbage token");
    }
}
