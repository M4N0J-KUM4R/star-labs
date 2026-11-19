//! Integration tests for star-features (Canvas feature_flags parity).
//!
//! Pinned behavior (Canvas feature_flags_controller + lib/feature_flags.rb):
//! - Flag states are Canvas's: off/on anywhere, allowed/allowed_on on
//!   accounts only; `PUT` rejects other states and a missing state with 400.
//! - Lookups inherit through the account chain (root → ... → context): a
//!   parent on/off flag is locked for children (PUT → 403); a parent
//!   allowed/allowed_on flag can be overridden.
//! - root_opt_in features are invisible below the root account until the
//!   root opts in, and read as a virtual off flag on the root account; a
//!   root account can never delete its opt-in flag.
//! - Account endpoints are admin/account-member only; course reads need
//!   enrollment or admin, course writes teacher or admin; user flags are
//!   self or admin. Missing or deleted contexts are 404 (before 403).
//! - POST /accounts/:id/features/early_access_program only exists on root
//!   accounts (404 elsewhere) and stores the acceptance in account settings.
//! - GET /features/environment requires a session and returns the fixed
//!   JS_ENV feature-name map.
//!
//! Fleet rule: every test uses DEDICATED accounts (never account 1) and
//! cleans up every seeded row, including feature_flags and account_users.
//! Skipped cleanly when DATABASE_URL is unset.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "ff_{}_{}",
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

    let server =
        TestServer::new(star_features::router().with_state(state)).expect("build TestServer");
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

/// Dedicated account per test (never account 1). `parent_id` None seeds a
/// root account.
async fn seed_account(pool: &PgPool, prefix: &str, name: &str, parent_id: Option<i64>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO accounts (name, parent_account_id) VALUES ($1, $2) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(parent_id)
    .fetch_one(pool)
    .await
    .expect("seed account")
}

async fn seed_account_user(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id) VALUES ($1, $2)")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account user");
}

