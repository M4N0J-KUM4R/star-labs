//! Integration tests for the Phase 4 favorites + release notes routes of
//! star-dashboard.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Unique per-run prefix + full teardown, safe against a shared database.
//!
//! Behavior pinned here (including the deviations from Canvas):
//! - `GET /users/self/favorites/courses` returns the user's favorited enrolled
//!   courses, or **every** enrolled course when the user has no favorites
//!   (Canvas `menu_courses`). Only id/name/course_code/workflow_state are
//!   returned, not the full Canvas course_json.
//! - `POST` is idempotent (`create_or_find_by`) and 404s on an unknown course.
//! - `DELETE /{id}` returns the Favorite object, or `{}` (200, never 404) when
//!   the favorite was not set; `DELETE /courses` returns `{}`.
//! - `POST/PUT/DELETE /release_notes*` require the `admin` role (403
//!   otherwise); `latest` and `unread_count` also serve anonymous callers.
//! - `latest` only lists published notes whose show_at already passed, and
//!   `new` is false for anonymous / never-acknowledged users.
//! - Only the `en` locale text is stored/served by this API.

use axum_test::TestServer;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

/// Unique-per-run prefix so repeat runs against a shared DB never collide.
fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "favrn-tests-{}-{}",
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

    let server = TestServer::new(dashboard_router(state)).expect("build TestServer");
    let prefix = run_prefix();
    Some((server, pool, prefix))
}

