//! Integration tests for the PORT-B user gaps:
//! - GET/POST/DELETE /users/self/favorites/groups[/{id}] (FavoritesController)
//! - PUT /users/self/communication_channels/.../notification_preferences[...]
//!   (NotificationPreferencesController, 3 routes)
//! - POST /users/{userId}/observer_pairing_codes
//! - GET  /users/{userId}/avatars                       (profile_pics)
//!
//! Tables: favorites (0097, context_type='Group'), groups (0018),
//! communication_channels + notification_preferences (0045),
//! observer_pairing_codes (0225).
//!
//! Conventions (mirrors users_tail_group_b_integration.rs):
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
        "usr-gaps-{}-{}",
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

/// Seed a communication channel; 0045 requires path/path_type/workflow_state.
async fn seed_channel(pool: &PgPool, user_id: i64, address: &str) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO communication_channels (user_id, path, path_type, workflow_state)
        VALUES ($1, $2, 'email', 'active')
        RETURNING id
        "#,
    )
    .bind(user_id)
    .bind(address)
    .fetch_one(pool)
    .await
    .expect("seed channel")
}

/// Delete everything this run created (users cascade their channel/policy/
/// pairing-code rows; courses go by prefix).
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM courses WHERE course_code LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// favorite groups
// ---------------------------------------------------------------------------

