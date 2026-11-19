//! Integration tests for the users-controller tail group B (issue #57):
//! - PUT /users/{id}/text_editor_preference       (set_text_editor_preference)
//! - PUT /users/{id}/files_ui_version_preference   (set_files_ui_version_preference)
//! - GET /users/{id}/new_user_tutorial_statuses    (get_new_user_tutorial_statuses)
//! - PUT /users/{id}/new_user_tutorial_statuses/{page} (set_new_user_tutorial_status)
//! - PUT /users/{id}/merge_into/{dest}             (merge_into, simple form)
//! - PUT /users/{id}/merge_into/accounts/{acct}/users/{dest} (cross-account form)
//! - POST /users/{id}/split                        (split)
//!
//! Migration 0123 (user_preferences + user_merges) is exercised end-to-end.
//!
//! Conventions (mirrors users_integration.rs):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Unique per-run prefix + full teardown keep repeated runs safe.

use axum_test::TestServer;
use serde_json::{json, Value};
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
        "usr-tailb-{}-{}",
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

    let server = TestServer::new(star_users_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn star_users_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_users::router())
        .with_state(state)
}

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

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

async fn seed_course(pool: &PgPool, prefix: &str, label: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix} {label}"))
        .bind(format!("{prefix}-{label}"))
        .fetch_one(pool)
        .await
        .expect("seed course")
}

async fn enroll(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state) \
         VALUES ($1, $2, $3, 'active') ON CONFLICT DO NOTHING",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

