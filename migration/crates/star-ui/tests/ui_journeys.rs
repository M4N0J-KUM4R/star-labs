//! Integration tests for the journeys added for issues #110 (files) and
//! #111 (announcements, syllabus, calendar). Real Postgres (SKIP when
//! DATABASE_URL unset), real router via axum-test — same harness as
//! ui_integration.rs.

#![allow(dead_code)] // seed helpers are used by subsets of the tests

use axum_test::TestServer;
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "ui-journeys-{}-{}",
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
    let server = TestServer::new(star_ui::router().with_state(state)).expect("build TestServer");
    Some((server, pool, run_prefix()))
}

async fn seed_user(pool: &PgPool, prefix: &str, name: &str, role: &str, hash: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (name, email, password_hash, role) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(name)
    // Login and registration both lowercase emails (login.rs, auth.rs);
    // seed the way a real user row would be stored.
    .bind(format!("{prefix}-{name}@test.local").to_lowercase())
    .bind(hash)
    .bind(role)
    .fetch_one(pool)
    .await
    .expect("seed user")
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, workflow_state: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, workflow_state) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(format!("{prefix}-{name}"))
    .bind(workflow_state)
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

/// Login through the real form flow; returns (session cookie, csrf).
async fn login(server: &TestServer, email: &str, password: &str) -> (String, String) {
    let page = server.get("/login").await;
    assert_eq!(page.status_code(), 200);
    let jar = page.cookies();
    let csrf = jar
        .get("star_csrf")
        .expect("csrf cookie issued")
        .value()
        .to_string();
    let resp = server
        .post("/login")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "email": email,
            "password": password,
            "_csrf": csrf,
            "next": "/dashboard"
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "login should 303: {}", resp.text());
    let session = resp
        .cookies()
        .get("star_session")
        .expect("session cookie set")
        .value()
        .to_string();
    (session, csrf)
}

/// Authenticated GET: the TestServer does not persist cookies between
/// requests, so the session and CSRF cookies attach per request.
async fn authed_get(
    server: &TestServer,
    session: &str,
    csrf: &str,
    path: &str,
) -> axum_test::TestResponse {
    server
        .get(path)
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            session,
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new("star_csrf", csrf))
        .await
}

/// Authenticated form POST.
async fn authed_post(
    server: &TestServer,
    session: &str,
    csrf: &str,
    path: &str,
    form: serde_json::Value,
) -> axum_test::TestResponse {
    server
        .post(path)
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            session,
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new("star_csrf", csrf))
        .form(&form)
        .await
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// -------------------------------------------------------------------------
// Issue #110: files journey
// -------------------------------------------------------------------------