async fn seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO courses (name, course_code, account_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(format!("{prefix}-{name}"))
    .bind(format!("{prefix}-{name}"))
    .bind(account_id)
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

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    // feature_flags rows for every seeded context (no FK; delete explicitly).
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_id IN (SELECT id FROM accounts WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_id IN (SELECT id FROM courses WHERE course_code LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "DELETE FROM feature_flags WHERE context_id IN (SELECT id FROM users WHERE email LIKE $1)",
    )
    .bind(format!("{prefix}%@test.local"))
    .execute(pool)
    .await;
    // Courses before accounts (courses.account_id has no ON DELETE).
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    // account_users cascade with accounts.
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Account scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn account_index_shape_and_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let member = seed_user(&pool, &prefix, "Member", "teacher").await;
    let root = seed_account(&pool, &prefix, "Root", None).await;
    seed_account_user(&pool, root, member).await;

    let resp = server
        .get(&format!("/accounts/{root}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let features = body.as_array().expect("array of features");
    assert!(!features.is_empty());

    let fancy = features
        .iter()
        .find(|f| f["feature"] == "fancy_wickets")
        .expect("fancy_wickets listed on root account");
    assert_eq!(fancy["display_name"], "Fancy Wickets");
    assert_eq!(fancy["applies_to"], "Account");
    assert_eq!(fancy["root_opt_in"], Value::Null);
    assert_eq!(fancy["beta"], Value::Null);
    assert_eq!(fancy["type"], "setting");
    // No override anywhere: the definition itself is the flag (global
    // default — no context fields, no locking_account_id).
    let flag = &fancy["feature_flag"];
    assert_eq!(flag["feature"], "fancy_wickets");
    assert_eq!(flag["state"], "allowed");
    assert!(flag.get("context_id").is_none());
    assert!(flag.get("context_type").is_none());
    assert!(flag.get("locking_account_id").is_none());
    assert_eq!(flag["locked"], false);
    assert_eq!(flag["parent_state"], "allowed");
    assert_eq!(flag["transitions"]["off"]["locked"], false);
    assert_eq!(flag["transitions"]["on"]["locked"], false);
    assert_eq!(flag["transitions"]["allowed"]["locked"], false);
    assert_eq!(flag["transitions"]["allowed_on"]["locked"], false);

    // Root accounts also see RootAccount features as a virtual off flag.
    let tutorial = features
        .iter()
        .find(|f| f["feature"] == "new_user_tutorial")
        .expect("new_user_tutorial listed on root account");
    let tutorial_flag = &tutorial["feature_flag"];
    assert_eq!(tutorial_flag["state"], "off");
    assert_eq!(tutorial_flag["context_type"], "Account");
    assert_eq!(tutorial_flag["context_id"], json!(root));
    assert_eq!(tutorial_flag["locking_account_id"], Value::Null);
    assert_eq!(tutorial_flag["locked"], false);
    assert_eq!(tutorial_flag["parent_state"], "off");
    // RootAccount features cannot be set to allowed states on the root.
    assert_eq!(tutorial_flag["transitions"]["on"]["locked"], false);
    assert_eq!(tutorial_flag["transitions"]["allowed"]["locked"], true);
    assert_eq!(tutorial_flag["transitions"]["allowed_on"]["locked"], true);

    // An account member (not a global admin) may read the features.
    let resp = server
        .get(&format!("/accounts/{root}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(member, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200);

    // Unassociated users may not.
    let resp = server
        .get(&format!("/accounts/{root}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Anonymous calls are rejected before the handler.
    let resp = server.get(&format!("/accounts/{root}/features")).await;
    assert_eq!(resp.status_code(), 401);

    // Missing account is 404 (before permission checks).
    let resp = server
        .get("/accounts/999999999/features")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Soft-deleted accounts are invisible.
    sqlx::query("UPDATE accounts SET workflow_state = 'deleted' WHERE id = $1")
        .bind(root)
        .execute(&pool)
        .await
        .expect("soft-delete account");
    let resp = server
        .get(&format!("/accounts/{root}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_flag_crud_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let _root = seed_account(&pool, &prefix, "Root", None).await;
    let account = seed_account(&pool, &prefix, "Sub", Some(_root)).await;
    let bearer = auth(admin, "admin");
    let url = format!("/accounts/{account}/features/flags/fancy_wickets");

    // Set the flag on.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["feature"], "fancy_wickets");
    assert_eq!(body["state"], "on");
    assert_eq!(body["context_type"], "Account");
    assert_eq!(body["context_id"], json!(account));
    assert_eq!(body["locking_account_id"], Value::Null);
    assert_eq!(body["locked"], false);
    assert_eq!(body["parent_state"], "allowed");
    assert_eq!(body["transitions"]["off"]["locked"], false);
    assert!(body["transitions"].get("on").is_none());
    assert_eq!(body["transitions"]["allowed"]["locked"], false);

    // Show returns the stored flag.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["context_id"], json!(account));

    // Accounts accept the inheritable states.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "allowed" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["state"], "allowed");

    // Invalid feature / states.
    let resp = server
        .put(&format!("/accounts/{account}/features/flags/not_a_feature"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 400);
    let resp = server
        .put(&format!("/accounts/{account}/features/flags/high_contrast"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 400, "user feature on an account");
    let resp = server
        .put(&format!(
            "/accounts/{account}/features/flags/new_user_tutorial"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(
        resp.status_code(),
        400,
        "RootAccount feature on a sub-account"
    );
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "bogus" }))
        .await;
    assert_eq!(resp.status_code(), 400);
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "missing state");

    // Unknown feature on show is 404 (Canvas RecordNotFound).
    let resp = server
        .get(&format!("/accounts/{account}/features/flags/not_a_feature"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // Delete the direct flag; show falls back to the global default.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["state"], "allowed");
    assert_eq!(body["context_id"], json!(account));

    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["state"], "allowed");
    assert!(
        body.get("context_id").is_none(),
        "default flag has no context"
    );

    // Deleting again: no direct flag left on the context.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // Missing account is 404 for every verb.
    let resp = server
        .put("/accounts/999999999/features/flags/fancy_wickets")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .delete("/accounts/999999999/features/flags/fancy_wickets")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_inheritance_and_locking() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let root = seed_account(&pool, &prefix, "Root", None).await;
    let sub = seed_account(&pool, &prefix, "Sub", Some(root)).await;
    let bearer = auth(admin, "admin");

    // Parent forces the feature off: children inherit a locked flag.
    let resp = server
        .put(&format!("/accounts/{root}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "off" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!("/accounts/{sub}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["state"], "off");
    assert_eq!(body["context_id"], json!(root));
    assert_eq!(body["context_type"], "Account");
    assert_eq!(body["locked"], true);
    // parent_state resolves from the ancestors: the root's off flag.
    assert_eq!(body["parent_state"], "off");

    // The child cannot override a locked parent flag.
    let resp = server
        .put(&format!("/accounts/{sub}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 403, "higher account disallows");

    // Parent "allowed" lets children override.
    let resp = server
        .put(&format!("/accounts/{root}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "allowed" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let resp = server
        .get(&format!("/accounts/{sub}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["state"], "allowed");
    assert_eq!(body["context_id"], json!(root));
    assert_eq!(body["locked"], false);

    let resp = server
        .put(&format!("/accounts/{sub}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["context_id"], json!(sub));

    // Deleting the child flag re-exposes the inherited parent flag.
    let resp = server
        .delete(&format!("/accounts/{sub}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let resp = server
        .get(&format!("/accounts/{sub}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["context_id"], json!(root));

    // Root turns it on for everyone below: inherited + enabled + locked.
    let resp = server
        .put(&format!("/accounts/{root}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let resp = server
        .get(&format!("/accounts/{sub}/features/enabled"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let enabled: Value = resp.json();
    assert_eq!(
        enabled,
        json!(["fancy_wickets"]),
        "enabled lists inherited enabled features"
    );

    // hide_inherited_enabled omits flags forced on by a higher context.
    let resp = server
        .get(&format!(
            "/accounts/{sub}/features?hide_inherited_enabled=true"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let hidden_list: Value = resp.json();
    assert!(
        hidden_list
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["feature"] != "fancy_wickets"),
        "locked-enabled flag omitted: {hidden_list}"
    );
    assert!(
        hidden_list
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["feature"] == "automatic_essay_grading"),
        "non-enabled flags stay listed"
    );

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn account_members_manage_flags() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    // A plain (non-global-admin) user holding an account_users grant on the
    // ROOT account administers the whole sub-tree (Canvas AccountAdmin).
    let subadmin = seed_user(&pool, &prefix, "Subadmin", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let root = seed_account(&pool, &prefix, "Root", None).await;
    let sub = seed_account(&pool, &prefix, "Sub", Some(root)).await;
    seed_account_user(&pool, root, subadmin).await;
    let bearer = auth(subadmin, "teacher");

    let resp = server
        .put(&format!("/accounts/{sub}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!("/accounts/{sub}/features"))
        .add_header(axum::http::header::AUTHORIZATION, bearer)
        .await;
    assert_eq!(resp.status_code(), 200);

    let resp = server
        .put(&format!("/accounts/{sub}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Course scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn course_flag_permissions_and_states() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let account = seed_account(&pool, &prefix, "Acct", None).await;
    let course = seed_course(&pool, &prefix, "Flags", account).await;
    seed_enrollment(&pool, teacher, course, "teacher").await;
    seed_enrollment(&pool, student, course, "student").await;
    let url = format!("/courses/{course}/features/flags/automatic_essay_grading");

    // Students cannot write flags.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .json(&json!({ "state": "enabled" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Teachers can; states are Canvas's off/on.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["state"], "on");
    assert_eq!(body["context_type"], "Course");
    assert_eq!(body["context_id"], json!(course));
    assert_eq!(body["parent_state"], "allowed");
    // Courses only transition between off/on.
    assert!(body["transitions"].get("allowed").is_none());
    assert_eq!(body["transitions"]["off"]["locked"], false);

    // "allowed" is an account-only state.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({ "state": "allowed" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // Enrolled students read the flag; outsiders cannot.
    let resp = server
        .get(&format!("/courses/{course}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    let flag_entry = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["feature"] == "automatic_essay_grading")
        .expect("listed feature");
    assert_eq!(flag_entry["feature_flag"]["state"], "on");
    assert_eq!(flag_entry["feature_flag"]["context_id"], json!(course));

    let resp = server
        .get(&format!("/courses/{course}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Enabled lists the course flag.
    let resp = server
        .get(&format!("/courses/{course}/features/enabled"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>(), json!(["automatic_essay_grading"]));

    // Teacher deletes the flag.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["state"], "on");

    // Missing course is 404; soft-deleted courses too.
    let resp = server
        .get("/courses/999999999/features")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);
    sqlx::query("UPDATE courses SET workflow_state = 'deleted' WHERE id = $1")
        .bind(course)
        .execute(&pool)
        .await
        .expect("soft-delete course");
    let resp = server
        .get(&format!("/courses/{course}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn course_inherits_account_flags() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let root = seed_account(&pool, &prefix, "Root", None).await;
    let sub = seed_account(&pool, &prefix, "Sub", Some(root)).await;
    let course = seed_course(&pool, &prefix, "Course", sub).await;
    let bearer = auth(admin, "admin");
    let url = format!("/courses/{course}/features/flags/automatic_essay_grading");

    // The course's account turns the feature on: the course inherits it.
    let resp = server
        .put(&format!(
            "/accounts/{sub}/features/flags/automatic_essay_grading"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["state"], "on");
    assert_eq!(body["context_type"], "Account");
    assert_eq!(body["context_id"], json!(sub));
    assert_eq!(body["locked"], true);

    // The course cannot override the account's on/off flag...
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "off" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // ...but "allowed" on the account leaves the course free to choose.
    let resp = server
        .put(&format!(
            "/accounts/{sub}/features/flags/automatic_essay_grading"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "allowed" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "off" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["context_type"], "Course");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// root_opt_in semantics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn root_opt_in_features_need_root_opt_in() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let root = seed_account(&pool, &prefix, "Root", None).await;
    let sub = seed_account(&pool, &prefix, "Sub", Some(root)).await;
    let course = seed_course(&pool, &prefix, "Course", sub).await;
    let bearer = auth(admin, "admin");
    let course_url = format!("/courses/{course}/features/flags/epub_export");

    // Before the root opts in, epub_export is invisible below the root...
    let resp = server
        .get(&format!("/courses/{course}/features"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["feature"] != "epub_export"),
        "root_opt_in feature hidden before opt-in: {listed}"
    );

    let resp = server
        .get(&course_url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // ...though the root account itself sees a virtual off flag.
    let resp = server
        .get(&format!("/accounts/{root}/features/flags/epub_export"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["state"], "off");
    assert_eq!(body["context_id"], json!(root));

    // The root opts in with "allowed"...
    let resp = server
        .put(&format!("/accounts/{root}/features/flags/epub_export"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "allowed" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // ...and the feature appears below, inherited and overridable.
    let resp = server
        .get(&course_url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let body: Value = resp.json();
    assert_eq!(body["state"], "allowed");
    assert_eq!(body["context_id"], json!(root));
    assert_eq!(body["locked"], false);

    let resp = server
        .put(&course_url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    let resp = server
        .get(&format!("/courses/{course}/features/enabled"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.json::<Value>(), json!(["epub_export"]));

    // The root account can never delete its opt-in flag.
    let resp = server
        .delete(&format!("/accounts/{root}/features/flags/epub_export"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 403);

    // Sub-accounts without their own flag have nothing to delete.
    let resp = server
        .delete(&format!("/accounts/{sub}/features/flags/epub_export"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 404);

    // The course can drop its own override again.
    let resp = server
        .delete(&course_url)
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);

    // RootAccount features: transitions to allowed states are locked on the
    // root account itself.
    let resp = server
        .put(&format!(
            "/accounts/{root}/features/flags/new_user_tutorial"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "allowed" }))
        .await;
    assert_eq!(resp.status_code(), 403, "state change not allowed");
    let resp = server
        .put(&format!(
            "/accounts/{root}/features/flags/new_user_tutorial"
        ))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["state"], "on");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// User scope
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_scope_flags() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let alice = seed_user(&pool, &prefix, "Alice", "student").await;
    let bob = seed_user(&pool, &prefix, "Bob", "student").await;
    let alice_bearer = auth(alice, "student");
    let url = format!("/users/{alice}/features/flags/high_contrast");

    // Only the two User features apply; defaults are global flags.
    let resp = server
        .get(&format!("/users/{alice}/features"))
        .add_header(axum::http::header::AUTHORIZATION, alice_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let names: Vec<&str> = body
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["feature"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["high_contrast", "underline_all_links"]);
    assert_eq!(body[0]["feature_flag"]["state"], "allowed");
    assert!(body[0]["feature_flag"].get("context_id").is_none());

    // Alice manages her own flag.
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, alice_bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["state"], "on");
    assert_eq!(body["context_type"], "User");
    assert_eq!(body["context_id"], json!(alice));
    assert_eq!(body["locked"], false);

    let resp = server
        .get(&format!("/users/{alice}/features/enabled"))
        .add_header(axum::http::header::AUTHORIZATION, alice_bearer.clone())
        .await;
    assert_eq!(resp.json::<Value>(), json!(["high_contrast"]));

    // Bob can neither read nor write Alice's flags.
    let resp = server
        .get(&format!("/users/{alice}/features"))
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);
    let resp = server
        .put(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(bob, "student"))
        .json(&json!({ "state": "off" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Admins manage user flags.
    let resp = server
        .put(&format!(
            "/users/{alice}/features/flags/underline_all_links"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "state": "off" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());

    // Account features do not apply to users.
    let resp = server
        .put(&format!("/users/{alice}/features/flags/fancy_wickets"))
        .add_header(axum::http::header::AUTHORIZATION, alice_bearer.clone())
        .json(&json!({ "state": "on" }))
        .await;
    assert_eq!(resp.status_code(), 400);

    // Deleting restores the default.
    let resp = server
        .delete(&url)
        .add_header(axum::http::header::AUTHORIZATION, alice_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, alice_bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>()["state"], "allowed");

    // Missing user is 404.
    let resp = server
        .get("/users/999999999/features")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Early Access Program
// ---------------------------------------------------------------------------

#[tokio::test]
async fn early_access_program_root_accounts_only() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let root = seed_account(&pool, &prefix, "Root", None).await;
    let sub = seed_account(&pool, &prefix, "Sub", Some(root)).await;

    // Only root accounts expose the endpoint (404 before the permission
    // check).
    let resp = server
        .post(&format!("/accounts/{sub}/features/early_access_program"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);
    let resp = server
        .post("/accounts/999999999/features/early_access_program")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 404);

    // Non-admins are rejected on root accounts.
    let resp = server
        .post(&format!("/accounts/{root}/features/early_access_program"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Admin acceptance is stored and echoed.
    let resp = server
        .post(&format!("/accounts/{root}/features/early_access_program"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>(),
        json!({ "early_access_program": true })
    );

    let stored: (bool,) = sqlx::query_as(
        "SELECT (settings->'early_access_program'->>'value') = 'true' FROM accounts WHERE id = $1",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .expect("read settings");
    assert!(stored.0, "EAP acceptance stored in account settings");

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Environment
// ---------------------------------------------------------------------------

#[tokio::test]
async fn environment_requires_session_and_lists_js_env_features() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let student = seed_user(&pool, &prefix, "Student", "student").await;

    // Anonymous calls are rejected (the non-mobile Canvas path).
    let resp = server.get("/features/environment").await;
    assert_eq!(resp.status_code(), 401);

    let resp = server
        .get("/features/environment")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    let env = body.as_object().expect("feature map");
    // Fixed JS_ENV lists (site-admin + root-account + services + brand),
    // minus the discussion_checkpoints duplicate.
    assert_eq!(env.len(), 36 + 58 + 1 + 2 - 1);
    for name in [
        "new_quizzes_media_type",
        "product_tours",
        "lti_registrations_next",
        "account_survey_notifications",
        "embedded_release_notes",
    ] {
        assert!(
            env.contains_key(name),
            "JS_ENV feature {name} missing from environment"
        );
        assert_eq!(env[name], false, "{name} has no flag on the root account");
    }

    cleanup(&pool, &prefix).await;
}