/// Delete everything this run created (users cascade the preference/merge
/// rows; courses go by prefix).
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
    // Split-restored users carry derived emails under the same prefix shape.
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("restored-%-{prefix}%"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// PUT /users/{id}/text_editor_preference
// ---------------------------------------------------------------------------

#[tokio::test]
async fn text_editor_preference_roundtrip_and_clear() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "EditorUser", "teacher").await;

    let set = server
        .put(&format!("/users/{user}/text_editor_preference"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "teacher"))
        .json(&json!({ "text_editor_preference": "rce" }))
        .await;
    assert_eq!(set.status_code(), 200, "body: {}", set.text());
    let body: Value = set.json();
    assert_eq!(body["text_editor_preference"], "rce");

    // Canvas stores "" as null and reads it back as null.
    let clear = server
        .put(&format!("/users/{user}/text_editor_preference"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "teacher"))
        .json(&json!({ "text_editor_preference": "" }))
        .await;
    assert_eq!(clear.status_code(), 200);
    let body: Value = clear.json();
    assert_eq!(body["text_editor_preference"], Value::Null);

    // Invalid value: Canvas raises ActiveRecord::RecordInvalid -> 500.
    let invalid = server
        .put(&format!("/users/{user}/text_editor_preference"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "teacher"))
        .json(&json!({ "text_editor_preference": "ms_word" }))
        .await;
    assert_eq!(invalid.status_code(), 500);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn text_editor_preference_permissions_and_404() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "EditorOwner", "student").await;
    let other = seed_user(&pool, &prefix, "EditorOther", "student").await;
    let admin = seed_user(&pool, &prefix, "EditorAdmin", "admin").await;

    // Another plain user: Canvas authorized_action [:manage, :manage_user_details] -> 403.
    let forbidden = server
        .put(&format!("/users/{owner}/text_editor_preference"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "text_editor_preference": "rce" }))
        .await;
    assert_eq!(forbidden.status_code(), 403);

    // Admin may set another user's preference.
    let as_admin = server
        .put(&format!("/users/{owner}/text_editor_preference"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "text_editor_preference": "block_editor" }))
        .await;
    assert_eq!(as_admin.status_code(), 200, "body: {}", as_admin.text());

    // Unknown user: api_find runs first -> 404 even before the permission.
    let missing = server
        .put("/users/999999999/text_editor_preference")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "text_editor_preference": "rce" }))
        .await;
    assert_eq!(missing.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /users/{id}/files_ui_version_preference
// ---------------------------------------------------------------------------

#[tokio::test]
async fn files_ui_version_preference_roundtrip_and_invalid() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "FilesUiUser", "student").await;

    let set = server
        .put(&format!("/users/{user}/files_ui_version_preference"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "files_ui_version": "v2" }))
        .await;
    assert_eq!(set.status_code(), 200, "body: {}", set.text());
    let body: Value = set.json();
    assert_eq!(body["files_ui_version"], "v2");

    // Invalid version -> 400 with Canvas' message.
    let invalid = server
        .put(&format!("/users/{user}/files_ui_version_preference"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "files_ui_version": "v3" }))
        .await;
    assert_eq!(invalid.status_code(), 400);

    // Non-admin may not touch another user's preference.
    let other = seed_user(&pool, &prefix, "FilesUiOther", "student").await;
    let forbidden = server
        .put(&format!("/users/{other}/files_ui_version_preference"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "files_ui_version": "v1" }))
        .await;
    assert_eq!(forbidden.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// GET/PUT /users/{id}/new_user_tutorial_statuses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tutorial_statuses_current_user_only() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "TutorialUser", "student").await;
    let other = seed_user(&pool, &prefix, "TutorialOther", "student").await;
    let admin = seed_user(&pool, &prefix, "TutorialAdmin", "admin").await;

    // Someone else's id — even an admin's — is a 401 (Canvas: "only works
    // against the current user").
    let as_other = server
        .get(&format!("/users/{user}/new_user_tutorial_statuses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(as_other.status_code(), 401);

    let as_admin = server
        .get(&format!("/users/{user}/new_user_tutorial_statuses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(as_admin.status_code(), 401, "admins also get the 401");

    // Fresh user: empty collapsed map.
    let fresh = server
        .get(&format!("/users/{user}/new_user_tutorial_statuses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(fresh.status_code(), 200, "body: {}", fresh.text());
    let body: Value = fresh.json();
    assert_eq!(body["new_user_tutorial_statuses"]["collapsed"], json!({}));

    // Set two pages.
    for (page, collapsed) in [("home", true), ("grades", false)] {
        let put = server
            .put(&format!("/users/{user}/new_user_tutorial_statuses/{page}"))
            .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
            .json(&json!({ "collapsed": collapsed }))
            .await;
        assert_eq!(put.status_code(), 200, "page {page}: {}", put.text());
    }
    let after: Value = server
        .get(&format!("/users/{user}/new_user_tutorial_statuses"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await
        .json();
    assert_eq!(
        after["new_user_tutorial_statuses"]["collapsed"],
        json!({ "home": true, "grades": false })
    );

    // Invalid page name -> 400 with Canvas' message.
    let invalid = server
        .put(&format!(
            "/users/{user}/new_user_tutorial_statuses/rocket_science"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "collapsed": true }))
        .await;
    assert_eq!(invalid.status_code(), 400);

    // Setting someone else's page is still the 401 current-user check.
    let as_other_put = server
        .put(&format!("/users/{other}/new_user_tutorial_statuses/home"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "collapsed": true }))
        .await;
    assert_eq!(as_other_put.status_code(), 401);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// PUT /users/{id}/merge_into/{destination_user_id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn merge_into_moves_data_and_records_the_merge() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "MergeAdmin", "admin").await;
    let from = seed_user(&pool, &prefix, "Merge From", "student").await;
    let into = seed_user(&pool, &prefix, "Merge Into", "student").await;
    let course_a = seed_course(&pool, &prefix, "merge-a").await;
    let course_b = seed_course(&pool, &prefix, "merge-b").await;

    enroll(&pool, from, course_a, "student").await;
    enroll(&pool, into, course_b, "student").await;
    // A conflicting enrollment on the same course+role: the survivor keeps
    // theirs; the from-user's duplicate is dropped.
    enroll(&pool, from, course_b, "student").await;
    sqlx::query("INSERT INTO communication_channels (user_id, path) VALUES ($1, $2)")
        .bind(from)
        .bind(format!("{prefix}-cc-from@test.local"))
        .execute(&pool)
        .await
        .expect("seed cc");

    let resp = server
        .put(&format!("/users/{from}/merge_into/{into}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let body: Value = resp.json();
    assert_eq!(body["id"], into, "responds with the surviving user");
    assert_eq!(body["name"], "Merge Into");

    // The merged-away user is gone (Canvas soft-deletes; this schema deletes).
    let gone: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(from)
        .fetch_optional(&pool)
        .await
        .expect("check from-user");
    assert!(gone.is_none(), "from_user row removed");

    // The merge ledger recorded the snapshot (split depends on it).
    let ledger: (i64, String, String) = sqlx::query_as(
        "SELECT from_user_id, COALESCE(from_name,''), COALESCE(from_email,'') \
         FROM user_merges WHERE user_id = $1 AND workflow_state = 'active'",
    )
    .bind(into)
    .fetch_one(&pool)
    .await
    .expect("merge ledger row");
    assert_eq!(ledger.0, from);
    assert_eq!(ledger.1, "Merge From");
    assert_eq!(
        ledger.2,
        format!("{prefix}-Merge From@test.local"),
        "snapshot email recorded"
    );

    // Enrollments moved; the conflicting duplicate on course_b was dropped.
    let enrollments: Vec<(i64,)> =
        sqlx::query_as("SELECT course_id FROM enrollments WHERE user_id = $1 ORDER BY course_id")
            .bind(into)
            .fetch_all(&pool)
            .await
            .expect("enrollments");
    let ids: Vec<i64> = enrollments.into_iter().map(|(c,)| c).collect();
    assert_eq!(ids, vec![course_a, course_b], "both unique enrollments");

    // Communication channel moved to the survivor.
    let cc: Option<(i64,)> =
        sqlx::query_as("SELECT user_id FROM communication_channels WHERE path = $1")
            .bind(format!("{prefix}-cc-from@test.local"))
            .fetch_optional(&pool)
            .await
            .expect("cc check");
    assert_eq!(cc.map(|(u,)| u), Some(into));

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn merge_into_permission_and_404_order() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "MergeAdmin2", "admin").await;
    let plain = seed_user(&pool, &prefix, "MergePlain", "student").await;
    let from = seed_user(&pool, &prefix, "Merge From2", "student").await;
    let into = seed_user(&pool, &prefix, "Merge Into2", "student").await;

    // Non-admin without the merge right: 403.
    let forbidden = server
        .put(&format!("/users/{from}/merge_into/{into}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(plain, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403);

    // Unknown from-user: api_find 404 before the permission.
    let missing_from = server
        .put(&format!("/users/999999999/merge_into/{into}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing_from.status_code(), 404);

    // Unknown destination user: 404 after passing the from-side checks.
    let missing_into = server
        .put(&format!("/users/{from}/merge_into/999999999"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing_into.status_code(), 404);

    // Self-merge: Canvas renders nothing (empty success), no data moves.
    let self_merge = server
        .put(&format!("/users/{from}/merge_into/{from}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(self_merge.status_code(), 200, "body: {}", self_merge.text());
    let still: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE id = $1")
        .bind(from)
        .fetch_optional(&pool)
        .await
        .expect("self-merge check");
    assert!(still.is_some(), "self-merge deletes nothing");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn merge_into_cross_account_form_checks_destination_account() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "MergeAdmin3", "admin").await;
    let from = seed_user(&pool, &prefix, "Merge From3", "student").await;
    let into = seed_user(&pool, &prefix, "Merge Into3", "student").await;
    let account_id: i64 =
        sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
            .bind(format!("{prefix} account"))
            .fetch_one(&pool)
            .await
            .expect("seed account");

    // Unknown destination account -> 404 (Canvas Account.find).
    let missing_account = server
        .put(&format!(
            "/users/{from}/merge_into/accounts/999999999/users/{into}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing_account.status_code(), 404);

    // Valid account form merges like the simple form.
    let ok = server
        .put(&format!(
            "/users/{from}/merge_into/accounts/{account_id}/users/{into}"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(ok.status_code(), 200, "body: {}", ok.text());
    let body: Value = ok.json();
    assert_eq!(body["id"], into);

    // The stale account row would leak without cleanup; delete by name.
    let _ = sqlx::query("DELETE FROM accounts WHERE name LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(&pool)
        .await;

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// POST /users/{id}/split
// ---------------------------------------------------------------------------

#[tokio::test]
async fn split_restores_merged_user_and_marks_ledger_deleted() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "SplitAdmin", "admin").await;
    let from = seed_user(&pool, &prefix, "Split From", "student").await;
    let into = seed_user(&pool, &prefix, "Split Into", "student").await;
    let course = seed_course(&pool, &prefix, "split-course").await;
    enroll(&pool, from, course, "student").await;

    // Merge first.
    let merge = server
        .put(&format!("/users/{from}/merge_into/{into}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(merge.status_code(), 200, "body: {}", merge.text());

    // Split back.
    let split = server
        .post(&format!("/users/{into}/split"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(split.status_code(), 200, "body: {}", split.text());
    let users: Vec<Value> = split.json();
    assert_eq!(users.len(), 2, "both users returned");
    let names: Vec<&str> = users.iter().map(|u| u["name"].as_str().unwrap()).collect();
    assert!(
        names.contains(&"Split From") && names.contains(&"Split Into"),
        "restored user keeps its name: {names:?}"
    );

    // The restored user exists again and owns the enrollment again.
    let restored: (i64,) = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(format!("{prefix}-Split From@test.local"))
        .fetch_one(&pool)
        .await
        .expect("restored user re-created with original email");
    let enrollment: (i64,) = sqlx::query_as("SELECT user_id FROM enrollments WHERE course_id = $1")
        .bind(course)
        .fetch_one(&pool)
        .await
        .expect("enrollment survived split");
    assert_eq!(enrollment.0, restored.0, "enrollment moved back");

    // The ledger entry is marked deleted -> a second split is the 400.
    // (The row is keyed to the surviving user; the restored user got a new id.)
    let ledger_state: (String,) = sqlx::query_as(
        "SELECT workflow_state FROM user_merges WHERE user_id = $1 AND from_email = $2",
    )
    .bind(into)
    .bind(format!("{prefix}-Split From@test.local"))
    .fetch_one(&pool)
    .await
    .expect("ledger row");
    assert_eq!(ledger_state.0, "deleted");

    let second = server
        .post(&format!("/users/{into}/split"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(second.status_code(), 400, "nothing left to split");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn split_400_message_and_permission_order() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "SplitAdmin2", "admin").await;
    let plain = seed_user(&pool, &prefix, "SplitPlain", "student").await;
    let user = seed_user(&pool, &prefix, "SplitUser", "student").await;

    // Canvas checks "nothing to split" BEFORE the permission, so even a
    // non-admin gets the 400 on a user with no merge history.
    let plain_split = server
        .post(&format!("/users/{user}/split"))
        .add_header(axum::http::header::AUTHORIZATION, auth(plain, "student"))
        .await;
    assert_eq!(plain_split.status_code(), 400);

    // Unknown user: api_find 404.
    let missing = server
        .post("/users/999999999/split")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(missing.status_code(), 404);

    // With merge history, a non-admin hits the :merge 403.
    let from = seed_user(&pool, &prefix, "Split From2", "student").await;
    let merge = server
        .put(&format!("/users/{from}/merge_into/{user}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(merge.status_code(), 200, "body: {}", merge.text());
    let forbidden = server
        .post(&format!("/users/{user}/split"))
        .add_header(axum::http::header::AUTHORIZATION, auth(plain, "student"))
        .await;
    assert_eq!(forbidden.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn split_requires_auth() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let resp = server.post("/users/1/split").await;
    assert_eq!(resp.status_code(), 401, "load_user precedes the action");

    cleanup(&pool, &prefix).await;
}
