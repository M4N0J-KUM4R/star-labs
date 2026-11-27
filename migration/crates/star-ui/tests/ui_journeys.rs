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
    let server = TestServer::new(star_ui::app(state)).expect("build TestServer");
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

/// A folder in the course's tree (`parent = None` is a root folder).
async fn seed_folder(pool: &PgPool, course_id: i64, parent: Option<i64>, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO folders (name, full_name, context_type, context_id, parent_folder_id) \
         VALUES ($1, $2, 'Course', $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(name.to_lowercase())
    .bind(course_id)
    .bind(parent)
    .fetch_one(pool)
    .await
    .expect("seed folder")
}

/// A file row inside `folder_id` — the workspace stores metadata only, so a
/// page-seeded file is the same shape an upload would leave behind.
async fn seed_file(
    pool: &PgPool,
    course_id: i64,
    uploader_id: i64,
    folder_id: i64,
    name: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO files (course_id, uploader_id, filename, display_name, content_type, size, folder_id) \
         VALUES ($1, $2, $3, $3, 'application/pdf', 12345, $4) RETURNING id",
    )
    .bind(course_id)
    .bind(uploader_id)
    .bind(name)
    .bind(folder_id)
    .fetch_one(pool)
    .await
    .expect("seed file")
}

/// A calendar event on the course calendar.
async fn seed_calendar_event(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    start_at: &str,
    location_name: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO calendar_events (course_id, title, start_at, workflow_state, location_name) \
         VALUES ($1, $2, $3::timestamptz, 'active', $4) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(start_at)
    .bind(location_name)
    .fetch_one(pool)
    .await
    .expect("seed calendar event")
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
        .post("/login/canvas")
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
    let _ = sqlx::query(
        "DELETE FROM folders WHERE context_type = 'Course' AND context_id IN \
         (SELECT id FROM courses WHERE course_code LIKE $1)",
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

// -------------------------------------------------------------------------
// Issue #110: files journey
// -------------------------------------------------------------------------

/// Files lists a seeded file and its folder tree, and `?folder_id=` browses
/// into a subfolder. The pages are read-only on purpose: Canvas routes
/// `POST .../files` to `files#create`, an action `FilesController` does not
/// define (real uploads are `POST /api/v1/courses/:course_id/files` =>
/// `courses#create_file`), and `.../files/:id/download` streams attachment
/// bytes this workspace does not store. Neither answer here.
#[tokio::test]
async fn files_index_and_show_are_read_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let teacher = seed_user(&pool, &prefix, "Files Teacher", "teacher", &hash).await;
    let outsider = seed_user(&pool, &prefix, "Files Outsider", "student", &hash).await;
    let course = seed_course(&pool, &prefix, "Files Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let root = seed_folder(&pool, course, None, "course files").await;
    let sub = seed_folder(&pool, course, Some(root), "Week 1").await;
    let file_name = format!("{prefix}-syllabus.pdf");
    let file_id = seed_file(&pool, course, teacher, root, &file_name).await;

    let (session, csrf) = login(
        &server,
        &format!("{prefix}-Files Teacher@test.local"),
        "Password123!",
    )
    .await;

    // Root folder lists the file row and the subfolder tree.
    let resp = authed_get(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files"),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "files index: {}", resp.text());
    let body = resp.text();
    assert!(body.contains(&file_name), "row missing: {body}");
    assert!(
        body.contains("Week 1"),
        "subfolder missing from tree: {body}"
    );

    // ?folder_id= browses into the subfolder, which has no files yet.
    let resp = authed_get(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files?folder_id={sub}"),
    )
    .await;
    assert_eq!(resp.status_code(), 200, "subfolder: {}", resp.text());
    assert!(resp.text().contains("No files have been added"));

    // Show page carries the metadata and no Download control.
    let resp = authed_get(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files/{file_id}"),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(body.contains(&file_name), "show body: {body}");
    assert!(
        !body.contains("Download"),
        "show page must not offer a download: {body}"
    );

    // The unrouted write verbs: POST on a GET-only collection is 405, and the
    // `/download` sibling is an unknown path.
    let resp = authed_post(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files"),
        serde_json::json!({ "_csrf": csrf, "filename": "late.pdf" }),
    )
    .await;
    assert_eq!(resp.status_code(), 405, "upload must not be routed");
    let resp = authed_get(
        &server,
        &session,
        &csrf,
        &format!("/courses/{course}/files/{file_id}/download"),
    )
    .await;
    assert_eq!(resp.status_code(), 404, "download must not be routed");

    let outsider_rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM enrollments WHERE user_id = $1 AND course_id = $2",
    )
    .bind(outsider)
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("outsider enrollment count");
    assert_eq!(
        outsider_rows, 0,
        "the 403 below only means something if the viewer is unenrolled"
    );

    // Non-enrolled viewer: 403 (the same gate the REST list applies).
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

// -------------------------------------------------------------------------
// Issue #111: announcements + syllabus journey
// -------------------------------------------------------------------------

/// Teacher posts an announcement via the form; the student sees it unread in
/// the index and read after opening it. The write verbs Canvas reserves for
/// the row itself (member DELETE) are not registered here.
#[tokio::test]
async fn announcements_create_show_and_mark_read() {
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

    let ann_id: i64 = sqlx::query_scalar(
        "SELECT id FROM announcements WHERE course_id = $1 AND title = 'Welcome!'",
    )
    .bind(course)
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

    // Deleting a row is a member DELETE in Canvas' route table (`resources
    // :announcements`, config/routes.rb:154-155) — the shell's `_method`
    // shim could reach that verb — but AnnouncementsController implements no
    // destroy action, so Canvas 500s there and this port registers neither
    // verb: the old `/delete` path is unrouted and the member route only
    // answers GET. The show page offers no control.
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
        404,
        "the delete form verb must not be routed: {}",
        resp.text()
    );

    let resp = server
        .delete(&format!("/courses/{course}/announcements/{ann_id}"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            t_session.as_str(),
        ))
        .await;
    assert_eq!(
        resp.status_code(),
        405,
        "member DELETE must not be routed: {}",
        resp.text()
    );

    let resp = authed_get(
        &server,
        &t_session,
        &t_csrf,
        &format!("/courses/{course}/announcements/{ann_id}"),
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    assert!(
        !resp.text().contains("Delete Announcement"),
        "show page must not offer a delete button: {}",
        resp.text()
    );

    let _ = cleanup(&pool, &prefix).await;
}

/// Syllabus renders the stored body and the course-summary table for both
/// roles, and shows no editor. Canvas registers this page as a collection GET
/// inside `resources :assignments` (config/routes.rb:344-346); the body is
/// saved with the course's syllabus assignment, not by posting to this path,
/// so the write verb is absent.
#[tokio::test]
async fn syllabus_show_and_summary_are_read_only() {
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

    let path = format!("/courses/{course}/assignments/syllabus");

    // Both roles see the empty state and the summary; neither sees an editor.
    for (session, csrf) in [(&s_session, &s_csrf), (&t_session, &t_csrf)] {
        let resp = authed_get(&server, session, csrf, &path).await;
        assert_eq!(resp.status_code(), 200, "syllabus: {}", resp.text());
        let body = resp.text();
        assert!(body.contains("No syllabus has been added"), "body: {body}");
        assert!(
            body.contains("Problem Set 1"),
            "summary missing assignment: {body}"
        );
        assert!(body.contains("Homework"), "summary missing group: {body}");
        assert!(
            !body.contains("edit_syllabus_form"),
            "page must not render a syllabus editor: {body}"
        );
    }

    // The stored body renders as-is for both roles.
    sqlx::query("UPDATE courses SET syllabus_body = $1 WHERE id = $2")
        .bind("<p>Read chapters 1-3.</p>")
        .bind(course)
        .execute(&pool)
        .await
        .expect("set syllabus body");

    for (session, csrf) in [(&s_session, &s_csrf), (&t_session, &t_csrf)] {
        let resp = authed_get(&server, session, csrf, &path).await;
        let body = resp.text();
        assert!(
            body.contains("Read chapters 1-3."),
            "stored body missing: {body}"
        );
        assert!(!body.contains("edit_syllabus_form"), "body: {body}");
        assert!(!body.contains("Update Syllabus"), "body: {body}");
    }

    // POSTing this collection path is method-not-allowed: the route is a GET,
    // and the empty 405 body proves no syllabus-save handler answers it.
    let resp = authed_post(
        &server,
        &t_session,
        &t_csrf,
        &path,
        serde_json::json!({ "_csrf": t_csrf, "syllabus_body": "hack" }),
    )
    .await;
    assert_eq!(resp.status_code(), 405, "syllabus save must not be routed");

    let _ = cleanup(&pool, &prefix).await;
}

// -------------------------------------------------------------------------
// Issue #111: calendar journey
// -------------------------------------------------------------------------

/// The calendar renders a 6x7 grid, seeded events land on the right day, and
/// the context filter excludes courses the viewer is not enrolled in. The page
/// is read-only: no event form, and the write verbs Canvas keeps in the
/// per-context `calendar_events` scopes do not answer here.
#[tokio::test]
async fn calendar_grid_filter_is_read_only() {
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
    assert!(
        body.matches("calendar-day").count() >= 42,
        "grid must render 42 cells"
    );
    assert!(
        !body.contains("calendar_event_form"),
        "page must not render an event form: {body}"
    );

    // An event on 2026-11-10, written the way the REST API writes it: Canvas
    // creates events under the per-context `resources :calendar_events`
    // scopes, never by POSTing to /calendar (config/routes.rb:1103).
    seed_calendar_event(
        &pool,
        course,
        "Field Trip",
        "2026-11-10 09:00:00+00",
        Some("Museum"),
    )
    .await;

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
    seed_calendar_event(
        &pool,
        other,
        "Secret Other Event",
        "2026-11-12 09:00:00+00",
        None,
    )
    .await;
    let resp = authed_get(&server, &s_session, &s_csrf, "/calendar?month=2026-11").await;
    let body = resp.text();
    assert!(body.contains("Field Trip"));
    assert!(
        !body.contains("Secret Other Event"),
        "unenrolled context leaked: {body}"
    );

    // Deleting an event is a member DELETE under those per-context scopes too;
    // the old /calendar/{id}/delete form path is unrouted and POST /calendar is
    // method-not-allowed, and the event still renders afterwards.
    let event_id: i64 = sqlx::query_scalar(
        "SELECT id FROM calendar_events WHERE course_id = $1 AND title = 'Field Trip'",
    )
    .bind(course)
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
        404,
        "the delete form path must not be routed: {}",
        resp.text()
    );
    let resp = authed_post(
        &server,
        &t_session,
        &t_csrf,
        "/calendar",
        serde_json::json!({ "_csrf": t_csrf, "title": "late event" }),
    )
    .await;
    assert_eq!(resp.status_code(), 405, "POST /calendar must not be routed");
    let resp = authed_get(&server, &t_session, &t_csrf, "/calendar?month=2026-11").await;
    assert!(resp.text().contains("Field Trip"));

    let _ = cleanup(&pool, &prefix).await;
}

// -------------------------------------------------------------------------
// Precision Workspace: dashboard shell, right-side mount
// -------------------------------------------------------------------------

/// The logged-in dashboard is the shell document with Canvas' dashboard sheets and the
/// right-side aside the To Do panel mounts into.
///
/// The rows themselves are not in this document any more, because Canvas does not put
/// them there either: `app/views/users/user_dashboard.html.erb` only provides
/// `:right_side`, and the panel comes from `GET /api/v1/users/self/todo`. That payload is
/// asserted in `star-dashboard/tests/dashboard_integration.rs`
/// (`todo_lists_grading_and_submitting_items_with_canvas_shape`), the cookie→Bearer fetch
/// of it in `ui_shell_serve.rs::a_cookie_get_reaches_the_bearer_only_handler`, and the
/// rendered rows in `browser-smoke/tests/dashboard-shell-styles.spec.ts`.
///
/// The card tray badges (new announcements, recent feedback) have no REST endpoint to
/// move to: Canvas derives them from the activity stream summary in a GraphQL query
/// (`ui/shared/dashboard-card/graphql/ActivityStream.ts`, mapped at
/// `ui/shared/dashboard-card/util/dashboardUtils.ts:112`), so the old
/// `load_today_items`/`load_course_badges` page queries are gone rather than ported.
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

    let (session, csrf) = login(
        &server,
        &format!("{prefix}-Dash Student@test.local"),
        "Password123!",
    )
    .await;

    let resp = authed_get(&server, &session, &csrf, "/dashboard").await;
    assert_eq!(resp.status_code(), 200, "dashboard: {}", resp.text());
    let body = resp.text();

    // Canvas' own dashboard sheets are in the cascade on first paint.
    for bundle in ["dashboard", "dashboard_card"] {
        assert!(
            body.contains(&format!("data-canvas-bundle=\"{bundle}\"")),
            "Canvas sheet {bundle} missing from /dashboard"
        );
    }

    // The aside Canvas' layout reserves for :right_side is asked for by the body class
    // (app/views/layouts/application.html.erb:94 adds `with-right-side` when the page
    // provides :right_side, and app/stylesheets/base/_layout.scss:78 sizes the aside).
    assert!(
        body.contains("<body class=\"primary-nav-expanded with-right-side\""),
        "body class missing: {body}"
    );

    // The page's data is fetched, not baked in: the built document is the boot shell
    // (ui/build/index.html has no markup between <body> and the start script), so no
    // card node can appear here.
    assert!(
        !body.contains("ic-DashboardCard"),
        "dashboard cards must not be server-rendered"
    );

    let _ = cleanup(&pool, &prefix).await;
}

/// `/quick-jump` is not a Canvas route at all: the dashboard's jump box is the
/// client-side QuickJump React component, which navigates to existing pages
/// instead of posting to a search document. No server page backs it here, and
/// the dashboard form is gone with it.
#[tokio::test]
async fn quick_jump_is_not_a_route() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let hash = star_auth::hash_password("Password123!").expect("hash");
    let teacher = seed_user(&pool, &prefix, "Jump Teacher", "teacher", &hash).await;
    let course = seed_course(&pool, &prefix, "Jump Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    let (session, csrf) = login(
        &server,
        &format!("{prefix}-Jump Teacher@test.local"),
        "Password123!",
    )
    .await;

    let resp = authed_get(&server, &session, &csrf, "/quick-jump?q=Jump").await;
    assert_eq!(resp.status_code(), 404, "quick-jump must not be routed");

    let resp = authed_get(&server, &session, &csrf, "/dashboard").await;
    assert_eq!(resp.status_code(), 200, "dashboard: {}", resp.text());
    assert!(
        !resp.text().contains("quick-jump"),
        "dashboard must not point at an unrouted search page: {}",
        resp.text()
    );

    let _ = cleanup(&pool, &prefix).await;
}
