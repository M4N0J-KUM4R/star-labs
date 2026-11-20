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
    let server = TestServer::new(star_ui::router().with_state(state)).expect("build TestServer");
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

/// AC3 + AC4 + AC8: dashboard renders 200 with the 18 phase-1 ENV keys of the
/// right types, one card per enrolled course linking /courses/{id}, an
/// unpublished style for unpublished courses, and only hashed asset links.
#[tokio::test]
async fn dashboard_renders_cards_and_env_contract() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "dashuser", "student", "not-a-real-hash").await;
    let published = seed_course(&pool, &prefix, "Published Course", "available").await;
    let unpublished = seed_course(&pool, &prefix, "Draft Course", "unpublished").await;
    seed_enrollment(&pool, user, published, "student").await;
    seed_enrollment(&pool, user, unpublished, "student").await;

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

    // Global nav.
    assert!(body.contains("ic-app-header"));
    assert!(body.contains("global_nav_dashboard_link"));

    // One card per enrolled course, linking /courses/{id}.
    assert!(body.contains(&format!("data-course-id=\"{published}\"")));
    assert!(body.contains(&format!("href=\"/courses/{published}\"")));
    assert!(body.contains(&format!("data-course-id=\"{unpublished}\"")));
    // Unpublished style per the API's published flag.
    assert!(body.contains("ic-DashboardCard__unpublished"));

    // AC8: hashed asset link present; sole inline <script> is the ENV block.
    let hash = star_ui::assets::app_css_hash();
    assert!(
        body.contains(&format!("/static/{hash}/app.css")),
        "hashed CSS link missing"
    );
    let script_count = body.matches("<script>").count();
    let env_block_count = body.matches("window.ENV = ").count();
    assert_eq!(
        script_count, 1,
        "exactly one inline <script> (the ENV block)"
    );
    assert_eq!(env_block_count, 1);

    // Static asset serves with immutable caching; wrong hash 404s.
    let asset = server.get(&format!("/static/{hash}/app.css")).await;
    assert_eq!(asset.status_code(), 200);
    let cache_control = asset
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(cache_control.contains("immutable"));
    let bad = server.get("/static/wrong-hash/app.css").await;
    assert_eq!(bad.status_code(), 404);

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

/// New quiz form: teacher POST creates (redirects to show); student POST 403.
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

    // Teacher GET renders the form.
    let page = server
        .get(&format!("/courses/{course}/quizzes/new"))
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(page.status_code(), 200);
    assert!(page.text().contains("new_quiz_form"));

    // Teacher POST creates and 303s to the show page.
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
    let new_id: i64 = location
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

    // Show page renders for the teacher.
    let show = server
        .get(&location)
        .add_cookie(session_cookie(teacher, "teacher"))
        .await;
    assert_eq!(show.status_code(), 200);
    assert!(show.text().contains("Created By Form"));

    // Student POST is 403.
    let (csrf_cookie, csrf) = csrf_pair();
    let resp = server
        .post(&format!("/courses/{course}/quizzes/new"))
        .add_cookie(session_cookie(student, "student"))
        .add_cookie(csrf_cookie)
        .form(&serde_json::json!({"_csrf": csrf, "title": "Nope", "published": ""}))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Student GET is 403 too.
    let resp = server
        .get(&format!("/courses/{course}/quizzes/new"))
        .add_cookie(session_cookie(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

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
        resp.headers().get("location").unwrap(),
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
    let (quiz, mc, essay) = seed_graded_quiz(&pool, course).await;

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
    let body_pairs = vec![
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
        .header("content-type", "application/x-www-form-urlencoded")
        .raw_body(body)
        .await;
    assert_eq!(resp.status_code(), 303, "submit: {}", resp.text());
    assert_eq!(
        resp.headers().get("location").unwrap(),
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
    assert!(body.contains(&format!("attempt 0")));

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