fn dashboard_router(state: AppState) -> axum::Router {
    axum::Router::new()
        .merge(star_dashboard::router())
        .with_state(state)
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
    .bind(star_auth::hash_password("pw-test-1234").expect("hash"))
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

fn auth(user_id: i64, role: &str) -> String {
    format!(
        "Bearer {}",
        star_auth::create_token(user_id, role, "test-jwt-secret").expect("mint token")
    )
}

/// Delete every row created by this run (users cascade to enrollments and
/// favorites; release notes have no owner FK, so match the title prefix).
async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query("DELETE FROM release_notes WHERE title LIKE $1")
        .bind(format!("{prefix}%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM favorites WHERE user_id IN (SELECT id FROM users WHERE email LIKE $1)",
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

// ---------------------------------------------------------------------------
// Favorites
// ---------------------------------------------------------------------------

#[tokio::test]
async fn favorite_course_lifecycle_matches_canvas() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let user = seed_user(&pool, &prefix, "Faver", "student").await;
    let alpha = seed_course(&pool, &prefix, "FavAlpha").await;
    let beta = seed_course(&pool, &prefix, "FavBeta").await;
    seed_enrollment(&pool, user, alpha, "student").await;
    seed_enrollment(&pool, user, beta, "student").await;
    let token = auth(user, "student");

    // No favorites yet -> Canvas falls back to every enrolled course.
    let resp = server
        .get("/users/self/favorites/courses")
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let courses: Value = resp.json();
    let ids: Vec<i64> = courses
        .as_array()
        .expect("courses array")
        .iter()
        .map(|c| c["id"].as_i64().expect("id"))
        .collect();
    assert_eq!(ids, vec![alpha, beta], "default list: {courses}");
    assert_eq!(courses[0]["name"], "FavAlpha");
    assert_eq!(courses[0]["course_code"], format!("{prefix}-FavAlpha"));

    // POST adds a favorite and returns the Favorite object.
    let resp = server
        .post(&format!("/users/self/favorites/courses/{alpha}"))
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let favorite: Value = resp.json();
    assert_eq!(favorite["context_id"], alpha);
    assert_eq!(favorite["context_type"], "Course");
    assert!(favorite["id"].as_i64().is_some(), "favorite: {favorite}");

    // Re-posting is idempotent: same Favorite id, still one row.
    let resp = server
        .post(&format!("/users/self/favorites/courses/{alpha}"))
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let again: Value = resp.json();
    assert_eq!(again["id"], favorite["id"], "idempotent POST");

    // With a favorite set, only favorited courses are listed.
    let resp = server
        .get("/users/self/favorites/courses")
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    let courses: Value = resp.json();
    assert_eq!(courses.as_array().expect("courses").len(), 1);
    assert_eq!(courses[0]["id"], alpha);

    // DELETE returns the removed Favorite...
    let resp = server
        .delete(&format!("/users/self/favorites/courses/{alpha}"))
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let removed: Value = resp.json();
    assert_eq!(removed["context_id"], alpha);
    assert_eq!(removed["context_type"], "Course");

    // ...and deleting an unset favorite returns `{}` with 200 (never 404).
    let resp = server
        .delete(&format!("/users/self/favorites/courses/{alpha}"))
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({}));

    // DELETE /courses resets the whole list back to `{}` + all enrollments.
    let _ = server
        .post(&format!("/users/self/favorites/courses/{beta}"))
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    let resp = server
        .delete("/users/self/favorites/courses")
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({}));

    let resp = server
        .get("/users/self/favorites/courses")
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    let courses: Value = resp.json();
    assert_eq!(courses.as_array().expect("courses").len(), 2, "{courses}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn favorites_require_auth_and_reject_unknown_courses() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    // Anonymous callers are rejected with 401 on every favorites route.
    let resp = server.get("/users/self/favorites/courses").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server.post("/users/self/favorites/courses/1").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server.delete("/users/self/favorites/courses").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    let user = seed_user(&pool, &prefix, "NoCourse", "student").await;
    let mine = seed_course(&pool, &prefix, "MineCourse").await;
    let other = seed_course(&pool, &prefix, "OtherCourse").await;
    seed_enrollment(&pool, user, mine, "student").await;
    let token = auth(user, "student");

    // Unknown course -> 404 (Canvas `api_find(Course, id)`).
    let resp = server
        .post("/users/self/favorites/courses/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());
    let resp = server
        .delete("/users/self/favorites/courses/999999999")
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    // Unenrolled courses never appear in the list.
    let resp = server
        .get("/users/self/favorites/courses")
        .add_header(axum::http::header::AUTHORIZATION, &token)
        .await;
    let courses: Value = resp.json();
    assert_eq!(courses.as_array().expect("courses").len(), 1, "{courses}");
    assert_eq!(courses[0]["id"], mine);
    assert!(courses
        .as_array()
        .expect("courses")
        .iter()
        .all(|c| c["id"] != other));

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Release notes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn release_notes_admin_crud_latest_and_unread_count() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "NoteAdmin", "admin").await;
    // Notes are targeted at a role unique to this run so the global
    // unread_count is not polluted by rows from other runs.
    let reader_role = format!("{prefix}-reader");
    let student = seed_user(&pool, &prefix, "NoteStudent", &reader_role).await;
    let admin_token = auth(admin, "admin");
    let student_token = auth(student, &reader_role);

    let shown_at = (Utc::now() - Duration::days(1)).to_rfc3339();
    let future_at = (Utc::now() + Duration::days(2)).to_rfc3339();

    // Non-admins are forbidden from the management endpoints.
    let resp = server
        .post("/release_notes")
        .add_header(axum::http::header::AUTHORIZATION, &student_token)
        .json(&json!({ "title": "nope" }))
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());
    let resp = server
        .get("/release_notes")
        .add_header(axum::http::header::AUTHORIZATION, &student_token)
        .await;
    assert_eq!(resp.status_code(), 403, "body: {}", resp.text());

    // Create a published, already-shown note.
    let resp = server
        .post("/release_notes")
        .add_header(axum::http::header::AUTHORIZATION, &admin_token)
        .json(&json!({
            "published": true,
            "target_roles": [reader_role],
            "show_ats": { "production": shown_at },
            "langs": { "en": { "title": format!("{prefix}-NoteOne"), "description": "first", "url": "/notes/1" } }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let note: Value = resp.json();
    let note_id = note["id"].as_i64().expect("note id");
    assert_eq!(note["published"], true);
    assert_eq!(note["target_roles"], json!([reader_role]));
    let returned_at = note["show_ats"]["production"]
        .as_str()
        .expect("production show_at");
    assert_eq!(
        DateTime::parse_from_rfc3339(returned_at)
            .expect("returned show_at")
            .timestamp_micros(),
        DateTime::parse_from_rfc3339(&shown_at)
            .expect("expected show_at")
            .timestamp_micros()
    );
    assert_eq!(note["langs"]["en"]["title"], format!("{prefix}-NoteOne"));
    assert_eq!(note["langs"]["en"]["url"], "/notes/1");

    // index hides `langs` unless include_langs=true.
    let resp = server
        .get("/release_notes")
        .add_header(axum::http::header::AUTHORIZATION, &admin_token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let first = listed
        .as_array()
        .expect("index array")
        .iter()
        .find(|n| n["id"] == note_id)
        .expect("note in index")
        .clone();
    assert!(first.get("langs").is_none(), "langs leaked: {first}");

    let resp = server
        .get("/release_notes?include_langs=true")
        .add_header(axum::http::header::AUTHORIZATION, &admin_token)
        .await;
    let listed: Value = resp.json();
    let with_langs = listed
        .as_array()
        .expect("index array")
        .iter()
        .find(|n| n["id"] == note_id)
        .expect("note in index")
        .clone();
    assert_eq!(
        with_langs["langs"]["en"]["title"],
        format!("{prefix}-NoteOne")
    );

    // update changes the title and keeps the rest.
    let resp = server
        .put(&format!("/release_notes/{note_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &admin_token)
        .json(&json!({ "langs": { "en": { "title": format!("{prefix}-NoteOneRenamed") } } }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(
        updated["langs"]["en"]["title"],
        format!("{prefix}-NoteOneRenamed")
    );
    assert_eq!(updated["langs"]["en"]["description"], "first");

    // latest lists the shown note as not-new for a user that never acked it.
    let resp = server
        .get("/release_notes/latest")
        .add_header(axum::http::header::AUTHORIZATION, &student_token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let latest: Value = resp.json();
    let entry = latest
        .as_array()
        .expect("latest array")
        .iter()
        .find(|n| n["id"] == note_id)
        .expect("note in latest")
        .clone();
    assert_eq!(entry["title"], format!("{prefix}-NoteOneRenamed"));
    assert_eq!(entry["new"], false);
    assert_eq!(entry["url"], "/notes/1");
    assert!(entry["date"].as_str().is_some());

    // A future note is not "latest" yet, but counts as unread.
    let resp = server
        .post("/release_notes")
        .add_header(axum::http::header::AUTHORIZATION, &admin_token)
        .json(&json!({
            "published": true,
            "target_roles": [reader_role],
            "show_ats": { "production": future_at },
            "langs": { "en": { "title": format!("{prefix}-NoteFuture") } }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let future_id = resp.json::<Value>()["id"].as_i64().expect("future id");

    let resp = server
        .get("/release_notes/latest")
        .add_header(axum::http::header::AUTHORIZATION, &student_token)
        .await;
    let latest: Value = resp.json();
    assert!(latest
        .as_array()
        .expect("latest array")
        .iter()
        .all(|n| n["id"] != future_id));

    let resp = server
        .get("/release_notes/unread_count")
        .add_header(axum::http::header::AUTHORIZATION, &student_token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let unread: Value = resp.json();
    assert_eq!(unread["unread_count"], json!(1), "unread: {unread}");

    // destroy returns {status: "ok"} then 404 on the second call.
    let resp = server
        .delete(&format!("/release_notes/{note_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &admin_token)
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>(), json!({ "status": "ok" }));
    let resp = server
        .delete(&format!("/release_notes/{note_id}"))
        .add_header(axum::http::header::AUTHORIZATION, &admin_token)
        .await;
    assert_eq!(resp.status_code(), 404, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn release_note_reads_allow_anonymous_but_management_does_not() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };

    let admin = seed_user(&pool, &prefix, "AnonAdmin", "admin").await;
    let admin_token = auth(admin, "admin");
    let shown_at = (Utc::now() - Duration::hours(1)).to_rfc3339();

    let resp = server
        .post("/release_notes")
        .add_header(axum::http::header::AUTHORIZATION, &admin_token)
        .json(&json!({
            "published": true,
            "show_ats": { "production": shown_at },
            "langs": { "en": { "title": format!("{prefix}-AnonNote") } }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let note_id = resp.json::<Value>()["id"].as_i64().expect("note id");

    // Anonymous readers get the note with new=false (Canvas treats anonymous
    // users as having already seen everything).
    let resp = server.get("/release_notes/latest").await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let latest: Value = resp.json();
    let entry = latest
        .as_array()
        .expect("latest array")
        .iter()
        .find(|n| n["id"] == note_id)
        .expect("note in latest")
        .clone();
    assert_eq!(entry["new"], false);
    assert_eq!(entry["title"], format!("{prefix}-AnonNote"));

    // Anonymous last_seen is "now", so already-shown notes are not unread.
    // (Only the shape is pinned: the count is global across roles.)
    let resp = server.get("/release_notes/unread_count").await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let unread: Value = resp.json();
    assert!(
        unread.get("unread_count").and_then(Value::as_i64).is_some(),
        "unread_count shape: {unread}"
    );

    // Unpublished notes are invisible to readers.
    let resp = server
        .post("/release_notes")
        .add_header(axum::http::header::AUTHORIZATION, &admin_token)
        .json(&json!({
            "published": false,
            "show_ats": { "production": shown_at },
            "langs": { "en": { "title": format!("{prefix}-DraftNote") } }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let draft_id = resp.json::<Value>()["id"].as_i64().expect("draft id");

    let resp = server.get("/release_notes/latest").await;
    let latest: Value = resp.json();
    assert!(latest
        .as_array()
        .expect("latest array")
        .iter()
        .all(|n| n["id"] != draft_id));

    // Management endpoints still require a token (401, not 403).
    let resp = server.get("/release_notes").await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());
    let resp = server
        .put("/release_notes/1")
        .json(&json!({ "published": true }))
        .await;
    assert_eq!(resp.status_code(), 401, "body: {}", resp.text());

    cleanup(&pool, &prefix).await;
}
