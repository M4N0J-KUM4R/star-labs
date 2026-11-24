//! Integration tests for the star-ui shell — the 9 acceptance criteria of
//! spec §4. Real Postgres (SKIP when DATABASE_URL unset), real router via
//! axum-test, real argon2 hashes for login.

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
        "ui-tests-{}-{}",
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
    .bind(format!("{prefix}-{name}@test.local"))
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

async fn seed_front_page(pool: &PgPool, course_id: i64, title: &str, body: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO pages (course_id, title, body, url, front_page) \
         VALUES ($1, $2, $3, $4, true) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(body)
    .bind("front-page")
    .fetch_one(pool)
    .await
    .expect("seed front page")
}

async fn seed_module(pool: &PgPool, course_id: i64, name: &str, position: i32) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO modules (course_id, name, position, workflow_state) \
         VALUES ($1, $2, $3, 'active') RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed module")
}

/// Login through the real form flow and return the session cookie value.
///
/// The TestServer does not persist cookies between requests (save_cookies
/// defaults to false), so the CSRF cookie must be attached to the POST
/// explicitly — the double-submit pair is cookie + form field.
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

/// AC1 + AC9(next): logged-out /dashboard redirects to /login?next=/dashboard;
/// /login renders 200 with the form and no current_user_id in ENV.
#[tokio::test]
async fn logged_out_dashboard_redirects_to_login_with_next() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.get("/dashboard").await;
    assert!(resp.status_code() == 303 || resp.status_code() == 302);
    let location = resp
        .headers()
        .get("location")
        .expect("location header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        location.starts_with("/login?next="),
        "unexpected location: {location}"
    );
    assert!(location.contains("%2Fdashboard") || location.contains("/dashboard"));

    let login = server.get("/login").await;
    assert_eq!(login.status_code(), 200);
    let body = login.text();
    assert!(body.contains("login_form"));
    assert!(body.contains("csrf"));
    let env = star_ui::env::parse_env_from_html(&body).expect("ENV parses");
    assert!(
        env.get("current_user_id").is_none(),
        "current_user_id must be absent on /login"
    );

    let _ = cleanup(&pool, &prefix).await;
}

/// AC1 + AC9: GET / redirects to /dashboard.
#[tokio::test]
async fn root_redirects_to_dashboard() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let resp = server.get("/").await;
    assert!(resp.status_code() == 303 || resp.status_code() == 302);
    assert_eq!(resp.headers().get("location").unwrap(), "/dashboard");
}

/// AC2: valid login sets the HttpOnly star_session cookie and 303s to
/// /dashboard; invalid credentials re-render with an error, identically for
/// unknown email and wrong password (no enumeration leak).
#[tokio::test]
async fn login_sets_session_cookie_and_rejects_bad_credentials() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let hash = star_auth::hash_password("Password123!").expect("hash");
    let user = seed_user(&pool, &prefix, "loginuser", "student", &hash).await;
    let email = format!("{prefix}-loginuser@test.local");

    // Valid credentials.
    let page = server.get("/login").await;
    let csrf = page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let resp = server
        .post("/login")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({"email": email, "password": "Password123!", "_csrf": csrf, "next": "/dashboard"}))
        .await;
    assert_eq!(resp.status_code(), 303, "body: {}", resp.text());
    let set_cookie: Vec<&str> = resp
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    let session_hdr = set_cookie
        .iter()
        .find(|h| h.starts_with("star_session="))
        .expect("star_session cookie");
    assert!(session_hdr.contains("HttpOnly"), "header: {session_hdr}");
    assert!(
        session_hdr.contains("SameSite=Lax"),
        "header: {session_hdr}"
    );
    assert_eq!(resp.headers().get("location").unwrap(), "/dashboard");

    // Wrong password.
    let page = server.get("/login").await;
    let csrf2 = page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let wrong = server
        .post("/login")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf2.clone(),
        ))
        .form(&serde_json::json!({"email": email, "password": "WrongPassword!", "_csrf": csrf2}))
        .await;
    assert_eq!(
        wrong.status_code(),
        401,
        "wrong password re-renders the form"
    );
    let wrong_body = wrong.text();
    assert!(wrong_body.contains("login_form"), "form re-rendered");
    assert!(
        wrong_body.contains("Invalid email or password"),
        "error shown"
    );

    // Unknown email: identical response shape.
    let page = server.get("/login").await;
    let csrf3 = page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let unknown = server
        .post("/login")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf3.clone(),
        ))
        .form(&serde_json::json!({"email": format!("{prefix}-nobody@test.local"), "password": "Whatever123!", "_csrf": csrf3}))
        .await;
    assert_eq!(unknown.status_code(), 401);
    assert!(unknown.text().contains("Invalid email or password"));

    let _ = user;
    cleanup(&pool, &prefix).await;
}

/// AC2/CSRF: POST /login with a mismatched _csrf token answers 403.
/// Regression (2026-11-23 403 reports): a request with no `star_csrf` cookie
/// at all must also be rejected, and `GET /login` must be `no-store` so a
/// cached page can never embed a token that no longer matches the cookie.
#[tokio::test]
async fn login_csrf_mismatch_is_403() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let resp = server
        .post("/login")
        .form(&serde_json::json!({"email": "a@b.c", "password": "x", "_csrf": "deadbeef"}))
        .await;
    assert_eq!(resp.status_code(), 403);
}

/// Regression (2026-11-23 403 reports): a POST with no `star_csrf` cookie at
/// all (stale/cleared cookie) is rejected 403 even when a token is present.
#[tokio::test]
async fn login_csrf_missing_cookie_is_403() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let resp = server
        .post("/login")
        .form(&serde_json::json!({"email": "a@b.c", "password": "x", "_csrf": "deadbeef"}))
        .await;
    assert_eq!(resp.status_code(), 403);
}

/// Regression (2026-11-23 403 reports): the login page must not be cacheable,
/// so a cached form can never pair a stale CSRF token with a newer cookie.
#[tokio::test]
async fn login_page_is_no_store() {
    let Some((server, _pool, _prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let resp = server.get("/login").await;
    assert_eq!(resp.status_code(), 200);
    let cache = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        cache, "no-store",
        "login page must send Cache-Control: no-store"
    );
}

/// Regression: the global nav Calendar link must point at the global
/// /calendar page (it previously linked /dashboard).
///
/// It asks for `/calendar` rather than `/dashboard` because `/dashboard` is now the
/// shell boot document and its nav comes from the bundle at runtime
/// (`ui/src/lib/components/AppHeader.svelte:98-103`), so no server-rendered nav exists
/// there to read. `browser-smoke/tests/dashboard-shell-styles` covers the bundle's own
/// link; this covers every Askama page, which still renders
/// `templates/layouts/authenticated.html:19`.
#[tokio::test]
async fn global_nav_calendar_link_points_to_calendar() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "navcaluser", "student", "not-a-real-hash").await;
    let token = star_auth::create_token(user, "student", JWT_SECRET).expect("mint");
    let resp = server
        .get("/calendar?month=2026-11")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            token.clone(),
        ))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    let link = body
        .split("global_nav_calendar_link")
        .nth(1)
        .expect("global nav calendar link present");
    let link = &link[..link.len().min(400)];
    assert!(
        link.contains("href=\"/calendar\""),
        "global_nav_calendar_link must href /calendar, got: {link}"
    );
}

/// AC3 + AC4 + AC8: `/dashboard` is the shell boot document with the 18 phase-1 ENV
/// keys of the right types, Canvas' own dashboard sheets linked in cascade order, and
/// only hashed asset links.
///
/// The card grid and the To Do list are no longer in this document: Canvas renders them
/// from `GET /api/v1/dashboard/dashboard_cards` and `GET /api/v1/users/self/todo`, and
/// those payloads are asserted in `star-dashboard/tests/dashboard_integration.rs`
/// (`dashboard_cards_return_menu_cards_ordered_by_position`,
/// `todo_lists_grading_and_submitting_items_with_canvas_shape`) and their DOM in
/// `browser-smoke/tests/dashboard-shell-styles.spec.ts`.
#[tokio::test]
async fn dashboard_renders_cards_and_env_contract() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "dashuser", "student", "not-a-real-hash").await;

    let token = star_auth::create_token(user, "student", JWT_SECRET).expect("mint");
    let resp = server
        .get("/dashboard")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            token.clone(),
        ))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();

    // ENV contract: exactly the 18 phase-1 keys with the specified types.
    let env = star_ui::env::parse_env_from_html(&body).expect("ENV parses");
    for key in [
        "current_user_id",
        "current_user_roles",
        "current_user_is_student",
        "DOMAIN_ROOT_ACCOUNT_ID",
        "RAILS_ENVIRONMENT",
        "ASSET_HOST",
        "DEEP_LINKING_POST_MESSAGE_ORIGIN",
        "active_context_tab",
        "context_asset_string",
        "PREFERENCES",
        "SETTINGS",
        "use_high_contrast",
        "help_link_name",
        "k12",
        "files_domain",
        "notices",
    ] {
        assert!(env.get(key).is_some(), "phase-1 key {key} missing from ENV");
    }
    assert!(env["current_user_id"].is_i64());
    assert_eq!(env["current_user_id"], serde_json::json!(user));
    assert!(env["current_user_roles"].is_array());
    assert!(env["current_user_is_student"].is_boolean());
    assert!(env["DOMAIN_ROOT_ACCOUNT_ID"].is_i64());
    assert!(env["RAILS_ENVIRONMENT"].is_string());
    assert!(env["DEEP_LINKING_POST_MESSAGE_ORIGIN"].is_string());
    assert!(env["PREFERENCES"].is_object());
    assert!(env["PREFERENCES"]["dashboard_view"].is_string());
    assert_eq!(env["PREFERENCES"]["dashboard_view"], "cards");
    assert!(env["PREFERENCES"]["custom_colors"].is_object());
    assert!(env["PREFERENCES"]["hide_dashcard_color_overlays"].is_boolean());
    assert!(env["SETTINGS"]["collapse_global_nav"].is_boolean());
    assert!(env["use_high_contrast"].is_boolean());
    assert!(env["help_link_name"].is_string());
    assert!(env["k12"].is_boolean());
    assert!(env["files_domain"].is_string());
    assert!(env["notices"].is_array());

    // AC8: the boot document, then Canvas' dashboard sheets on top of the common set.
    assert!(
        body.contains("kit.start(app, element)"),
        "boot script missing"
    );
    assert!(
        body.contains("/_app/immutable/entry/start."),
        "entry link missing"
    );
    for bundle in ["tokens", "common", "dashboard", "dashboard_card"] {
        assert!(
            body.contains(&format!("data-canvas-bundle=\"{bundle}\"")),
            "Canvas sheet {bundle} missing from the cascade"
        );
    }
    // `with-right-side` is what Canvas' layout adds because user_dashboard provides
    // :right_side (app/views/users/user_dashboard.html.erb:84); the To Do panel sits in
    // that aside and `.with-right-side` sizes it (app/stylesheets/base/_layout.scss:78).
    assert!(
        body.contains("<body class=\"primary-nav-expanded with-right-side\""),
        "body class missing: {:?}",
        body.chars().take(300).collect::<String>()
    );

    // Two inline <script> tags and no more: the one `shell::render` injects with this
    // user's `window.ENV`, and SvelteKit's own boot block, which is a bare `<script>`
    // too (`ui/build/index.html`: `__sveltekit_… = { base }` plus `kit.start(…)`).
    // A third would mean per-request data reached the cacheable bundle.
    let script_count = body.matches("<script>").count();
    let env_block_count = body.matches("window.ENV = ").count();
    assert_eq!(
        script_count, 2,
        "the ENV block and the boot block, nothing else"
    );
    assert_eq!(env_block_count, 1);

    // One user's document must never be cached.
    assert!(resp.headers()["cache-control"]
        .to_str()
        .unwrap_or_default()
        .contains("no-store"));

    cleanup(&pool, &prefix).await;
}

