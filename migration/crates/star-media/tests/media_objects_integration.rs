//! Integration tests for star-media (`media_objects_api` parity).
//!
//! Pinned behavior:
//! - `POST /media_attachments` is metadata only, answers 201 with the
//!   `MediaObject` shape (`can_add_captions`, `media_sources: []`,
//!   `media_tracks: []`, `viewer_restrictions: {}`); a blank `media_id` is a
//!   422 validation error. Re-posting the same `media_id` upserts (200 vs 201
//!   is not distinguished by Canvas `first_or_initialize`).
//! - `GET /media_attachments` returns only the caller's objects, sorted by
//!   `COALESCE(user_entered_title, title)` ascending unless `sort`/`order` say
//!   otherwise; anonymous callers get 401.
//! - `PUT /media_attachments/{id}` requires the owner (or a global admin, 403
//!   otherwise), truncates to 255 chars, and answers 400 with
//!   `{"message":"The user_entered_title parameter must have a value"}` when
//!   the title is missing/blank (Canvas `update_media_object`).
//! - `GET /media_objects/{id}/media_tracks` is owner/admin only (403 for other
//!   users, 404 for an unknown media object). Default fields mirror
//!   `media_track_api_json`; `include[]=content` adds the stored SRT/VTT body.
//!
//! Skipped cleanly when DATABASE_URL is not set; unique per-run prefix plus
//! full teardown keeps shared-DB runs safe.

use axum_test::TestServer;
use serde_json::{json, Value};
use sqlx::PgPool;
use star_common::AppState;