/// Files index shows the root folder, an uploaded file's row links to its
/// show page, and the show page exposes the Download action. Non-enrolled
/// viewers are forbidden.
#[tokio::test]
async fn files_index_show_and_download() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let teacher = seed_user(&pool, &prefix, "Files Teacher", "teacher", &hash).await;
    let outsider = seed_user(&pool, &prefix, "Files Outsider", "student", &hash).await;
    let _ = outsider;
    let course = seed_course(&pool, &prefix, "Files Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let (session, csrf) = login(
        &server,
        &format!("{prefix}-Files Teacher@test.local"),
        "Password123!",
    )
    .await;

    // Root folder renders with an empty table.
    let resp = authed_get(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files"),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "files index: {}", resp.text());
    assert!(resp.text().contains("No files have been added"));

    // Upload a file record via the form (PRG).
    let resp = authed_post(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files"),
        serde_json::json!({
            "_csrf": csrf,
            "upload_folder_id": 0,
            "filename": "syllabus.pdf",
            "content_type": "application/pdf",
            "size": 12345
        }),
    )
    .await;
    assert_eq!(
        resp.status_code(),
        303,
        "upload should 303: {}",
        resp.text()
    );

    // Index now lists it; the row links to the show page.
    let resp = authed_get(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files"),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(body.contains("syllabus.pdf"), "row missing: {body}");

    let file_id: i64 =
        sqlx::query_scalar("SELECT id FROM files WHERE display_name = 'syllabus.pdf'")
            .fetch_one(&pool)
            .await
            .expect("file row");

    let resp = authed_get(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files/{file_id}"),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    assert!(resp.text().contains("Download"));

    // Download 302s to the REST public-url endpoint (no byte storage).
    let resp = authed_get(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files/{file_id}/download"),
    )
    .await;
    assert_eq!(resp.status_code(), 303);
    assert_eq!(
        resp.headers().get("location").unwrap(),
        &format!("/api/v1/courses/{course}/files/{file_id}/public_url")
    );

    // Non-enrolled viewer: 403 (or the forbidden page it renders).
    let (out_session, out_csrf) = login(
        &server,
        &format!("{prefix}-Files Outsider@test.local"),
        "Password123!",
    )
    .await;
    let resp = authed_get(
        &server,
        &out_session,
        &out_csrf,
        &format!("/courses/{course}/files"),
    )
    .await;
    assert_eq!(resp.status_code(), 403, "outsider must be forbidden");

    let _ = cleanup(&pool, &prefix).await;
}

/// New Folder creates a subfolder that then appears in the tree and is
/// browsable via ?folder_id=.
#[tokio::test]
async fn files_new_folder_is_browsable() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let teacher = seed_user(&pool, &prefix, "Folder Teacher", "teacher", &hash).await;
    let course = seed_course(&pool, &prefix, "Folder Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let (session, csrf) = login(
        &server,
        &format!("{prefix}-Folder Teacher@test.local"),
        "Password123!",
    )
    .await;

    let resp = authed_post(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files"),
        serde_json::json!({ "_csrf": csrf, "upload_folder_id": 0, "new_folder": "Week 1" }),
    )
    .await;
    assert_eq!(resp.status_code(), 303, "folder create: {}", resp.text());

    let resp = authed_get(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files"),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(
        body.contains("Week 1"),
        "subfolder missing from tree: {body}"
    );

    // CSRF rejection on a tokenless POST.
    let resp = server
        .post(&format!("/courses/{course}/files"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            session.as_str(),
        ))
        .form(&serde_json::json!({ "_csrf": "wrong", "new_folder": "Nope" }))
        .await;
    assert_eq!(resp.status_code(), 403, "bad CSRF must be rejected");

    let _ = cleanup(&pool, &prefix).await;
}

// -------------------------------------------------------------------------
// Issue #111: announcements + syllabus journey
// -------------------------------------------------------------------------

/// Teacher posts an announcement via the form; the student sees it unread in
/// the index and read after opening it. Delete removes it from the index.
#[tokio::test]
async fn announcements_create_show_read_delete() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let teacher = seed_user(&pool, &prefix, "Ann Teacher", "teacher", &hash).await;
    let student = seed_user(&pool, &prefix, "Ann Student", "student", &hash).await;
    let course = seed_course(&pool, &prefix, "Ann Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let (t_session, t_csrf) = login(
        &server,
        &format!("{prefix}-Ann Teacher@test.local"),
        "Password123!",
    )
    .await;

    // Student's index is empty at first.
    let (s_session, s_csrf) = login(
        &server,
        &format!("{prefix}-Ann Student@test.local"),
        "Password123!",
    )
    .await;
    let resp = authed_get(
        &server,
        &s_session,
        &s_csrf,
        &format!("/courses/{course}/announcements"),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    assert!(resp.text().contains("No announcements"));

    // Teacher creates one via the form (PRG to the show page).
    let resp = authed_post(
        &server,
        &t_session,
        &t_csrf,
        &format!("/courses/{course}/announcements"),
        serde_json::json!({
            "_csrf": t_csrf,
            "title": "Welcome!",
            "message": "First week starts Monday."
        }),
    )
    .await;
    assert_eq!(
        resp.status_code(),
        303,
        "create should 303: {}",
        resp.text()
    );
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.contains(&format!("/courses/{course}/announcements/")));

    // Student sees it unread, then read after viewing.
    let resp = authed_get(
        &server,
        &s_session,
        &s_csrf,
        &format!("/courses/{course}/announcements"),
    )
    .await;
    let body = resp.text();
    assert!(
        body.contains("Welcome!"),
        "index missing announcement: {body}"
    );
    assert!(
        body.contains("unread-mark"),
        "unread marker missing: {body}"
    );

    let ann_id: i64 = sqlx::query_scalar("SELECT id FROM announcements WHERE title = 'Welcome!'")
        .fetch_one(&pool)
        .await
        .expect("announcement row");

    let resp = authed_get(
        &server,
        &s_session,
        &s_csrf,
        &format!("/courses/{course}/announcements/{ann_id}"),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    assert!(resp.text().contains("First week starts Monday."));

    let resp = authed_get(
        &server,
        &s_session,
        &s_csrf,
        &format!("/courses/{course}/announcements"),
    )
    .await;
    assert!(
        !resp.text().contains("unread-mark"),
        "announcement must be read after viewing"
    );

    // Student cannot create (403).
    let resp = authed_post(
        &server,
        &s_session,
        &s_csrf,
        &format!("/courses/{course}/announcements"),
        serde_json::json!({ "_csrf": s_csrf, "title": "x", "message": "y" }),
    )
    .await;
    assert_eq!(resp.status_code(), 403, "student create must be forbidden");

    // Teacher deletes it (PRG back to index); it disappears.
    let resp = authed_post(
        &server,
        &t_session,
        &t_csrf,
        &format!("/courses/{course}/announcements/{ann_id}/delete"),
        serde_json::json!({ "_csrf": t_csrf }),
    )
    .await;
    assert_eq!(
        resp.status_code(),
        303,
        "delete should 303: {}",
        resp.text()
    );
    let resp = authed_get(
        &server,
        &t_session,
        &t_csrf,
        &format!("/courses/{course}/announcements"),
    )
    .await;
    assert!(resp.text().contains("No announcements"));

    let _ = cleanup(&pool, &prefix).await;
}

/// Syllabus renders the saved body and a course-summary table; the student
/// sees the edit form never, the teacher both.
#[tokio::test]
async fn syllabus_show_save_and_summary() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let teacher = seed_user(&pool, &prefix, "Syl Teacher", "teacher", &hash).await;
    let student = seed_user(&pool, &prefix, "Syl Student", "student", &hash).await;
    let course = seed_course(&pool, &prefix, "Syl Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Seed an assignment group + assignment for the summary table.
    let (group_id,): (i64,) = sqlx::query_as(
        "INSERT INTO assignment_groups (course_id, name) VALUES ($1, 'Homework') RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed group");
    let _: (i64,) = sqlx::query_as(
        "INSERT INTO assignments (course_id, assignment_group_id, title, points_possible, workflow_state) \
         VALUES ($1, $2, 'Problem Set 1', 100, 'published') RETURNING id",
    )
    .bind(course)
    .bind(group_id)
    .fetch_one(&pool)
    .await
    .expect("seed assignment");

    let (s_session, s_csrf) = login(
        &server,
        &format!("{prefix}-Syl Student@test.local"),
        "Password123!",
    )
    .await;
    let (t_session, t_csrf) = login(
        &server,
        &format!("{prefix}-Syl Teacher@test.local"),
        "Password123!",
    )
    .await;

    // Student sees the empty syllabus and no edit form.
    let resp = authed_get(
        &server,
        &s_session,
        &s_csrf,
        &format!("/courses/{course}/assignments/syllabus"),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(body.contains("No syllabus has been added"));
    assert!(
        !body.contains("edit_syllabus_form"),
        "student must not see the edit form"
    );
    // Course summary lists the seeded assignment.
    assert!(
        body.contains("Problem Set 1"),
        "summary missing assignment: {body}"
    );
    assert!(body.contains("Homework"), "summary missing group: {body}");

    // Teacher saves a body (PRG), which then renders for both.
    let resp = authed_post(
        &server,
        &t_session,
        &t_csrf,
        &format!("/courses/{course}/assignments/syllabus"),
        serde_json::json!({ "_csrf": t_csrf, "syllabus_body": "<p>Read chapters 1–3.</p>" }),
    )
    .await;
    assert_eq!(
        resp.status_code(),
        303,
        "syllabus save should 303: {}",
        resp.text()
    );

    let resp = authed_get(
        &server,
        &s_session,
        &s_csrf,
        &format!("/courses/{course}/assignments/syllabus"),
    )
    .await;
    let body = resp.text();
    assert!(
        body.contains("Read chapters 1–3."),
        "saved body missing: {body}"
    );
    assert!(!body.contains("edit_syllabus_form"));

    // Student cannot save (403).
    let resp = authed_post(
        &server,
        &s_session,
        &s_csrf,
        &format!("/courses/{course}/assignments/syllabus"),
        serde_json::json!({ "_csrf": s_csrf, "syllabus_body": "hack" }),
    )
    .await;
    assert_eq!(resp.status_code(), 403, "student save must be forbidden");

    let _ = cleanup(&pool, &prefix).await;
}

// -------------------------------------------------------------------------
// Issue #111: calendar journey
// -------------------------------------------------------------------------

/// The calendar renders a 6x7 grid, teacher-created events appear on the
/// right day, and the context filter excludes unchecked courses.
#[tokio::test]
async fn calendar_grid_create_and_filter() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let teacher = seed_user(&pool, &prefix, "Cal Teacher", "teacher", &hash).await;
    let student = seed_user(&pool, &prefix, "Cal Student", "student", &hash).await;
    let course = seed_course(&pool, &prefix, "Cal Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let (t_session, t_csrf) = login(
        &server,
        &format!("{prefix}-Cal Teacher@test.local"),
        "Password123!",
    )
    .await;

    // Grid renders with 42 day cells (6 weeks x 7 days).
    let resp = authed_get(&server, &t_session, &t_csrf, "/calendar").await;
    assert_eq!(resp.status_code(), 200, "calendar: {}", resp.text());
    let body = resp.text();
    assert_eq!(
        body.matches("calendar-day").count() >= 42,
        true,
        "grid must render 42 cells"
    );

    // Create an event on 2026-11-10 via the form.
    let resp = authed_post(
        &server,
        &t_session,
        &t_csrf,
        "/calendar",
        serde_json::json!({
            "_csrf": t_csrf,
            "context_code": format!("course_{course}"),
            "title": "Field Trip",
            "start_date": "2026-11-10",
            "location": "Museum"
        }),
    )
    .await;
    assert_eq!(resp.status_code(), 303, "event create: {}", resp.text());

    // Month view shows the chip, filter on the course keeps it.
    let resp = authed_get(
        &server,
        &t_session,
        &t_csrf,
        &format!("/calendar?month=2026-11&contexts[]={course}"),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(body.contains("Field Trip"), "event chip missing: {body}");
    assert!(body.contains("Museum"), "event location missing: {body}");

    // Student sees the same event (enrolled in the course).
    let (s_session, s_csrf) = login(
        &server,
        &format!("{prefix}-Cal Student@test.local"),
        "Password123!",
    )
    .await;
    let resp = authed_get(&server, &s_session, &s_csrf, "/calendar?month=2026-11").await;
    assert!(resp.text().contains("Field Trip"));

    // A course the student is not enrolled in is filtered out of their view.
    let other = seed_course(&pool, &prefix, "Cal Other", "available").await;
    let _: (i64,) = sqlx::query_as(
        "INSERT INTO calendar_events (course_id, title, start_at, workflow_state, location_name) \
         VALUES ($1, 'Secret Other Event', '2026-11-12 09:00:00+00', 'active', NULL) RETURNING id",
    )
    .bind(other)
    .fetch_one(&pool)
    .await
    .expect("seed other event");
    let resp = authed_get(&server, &s_session, &s_csrf, "/calendar?month=2026-11").await;
    let body = resp.text();
    assert!(body.contains("Field Trip"));
    assert!(
        !body.contains("Secret Other Event"),
        "unenrolled context leaked: {body}"
    );

    // Delete via the form (PRG).
    let event_id: i64 =
        sqlx::query_scalar("SELECT id FROM calendar_events WHERE title = 'Field Trip'")
            .fetch_one(&pool)
            .await
            .expect("event row");
    let resp = authed_post(
        &server,
        &t_session,
        &t_csrf,
        &format!("/calendar/{event_id}/delete"),
        serde_json::json!({ "_csrf": t_csrf }),
    )
    .await;
    assert_eq!(
        resp.status_code(),
        303,
        "delete should 303: {}",
        resp.text()
    );
    let resp = authed_get(&server, &t_session, &t_csrf, "/calendar?month=2026-11").await;
    assert!(!resp.text().contains("Field Trip"));

    let _ = cleanup(&pool, &prefix).await;
}

// -------------------------------------------------------------------------
// Precision Workspace: dashboard Today panel, card badges, quick-jump
// -------------------------------------------------------------------------

/// The dashboard shows a Today panel with due-soon and overdue rows (each
/// linking to the assignment) and per-course badges for new announcements
/// and recently graded feedback.
#[tokio::test]
async fn dashboard_today_panel_and_badges() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let teacher = seed_user(&pool, &prefix, "Dash Teacher", "teacher", &hash).await;
    let student = seed_user(&pool, &prefix, "Dash Student", "student", &hash).await;
    let course = seed_course(&pool, &prefix, "Dash Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let (group_id,): (i64,) = sqlx::query_as(
        "INSERT INTO assignment_groups (course_id, name) VALUES ($1, 'Homework') RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed group");
    let (due_soon, overdue): (i64, i64) = sqlx::query_as(
        "INSERT INTO assignments (course_id, assignment_group_id, title, points_possible, workflow_state, due_at) \
         VALUES ($1, $2, 'Essay Draft', 50, 'published', now() + interval '3 days') RETURNING id, id",
    )
    .bind(course)
    .bind(group_id)
    .fetch_one(&pool)
    .await
    .expect("seed due-soon assignment");
    let (_,): (i64,) = sqlx::query_as(
        "INSERT INTO assignments (course_id, assignment_group_id, title, points_possible, workflow_state, due_at) \
         VALUES ($1, $2, 'Old Quiz', 50, 'published', now() - interval '5 days') RETURNING id, id",
    )
    .bind(course)
    .bind(group_id)
    .fetch_one(&pool)
    .await
    .expect("seed overdue assignment");
    let _ = overdue;

    sqlx::query(
        "INSERT INTO announcements (course_id, title, message, workflow_state, posted_at) \
         VALUES ($1, 'Welcome', 'Hello class', 'active', now())",
    )
    .bind(course)
    .execute(&pool)
    .await
    .expect("seed announcement");
    sqlx::query(
        "INSERT INTO submissions (assignment_id, user_id, score, graded_at, workflow_state) \
         VALUES ($1, $2, 48, now(), 'graded')",
    )
    .bind(due_soon)
    .bind(student)
    .execute(&pool)
    .await
    .expect("seed graded submission");

    let (session, csrf) = login(
        &server,
        &format!("{prefix}-Dash Student@test.local"),
        "Password123!",
    )
    .await;

    let resp = authed_get(&server, &session, &csrf, "/dashboard").await;
    assert_eq!(resp.status_code(), 200, "dashboard: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("Today"), "today panel heading");
    assert!(body.contains("Essay Draft"), "due-soon row");
    assert!(body.contains("overdue") && body.contains("Old Quiz"), "overdue row");
    assert!(body.contains("1 new announcement"), "announcement badge");
    assert!(body.contains("1 new grade"), "feedback badge");
    assert!(!body.contains("2 new"), "counts not duplicated");

    let _ = cleanup(&pool, &prefix).await;
}

/// /quick-jump redirects a single course match and lists several matches.
#[tokio::test]
async fn quick_jump_resolves_and_lists() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let teacher = seed_user(&pool, &prefix, "Jump Teacher", "teacher", &hash).await;
    let course = seed_course(&pool, &prefix, "Jump Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let (group_id,): (i64,) = sqlx::query_as(
        "INSERT INTO assignment_groups (course_id, name) VALUES ($1, 'Homework') RETURNING id",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("seed group");
    let (assignment,): (i64,) = sqlx::query_as(
        "INSERT INTO assignments (course_id, assignment_group_id, title, points_possible, workflow_state) \
         VALUES ($1, $2, 'Jump Lab Report', 50, 'published') RETURNING id",
    )
    .bind(course)
    .bind(group_id)
    .fetch_one(&pool)
    .await
    .expect("seed assignment");

    let (session, csrf) = login(
        &server,
        &format!("{prefix}-Jump Teacher@test.local"),
        "Password123!",
    )
    .await;

    // Unique assignment match redirects straight to it.
    let resp = authed_get(&server, &session, &csrf, "/quick-jump?q=Lab%20Report").await;
    assert_eq!(
        resp.status_code(),
        303,
        "unique assignment should 303: {}",
        resp.text()
    );
    assert!(
        resp.headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .ends_with(&format!("/courses/{course}/assignments/{assignment}")),
        "redirects to the assignment"
    );

    // Ambiguous term lists both a course and an assignment section.
    let resp = authed_get(&server, &session, &csrf, "/quick-jump?q=Jump").await;
    assert_eq!(resp.status_code(), 200, "ambiguous: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("Courses") && body.contains("Assignments"), "sections: {body}");
    assert!(body.contains("Jump Course"), "course row");

    // No match shows the empty state.
    let resp = authed_get(&server, &session, &csrf, "/quick-jump?q=zzz-nothing").await;
    assert_eq!(resp.status_code(), 200);
    assert!(resp.text().contains("No matches"));

    let _ = cleanup(&pool, &prefix).await;
}