/// AC5 + AC6: course home renders tabs in API order, the right context ENV,
/// the front page when one exists, and the module list otherwise.
#[tokio::test]
async fn course_home_tabs_and_content_branches() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "courseuser", "student", "not-a-real-hash").await;
    let wiki_course = seed_course(&pool, &prefix, "Wiki Course", "available").await;
    let modules_course = seed_course(&pool, &prefix, "Modules Course", "available").await;
    seed_enrollment(&pool, user, wiki_course, "student").await;
    seed_enrollment(&pool, user, modules_course, "student").await;
    seed_front_page(
        &pool,
        wiki_course,
        "Welcome",
        "<p>Welcome to the course!</p>",
    )
    .await;
    seed_module(&pool, modules_course, "Module One", 1).await;
    seed_module(&pool, modules_course, "Module Two", 2).await;

    let token = star_auth::create_token(user, "student", JWT_SECRET).expect("mint");
    let cookie = axum_extra::extract::cookie::Cookie::new("star_session", token.clone());

    // Wiki-branch course: front page body rendered, tabs in API order, ENV.
    let resp = server
        .get(&format!("/courses/{wiki_course}"))
        .add_cookie(cookie.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("Welcome to the course!"), "front page body");
    assert!(body.contains("section-tabs"), "left-side tabs present");

    // Tab order matches the tabs API: home, announcements, assignments, ...
    let expected_order = [
        "home",
        "announcements",
        "assignments",
        "discussions",
        "grades",
        "people",
        "pages",
        "files",
        "syllabus",
        "modules",
        "outcomes",
        "quizzes",
    ];
    let mut last = 0usize;
    for tab in expected_order {
        let needle = format!("{tab}-link");
        let pos = body
            .find(&needle)
            .unwrap_or_else(|| panic!("tab {tab} missing"));
        assert!(pos > last, "tab {tab} out of API order");
        last = pos;
    }
    // No Settings tab for a student.
    assert!(
        !body.contains("settings-link"),
        "no settings tab for student"
    );

    let env = star_ui::env::parse_env_from_html(&body).expect("ENV parses");
    assert_eq!(env["context_asset_string"], format!("course_{wiki_course}"));
    assert_eq!(env["active_context_tab"], "home");

    // Modules-branch course: module names rendered.
    let resp = server
        .get(&format!("/courses/{modules_course}"))
        .add_cookie(cookie.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(body.contains("Module One"));
    assert!(body.contains("Module Two"));
    assert!(body.contains("context_module"), "module list markup");

    // Teacher sees the Settings tab last (tabs API parity).
    let teacher = seed_user(&pool, &prefix, "teacher", "teacher", "not-a-real-hash").await;
    seed_enrollment(&pool, teacher, wiki_course, "teacher").await;
    let teacher_token = star_auth::create_token(teacher, "teacher", JWT_SECRET).expect("mint");
    let resp = server
        .get(&format!("/courses/{wiki_course}"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            teacher_token,
        ))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert!(
        resp.text().contains("settings-link"),
        "teacher sees settings tab"
    );

    // Tab landing page still gates + highlights.
    let resp = server
        .get(&format!("/courses/{wiki_course}/assignments"))
        .add_cookie(cookie)
        .await;
    assert_eq!(resp.status_code(), 200);
    let env = star_ui::env::parse_env_from_html(&resp.text()).expect("ENV parses");
    assert_eq!(env["active_context_tab"], "assignments");

    cleanup(&pool, &prefix).await;
}

/// AC7: an unenrolled non-admin user gets no course data (403/404 page), and
/// the page never leaks the course name.
#[tokio::test]
async fn unenrolled_user_gets_no_course_data() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let outsider = seed_user(&pool, &prefix, "outsider", "student", "not-a-real-hash").await;
    let owner = seed_user(&pool, &prefix, "owner", "teacher", "not-a-real-hash").await;
    let secret = format!("{prefix}-SecretCourseName");
    let course = seed_course(&pool, &prefix, &secret, "available").await;
    seed_enrollment(&pool, owner, course, "teacher").await;

    let token = star_auth::create_token(outsider, "student", JWT_SECRET).expect("mint");
    let resp = server
        .get(&format!("/courses/{course}"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            token,
        ))
        .await;
    assert!(
        resp.status_code() == 403 || resp.status_code() == 404,
        "unenrolled non-admin must not see the course"
    );
    assert!(!resp.text().contains(&secret), "course name must not leak");

    cleanup(&pool, &prefix).await;
}

/// AC9: logout stamps last_logged_out server-side (asserted via DB), clears
/// the cookie, redirects to /login, and the old session token is rejected.
#[tokio::test]
async fn logout_invalidates_session_server_side() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let hash = star_auth::hash_password("Password123!").expect("hash");
    let user = seed_user(&pool, &prefix, "logoutuser", "student", &hash).await;
    let email = format!("{prefix}-logoutuser@test.local");

    // Login through the real form.
    let page = server.get("/login").await;
    let csrf = page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let login = server
        .post("/login")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({"email": email, "password": "Password123!", "_csrf": csrf}))
        .await;
    assert_eq!(login.status_code(), 303);
    let session = login
        .cookies()
        .get("star_session")
        .expect("session")
        .value()
        .to_string();
    // The CSRF cookie the browser holds after login is the one issued by
    // GET /login (the POST's Set-Cookie delta only carries star_session).
    let login_csrf = csrf;

    // Dashboard works with the session.
    let dash = server
        .get("/dashboard")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            session.clone(),
        ))
        .await;
    assert_eq!(dash.status_code(), 200);

    // before: last_logged_out is null
    let before: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT last_logged_out FROM users WHERE id = $1")
            .bind(user)
            .fetch_optional(&pool)
            .await
            .expect("query");
    assert!(before.unwrap().0.is_none(), "last_logged_out starts null");

    // Logout (CSRF double-submit).
    let resp = server
        .post("/logout")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            session.clone(),
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            login_csrf.clone(),
        ))
        .form(&serde_json::json!({"_csrf": login_csrf}))
        .await;
    assert_eq!(resp.status_code(), 303, "logout redirects");
    assert_eq!(resp.headers().get("location").unwrap(), "/login");

    // after: last_logged_out is stamped in the DB.
    let after: Option<(Option<chrono::DateTime<chrono::Utc>>,)> =
        sqlx::query_as("SELECT last_logged_out FROM users WHERE id = $1")
            .bind(user)
            .fetch_optional(&pool)
            .await
            .expect("query");
    assert!(
        after.unwrap().0.is_some(),
        "last_logged_out must be stamped by POST /logout"
    );

    // The old session cookie is rejected: /dashboard redirects to /login.
    // The 5-second nudge mirrors star-users' account-mgmt test: issued-at is
    // derived from exp with second granularity, so a login and logout inside
    // the same wall-clock second would otherwise race the comparison.
    sqlx::query("UPDATE users SET last_logged_out = now() + interval '5 seconds' WHERE id = $1")
        .bind(user)
        .execute(&pool)
        .await
        .expect("nudge last_logged_out");
    let dash = server
        .get("/dashboard")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            session,
        ))
        .await;
    assert!(
        dash.status_code() == 303 || dash.status_code() == 302,
        "stale session must redirect to login, got {}",
        dash.status_code()
    );

    cleanup(&pool, &prefix).await;
}

/// AC9: the `next` param round-trips through login.
#[tokio::test]
async fn next_param_round_trips_through_login() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let hash = star_auth::hash_password("Password123!").expect("hash");
    let user = seed_user(&pool, &prefix, "nextuser", "student", &hash).await;
    let email = format!("{prefix}-nextuser@test.local");
    let course = seed_course(&pool, &prefix, "Next Course", "available").await;
    seed_enrollment(&pool, user, course, "student").await;

    // Logged-out hit on a deep link stores the path.
    let resp = server.get(&format!("/courses/{course}")).await;
    let location = resp
        .headers()
        .get("location")
        .expect("location")
        .to_str()
        .unwrap()
        .to_string();
    assert!(location.starts_with("/login?next="));

    // Login with that next value lands on the originally requested path.
    let page = server.get("/login").await;
    let csrf = page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
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
            "password": "Password123!",
            "_csrf": csrf,
            "next": format!("/courses/{course}")
        }))
        .await;
    assert_eq!(resp.status_code(), 303);
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}").as_str(),
        "next must round-trip"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Quiz-taking journey (issue #109) — list, new, show, take, submit, results.
// ---------------------------------------------------------------------------

/// Seed helpers for the quiz journey (mirror star-quizzes' test seeding).
async fn seed_quiz(
    pool: &PgPool,
    course_id: i64,
    title: &str,
    published: bool,
    points: f64,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO quizzes (course_id, title, published, points_possible)          VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(published)
    .bind(points)
    .fetch_one(pool)
    .await
    .expect("seed quiz")
}

async fn seed_question(
    pool: &PgPool,
    quiz_id: i64,
    position: i32,
    question_type: &str,
    points: f64,
    answers: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO quiz_questions              (quiz_id, position, question_name, question_type, question_text, points_possible, answers)          VALUES ($1, $2, $3, $4, 'Body', $5, $6) RETURNING id",
    )
    .bind(quiz_id)
    .bind(position)
    .bind(format!("Q{position}"))
    .bind(question_type)
    .bind(points)
    .bind(answers)
    .fetch_one(pool)
    .await
    .expect("seed question")
}

// ---------------------------------------------------------------------------
// Discussions + inbox journey (issue #72)
// ---------------------------------------------------------------------------

/// Seed a discussion topic; returns its id.
async fn seed_topic(
    pool: &PgPool,
    course_id: i64,
    author_id: i64,
    title: &str,
    message: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO discussion_topics (course_id, title, message, user_id) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(course_id)
    .bind(title)
    .bind(message)
    .bind(author_id)
    .fetch_one(pool)
    .await
    .expect("seed topic")
}

/// Seed a discussion entry (root when parent is None); returns its id.
async fn seed_entry(
    pool: &PgPool,
    topic_id: i64,
    user_id: i64,
    message: &str,
    parent: Option<i64>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO discussion_entries (topic_id, parent_id, user_id, message) \
         VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(topic_id)
    .bind(parent)
    .bind(user_id)
    .bind(message)
    .fetch_one(pool)
    .await
    .expect("seed entry")
}

/// Seed a two-participant conversation with one message; returns its id.
async fn seed_conversation(
    pool: &PgPool,
    author_id: i64,
    recipient_id: i64,
    subject: &str,
    body: &str,
) -> i64 {
    let mut tx = pool.begin().await.expect("begin");
    let id: i64 =
        sqlx::query_scalar("INSERT INTO conversations (subject) VALUES ($1) RETURNING id")
            .bind(subject)
            .fetch_one(&mut *tx)
            .await
            .expect("seed conversation");
    for (uid, read) in [(author_id, true), (recipient_id, false)] {
        sqlx::query(
            "INSERT INTO conversation_participants \
             (conversation_id, user_id, workflow_state, last_message_at, read_at) \
             VALUES ($1, $2, 'active', now(), CASE WHEN $3 THEN now() ELSE NULL END)",
        )
        .bind(id)
        .bind(uid)
        .bind(read)
        .execute(&mut *tx)
        .await
        .expect("seed participant");
    }
    sqlx::query(
        "INSERT INTO conversation_messages (conversation_id, author_id, body) \
         VALUES ($1, $2, $3)",
    )
    .bind(id)
    .bind(author_id)
    .bind(body)
    .execute(&mut *tx)
    .await
    .expect("seed message");
    tx.commit().await.expect("commit");
    id
}

/// Fetch a shell page with the session cookie (CSRF cookie attached too, so
/// follow-up POSTs can double-submit).
async fn get_with_session(
    server: &TestServer,
    path: &str,
    session: &str,
) -> axum_test::TestResponse {
    server
        .get(path)
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            session.to_string(),
        ))
        .await
}