#[tokio::test]
async fn favorite_groups_add_list_remove_reset() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "FavUser", "student").await;
    let course_id = seed_course(&pool, &prefix, "fav-course").await;
    let group_id: i64 =
        sqlx::query_scalar("INSERT INTO groups (course_id, name) VALUES ($1, $2) RETURNING id")
            .bind(course_id)
            .bind(format!("{prefix} study group"))
            .fetch_one(&pool)
            .await
            .expect("seed group");

    let unknown = server
        .post("/users/self/favorites/groups/999999999")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(unknown.status_code(), 404, "unknown group");

    let added = server
        .post(&format!("/users/self/favorites/groups/{group_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(added.status_code(), 200, "body: {}", added.text());
    let group: Value = added.json();
    assert_eq!(group["id"], group_id);
    assert_eq!(group["name"], format!("{prefix} study group"));
    assert_eq!(group["course_id"], course_id);

    let index = server
        .get("/users/self/favorites/groups")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(index.status_code(), 200);
    let groups: Vec<Value> = index.json();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0]["id"], group_id);

    let removed = server
        .delete(&format!("/users/self/favorites/groups/{group_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(removed.status_code(), 204);

    let remove_missing = server
        .delete(&format!("/users/self/favorites/groups/{group_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(remove_missing.status_code(), 404, "absent favorite");

    // Reset clears all group favorites.
    let re_add = server
        .post(&format!("/users/self/favorites/groups/{group_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(re_add.status_code(), 200);
    let reset = server
        .delete("/users/self/favorites/groups")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    assert_eq!(reset.status_code(), 204);
    let empty = server
        .get("/users/self/favorites/groups")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .await;
    let empty_body: Value = empty.json();
    assert_eq!(empty_body.as_array().expect("array").len(), 0);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// notification preferences
// ---------------------------------------------------------------------------

#[tokio::test]
async fn notification_preferences_update_update_all_and_category() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "NpUser", "student").await;
    let address = format!("{prefix}-np@test.local");
    let channel_id = seed_channel(&pool, user, &address).await;
    let base = format!("/users/self/communication_channels/email/{address}");

    // Unknown channel -> 404.
    let unknown = server
        .put("/users/self/communication_channels/email/ghost@test.local/notification_preferences/new_announcement")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "notification_preferences": { "frequency": "daily" } }))
        .await;
    assert_eq!(unknown.status_code(), 404);

    // Invalid frequency -> 422.
    let invalid = server
        .put(&format!("{base}/notification_preferences/new_announcement"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "notification_preferences": { "frequency": "hourly" } }))
        .await;
    assert_eq!(invalid.status_code(), 422);

    // Single update.
    let single = server
        .put(&format!("{base}/notification_preferences/new_announcement"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "notification_preferences": { "frequency": "daily" } }))
        .await;
    assert_eq!(single.status_code(), 200, "body: {}", single.text());
    let single_body: Value = single.json();
    let prefs: Vec<Value> = single_body["notification_preferences"]
        .as_array()
        .expect("preferences")
        .clone();
    assert_eq!(prefs.len(), 1);
    assert_eq!(prefs[0]["notification"], "new_announcement");
    assert_eq!(prefs[0]["frequency"], "daily");
    assert_eq!(prefs[0]["category"], "course_activities");

    // update_all with a list.
    let update_all = server
        .put(&format!("{base}/notification_preferences"))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({
            "notification_preferences": [
                { "notification": "new_announcement", "frequency": "weekly" },
                { "notification": "submission_grade_change", "frequency": "never" },
            ]
        }))
        .await;
    assert_eq!(update_all.status_code(), 200, "body: {}", update_all.text());
    let update_all_body: Value = update_all.json();
    let all: Vec<Value> = update_all_body["notification_preferences"]
        .as_array()
        .expect("preferences")
        .clone();
    assert_eq!(all.len(), 2, "both preferences upserted");

    // Category route: 'all' flips every policy of the channel.
    let by_category = server
        .put(&format!(
            "/users/self/communication_channels/{channel_id}/notification_preference_categories/all"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "notification_preferences": { "frequency": "immediately" } }))
        .await;
    assert_eq!(
        by_category.status_code(),
        200,
        "body: {}",
        by_category.text()
    );
    let by_category_body: Value = by_category.json();
    let after: Vec<Value> = by_category_body["notification_preferences"]
        .as_array()
        .expect("preferences")
        .clone();
    assert!(after.iter().all(|p| p["frequency"] == "immediately"));

    // Unknown channel id on the category route -> 404; bad category -> 422.
    let unknown_channel = server
        .put("/users/self/communication_channels/999999999/notification_preference_categories/all")
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "notification_preferences": { "frequency": "daily" } }))
        .await;
    assert_eq!(unknown_channel.status_code(), 404);
    let bad_category = server
        .put(&format!(
            "/users/self/communication_channels/{channel_id}/notification_preference_categories/nonsense"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(user, "student"))
        .json(&json!({ "notification_preferences": { "frequency": "daily" } }))
        .await;
    assert_eq!(bad_category.status_code(), 422);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// observer pairing codes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn observer_pairing_codes_gate_self_teacher_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let student = seed_user(&pool, &prefix, "PairStudent", "student").await;
    let teacher = seed_user(&pool, &prefix, "PairTeacher", "teacher").await;
    let stranger = seed_user(&pool, &prefix, "PairStranger", "student").await;
    let admin = seed_user(&pool, &prefix, "PairAdmin", "admin").await;
    let course_id = seed_course(&pool, &prefix, "pair-course").await;
    enroll(&pool, teacher, course_id, "teacher").await;
    enroll(&pool, student, course_id, "student").await;

    let unknown = server
        .post("/users/999999999/observer_pairing_codes")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(unknown.status_code(), 404);

    let denied = server
        .post(&format!("/users/{student}/observer_pairing_codes"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .await;
    assert_eq!(denied.status_code(), 403, "unrelated users are denied");

    for (who, role) in [(student, "student"), (teacher, "teacher"), (admin, "admin")] {
        let resp = server
            .post(&format!("/users/{student}/observer_pairing_codes"))
            .add_header(axum::http::header::AUTHORIZATION, auth(who, role))
            .await;
        assert_eq!(
            resp.status_code(),
            200,
            "{role} allowed; body: {}",
            resp.text()
        );
        let resp_body: Value = resp.json();
        let code: Value = resp_body["pairing_code"].clone();
        assert_eq!(code["user_id"], student);
        assert_eq!(code["workflow_state"], "active");
        let code_str = code["code"].as_str().expect("code").to_string();
        assert_eq!(code_str.len(), 6, "6-char alphanumeric code");
        assert!(code_str.chars().all(|c| c.is_ascii_alphanumeric()));
        assert!(code["expires_at"].as_str().is_some(), "expiry present");
    }

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// avatars
// ---------------------------------------------------------------------------

#[tokio::test]
async fn avatars_profile_pics_self_or_admin() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "AvUser", "student").await;
    let other = seed_user(&pool, &prefix, "AvOther", "student").await;
    let admin = seed_user(&pool, &prefix, "AvAdmin", "admin").await;

    let unknown = server
        .get("/users/999999999/avatars")
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .await;
    assert_eq!(unknown.status_code(), 404);

    let denied = server
        .get(&format!("/users/{user}/avatars"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(denied.status_code(), 403);

    for (who, role) in [(user, "student"), (admin, "admin")] {
        let resp = server
            .get(&format!("/users/{user}/avatars"))
            .add_header(axum::http::header::AUTHORIZATION, auth(who, role))
            .await;
        assert_eq!(resp.status_code(), 200, "{role} allowed");
        let avatars: Vec<Value> = resp.json();
        assert_eq!(avatars.len(), 1);
        assert_eq!(avatars[0]["type"], "gravatar");
        let token = avatars[0]["token"].as_str().expect("token");
        assert_eq!(token.len(), 16, "deterministic hex token");
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    cleanup(&pool, &prefix).await;
}
