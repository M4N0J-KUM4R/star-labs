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
    let _ = sqlx::query("DELETE FROM attachments WHERE media_entry_id LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query("DELETE FROM media_objects WHERE media_id LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM group_memberships WHERE group_id IN
         (SELECT id FROM groups WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM groups WHERE name LIKE $1")
        .bind(format!("{prefix}-%"))
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "DELETE FROM enrollments WHERE course_id IN
         (SELECT id FROM courses WHERE name LIKE $1)",
    )
    .bind(format!("{prefix}-%"))
    .execute(pool)
    .await;
    let _ = sqlx::query("DELETE FROM courses WHERE name LIKE $1")
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
// Issue #65: context-scoped index, media_objects variants, track batch
// update and ASR
// ---------------------------------------------------------------------------

#[tokio::test]
async fn media_object_create_with_context_and_owner_only_update() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let admin = seed_user(&pool, &prefix, "Admin", "admin").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let course: i64 =
        sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix}-Course"))
            .bind(format!("{prefix}-C"))
            .fetch_one(&pool)
            .await
            .expect("seed course");
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student')")
        .bind(owner)
        .bind(course)
        .execute(&pool)
        .await
        .expect("seed enrollment");

    // POST /media_objects without a context behaves like /media_attachments.
    let created = server
        .post("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "media_id": format!("{prefix}-m1"), "media_type": "video", "title": "Lecture" }))
        .await;
    assert_eq!(created.status_code(), 201, "body: {}", created.text());
    let created_body: Value = created.json();
    let media_id = created_body["id"].as_i64().expect("media object id");

    // POST /media_objects with an enrolled course context is accepted.
    let contextual = server
        .post("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({
            "media_id": format!("{prefix}-m2"),
            "media_type": "video",
            "title": "Seminar",
            "context_code": format!("course_{course}")
        }))
        .await;
    assert_eq!(contextual.status_code(), 201, "body: {}", contextual.text());

    // Unknown course in the context code: 404.
    let unknown = server
        .post("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({
            "media_id": format!("{prefix}-m3"),
            "context_code": "course_99999999"
        }))
        .await;
    assert_eq!(unknown.status_code(), 404);

    // Caller without read access to the context: 403.
    let denied = server
        .post("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .json(&json!({
            "media_id": format!("{prefix}-m4"),
            "context_code": format!("course_{course}")
        }))
        .await;
    assert_eq!(denied.status_code(), 403);

    // PUT /media_objects/{id}: Canvas updates the title (blank -> 400 JSON
    // message) and the response omits sources/tracks.
    let renamed = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "user_entered_title": "Renamed Lecture" }))
        .await;
    assert_eq!(renamed.status_code(), 200, "body: {}", renamed.text());
    let renamed_body: Value = renamed.json();
    assert_eq!(
        renamed_body["user_entered_title"].as_str(),
        Some("Renamed Lecture")
    );
    assert!(renamed_body.get("media_tracks").is_none());
    assert!(renamed_body.get("media_sources").is_none());

    let blank = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "user_entered_title": "  " }))
        .await;
    assert_eq!(blank.status_code(), 400);
    assert!(blank.text().contains("user_entered_title"));

    // Canvas' media-object variant is strictly owner-only: even admins 403.
    let admin_denied = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(admin, "admin"))
        .json(&json!({ "user_entered_title": "Nope" }))
        .await;
    assert_eq!(admin_denied.status_code(), 403);

    let stranger_denied = server
        .put(&format!("/media_objects/{media_id}"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .json(&json!({ "user_entered_title": "Nope" }))
        .await;
    assert_eq!(stranger_denied.status_code(), 403);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn context_scoped_listing_follows_media_attachments() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let outsider = seed_user(&pool, &prefix, "Outsider", "student").await;
    let course: i64 =
        sqlx::query_scalar("INSERT INTO courses (name, course_code) VALUES ($1, $2) RETURNING id")
            .bind(format!("{prefix}-Course"))
            .bind(format!("{prefix}-C"))
            .fetch_one(&pool)
            .await
            .expect("seed course");
    sqlx::query("INSERT INTO enrollments (user_id, course_id, role) VALUES ($1, $2, 'student')")
        .bind(owner)
        .bind(course)
        .execute(&pool)
        .await
        .expect("seed enrollment");

    // Two media objects for the owner; only the first is linked to the course
    // through a video attachment.
    let linked = create_ok(
        &server,
        &auth(owner, "student"),
        &format!("{prefix}-linked"),
        "Course Video",
    )
    .await;
    create_ok(
        &server,
        &auth(owner, "student"),
        &format!("{prefix}-private"),
        "Private Video",
    )
    .await;
    sqlx::query(
        "INSERT INTO attachments (context_id, context_type, media_entry_id, content_type) \
         VALUES ($1, 'Course', $2, 'video/mp4')",
    )
    .bind(course)
    .bind(format!("{prefix}-linked"))
    .execute(&pool)
    .await
    .expect("seed attachment");

    // A non-media attachment and a deleted one never surface.
    sqlx::query(
        "INSERT INTO attachments (context_id, context_type, media_entry_id, content_type) \
         VALUES ($1, 'Course', $2, 'application/pdf')",
    )
    .bind(course)
    .bind(format!("{prefix}-private"))
    .execute(&pool)
    .await
    .expect("seed pdf attachment");

    for path in [
        format!("/courses/{course}/media_objects"),
        format!("/courses/{course}/media_attachments"),
    ] {
        let listed = server
            .get(&path)
            .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
            .await;
        assert_eq!(listed.status_code(), 200, "body: {}", listed.text());
        let rows: Vec<Value> = listed.json();
        assert_eq!(rows.len(), 1, "only the linked media object appears");
        assert_eq!(rows[0]["id"].as_i64(), linked["id"].as_i64());
    }

    // The unscoped user listing still shows both of the caller's objects.
    let mine = server
        .get("/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(mine.status_code(), 200);
    let mine_rows: Vec<Value> = mine.json();
    assert_eq!(mine_rows.len(), 2);

    // Not enrolled in the course: 403 (Canvas render_unauthorized_action).
    let denied = server
        .get(&format!("/courses/{course}/media_objects"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(denied.status_code(), 403);

    // Unknown course: 404.
    let missing = server
        .get("/courses/99999999/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(missing.status_code(), 404);

    // Group context: member sees the linked object; non-members 403.
    let group: i64 = sqlx::query_scalar("INSERT INTO groups (name) VALUES ($1) RETURNING id")
        .bind(format!("{prefix}-Group"))
        .fetch_one(&pool)
        .await
        .expect("seed group");
    sqlx::query("INSERT INTO group_memberships (group_id, user_id) VALUES ($1, $2)")
        .bind(group)
        .bind(owner)
        .execute(&pool)
        .await
        .expect("seed membership");
    sqlx::query(
        "INSERT INTO attachments (context_id, context_type, media_entry_id, content_type) \
         VALUES ($1, 'Group', $2, 'audio/ogg')",
    )
    .bind(group)
    .bind(format!("{prefix}-linked"))
    .execute(&pool)
    .await
    .expect("seed group attachment");

    let group_listed = server
        .get(&format!("/groups/{group}/media_objects"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(
        group_listed.status_code(),
        200,
        "body: {}",
        group_listed.text()
    );
    let group_rows: Vec<Value> = group_listed.json();
    assert_eq!(group_rows.len(), 1);

    let group_denied = server
        .get(&format!("/groups/{group}/media_attachments"))
        .add_header(axum::http::header::AUTHORIZATION, auth(outsider, "student"))
        .await;
    assert_eq!(group_denied.status_code(), 403);

    let group_missing = server
        .get("/groups/99999999/media_objects")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(group_missing.status_code(), 404);

    cleanup(&pool, &prefix).await;
}

#[tokio::test]
async fn media_tracks_batch_update_and_asr_requests() {
    let Some((server, pool, prefix)) = setup().await else {
        println!("SKIP: DATABASE_URL not set");
        return;
    };
    let owner = seed_user(&pool, &prefix, "Owner", "student").await;
    let stranger = seed_user(&pool, &prefix, "Stranger", "student").await;
    let object = create_ok(
        &server,
        &auth(owner, "student"),
        &format!("{prefix}-m1"),
        "Lecture",
    )
    .await;
    let id = object["id"].as_i64().expect("media object id");
    seed_track(&pool, id, owner, "en", "hello").await;
    seed_track(&pool, id, owner, "es", "hola").await;

    // Batch update: keep en with new content, add fr; es is dropped because
    // its locale is missing; the blank-content entry is left alone (it refers
    // to an unedited client row and creates nothing).
    let updated = server
        .put(&format!("/media_objects/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!([
            { "locale": "en", "content": "hello (v2)" },
            { "locale": "fr", "content": "bonjour" },
            { "locale": "de" }
        ]))
        .await;
    assert_eq!(updated.status_code(), 200, "body: {}", updated.text());
    let rows: Vec<Value> = updated.json();
    let locales: Vec<&str> = rows.iter().filter_map(|r| r["locale"].as_str()).collect();
    assert!(locales.contains(&"en"));
    assert!(locales.contains(&"fr"));
    assert!(!locales.contains(&"es"), "missing locale is deleted");
    assert!(!locales.contains(&"de"), "blank content creates nothing");

    let (content,): (String,) = sqlx::query_as(
        "SELECT content FROM media_tracks WHERE media_object_id = $1 AND locale = 'en'",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("fetch en track");
    assert_eq!(content, "hello (v2)");

    // The attachment-keyed twin lists the same tracks.
    let twin = server
        .get(&format!(
            "/media_attachments/{id}/media_tracks?include[]=content"
        ))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .await;
    assert_eq!(twin.status_code(), 200, "body: {}", twin.text());
    let twin_rows: Vec<Value> = twin.json();
    assert_eq!(twin_rows.len(), 2);

    // Non-owners cannot rewrite the set.
    let denied = server
        .put(&format!("/media_attachments/{id}/media_tracks"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .json(&json!([{ "locale": "en", "content": "hijack" }]))
        .await;
    assert_eq!(denied.status_code(), 403);

    // ASR: a valid locale creates the pending track (asr marker), and the
    // response is the media object json with the new track embedded.
    let asr = server
        .post(&format!("/media_objects/{id}/asr"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "locale": "en-US" }))
        .await;
    assert_eq!(asr.status_code(), 200, "body: {}", asr.text());
    let asr_body: Value = asr.json();
    assert_eq!(asr_body["id"].as_i64(), Some(id));
    let track = &asr_body["media_tracks"][0];
    assert_eq!(track["locale"].as_str(), Some("en-US"));
    assert_eq!(track["kind"].as_str(), Some("subtitles"));
    assert_eq!(track["asr"].as_bool(), Some(true));

    // An invalid locale is a 400 with the Canvas error body.
    let invalid = server
        .post(&format!("/media_attachments/{id}/asr"))
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "locale": "en_US" }))
        .await;
    assert_eq!(invalid.status_code(), 400);
    assert!(invalid.text().contains("Invalid or missing locale"));

    // Non-owners cannot request captions.
    let denied_asr = server
        .post(&format!("/media_objects/{id}/asr"))
        .add_header(axum::http::header::AUTHORIZATION, auth(stranger, "student"))
        .json(&json!({ "locale": "fr" }))
        .await;
    assert_eq!(denied_asr.status_code(), 403);

    // Unknown media object: 404.
    let missing = server
        .post("/media_objects/99999999/asr")
        .add_header(axum::http::header::AUTHORIZATION, auth(owner, "student"))
        .json(&json!({ "locale": "en" }))
        .await;
    assert_eq!(missing.status_code(), 404);

    cleanup(&pool, &prefix).await;
}