/// The discussions index gates on enrollment, shows the teacher-only
/// "+ Discussion" button, and carries the per-viewer unread marker that
/// clears after the topic is viewed.
#[tokio::test]
async fn discussions_index_teacher_button_and_unread_marker() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "dteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "dstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Disc Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let topic = seed_topic(&pool, course, teacher, "Unread Topic", "<p>Read me</p>").await;
    let _ = seed_entry(&pool, topic, teacher, "first entry", None).await;

    let teacher_token = star_auth::create_token(teacher, "teacher", JWT_SECRET).expect("mint");
    let student_token = star_auth::create_token(student, "student", JWT_SECRET).expect("mint");

    // Teacher: create button present.
    let resp = get_with_session(
        &server,
        &format!("/courses/{course}/discussions"),
        &teacher_token,
    )
    .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("add_discussion"), "teacher sees + Discussion");
    assert!(body.contains("Unread Topic"));

    // Student: no create button, unread marker on the un-viewed topic.
    let resp = get_with_session(
        &server,
        &format!("/courses/{course}/discussions"),
        &student_token,
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(
        !body.contains("add_discussion"),
        "student must not see + Discussion"
    );
    let unread_row = format!("discussion_topic_{topic}\"");
    assert!(body.contains(&unread_row), "topic row present");
    assert!(
        body.contains("unread-mark"),
        "unread marker on the un-viewed topic"
    );

    // Student opens the topic: marker clears in the index afterwards.
    let resp = get_with_session(
        &server,
        &format!("/courses/{course}/discussions/{topic}"),
        &student_token,
    )
    .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = get_with_session(
        &server,
        &format!("/courses/{course}/discussions"),
        &student_token,
    )
    .await;
    assert_eq!(resp.status_code(), 200);
    assert!(
        !resp.text().contains("unread-mark"),
        "marker cleared after viewing"
    );

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Assignments + gradebook journey (issue #71)
// ---------------------------------------------------------------------------

/// Seed helpers for the journey tests. Same sentinel discipline as the
/// course tests: unique run prefix, cleanup at the end.
async fn seed_assignment_group(pool: &PgPool, course_id: i64, name: &str, position: i32) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignment_groups (course_id, name, position) \
         VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(course_id)
    .bind(name)
    .bind(position)
    .fetch_one(pool)
    .await
    .expect("seed assignment group")
}

#[allow(clippy::too_many_arguments)]
async fn seed_assignment(
    pool: &PgPool,
    course_id: i64,
    group_id: Option<i64>,
    title: &str,
    description: Option<&str>,
    points: f64,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO assignments \
         (course_id, assignment_group_id, title, description, points_possible, workflow_state) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(course_id)
    .bind(group_id)
    .bind(title)
    .bind(description)
    .bind(points)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed assignment")
}

async fn seed_submission(
    pool: &PgPool,
    assignment_id: i64,
    user_id: i64,
    body: &str,
    score: Option<f64>,
    workflow_state: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO submissions \
         (assignment_id, user_id, body, submission_type, score, workflow_state) \
         VALUES ($1, $2, $3, 'online_text_entry', $4, $5) RETURNING id",
    )
    .bind(assignment_id)
    .bind(user_id)
    .bind(body)
    .bind(score)
    .bind(workflow_state)
    .fetch_one(pool)
    .await
    .expect("seed submission")
}

/// A session cookie helper: mint the JWT directly (the login flow is covered
/// by the earlier tests) and attach it to requests.
fn session_cookie(user_id: i64, role: &str) -> axum_extra::extract::cookie::Cookie<'static> {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint");
    axum_extra::extract::cookie::Cookie::new("star_session", token)
}

fn csrf_pair() -> (axum_extra::extract::cookie::Cookie<'static>, String) {
    let token = star_ui::session::generate_csrf_token();
    (
        axum_extra::extract::cookie::Cookie::new("star_csrf", token.clone()),
        token,
    )
}

/// One MC (answer ids 7 correct / 8 wrong), one true_false, one essay —
/// the grading fixture. Returns (quiz, mc_question, essay_question).
async fn seed_graded_quiz(pool: &PgPool, course_id: i64) -> (i64, i64, i64) {
    let quiz = seed_quiz(pool, course_id, "Graded Quiz", true, 30.0).await;
    let mc = seed_question(
        pool,
        quiz,
        1,
        "multiple_choice_question",
        10.0,
        r#"[{"id": 7, "answer_text": "Correct", "answer_weight": 100}, {"id": 8, "answer_text": "Wrong", "answer_weight": 0}]"#,
    )
    .await;
    seed_question(
        pool,
        quiz,
        2,
        "true_false_question",
        10.0,
        r#"[{"id": 21, "answer_text": "True", "answer_weight": 100}, {"id": 22, "answer_text": "False", "answer_weight": 0}]"#,
    )
    .await;
    let essay = seed_question(pool, quiz, 3, "essay_question", 10.0, "[]").await;
    (quiz, mc, essay)
}