const JWT_SECRET: &str = "test-jwt-secret";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn run_prefix() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    format!(
        "media_{}_{}",
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

    let server = TestServer::new(star_media::router().with_state(state)).expect("build TestServer");
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

async fn seed_track(
    pool: &PgPool,
    media_object_id: i64,
    user_id: i64,
    locale: &str,
    content: &str,
) {
    sqlx::query(
        r#"
        INSERT INTO media_tracks (media_object_id, user_id, kind, locale, content)
        VALUES ($1, $2, 'subtitles', $3, $4)
        "#,
    )
    .bind(media_object_id)
    .bind(user_id)
    .bind(locale)
    .bind(content)
    .execute(pool)
    .await
    .expect("seed track");
}

fn auth(user_id: i64, role: &str) -> String {
    let token = star_auth::create_token(user_id, role, JWT_SECRET).expect("mint token");
    format!("Bearer {token}")
}

async fn create_ok(server: &TestServer, bearer: &str, media_id: &str, title: &str) -> Value {
    let resp = server
        .post("/media_attachments")
        .add_header(axum::http::header::AUTHORIZATION, bearer.to_string())
        .json(&json!({ "media_id": media_id, "media_type": "video", "title": title }))
        .await;
    assert_eq!(resp.status_code(), 201, "body: {}", resp.text());
    resp.json()
}

async fn cleanup(pool: &PgPool, prefix: &str) {
    let _ = sqlx::query(
        "DELETE FROM media_tracks WHERE media_object_id IN
         (SELECT id FROM media_objects WHERE media_id LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM media_objects WHERE media_id LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_list_update_round_trip() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let bearer = auth(owner, "student");

    // Create returns the Canvas MediaObject shape.
    let created = create_ok(
        &server,
        &bearer,
        &format!("{prefix}-m-1"),
        &format!("{prefix}-Recording"),
    )
    .await;
    assert_eq!(created["title"], format!("{prefix}-Recording"));
    assert_eq!(created["media_type"], "video");
    assert_eq!(created["can_add_captions"], true);
    assert_eq!(created["media_sources"], json!([]));
    assert_eq!(created["media_tracks"], json!([]));
    assert_eq!(created["viewer_restrictions"], json!({}));
    let id = created["id"].as_i64().unwrap();

    // A blank media_id is a validation error (422).
    let resp = server
        .post("/media_attachments")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "media_id": "  " }))
        .await;
    assert_eq!(resp.status_code(), 422);

    // Anonymous listing is rejected by the auth extractor (401).
    let resp = server.get("/media_attachments").await;
    assert_eq!(resp.status_code(), 401);

    // Listing shows the object; title falls back to `title` until overridden.
    let resp = server
        .get("/media_attachments")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], id);

    // PUT without a title is Canvas' 400 with a JSON message.
    let resp = server
        .put(&format!("/media_attachments/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "The user_entered_title parameter must have a value"
    );

    // PUT with a title updates the response title.
    let resp = server
        .put(&format!("/media_attachments/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "user_entered_title": "Edited title" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["user_entered_title"], "Edited title");
    assert_eq!(updated["title"], "Edited title");

    // Unknown ids are 404.
    let resp = server
        .put("/media_attachments/999999999")
        .add_header(axum::http::header::AUTHORIZATION, bearer.clone())
        .json(&json!({ "user_entered_title": "x" }))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn update_requires_owner_and_listing_is_scoped() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;

    let created = create_ok(
        &server,
        &auth(owner, "student"),
        &format!("{prefix}-m-owned"),
        "Owned",
    )
    .await;
    let id = created["id"].as_i64().unwrap();

    // Another user cannot update it.
    let resp = server
        .put(&format!("/media_attachments/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "user_entered_title": "hijack" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // They also do not see it in their own listing.
    let resp = server
        .get("/media_attachments")
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    assert_eq!(resp.json::<Value>(), json!([]));

    // Global admins pass the owner check.
    let resp = server
        .put(&format!("/media_attachments/{id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "user_entered_title": "admin edit" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    assert_eq!(resp.json::<Value>()["title"], "admin edit");

    // `order=desc` reverses the title sort.
    for (suffix, title) in [("m-a", "AAA"), ("m-z", "ZZZ")] {
        create_ok(
            &server,
            &auth(owner, "student"),
            &format!("{prefix}-{suffix}"),
            &format!("{prefix}-{title}"),
        )
        .await;
    }
    let resp = server
        .get("/media_attachments?sort=title&order=desc")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    let titles: Vec<String> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value["title"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(titles.len(), 3);
    let zzz = titles
        .iter()
        .position(|title| title.ends_with("-ZZZ"))
        .unwrap();
    let aaa = titles
        .iter()
        .position(|title| title.ends_with("-AAA"))
        .unwrap();
    assert!(zzz < aaa, "desc should put ZZZ before AAA: {titles:?}");

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn media_tracks_listing_and_include_content() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;

    let created = create_ok(
        &server,
        &auth(owner, "student"),
        &format!("{prefix}-m-tracks"),
        "Captioned",
    )
    .await;
    let id = created["id"].as_i64().unwrap();
    seed_track(
        &pool,
        id,
        owner,
        "es",
        "1\n00:00:00,000 --> 00:00:01,000\nHola",
    )
    .await;

    let url = format!("/media_objects/{id}/media_tracks");

    // Default fields mirror media_track_api_json (no content).
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tracks: Value = resp.json();
    assert_eq!(tracks.as_array().unwrap().len(), 1);
    assert_eq!(tracks[0]["locale"], "es");
    assert_eq!(tracks[0]["kind"], "subtitles");
    assert_eq!(tracks[0]["asr"], false);
    assert_eq!(tracks[0]["media_object_id"], id);
    assert!(tracks[0].get("content").is_none());

    // include[]=content adds the stored body.
    let resp = server
        .get(&format!("{url}?include%5B%5D=content"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let tracks: Value = resp.json();
    assert_eq!(
        tracks[0]["content"],
        "1\n00:00:00,000 --> 00:00:01,000\nHola"
    );

    // Non-owners get 403; unknown media objects get 404.
    let resp = server
        .get(&url)
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    let resp = server
        .get("/media_objects/999999999/media_tracks")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

// ---------------------------------------------------------------------------
// Canvas API v1 media object routes (`src/media_objects.rs`)
//
// Ground truth: `canvas-src/app/controllers/media_objects_controller.rb`,
// `canvas-src/lib/api/v1/media_object.rb` and
// `canvas-src/spec/controllers/media_objects_controller_spec.rb`.
//
// - `GET /media_objects` lists the media objects of the caller and answers
//   with the `media_object_api_json` shape: `media_id`, `created_at`,
//   `media_type`, `title`, `can_add_captions`, `media_sources`,
//   `embedded_iframe_url`, `auto_caption_status`, `viewer_restrictions` and
//   `media_tracks` — and nothing else (no `id`, no `user_entered_title`).
// - `exclude[]=sources` drops `media_sources`, `exclude[]=tracks` drops
//   `media_tracks`.
// - `GET /courses/{id}/media_objects` and `GET /groups/{id}/media_attachments`
//   are the same action: the scoped listing. An unknown course/group is a
//   `404`, a caller without read access is a `403`.
// - Pagination follows Canvas: `per_page` defaults to 10 (max 100) and the
//   response carries a `Link` header with `current`/`next`/`prev`/`first`/`last`.
// - `sort` is `title` (default) or `created_at`, `order` is `asc` (default) or
//   `desc`.
// - `PUT /media_objects/{media_object_id}` — `media_object_id` is the Kaltura
//   entry id, not the database id. Only the owner may update (403), a blank
//   `user_entered_title` is a `400` with Canvas' message, and the answer
//   excludes `media_sources` and `media_tracks`.
// - `POST /media_objects` is context scoped (`context_code=course_17`): an
//   unknown context is a `404`, a caller without read access is a `403`, and
//   the answer is `{"media_object": {...}, "embedded_iframe_url": "..."}`.
// ---------------------------------------------------------------------------

async fn api_seed_account(pool: &PgPool, prefix: &str, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO accounts (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .fetch_one(pool)
        .await
        .expect("seed account")
}

async fn api_seed_account_user(pool: &PgPool, account_id: i64, user_id: i64) {
    sqlx::query("INSERT INTO account_users (account_id, user_id, role) VALUES ($1, $2, 'admin')")
        .bind(account_id)
        .bind(user_id)
        .execute(pool)
        .await
        .expect("seed account user");
}

async fn api_seed_course(pool: &PgPool, prefix: &str, name: &str, account_id: i64) -> i64 {
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

async fn api_seed_enrollment(pool: &PgPool, user_id: i64, course_id: i64, role: &str) {
    sqlx::query(
        "INSERT INTO enrollments (user_id, course_id, role, workflow_state)
         VALUES ($1, $2, $3, 'active')",
    )
    .bind(user_id)
    .bind(course_id)
    .bind(role)
    .execute(pool)
    .await
    .expect("seed enrollment");
}

async fn api_seed_group(pool: &PgPool, prefix: &str, name: &str, course_id: i64) -> i64 {
    sqlx::query_scalar("INSERT INTO groups (name, course_id) VALUES ($1, $2) RETURNING id")
        .bind(format!("{prefix}-{name}"))
        .bind(course_id)
        .fetch_one(pool)
        .await
        .expect("seed group")
}

async fn api_seed_group_member(pool: &PgPool, group_id: i64, user_id: i64) {
    sqlx::query(
        "INSERT INTO group_memberships (group_id, user_id, workflow_state)
         VALUES ($1, $2, 'accepted')",
    )
    .bind(group_id)
    .bind(user_id)
    .execute(pool)
    .await
    .expect("seed group member");
}

/// A media object in a context, like Canvas' `MediaObject.create!`.
#[allow(clippy::too_many_arguments)]
async fn api_seed_media_object(
    pool: &PgPool,
    prefix: &str,
    suffix: &str,
    user_id: i64,
    context_type: &str,
    context_id: i64,
    title: Option<&str>,
    user_entered_title: Option<&str>,
    media_type: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        r#"
        INSERT INTO media_objects (media_id, media_type, title, user_entered_title,
                                   user_id, context_type, context_id, viewer_restrictions)
        VALUES ($1, $2, $3, $4, $5, $6, $7, '{}'::jsonb)
        RETURNING id
        "#,
    )
    .bind(format!("{prefix}-{suffix}"))
    .bind(media_type.unwrap_or("video"))
    .bind(title)
    .bind(user_entered_title)
    .bind(user_id)
    .bind(context_type)
    .bind(context_id)
    .fetch_one(pool)
    .await
    .expect("seed media object")
}

async fn api_cleanup(pool: &PgPool, prefix: &str) {
    for statement in [
        "DELETE FROM media_tracks WHERE media_object_id IN
            (SELECT id FROM media_objects WHERE media_id LIKE $1)",
        "DELETE FROM media_objects WHERE media_id LIKE $1",
        "DELETE FROM files WHERE filename LIKE $1",
        "DELETE FROM group_files WHERE filename LIKE $1",
        "DELETE FROM user_files WHERE filename LIKE $1",
        "DELETE FROM group_memberships WHERE group_id IN
            (SELECT id FROM groups WHERE name LIKE $1)",
        "DELETE FROM groups WHERE name LIKE $1",
        "DELETE FROM enrollments WHERE course_id IN
            (SELECT id FROM courses WHERE name LIKE $1)",
        "DELETE FROM courses WHERE name LIKE $1",
        "DELETE FROM account_users WHERE account_id IN
            (SELECT id FROM accounts WHERE name LIKE $1)",
        "DELETE FROM accounts WHERE name LIKE $1",
        "DELETE FROM users WHERE email LIKE $1",
    ] {
        let pattern = format!("{prefix}-%");
        let _ = sqlx::query(statement).bind(&pattern).execute(pool).await;
    }
    let _ = sqlx::query("DELETE FROM users WHERE email LIKE $1")
        .bind(format!("{prefix}%@test.local"))
        .execute(pool)
        .await;
}

/// The keys Canvas' `media_object_api_json` produces.
fn api_canvas_keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn api_global_listing_uses_the_canvas_media_object_shape() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;

    api_seed_media_object(
        &pool,
        &prefix,
        "own",
        owner,
        "User",
        owner,
        Some("The Title"),
        None,
        Some("audio"),
    )
    .await;
    // Media objects of another user are never listed.
    api_seed_media_object(
        &pool,
        &prefix,
        "foreign",
        other,
        "User",
        other,
        Some("Foreign"),
        None,
        Some("video"),
    )
    .await;

    let resp = server
        .get("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let listed = listed.as_array().expect("array");
    assert_eq!(listed.len(), 1, "only the caller's media: {listed:?}");
    let entry = &listed[0];

    assert_eq!(
        api_canvas_keys(entry),
        vec![
            "auto_caption_status",
            "can_add_captions",
            "created_at",
            "embedded_iframe_url",
            "media_id",
            "media_sources",
            "media_tracks",
            "media_type",
            "title",
            "viewer_restrictions",
        ]
    );
    assert_eq!(entry["title"], "The Title");
    assert_eq!(entry["media_type"], "audio");
    assert_eq!(entry["can_add_captions"], true);
    assert_eq!(entry["media_sources"], json!([]));
    assert_eq!(entry["media_tracks"], json!([]));
    assert_eq!(entry["viewer_restrictions"], json!({}));
    assert_eq!(entry["auto_caption_status"], Value::Null);
    assert!(
        entry["embedded_iframe_url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/media_objects_iframe/{prefix}-own")),
        "iframe url: {}",
        entry["embedded_iframe_url"]
    );

    // Anonymous callers are rejected by the auth extractor.
    let resp = server.get("/media_objects").await;
    assert_eq!(resp.status_code(), 401);

    api_cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn api_exclude_drops_media_sources_and_media_tracks() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    api_seed_media_object(
        &pool,
        &prefix,
        "one",
        owner,
        "User",
        owner,
        Some("A"),
        None,
        None,
    )
    .await;

    let resp = server
        .get("/media_objects?exclude%5B%5D=sources&exclude%5B%5D=tracks")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let entry = &listed.as_array().unwrap()[0];
    assert!(entry.get("media_sources").is_none());
    assert!(entry.get("media_tracks").is_none());
    assert_eq!(entry["title"], "A");

    // Only `sources` excluded keeps the tracks.
    let resp = server
        .get("/media_objects?exclude%5B%5D=sources")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    let entry = &listed.as_array().unwrap()[0];
    assert!(entry.get("media_sources").is_none());
    assert!(entry.get("media_tracks").is_some());

    api_cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn api_scoped_listings_check_the_context_and_its_permissions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let account = api_seed_account(&pool, &prefix, "Acct").await;
    let course = api_seed_course(&pool, &prefix, "Course", account).await;
    let group = api_seed_group(&pool, &prefix, "Group", course).await;

    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let student = seed_user(&pool, &prefix, "Student", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;

    api_seed_enrollment(&pool, teacher, course, "teacher").await;
    api_seed_enrollment(&pool, student, course, "student").await;
    api_seed_group_member(&pool, group, student).await;

    api_seed_media_object(
        &pool,
        &prefix,
        "in-course",
        teacher,
        "Course",
        course,
        Some("Course media"),
        None,
        None,
    )
    .await;
    api_seed_media_object(
        &pool,
        &prefix,
        "in-group",
        teacher,
        "Group",
        group,
        Some("Group media"),
        None,
        None,
    )
    .await;

    // The course listing only returns the course's media.
    let resp = server
        .get(&format!("/courses/{course}/media_objects"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["title"], "Course media");

    // `media_attachments` is the same action under a different path.
    let resp = server
        .get(&format!("/courses/{course}/media_attachments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    assert_eq!(listed.as_array().unwrap().len(), 1);

    // The group listing only returns the group's media.
    let resp = server
        .get(&format!("/groups/{group}/media_attachments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["title"], "Group media");

    // A caller without access to the context is a 403.
    let resp = server
        .get(&format!("/courses/{course}/media_objects"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    let resp = server
        .get(&format!("/groups/{group}/media_objects"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(resp.status_code(), 403);

    // An unknown context is a 404.
    let resp = server
        .get("/courses/99999999/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    let resp = server
        .get("/groups/99999999/media_attachments")
        .add_header(axum::http::header::AUTHORIZATION, auth(student, "student"))
        .await;
    assert_eq!(resp.status_code(), 404);

    api_cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn api_listing_paginates_and_sorts_like_canvas() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    for index in 0..12 {
        api_seed_media_object(
            &pool,
            &prefix,
            &format!("m{index}"),
            owner,
            "User",
            owner,
            Some(&format!("Title {index:02}")),
            None,
            None,
        )
        .await;
    }

    // Canvas' default page size is 10.
    let resp = server
        .get("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    assert_eq!(listed.as_array().unwrap().len(), 10);

    let link = resp
        .headers()
        .get("link")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(link.contains("rel=\"next\""), "link: {link}");
    assert!(link.contains("rel=\"last\""), "link: {link}");
    assert!(link.contains("per_page=10"), "link: {link}");
    // The global listing paginates against the `media_attachments` URL, like
    // Canvas' `api_v1_media_attachments_url`.
    assert!(link.contains("/api/v1/media_attachments?"), "link: {link}");

    // `per_page` and `page` walk the collection.
    let resp = server
        .get("/media_objects?per_page=2&page=2&sort=title&order=asc")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0]["title"], "Title 02");
    assert_eq!(listed[1]["title"], "Title 03");

    // `order=desc` reverses the title sort.
    let resp = server
        .get("/media_objects?per_page=3&sort=title&order=desc")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200);
    let listed: Value = resp.json();
    let listed = listed.as_array().unwrap();
    assert_eq!(listed[0]["title"], "Title 11");
    assert_eq!(listed[2]["title"], "Title 09");

    // A search term shorter than two characters is Canvas' 400.
    let resp = server
        .get("/media_objects?search_term=a")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 400);

    // A search term filters on the `title` column.
    let resp = server
        .get("/media_objects?search_term=Title%2007")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["title"], "Title 07");

    api_cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn api_update_media_object_requires_the_owner_and_a_title() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let other = seed_user(&pool, &prefix, "Other", "student").await;

    let media_id = format!("{prefix}-video");
    api_seed_media_object(
        &pool,
        &prefix,
        "video",
        owner,
        "User",
        owner,
        Some("filename.mov"),
        None,
        None,
    )
    .await;

    // The owner may rename the object.
    let resp = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "user_entered_title": "new title" }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(updated["title"], "new title");
    // Canvas answers the update without the excluded sections.
    assert!(updated.get("media_sources").is_none());
    assert!(updated.get("media_tracks").is_none());
    assert_eq!(updated["viewer_restrictions"], json!({}));

    // A blank title is Canvas' 400 with a JSON message.
    let resp = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({}))
        .await;
    assert_eq!(resp.status_code(), 400, "body: {}", resp.text());
    assert_eq!(
        resp.json::<Value>()["message"],
        "The user_entered_title parameter must have a value"
    );

    // Another user may not touch the object.
    let resp = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(other, "student"))
        .json(&json!({ "user_entered_title": "hijack" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // An unknown media id is refused the same way.
    let resp = server
        .put(&format!("/media_objects/{prefix}-unknown"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "user_entered_title": "x" }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Anonymous callers are rejected by the auth extractor.
    let resp = server
        .put(&format!("/media_objects/{media_id}"))
        .json(&json!({ "user_entered_title": "x" }))
        .await;
    assert_eq!(resp.status_code(), 401);

    api_cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn api_update_merges_only_permitted_viewer_restrictions() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let media_id = format!("{prefix}-video");
    api_seed_media_object(
        &pool,
        &prefix,
        "video",
        owner,
        "User",
        owner,
        Some("filename.mov"),
        None,
        None,
    )
    .await;

    // Unknown keys are ignored, known ones are stored.
    let resp = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({
            "user_entered_title": "title",
            "viewer_restrictions": { "show_rolling_transcript": true, "unknown_key": true }
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let updated: Value = resp.json();
    assert_eq!(
        updated["viewer_restrictions"],
        json!({ "show_rolling_transcript": true })
    );

    // The restrictions survive a later update that does not mention them.
    let resp = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "user_entered_title": "another" }))
        .await;
    assert_eq!(resp.status_code(), 200);
    let updated: Value = resp.json();
    assert_eq!(
        updated["viewer_restrictions"],
        json!({ "show_rolling_transcript": true })
    );

    // A non boolean value fails the model validation (422).
    let resp = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({
            "user_entered_title": "title",
            "viewer_restrictions": { "show_rolling_transcript": "yes" }
        }))
        .await;
    assert_eq!(resp.status_code(), 422, "body: {}", resp.text());

    api_cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn api_create_media_object_is_context_scoped() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let account = api_seed_account(&pool, &prefix, "Acct").await;
    let course = api_seed_course(&pool, &prefix, "Course", account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    api_seed_enrollment(&pool, teacher, course, "teacher").await;

    let media_id = format!("{prefix}-new");
    let resp = server
        .post("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "context_code": format!("course_{course}"),
            "id": media_id,
            "type": "audio",
            "title": "My Video"
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    assert!(created.get("media_object").is_some(), "{created:?}");
    assert_eq!(created["media_object"]["media_id"], media_id);
    assert_eq!(created["media_object"]["media_type"], "audio");
    assert_eq!(created["media_object"]["title"], "My Video");
    assert_eq!(created["media_object"]["user_id"], teacher);
    assert_eq!(created["media_object"]["context_type"], "Course");
    assert_eq!(created["media_object"]["context_id"], course);
    assert_eq!(
        created["media_object"]["viewer_restrictions"],
        json!({ "show_rolling_transcript": false })
    );
    assert!(
        created["embedded_iframe_url"]
            .as_str()
            .unwrap()
            .starts_with("http://"),
        "iframe url: {}",
        created["embedded_iframe_url"]
    );

    // The object now belongs to the course.
    let resp = server
        .get(&format!("/courses/{course}/media_objects"))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let listed: Value = resp.json();
    assert_eq!(listed.as_array().unwrap().len(), 1);

    // An unknown context is a 404.
    let resp = server
        .post("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "context_code": "course_99999999",
            "id": format!("{prefix}-nope"),
            "type": "video"
        }))
        .await;
    assert_eq!(resp.status_code(), 404);

    // A caller without read access to the context is a 403.
    let resp = server
        .post("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .json(&json!({
            "context_code": format!("course_{course}"),
            "id": format!("{prefix}-nope2"),
            "type": "video"
        }))
        .await;
    assert_eq!(resp.status_code(), 403);

    // Posting the same media id again updates the existing object.
    let resp = server
        .post("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .json(&json!({
            "context_code": format!("course_{course}"),
            "id": media_id,
            "type": "audio",
            "title": "Renamed"
        }))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["media_object"]["title"], "Renamed");

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM media_objects WHERE media_id = $1")
        .bind(&media_id)
        .fetch_one(&pool)
        .await
        .expect("count media objects");
    assert_eq!(count.0, 1);

    api_cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn api_create_media_object_accepts_query_parameters() {
    let Some((server, pool, prefix)) = setup().await else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let account = api_seed_account(&pool, &prefix, "Acct").await;
    let course = api_seed_course(&pool, &prefix, "Course", account).await;
    let teacher = seed_user(&pool, &prefix, "Teacher", "teacher").await;
    api_seed_enrollment(&pool, teacher, course, "teacher").await;

    let media_id = format!("{prefix}-query");
    let resp = server
        .post(&format!(
            "/media_objects?context_code=course_{course}&id={media_id}&type=video&title=Query%20Title"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(teacher, "teacher"))
        .await;
    assert_eq!(resp.status_code(), 200, "body: {}", resp.text());
    let created: Value = resp.json();
    assert_eq!(created["media_object"]["media_id"], media_id);
    assert_eq!(created["media_object"]["title"], "Query Title");
    assert_eq!(created["media_object"]["media_type"], "video");

    api_cleanup(&pool, &prefix).await;
}