/// List: teacher sees "+ Quiz"; student sees published only; unenrolled
/// non-admin hits the course gate.
#[tokio::test]
async fn quiz_list_roles_and_filter() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "qteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "qstudent", "student", "not-a-real-hash").await;
    let outsider = seed_user(&pool, &prefix, "qoutsider", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Quiz Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let published = seed_quiz(&pool, course, "Published Quiz", true, 5.0).await;
    let _draft = seed_quiz(&pool, course, "Draft Quiz", false, 5.0).await;

    // Teacher: "+ Quiz" link + both quizzes.
    let resp = server
        .get(&format!("/courses/{course}/quizzes"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("new_quiz_link"), "teacher sees + Quiz");
    assert!(body.contains("Published Quiz"));
    assert!(body.contains("Draft Quiz"));

    // Student: no "+ Quiz", published only.
    let resp = server
        .get(&format!("/courses/{course}/quizzes"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(!body.contains("new_quiz_link"), "no + Quiz for student");
    assert!(body.contains("Published Quiz"));
    assert!(
        !body.contains("Draft Quiz"),
        "unpublished hidden from student"
    );
    assert!(body.contains(&format!("quiz_row_{published}")));

    // Unenrolled non-admin: course gate (403/404, no leak).
    let resp = server
        .get(&format!("/courses/{course}/quizzes"))
        .add_cookie(session_cookie(outsider, "student"))
        .await;
    assert!(
        resp.status_code() == 403 || resp.status_code() == 404,
        "unenrolled non-admin must hit the gate"
    );
    assert!(!resp.text().contains("Published Quiz"));

    cleanup(&pool, &prefix).await;
}

/// Issue #71: teacher sees groups + rows + unpublished styling on the
/// assignments index; a student sees published rows with their submission
/// pill and no "Add Assignment"; an unenrolled non-admin gets no data.
#[tokio::test]
async fn assignments_index_roles_and_grouping() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "ateacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "astudent", "student", "not-a-real-hash").await;
    let outsider = seed_user(&pool, &prefix, "aoutsider", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Assignments Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let group_a = seed_assignment_group(&pool, course, "Essays", 1).await;
    let group_b = seed_assignment_group(&pool, course, "Quizzes", 2).await;
    let essay = seed_assignment(
        &pool,
        course,
        Some(group_a),
        "Essay One",
        Some("<p>Write about <b>topics</b>.</p>"),
        10.0,
        "published",
    )
    .await;
    let _quiz = seed_assignment(
        &pool,
        course,
        Some(group_b),
        "Quiz One",
        None,
        5.0,
        "unpublished",
    )
    .await;
    seed_submission(&pool, essay, student, "my essay text", Some(8.0), "graded").await;

    // Teacher: groups + rows + unpublished styling + Add Assignment.
    let resp = server
        .get(&format!("/courses/{course}/assignments"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("Essays"), "group heading renders");
    assert!(body.contains("Quizzes"), "second group renders");
    assert!(body.contains("Essay One"), "assignment row renders");
    assert!(body.contains("ig-row-unpublished"), "unpublished style");
    assert!(body.contains("id=\"add_assignment\""), "teacher add button");
    assert!(body.contains("15"), "total points 10+5 rendered");

    // Group order: Essays bucket before Quizzes bucket. The plain text
    // "Quizzes" also appears in the left-side course tabs, so compare the
    // group buckets by their data-group-id attributes instead.
    let pos_a = body
        .find(&format!("data-group-id=\"{group_a}\""))
        .expect("essays bucket");
    let pos_b = body
        .find(&format!("data-group-id=\"{group_b}\""))
        .expect("quizzes bucket");
    assert!(pos_a < pos_b, "groups ordered by position");

    // Student: published rows + own submission pill, no unpublished, no add.
    let resp = server
        .get(&format!("/courses/{course}/assignments"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(body.contains("Essay One"));
    assert!(
        !body.contains("Quiz One"),
        "unpublished hidden from student"
    );
    assert!(
        !body.contains("add_assignment"),
        "no add button for student"
    );
    assert!(
        body.contains("pill-graded") && body.contains("Graded"),
        "graded pill for own submission"
    );

    // Unenrolled non-admin: no data (404-before-403 gate).
    let resp = server
        .get(&format!("/courses/{course}/assignments"))
        .add_cookie(session_cookie(outsider, "student"))
        .await;
    assert!(
        resp.status_code() == 403 || resp.status_code() == 404,
        "unenrolled user must not see the assignments index"
    );
    assert!(!resp.text().contains("Essay One"), "no data leak");

    cleanup(&pool, &prefix).await;
}

/// Canvas' `quizzes/new` is POST-only (`post "quizzes/new"`, canvas
/// config/routes.rb:446, under `resources :quizzes, except: :new` at :447), so
/// GET is a method mismatch, teacher POST creates (redirects to the editor,
/// quizzes_controller.rb:352-355), a title-less POST gets Canvas' "Unnamed Quiz"
/// (app/models/quizzes/quiz.rb:161), and a student POST is 403.
#[tokio::test]
async fn quiz_new_teacher_creates_student_forbidden() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "nteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "nstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "New Quiz Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // No GET on this path, for anyone: Canvas never generated the form page.
    let page = server
        .get(&format!("/courses/{course}/quizzes/new"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(page.status_code(), 405, "GET /quizzes/new must not exist");

    // The "+ Quiz" control lives on the index as a POST form.
    let index = server
        .get(&format!("/courses/{course}/quizzes"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(index.status_code(), 200);
    let index_body = index.text();
    assert!(
        index_body.contains("new_quiz_form"),
        "index must carry the + Quiz POST form: {index_body}"
    );

    // Teacher POST creates and 303s to the editor.
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/quizzes/new"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({
            "_csrf": csrf,
            "title": "Created By Form",
            "description": "<p>Instructions</p>",
            "points_possible": "12.5",
            "available_from": "",
            "available_until": "",
            "published": "true"
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "body: {}", resp.text());
    let location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    // Canvas' `new` redirects to the editor url (quizzes_controller.rb:352-355),
    // so the location must be `.../quizzes/:id/edit`.
    assert!(
        location.ends_with("/edit"),
        "create must hand off to the editor: {location}"
    );
    let new_id: i64 = location
        .trim_end_matches("/edit")
        .rsplit('/')
        .next()
        .unwrap()
        .parse()
        .expect("quiz id in location");
    assert!(location.starts_with(&format!("/courses/{course}/quizzes/")));

    let stored: Option<(String, bool, f64)> = sqlx::query_as(
        "SELECT title, published, points_possible::float8 FROM quizzes WHERE id = $1",
    )
    .bind(new_id)
    .fetch_optional(&pool)
    .await
    .expect("query");
    let (title, published, points) = stored.expect("quiz created");
    assert_eq!(title, "Created By Form");
    assert!(published);
    assert_eq!(points, 12.5);

    // The handoff target — the editor — renders for the teacher.
    let edit = server
        .get(&location)
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(edit.status_code(), 200, "body: {}", edit.text());
    let edit_body = edit.text();
    assert!(
        edit_body.contains(&format!("action=\"/courses/{course}/quizzes/{new_id}\"")),
        "the editor form must post to the Canvas member path: {edit_body}"
    );
    assert!(
        edit_body.contains("value=\"Created By Form\""),
        "the saved title must be prefilled: {edit_body}"
    );

    // Show page renders for the teacher, with the Edit affordance.
    let show = server
        .get(&format!("/courses/{course}/quizzes/{new_id}"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(show.status_code(), 200);
    assert!(show.text().contains("Created By Form"));
    assert!(
        show.text()
            .contains(&format!("/courses/{course}/quizzes/{new_id}/edit")),
        "teacher show page must link to the editor"
    );

    // A POST with no title is Canvas' index-button request: it creates a
    // draft, it does not 422.
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/quizzes/new"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(
        resp.status_code(),
        303,
        "blank title must create: {}",
        resp.text()
    );
    let bare_location = resp
        .headers()
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let bare_id: i64 = bare_location
        .trim_end_matches("/edit")
        .rsplit('/')
        .next()
        .unwrap()
        .parse()
        .expect("quiz id in location");
    let bare_title: String = sqlx::query_scalar("SELECT title FROM quizzes WHERE id = $1")
        .bind(bare_id)
        .fetch_one(&pool)
        .await
        .expect("unnamed quiz");
    assert_eq!(bare_title, "Unnamed Quiz");

    // Student POST is 403.
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/quizzes/new"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf, "title": "Nope", "published": ""}))
        .await;
    assert_eq!(resp.status_code(), 403);
}

/// The quiz editor journey: `GET .../quizzes/:id/edit` (Canvas' `quizzes#edit`,
/// quizzes_controller.rb:359, which `render :new` at :431) is teacher-only;
/// saving it reaches Canvas' `PATCH` member route (`resources :quizzes`,
/// config/routes.rb:447) through the `_method` shim, and `quizzes#update`
/// flashes "Quiz successfully updated" while redirecting to the show url
/// (:612-613).
#[tokio::test]
async fn quiz_editor_save_uses_member_patch() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "eteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "estudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Editor Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let quiz = seed_quiz(&pool, course, "Midterm", true, 10.0).await;

    // Students have no `:update` right on the quiz -> 403, like Canvas.
    let denied = server
        .get(&format!("/courses/{course}/quizzes/{quiz}/edit"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(denied.status_code(), 403, "body: {}", denied.text());

    // Teacher GET renders the form and posts to the member path, never /edit.
    let page = server
        .get(&format!("/courses/{course}/quizzes/{quiz}/edit"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(page.status_code(), 200, "body: {}", page.text());
    let body = page.text();
    assert!(
        body.contains(&format!("action=\"/courses/{course}/quizzes/{quiz}\"")),
        "form action must be the Canvas member path: {body}"
    );
    assert!(
        body.contains("name=\"_method\" value=\"patch\""),
        "hidden _method field: {body}"
    );
    assert!(
        !body.contains(&format!("/courses/{course}/quizzes/{quiz}/edit\"")),
        "the form must not post to /edit: {body}"
    );
    assert!(
        body.contains("value=\"Midterm\"") && body.contains("value=\"10\""),
        "stored title and points are prefilled: {body}"
    );
    let csrf = page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();

    // POST + `_method=patch` on the member path saves and PRGs to show.
    let resp = server
        .post(&format!("/courses/{course}/quizzes/{quiz}"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "_csrf": csrf.clone(),
            "_method": "patch",
            "title": "Midterm v2",
            "description": "<p>Bring a pen</p>",
            "points_possible": "25",
            "available_from": "2027-03-01T09:30",
            "available_until": "2027-03-05T17:00",
            "published": "true"
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "body: {}", resp.text());
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/quizzes/{quiz}?saved=1").as_str(),
        "update redirects to the show url (quizzes_controller.rb:613)"
    );

    let row: (String, String, f64, bool, Option<String>, Option<String>) = sqlx::query_as(
        r#"SELECT title, COALESCE(description, ''), points_possible::float8, published,
                  to_char(available_from AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI'),
                  to_char(available_until AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI')
             FROM quizzes WHERE id = $1"#,
    )
    .bind(quiz)
    .fetch_one(&pool)
    .await
    .expect("read quiz");
    assert_eq!(row.0, "Midterm v2");
    assert_eq!(row.1, "<p>Bring a pen</p>");
    assert_eq!(row.2, 25.0);
    assert!(row.3);
    // `datetime-local` values have no offset, and the port reads them as UTC.
    assert_eq!(row.4.as_deref(), Some("2027-03-01T09:30"));
    assert_eq!(row.5.as_deref(), Some("2027-03-05T17:00"));

    // The redirect target renders Canvas' flash text.
    let show = server
        .get(&format!("/courses/{course}/quizzes/{quiz}?saved=1"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(show.status_code(), 200);
    assert!(
        show.text().contains("Quiz successfully updated"),
        "PRG notice must render: {}",
        show.text()
    );

    // A bare POST on the member path is not a Canvas route: 405, row untouched.
    let resp = server
        .post(&format!("/courses/{course}/quizzes/{quiz}"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "_csrf": csrf.clone(),
            "title": "Bare Post",
            "points_possible": "1"
        }))
        .await;
    assert_eq!(resp.status_code(), 405, "member POST must not be routable");
    let title: String = sqlx::query_scalar("SELECT title FROM quizzes WHERE id = $1")
        .bind(quiz)
        .fetch_one(&pool)
        .await
        .expect("quiz still there");
    assert_eq!(title, "Midterm v2", "405 must not write");

    // `?title=` overrides the form value only — Canvas assigns the param to the
    // unsaved quiz (:369) after pushing the stored title onto the crumb (:367).
    let page = server
        .get(&format!(
            "/courses/{course}/quizzes/{quiz}/edit?title=Draft%20Handoff"
        ))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(page.status_code(), 200);
    let body = page.text();
    assert!(
        body.contains("value=\"Draft Handoff\""),
        "?title= must fill the form: {body}"
    );
    assert!(
        body.contains("Midterm v2"),
        "the crumb keeps the stored title: {body}"
    );

    cleanup(&pool, &prefix).await;
}

/// Take: student start creates an in-progress submission; the take page
/// carries the exact DOM contract field names; a second start resumes the
/// SAME submission (not 409).
#[tokio::test]
async fn quiz_take_starts_and_resumes() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "tteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "tstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Take Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let (quiz, mc, _essay) = seed_graded_quiz(&pool, course).await;

    // Show page offers Take Quiz.
    let page = server
        .get(&format!("/courses/{course}/quizzes/{quiz}"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(page.status_code(), 200);
    let body = page.text();
    assert!(body.contains("take_quiz_link"), "Take Quiz button");
    assert!(!body.contains("Score for this attempt"));

    // POST take starts the attempt and redirects to the take page.
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/quizzes/{quiz}/take"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 303, "start: {}", resp.text());
    assert_eq!(
        resp.headers().get("location").unwrap().to_str().unwrap(),
        format!("/courses/{course}/quizzes/{quiz}/take")
    );

    let submission_id: i64 =
        sqlx::query_scalar("SELECT id FROM quiz_submissions WHERE quiz_id = $1 AND user_id = $2")
            .bind(quiz)
            .bind(student)
            .fetch_one(&pool)
            .await
            .expect("submission row");

    // GET take renders ALL questions in one form with the DOM contract names.
    let take = server
        .get(&format!("/courses/{course}/quizzes/{quiz}/take"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(take.status_code(), 200, "take: {}", take.text());
    let body = take.text();
    assert!(body.contains("submit_quiz_form"));
    assert!(body.contains("submit_quiz_button"));
    // EXACT radio field name for the MC question.
    assert!(
        body.contains(&format!("name=\"question_{mc}\"")),
        "radio name contract missing: {body}"
    );
    assert!(body.contains(&format!("id=\"question_{mc}\"")));
    assert!(body.contains("question_text"));

    // Second start while in-progress: same submission (resume, not 409).
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/quizzes/{quiz}/take"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(
        resp.status_code(),
        303,
        "resume must not 409: {}",
        resp.text()
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM quiz_submissions WHERE quiz_id = $1 AND user_id = $2",
    )
    .bind(quiz)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(count, 1, "resume must reuse the one submission");

    // Teacher gets 403 on take (students-only, Canvas parity).
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/quizzes/{quiz}/take"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 403);

    let _ = submission_id;
    cleanup(&pool, &prefix).await;
}

/// Submit: one correct MC + one wrong true_false + one essay -> score is the
/// MC points only, workflow_state pending_review, results show the
/// per-question state, and the teacher can view the student's results.
#[tokio::test]
async fn quiz_submit_grades_and_results() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "steacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "sstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Submit Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let (quiz, mc, _essay) = seed_graded_quiz(&pool, course).await;

    // Start + load the take page.
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/quizzes/{quiz}/take"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 303);
    let submission_id: i64 =
        sqlx::query_scalar("SELECT id FROM quiz_submissions WHERE quiz_id = $1 AND user_id = $2")
            .bind(quiz)
            .bind(student)
            .fetch_one(&pool)
            .await
            .expect("submission");

    // Submit: MC correct (answer 7), true_false wrong (22 instead of 21),
    // essay text. The field names are dynamic (`question_<id>`), so the body
    // is built by hand and POSTed raw.
    let questions: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, question_type FROM quiz_questions WHERE quiz_id = $1 ORDER BY position",
    )
    .bind(quiz)
    .fetch_all(&pool)
    .await
    .expect("questions");
    let tf_id = questions
        .iter()
        .find(|(_, qtype)| qtype == "true_false_question")
        .map(|(id, _)| *id)
        .expect("true_false question");
    let essay_id = questions
        .iter()
        .find(|(_, qtype)| qtype == "essay_question")
        .map(|(id, _)| *id)
        .expect("essay question");

    let (csrf_cookie, csrf) = csrf_pair();
    let body_pairs = [
        ("_csrf".to_string(), csrf.clone()),
        (format!("question_{mc}"), "7".to_string()),
        (format!("question_{tf_id}"), "22".to_string()),
        (format!("question_{essay_id}"), "My essay".to_string()),
    ];
    let body: String = body_pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencoding_encode(v)))
        .collect::<Vec<_>>()
        .join("&");

    let resp = server
        .post(&format!(
            "/courses/{course}/quizzes/{quiz}/submissions/{submission_id}/submit"
        ))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .add_header("content-type", "application/x-www-form-urlencoded")
        .bytes(body.into())
        .await;
    assert_eq!(resp.status_code(), 303, "submit: {}", resp.text());
    assert_eq!(
        resp.headers().get("location").unwrap().to_str().unwrap(),
        format!("/courses/{course}/quizzes/{quiz}/submissions/{submission_id}")
    );

    // DB: pending_review + partial score = MC points (10) only.
    let row: (String, Option<f64>) =
        sqlx::query_as("SELECT workflow_state, score::float8 FROM quiz_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(row.0, "pending_review", "essay forces pending_review");
    assert_eq!(row.1, Some(10.0), "auto-graded partial score = MC points");

    // Results page (student): per-question state.
    let results = server
        .get(&format!(
            "/courses/{course}/quizzes/{quiz}/submissions/{submission_id}"
        ))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(results.status_code(), 200);
    let body = results.text();
    assert!(body.contains("Score for this attempt: 10 / 30"));
    assert!(body.contains("pending_review_note"));
    assert!(body.contains(&format!("id=\"question_{mc}\"")));
    assert!(body.contains("Correct"));
    assert!(body.contains("Incorrect"));
    assert!(body.contains("Needs review"));
    assert!(body.contains("My essay"));
    assert!(body.contains("attempt 0"));

    // Teacher views the student's results (submissions table + results page).
    let show = server
        .get(&format!("/courses/{course}/quizzes/{quiz}"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(show.status_code(), 200);
    let show_body = show.text();
    assert!(show_body.contains("quiz_submissions_table"));
    assert!(show_body.contains(&format!(
        "/courses/{course}/quizzes/{quiz}/submissions/{submission_id}"
    )));
    let teacher_view = server
        .get(&format!(
            "/courses/{course}/quizzes/{quiz}/submissions/{submission_id}"
        ))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(teacher_view.status_code(), 200);
    assert!(teacher_view
        .text()
        .contains("Score for this attempt: 10 / 30"));

    // Second submit on a finished attempt redirects to results (no regrade).
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!(
            "/courses/{course}/quizzes/{quiz}/submissions/{submission_id}/submit"
        ))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 303);

    cleanup(&pool, &prefix).await;
}

/// Issue #71: assignment show renders the description safe (tags stripped),
/// the due date and points; a student text-entry submit creates the
/// submission row and flips the state pill.
#[tokio::test]
async fn assignment_show_and_student_submit() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "bteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "bstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Submit Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let group = seed_assignment_group(&pool, course, "Homework", 1).await;
    let hw = seed_assignment(
        &pool,
        course,
        Some(group),
        "Homework One",
        Some("<p>Read <script>alert('x')</script><strong>chapter 2</strong>.</p>"),
        10.0,
        "published",
    )
    .await;

    // Student show: description safe (tags stripped, no script tag), submit
    // form present, "Not Submitted" pill.
    let resp = server
        .get(&format!("/courses/{course}/assignments/{hw}"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("Homework One"));
    assert!(
        !body.contains("<script>alert"),
        "script tag must be stripped from the description"
    );
    assert!(body.contains("chapter 2"), "text content preserved");
    assert!(body.contains("id=\"submit_assignment\""), "submit form");
    assert!(
        body.contains("pill-unsubmitted") && body.contains("Not Submitted"),
        "not-submitted pill before submitting"
    );
    assert!(body.contains("10"), "points render");

    // CSRF cookie comes from the show page; POST the text entry.
    let csrf = resp
        .cookies()
        .get("star_csrf")
        .expect("csrf cookie issued")
        .value()
        .to_string();
    let resp = server
        .post(&format!("/courses/{course}/assignments/{hw}/submit"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "body": "My homework answers",
            "_csrf": csrf
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "submit redirects: {}", resp.text());
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/assignments/{hw}?submitted=1").as_str()
    );

    // The submission row exists in the DB (same upsert shape as the API).
    let (count, state, body_text): (i64, String, String) = sqlx::query_as(
        "SELECT COUNT(*), MAX(workflow_state), MAX(body) FROM submissions \
         WHERE assignment_id = $1 AND user_id = $2",
    )
    .bind(hw)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("query submission");
    assert_eq!(count, 1, "exactly one submission row");
    assert_eq!(state, "submitted");
    assert_eq!(body_text, "My homework answers");

    // Show again: the pill now reads Submitted.
    let resp = server
        .get(&format!("/courses/{course}/assignments/{hw}"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert!(
        resp.text().contains("pill-submitted")
            && resp.text().contains("Submitted")
            && !resp.text().contains("Not Submitted"),
        "state pill flipped after submit"
    );

    // Teacher show: read-only submissions table with the student row.
    let resp = server
        .get(&format!("/courses/{course}/assignments/{hw}"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(body.contains("id=\"submissions_table\""), "teacher table");
    assert!(body.contains("bstudent"), "student name in table");

    cleanup(&pool, &prefix).await;
}

/// Percent-encode a form value the way url::form_urlencoded expects
/// (application/x-www-form-urlencoded: space as '+', reserved escaped).
fn urlencoding_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// CSRF mismatch on submit answers 403 and leaves the attempt untouched.
#[tokio::test]
async fn quiz_submit_csrf_mismatch_is_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "cstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "CSRF Course", "available").await;
    seed_enrollment(&pool, student, course, "student").await;
    let (quiz, _mc, _essay) = seed_graded_quiz(&pool, course).await;

    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/quizzes/{quiz}/take"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 303);
    let submission_id: i64 =
        sqlx::query_scalar("SELECT id FROM quiz_submissions WHERE quiz_id = $1 AND user_id = $2")
            .bind(quiz)
            .bind(student)
            .fetch_one(&pool)
            .await
            .expect("submission");

    // Mismatched token: cookie says one thing, form another.
    let resp = server
        .post(&format!(
            "/courses/{course}/quizzes/{quiz}/submissions/{submission_id}/submit"
        ))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            "real-token",
        ))
        .form(&serde_json::json!({"_csrf": "forged-token"}))
        .await;
    assert_eq!(resp.status_code(), 403, "CSRF mismatch must 403");

    let row: (String, Option<f64>) =
        sqlx::query_as("SELECT workflow_state, score::float8 FROM quiz_submissions WHERE id = $1")
            .bind(submission_id)
            .fetch_one(&pool)
            .await
            .expect("row");
    assert_eq!(row.0, "untaken", "attempt untouched");
    assert!(row.1.is_none());

    cleanup(&pool, &prefix).await;
}

/// Issue #71: teacher POST /assignments creates the row (it appears in the
/// list); a student POST is 403. `/assignments/new` is Canvas' form PAGE, so
/// a POST there must stay a method mismatch (`resources :assignments`, canvas
/// config/routes.rb:270 — create is the collection POST).
#[tokio::test]
async fn new_assignment_create_and_gates() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "cteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "cstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Create Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let group = seed_assignment_group(&pool, course, "Projects", 1).await;

    // Student cannot even load the form.
    let resp = server
        .get(&format!("/courses/{course}/assignments/new"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student GET new -> 403");

    // Teacher loads the form (with the group option).
    let resp = server
        .get(&format!("/courses/{course}/assignments/new"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("Add Assignment"));
    assert!(body.contains("Projects"), "group option listed");
    let csrf = resp
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();

    // POST on the form page is not a Canvas route: 405, no row.
    let resp = server
        .post(&format!("/courses/{course}/assignments/new"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "name": "Should Not Exist",
            "points_possible": "1",
            "_csrf": csrf.clone(),
            "published": "published"
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        405,
        "POST /assignments/new must not be routable"
    );

    // Student POST -> 403 (before any insert).
    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "name": "Hacked",
            "points_possible": "1",
            "_csrf": csrf.clone(),
            "published": "published"
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "student POST new -> 403");

    // Teacher POST creates the assignment.
    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "name": "Project One",
            "description": "Build something",
            "points_possible": "25",
            "due_date": "2026-12-01",
            "due_time": "17:00",
            "assignment_group_id": group.to_string(),
            "_csrf": csrf.clone(),
            "published": "published"
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "create redirects: {}", resp.text());
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/assignments").as_str()
    );

    // The row exists with the right values.
    let row: Option<(String, f64, Option<i64>, String)> = sqlx::query_as(
        "SELECT title, points_possible::float8, assignment_group_id, workflow_state \
         FROM assignments WHERE course_id = $1 AND title = 'Project One'",
    )
    .bind(course)
    .fetch_optional(&pool)
    .await
    .expect("query assignment");
    let Some((title, points, group_id, state)) = row else {
        panic!("created assignment row missing");
    };
    assert_eq!(title, "Project One");
    assert_eq!(points, 25.0);
    assert_eq!(group_id, Some(group));
    assert_eq!(state, "published");

    // It appears in the teacher's index.
    let resp = server
        .get(&format!("/courses/{course}/assignments"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert!(resp.text().contains("Project One"), "row appears in list");

    // Validation: empty name re-renders the form with 422, no row created.
    // The CSRF cookie and the form token must match (the handler checks the
    // token pair before validating the fields).
    let resp = server
        .post(&format!("/courses/{course}/assignments"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "name": "  ",
            "_csrf": csrf,
            "published": "published"
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "empty name -> 422");

    cleanup(&pool, &prefix).await;
}

/// Canvas' edit form posts `POST /courses/:id/assignments/:id` with a hidden
/// `_method=patch` field and Rack::MethodOverride turns that into the PATCH
/// the route table carries (`resources :assignments`, config/routes.rb:270).
/// `star_ui::method_override` is that shim, so the verbs on this journey are
/// Canvas' verbs: PATCH/PUT on the member path, GET on `/edit`, and nothing
/// routable as POST on either.
#[tokio::test]
async fn assignment_edit_form_uses_the_method_override() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "eteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "estudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Override Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let group = seed_assignment_group(&pool, course, "Weekly", 1).await;
    let hw = seed_assignment(
        &pool,
        course,
        Some(group),
        "Old Title",
        None,
        5.0,
        "published",
    )
    .await;

    // The form page: Canvas' /edit page, GET only, and it points at the
    // member path with the override field — not at /edit, and not at PATCH
    // (which no HTML form can submit).
    let resp = server
        .get(&format!("/courses/{course}/assignments/{hw}/edit"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(
        body.contains(&format!("action=\"/courses/{course}/assignments/{hw}\"")),
        "form action must be the Canvas member path, got: {body}"
    );
    assert!(
        body.contains("name=\"_method\" value=\"patch\""),
        "hidden _method field: {body}"
    );
    assert!(
        !body.contains(&format!("/courses/{course}/assignments/{hw}/edit")),
        "the form must not post to /edit"
    );
    let csrf = resp
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();

    // POST + `_method=patch` on the member path updates the row.
    let resp = server
        .post(&format!("/courses/{course}/assignments/{hw}"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "_method": "patch",
            "name": "New Title",
            "points_possible": "8",
            "assignment_group_id": group.to_string(),
            "_csrf": csrf.clone(),
            "published": "published"
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        303,
        "override PATCH must update: {}",
        resp.text()
    );
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/assignments/{hw}").as_str()
    );
    let (title, points): (String, f64) =
        sqlx::query_as("SELECT title, points_possible::float8 FROM assignments WHERE id = $1")
            .bind(hw)
            .fetch_one(&pool)
            .await
            .expect("read assignment");
    assert_eq!(title, "New Title");
    assert_eq!(points, 8.0);

    // Without the override field the same request is a bare POST, and Canvas
    // has no POST on the member path: 405, row untouched.
    let resp = server
        .post(&format!("/courses/{course}/assignments/{hw}"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "name": "Bare Post",
            "points_possible": "1",
            "_csrf": csrf.clone()
        }))
        .await;
    assert_eq!(resp.status_code(), 405, "member POST must not be routable");
    let title: String = sqlx::query_scalar("SELECT title FROM assignments WHERE id = $1")
        .bind(hw)
        .fetch_one(&pool)
        .await
        .expect("read title");
    assert_eq!(title, "New Title", "bare POST must not write");

    // The invented verb on the form PAGE is gone.
    let resp = server
        .post(&format!("/courses/{course}/assignments/{hw}/edit"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "name": "Via /edit",
            "points_possible": "1",
            "_csrf": csrf.clone()
        }))
        .await;
    assert_eq!(
        resp.status_code(),
        405,
        "POST /assignments/:id/edit must not be routable"
    );

    // A verb outside PATCH/PUT/DELETE is not honoured (see the module docs),
    // so this stays the POST the router has no route for.
    let resp = server
        .post(&format!("/courses/{course}/assignments/{hw}"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "_method": "get",
            "name": "Tunneled",
            "points_possible": "1",
            "_csrf": csrf.clone()
        }))
        .await;
    assert_eq!(resp.status_code(), 405, "_method=get must not be honoured");

    // The gate runs on the overridden verb too: a student's PATCH-shaped
    // form is 403 before any update.
    let resp = server
        .post(&format!("/courses/{course}/assignments/{hw}"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "_method": "patch",
            "name": "Hacked",
            "points_possible": "1",
            "_csrf": csrf
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "student override PATCH -> 403");
    let title: String = sqlx::query_scalar("SELECT title FROM assignments WHERE id = $1")
        .bind(hw)
        .fetch_one(&pool)
        .await
        .expect("read title");
    assert_eq!(title, "New Title", "student write must not land");

    cleanup(&pool, &prefix).await;
}

/// Issue #71: the grades page shows the student's own scores + the course
/// total; a teacher can view a specific student via ?student_id=; a student
/// cannot view another student.
#[tokio::test]
async fn grades_page_scores_and_totals() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "dteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "dstudent", "student", "not-a-real-hash").await;
    let other = seed_user(&pool, &prefix, "dother", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Grades Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, other, course, "student").await;
    let group = seed_assignment_group(&pool, course, "Exams", 1).await;
    let exam = seed_assignment(
        &pool,
        course,
        Some(group),
        "Exam One",
        None,
        20.0,
        "published",
    )
    .await;
    let extra = seed_assignment(
        &pool,
        course,
        Some(group),
        "Exam Two",
        None,
        30.0,
        "published",
    )
    .await;
    // student: 16/20 graded on exam one, submitted (ungraded) on exam two.
    seed_submission(&pool, exam, student, "answers", Some(16.0), "graded").await;
    seed_submission(&pool, extra, student, "answers two", None, "submitted").await;

    // Student's own grades: score + total 16/20 = 80%.
    let resp = server
        .get(&format!("/courses/{course}/grades"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("id=\"grades_summary\""), "summary table");
    assert!(body.contains("Exam One"));
    assert!(body.contains("Exam Two"));
    assert!(body.contains("16 / 20"), "graded score renders");
    assert!(body.contains("80%"), "percent 16/20 = 80%");
    assert!(body.contains("id=\"final_grade\""), "total block");
    assert!(
        body.contains("pill-graded") && body.contains("pill-submitted"),
        "state pills per row"
    );

    // Teacher with ?student_id= sees the same summary for the student.
    let resp = server
        .get(&format!("/courses/{course}/grades?student_id={student}"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "teacher view: {}", resp.text());
    assert!(resp.text().contains("16 / 20"));
    assert!(resp.text().contains("80%"));

    // A student cannot view another student's grades.
    let resp = server
        .get(&format!("/courses/{course}/grades?student_id={other}"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "cross-student view -> 403");

    cleanup(&pool, &prefix).await;
}

/// Topic show renders the body HTML, entries with author names; teacher
/// creates a topic via POST (PRG), student posts an entry and a reply.
#[tokio::test]
async fn discussion_show_create_topic_entry_and_reply() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "steacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "sstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Show Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let teacher_token = star_auth::create_token(teacher, "teacher", JWT_SECRET).expect("mint");
    let student_token = star_auth::create_token(student, "student", JWT_SECRET).expect("mint");

    // Teacher creates a topic through the form (PRG -> topic page).
    let csrf_page = get_with_session(
        &server,
        &format!("/courses/{course}/discussions/new"),
        &teacher_token,
    )
    .await;
    assert_eq!(csrf_page.status_code(), 200, "teacher gets the form");
    let csrf = csrf_page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();

    let resp = server
        .post(&format!("/courses/{course}/discussions"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            teacher_token.clone(),
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "_csrf": csrf,
            "title": "Form Created Topic",
            "message": "<p>Hello <strong>world</strong></p>"
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "PRG redirect: {}", resp.text());
    let location = resp
        .headers()
        .get("location")
        .expect("location")
        .to_str()
        .unwrap()
        .to_string();
    let topic_id: i64 = location
        .rsplit('/')
        .next()
        .and_then(|s| s.parse().ok())
        .expect("topic id in redirect");

    // The row exists in the DB.
    let count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM discussion_topics WHERE id = $1 AND title = 'Form Created Topic'",
    )
    .bind(topic_id)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(count.0, 1, "teacher-created topic row appears");

    // Topic show: body HTML renders safe, title present.
    let show = get_with_session(&server, &location, &student_token).await;
    assert_eq!(show.status_code(), 200);
    let body = show.text();
    assert!(body.contains("Form Created Topic"));
    assert!(body.contains("<strong>world</strong>"), "body HTML is safe");

    // Student posts a root entry through the form.
    let csrf_page = get_with_session(&server, &location, &student_token).await;
    let csrf = csrf_page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let resp = server
        .post(&format!("/courses/{course}/discussions/{topic_id}/entries"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            student_token.clone(),
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({"_csrf": csrf, "message": "Student entry one"}))
        .await;
    assert_eq!(resp.status_code(), 303, "entry PRG: {}", resp.text());

    // The entry appears on the show page with the author name.
    let show = get_with_session(&server, &location, &student_token).await;
    let body = show.text();
    assert!(body.contains("Student entry one"), "entry rendered");
    assert!(body.contains("sstudent"), "entry author name rendered");

    // Reply to that entry (parent_id set on the inline reply form).
    let entry_id: (i64,) = sqlx::query_as(
        "SELECT id FROM discussion_entries WHERE topic_id = $1 AND message = 'Student entry one'",
    )
    .bind(topic_id)
    .fetch_one(&pool)
    .await
    .expect("entry id");
    let csrf_page = get_with_session(&server, &location, &student_token).await;
    let csrf = csrf_page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let resp = server
        .post(&format!("/courses/{course}/discussions/{topic_id}/entries"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            student_token.clone(),
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "_csrf": csrf,
            "message": "Inline reply to entry",
            "parent_id": entry_id.0
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "reply PRG: {}", resp.text());

    let show = get_with_session(&server, &location, &student_token).await;
    let body = show.text();
    assert!(body.contains("Inline reply to entry"), "reply rendered");
    assert!(
        body.contains(&format!("entry_{}", entry_id.0)),
        "reply references its parent entry"
    );

    // The reply row carries parent_id in the DB.
    let replies: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM discussion_entries \
         WHERE topic_id = $1 AND message = 'Inline reply to entry' AND parent_id = $2",
    )
    .bind(topic_id)
    .bind(entry_id.0)
    .fetch_one(&pool)
    .await
    .expect("reply count");
    assert_eq!(replies.0, 1, "reply stored with parent_id");

    // A student cannot create topics: POST answers 403.
    let csrf_page = get_with_session(
        &server,
        &format!("/courses/{course}/discussions/new"),
        &student_token,
    )
    .await;
    assert_eq!(csrf_page.status_code(), 403, "student blocked from form");
    // The 403 page never sets a CSRF cookie, and the teacher gate rejects
    // before CSRF validation matters — any well-formed token exercises it.
    let csrf = "0123456789abcdef0123456789abcdef".to_string();
    let resp = server
        .post(&format!("/courses/{course}/discussions"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            student_token.clone(),
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({
            "_csrf": csrf,
            "title": "Student Topic",
            "message": "should not exist"
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "student cannot create topics");

    cleanup(&pool, &prefix).await;
}

/// The tab alias `/courses/{id}/discussion_topics` serves the same index
/// page as `/courses/{id}/discussions` (the tabs API emits html_url with
/// the discussion_topics suffix).
#[tokio::test]
async fn discussion_topics_alias_serves_the_index() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "ateacher", "teacher", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Alias Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let topic = seed_topic(&pool, course, teacher, "Alias Topic", "body").await;

    let token = star_auth::create_token(teacher, "teacher", JWT_SECRET).expect("mint");

    let a = get_with_session(&server, &format!("/courses/{course}/discussions"), &token).await;
    let b = get_with_session(
        &server,
        &format!("/courses/{course}/discussion_topics"),
        &token,
    )
    .await;
    assert_eq!(a.status_code(), 200);
    assert_eq!(b.status_code(), 200);
    let a_body = a.text();
    let b_body = b.text();
    // The CSRF token differs per request; mask each page's own token before
    // comparing, the same normalization the auth tests apply.
    let mask_csrf = |body: &str| {
        let token = body
            .split("name=\"_csrf\" value=\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_default();
        body.replace(&format!("value=\"{token}\""), "value=\"CSRF\"")
    };
    assert_eq!(
        mask_csrf(&a_body),
        mask_csrf(&b_body),
        "alias serves the identical page"
    );
    assert!(a_body.contains("Alias Topic"));
    let _ = topic;

    cleanup(&pool, &prefix).await;
}

/// The inbox: unread styling on a seeded conversation, open marks it read,
/// reply appends a message, mark-all-read clears the unread count badge,
/// compose creates a conversation the recipient can see.
#[tokio::test]
async fn inbox_unread_open_reply_mark_all_and_compose() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "iteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "istudent", "student", "not-a-real-hash").await;
    let conversation = seed_conversation(
        &pool,
        teacher,
        student,
        "Question about homework",
        "What is due tomorrow?",
    )
    .await;

    let student_token = star_auth::create_token(student, "student", JWT_SECRET).expect("mint");
    let teacher_token = star_auth::create_token(teacher, "teacher", JWT_SECRET).expect("mint");

    // Inbox shows the unread-styled conversation + global nav badge.
    let resp = get_with_session(&server, "/conversations", &student_token).await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("Question about homework"));
    assert!(
        body.contains(&format!("conversation_{conversation}\"")),
        "conversation row present"
    );
    assert!(
        body.contains("conversation-item unread"),
        "unread style on the un-opened conversation"
    );
    assert!(
        body.contains("global_nav_conversations_unread_badge"),
        "global nav unread badge shows for a nonzero count"
    );

    // Opening the thread marks it read (participant names + body render).
    let thread = get_with_session(
        &server,
        &format!("/conversations/{conversation}"),
        &student_token,
    )
    .await;
    assert_eq!(thread.status_code(), 200, "body: {}", thread.text());
    let body = thread.text();
    assert!(body.contains("What is due tomorrow?"), "message body");
    assert!(body.contains("iteacher"), "participant names render");

    // The badge is gone on the next inbox fetch.
    let resp = get_with_session(&server, "/conversations", &student_token).await;
    let body = resp.text();
    assert!(
        !body.contains("global_nav_conversations_unread_badge"),
        "badge cleared after opening the conversation"
    );

    // Reply through the form; the message appears on the thread.
    let csrf_page = get_with_session(
        &server,
        &format!("/conversations/{conversation}"),
        &student_token,
    )
    .await;
    let csrf = csrf_page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let resp = server
        .post(&format!("/conversations/{conversation}/add_message"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            student_token.clone(),
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({"_csrf": csrf, "body": "The reading is due."}))
        .await;
    assert_eq!(resp.status_code(), 303, "reply PRG: {}", resp.text());
    let thread = get_with_session(
        &server,
        &format!("/conversations/{conversation}"),
        &student_token,
    )
    .await;
    assert!(thread.text().contains("The reading is due."));

    // Teacher (recipient of the reply) sees the conversation unread again
    // (add_message resets every non-author's read_at).
    let resp = get_with_session(&server, "/conversations", &teacher_token).await;
    let body = resp.text();
    assert!(
        body.contains("global_nav_conversations_unread_badge"),
        "reply makes the thread unread for the other participant"
    );

    // Teacher replies, then marks everything read; the badge clears.
    let csrf_page = get_with_session(
        &server,
        &format!("/conversations/{conversation}"),
        &teacher_token,
    )
    .await;
    let csrf = csrf_page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let resp = server
        .post(&format!("/conversations/{conversation}/add_message"))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            teacher_token.clone(),
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({"_csrf": csrf, "body": "Thanks!"}))
        .await;
    assert_eq!(resp.status_code(), 303);

    let csrf_page = get_with_session(&server, "/conversations", &teacher_token).await;
    let csrf = csrf_page
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let resp = server
        .post("/conversations/mark_all_as_read")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            teacher_token.clone(),
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 303, "mark-all-read PRG");

    let resp = get_with_session(&server, "/conversations", &teacher_token).await;
    let body = resp.text();
    assert!(
        !body.contains("global_nav_conversations_unread_badge"),
        "mark-all-read clears the unread badge"
    );

    // Student composes a NEW conversation to the teacher via the form.
    let compose = get_with_session(&server, "/conversations/compose", &student_token).await;
    assert_eq!(compose.status_code(), 200, "body: {}", compose.text());
    let body = compose.text();
    assert!(body.contains("compose_form"));
    assert!(
        body.contains(&format!("value=\"{teacher}\"")),
        "recipient option"
    );
    let csrf = compose
        .cookies()
        .get("star_csrf")
        .expect("csrf")
        .value()
        .to_string();
    let resp = server
        .post("/conversations")
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_session",
            student_token.clone(),
        ))
        .add_cookie(axum_extra::extract::cookie::Cookie::new(
            "star_csrf",
            csrf.clone(),
        ))
        // Vec<String> forms decode repeated keys, so send recipients as
        // urlencoded pairs rather than a JSON array (serde_urlencoded cannot
        // serialize arrays). `.bytes()` sets the body without touching the
        // content type (unlike `.text()`).
        .add_header("content-type", "application/x-www-form-urlencoded")
        .bytes(
            format!(
                "_csrf={csrf}&recipients={teacher}&subject=New+compose+subject&body=Composed+through+the+form."
            )
            .into(),
        )
        .await;
    assert_eq!(resp.status_code(), 303, "compose PRG: {}", resp.text());
    let location = resp
        .headers()
        .get("location")
        .expect("location")
        .to_str()
        .unwrap()
        .to_string();

    // The recipient (teacher) sees it in their inbox, unread.
    let resp = get_with_session(&server, "/conversations", &teacher_token).await;
    let body = resp.text();
    assert!(
        body.contains("New compose subject"),
        "composed conversation visible to the recipient"
    );

    // The PRG target renders the new thread for the composer.
    let thread = get_with_session(&server, &location, &student_token).await;
    assert_eq!(thread.status_code(), 200);
    assert!(thread.text().contains("Composed through the form."));

    // A non-participant gets 404 on the private thread.
    let outsider = seed_user(&pool, &prefix, "ioutsider", "student", "not-a-real-hash").await;
    let outsider_token = star_auth::create_token(outsider, "student", JWT_SECRET).expect("mint");
    let resp = get_with_session(&server, &location, &outsider_token).await;
    assert_eq!(resp.status_code(), 404, "non-participant gets 404");

    cleanup(&pool, &prefix).await;
}

/// Unenrolled non-admin users get no discussion data: the shell routes gate
/// through load_course_for_viewer, so the course name never leaks.
#[tokio::test]
async fn unenrolled_user_gets_no_discussion_data() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "gteacher", "teacher", "not-a-real-hash").await;
    let outsider = seed_user(&pool, &prefix, "goutsider", "student", "not-a-real-hash").await;
    let secret = format!("{prefix}-SecretDiscussionCourse");
    let course = seed_course(&pool, &prefix, &secret, "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    let topic = seed_topic(&pool, course, teacher, "Secret Topic", "secret body").await;

    let outsider_token = star_auth::create_token(outsider, "student", JWT_SECRET).expect("mint");

    for path in [
        format!("/courses/{course}/discussions"),
        format!("/courses/{course}/discussion_topics"),
        format!("/courses/{course}/discussions/{topic}"),
        format!("/courses/{course}/discussions/new"),
    ] {
        let resp = get_with_session(&server, &path, &outsider_token).await;
        assert!(
            resp.status_code() == 403 || resp.status_code() == 404,
            "unenrolled non-admin must not see {path}"
        );
        assert!(
            !resp.text().contains(&secret),
            "course name must not leak on {path}"
        );
        assert!(
            !resp.text().contains("Secret Topic"),
            "topic title must not leak on {path}"
        );
    }

    cleanup(&pool, &prefix).await;
}

/// Issue #71: the teacher gradebook grid renders one row per enrollment and
/// one column per assignment with correct cells; the CSV export serves
/// text/csv with a header and data row; students get 403.
#[tokio::test]
async fn gradebook_grid_and_csv() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "eteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "estudent", "student", "not-a-real-hash").await;
    let student2 = seed_user(&pool, &prefix, "estudent2", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Grid Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, student2, course, "student").await;
    let group = seed_assignment_group(&pool, course, "Labs", 1).await;
    let lab1 = seed_assignment(
        &pool,
        course,
        Some(group),
        "Lab One",
        None,
        10.0,
        "published",
    )
    .await;
    let lab2 = seed_assignment(
        &pool,
        course,
        Some(group),
        "Lab Two",
        None,
        10.0,
        "published",
    )
    .await;
    // student: 9/10 graded on lab1, nothing on lab2.
    seed_submission(&pool, lab1, student, "lab one text", Some(9.0), "graded").await;
    // student2: submitted lab2, ungraded.
    seed_submission(&pool, lab2, student2, "lab two text", None, "submitted").await;

    // Teacher grid.
    let resp = server
        .get(&format!("/courses/{course}/gradebook"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("id=\"gradebook_grid\""), "grid table");
    assert!(
        body.contains("Lab One") && body.contains("Lab Two"),
        "columns"
    );
    // One row per enrollment (3 incl. teacher).
    let row_count = body.matches("gradebook-row").count();
    assert_eq!(row_count, 3, "one row per enrollment, got {row_count}");
    // Cells: student has 9 on lab1; student2 has a submitted pill on lab2.
    assert!(body.contains("gradebook-score\">9<"), "score cell");
    assert!(
        body.contains("pill-submitted\">submitted<"),
        "submitted state cell"
    );

    // Student -> 403 (grid is teacher-only).
    let resp = server
        .get(&format!("/courses/{course}/gradebook"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student gradebook -> 403");

    // CSV: content-type + header + a data row.
    let resp = server
        .get(&format!("/courses/{course}/gradebook.csv"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/csv"),
        "content-type: {content_type}"
    );
    let csv = resp.text();
    assert!(
        csv.starts_with("Student,Lab One,Lab Two,Total"),
        "csv: {csv}"
    );
    let lines: Vec<&str> = csv.lines().collect();
    assert!(lines.len() >= 4, "header + 3 enrollment rows");
    // The student's row carries the 9 score.
    let student_row = lines
        .iter()
        .find(|l| l.contains("estudent,") && !l.contains("estudent2"))
        .expect("student row in csv");
    assert!(student_row.contains("9"), "score in csv: {student_row}");

    // Student CSV -> 403.
    let resp = server
        .get(&format!("/courses/{course}/gradebook.csv"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student csv -> 403");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Course settings + people journey (issue #73)
// ---------------------------------------------------------------------------

/// Settings: teacher GET renders the form; student GET is 403.
#[tokio::test]
async fn settings_teacher_opens_student_403() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "setteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "setstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Settings Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let resp = server
        .get(&format!("/courses/{course}/settings"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("course_settings_form"), "settings form");
    assert!(body.contains("Course is Published"), "publish state shown");

    let resp = server
        .get(&format!("/courses/{course}/settings"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student settings GET -> 403");

    cleanup(&pool, &prefix).await;
}

/// Settings save round-trips name + syllabus + workflow_state into the DB
/// (PRG redirect with the success notice); student POST is 403; a blank
/// name re-renders 422.
#[tokio::test]
async fn settings_save_round_trip_and_gates() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "saveteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "savestudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Save Course", "unpublished").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let (csrf_cookie, csrf) = csrf_pair();

    // Student POST -> 403 before any write.
    let resp = server
        .post(&format!("/courses/{course}/settings"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie.clone())
        .form(&serde_json::json!({
            "name": "Hacked Course", "_csrf": csrf.clone(), "published": "published"
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "student settings POST -> 403");

    // Teacher saves: name, course_code, syllabus_body, published.
    let resp = server
        .post(&format!("/courses/{course}/settings"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie.clone())
        .form(&serde_json::json!({
            "name": "Renamed Course",
            "course_code": "RC-101",
            "description": "A description",
            "syllabus_body": "Read the textbook",
            "_csrf": csrf.clone(),
            "published": "published"
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "save PRG: {}", resp.text());
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/settings?saved=1").as_str()
    );

    let row: (String, String, Option<String>, String) = sqlx::query_as(
        "SELECT name, course_code, syllabus_body, workflow_state FROM courses WHERE id = $1",
    )
    .bind(course)
    .fetch_one(&pool)
    .await
    .expect("course row");
    assert_eq!(row.0, "Renamed Course");
    assert_eq!(row.1, "RC-101");
    assert_eq!(row.2.as_deref(), Some("Read the textbook"));
    assert_eq!(row.3, "available", "publish checkbox persisted");

    // Unpublish round trip (checkbox absent).
    let resp = server
        .post(&format!("/courses/{course}/settings"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie.clone())
        .form(
            &serde_json::json!({"name": "Renamed Course", "course_code": "RC-101", "_csrf": csrf}),
        )
        .await;
    assert_eq!(resp.status_code(), 303);
    let state: String = sqlx::query_scalar("SELECT workflow_state FROM courses WHERE id = $1")
        .bind(course)
        .fetch_one(&pool)
        .await
        .expect("state");
    assert_eq!(state, "unpublished", "unpublish persisted");

    // Blank name -> 422 re-render with the error.
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/settings"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"name": "   ", "_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 422, "blank name -> 422");
    assert!(resp.text().contains("Name is required."));

    cleanup(&pool, &prefix).await;
}

/// People page lists the seeded enrollments with role + state; teacher sees
/// Add People, student does not.
#[tokio::test]
async fn people_page_lists_roster() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "pteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "pstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "People Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let resp = server
        .get(&format!("/courses/{course}/users"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("roster_table"), "roster table present");
    assert!(body.contains("pstudent"), "student name listed");
    assert!(body.contains("Teacher"), "role shown");
    assert!(body.contains("Active"), "state shown");
    assert!(body.contains("enrollment-active"), "state styling class");
    assert!(
        !body.contains("add_people_link"),
        "student must not see Add People"
    );

    let resp = server
        .get(&format!("/courses/{course}/users"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert!(
        resp.text().contains("add_people_link"),
        "teacher sees Add People"
    );

    cleanup(&pool, &prefix).await;
}

/// Add People: teacher creates an enrollment (assert DB) + PRG; student
/// blocked on GET and POST; unknown login re-renders 422.
#[tokio::test]
async fn add_people_creates_enrollment_and_gates() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "atteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "atstudent", "student", "not-a-real-hash").await;
    let outsider = seed_user(&pool, &prefix, "atoutsider", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Add Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    // Student GET -> 403.
    let resp = server
        .get(&format!("/courses/{course}/users/add"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student GET add -> 403");

    // Teacher loads the form.
    let resp = server
        .get(&format!("/courses/{course}/users/add"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let (csrf_cookie, csrf) = csrf_pair();

    // Student POST -> 403 (no row created).
    let resp = server
        .post(&format!("/courses/{course}/users/add"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie.clone())
        .form(&serde_json::json!({
            "login": format!("{prefix}-atoutsider@test.local"),
            "role": "student",
            "_csrf": csrf.clone()
        }))
        .await;
    assert_eq!(resp.status_code(), 403, "student POST add -> 403");

    // Teacher adds the outsider as a student.
    let resp = server
        .post(&format!("/courses/{course}/users/add"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie.clone())
        .form(&serde_json::json!({
            "login": format!("{prefix}-atoutsider@test.local"),
            "role": "student",
            "_csrf": csrf.clone()
        }))
        .await;
    assert_eq!(resp.status_code(), 303, "add PRG: {}", resp.text());
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/users?added=1").as_str()
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM enrollments WHERE course_id = $1 AND user_id = $2 AND role = 'student'",
    )
    .bind(course)
    .bind(outsider)
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(count, 1, "enrollment created in DB");

    // The roster now lists the outsider.
    let resp = server
        .get(&format!("/courses/{course}/users"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert!(resp.text().contains("atoutsider"), "roster row added");

    // Duplicate -> 422 with the conflict message.
    let resp = server
        .post(&format!("/courses/{course}/users/add"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie.clone())
        .form(&serde_json::json!({
            "login": format!("{prefix}-atoutsider@test.local"),
            "role": "student",
            "_csrf": csrf.clone()
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "duplicate -> 422");
    assert!(resp.text().contains("already enrolled"));

    // Unknown login -> 422.
    let resp = server
        .post(&format!("/courses/{course}/users/add"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({
            "login": format!("{prefix}-nobody@test.local"),
            "role": "student",
            "_csrf": csrf
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "unknown login -> 422");

    cleanup(&pool, &prefix).await;
}

/// Conclude flips the enrollment to inactive; reactivate flips it back; a
/// student POST on either is 403.
#[tokio::test]
async fn conclude_and_reactivate_toggle_state() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "cnteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "cnstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Conclude Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;

    let (csrf_cookie, csrf) = csrf_pair();

    // Student cannot conclude.
    let resp = server
        .post(&format!("/courses/{course}/users/{student}/conclude"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie.clone())
        .form(&serde_json::json!({"_csrf": csrf.clone()}))
        .await;
    assert_eq!(resp.status_code(), 403, "student conclude -> 403");

    // Teacher concludes the student.
    let resp = server
        .post(&format!("/courses/{course}/users/{student}/conclude"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie.clone())
        .form(&serde_json::json!({"_csrf": csrf.clone()}))
        .await;
    assert_eq!(resp.status_code(), 303, "conclude PRG: {}", resp.text());
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/users?concluded=1").as_str()
    );

    let state: String = sqlx::query_scalar(
        "SELECT workflow_state FROM enrollments WHERE course_id = $1 AND user_id = $2",
    )
    .bind(course)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("state");
    assert_eq!(state, "inactive", "concluded -> inactive");

    // The roster row now shows Inactive + the reactivate action.
    let resp = server
        .get(&format!("/courses/{course}/users"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    let body = resp.text();
    assert!(body.contains("Inactive"), "Inactive label rendered");
    assert!(body.contains("reactivate"), "reactivate action offered");

    // Teacher reactivates.
    let resp = server
        .post(&format!("/courses/{course}/users/{student}/reactivate"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie.clone())
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 303, "reactivate PRG");
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("/courses/{course}/users?reactivated=1").as_str()
    );

    let state: String = sqlx::query_scalar(
        "SELECT workflow_state FROM enrollments WHERE course_id = $1 AND user_id = $2",
    )
    .bind(course)
    .bind(student)
    .fetch_one(&pool)
    .await
    .expect("state");
    assert_eq!(state, "active", "reactivated -> active");

    // Student cannot reactivate either.
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/users/{student}/reactivate"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 403, "student reactivate -> 403");

    cleanup(&pool, &prefix).await;
}

/// An unenrolled non-admin gets no settings/people data at all (course gate:
/// 404 before 403 — the shell cannot bypass API authorization).
#[tokio::test]
async fn unenrolled_user_gets_no_settings_or_people_data() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let outsider = seed_user(&pool, &prefix, "soutsider", "student", "not-a-real-hash").await;
    let teacher = seed_user(&pool, &prefix, "steacher", "teacher", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Gate Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;

    for path in [
        format!("/courses/{course}/settings"),
        format!("/courses/{course}/users"),
        format!("/courses/{course}/users/add"),
    ] {
        let resp = server
            .get(&path)
            .add_cookie(session_cookie(outsider, "student"))
            .await;
        // The course gate hides the course entirely (404); when the course
        // is visible to the viewer, the teacher gate 403s — both prove no
        // settings/people data leaks (same convention as the discussions test).
        assert!(
            resp.status_code() == 403 || resp.status_code() == 404,
            "unenrolled GET {path} -> 403/404, got {}",
            resp.status_code()
        );
        assert!(!resp.text().contains("Gate Course"), "no course data leaks");
    }

    // Mutations for a nonexistent course also 404 (before the teacher gate).
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post("/courses/999999/settings")
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"name": "X", "_csrf": csrf}))
        .await;
    assert_eq!(resp.status_code(), 404, "missing course -> 404");

    cleanup(&pool, &prefix).await;
}
// ---------------------------------------------------------------------------
// SpeedGrader grading workflow (issue #108)
// ---------------------------------------------------------------------------

/// Seed a rubric (2 criteria x 2 ratings) associated with an assignment —
/// the grading fixture. Returns (rubric_id, criterion ids).
async fn seed_rubric(pool: &PgPool, course_id: i64, assignment_id: i64) -> (i64, Vec<i64>) {
    let rubric: i64 = sqlx::query_scalar(
        "INSERT INTO rubrics (course_id, title, points_possible) \
         VALUES ($1, 'Essay Rubric', 10) RETURNING id",
    )
    .bind(course_id)
    .fetch_one(pool)
    .await
    .expect("seed rubric");

    let mut criterion_ids = Vec::new();
    for (position, (description, points)) in
        [("Thesis", 5.0), ("Evidence", 5.0)].into_iter().enumerate()
    {
        let criterion: i64 = sqlx::query_scalar(
            "INSERT INTO rubric_criteria (rubric_id, description, points, position) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(rubric)
        .bind(description)
        .bind(points)
        .bind(position as i32)
        .fetch_one(pool)
        .await
        .expect("seed criterion");
        criterion_ids.push(criterion);

        for (rating_pos, (rating_desc, rating_points)) in
            [("Full", points), ("Partial", points / 2.0)]
                .into_iter()
                .enumerate()
        {
            sqlx::query(
                "INSERT INTO rubric_ratings (criterion_id, description, points, position) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(criterion)
            .bind(rating_desc)
            .bind(rating_points)
            .bind(rating_pos as i32)
            .execute(pool)
            .await
            .expect("seed rating");
        }
    }

    // Associate the rubric with the assignment (Canvas association_type).
    sqlx::query(
        "INSERT INTO rubric_associations \
         (rubric_id, association_id, association_type, context_id, context_type, purpose) \
         VALUES ($1, $2, 'Assignment', $3, 'Course', 'grading')",
    )
    .bind(rubric)
    .bind(assignment_id)
    .bind(course_id)
    .execute(pool)
    .await
    .expect("seed rubric association");

    (rubric, criterion_ids)
}

/// The teacher opens the SpeedGrader queue: student list ordered by name,
/// the selected student's submission + current score, the grading form, and
/// the rubric panel when the assignment has one.
#[tokio::test]
async fn speedgrader_queue_and_rubric_panel() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "gteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "gstudent", "student", "not-a-real-hash").await;
    let student2 = seed_user(&pool, &prefix, "gstudent2", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Grading Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, student2, course, "student").await;
    let group = seed_assignment_group(&pool, course, "Essays", 1).await;
    let essay = seed_assignment(
        &pool,
        course,
        Some(group),
        "Grading Essay",
        None,
        10.0,
        "published",
    )
    .await;
    seed_submission(&pool, essay, student, "my essay text", None, "submitted").await;
    let (_, criterion_ids) = seed_rubric(&pool, course, essay).await;

    // Teacher queue (default assignment = first, default student = first by
    // name: gstudent sorts before gstudent2).
    let resp = server
        .get(&format!("/courses/{course}/gradebook/speedgrader"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body = resp.text();
    assert!(body.contains("id=\"speedgrader\""), "speedgrader page");
    assert!(
        body.contains("id=\"speedgrader_grade_form\""),
        "grading form"
    );
    assert!(body.contains("my essay text"), "submission body shown");
    assert!(
        body.contains("Grading Essay"),
        "assignment title shown: {body}"
    );
    // Student nav rows: one per enrollment (3 incl. teacher), ordered.
    let row_count = body.matches("speedgrader-student-row").count();
    assert_eq!(row_count, 3, "one row per enrollment, got {row_count}");
    let pos1 = body.find(">gstudent<").expect("first student in the nav");
    let pos2 = body.find(">gstudent2<").expect("second student in the nav");
    assert!(pos1 < pos2, "students ordered by name");
    // Selected student pill + meta.
    assert!(body.contains("pill-submitted\">Submitted<"), "state pill");
    assert!(body.contains("Attempt"), "attempt info");
    // Next/prev are plain links.
    assert!(body.contains("id=\"speedgrader_next\""), "next link");
    assert!(body.contains("id=\"speedgrader_prev\""), "prev link");
    // Rubric panel renders: criteria + rating selects.
    assert!(body.contains("id=\"speedgrader_rubric\""), "rubric panel");
    assert!(body.contains("Thesis"), "criterion 1");
    assert!(body.contains("Evidence"), "criterion 2");
    assert!(
        body.contains(&format!("name=\"criterion_{}\"", criterion_ids[0])),
        "pick field"
    );

    cleanup(&pool, &prefix).await;
}

/// Permission gates: students 403, unenrolled users the course gate 404/403,
/// on both the queue and the POST.
#[tokio::test]
async fn speedgrader_permission_gates() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "sgateacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "sgatestudent", "student", "not-a-real-hash").await;
    let outsider = seed_user(
        &pool,
        &prefix,
        "sgateoutsider",
        "student",
        "not-a-real-hash",
    )
    .await;
    let course = seed_course(&pool, &prefix, "Gate Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let essay = seed_assignment(&pool, course, None, "Gate Essay", None, 10.0, "published").await;

    // Student -> 403 on the queue.
    let resp = server
        .get(&format!("/courses/{course}/gradebook/speedgrader"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403, "student queue -> 403");

    // Student -> 403 on the POST (course gate passes, teacher check fails).
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/gradebook/speedgrader/grade"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .add_header("content-type", "application/x-www-form-urlencoded")
        .bytes(format!("_csrf={csrf}&assignment_id={essay}&student_id={student}&score=5").into())
        .await;
    assert_eq!(resp.status_code(), 403, "student POST -> 403");

    // Unenrolled non-admin -> 404/403 on the queue (course gate).
    let resp = server
        .get(&format!("/courses/{course}/gradebook/speedgrader"))
        .add_cookie(session_cookie(outsider, "student"))
        .await;
    assert!(
        resp.status_code() == 403 || resp.status_code() == 404,
        "unenrolled non-admin must not see the grading queue"
    );

    cleanup(&pool, &prefix).await;
}

/// The grading round-trip: POST score + comment -> DB assert + PRG target
/// re-renders both; the rubric picks persist as a rubric assessment.
#[tokio::test]
async fn speedgrader_grade_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "rteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "rstudent", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "RoundTrip Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let essay = seed_assignment(
        &pool,
        course,
        None,
        "RoundTrip Essay",
        None,
        10.0,
        "published",
    )
    .await;
    let submission =
        seed_submission(&pool, essay, student, "round trip text", None, "submitted").await;
    let (rubric_id, criterion_ids) = seed_rubric(&pool, course, essay).await;
    let rating_ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM rubric_ratings WHERE criterion_id = $1 ORDER BY position ASC",
    )
    .bind(criterion_ids[0])
    .fetch_all(&pool)
    .await
    .expect("rating ids");

    // POST the ONE grading form: score + comment + the first criterion pick.
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/gradebook/speedgrader/grade"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie)
        .add_header("content-type", "application/x-www-form-urlencoded")
        .bytes(
            format!(
                "_csrf={csrf}&assignment_id={essay}&student_id={student}\
                 &score=8.5&comment=Nice+work+on+the+thesis\
                 &criterion_{}={}",
                criterion_ids[0], rating_ids[0]
            )
            .into(),
        )
        .await;
    assert_eq!(resp.status_code(), 303, "grade PRG: {}", resp.text());
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert_eq!(
        location,
        format!(
            "/courses/{course}/gradebook/speedgrader?assignment_id={essay}&student_id={student}&graded=1"
        )
    );

    // DB: score + graded state on the submission row.
    let row: (Option<f64>, String, Option<i64>) = sqlx::query_as(
        "SELECT score::float8, workflow_state, grader_id FROM submissions WHERE id = $1",
    )
    .bind(submission)
    .fetch_one(&pool)
    .await
    .expect("submission row");
    assert_eq!(row.0, Some(8.5), "score persisted");
    assert_eq!(row.1, "graded", "workflow_state graded");
    assert_eq!(row.2, Some(teacher), "grader stamped");

    // DB: the comment row (author = teacher).
    let comment: (i64, String) = sqlx::query_as(
        "SELECT author_id, comment FROM submission_comments WHERE submission_id = $1",
    )
    .bind(submission)
    .fetch_one(&pool)
    .await
    .expect("comment row");
    assert_eq!(comment.0, teacher, "comment author");
    assert_eq!(comment.1, "Nice work on the thesis");

    // DB: the rubric assessment (grading type, linked to the submission,
    // data carries the picked rating).
    let assessment: (f64, String) = sqlx::query_as(
        "SELECT score::float8, data::text FROM rubric_assessments \
         WHERE rubric_id = $1 AND submission_id = $2 AND assessment_type = 'grading'",
    )
    .bind(rubric_id)
    .bind(submission)
    .fetch_one(&pool)
    .await
    .expect("rubric assessment row");
    assert_eq!(assessment.0, 5.0, "rubric total = picked rating points");
    // jsonb::text serializes with a space after each colon.
    assert!(
        assessment
            .1
            .contains(&format!("\"rating_id\": {}", rating_ids[0])),
        "data carries the pick: {}",
        assessment.1
    );

    // The PRG target re-renders score + comment + rubric prefill + notice.
    let resp = server
        .get(location)
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(body.contains("Grade saved."), "success notice");
    assert!(body.contains("8.5 / 10"), "current score shown: {body}");
    assert!(body.contains("Nice work on the thesis"), "comment visible");
    assert!(body.contains("pill-graded\">Graded<"), "graded pill");
    // The picked rating is preselected in the rubric select.
    let needle = format!("value=\"{}\" selected", rating_ids[0]);
    assert!(body.contains(&needle), "rubric pick preselected: {needle}");

    // CSRF mismatch on the POST -> 403, nothing persisted.
    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM submission_comments WHERE submission_id = $1")
            .bind(submission)
            .fetch_one(&pool)
            .await
            .expect("count");
    let (csrf_cookie2, _) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/gradebook/speedgrader/grade"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie2)
        .add_header("content-type", "application/x-www-form-urlencoded")
        .bytes(format!("_csrf=deadbeef&assignment_id={essay}&student_id={student}&score=1").into())
        .await;
    assert_eq!(resp.status_code(), 403, "csrf mismatch -> 403");
    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM submission_comments WHERE submission_id = $1")
            .bind(submission)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(before, after, "no comment persisted on csrf failure");

    // A blank score re-renders the form with the validation message (422),
    // mirroring the REST "score is required" validation.
    let (csrf_cookie3, csrf3) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/gradebook/speedgrader/grade"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .add_cookie(csrf_cookie3)
        .add_header("content-type", "application/x-www-form-urlencoded")
        .bytes(format!("_csrf={csrf3}&assignment_id={essay}&student_id={student}&score=").into())
        .await;
    assert_eq!(resp.status_code(), 422, "blank score -> 422");
    assert!(
        resp.text().contains("Score is required"),
        "validation message"
    );

    cleanup(&pool, &prefix).await;
}

/// Next/prev navigation round-trips the selection: the links preserve the
/// assignment and move the student.
#[tokio::test]
async fn speedgrader_next_prev_navigation() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let teacher = seed_user(&pool, &prefix, "nteacher", "teacher", "not-a-real-hash").await;
    let student = seed_user(&pool, &prefix, "nstudent", "student", "not-a-real-hash").await;
    let student2 = seed_user(&pool, &prefix, "nstudent2", "student", "not-a-real-hash").await;
    let course = seed_course(&pool, &prefix, "Nav Course", "available").await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    seed_enrollment(&pool, student2, course, "student").await;
    let essay = seed_assignment(&pool, course, None, "Nav Essay", None, 10.0, "published").await;

    // Open student2 directly; "Previous" goes to student (name order).
    let resp = server
        .get(&format!(
            "/courses/{course}/gradebook/speedgrader?assignment_id={essay}&student_id={student2}"
        ))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    // Askama HTML-escapes the href's ampersands (& -> &#38;); a browser
    // resolves them back to & when following the link, so compare escaped.
    let href = body
        .split("id=\"speedgrader_prev\"")
        .nth(1)
        .and_then(|rest| rest.split('"').find(|s| s.starts_with("/courses/")))
        .expect("prev href")
        .to_string();
    assert_eq!(
        href,
        format!(
            "/courses/{course}/gradebook/speedgrader?assignment_id={essay}&#38;student_id={student}"
        ),
        "prev link preserves assignment + points at the name-previous student"
    );
    // Following it (with the entity decoded, like a browser does) lands on
    // that student.
    let resp = server
        .get(&href.replace("&#38;", "&"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let body = resp.text();
    assert!(body.contains(&format!(
        "speedgrader-student-row selected\" data-user-id=\"{student}\""
    )));

    cleanup(&pool, &prefix).await;
}
